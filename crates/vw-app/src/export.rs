//! ラベルによる絞り込みと抜き出し（コピー、CSV / JSONL の一覧）。

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use vw_core::Registry;

use crate::labels::{Flag, Label, LabelStore};
use crate::sort::SortOrder;
use crate::tree;

/// 表示や抜き出しの対象にする画像の条件。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    /// このフラグが付いた画像。`None` はフラグのない画像。
    Flag(Option<Flag>),
    Tag(String),
}

impl Filter {
    /// `label` の画像が条件に合うか。`folder` は画像のあるフォルダ自身のラベルで、
    /// `with_folder_flag` のときはフォルダのフラグを中の画像すべてに付いているものとして扱う。
    pub fn matches(&self, label: &Label, folder: &Label, with_folder_flag: bool) -> bool {
        let flag = match (label.flag, with_folder_flag) {
            (None, true) => folder.flag,
            (flag, _) => flag,
        };
        match self {
            Filter::All => true,
            Filter::Flag(want) => flag == *want,
            Filter::Tag(tag) => label.tags.contains(tag),
        }
    }

    pub fn is_all(&self) -> bool {
        *self == Filter::All
    }
}

/// 絞り込みの条件と、ラベルの読み先をまとめたもの。
pub struct Matcher<'a> {
    pub filter: &'a Filter,
    pub with_folder_flag: bool,
    pub labels: &'a LabelStore,
}

impl Matcher<'_> {
    pub fn matches(&self, path: &Path) -> bool {
        if self.filter.is_all() {
            return true;
        }
        let folder = path
            .parent()
            .map(|d| self.labels.folder(d))
            .unwrap_or_default();
        self.filter
            .matches(&self.labels.get(path), &folder, self.with_folder_flag)
    }
}

