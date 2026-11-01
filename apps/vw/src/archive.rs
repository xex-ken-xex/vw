//! ZIP・7z・tar（gzip / bzip2 / xz / zstd で圧縮したものも）と、1 ファイルだけを圧縮した
//! .gz などをフォルダとして開く。vw 専用の展開先（既定は OS の一時フォルダ）に展開し、
//! アンマウントするまで保つ。
//!
//! ZIP の中を直接読まずに展開するのは、ツリー・フォルダの読み込み・
//! フォルダの監視がすべてファイルのパスで動いているため。展開すれば ZIP の中も普通の
//! フォルダと同じに扱える。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
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
    /// 展開すると上限（[`Archives::set_size_limit`]）を超える。`size` は展開後の大きさで、
    /// tar 系のように書いてないものは `None`。続けるなら `over_limit` を付けて開き直す。
    TooLarge {
        zip: PathBuf,
        size: Option<u64>,
        limit: u64,
        password: Option<String>,
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
    budget: Arc<Budget>,
    password: Option<String>,
}

/// 展開してよい大きさ。
struct Budget {
    /// 上限。`None` は利用者が上限を超えてよいと確かめたとき。
    limit: Option<u64>,
    /// 上限を超えたので止めた。
    over_limit: std::sync::atomic::AtomicBool,
    /// 止めたときの展開後の大きさ（アーカイブに書いてあれば）。
    size: std::sync::Mutex<Option<u64>>,
}

impl Budget {
    fn new(limit: Option<u64>) -> Self {
        Self {
            limit,
            over_limit: Default::default(),
            size: Default::default(),
        }
    }

    fn refuse(&self, size: Option<u64>) -> ArchiveError {
        self.over_limit.store(true, Ordering::Relaxed);
        *self.size.lock().unwrap_or_else(|e| e.into_inner()) = size;
        ArchiveError::Other("larger than the archive size limit".into())
    }
}

