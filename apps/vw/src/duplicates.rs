//! 重複データ探索の比べる側。データセットのフォルダ（`dataset.roots`）以下のファイルの大きさを
//! 先に全部集めておき、起点の中のファイルやフォルダと同じものを探す。
//!
//! 中身を読むのは大きさが合ったものだけで、まず先頭の 64 KiB、それも合えば全体の SHA-256 を
//! 比べる。同じ大きさのファイルが多い形式（圧縮しない DICOM など）でも、先頭で違いが分かる
//! ものは全体を読まずに済む。

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::SystemTime;

use regex::Regex;
use sha2::{Digest, Sha256};

/// 先に比べる先頭の大きさ。
const HEAD_BYTES: u64 = 64 * 1024;
/// 読むときの 1 回の大きさ。止める頼みはこの区切りごとに見る。
const CHUNK: usize = 1024 * 1024;

/// 比べないものと、小さすぎて比べないファイルの決まり。
#[derive(Debug, Clone)]
pub struct Rules {
    ignore: Option<Regex>,
    pub min_file_bytes: u64,
}

impl Rules {
    /// `ignore_names` は比べないファイル名の正規表現（空なら何も外さない）。
    pub fn new(ignore_names: &str, min_file_bytes: u64) -> Result<Self, regex::Error> {
        let ignore = if ignore_names.is_empty() {
            None
        } else {
            Some(Regex::new(ignore_names)?)
        };
        Ok(Self {
            ignore,
            min_file_bytes,
        })
    }

    /// 比べないファイルか。vw が書くファイル（ラベル、AI タグ、検索カタログ）は、
    /// タグを付けただけで同じものでなくなるのを避けるため、いつも外す。
    pub fn skips_file(&self, name: &str) -> bool {
        name.starts_with(".vw-")
            || name.starts_with(crate::catalog::FILE_NAME.trim_end_matches(".json"))
            || self.ignore.as_ref().is_some_and(|r| r.is_match(name))
    }

    pub fn skips_folder(&self, name: &str) -> bool {
        vw_tree::is_recycle_bin(name)
    }
}

/// 止める頼みと、進み具合。
#[derive(Debug, Default)]
pub struct Progress {
    pub cancel: AtomicBool,
    /// 一覧に入れたファイルの数。
    pub listed: AtomicUsize,
    /// 中身を読んだファイルの数。
    pub hashed: AtomicUsize,
    pub hashed_bytes: AtomicU64,
}

impl Progress {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// 止めた。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    path: PathBuf,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Folder {
    path: PathBuf,
    /// このフォルダの下（深さを問わない）のファイル（[`Index::files`] の範囲）。
    files: Range<usize>,
    bytes: u64,
}

/// データセット側のファイルとフォルダの一覧。中身は読まず、パスと大きさだけを持つ。
#[derive(Debug, Default)]
pub struct Index {
    /// 深さ優先の順。フォルダの下のファイルが続いた範囲になるようにしている。
    files: Vec<Listed>,
    folders: Vec<Folder>,
    by_size: HashMap<u64, Vec<usize>>,
    /// （ファイルの数、合計の大きさ）ごとのフォルダ。
    by_shape: HashMap<(usize, u64), Vec<usize>>,
    /// 読めなかったフォルダ。
    pub problems: Vec<String>,
}

