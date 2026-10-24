//! ZIP・7z・tar（gzip / bzip2 / xz / zstd で圧縮したものも）と、1 ファイルだけを圧縮した
//! .gz などをフォルダとして開く。vw 専用の展開先（既定は OS の一時フォルダ）に展開し、
//! アンマウントするまで保つ。
//!
//! ZIP の中を直接読まずに展開するのは、ツリー・フォルダの読み込み・
//! フォルダの監視がすべてファイルのパスで動いているため。展開すれば ZIP の中も普通の
//! フォルダと同じに扱える。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, OnceLock};
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

/// 展開の進み具合（展開後のバイト数）。`total` は目次を読み終えるまで 0。
///
/// 項目の数で数えないのは、数 GB のファイルが数個だけの ZIP だと、1 つ書き終えるまで
/// 進み具合が 0 のまま動かず、止まって見えるため。
#[derive(Default)]
struct Count {
    done: AtomicUsize,
    total: AtomicUsize,
}

/// 展開先のフォルダ名。設定で選んだフォルダの直下にこれを作り、その中だけを消す。
/// 選んだフォルダをそのまま使わないのは、古いものの後片付けでそこにある利用者のファイルを
/// 消してしまわないため。
const FOLDER_NAME: &str = "vw-archives";

/// 開いている ZIP の一覧。
pub struct Archives {
    /// このプロセスの展開先の親。終了時にまとめて消す。
    base: PathBuf,
    /// 設定で展開先を変える前の `base`。そこに展開したものは閉じるまでそのまま使い、
    /// 終了時に一緒に消す。
    earlier: Vec<PathBuf>,
    mounted: Vec<Mount>,
    pending: Vec<Pending>,
    next: u64,
}

impl Archives {
    /// `base` の下に展開する。
    pub fn new(base: PathBuf) -> Self {
        Self {
            base,
            earlier: Vec::new(),
            mounted: Vec::new(),
            pending: Vec::new(),
            next: 0,
        }
    }

    /// `parent`（`None` なら OS の一時フォルダ）の下に展開する。前回の異常終了で残ったものはここで消す。
    pub fn in_dir(parent: Option<&Path>) -> Self {
        Self::new(prepare_base(parent))
    }

    /// これから開くものの展開先を `parent` の下に変える。もう開いているものは閉じるまで今の場所に置く。
    pub fn set_parent(&mut self, parent: Option<&Path>) {
        let base = base_under(parent);
        if base == self.base {
            return;
        }
        let base = prepare_base(parent);
        let old = std::mem::replace(&mut self.base, base);
        self.earlier.retain(|b| *b != self.base);
        self.earlier.push(old);
    }

    /// これから開くものの展開先。
    pub fn base(&self) -> &Path {
        &self.base
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

    /// 展開中のもの 1 つずつの（ZIP の名前, 書いたバイト数, 全部のバイト数）。目次を読んでいる間は全部が 0。
    pub fn progress(&self) -> Vec<(String, usize, usize)> {
        self.pending
            .iter()
            .map(|p| {
                (
                    p.mount.name(),
                    p.progress.done.load(Ordering::Relaxed),
                    p.progress.total.load(Ordering::Relaxed),
                )
            })
            .collect()
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
        let extracted = std::iter::once(&self.base)
            .chain(&self.earlier)
            .any(|b| path.starts_with(b));
        extracted && self.mount_of(path).is_none()
    }

    /// `zip` を展開したもの。
    pub fn mounted_zip(&self, zip: &Path) -> Option<&Mount> {
        self.mounted.iter().find(|m| m.zip == zip)
    }
}

impl Drop for Archives {
    fn drop(&mut self) {
        for base in std::iter::once(&self.base).chain(&self.earlier) {
            let _ = std::fs::remove_dir_all(base);
        }
    }
}

/// `parent` の下の、このプロセスの展開先。
fn base_under(parent: Option<&Path>) -> PathBuf {
    let root = parent.map_or_else(std::env::temp_dir, Path::to_path_buf);
    root.join(FOLDER_NAME).join(std::process::id().to_string())
}

/// [`base_under`] を返す。同じ親に前の vw が残したものがあれば、先に消す。
fn prepare_base(parent: Option<&Path>) -> PathBuf {
    let base = base_under(parent);
    if let Some(root) = base.parent() {
        remove_stale(root, &std::process::id().to_string());
    }
    base
}

/// ツリーでフォルダのように開けるアーカイブか。拡張子で見る。
///
/// 名前だけでなく、フォルダでないことも見る。データセットの取り出しで ZIP の中身を
/// `data.zip` という名前のフォルダに置くので、名前だけではそのフォルダをアーカイブとして
/// 開こうとしてしまうため。
pub fn is_archive(path: &Path) -> bool {
    kind(path).is_some() && !path.is_dir()
}

/// 中身の並べ方。圧縮の方式（gzip など）は中身の先頭のバイト列で見分けるので、ここでは区別しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// ZIP か 7z。どちらかは先頭のバイト列で見分ける。
    ZipOr7z,
    Tar,
    /// 1 ファイルだけを圧縮したもの（`photo.png.gz`）。
    Single,
}

