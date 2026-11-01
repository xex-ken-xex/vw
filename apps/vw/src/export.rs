//! ラベルによる絞り込みと抜き出し（コピー、CSV / JSONL の一覧）。

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use vw_core::Registry;

use crate::archive::{self, ArchiveError, EntryError};
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
    /// `label` の画像が条件に合うか。
    pub fn matches(&self, label: &Label) -> bool {
        match self {
            Filter::All => true,
            Filter::Flag(want) => label.flag == *want,
            Filter::Tag(tag) => label.tags.contains(tag),
        }
    }

    pub fn is_all(&self) -> bool {
        *self == Filter::All
    }

    /// サイドバーのボタンを押したときの次の条件。すべて → 良い → 悪い → フラグなし →
    /// 各タグ → すべて の順に回る。設定から消えたタグで絞り込んでいたら、すべてに戻す。
    pub fn next(&self, tags: &[String]) -> Filter {
        let mut cycle = vec![
            Filter::All,
            Filter::Flag(Some(Flag::Good)),
            Filter::Flag(Some(Flag::Bad)),
            Filter::Flag(None),
        ];
        cycle.extend(tags.iter().cloned().map(Filter::Tag));
        cycle
            .iter()
            .position(|f| f == self)
            .map_or(Filter::All, |i| cycle[(i + 1) % cycle.len()].clone())
    }
}

/// 絞り込みの条件と、ラベルの読み先をまとめたもの。
pub struct Matcher<'a> {
    pub filter: &'a Filter,
    pub labels: &'a LabelStore,
}

impl Matcher<'_> {
    pub fn matches(&self, path: &Path) -> bool {
        self.filter.is_all() || self.filter.matches(&self.labels.get(path))
    }
}

/// [`collect`] で集めたもの。
#[derive(Debug, Default, PartialEq)]
pub struct Collected {
    pub images: Vec<PathBuf>,
    /// 辿ったフォルダにある ZIP / 7z。中の画像にはラベルがないので、条件で絞らずに全部集める。
    pub archives: Vec<PathBuf>,
}

