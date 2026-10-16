//! ZIP と 7z をフォルダとして開く。vw 専用の一時フォルダに展開し、アンマウントするまで保つ。
//!
//! ZIP の中を直接読まずに展開するのは、ツリー・フォルダの読み込み・
//! フォルダの監視がすべてファイルのパスで動いているため。展開すれば ZIP の中も普通の
//! フォルダと同じに扱える。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// 展開したまま残った一時フォルダを消すまでの時間。vw が異常終了すると後片付けできないので、
/// 次に起動したときに、これより古いものを消す。動いている別の vw のものを消さないよう、
/// 作ったばかりのものは残す。
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// 展開した ZIP。
#[derive(Debug, Clone, PartialEq)]
pub struct Mount {
    pub zip: PathBuf,
    /// 展開先。ツリーではこれをルートとして出す。
    pub dir: PathBuf,
}

impl Mount {
    /// ツリーのルートの名前（ZIP のファイル名）。
    pub fn name(&self) -> String {
        self.zip
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.zip.display().to_string())
    }
}

/// 展開が終わった知らせ。
#[derive(Debug)]
pub enum Event {
    Mounted {
        mount: Mount,
        files: usize,
    },
    Failed {
        zip: PathBuf,
        error: String,
    },
    /// 暗号化されていてパスワードが要る。`wrong` なら渡したパスワードでは開けなかった。
    NeedsPassword {
        zip: PathBuf,
        wrong: bool,
    },
}

/// 開けなかった理由。
#[derive(Debug, PartialEq)]
pub enum ArchiveError {
    NeedsPassword,
    WrongPassword,
    Other(String),
}

impl<E: std::fmt::Display> From<E> for ArchiveError {
    fn from(e: E) -> Self {
        ArchiveError::Other(e.to_string())
    }
}

/// [`for_each_entry`] に渡す処理の失敗。中身を読めなかった（壊れたデータや違うパスワード）のか、
/// 受け取った側の都合（書き込めないなど）なのかを分けて、パスワードの誤りと見分けられるようにする。
pub enum EntryError {
    Read(std::io::Error),
    Other(String),
}

struct Pending {
    mount: Mount,
    done: Receiver<Result<usize, ArchiveError>>,
    progress: Arc<Count>,
}

/// 展開の進み具合（項目の数）。
#[derive(Default)]
struct Count {
    done: AtomicUsize,
    total: AtomicUsize,
}

/// 開いている ZIP の一覧。
pub struct Archives {
    /// このプロセスの展開先の親。終了時にまとめて消す。
    base: PathBuf,
    mounted: Vec<Mount>,
    pending: Vec<Pending>,
    next: u64,
}

impl Archives {
    /// `base` の下に展開する。
    pub fn new(base: PathBuf) -> Self {
        Self {
            base,
            mounted: Vec::new(),
            pending: Vec::new(),
            next: 0,
        }
    }

    /// OS の一時フォルダの下に展開する。前回の異常終了で残ったものはここで消す。
    pub fn in_temp_dir() -> Self {
        let root = std::env::temp_dir().join("vw-archives");
        remove_stale(&root, &std::process::id().to_string());
        Self::new(root.join(std::process::id().to_string()))
    }

    /// `zip`（ZIP か 7z）を別スレッドで展開し始める。終わったら `wake` を呼ぶ。すでに開いているか
    /// 展開中なら何もせず `false`。暗号化されたものは `password` で開く。
    pub fn mount(
        &mut self,
        zip: &Path,
        password: Option<String>,
        wake: impl Fn() + Send + 'static,
    ) -> bool {
        let busy = |m: &Mount| m.zip == zip;
        if self.mounted.iter().any(busy) || self.pending.iter().any(|p| busy(&p.mount)) {
            return false;
        }
        let stem = zip
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dir = self.base.join(format!("{}-{stem}", self.next));
        self.next += 1;
        let mount = Mount {
            zip: zip.to_path_buf(),
            dir,
        };
        let (tx, rx) = channel();
        let (from, to) = (mount.zip.clone(), mount.dir.clone());
        let progress = Arc::new(Count::default());
        let count = progress.clone();
        std::thread::spawn(move || {
            let _ = tx.send(extract(&from, &to, password.as_deref(), &count));
            wake();
        });
        self.pending.push(Pending {
            mount,
            done: rx,
            progress,
        });
        true
    }