/// 拡張子（小文字）と形式。.cbz・.cb7・.cbt は漫画のページ画像を束ねるときの名前で、中身は ZIP・7z・tar。
/// `tar.gz` が `gz` より先にあるのは、長い方から当てるため。
const KINDS: &[(&str, Kind)] = &[
    ("zip", Kind::ZipOr7z),
    ("cbz", Kind::ZipOr7z),
    ("7z", Kind::ZipOr7z),
    ("cb7", Kind::ZipOr7z),
    ("tar", Kind::Tar),
    ("cbt", Kind::Tar),
    ("tar.gz", Kind::Tar),
    ("tgz", Kind::Tar),
    ("tar.bz2", Kind::Tar),
    ("tbz2", Kind::Tar),
    ("tbz", Kind::Tar),
    ("tar.xz", Kind::Tar),
    ("txz", Kind::Tar),
    ("tar.zst", Kind::Tar),
    ("tzst", Kind::Tar),
    ("gz", Kind::Single),
    ("bz2", Kind::Single),
    ("xz", Kind::Single),
    ("zst", Kind::Single),
];

fn kind(path: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    let (_, kind) = KINDS.iter().find(|(ext, _)| has_extension(&name, ext))?;
    (*kind != Kind::Single || !decoded_directly(&name)).then_some(*kind)
}

/// `name` が `.ext` で終わり、その前に名前があるか。
fn has_extension(name: &str, ext: &str) -> bool {
    name.len() > ext.len() + 1
        && name.ends_with(ext)
        && name.as_bytes()[name.len() - ext.len() - 1] == b'.'
}

/// 画像のデコーダが圧縮したまま読む拡張子（`fits.gz` など）。
///
/// 呼び出し側ごとにデコーダの一覧を渡さず、起動時に一度だけ決めておくのは、
/// [`is_archive`] がツリー・カタログ・書き出しなど十数か所から呼ばれ、どこもデコーダの
/// 一覧を持っているとは限らないため。
static DECODED_DIRECTLY: OnceLock<Vec<String>> = OnceLock::new();

/// デコーダが圧縮したまま読む拡張子を伝える。これらはアーカイブとして開かず、画像として読む。
pub fn set_decoded_directly(extensions: Vec<String>) {
    let _ = DECODED_DIRECTLY.set(extensions);
}

fn decoded_directly(name: &str) -> bool {
    DECODED_DIRECTLY
        .get()
        .is_some_and(|exts| exts.iter().any(|ext| has_extension(name, ext)))
}

/// 読めないアーカイブの拡張子。分割したもの（`.7z.001`、`.z01`、`.r00`）は [`is_unsupported_archive`] で見る。
const UNSUPPORTED: &[&str] = &[
    "rar", "cbr", "lzh", "lha", "cab", "iso", "arj", "ace", "cpio", "xar", "dmg", "wim",
];

