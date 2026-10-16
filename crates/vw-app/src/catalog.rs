//! データセットの検索カタログ（`vw-search-catalog.json`）。データセットのフォルダ直下に置き、
//! 名前・メモ・タグ・足した項目と、フォルダ以下の画像（ZIP / 7z の中も）のメタデータを持つ。
//!
//! メタデータは「種類・キー・値」の 3 つ組にし、同じ組はデータセットで 1 つだけ持つ
//! （`meta`）。各ファイルはその番号の一覧を持つ。ファイルごとに組を書き並べないのは、
//! 同じ装置・同じ患者の画像では大半の値が重なり、カタログが何倍にも膨らむため。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use vw_core::{Metadata, Registry};

use crate::archive::{self, ArchiveError, EntryError};

pub const FILE_NAME: &str = "vw-search-catalog.json";
const FORMAT: u32 = 1;
/// ZIP の場所と ZIP の中のパスの区切り（`data.zip!/series1/IM0001.dcm`）。
pub const IN_ARCHIVE: &str = "!/";

/// メタデータの 1 組（種類・キー・値）。種類はメタデータ欄のグループ名を小文字にしたもの
/// （`exif`、`dicom` など）と、形式によらない基本情報の `image`。
pub type Triple = (String, String, String);

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Catalog {
    pub format: u32,
    pub name: String,
    #[serde(default)]
    pub memo: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// 設定（`dataset.fields`）で足した項目。設定から消した項目や、他の人が足した項目も残す。
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub updated: String,
    /// 取り出して作ったデータセットなら、元のデータセット。
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub meta: Vec<Triple>,
    #[serde(default)]
    pub files: Vec<FileEntry>,
    /// 読めなかったファイルや、パスワードを入れずに飛ばしたアーカイブ。
    #[serde(default)]
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skipped {
    pub path: String,
    /// パスワードを入れずに飛ばしたアーカイブは `password`。ほかは読めなかった理由。
    pub reason: String,
}

impl std::fmt::Display for Skipped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.reason == PASSWORD_SKIPPED {
            write!(f, "{}: skipped (no password)", self.path)
        } else {
            write!(f, "{}: {}", self.path, self.reason)
        }
    }
}

pub const PASSWORD_SKIPPED: &str = "password";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    /// データセットのフォルダからの相対パス（区切りは `/`）。ZIP の中は [`IN_ARCHIVE`] でつなぐ。
    pub path: String,
    #[serde(default)]
    pub size: u64,
    /// 更新時刻（UTC、RFC 3339）。ZIP の中のファイルは ZIP の更新時刻。
    #[serde(default)]
    pub modified: String,
    /// [`Catalog::meta`] の番号。
    #[serde(default)]
    pub meta: Vec<u32>,
}

/// 人が入力する部分。登録・編集のダイアログで変える。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Info {
    pub name: String,
    pub memo: String,
    pub tags: Vec<String>,
    pub fields: BTreeMap<String, String>,
}

impl Catalog {
    pub fn set_info(&mut self, info: Info) {
        self.name = info.name;
        self.memo = info.memo;
        self.tags = info.tags;
        self.fields = info.fields;
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

pub fn exists_in(dir: &Path) -> bool {
    path_in(dir).is_file()
}

pub fn load(dir: &Path) -> Result<Catalog, String> {
    let path = path_in(dir);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// 一時ファイルに書いてから置き換える。書いている途中で落ちても、前のカタログが壊れないように。
pub fn save(dir: &Path, catalog: &Catalog) -> Result<(), String> {
    let path = path_in(dir);
    let temp = dir.join(format!("{FILE_NAME}.tmp"));
    let text = serde_json::to_string(catalog).expect("catalog is always representable in JSON");
    std::fs::write(&temp, text)
        .and_then(|()| std::fs::rename(&temp, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            format!("{}: {e}", path.display())
        })
}

/// 登録できるか。カタログを置くので書き込めること。
///
/// データセットの中に別のデータセットがあってもよい。外側は内側のファイルも含めて持つ。
pub fn check_can_register(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!("{FILE_NAME}.write-test"));
    std::fs::write(&probe, b"")
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|e| format!("Cannot write to {}: {e}", dir.display()))
}

/// 調べている途中の様子。画面から読み、キャンセルを伝える。
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub current: Mutex<String>,
    pub cancel: AtomicBool,
}

impl Progress {
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub(crate) fn step(&self, current: &str) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = current.to_owned();
    }
}