/// `root` の画像のうち条件に合うものを、ツリーと同じ順で集める。`recursive` ならサブフォルダと、
/// その途中にある ZIP / 7z も辿る。
pub fn collect(
    root: &Path,
    recursive: bool,
    registry: &Registry,
    order: SortOrder,
    matcher: &Matcher,
) -> Collected {
    let mut out = Collected::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let listing = tree::list_folder(&dir, registry, order);
        out.images
            .extend(listing.images.into_iter().filter(|f| matcher.matches(f)));
        if recursive {
            out.archives.extend(listing.archives);
            // 深さ優先で、ツリーで上にあるフォルダから辿るよう逆順に積む。
            stack.extend(listing.folders.into_iter().rev());
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

/// コピーするもの。
pub enum Copy {
    File {
        from: PathBuf,
        to: PathBuf,
    },
    /// ZIP / 7z の中の画像を `dir` へ。`flat` なら中のフォルダ構成を捨てて `dir` の直下に置く。
    Archive {
        archive: PathBuf,
        dir: PathBuf,
        flat: bool,
    },
}

/// 別スレッドでのコピーの進み具合。
pub struct CopyJob {
    pub total: usize,
    pub done: Arc<AtomicUsize>,
    /// 写せた画像の数。ZIP は中の画像を 1 枚ずつ数える。
    pub copied: Arc<AtomicUsize>,
    pub failed: Arc<std::sync::Mutex<Vec<String>>>,
    /// 撮影地点を消して写した画像の数。
    pub located: Arc<AtomicUsize>,
    pub finished: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
}

impl CopyJob {
    /// `items` をコピーし始める。大きな画像を何百枚もコピーすると数十秒かかるので、
    /// 画面を止めないよう別スレッドで行う。既にあるファイルは上書きしない。
    /// `strip_location` なら、写すファイルから撮影地点を消す（[`crate::strip_location`]）。
    pub fn start(
        items: Vec<Copy>,
        registry: Registry,
        strip_location: bool,
        repaint: impl Fn() + Send + 'static,
    ) -> Self {
        let job = Self {
            total: items.len(),
            done: Arc::default(),
            copied: Arc::default(),
            failed: Arc::default(),
            located: Arc::default(),
            finished: Arc::default(),
            cancel: Arc::default(),
        };
        let (done, copied, failed, located, finished, cancel) = (
            job.done.clone(),
            job.copied.clone(),
            job.failed.clone(),
            job.located.clone(),
            job.finished.clone(),
            job.cancel.clone(),
        );
        std::thread::spawn(move || {
            let strip = strip_location.then_some(located.as_ref());
            for item in items {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                match item {
                    Copy::File { from, to } => match copy_one(&from, &to, strip) {
                        Ok(()) => {
                            copied.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => failed.lock().unwrap_or_else(|e| e.into_inner()).push(e),
                    },
                    Copy::Archive { archive, dir, flat } => {
                        let result = unpack_images(&archive, &dir, flat, &registry, &cancel, strip);
                        copied.fetch_add(result.copied, Ordering::Relaxed);
                        failed
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .extend(result.failed);
                    }
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

/// `strip` があれば撮影地点を消して写し、消したら数える。
fn copy_one(from: &Path, to: &Path, strip: Option<&AtomicUsize>) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{} -> {}: {e}", from.display(), to.display());
    if to.exists() {
        return Err(format!("{}: already exists", to.display()));
    }
    let Some(located) = strip else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(fail)?;
        }
        return std::fs::copy(from, to).map(|_| ()).map_err(fail);
    };
    let mut bytes = std::fs::read(from).map_err(fail)?;
    strip_bytes(&mut bytes, located).map_err(|e| format!("{}: {e}", from.display()))?;
    write_new(to, &bytes).map_err(fail)
}

/// 撮影地点を消す。消せない形式なら写さない。消すように頼まれたのに、地点を残したまま
/// 黙って写すと、人に渡したあとで気づくことになるため。
fn strip_bytes(bytes: &mut [u8], located: &AtomicUsize) -> Result<(), String> {
    match crate::strip_location::strip(bytes) {
        Ok(removed) => {
            if removed.any() {
                located.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        }
        Err(e) => Err(format!(
            "{e}, so it was not copied (turn off removing the location to copy it as it is)"
        )),
    }
}

#[derive(Debug, Default)]
struct Unpacked {
    copied: usize,
    failed: Vec<String>,
}

/// `archive` の中の画像を `dir` へ書き出す。
///
/// パスワードは尋ねない。書き出しは別スレッドで何十もの ZIP を順に処理するので、途中で
/// 1 つずつ尋ねると終わるまで画面の前を離れられなくなるため。暗号化された ZIP は
/// ツリーで開いて（パスワードを入れて）から、その中で書き出してもらう。
fn unpack_images(
    archive: &Path,
    dir: &Path,
    flat: bool,
    registry: &Registry,
    cancel: &AtomicBool,
    strip: Option<&AtomicUsize>,
) -> Unpacked {
    let mut out = Unpacked::default();
    let result = archive::for_each_entry(archive, None, |inner, data| {
        if cancel.load(Ordering::Relaxed) {
            return Err(EntryError::Other("Cancelled".into()));
        }
        let Some(data) = data else {
            return Ok(());
        };
        if !registry.supports_path(inner) && !registry.needs_probe(inner) {
            return Ok(());
        }
        let mut bytes = Vec::new();
        data.read_to_end(&mut bytes).map_err(EntryError::Read)?;
        if registry.find(inner, &bytes).is_none() {
            return Ok(());
        }
        let target = if flat {
            free_name(dir, inner.file_name().unwrap_or_default())
        } else {
            dir.join(inner)
        };
        if let Some(located) = strip {
            if let Err(e) = strip_bytes(&mut bytes, located) {
                out.failed.push(format!(
                    "{}{}{}: {e}",
                    archive.display(),
                    crate::catalog::IN_ARCHIVE,
                    inner.display()
                ));
                return Ok(());
            }
        }
        match write_new(&target, &bytes) {
            Ok(()) => out.copied += 1,
            Err(e) => out.failed.push(format!("{}: {e}", target.display())),
        }
        Ok(())
    });
    match result {
        Ok(_) => {}
        Err(ArchiveError::NeedsPassword | ArchiveError::WrongPassword) => out.failed.push(format!(
            "{}: password-protected. Open it in the tree and export from inside it.",
            archive.display()
        )),
        Err(ArchiveError::Other(e)) => out.failed.push(format!("{}: {e}", archive.display())),
    }
    out
}

fn write_new(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::File::create_new(target)?.write_all(bytes)
}

/// `dir` の中で、まだ使われていない `name`（重なれば `name-1.png` のように番号を付ける）。
fn free_name(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let mut taken = HashSet::new();
    copy_target(Path::new(""), Path::new(name), dir, false, &mut taken)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{png_bytes, png_registry, write_zip, TempDir};

    fn label(flag: Option<Flag>, tags: &[&str]) -> Label {
        Label {
            flag,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn flag_filter_matches_only_that_flag() {
        let good = label(Some(Flag::Good), &[]);
        let none = Label::default();
        let f = Filter::Flag(Some(Flag::Good));
        assert!(f.matches(&good));
        assert!(!f.matches(&none));
        assert!(Filter::Flag(None).matches(&none));
    }

    #[test]
    fn tag_filter_matches_images_with_that_tag() {
        let f = Filter::Tag("cat".into());
        assert!(f.matches(&label(None, &["cat", "dog"])));
        assert!(!f.matches(&label(None, &["dog"])));
    }

    #[test]
    fn filter_button_cycles_through_flags_then_tags_and_back_to_all() {
        let tags = ["cat".to_string(), "dog".to_string()];
        let mut seen = vec![Filter::All];
        for _ in 0..6 {
            seen.push(seen.last().unwrap().next(&tags));
        }
        assert_eq!(
            seen,
            [
                Filter::All,
                Filter::Flag(Some(Flag::Good)),
                Filter::Flag(Some(Flag::Bad)),
                Filter::Flag(None),
                Filter::Tag("cat".into()),
                Filter::Tag("dog".into()),
                Filter::All,
            ]
        );
    }

    #[test]
    fn filter_on_a_removed_tag_cycles_back_to_all() {
        assert_eq!(Filter::Tag("gone".into()).next(&[]), Filter::All);
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
        let labels = LabelStore::new(None);
        labels.toggle_flag(&top, Flag::Good).unwrap();
        labels.toggle_flag(&deep, Flag::Good).unwrap();
        let filter = Filter::Flag(Some(Flag::Good));
        let matcher = Matcher {
            filter: &filter,
            labels: &labels,
        };
        let order = SortOrder::default();
        let flat = collect(dir.path(), false, &png_registry(), order, &matcher);
        assert_eq!(flat.images, std::slice::from_ref(&top));
        let all = collect(dir.path(), true, &png_registry(), order, &matcher);
        assert_eq!(all.images, [top, deep]);
    }

    fn copy_all(items: Vec<Copy>) -> CopyJob {
        run(CopyJob::start(items, png_registry(), false, || {}))
    }

    fn run(job: CopyJob) -> CopyJob {
        while !job.finished.load(Ordering::Relaxed) {
            std::thread::yield_now();
        }
        job
    }

    #[test]
    fn collect_lists_archives_in_subfolders_only_when_recursive() {
        let dir = TempDir::new();
        let sub = dir.mkdir("sub");
        let zip = sub.join("data.zip");
        write_zip(&zip, &[("a.png", &png_bytes(2, 2))], None);
        let filter = Filter::All;
        let labels = LabelStore::new(None);
        let matcher = Matcher {
            filter: &filter,
            labels: &labels,
        };
        let order = SortOrder::default();
        let flat = collect(dir.path(), false, &png_registry(), order, &matcher);
        assert!(flat.archives.is_empty());
        let all = collect(dir.path(), true, &png_registry(), order, &matcher);
        assert_eq!(all.archives, [zip]);
    }

    #[test]
    fn images_in_an_archive_go_under_a_folder_named_like_it() {
        let src = TempDir::new();
        let dest = TempDir::new();
        let zip = src.path().join("data.zip");
        let png = png_bytes(2, 2);
        write_zip(
            &zip,
            &[("day1/a.png", &png), ("notes.txt", b"not an image")],
            None,
        );
        let job = copy_all(vec![Copy::Archive {
            archive: zip,
            dir: dest.path().join("data.zip"),
            flat: false,
        }]);
        assert_eq!(job.copied.load(Ordering::Relaxed), 1);
        assert!(dest.path().join("data.zip/day1/a.png").is_file());
        assert!(!dest.path().join("data.zip/notes.txt").exists());
    }

    #[test]
    fn flattened_archive_images_are_numbered_beside_existing_files() {
        let src = TempDir::new();
        let dest = TempDir::new();
        dest.touch("a.png");
        let zip = src.path().join("data.zip");
        let png = png_bytes(2, 2);
        write_zip(&zip, &[("x/a.png", &png), ("y/a.png", &png)], None);
        let job = copy_all(vec![Copy::Archive {
            archive: zip,
            dir: dest.path().to_path_buf(),
            flat: true,
        }]);
        assert_eq!(job.copied.load(Ordering::Relaxed), 2);
        assert!(dest.path().join("a-1.png").is_file());
        assert!(dest.path().join("a-2.png").is_file());
    }

    #[test]
    fn a_password_protected_archive_is_reported_and_not_copied() {
        let src = TempDir::new();
        let dest = TempDir::new();
        let zip = src.path().join("secret.zip");
        write_zip(&zip, &[("a.png", &png_bytes(2, 2))], Some("vw"));
        let job = copy_all(vec![Copy::Archive {
            archive: zip,
            dir: dest.path().join("secret.zip"),
            flat: false,
        }]);
        assert_eq!(job.copied.load(Ordering::Relaxed), 0);
        let failed = job.failed.lock().unwrap_or_else(|e| e.into_inner());
        assert!(failed[0].contains("password-protected"), "{failed:?}");
        assert!(!dest.path().join("secret.zip").exists());
    }

    #[test]
    fn removing_the_location_copies_without_gps_and_refuses_unknown_formats() {
        let src = TempDir::new();
        let dest = TempDir::new();
        let exif = vw_core::exif::sample::gps(35.0, 139.0, None);
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend(((exif.len() + 8) as u16).to_be_bytes());
        jpeg.extend(b"Exif\0\0");
        jpeg.extend(&exif);
        jpeg.extend([0xFF, 0xD9]);
        let a = src.path().join("a.jpg");
        let b = src.path().join("b.heic");
        std::fs::write(&a, &jpeg).unwrap();
        std::fs::write(&b, b"\0\0\0\x18ftypheic").unwrap();
        let items = [&a, &b]
            .into_iter()
            .map(|from| Copy::File {
                from: from.clone(),
                to: dest.path().join(from.file_name().unwrap()),
            })
            .collect();
        let job = run(CopyJob::start(items, png_registry(), true, || {}));
        assert_eq!(job.copied.load(Ordering::Relaxed), 1);
        assert_eq!(job.located.load(Ordering::Relaxed), 1);
        assert_eq!(
            job.failed.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1
        );
        assert!(!dest.path().join("b.heic").exists());
        let copied = std::fs::read(dest.path().join("a.jpg")).unwrap();
        assert_eq!(copied.len(), jpeg.len());
        assert_eq!(vw_core::exif::gps_of_file(&dest.path().join("a.jpg")), None);
        assert!(vw_core::exif::gps_of_file(&a).is_some());
    }
}