/// 読めない形式のアーカイブを見つけたときのログ。`{}` はフルパス。
pub const UNSUPPORTED_MESSAGE: &str = "Unsupported archive file: {}";

/// 読めない形式のアーカイブか。ログで知らせるためだけに使う。
///
/// 数字だけの拡張子は DICOM の連番（`IMG.001`）にも使われるので、手前の名前が
/// アーカイブの拡張子で終わるとき（`scans.7z.001`）だけ分割したものとみなす。
pub fn is_unsupported_archive(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let split_part = digits(ext)
        && KINDS
            .iter()
            .map(|(e, _)| *e)
            .chain(UNSUPPORTED.iter().copied())
            .any(|e| has_extension(stem, e) || stem == e);
    let volume = ext.len() == 3 && matches!(ext.as_bytes()[0], b'z' | b'r') && digits(&ext[1..]);
    let unsupported = !stem.is_empty() && (UNSUPPORTED.contains(&ext) || split_part || volume);
    unsupported && !path.is_dir()
}

const SEVEN_ZIP_MAGIC: &[u8] = b"7z\xbc\xaf\x27\x1c";

/// `archive` を `dir` に展開し、展開したファイルの数を返す。
///
/// tar 系は目次がなく、全部のバイト数は最後まで読まないと分からない。そこで進み具合は、
/// ZIP・7z は書いたバイト数、tar 系は元のファイルを読んだバイト数で数える。
fn extract(
    archive: &Path,
    dir: &Path,
    password: Option<&str>,
    count: &Count,
) -> Result<usize, ArchiveError> {
    std::fs::create_dir_all(dir)?;
    let streamed = kind(archive).is_some_and(|k| k != Kind::ZipOr7z);
    let total = if streamed {
        std::fs::metadata(archive).ok().map(|m| m.len() as usize)
    } else {
        total_size(archive, password)
    };
    if let Some(n) = total {
        count.total.store(n, Ordering::Relaxed);
    }
    let read = streamed.then_some(&count.done);
    each_entry(archive, password, read, |relative, data| {
        let out = dir.join(relative);
        let Some(data) = data else {
            return std::fs::create_dir_all(&out).map_err(|e| EntryError::Other(e.to_string()));
        };
        let target = out
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::File::create(&out));
        match target {
            Ok(mut file) if streamed => copy_entry(data, &mut file),
            Ok(mut file) => copy_entry(
                &mut Counted {
                    inner: data,
                    count: &count.done,
                },
                &mut file,
            ),
            Err(e) => Err(EntryError::Other(e.to_string())),
        }
    })
}

/// 読んだバイト数を数えながら読む。
struct Counted<'a, R> {
    inner: R,
    count: &'a AtomicUsize,
}

impl<R: std::io::Read> std::io::Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n, Ordering::Relaxed);
        Ok(n)
    }
}

/// `archive` の中身を展開したときのバイト数の合計。進み具合を出すためだけに使うので、
/// 読めなければ `None`。
fn total_size(archive: &Path, password: Option<&str>) -> Option<usize> {
    let file = std::fs::File::open(archive).ok()?;
    if let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) {
        let sizes = (0..zip.len()).filter_map(|i| zip.by_index_raw(i).ok().map(|e| e.size()));
        return Some(sizes.sum::<u64>() as usize);
    }
    let key = password.map_or_else(sevenz_rust2::Password::empty, sevenz_rust2::Password::from);
    let reader = sevenz_rust2::ArchiveReader::open(archive, key).ok()?;
    Some(reader.archive().files.iter().map(|f| f.size).sum::<u64>() as usize)
}