    /// 展開中のものの進み具合（済んだ項目, 全部の項目）。展開中のものがなければ `None`。
    pub fn progress(&self) -> Option<(usize, usize)> {
        if self.pending.is_empty() {
            return None;
        }
        Some(self.pending.iter().fold((0, 0), |(done, total), p| {
            (
                done + p.progress.done.load(Ordering::Relaxed),
                total + p.progress.total.load(Ordering::Relaxed),
            )
        }))
    }

    /// 展開が終わったものを受け取る。
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let mut i = 0;
        while i < self.pending.len() {
            let result = match self.pending[i].done.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(TryRecvError::Disconnected) => {
                    Err(ArchiveError::Other("extraction stopped".into()))
                }
            };
            let Pending { mount, .. } = self.pending.remove(i);
            match result {
                Ok(files) => {
                    self.mounted.push(mount.clone());
                    events.push(Event::Mounted { mount, files });
                }
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&mount.dir);
                    let zip = mount.zip;
                    events.push(match error {
                        ArchiveError::NeedsPassword => Event::NeedsPassword { zip, wrong: false },
                        ArchiveError::WrongPassword => Event::NeedsPassword { zip, wrong: true },
                        ArchiveError::Other(error) => Event::Failed { zip, error },
                    });
                }
            }
        }
        events
    }

    /// 展開先 `dir` の ZIP を閉じ、展開したファイルを消す。
    pub fn unmount(&mut self, dir: &Path) -> Option<Mount> {
        let i = self.mounted.iter().position(|m| m.dir == dir)?;
        let mount = self.mounted.remove(i);
        let _ = std::fs::remove_dir_all(&mount.dir);
        Some(mount)
    }

    /// `path` が入っている展開済みの ZIP。
    pub fn mount_of(&self, path: &Path) -> Option<&Mount> {
        self.mounted.iter().find(|m| path.starts_with(&m.dir))
    }

    /// 閉じた ZIP の展開先（消したところ）か、その中のパス。
    pub fn is_closed(&self, path: &Path) -> bool {
        path.starts_with(&self.base) && self.mount_of(path).is_none()
    }

    /// `zip` を展開したもの。
    pub fn mounted_zip(&self, zip: &Path) -> Option<&Mount> {
        self.mounted.iter().find(|m| m.zip == zip)
    }
}

impl Drop for Archives {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// ツリーでフォルダのように開けるアーカイブ（ZIP と 7z）か。
///
/// 名前だけでなく、フォルダでないことも見る。データセットの取り出しで ZIP の中身を
/// `data.zip` という名前のフォルダに置くので、名前だけではそのフォルダをアーカイブとして
/// 開こうとしてしまうため。
pub fn is_archive(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("7z"))
        && !path.is_dir()
}

const SEVEN_ZIP_MAGIC: &[u8] = b"7z\xbc\xaf\x27\x1c";

/// `archive` を `dir` に展開し、展開したファイルの数を返す。
fn extract(
    archive: &Path,
    dir: &Path,
    password: Option<&str>,
    count: &Count,
) -> Result<usize, ArchiveError> {
    std::fs::create_dir_all(dir)?;
    if let Some(n) = entry_count(archive, password) {
        count.total.store(n, Ordering::Relaxed);
    }
    for_each_entry(archive, password, |relative, data| {
        count.done.fetch_add(1, Ordering::Relaxed);
        let out = dir.join(relative);
        let Some(data) = data else {
            return std::fs::create_dir_all(&out).map_err(|e| EntryError::Other(e.to_string()));
        };
        let target = out
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::File::create(&out));
        match target {
            Ok(mut file) => copy_entry(data, &mut file),
            Err(e) => Err(EntryError::Other(e.to_string())),
        }
    })
}

