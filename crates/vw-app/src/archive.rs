//! ZIP をフォルダとして開く。vw 専用の一時フォルダに展開し、アンマウントするまで保つ。
//!
//! ZIP の中を直接読まずに展開するのは、ツリー・フォルダの読み込み・
//! フォルダの監視がすべてファイルのパスで動いているため。展開すれば ZIP の中も普通の
//! フォルダと同じに扱える。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
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
    Mounted { mount: Mount, files: usize },
    Failed { zip: PathBuf, error: String },
}

struct Pending {
    mount: Mount,
    done: Receiver<Result<usize, String>>,
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

    /// `zip` を別スレッドで展開し始める。終わったら `wake` を呼ぶ。すでに開いているか
    /// 展開中なら何もせず `false`。
    pub fn mount(&mut self, zip: &Path, wake: impl Fn() + Send + 'static) -> bool {
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
        std::thread::spawn(move || {
            let _ = tx.send(extract(&from, &to));
            wake();
        });
        self.pending.push(Pending { mount, done: rx });
        true
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
                Err(TryRecvError::Disconnected) => Err("extraction stopped".into()),
            };
            let Pending { mount, .. } = self.pending.remove(i);
            match result {
                Ok(files) => {
                    self.mounted.push(mount.clone());
                    events.push(Event::Mounted { mount, files });
                }
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&mount.dir);
                    events.push(Event::Failed {
                        zip: mount.zip,
                        error,
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

/// ツリーで ZIP として出すファイルか。
pub fn is_zip(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// `zip` を `dir` に展開し、展開したファイルの数を返す。
fn extract(zip: &Path, dir: &Path) -> Result<usize, String> {
    let file = std::fs::File::open(zip).map_err(|e| e.to_string())?;
    let mut archive =
        zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut files = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        // 「../」や絶対パスで展開先の外を指す項目は飛ばす。悪意のある ZIP で
        // 一時フォルダの外のファイルを書き換えられないようにするため。
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let out = dir.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut target = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut target)
            .map_err(|e| format!("{}: {e}", relative.display()))?;
        files += 1;
    }
    Ok(files)
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
        assert!(archives.mount(&zip, || {}));
        assert!(!archives.mount(&zip, || {}), "the same zip is opened once");
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
    fn entries_pointing_outside_the_extraction_folder_are_skipped() {
        let tmp = TempDir::new();
        let zip = tmp.path().join("evil.zip");
        write_zip(&zip, &[("../escaped.txt", b"x"), ("ok.png", b"y")]);
        let out = tmp.path().join("out");
        assert_eq!(extract(&zip, &out).unwrap(), 1);
        assert!(!tmp.path().join("escaped.txt").exists());
    }

    #[test]
    fn a_broken_zip_reports_an_error_and_leaves_nothing_behind() {
        let tmp = TempDir::new();
        let zip = tmp.path().join("broken.zip");
        std::fs::write(&zip, b"not a zip").unwrap();
        let mut archives = Archives::new(tmp.path().join("work"));
        archives.mount(&zip, || {});
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
        archives.mount(&zip, || {});
        wait(&mut archives);
        drop(archives);
        assert!(!base.exists());
    }

    #[test]
    fn zip_files_are_recognised_by_extension_in_any_case() {
        assert!(is_zip(Path::new("a.ZIP")));
        assert!(!is_zip(Path::new("a.png")));
    }
}