/// `archive` の項目を順に `visit` に渡し、渡したファイルの数を返す。`visit` には
/// アーカイブの中の相対パスと、ファイルなら中身を読むもの、フォルダなら `None` を渡す。
/// 展開先の外を指す名前の項目は渡さない（[`enclosed_name`]）。
///
/// ZIP と 7z、圧縮の方式は拡張子ではなく先頭のバイト列で見分ける（拡張子と中身が違うファイルもあるため）。
pub fn for_each_entry(
    archive: &Path,
    password: Option<&str>,
    visit: impl FnMut(&Path, Option<&mut dyn std::io::Read>) -> Result<(), EntryError>,
) -> Result<usize, ArchiveError> {
    each_entry(archive, password, None, visit)
}

/// [`for_each_entry`] に加えて、tar 系なら元のファイルを読んだバイト数を `read` に足していく。
fn each_entry(
    archive: &Path,
    password: Option<&str>,
    read: Option<&AtomicUsize>,
    visit: impl FnMut(&Path, Option<&mut dyn std::io::Read>) -> Result<(), EntryError>,
) -> Result<usize, ArchiveError> {
    match kind(archive) {
        Some(Kind::Tar) => each_in_tar(decompressed(archive, read)?, visit),
        Some(Kind::Single) => each_in_single(archive, decompressed(archive, read)?, visit),
        Some(Kind::ZipOr7z) | None => each_in_zip_or_7z(archive, password, visit),
    }
}

fn each_in_zip_or_7z(
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
        .is_some_and(|e| e.eq_ignore_ascii_case("7z") || e.eq_ignore_ascii_case("cb7"));
    if seven_zip {
        each_in_7z(archive, password, visit)
    } else if named_7z && !head.starts_with(b"PK") {
        Err(ArchiveError::Other("not a 7z archive".into()))
    } else {
        each_in_zip(archive, password, visit)
    }
}

/// `archive` を、先頭のバイト列から決めた方式で伸長しながら読むもの。どれでもなければそのまま読む
/// （圧縮していない tar）。
fn decompressed<'a>(
    archive: &Path,
    read: Option<&'a AtomicUsize>,
) -> Result<Box<dyn std::io::Read + 'a>, ArchiveError> {
    use std::io::BufRead as _;
    let file = std::fs::File::open(archive)?;
    let counted: Box<dyn std::io::Read + 'a> = match read {
        Some(count) => Box::new(Counted { inner: file, count }),
        None => Box::new(file),
    };
    let mut input = std::io::BufReader::new(counted);
    let head = input.fill_buf()?.to_vec();
    let reader: Box<dyn std::io::Read + 'a> = if head.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::bufread::MultiGzDecoder::new(input))
    } else if head.starts_with(b"BZh") {
        Box::new(bzip2::bufread::MultiBzDecoder::new(input))
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        Box::new(lzma_rust2::XzReader::new(input, true))
    } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Box::new(ZstdFrames::new(input)?)
    } else {
        Box::new(input)
    };
    Ok(reader)
}

/// zstd の続けて並んだフレームを順に伸長する。ruzstd の `StreamingDecoder` は 1 フレームしか読まず、
/// `zstd -T0` などで作ると複数のフレームになるため。
struct ZstdFrames<R: std::io::BufRead> {
    frame: Option<ruzstd::decoding::StreamingDecoder<R, ruzstd::decoding::FrameDecoder>>,
}

impl<R: std::io::BufRead> ZstdFrames<R> {
    fn new(input: R) -> std::io::Result<Self> {
        let frame =
            ruzstd::decoding::StreamingDecoder::new(input).map_err(std::io::Error::other)?;
        Ok(Self { frame: Some(frame) })
    }
}

impl<R: std::io::BufRead> std::io::Read for ZstdFrames<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let Some(frame) = self.frame.as_mut() else {
                return Ok(0);
            };
            let n = frame.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            let mut rest = self.frame.take().expect("checked above").into_inner();
            if rest.fill_buf()?.is_empty() {
                return Ok(0);
            }
            self.frame =
                Some(ruzstd::decoding::StreamingDecoder::new(rest).map_err(std::io::Error::other)?);
        }
    }
}