impl Index {
    /// `roots` 以下を全部集める。ほかの場所の中にある場所は、同じファイルを二度数えないよう外す。
    pub fn build(roots: &[PathBuf], rules: &Rules, progress: &Progress) -> Result<Self, Cancelled> {
        let mut index = Self::default();
        for root in outermost(roots) {
            if !root.is_dir() {
                index
                    .problems
                    .push(format!("Not a folder: {}", root.display()));
                continue;
            }
            index.walk(&root, rules, progress)?;
        }
        for (i, f) in index.files.iter().enumerate() {
            index.by_size.entry(f.size).or_default().push(i);
        }
        for (i, f) in index.folders.iter().enumerate() {
            if !f.files.is_empty() {
                index
                    .by_shape
                    .entry((f.files.len(), f.bytes))
                    .or_default()
                    .push(i);
            }
        }
        Ok(index)
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    fn walk(&mut self, dir: &Path, rules: &Rules, progress: &Progress) -> Result<u64, Cancelled> {
        if progress.cancelled() {
            return Err(Cancelled);
        }
        let start = self.files.len();
        let at = self.folders.len();
        self.folders.push(Folder {
            path: dir.to_path_buf(),
            files: start..start,
            bytes: 0,
        });
        let (files, folders) = match list(dir, rules) {
            Ok(found) => found,
            Err(e) => {
                self.problems
                    .push(format!("Cannot read {}: {e}", dir.display()));
                (Vec::new(), Vec::new())
            }
        };
        let mut bytes = 0;
        for (path, size) in files {
            bytes += size;
            self.files.push(Listed { path, size });
            progress.listed.fetch_add(1, Ordering::Relaxed);
        }
        for sub in folders {
            bytes += self.walk(&sub, rules, progress)?;
        }
        let folder = &mut self.folders[at];
        folder.files = start..self.files.len();
        folder.bytes = bytes;
        Ok(bytes)
    }
}

/// ほかの場所の中にない場所だけ。
fn outermost(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in roots {
        let inner = roots
            .iter()
            .any(|other| other != root && root.starts_with(other));
        if !inner && !out.contains(root) {
            out.push(root.clone());
        }
    }
    out
}

/// フォルダの直下のファイル（パスと大きさ）とフォルダ。
type Listing = (Vec<(PathBuf, u64)>, Vec<PathBuf>);

/// フォルダの直下の、比べるファイル（パスと大きさ）とフォルダ。名前の順。
/// フォルダへのリンクはたどらない（ループして終わらなくなることがあるため）。
fn list(dir: &Path, rules: &Rules) -> io::Result<Listing> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    let mut files = Vec::new();
    let mut folders = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if !rules.skips_folder(&name) {
                folders.push(path);
            }
            continue;
        }
        if rules.skips_file(&name) {
            continue;
        }
        if let Ok(m) = fs::metadata(&path)
            && m.is_file()
        {
            files.push((path, m.len()));
        }
    }
    Ok((files, folders))
}

/// フォルダの下の比べるファイルを全部、[`Index`] と同じ順で。
fn listing(
    dir: &Path,
    rules: &Rules,
    progress: &Progress,
) -> Result<Vec<(PathBuf, u64)>, Cancelled> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    // Index::walk と同じ順（ファイル、次にフォルダを名前の順に深さ優先）にするため、
    // フォルダは逆順に積む。
    while let Some(d) = stack.pop() {
        if progress.cancelled() {
            return Err(Cancelled);
        }
        let (files, folders) = list(&d, rules).unwrap_or_default();
        out.extend(files);
        stack.extend(folders.into_iter().rev());
    }
    Ok(out)
}

/// 調べた結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 同じものがある場所（なければ空）。
    Found(Vec<PathBuf>),
    /// ファイルがないフォルダ。
    Empty,
    /// 設定の大きさより小さいファイル。
    Small,
    /// 比べないファイル（[`Rules::skips_file`]）。
    Skipped,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Known {
    head: Option<[u8; 32]>,
    full: Option<[u8; 32]>,
}

/// 計算したハッシュ。ウインドウを開いている間だけ覚えておき、ファイルは書かない。
/// 大きさか更新日時が変わったファイルは計算し直す。どちらも変えずに中身だけ書き換えたものは
/// 見分けられないが、毎回中身を読み直して確かめると、覚えておく意味がなくなるため。
#[derive(Debug, Default)]
pub struct Hashes {
    known: Mutex<HashMap<PathBuf, (Stamp, Known)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Head,
    Full,
}

impl Hashes {
    fn get(&self, path: &Path, part: Part, progress: &Progress) -> io::Result<[u8; 32]> {
        let m = fs::metadata(path)?;
        let stamp = Stamp {
            size: m.len(),
            modified: m.modified().ok(),
        };
        let known = {
            let map = self.known.lock().unwrap_or_else(|e| e.into_inner());
            map.get(path)
                .filter(|(s, _)| *s == stamp)
                .map(|(_, k)| *k)
                .unwrap_or_default()
        };
        let cached = match part {
            Part::Head => known
                .head
                .or(known.full.filter(|_| stamp.size <= HEAD_BYTES)),
            Part::Full => known
                .full
                .or(known.head.filter(|_| stamp.size <= HEAD_BYTES)),
        };
        if let Some(h) = cached {
            return Ok(h);
        }
        let limit = match part {
            Part::Head => HEAD_BYTES,
            Part::Full => u64::MAX,
        };
        let hash = digest(path, limit, progress)?;
        let mut map = self.known.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map
            .entry(path.to_path_buf())
            .or_insert((stamp, Known::default()));
        if entry.0 != stamp {
            *entry = (stamp, Known::default());
        }
        match part {
            Part::Head => entry.1.head = Some(hash),
            Part::Full => entry.1.full = Some(hash),
        }
        Ok(hash)
    }