/// 調べ終わった結果。
#[derive(Debug)]
pub struct Report {
    pub catalog: Catalog,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}

/// `dir` 以下の画像（ZIP / 7z の中も）のメタデータを集めてカタログを作る。`previous` があれば
/// 作った日時を引き継ぎ、ファイルの増減を数える。暗号化されたアーカイブは `ask_password` で
/// パスワードを聞き（`true` なら前に聞いたものが違った）、`None` ならそのアーカイブを飛ばす。
///
/// 前のカタログのうち変わっていないファイルの組を使い回さず、全ファイルを読み直す。
/// 組はデータセット全体で 1 つずつしか持たないので、消えたファイルの組だけを除くことが
/// できないため。
pub fn scan(
    dir: &Path,
    info: Info,
    previous: Option<&Catalog>,
    registry: &Registry,
    progress: &Progress,
    ask_password: &mut dyn FnMut(&Path, bool) -> Option<String>,
) -> Result<Report, String> {
    let mut candidates = Vec::new();
    walk(dir, registry, &mut candidates)?;
    progress.total.store(candidates.len(), Ordering::Relaxed);
    let mut builder = Builder::default();
    let mut problems: Vec<Skipped> = Vec::new();
    for path in &candidates {
        if progress.cancelled() {
            return Err("Cancelled".into());
        }
        let relative = relative_name(dir, path);
        progress.step(&relative);
        let modified = modified_time(path);
        if archive::is_archive(path) {
            scan_archive(
                path,
                &relative,
                &modified,
                registry,
                progress,
                ask_password,
                &mut builder,
                &mut problems,
            );
        } else {
            match std::fs::read(path) {
                Ok(data) => {
                    let size = data.len() as u64;
                    match triples(registry, path, &data) {
                        Ok(t) => builder.add(relative, size, modified, t),
                        Err(e) => problems.push(skip(&relative, e)),
                    }
                }
                Err(e) => problems.push(skip(&relative, e.to_string())),
            }
        }
        progress.done.fetch_add(1, Ordering::Relaxed);
    }
    if progress.cancelled() {
        return Err("Cancelled".into());
    }
    let now = now();
    let mut catalog = Catalog {
        format: FORMAT,
        created: previous.map_or_else(|| now.clone(), |p| p.created.clone()),
        updated: now,
        source: previous.and_then(|p| p.source.clone()),
        meta: builder.triples,
        files: builder.files,
        skipped: problems,
        ..Catalog::default()
    };
    catalog.set_info(info);
    let (added, removed, changed) = previous.map_or((catalog.files.len(), 0, 0), |p| {
        differences(&p.files, &catalog.files)
    });
    Ok(Report {
        catalog,
        added,
        removed,
        changed,
    })
}

pub fn skip(path: &str, reason: impl Into<String>) -> Skipped {
    Skipped {
        path: path.to_owned(),
        reason: reason.into(),
    }
}