fn each_in_tar(
    input: impl std::io::Read,
    mut visit: impl FnMut(&Path, Option<&mut dyn std::io::Read>) -> Result<(), EntryError>,
) -> Result<usize, ArchiveError> {
    let mut archive = tar::Archive::new(input);
    let mut files = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = decode_name(&entry.path_bytes())
            .unwrap_or_else(|| String::from_utf8_lossy(&entry.path_bytes()).into_owned());
        let Some(relative) = enclosed_name(&name) else {
            continue;
        };
        // リンクは展開しない。シンボリックリンクは展開先の外を指せ、その先へ書き込まれうるため。
        let result = match entry.header().entry_type() {
            tar::EntryType::Directory => visit(&relative, None),
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                files += 1;
                visit(&relative, Some(&mut entry))
            }
            _ => continue,
        };
        result.map_err(|e| entry_failed(&relative, e, false))?;
    }
    Ok(files)
}

/// 1 ファイルだけを圧縮したもの。中身の名前は、圧縮したファイルの名前から最後の拡張子を外したもの。
fn each_in_single(
    archive: &Path,
    mut input: impl std::io::Read,
    mut visit: impl FnMut(&Path, Option<&mut dyn std::io::Read>) -> Result<(), EntryError>,
) -> Result<usize, ArchiveError> {
    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let relative = enclosed_name(&stem).ok_or_else(|| ArchiveError::Other("no name".into()))?;
    visit(&relative, Some(&mut input)).map_err(|e| entry_failed(&relative, e, false))?;
    Ok(1)
}