/// `archive` の項目（フォルダも含む）の数。進み具合を出すためだけに使うので、読めなければ `None`。
fn entry_count(archive: &Path, password: Option<&str>) -> Option<usize> {
    let file = std::fs::File::open(archive).ok()?;
    if let Ok(zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) {
        return Some(zip.len());
    }
    let key = password.map_or_else(sevenz_rust2::Password::empty, sevenz_rust2::Password::from);
    let reader = sevenz_rust2::ArchiveReader::open(archive, key).ok()?;
    Some(reader.archive().files.len())
}

/// `archive`（ZIP か 7z）の項目を順に `visit` に渡し、渡したファイルの数を返す。`visit` には
/// アーカイブの中の相対パスと、ファイルなら中身を読むもの、フォルダなら `None` を渡す。
/// 展開先の外を指す名前の項目は渡さない（[`enclosed_name`]）。
///
/// 形式は拡張子ではなく先頭のバイト列で見分ける（拡張子と中身が違うファイルもあるため）。
pub fn for_each_entry(
    archive: &Path,
    password: Option<&str>,
    visit: impl FnMut(&Path, Option<&mut dyn std::io::Read>) -> Result<(), EntryError>,
) -> Result<usize, ArchiveError> {
    let mut head = [0u8; 6];
    let mut file = std::fs::File::open(archive)?;
    let seven_zip =
        std::io::Read::read_exact(&mut file, &mut head).is_ok() && head == SEVEN_ZIP_MAGIC;
    let named_7z = archive
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("7z"));
    if seven_zip {
        each_in_7z(archive, password, visit)
    } else if named_7z && !head.starts_with(b"PK") {
        Err(ArchiveError::Other("not a 7z archive".into()))
    } else {
        each_in_zip(archive, password, visit)
    }
}

fn each_in_zip(
    zip: &Path,
    password: Option<&str>,
    mut visit: impl FnMut(&Path, Option<&mut dyn std::io::Read>) -> Result<(), EntryError>,
) -> Result<usize, ArchiveError> {
    let file = std::fs::File::open(zip)?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))?;
    let mut files = 0;
    for i in 0..archive.len() {
        let encrypted = match archive.by_index(i) {
            Ok(_) => false,
            Err(zip::result::ZipError::UnsupportedArchive(m))
                if m == zip::result::ZipError::PASSWORD_REQUIRED =>
            {
                true
            }
            Err(e) => return Err(e.into()),
        };
        let mut entry = match (encrypted, password) {
            (false, _) => archive.by_index(i)?,
            (true, None) => return Err(ArchiveError::NeedsPassword),
            (true, Some(pw)) => match archive.by_index_decrypt(i, pw.as_bytes()) {
                Err(zip::result::ZipError::InvalidPassword) => {
                    return Err(ArchiveError::WrongPassword)
                }
                other => other?,
            },
        };
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        if entry.is_dir() {
            visit(&relative, None).map_err(|e| entry_failed(&relative, e, false))?;
            continue;
        }
        // ZipCrypto はパスワードの確認が 1 バイトしかなく、違うパスワードでも 256 回に 1 回は
        // 確認を通ってしまう。そのときは中身の CRC が合わずに読む途中で失敗する。
        visit(&relative, Some(&mut entry)).map_err(|e| entry_failed(&relative, e, encrypted))?;
        files += 1;
    }
    Ok(files)
}

fn entry_failed(relative: &Path, e: EntryError, encrypted: bool) -> ArchiveError {
    match e {
        EntryError::Read(_) if encrypted => ArchiveError::WrongPassword,
        EntryError::Read(e) => ArchiveError::Other(format!("{}: {e}", relative.display())),
        EntryError::Other(e) => ArchiveError::Other(format!("{}: {e}", relative.display())),
    }
}