/// `root` の画像のうち条件に合うものを、ツリーと同じ順で集める。`recursive` ならサブフォルダも辿る。
pub fn collect(
    root: &Path,
    recursive: bool,
    registry: &Registry,
    order: SortOrder,
    matcher: &Matcher,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let (dirs, files) = tree::list_folder(&dir, registry, order);
        out.extend(files.into_iter().filter(|f| matcher.matches(f)));
        if recursive {
            // 深さ優先で、ツリーで上にあるフォルダから辿るよう逆順に積む。
            stack.extend(dirs.into_iter().rev());
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFormat {
    Csv,
    Jsonl,
}

/// 一覧の 1 行。CSV は path, flag, tags（タグは ; 区切り）の 3 列。
pub fn list_line(format: ListFormat, path: &Path, label: &Label) -> String {
    let flag = match label.flag {
        Some(Flag::Good) => "good",
        Some(Flag::Bad) => "bad",
        None => "",
    };
    match format {
        ListFormat::Csv => {
            let tags = label.tags.iter().cloned().collect::<Vec<_>>().join(";");
            [path.to_string_lossy().as_ref(), flag, &tags]
                .map(csv_field)
                .join(",")
        }
        ListFormat::Jsonl => serde_json::json!({
            "path": path.to_string_lossy(),
            "flag": label.flag.map(|_| flag),
            "tags": label.tags,
        })
        .to_string(),
    }
}

/// CSV の 1 項目。区切りや引用符、改行を含むときだけ引用符で囲む。
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// 一覧をファイルに書く。
///
/// CSV に BOM を付けないのは、pandas などで読むと先頭の列名に BOM が混ざり、機械学習の
/// 前処理でつまずくため。Excel で開くと日本語のパスが化けるので、そのときは JSONL か
/// Excel の「データの取り込み」（UTF-8 を指定）を使う。
pub fn write_list(
    out: &Path,
    format: ListFormat,
    items: &[(PathBuf, Label)],
) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", out.display());
    let mut w = std::io::BufWriter::new(std::fs::File::create(out).map_err(fail)?);
    if format == ListFormat::Csv {
        writeln!(w, "path,flag,tags").map_err(fail)?;
    }
    for (path, label) in items {
        writeln!(w, "{}", list_line(format, path, label)).map_err(fail)?;
    }
    w.flush().map_err(fail)
}

/// コピー先のパス。`keep_structure` なら `root` からの相対パスを保ち、そうでなければ
/// `dest` の直下に置く。直下に置くときに名前が重なったら `name-1.png` のように番号を付ける。
pub fn copy_target(
    root: &Path,
    file: &Path,
    dest: &Path,
    keep_structure: bool,
    taken: &mut HashSet<PathBuf>,
) -> PathBuf {
    if keep_structure {
        if let Ok(rel) = file.strip_prefix(root) {
            return dest.join(rel);
        }
    }
    let name = file.file_name().unwrap_or_default();
    let mut target = dest.join(name);
    let stem = Path::new(name)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let ext = Path::new(name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut n = 1;
    while taken.contains(&target) || target.exists() {
        target = dest.join(format!("{stem}-{n}{ext}"));
        n += 1;
    }
    taken.insert(target.clone());
    target
}

/// 別スレッドでのコピーの進み具合。
pub struct CopyJob {
    pub total: usize,
    pub done: Arc<AtomicUsize>,
    pub failed: Arc<std::sync::Mutex<Vec<String>>>,
    pub finished: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
}

impl CopyJob {
    /// `pairs`（元, 先）をコピーし始める。大きな画像を何百枚もコピーすると数十秒かかるので、
    /// 画面を止めないよう別スレッドで行う。既にあるファイルは上書きしない。
    pub fn start(pairs: Vec<(PathBuf, PathBuf)>, repaint: impl Fn() + Send + 'static) -> Self {
        let job = Self {
            total: pairs.len(),
            done: Arc::default(),
            failed: Arc::default(),
            finished: Arc::default(),
            cancel: Arc::default(),
        };
        let (done, failed, finished, cancel) = (
            job.done.clone(),
            job.failed.clone(),
            job.finished.clone(),
            job.cancel.clone(),
        );
        std::thread::spawn(move || {
            for (from, to) in pairs {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(e) = copy_one(&from, &to) {
                    failed.lock().unwrap().push(e);
                }
                done.fetch_add(1, Ordering::Relaxed);
                repaint();
            }
            finished.store(true, Ordering::Relaxed);
            repaint();
        });
        job
    }
}

fn copy_one(from: &Path, to: &Path) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{} -> {}: {e}", from.display(), to.display());
    if to.exists() {
        return Err(format!("{}: already exists", to.display()));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(fail)?;
    }
    std::fs::copy(from, to).map(|_| ()).map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{png_registry, TempDir};

    fn label(flag: Option<Flag>, tags: &[&str]) -> Label {
        Label {
            flag,
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn flag_filter_matches_only_that_flag() {
        let good = label(Some(Flag::Good), &[]);
        let none = Label::default();
        let f = Filter::Flag(Some(Flag::Good));
        assert!(f.matches(&good, &none, false));
        assert!(!f.matches(&none, &none, false));
        assert!(Filter::Flag(None).matches(&none, &none, false));
    }

    #[test]
    fn folder_flag_counts_for_unflagged_images_when_asked() {
        let folder = label(Some(Flag::Good), &[]);
        let f = Filter::Flag(Some(Flag::Good));
        assert!(f.matches(&Label::default(), &folder, true));
        assert!(!f.matches(&Label::default(), &folder, false));
        assert!(!f.matches(&label(Some(Flag::Bad), &[]), &folder, true));
    }

    #[test]
    fn tag_filter_matches_images_with_that_tag() {
        let f = Filter::Tag("cat".into());
        assert!(f.matches(&label(None, &["cat", "dog"]), &Label::default(), false));
        assert!(!f.matches(&label(None, &["dog"]), &Label::default(), false));
    }

    #[test]
    fn csv_line_quotes_fields_with_commas() {
        let line = list_line(
            ListFormat::Csv,
            Path::new("/a,b/x.png"),
            &label(Some(Flag::Good), &["cat", "dog"]),
        );
        assert_eq!(line, "\"/a,b/x.png\",good,cat;dog");
    }

    #[test]
    fn jsonl_line_has_path_flag_and_tags() {
        let line = list_line(
            ListFormat::Jsonl,
            Path::new("/d/x.png"),
            &label(None, &["cat"]),
        );
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["path"], "/d/x.png");
        assert!(v["flag"].is_null());
        assert_eq!(v["tags"][0], "cat");
    }

    #[test]
    fn flattened_copies_get_numbered_when_names_collide() {
        let dest = TempDir::new();
        let mut taken = HashSet::new();
        let root = Path::new("/src");
        let a = copy_target(
            root,
            Path::new("/src/a/x.png"),
            dest.path(),
            false,
            &mut taken,
        );
        let b = copy_target(
            root,
            Path::new("/src/b/x.png"),
            dest.path(),
            false,
            &mut taken,
        );
        assert_eq!(a, dest.path().join("x.png"));
        assert_eq!(b, dest.path().join("x-1.png"));
    }

    #[test]
    fn copies_keep_the_folder_structure_when_asked() {
        let mut taken = HashSet::new();
        let t = copy_target(
            Path::new("/src"),
            Path::new("/src/a/x.png"),
            Path::new("/out"),
            true,
            &mut taken,
        );
        assert_eq!(t, Path::new("/out/a/x.png"));
    }

    #[test]
    fn collect_walks_subfolders_and_applies_the_filter() {
        let dir = TempDir::new();
        let sub = dir.mkdir("sub");
        let top = dir.touch("top.png");
        dir.touch("other.png");
        let deep = sub.join("deep.png");
        std::fs::write(&deep, b"").unwrap();
        let mut labels = LabelStore::new(None);
        labels.toggle_flag(&top, Flag::Good).unwrap();
        labels.toggle_flag(&deep, Flag::Good).unwrap();
        let filter = Filter::Flag(Some(Flag::Good));
        let matcher = Matcher {
            filter: &filter,
            with_folder_flag: false,
            labels: &labels,
        };
        let order = SortOrder::default();
        let flat = collect(dir.path(), false, &png_registry(), order, &matcher);
        assert_eq!(flat, std::slice::from_ref(&top));
        let all = collect(dir.path(), true, &png_registry(), order, &matcher);
        assert_eq!(all, [top, deep]);
    }
}