    /// 大きさが同じ 2 つのファイルの中身が同じか。
    fn same(&self, a: &Path, b: &Path, progress: &Progress) -> io::Result<bool> {
        if self.get(a, Part::Head, progress)? != self.get(b, Part::Head, progress)? {
            return Ok(false);
        }
        Ok(self.get(a, Part::Full, progress)? == self.get(b, Part::Full, progress)?)
    }
}

fn digest(path: &Path, limit: u64, progress: &Progress) -> io::Result<[u8; 32]> {
    let mut file = fs::File::open(path)?.take(limit);
    let mut hasher = Sha256::new();
    let mut buf = vec![0; CHUNK];
    loop {
        if progress.cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        progress.hashed_bytes.fetch_add(n as u64, Ordering::Relaxed);
    }
    progress.hashed.fetch_add(1, Ordering::Relaxed);
    Ok(hasher.finalize().into())
}

/// 探すときの前提。
pub struct Search<'a> {
    pub index: &'a Index,
    pub hashes: &'a Hashes,
    pub rules: &'a Rules,
    pub progress: &'a Progress,
    /// この中（またはこれ自身）は見つかったものに数えない（起点）。
    pub origin: &'a Path,
}

impl Search<'_> {
    fn outside_origin(&self, path: &Path) -> bool {
        !path.starts_with(self.origin)
    }

    /// `path`（ファイルかフォルダ）と同じものを探す。
    pub fn find(&self, path: &Path) -> Result<Outcome, Cancelled> {
        let result = if path.is_dir() {
            self.find_folder(path)
        } else {
            self.find_file(path)
        };
        match result {
            Err(_) if self.progress.cancelled() => Err(Cancelled),
            Err(e) => Ok(Outcome::Failed(e.to_string())),
            Ok(outcome) => Ok(outcome),
        }
    }

    fn find_file(&self, path: &Path) -> io::Result<Outcome> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if self.rules.skips_file(&name) {
            return Ok(Outcome::Skipped);
        }
        let size = fs::metadata(path)?.len();
        if size < self.rules.min_file_bytes {
            return Ok(Outcome::Small);
        }
        let mut found = Vec::new();
        for &i in self.index.by_size.get(&size).into_iter().flatten() {
            let other = &self.index.files[i].path;
            if !self.outside_origin(other) || other == path {
                continue;
            }
            // 消されたり読めなかったりする相手は、同じではないものとして先へ進む。
            if self
                .hashes
                .same(path, other, self.progress)
                .unwrap_or(false)
            {
                found.push(other.clone());
            }
            if self.progress.cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
        }
        Ok(Outcome::Found(found))
    }

    fn find_folder(&self, path: &Path) -> io::Result<Outcome> {
        let mine = listing(path, self.rules, self.progress)
            .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "cancelled"))?;
        if mine.is_empty() {
            return Ok(Outcome::Empty);
        }
        let bytes = mine.iter().map(|(_, s)| s).sum();
        let mut found = Vec::new();
        for &i in self
            .index
            .by_shape
            .get(&(mine.len(), bytes))
            .into_iter()
            .flatten()
        {
            let folder = &self.index.folders[i];
            if !self.outside_origin(&folder.path) || folder.path == path {
                continue;
            }
            if self.same_folder(path, &mine, folder)? {
                found.push(folder.path.clone());
            }
        }
        Ok(Outcome::Found(found))
    }

    /// 中の相対パス・大きさ・中身が全部同じか。フォルダの名前そのものは問わない。
    fn same_folder(
        &self,
        path: &Path,
        mine: &[(PathBuf, u64)],
        other: &Folder,
    ) -> io::Result<bool> {
        let theirs = &self.index.files[other.files.clone()];
        let shaped = mine.iter().zip(theirs).all(|((p, s), t)| {
            *s == t.size && p.strip_prefix(path).ok() == t.path.strip_prefix(&other.path).ok()
        });
        if !shaped {
            return Ok(false);
        }
        for ((p, _), t) in mine.iter().zip(theirs) {
            if self.progress.cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            match self.hashes.same(p, &t.path, self.progress) {
                Ok(true) => {}
                Ok(false) => return Ok(false),
                // 相手が読めないなら同じとは言えない。自分が読めないのは失敗として出す。
                Err(e) if fs::metadata(p).is_err() => return Err(e),
                Err(_) => return Ok(false),
            }
        }
        Ok(true)
    }
}