fn each_in_7z(
    path: &Path,
    password: Option<&str>,
    mut visit: impl FnMut(&Path, Option<&mut dyn std::io::Read>) -> Result<(), EntryError>,
) -> Result<usize, ArchiveError> {
    use sevenz_rust2::{ArchiveReader, Error, Password};
    // 違うパスワードでは、復号した見出しや中身が壊れたデータになるだけで、どのエラーになるかは
    // 決まらない（「見出しがない」などになる）。パスワードを渡したときの失敗はパスワードの誤りとみなす。
    // 受け取った側の失敗は `failed` で別に返すので、ここに来るのは読む側の失敗だけ。
    let map = |e: Error| match (e, password) {
        (_, Some(_)) => ArchiveError::WrongPassword,
        (Error::PasswordRequired | Error::MaybeBadPassword(_), None) => ArchiveError::NeedsPassword,
        (other, None) => ArchiveError::Other(other.to_string()),
    };
    let key = password.map_or_else(Password::empty, Password::from);
    let mut reader = ArchiveReader::open(path, key).map_err(map)?;
    let mut files = 0;
    let mut failed = None;
    reader
        .for_each_entries(|entry, data| {
            let Some(relative) = enclosed_name(entry.name()) else {
                return Ok(true);
            };
            let result = if entry.is_directory() {
                visit(&relative, None)
            } else {
                visit(&relative, Some(data))
            };
            match result {
                Ok(()) => {
                    files += usize::from(!entry.is_directory());
                    Ok(true)
                }
                Err(EntryError::Read(e)) => Err(e.into()),
                Err(EntryError::Other(e)) => {
                    failed = Some(format!("{}: {e}", relative.display()));
                    Ok(false)
                }
            }
        })
        .map_err(map)?;
    match failed {
        Some(error) => Err(ArchiveError::Other(error)),
        None => Ok(files),
    }
}

/// 中身を書き写す。`std::io::copy` を使わないのは、読めなかったのか書けなかったのかを
/// 同じ `io::Error` で返すため。
pub fn copy_entry(
    data: &mut dyn std::io::Read,
    target: &mut std::fs::File,
) -> Result<(), EntryError> {
    use std::io::Write as _;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = data.read(&mut buf).map_err(EntryError::Read)?;
        if n == 0 {
            return Ok(());
        }
        target
            .write_all(&buf[..n])
            .map_err(|e| EntryError::Other(e.to_string()))?;
    }
}