/// 調べるファイル（画像とアーカイブ）を名前順に集める。
fn walk(dir: &Path, registry: &Registry, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let Ok(kind) = e.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !crate::tree::is_recycle_bin(&e.file_name().to_string_lossy()) {
                walk(&path, registry, out)?;
            }
        } else if kind.is_file() && (archive::is_archive(&path) || registry.supports_file(&path)) {
            out.push(path);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn scan_archive(
    path: &Path,
    relative: &str,
    modified: &str,
    registry: &Registry,
    progress: &Progress,
    ask_password: &mut dyn FnMut(&Path, bool) -> Option<String>,
    builder: &mut Builder,
    problems: &mut Vec<Skipped>,
) {
    let mut password = None;
    loop {
        // パスワードが違うと途中で失敗するので、読み終わるまでは別に貯めておく。
        let mut found = Vec::new();
        let result = archive::for_each_entry(path, password.as_deref(), |inner, data| {
            if progress.cancelled() {
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
            let name = format!("{relative}{IN_ARCHIVE}{}", slash_path(inner));
            if let Ok(t) = triples(registry, inner, &bytes) {
                found.push((name, bytes.len() as u64, t));
            } else if registry.supports_path(inner) {
                // 中身で判定するだけのファイル（拡張子のないものなど）は、画像でなくても問題にしない。
                found.push((name, bytes.len() as u64, Vec::new()));
            }
            Ok(())
        });
        match result {
            Ok(_) => {
                for (name, size, t) in found {
                    if t.is_empty() {
                        problems.push(skip(&name, "cannot read the metadata"));
                    } else {
                        builder.add(name, size, modified.to_owned(), t);
                    }
                }
                return;
            }
            Err(ArchiveError::NeedsPassword | ArchiveError::WrongPassword)
                if !progress.cancelled() =>
            {
                let wrong = password.is_some();
                match ask_password(path, wrong) {
                    Some(p) => password = Some(p),
                    None => {
                        problems.push(skip(relative, PASSWORD_SKIPPED));
                        return;
                    }
                }
            }
            Err(ArchiveError::NeedsPassword | ArchiveError::WrongPassword) => return,
            Err(ArchiveError::Other(e)) => {
                if !progress.cancelled() {
                    problems.push(skip(relative, e));
                }
                return;
            }
        }
    }
}

/// 1 ファイルのメタデータを組にする。
fn triples(registry: &Registry, path: &Path, data: &[u8]) -> Result<Vec<Triple>, String> {
    let (metadata, plugin) = registry
        .read_all_metadata_bytes(path, data)
        .map_err(|e| e.to_string())?;
    let mut out: Vec<Triple> = basics(plugin, &metadata)
        .into_iter()
        .map(|(k, v)| ("image".to_owned(), k.to_owned(), v))
        .collect();
    out.extend(
        metadata
            .entries
            .into_iter()
            .map(|(group, key, value)| (group.to_lowercase(), key, limit(value))),
    );
    Ok(out)
}

/// これより長い値はカタログに入れない。画像のデータを文字列のタグに入れたファイルがあり、
/// そのまま入れるとカタログが膨らむうえ検索の役にも立たないため。
const LONGEST_VALUE: usize = 2048;

/// 長すぎる値は、キーを「存在する」で探せるよう、値の代わりに長さを入れる。
fn limit(value: String) -> String {
    if value.len() <= LONGEST_VALUE {
        value
    } else {
        format!("(2KB 超: {} バイト)", value.len())
    }
}

/// 形式によらない基本情報（形式名・幅・高さ・ビット深度）。形式ごとのメタデータの書き方から拾う。
/// プラグインに別の口を足さないのは、どの形式もすでに同じ情報をメタデータ欄に出しているため。
fn basics(plugin: &str, m: &Metadata) -> Vec<(&'static str, String)> {
    let mut out = vec![("format", plugin.to_owned())];
    let size = m
        .entries
        .iter()
        .find(|(_, k, _)| k == "Size")
        .and_then(|(_, _, v)| v.split_once(" x "))
        .map(|(w, h)| (w.trim().to_owned(), h.trim().to_owned()));
    let dicom = |key| m.get("DICOM", key).map(str::to_owned);
    let (width, height) = match size {
        Some((w, h)) => (Some(w), Some(h)),
        None => (dicom("0028,0011"), dicom("0028,0010")),
    };
    let depth = ["Bit depth", "Bits per pixel"]
        .iter()
        .find_map(|key| m.entries.iter().find(|(_, k, _)| k == key))
        .map(|(_, _, v)| v.clone())
        .or_else(|| dicom("0028,0101"));
    for (key, value) in [("width", width), ("height", height), ("bit depth", depth)] {
        if let Some(v) = value {
            out.push((key, v));
        }
    }
    out
}

/// 組の辞書とファイルの一覧を作る。
#[derive(Default)]
struct Builder {
    triples: Vec<Triple>,
    index: HashMap<Triple, u32>,
    files: Vec<FileEntry>,
}

impl Builder {
    fn add(&mut self, path: String, size: u64, modified: String, triples: Vec<Triple>) {
        let mut ids: Vec<u32> = triples
            .into_iter()
            .map(|t| {
                let next = self.triples.len() as u32;
                *self.index.entry(t.clone()).or_insert_with(|| {
                    self.triples.push(t);
                    next
                })
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        self.files.push(FileEntry {
            path,
            size,
            modified,
            meta: ids,
        });
    }
}

/// 増えた・消えた・変わった（大きさか更新時刻が違う）ファイルの数。
fn differences(old: &[FileEntry], new: &[FileEntry]) -> (usize, usize, usize) {
    let before: HashMap<&str, &FileEntry> = old.iter().map(|f| (f.path.as_str(), f)).collect();
    let after: HashMap<&str, &FileEntry> = new.iter().map(|f| (f.path.as_str(), f)).collect();
    let added = new
        .iter()
        .filter(|f| !before.contains_key(f.path.as_str()))
        .count();
    let removed = old
        .iter()
        .filter(|f| !after.contains_key(f.path.as_str()))
        .count();
    let changed = new
        .iter()
        .filter(|f| {
            before
                .get(f.path.as_str())
                .is_some_and(|b| b.size != f.size || b.modified != f.modified)
        })
        .count();
    (added, removed, changed)
}

fn relative_name(root: &Path, path: &Path) -> String {
    slash_path(path.strip_prefix(root).unwrap_or(path))
}

/// OS によらず `/` で区切る。Windows で作ったカタログを Linux でも使えるように。
pub fn slash_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn modified_time(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(rfc3339)
        .unwrap_or_default()
}

fn now() -> String {
    rfc3339(std::time::SystemTime::now())
}

/// UTC で書く。地方時にしないのは、時差の違う場所で作ったカタログを比べても、同じ時刻が
/// 同じ文字列になるようにするため。
fn rfc3339(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{all_registry, png_bytes, png_registry, write_png, write_zip, TempDir};

    fn no_password(_: &Path, _: bool) -> Option<String> {
        None
    }

    fn run(dir: &Path, previous: Option<&Catalog>) -> Result<Report, String> {
        let info = Info {
            name: "set".into(),
            ..Info::default()
        };
        scan(
            dir,
            info,
            previous,
            &png_registry(),
            &Progress::default(),
            &mut no_password,
        )
    }

    fn values_of<'a>(c: &'a Catalog, file: &str, kind: &str, key: &str) -> Vec<&'a str> {
        let f = c.files.iter().find(|f| f.path == file).unwrap();
        f.meta
            .iter()
            .map(|&i| &c.meta[i as usize])
            .filter(|(k, key2, _)| k == kind && key2 == key)
            .map(|(_, _, v)| v.as_str())
            .collect()
    }

    #[test]
    fn images_in_the_recycle_bin_are_left_out_of_the_catalog() {
        let tmp = TempDir::new();
        write_png(&tmp.mkdir("$RECYCLE.BIN").join("$R1.png"), 2, 3, &[0; 18]);
        write_png(&tmp.path().join("kept.png"), 2, 3, &[0; 18]);
        let report = run(tmp.path(), None).unwrap();
        let paths: Vec<_> = report
            .catalog
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(paths, ["kept.png"]);
    }

    #[test]
    fn images_in_subfolders_are_listed_with_slash_separated_paths() {
        let tmp = TempDir::new();
        let sub = tmp.mkdir("a/b");
        write_png(&sub.join("x.png"), 2, 3, &[0; 18]);
        std::fs::write(tmp.path().join("notes.txt"), "not an image").unwrap();
        let report = run(tmp.path(), None).unwrap();
        let paths: Vec<_> = report
            .catalog
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(paths, ["a/b/x.png"]);
        assert_eq!(report.added, 1);
        let c = &report.catalog;
        assert_eq!(values_of(c, "a/b/x.png", "image", "format"), ["PNG"]);
        assert_eq!(values_of(c, "a/b/x.png", "image", "width"), ["2"]);
        assert_eq!(values_of(c, "a/b/x.png", "image", "height"), ["3"]);
        assert_eq!(values_of(c, "a/b/x.png", "png", "Bit depth"), ["8"]);
    }

    #[test]
    fn the_same_triple_is_stored_once_for_the_whole_dataset() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("1.png"), 4, 4, &[0; 48]);
        write_png(&tmp.path().join("2.png"), 4, 4, &[9; 48]);
        let c = run(tmp.path(), None).unwrap().catalog;
        let widths = c.meta.iter().filter(|(_, k, _)| k == "width").count();
        assert_eq!(widths, 1);
        assert_eq!(c.files[0].meta, c.files[1].meta);
    }

    #[test]
    fn dicom_tags_are_stored_by_number() {
        let tmp = TempDir::new();
        let bytes = vw_dicom::synth::Synth::ct(2, 2).patient("Doe^Jane").bytes();
        std::fs::write(tmp.path().join("IM0001"), bytes).unwrap();
        let info = Info::default();
        let report = scan(
            tmp.path(),
            info,
            None,
            &all_registry(),
            &Progress::default(),
            &mut no_password,
        )
        .unwrap();
        let c = &report.catalog;
        assert_eq!(values_of(c, "IM0001", "dicom", "0010,0010"), ["Doe^Jane"]);
        assert_eq!(values_of(c, "IM0001", "image", "width"), ["2"]);
    }

    #[test]
    fn images_inside_zip_files_are_listed_after_an_exclamation_mark() {
        let tmp = TempDir::new();
        let png = png_bytes(1, 1);
        write_zip(
            &tmp.path().join("data.zip"),
            &[("s1/a.png", &png), ("readme.txt", b"hi")],
            None,
        );
        let c = run(tmp.path(), None).unwrap().catalog;
        let paths: Vec<_> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["data.zip!/s1/a.png"]);
    }

    #[test]
    fn encrypted_zip_is_read_with_the_password_given_after_a_wrong_one() {
        let tmp = TempDir::new();
        let png = png_bytes(1, 1);
        write_zip(
            &tmp.path().join("secret.zip"),
            &[("a.png", &png)],
            Some("vw"),
        );
        let mut asked = Vec::new();
        let mut answer = |_: &Path, wrong: bool| {
            asked.push(wrong);
            Some(if wrong { "vw" } else { "nope" }.to_owned())
        };
        let report = scan(
            tmp.path(),
            Info::default(),
            None,
            &png_registry(),
            &Progress::default(),
            &mut answer,
        )
        .unwrap();
        assert_eq!(asked, [false, true]);
        assert_eq!(report.catalog.files[0].path, "secret.zip!/a.png");
        assert!(report.catalog.skipped.is_empty());
    }

    #[test]
    fn skipping_the_password_leaves_the_zip_out_and_says_so() {
        let tmp = TempDir::new();
        let png = png_bytes(1, 1);
        write_zip(
            &tmp.path().join("secret.zip"),
            &[("a.png", &png)],
            Some("vw"),
        );
        write_png(&tmp.path().join("b.png"), 1, 1, &[0; 3]);
        let report = run(tmp.path(), None).unwrap();
        assert_eq!(report.catalog.files.len(), 1);
        let skipped = &report.catalog.skipped;
        assert_eq!(skipped, &[skip("secret.zip", "password")]);
        assert_eq!(skipped[0].to_string(), "secret.zip: skipped (no password)");
    }

    #[test]
    fn updating_counts_added_removed_and_changed_files_and_keeps_the_creation_time() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("keep.png"), 1, 1, &[0; 3]);
        write_png(&tmp.path().join("change.png"), 1, 1, &[0; 3]);
        write_png(&tmp.path().join("remove.png"), 1, 1, &[0; 3]);
        let mut first = run(tmp.path(), None).unwrap().catalog;
        first.created = "2020-01-01T00:00:00Z".into();
        write_png(&tmp.path().join("change.png"), 2, 2, &[0; 12]);
        std::fs::remove_file(tmp.path().join("remove.png")).unwrap();
        write_png(&tmp.path().join("new.png"), 1, 1, &[0; 3]);
        let report = run(tmp.path(), Some(&first)).unwrap();
        assert_eq!((report.added, report.removed, report.changed), (1, 1, 1));
        assert_eq!(report.catalog.created, "2020-01-01T00:00:00Z");
    }

    #[test]
    fn a_dataset_inside_the_folder_is_included_in_the_outer_one() {
        let tmp = TempDir::new();
        let inner = tmp.mkdir("inner");
        write_png(&inner.join("a.png"), 1, 1, &[0; 3]);
        std::fs::write(inner.join(FILE_NAME), "{}").unwrap();
        let c = run(tmp.path(), None).unwrap().catalog;
        let paths: Vec<_> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["inner/a.png"]);
        assert!(check_can_register(&inner).is_ok());
    }

    #[test]
    fn values_longer_than_2kb_keep_only_their_length() {
        assert_eq!(limit("x".repeat(2048)).len(), 2048);
        assert_eq!(limit("x".repeat(3000)), "(2KB 超: 3000 バイト)");
    }

    #[test]
    fn saved_catalog_is_read_back_with_unknown_fields_kept() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        let mut c = run(tmp.path(), None).unwrap().catalog;
        c.fields.insert("Source".into(), "Hospital A".into());
        save(tmp.path(), &c).unwrap();
        assert_eq!(load(tmp.path()).unwrap(), c);
        assert!(!tmp.path().join(format!("{FILE_NAME}.tmp")).exists());
    }

    #[test]
    fn catalog_written_by_hand_with_only_a_name_can_be_read() {
        let tmp = TempDir::new();
        std::fs::write(tmp.path().join(FILE_NAME), r#"{"format":1,"name":"x"}"#).unwrap();
        assert_eq!(load(tmp.path()).unwrap().name, "x");
    }

    #[test]
    fn cancelling_stops_the_scan() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        let progress = Progress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        let result = scan(
            tmp.path(),
            Info::default(),
            None,
            &png_registry(),
            &progress,
            &mut no_password,
        );
        assert!(result.is_err());
    }
}