/// アーカイブの中の名前を文字列にする。UTF-8 として読めなければ Shift_JIS（日本語の Windows の
/// 文字コード）として読む。どちらでも読めなければ `None`。
///
/// 名前の文字コードを記録していない ZIP や tar は、作った PC の文字コードで名前が入っている。
/// 日本語名が化けないよう、UTF-8 の次は Shift_JIS を試す。設定で選ばせないのは、ほとんどの
/// 名前はこの 2 つのどちらかで読め、選ばせても迷わせるだけなため。
fn decode_name(raw: &[u8]) -> Option<String> {
    if let Ok(name) = std::str::from_utf8(raw) {
        return Some(name.to_owned());
    }
    encoding_rs::SHIFT_JIS
        .decode_without_bom_handling_and_without_replacement(raw)
        .map(|name| name.into_owned())
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
        let name = match decode_name(entry.name_raw()) {
            Some(name) => name,
            None => entry.name()?.into_owned(),
        };
        let Some(relative) = enclosed_name(&name) else {
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
/// ZIP は zip クレートの `enclosed_name` が同じことをするが、名前を Shift_JIS としても読むため
/// （[`decode_name`]）、どの形式もこちらで確かめる。
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
    fn extracting_counts_the_bytes_written_for_the_progress_line() {
        let tmp = TempDir::new();
        for (name, write) in [
            (
                "photos.7z",
                write_7z as fn(&Path, &[(&str, &[u8])], Option<&str>),
            ),
            (
                "photos.zip",
                |p: &Path, e: &[(&str, &[u8])], _: Option<&str>| write_zip(p, e),
            ),
        ] {
            let archive = tmp.path().join(name);
            write(
                &archive,
                &[("a.png", b"AAAA"), ("sub/b.png", b"BBBBBB")],
                None,
            );
            let count = Count::default();
            extract(
                &archive,
                &tmp.path().join(name).with_extension("out"),
                None,
                &count,
            )
            .unwrap();
            assert_eq!(count.total.load(Ordering::Relaxed), 10, "{name}");
            assert_eq!(count.done.load(Ordering::Relaxed), 10, "{name}");
        }
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
    fn changing_the_folder_extracts_new_archives_there_and_keeps_open_ones() {
        let tmp = TempDir::new();
        let (a, b) = (tmp.path().join("a.zip"), tmp.path().join("b.zip"));
        write_zip(&a, &[("a.png", b"A")]);
        write_zip(&b, &[("b.png", b"B")]);
        let mut archives = Archives::in_dir(Some(&tmp.path().join("first")));
        archives.mount(&a, None, || {});
        wait(&mut archives);
        let second = tmp.path().join("second");
        archives.set_parent(Some(&second));
        archives.mount(&b, None, || {});
        wait(&mut archives);

        let first_dir = archives.mounted_zip(&a).unwrap().dir.clone();
        let second_dir = archives.mounted_zip(&b).unwrap().dir.clone();
        assert!(first_dir.starts_with(tmp.path().join("first")));
        assert!(first_dir.join("a.png").is_file(), "an open archive stays");
        assert!(second_dir.starts_with(second.join(FOLDER_NAME)));
        assert!(second_dir.join("b.png").is_file());
    }

    #[test]
    fn closing_vw_removes_what_it_extracted_in_every_folder_it_used() {
        let tmp = TempDir::new();
        let (a, b) = (tmp.path().join("a.zip"), tmp.path().join("b.zip"));
        write_zip(&a, &[("a.png", b"A")]);
        write_zip(&b, &[("b.png", b"B")]);
        let mut archives = Archives::in_dir(Some(&tmp.path().join("first")));
        archives.mount(&a, None, || {});
        wait(&mut archives);
        archives.set_parent(Some(&tmp.path().join("second")));
        archives.mount(&b, None, || {});
        wait(&mut archives);
        let dirs: Vec<PathBuf> = [&a, &b]
            .map(|z| archives.mounted_zip(z).unwrap().dir.clone())
            .into();
        drop(archives);

        assert!(dirs.iter().all(|d| !d.exists()));
    }

    #[test]
    fn the_chosen_folder_keeps_its_own_files() {
        let tmp = TempDir::new();
        let chosen = tmp.path().join("work");
        let mine = chosen.join("notes.txt");
        std::fs::create_dir_all(&chosen).unwrap();
        std::fs::write(&mine, b"keep").unwrap();
        let zip = tmp.path().join("a.zip");
        write_zip(&zip, &[("a.png", b"A")]);
        let mut archives = Archives::in_dir(Some(&chosen));
        archives.mount(&zip, None, || {});
        wait(&mut archives);
        drop(archives);

        assert!(mine.is_file());
    }

    #[test]
    fn leftovers_in_the_previous_folder_are_left_as_they_are() {
        let tmp = TempDir::new();
        let leftover = tmp.path().join("first").join(FOLDER_NAME).join("0");
        std::fs::create_dir_all(&leftover).unwrap();
        let mut archives = Archives::in_dir(Some(&tmp.path().join("second")));
        archives.set_parent(Some(&tmp.path().join("third")));

        assert!(leftover.exists());
    }

    /// `entries` の tar を作る。名前が `/` で終わるものはフォルダ。
    fn tar_bytes(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            let dir = name.ends_with(b"/");
            header.set_entry_type(if dir {
                tar::EntryType::Directory
            } else {
                tar::EntryType::Regular
            });
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            // set_path は UTF-8 の名前しか受け付けないので、Shift_JIS の名前を入れるために直接書く。
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap()
    }

    type Compress = fn(&[u8]) -> Vec<u8>;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(data).unwrap();
        gz.finish().unwrap()
    }

    fn bzip2(data: &[u8]) -> Vec<u8> {
        let mut bz = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        bz.write_all(data).unwrap();
        bz.finish().unwrap()
    }

    fn xz(data: &[u8]) -> Vec<u8> {
        let mut xz =
            lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1)).unwrap();
        xz.write_all(data).unwrap();
        xz.finish().unwrap()
    }

    /// 2 つのフレームに分けた zstd（並列で圧縮したものと同じ形）。
    fn zstd_in_two_frames(data: &[u8]) -> Vec<u8> {
        let level = ruzstd::encoding::CompressionLevel::Fastest;
        let (head, tail) = data.split_at(data.len() / 2);
        let mut out = ruzstd::encoding::compress_to_vec(head, level);
        out.extend(ruzstd::encoding::compress_to_vec(tail, level));
        out
    }

    #[test]
    fn tar_is_extracted_with_its_folders() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("photos.tar");
        let tar = tar_bytes(&[(b"sub/", b""), (b"a.png", b"A"), (b"sub/b.png", b"B")]);
        std::fs::write(&archive, tar).unwrap();
        let out = tmp.path().join("out");

        assert_eq!(extract(&archive, &out, None, &Count::default()), Ok(2));
        assert_eq!(std::fs::read(out.join("a.png")).unwrap(), b"A");
        assert_eq!(std::fs::read(out.join("sub/b.png")).unwrap(), b"B");
    }

    #[test]
    fn compressed_tars_are_extracted_whatever_the_compression() {
        let tmp = TempDir::new();
        let tar = tar_bytes(&[(b"a.png", b"AAAA"), (b"sub/b.png", b"BBBBBB")]);
        let compress: [(&str, Compress); 4] = [
            ("photos.tar.gz", gzip),
            ("photos.tar.bz2", bzip2),
            ("photos.tar.xz", xz),
            ("photos.tar.zst", zstd_in_two_frames),
        ];
        for (name, compress) in compress {
            let archive = tmp.path().join(name);
            std::fs::write(&archive, compress(&tar)).unwrap();
            let out = tmp.path().join(format!("{name}.out"));

            assert_eq!(
                extract(&archive, &out, None, &Count::default()),
                Ok(2),
                "{name}"
            );
            assert_eq!(
                std::fs::read(out.join("sub/b.png")).unwrap(),
                b"BBBBBB",
                "{name}"
            );
        }
    }

    #[test]
    fn the_compression_is_told_by_the_contents_not_the_name() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("photos.tgz");
        std::fs::write(&archive, xz(&tar_bytes(&[(b"a.png", b"A")]))).unwrap();

        assert_eq!(
            extract(&archive, &tmp.path().join("out"), None, &Count::default()),
            Ok(1)
        );
    }

    #[test]
    fn links_in_a_tar_are_not_extracted() {
        let tmp = TempDir::new();
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder
            .append_link(&mut header, "evil", "/etc/passwd")
            .unwrap();
        let mut file = tar::Header::new_gnu();
        file.set_size(1);
        builder.append_data(&mut file, "a.png", &b"A"[..]).unwrap();
        let archive = tmp.path().join("links.tar");
        std::fs::write(&archive, builder.into_inner().unwrap()).unwrap();
        let out = tmp.path().join("out");

        assert_eq!(extract(&archive, &out, None, &Count::default()), Ok(1));
        assert!(std::fs::symlink_metadata(out.join("evil")).is_err());
    }

    #[test]
    fn a_broken_tar_reports_an_error() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("broken.tar.gz");
        let gz = gzip(&tar_bytes(&[(b"a.png", &[7u8; 4096])]));
        std::fs::write(&archive, &gz[..gz.len() / 2]).unwrap();

        let result = extract(&archive, &tmp.path().join("out"), None, &Count::default());
        assert!(matches!(result, Err(ArchiveError::Other(_))), "{result:?}");
    }

    #[test]
    fn a_single_compressed_file_opens_as_a_folder_holding_it() {
        let tmp = TempDir::new();
        let out = tmp.path().join("out");
        for (name, data) in [
            ("photo.png.gz", gzip(b"PNG")),
            ("photo.png.bz2", bzip2(b"PNG")),
            ("photo.png.xz", xz(b"PNG")),
            ("photo.png.zst", zstd_in_two_frames(b"PNG")),
        ] {
            let archive = tmp.path().join(name);
            std::fs::write(&archive, data).unwrap();
            let out = out.join(name);

            assert_eq!(
                extract(&archive, &out, None, &Count::default()),
                Ok(1),
                "{name}"
            );
            assert_eq!(
                std::fs::read(out.join("photo.png")).unwrap(),
                b"PNG",
                "{name}"
            );
        }
    }

    #[test]
    fn extracting_a_tar_counts_the_bytes_read_from_the_file() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("photos.tar.gz");
        let gz = gzip(&tar_bytes(&[(b"a.png", &[1u8; 5000])]));
        std::fs::write(&archive, &gz).unwrap();
        let count = Count::default();
        extract(&archive, &tmp.path().join("out"), None, &count).unwrap();

        assert_eq!(count.total.load(Ordering::Relaxed), gz.len());
        assert_eq!(count.done.load(Ordering::Relaxed), gz.len());
    }

    /// 「ソ」は Shift_JIS で 2 バイト目が `\` と同じ 0x5C になる。名前を文字にしてから区切らないと、
    /// フォルダの区切りと取り違える。
    const SO_PNG_SJIS: &[u8] = b"\x83\x5c.png";

    #[test]
    fn shift_jis_names_in_a_tar_are_read_as_japanese() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("japanese.tar");
        std::fs::write(&archive, tar_bytes(&[(SO_PNG_SJIS, b"A")])).unwrap();
        let out = tmp.path().join("out");

        assert_eq!(extract(&archive, &out, None, &Count::default()), Ok(1));
        assert!(out.join("ソ.png").is_file());
    }

    #[test]
    fn shift_jis_names_in_a_zip_are_read_as_japanese() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("japanese.zip");
        // zip クレートは UTF-8 の名前でしか書けないので、同じ長さの名前で書いてから名前のバイト列を置き換える。
        write_zip(&archive, &[("XX.png", b"A")]);
        let bytes = std::fs::read(&archive).unwrap();
        let mut patched = Vec::new();
        let mut rest = &bytes[..];
        while let Some(at) = rest.windows(6).position(|w| w == b"XX.png") {
            patched.extend_from_slice(&rest[..at]);
            patched.extend_from_slice(SO_PNG_SJIS);
            rest = &rest[at + 6..];
        }
        patched.extend_from_slice(rest);
        std::fs::write(&archive, patched).unwrap();
        let out = tmp.path().join("out");

        assert_eq!(extract(&archive, &out, None, &Count::default()), Ok(1));
        assert!(out.join("ソ.png").is_file());
    }

    #[test]
    fn tar_and_comic_book_names_are_archives() {
        for name in [
            "a.tar",
            "a.TGZ",
            "a.tar.gz",
            "a.tar.bz2",
            "a.tbz2",
            "a.tar.xz",
            "a.txz",
            "a.tar.zst",
            "a.tzst",
            "a.cbz",
            "a.cb7",
            "a.cbt",
            "photo.png.gz",
            "photo.png.bz2",
            "a.xz",
            "a.zst",
        ] {
            assert!(is_archive(Path::new(name)), "{name}");
        }
        assert!(!is_archive(Path::new(".gz")));
    }

    #[test]
    fn files_a_decoder_reads_compressed_are_not_archives() {
        set_decoded_directly(vec!["fits.gz".into()]);

        assert!(!is_archive(Path::new("m31.FITS.gz")));
        assert!(is_archive(Path::new("m31.png.gz")));
    }

    #[test]
    fn unsupported_archives_are_told_apart() {
        for name in [
            "a.rar",
            "a.CBR",
            "a.lzh",
            "a.lha",
            "a.cab",
            "a.iso",
            "a.7z.001",
            "a.zip.002",
            "a.z01",
            "a.r00",
        ] {
            assert!(is_unsupported_archive(Path::new(name)), "{name}");
        }
        for name in ["IMG.001", "a.zip", "a.tar.gz", "a.png", "rar", "a.zip.part"] {
            assert!(!is_unsupported_archive(Path::new(name)), "{name}");
        }
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