/// 展開してよい大きさの既定値（GiB）。展開すると巨大になるアーカイブで、
/// 展開先のディスクを使い切らないようにする。
pub const DEFAULT_SIZE_LIMIT_GIB: u32 = 20;

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
    size_limit: u64,
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
            size_limit: u64::from(DEFAULT_SIZE_LIMIT_GIB) << 30,
        }
    }

    /// これから開くものを展開してよい大きさ（バイト）。超えるものは [`Event::TooLarge`] で知らせる。
    pub fn set_size_limit(&mut self, bytes: u64) {
        self.size_limit = bytes;
    }

    /// `parent`（`None` なら OS の一時フォルダ）の下に展開する。前回の異常終了で残ったものはここで消す。
    pub fn in_dir(parent: Option<&Path>) -> Self {
        Self::new(prepare_base(parent))
    }

    /// これから開くものの展開先を `parent` の下に変える。もう開いているものは閉じるまで今の場所に置く。
    pub fn set_parent(&mut self, parent: Option<&Path>) {
        let root = parent.map_or_else(std::env::temp_dir, Path::to_path_buf);
        if self.base.parent().and_then(Path::parent) == Some(root.as_path()) {
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
    /// 展開中なら何もせず `false`。暗号化されたものは `password` で開く。`over_limit` なら
    /// 大きさの上限を超えても展開する（空き容量は超えない）。
    pub fn mount(
        &mut self,
        zip: &Path,
        password: Option<String>,
        over_limit: bool,
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
        let budget = Arc::new(Budget::new((!over_limit).then_some(self.size_limit)));
        let limit = budget.clone();
        let key = password.clone();
        std::thread::spawn(move || {
            let _ = tx.send(extract(&from, &to, key.as_deref(), &count, &limit));
            wake();
        });
        self.pending.push(Pending {
            mount,
            done: rx,
            progress,
            budget,
            password,
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
            let Pending {
                mount,
                budget,
                password,
                ..
            } = self.pending.remove(i);
            match result {
                Ok(files) => {
                    self.mounted.push(mount.clone());
                    events.push(Event::Mounted { mount, files });
                }
                Err(_) if budget.over_limit.load(Ordering::Relaxed) => {
                    let _ = std::fs::remove_dir_all(&mount.dir);
                    let size = *budget.size.lock().unwrap_or_else(|e| e.into_inner());
                    events.push(Event::TooLarge {
                        zip: mount.zip,
                        size,
                        limit: budget.limit.unwrap_or(self.size_limit),
                        password,
                    });
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

/// `parent`（`None` なら OS の一時フォルダ）の下に、このプロセスの展開先を作って返す。
/// 同じ親に前の vw が残したものがあれば、先に消す。
///
/// 展開先は自分だけが読み書きできる権限で作る。Linux の `/tmp` はほかのユーザーと共有なので、
/// 普通の権限では展開した中身（パスワード付きのアーカイブの中身も）がほかのユーザーから読める。
/// 作るのは既にあれば失敗する `create_dir` で、ほかのユーザーが先に同じ名前（シンボリックリンク
/// を含む）を置いていたら、その名前は使わず番号を足した名前にする。
fn prepare_base(parent: Option<&Path>) -> PathBuf {
    let parent = parent.map_or_else(std::env::temp_dir, Path::to_path_buf);
    let root = own_root(&parent);
    let pid = std::process::id();
    remove_stale(&root, pid);
    let _ = std::fs::create_dir_all(&root);
    for n in 0..100 {
        let name = match n {
            0 => pid.to_string(),
            n => format!("{pid}-{n}"),
        };
        let base = root.join(name);
        match create_private_dir(&base) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // 作れない理由がほかにあるなら、展開するときに同じ理由で失敗し、そこで知らせる。
            _ => return base,
        }
    }
    root.join(pid.to_string())
}

/// `parent` の下の、展開先をまとめるフォルダ。`vw-archives` がシンボリックリンクか、ほかの
/// ユーザーの物なら使わず、ユーザーごとの名前にする。ほかのユーザーの物の中に作ると、その
/// ユーザーが展開先を差し替えて、展開した中身を好きな場所に書かせることができるため。
fn own_root(parent: &Path) -> PathBuf {
    let shared = parent.join(FOLDER_NAME);
    if is_usable_root(&shared) {
        return shared;
    }
    parent.join(format!("{FOLDER_NAME}-{}", user_id().unwrap_or(0)))
}

/// まだないか、自分の物の本物のフォルダなら使える。
fn is_usable_root(root: &Path) -> bool {
    match std::fs::symlink_metadata(root) {
        Err(_) => true,
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => false,
        Ok(m) => owner(&m).is_none_or(|uid| Some(uid) == user_id()),
    }
}

#[cfg(unix)]
fn owner(m: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    Some(m.uid())
}

#[cfg(not(unix))]
fn owner(_: &std::fs::Metadata) -> Option<u32> {
    None
}

/// このプロセスのユーザーの番号。libc を足さずに済むよう、/proc/self の持ち主で知る。
#[cfg(unix)]
fn user_id() -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

#[cfg(not(unix))]
fn user_id() -> Option<u32> {
    None
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
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
///
/// 書く量には上限を設ける。ZIP・7z は書いてある展開後の大きさを先に見て、空き容量や
/// `budget` の上限を超えるなら書き始めない。書いてある大きさは偽れるので、書く途中でもそれを
/// 超えたら止める。tar 系は書いてないので、空き容量と上限までで止める。
fn extract(
    archive: &Path,
    dir: &Path,
    password: Option<&str>,
    count: &Count,
    budget: &Budget,
) -> Result<usize, ArchiveError> {
    std::fs::create_dir_all(dir)?;
    let streamed = kind(archive).is_some_and(|k| k != Kind::ZipOr7z);
    let declared = if streamed {
        None
    } else {
        total_size(archive, password)
    };
    let free = free_space(dir);
    if let (Some(size), Some(free)) = (declared, free)
        && size > free
    {
        return Err(ArchiveError::Other(format!(
            "needs {} but only {} is free in {}",
            gib(size),
            gib(free),
            dir.display()
        )));
    }
    if let (Some(size), Some(limit)) = (declared, budget.limit)
        && size > limit
    {
        return Err(budget.refuse(Some(size)));
    }
    let total = if streamed {
        std::fs::metadata(archive).ok().map(|m| m.len())
    } else {
        declared
    };
    if let Some(n) = total {
        count.total.store(n as usize, Ordering::Relaxed);
    }
    // 先に見た大きさより多く書けたら、書いてある大きさが偽り。
    let room = [declared, free, budget.limit]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(u64::MAX);
    let left = AtomicU64::new(room);
    let read = streamed.then_some(&count.done);
    let result = each_entry(archive, password, read, |relative, data| {
        let out = dir.join(relative);
        let Some(data) = data else {
            return std::fs::create_dir_all(&out).map_err(|e| EntryError::Other(e.to_string()));
        };
        let target = out
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::File::create(&out));
        let mut target = match target {
            Ok(file) => Capped {
                inner: file,
                left: &left,
            },
            Err(e) => return Err(EntryError::Other(e.to_string())),
        };
        if streamed {
            copy_entry(data, &mut target)
        } else {
            let mut counted = Counted {
                inner: data,
                count: &count.done,
            };
            copy_entry(&mut counted, &mut target)
        }
    });
    if result.is_ok() || left.load(Ordering::Relaxed) != OVER {
        return result;
    }
    if declared.is_some() {
        Err(ArchiveError::Other(
            "the contents are larger than the archive says".into(),
        ))
    } else if budget.limit.is_some_and(|l| free.is_none_or(|f| l <= f)) {
        Err(budget.refuse(None))
    } else {
        Err(ArchiveError::Other(format!(
            "not enough free space in {}",
            dir.display()
        )))
    }
}

/// [`Capped`] が書ける量を超えたしるし。
const OVER: u64 = u64::MAX;

/// 残りの量（`left`）までしか書かない。超えたら `left` を [`OVER`] にして失敗する。
struct Capped<'a, W> {
    inner: W,
    left: &'a AtomicU64,
}

impl<W: std::io::Write> std::io::Write for Capped<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let left = self.left.load(Ordering::Relaxed);
        let n = buf.len() as u64;
        if n > left {
            self.left.store(OVER, Ordering::Relaxed);
            return Err(std::io::Error::other("extraction size limit reached"));
        }
        let written = self.inner.write(buf)?;
        self.left.store(left - written as u64, Ordering::Relaxed);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// バイト数を GB（1 GB = 2^30 バイト）で読める形にする。
pub fn gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
}

/// `dir` のあるドライブの空き容量。分からなければ `None`（そのときは空き容量で止めない）。
#[cfg(unix)]
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` は NUL で終わる文字列で、`stat` は statvfs が書き込む大きさがある。
    let ok = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } == 0;
    if !ok {
        return None;
    }
    // SAFETY: statvfs が成功したので書き込まれている。
    let stat = unsafe { stat.assume_init() };
    #[allow(clippy::unnecessary_cast)]
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(windows)]
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt as _;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    // SAFETY: `wide` は NUL で終わり、受け取る値は使わないものに null を渡している。
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } != 0;
    ok.then_some(free)
}

#[cfg(not(any(unix, windows)))]
fn free_space(_: &Path) -> Option<u64> {
    None
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

/// `archive` の中身を展開したときのバイト数の合計（アーカイブに書いてある値）。読めなければ `None`。
fn total_size(archive: &Path, password: Option<&str>) -> Option<u64> {
    let file = std::fs::File::open(archive).ok()?;
    if let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) {
        let sizes = (0..zip.len()).filter_map(|i| zip.by_index_raw(i).ok().map(|e| e.size()));
        return Some(sizes.fold(0u64, u64::saturating_add));
    }
    let key = password.map_or_else(sevenz_rust2::Password::empty, sevenz_rust2::Password::from);
    let reader = sevenz_rust2::ArchiveReader::open(archive, key).ok()?;
    Some(
        reader
            .archive()
            .files
            .iter()
            .map(|f| f.size)
            .fold(0u64, u64::saturating_add),
    )
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
                    return Err(ArchiveError::WrongPassword);
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
    target: &mut impl std::io::Write,
) -> Result<(), EntryError> {
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
///
/// 消すのは vw が付ける名前（プロセス番号、または「番号-n」）のものだけ。展開先の設定で選んだ
/// フォルダに、利用者がたまたま `vw-archives` という名前のフォルダを持っていても、その中の
/// 利用者のファイルは消さない。
fn remove_stale(root: &Path, own: u32) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(stale_pid) else {
            continue;
        };
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > STALE_AFTER);
        // Linux ではプロセスが動いているか /proc で分かるので、終わったものはすぐ消す。
        let dead = cfg!(target_os = "linux") && !Path::new("/proc").join(pid.to_string()).exists();
        if (old || dead) && pid != own {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// vw が付けた展開先の名前（「1234」か「1234-2」）なら、そのプロセス番号。
fn stale_pid(name: &str) -> Option<u32> {
    let (pid, n) = name.split_once('-').unwrap_or((name, "0"));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(pid) && digits(n))
        .then(|| pid.parse().ok())
        .flatten()
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
        assert!(archives.mount(&zip, None, false, || {}));
        assert!(
            !archives.mount(&zip, None, false, || {}),
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
                extract(&zip, &out, None, &Count::default(), &Budget::new(None)),
                Err(ArchiveError::NeedsPassword)
            );
            assert_eq!(
                extract(
                    &zip,
                    &out,
                    Some("wrong"),
                    &Count::default(),
                    &Budget::new(None)
                ),
                Err(ArchiveError::WrongPassword),
                "aes = {aes}"
            );
            assert_eq!(
                extract(
                    &zip,
                    &out,
                    Some("open sesame"),
                    &Count::default(),
                    &Budget::new(None)
                ),
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
        archives.mount(&zip, None, false, || {});
        let events = wait(&mut archives);
        assert!(
            matches!(&events[0], Event::NeedsPassword { wrong: false, .. }),
            "{events:?}"
        );
        archives.mount(&zip, Some("pw".into()), false, || {});
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
                &Budget::new(None),
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
        assert_eq!(
            extract(&archive, &out, None, &Count::default(), &Budget::new(None)),
            Ok(2)
        );
        assert_eq!(std::fs::read(out.join("sub/b.png")).unwrap(), b"B");
    }

    #[test]
    fn encrypted_seven_zip_needs_the_right_password() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("secret.7z");
        write_7z(&archive, &[("a.png", &[1; 4096])], Some("pw"));
        let out = tmp.path().join("out");
        assert_eq!(
            extract(&archive, &out, None, &Count::default(), &Budget::new(None)),
            Err(ArchiveError::NeedsPassword)
        );
        assert_eq!(
            extract(
                &archive,
                &out,
                Some("nope"),
                &Count::default(),
                &Budget::new(None)
            ),
            Err(ArchiveError::WrongPassword)
        );
        assert_eq!(
            extract(
                &archive,
                &out,
                Some("pw"),
                &Count::default(),
                &Budget::new(None)
            ),
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
        assert_eq!(
            extract(&zip, &out, None, &Count::default(), &Budget::new(None)).unwrap(),
            1
        );
        assert!(!tmp.path().join("escaped.txt").exists());
    }

    #[test]
    fn a_broken_zip_reports_an_error_and_leaves_nothing_behind() {
        let tmp = TempDir::new();
        let zip = tmp.path().join("broken.zip");
        std::fs::write(&zip, b"not a zip").unwrap();
        let mut archives = Archives::new(tmp.path().join("work"));
        archives.mount(&zip, None, false, || {});
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
        archives.mount(&zip, None, false, || {});
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
        archives.mount(&a, None, false, || {});
        wait(&mut archives);
        let second = tmp.path().join("second");
        archives.set_parent(Some(&second));
        archives.mount(&b, None, false, || {});
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
        archives.mount(&a, None, false, || {});
        wait(&mut archives);
        archives.set_parent(Some(&tmp.path().join("second")));
        archives.mount(&b, None, false, || {});
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
        archives.mount(&zip, None, false, || {});
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

        assert_eq!(
            extract(&archive, &out, None, &Count::default(), &Budget::new(None)),
            Ok(2)
        );
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
                extract(&archive, &out, None, &Count::default(), &Budget::new(None)),
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
            extract(
                &archive,
                &tmp.path().join("out"),
                None,
                &Count::default(),
                &Budget::new(None)
            ),
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

        assert_eq!(
            extract(&archive, &out, None, &Count::default(), &Budget::new(None)),
            Ok(1)
        );
        assert!(std::fs::symlink_metadata(out.join("evil")).is_err());
    }

    #[test]
    fn a_broken_tar_reports_an_error() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("broken.tar.gz");
        let gz = gzip(&tar_bytes(&[(b"a.png", &[7u8; 4096])]));
        std::fs::write(&archive, &gz[..gz.len() / 2]).unwrap();

        let result = extract(
            &archive,
            &tmp.path().join("out"),
            None,
            &Count::default(),
            &Budget::new(None),
        );
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
                extract(&archive, &out, None, &Count::default(), &Budget::new(None)),
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
        extract(
            &archive,
            &tmp.path().join("out"),
            None,
            &count,
            &Budget::new(None),
        )
        .unwrap();

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

        assert_eq!(
            extract(&archive, &out, None, &Count::default(), &Budget::new(None)),
            Ok(1)
        );
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

        assert_eq!(
            extract(&archive, &out, None, &Count::default(), &Budget::new(None)),
            Ok(1)
        );
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

    #[test]
    fn only_folders_named_like_a_process_are_cleaned_up() {
        assert_eq!(stale_pid("1234"), Some(1234));
        assert_eq!(stale_pid("1234-2"), Some(1234));
        assert_eq!(stale_pid("photos"), None);
        assert_eq!(stale_pid("1234-old"), None);
        assert_eq!(stale_pid("-2"), None);
    }

    #[test]
    fn user_files_in_a_folder_named_vw_archives_are_kept() {
        let tmp = TempDir::new();
        let mine = tmp.path().join(FOLDER_NAME).join("photos");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::write(mine.join("a.jpg"), b"x").unwrap();
        // 動いていないプロセスの番号の残り物は消える。
        let dead = tmp.path().join(FOLDER_NAME).join("4000000000");
        std::fs::create_dir_all(&dead).unwrap();
        let archives = Archives::in_dir(Some(tmp.path()));
        assert!(mine.join("a.jpg").is_file());
        if cfg!(target_os = "linux") {
            assert!(!dead.exists());
        }
        drop(archives);
        assert!(mine.join("a.jpg").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn extraction_folder_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = TempDir::new();
        let archives = Archives::in_dir(Some(tmp.path()));
        let mode = std::fs::metadata(archives.base())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn a_name_taken_in_advance_is_not_used() {
        let tmp = TempDir::new();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let root = tmp.path().join(FOLDER_NAME);
        std::fs::create_dir(&root).unwrap();
        let taken = root.join(std::process::id().to_string());
        std::os::unix::fs::symlink(&elsewhere, &taken).unwrap();
        let archives = Archives::in_dir(Some(tmp.path()));
        assert_ne!(archives.base(), taken);
        assert!(archives.base().starts_with(&root));
        assert!(
            !std::fs::symlink_metadata(archives.base())
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_shared_folder_is_not_used() {
        let tmp = TempDir::new();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, tmp.path().join(FOLDER_NAME)).unwrap();
        let archives = Archives::in_dir(Some(tmp.path()));
        assert!(!archives.base().starts_with(tmp.path().join(FOLDER_NAME)));
        assert!(archives.base().starts_with(tmp.path()));
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
    }

    #[test]
    fn an_archive_over_the_size_limit_is_extracted_only_when_allowed() {
        let tmp = TempDir::new();
        let zip = tmp.path().join("big.zip");
        write_zip(&zip, &[("a.png", &[1; 600]), ("b.png", &[2; 600])]);
        let mut archives = Archives::new(tmp.path().join("x"));
        archives.set_size_limit(1000);

        archives.mount(&zip, None, false, || {});
        let events = wait(&mut archives);
        assert!(
            matches!(
                &events[0],
                Event::TooLarge {
                    size: Some(1200),
                    limit: 1000,
                    password: None,
                    ..
                }
            ),
            "{events:?}"
        );
        assert!(std::fs::read_dir(archives.base()).unwrap().next().is_none());

        archives.mount(&zip, None, true, || {});
        let events = wait(&mut archives);
        assert!(
            matches!(&events[0], Event::Mounted { files: 2, .. }),
            "{events:?}"
        );
    }

    #[test]
    fn a_tar_growing_over_the_size_limit_is_stopped_and_removed() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("big.tar.gz");
        let tar = tar_bytes(&[(b"a.png", &[0u8; 4096]), (b"b.png", &[0u8; 4096])]);
        std::fs::write(&archive, gzip(&tar)).unwrap();
        let mut archives = Archives::new(tmp.path().join("x"));
        archives.set_size_limit(5000);

        archives.mount(&archive, None, false, || {});
        let events = wait(&mut archives);
        assert!(
            matches!(
                &events[0],
                Event::TooLarge {
                    size: None,
                    limit: 5000,
                    ..
                }
            ),
            "{events:?}"
        );
        assert!(std::fs::read_dir(archives.base()).unwrap().next().is_none());
    }

    #[test]
    fn writing_stops_when_the_bytes_left_run_out() {
        let left = AtomicU64::new(10);
        let mut out = Capped {
            inner: Vec::new(),
            left: &left,
        };
        assert!(copy_entry(&mut &[7u8; 8][..], &mut out).is_ok());
        assert!(copy_entry(&mut &[7u8; 8][..], &mut out).is_err());
        assert_eq!(out.inner.len(), 8);
        assert_eq!(left.load(Ordering::Relaxed), OVER);
    }
}