/// アーカイブの中の名前を、展開先の中を指す相対パスにする。「../」や絶対パスで展開先の外を
/// 指すものは `None`。悪意のあるアーカイブで一時フォルダの外のファイルを書き換えられないようにするため。
/// ZIP は zip クレートの `enclosed_name` が同じことをするが、7z のクレートにはない。
fn enclosed_name(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            // Windows のドライブ名（C:）や代替データストリームを含む名前も外を指しうる。
            p if p.contains(':') => return None,
            p => out.push(p),
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// 前回までの vw が残した展開先のうち、古いものを消す。
fn remove_stale(root: &Path, own: &str) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > STALE_AFTER);
        // Linux ではプロセスが動いているか /proc で分かるので、終わったものはすぐ消す。
        let dead = cfg!(target_os = "linux") && !Path::new("/proc").join(e.file_name()).exists();
        if (old || dead) && e.file_name() != own {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use std::io::Write as _;

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            w.start_file(*name, options).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    fn write_encrypted_zip(path: &Path, password: &str, aes: bool) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        let options = if aes {
            options.with_aes_encryption(zip::AesMode::Aes256, password)
        } else {
            use zip::unstable::write::FileOptionsExt as _;
            options
                .with_deprecated_encryption(password.as_bytes())
                .unwrap()
        };
        w.start_file("secret.png", options).unwrap();
        w.write_all(&[7; 4096]).unwrap();
        w.finish().unwrap();
    }

    /// `files` を入れた 7z を作る。`password` があれば暗号化する。
    fn write_7z(path: &Path, files: &[(&str, &[u8])], password: Option<&str>) {
        let src = path.with_extension("src");
        for (name, data) in files {
            let file = src.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, data).unwrap();
        }
        match password {
            Some(pw) => sevenz_rust2::compress_to_path_encrypted(&src, path, pw.into()).unwrap(),
            None => sevenz_rust2::compress_to_path(&src, path).unwrap(),
        }
    }

    fn wait(archives: &mut Archives) -> Vec<Event> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let events = archives.poll();
            if !events.is_empty() {
                return events;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "extraction did not finish"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn mounting_extracts_the_zip_and_unmounting_removes_it() {
        let tmp = TempDir::new();
        let zip = tmp.path().join("scans.zip");
        write_zip(&zip, &[("a.png", b"A"), ("sub/b.dcm", b"B")]);
        let mut archives = Archives::new(tmp.path().join("work"));
        assert!(archives.mount(&zip, None, || {}));
        assert!(
            !archives.mount(&zip, None, || {}),
            "the same zip is opened once"
        );
        let events = wait(&mut archives);
        let Event::Mounted { mount, files } = &events[0] else {
            panic!("{events:?}");
        };
        assert_eq!(*files, 2);
        assert_eq!(mount.name(), "scans.zip");
        let b = mount.dir.join("sub/b.dcm");
        assert_eq!(std::fs::read(&b).unwrap(), b"B");
        assert_eq!(archives.mount_of(&b), Some(mount));
        assert!(archives.mount_of(&zip).is_none());

        let dir = mount.dir.clone();
        assert!(archives.unmount(&dir).is_some());
        assert!(!dir.exists());
        assert!(archives.mount_of(&b).is_none());
        assert!(archives.is_closed(&b));
        assert!(!archives.is_closed(&zip));
    }

    #[test]
    fn encrypted_zip_asks_for_a_password_then_opens_with_the_right_one() {
        for aes in [false, true] {
            let tmp = TempDir::new();
            let zip = tmp.path().join("secret.zip");
            write_encrypted_zip(&zip, "open sesame", aes);
            let out = tmp.path().join("out");
            assert_eq!(
                extract(&zip, &out, None, &Count::default()),
                Err(ArchiveError::NeedsPassword)
            );
            assert_eq!(
                extract(&zip, &out, Some("wrong"), &Count::default()),
                Err(ArchiveError::WrongPassword),
                "aes = {aes}"
            );
            assert_eq!(
                extract(&zip, &out, Some("open sesame"), &Count::default()),
                Ok(1)
            );
            assert_eq!(std::fs::read(out.join("secret.png")).unwrap(), [7; 4096]);
        }
    }

    #[test]
    fn mounting_an_encrypted_zip_reports_that_a_password_is_needed() {
        let tmp = TempDir::new();
        let zip = tmp.path().join("secret.zip");
        write_encrypted_zip(&zip, "pw", true);
        let mut archives = Archives::new(tmp.path().join("work"));
        archives.mount(&zip, None, || {});
        let events = wait(&mut archives);
        assert!(
            matches!(&events[0], Event::NeedsPassword { wrong: false, .. }),
            "{events:?}"
        );
        archives.mount(&zip, Some("pw".into()), || {});
        let events = wait(&mut archives);
        assert!(
            matches!(&events[0], Event::Mounted { files: 1, .. }),
            "{events:?}"
        );
    }

    #[test]
    fn extracting_counts_every_entry_for_the_progress_line() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("photos.7z");
        write_7z(&archive, &[("a.png", b"A"), ("sub/b.png", b"B")], None);
        let count = Count::default();
        extract(&archive, &tmp.path().join("out"), None, &count).unwrap();
        let total = count.total.load(Ordering::Relaxed);
        assert!(total >= 2, "{total}");
        assert_eq!(count.done.load(Ordering::Relaxed), total);
    }

    #[test]
    fn seven_zip_is_extracted_with_its_folders() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("photos.7z");
        write_7z(&archive, &[("a.png", b"A"), ("sub/b.png", b"B")], None);
        let out = tmp.path().join("out");
        assert_eq!(extract(&archive, &out, None, &Count::default()), Ok(2));
        assert_eq!(std::fs::read(out.join("sub/b.png")).unwrap(), b"B");
    }

    #[test]
    fn encrypted_seven_zip_needs_the_right_password() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("secret.7z");
        write_7z(&archive, &[("a.png", &[1; 4096])], Some("pw"));
        let out = tmp.path().join("out");
        assert_eq!(
            extract(&archive, &out, None, &Count::default()),
            Err(ArchiveError::NeedsPassword)
        );
        assert_eq!(
            extract(&archive, &out, Some("nope"), &Count::default()),
            Err(ArchiveError::WrongPassword)
        );
        assert_eq!(
            extract(&archive, &out, Some("pw"), &Count::default()),
            Ok(1)
        );
    }

    #[test]
    fn archive_names_outside_the_folder_are_rejected() {
        assert_eq!(
            enclosed_name("a/b.png"),
            Some(PathBuf::from("a").join("b.png"))
        );
        assert_eq!(
            enclosed_name("a\\b.png"),
            Some(PathBuf::from("a").join("b.png"))
        );
        assert_eq!(
            enclosed_name("/etc/passwd"),
            Some(PathBuf::from("etc").join("passwd"))
        );
        assert_eq!(enclosed_name("../x.png"), None);
        assert_eq!(enclosed_name("a/../../x.png"), None);
        assert_eq!(enclosed_name("C:/x.png"), None);
        assert_eq!(enclosed_name(""), None);
    }

    #[test]
    fn entries_pointing_outside_the_extraction_folder_are_skipped() {
        let tmp = TempDir::new();
        let zip = tmp.path().join("evil.zip");
        write_zip(&zip, &[("../escaped.txt", b"x"), ("ok.png", b"y")]);
        let out = tmp.path().join("out");
        assert_eq!(extract(&zip, &out, None, &Count::default()).unwrap(), 1);
        assert!(!tmp.path().join("escaped.txt").exists());
    }

    #[test]
    fn a_broken_zip_reports_an_error_and_leaves_nothing_behind() {
        let tmp = TempDir::new();
        let zip = tmp.path().join("broken.zip");
        std::fs::write(&zip, b"not a zip").unwrap();
        let mut archives = Archives::new(tmp.path().join("work"));
        archives.mount(&zip, None, || {});
        assert!(matches!(wait(&mut archives)[0], Event::Failed { .. }));
        assert!(archives.mount_of(&tmp.path().join("work")).is_none());
    }

    #[test]
    fn closing_vw_removes_everything_it_extracted() {
        let tmp = TempDir::new();
        let zip = tmp.path().join("a.zip");
        write_zip(&zip, &[("a.png", b"A")]);
        let base = tmp.path().join("work");
        let mut archives = Archives::new(base.clone());
        archives.mount(&zip, None, || {});
        wait(&mut archives);
        drop(archives);
        assert!(!base.exists());
    }

    #[test]
    fn zip_and_7z_files_are_recognised_by_extension_in_any_case() {
        assert!(is_archive(Path::new("a.ZIP")));
        assert!(is_archive(Path::new("a.7Z")));
        assert!(!is_archive(Path::new("a.png")));
    }

    #[test]
    fn a_folder_named_like_an_archive_is_a_folder() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("data.zip");
        std::fs::create_dir_all(&dir).unwrap();

        assert!(!is_archive(&dir));
    }
}