/// 起点がフォルダのときに左に並べる、直下の比べるファイルとフォルダ（フォルダが先、名前の順）。
pub fn children(dir: &Path, rules: &Rules) -> io::Result<Vec<PathBuf>> {
    let (files, mut folders) = list(dir, rules)?;
    folders.extend(files.into_iter().map(|(p, _)| p));
    Ok(folders)
}

/// フォルダなら下の比べるファイルの合計、ファイルならその大きさ。
pub fn size_of(path: &Path, rules: &Rules) -> u64 {
    if path.is_dir() {
        let progress = Progress::default();
        listing(path, rules, &progress)
            .map(|l| l.iter().map(|(_, s)| s).sum())
            .unwrap_or(0)
    } else {
        fs::metadata(path).map_or(0, |m| m.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn rules() -> Rules {
        Rules::new(r"^(Thumbs\.db|desktop\.ini|\.DS_Store)$", 0).unwrap()
    }

    fn find(t: &TempDir, roots: &[&str], origin: &str, item: &str) -> Outcome {
        let rules = rules();
        let progress = Progress::default();
        let roots: Vec<PathBuf> = roots.iter().map(|r| t.path().join(r)).collect();
        let index = Index::build(&roots, &rules, &progress).unwrap();
        let hashes = Hashes::default();
        let origin = t.path().join(origin);
        Search {
            index: &index,
            hashes: &hashes,
            rules: &rules,
            progress: &progress,
            origin: &origin,
        }
        .find(&t.path().join(item))
        .unwrap()
    }

    fn found(t: &TempDir, names: &[&str]) -> Outcome {
        Outcome::Found(names.iter().map(|n| t.path().join(n)).collect())
    }

    #[test]
    fn file_with_the_same_content_is_found_under_another_name() {
        let t = TempDir::new();
        write(&t, "in/a.png", b"same bytes");
        write(&t, "sets/x/renamed.png", b"same bytes");
        write(&t, "sets/x/other.png", b"diff bytes");
        assert_eq!(
            find(&t, &["sets"], "in", "in/a.png"),
            found(&t, &["sets/x/renamed.png"])
        );
    }

    #[test]
    fn files_that_differ_after_the_first_64_kib_are_not_the_same() {
        let t = TempDir::new();
        let mut a = vec![7; 100_000];
        write(&t, "in/a.dcm", &a);
        a[90_000] = 8;
        write(&t, "sets/b.dcm", &a);
        assert_eq!(find(&t, &["sets"], "in", "in/a.dcm"), found(&t, &[]));
    }

    #[test]
    fn folder_with_the_same_tree_and_contents_is_found_whatever_its_name() {
        let t = TempDir::new();
        for root in ["in/day1", "sets/A/copy"] {
            write(&t, &format!("{root}/1.png"), b"one");
            write(&t, &format!("{root}/sub/2.png"), b"two");
        }
        assert_eq!(
            find(&t, &["sets"], "in", "in/day1"),
            found(&t, &["sets/A/copy"])
        );
    }

    #[test]
    fn folder_whose_file_has_another_relative_path_is_not_the_same() {
        let t = TempDir::new();
        write(&t, "in/day1/1.png", b"one");
        write(&t, "in/day1/sub/2.png", b"two");
        write(&t, "sets/day1/1.png", b"one");
        write(&t, "sets/day1/other/2.png", b"two");
        assert_eq!(find(&t, &["sets"], "in", "in/day1"), found(&t, &[]));
    }

    #[test]
    fn folder_with_one_changed_file_is_not_the_same() {
        let t = TempDir::new();
        write(&t, "in/day1/1.png", b"one");
        write(&t, "sets/day1/1.png", b"ONE");
        assert_eq!(find(&t, &["sets"], "in", "in/day1"), found(&t, &[]));
    }

    #[test]
    fn labels_catalogs_and_ignored_names_do_not_make_folders_differ() {
        let t = TempDir::new();
        write(&t, "in/day1/1.png", b"one");
        write(&t, "in/day1/.vw-labels.json", b"{\"flags\":1}");
        write(&t, "sets/day1/1.png", b"one");
        write(&t, "sets/day1/vw-search-catalog.json", b"{}");
        write(&t, "sets/day1/Thumbs.db", b"thumbs");
        write(&t, "sets/day1/$RECYCLE.BIN/x.png", b"gone");
        assert_eq!(
            find(&t, &["sets"], "in", "in/day1"),
            found(&t, &["sets/day1"])
        );
    }

    #[test]
    fn nothing_inside_the_origin_counts_as_found() {
        let t = TempDir::new();
        write(&t, "sets/in/a.png", b"same");
        write(&t, "sets/in/copy/a.png", b"same");
        write(&t, "sets/old/a.png", b"same");
        assert_eq!(
            find(&t, &["sets"], "sets/in", "sets/in/a.png"),
            found(&t, &["sets/old/a.png"])
        );
    }

    #[test]
    fn origin_folder_inside_a_dataset_finds_its_copy_but_not_itself() {
        let t = TempDir::new();
        write(&t, "sets/in/a.png", b"same");
        write(&t, "sets/old/in/a.png", b"same");
        assert_eq!(
            find(&t, &["sets"], "sets/in", "sets/in"),
            found(&t, &["sets/old/in"])
        );
    }

    #[test]
    fn empty_folder_is_reported_as_empty() {
        let t = TempDir::new();
        t.mkdir("in/empty");
        t.mkdir("sets/empty");
        assert_eq!(find(&t, &["sets"], "in", "in/empty"), Outcome::Empty);
    }

    #[test]
    fn small_files_are_not_compared() {
        let t = TempDir::new();
        let a = write(&t, "in/a.txt", b"x");
        write(&t, "sets/a.txt", b"x");
        let rules = Rules::new("", 1024).unwrap();
        let progress = Progress::default();
        let index = Index::build(&[t.path().join("sets")], &rules, &progress).unwrap();
        let search = Search {
            index: &index,
            hashes: &Hashes::default(),
            rules: &rules,
            progress: &progress,
            origin: &t.path().join("in"),
        };
        assert_eq!(search.find(&a).unwrap(), Outcome::Small);
    }

    #[test]
    fn nested_dataset_roots_list_each_file_once() {
        let t = TempDir::new();
        write(&t, "sets/inner/a.png", b"a");
        let progress = Progress::default();
        let roots = [t.path().join("sets"), t.path().join("sets/inner")];
        let index = Index::build(&roots, &rules(), &progress).unwrap();
        assert_eq!(index.file_count(), 1);
    }

    #[test]
    fn a_file_rewritten_with_a_new_modified_time_is_hashed_again() {
        let t = TempDir::new();
        let a = write(&t, "a", b"first");
        let b = write(&t, "b", b"first");
        let hashes = Hashes::default();
        let progress = Progress::default();
        assert!(hashes.same(&a, &b, &progress).unwrap());
        fs::write(&b, b"other").unwrap();
        // 書き直しが同じ時刻の目盛りに収まるファイルシステムもあるので、時刻ははっきりずらす。
        let later = SystemTime::now() + std::time::Duration::from_secs(10);
        fs::File::options()
            .write(true)
            .open(&b)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(!hashes.same(&a, &b, &progress).unwrap());
    }

    #[test]
    fn known_hashes_are_not_read_again() {
        let t = TempDir::new();
        let a = write(&t, "a", b"same");
        let b = write(&t, "b", b"same");
        let hashes = Hashes::default();
        let progress = Progress::default();
        hashes.same(&a, &b, &progress).unwrap();
        let read = progress.hashed.load(Ordering::Relaxed);
        hashes.same(&a, &b, &progress).unwrap();
        assert_eq!(progress.hashed.load(Ordering::Relaxed), read);
    }

    #[test]
    fn children_list_folders_first_then_files_by_name() {
        let t = TempDir::new();
        write(&t, "in/b.png", b"b");
        write(&t, "in/a.png", b"a");
        write(&t, "in/z/1.png", b"1");
        write(&t, "in/.vw-labels.json", b"{}");
        let names: Vec<String> = children(&t.path().join("in"), &rules())
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["z", "a.png", "b.png"]);
    }

    #[test]
    fn cancelling_stops_building_the_index() {
        let t = TempDir::new();
        write(&t, "sets/a.png", b"a");
        let progress = Progress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        assert!(Index::build(&[t.path().join("sets")], &rules(), &progress).is_err());
    }
}
