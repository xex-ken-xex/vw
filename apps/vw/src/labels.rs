//! 画像のラベル（良い / 悪いのフラグと、タグ）。
//!
//! フォルダごとに 1 つのファイル（[`FILE_NAME`]）に、そのフォルダの画像のラベルを持つ。
//! フォルダ自身にはラベルを付けない。フォルダのフラグと中の画像のフラグが食い違うと、
//! どの画像が絞り込みに残るのか分かりにくくなるため。
//! 画像ファイルには書き込まない。DICOM や RAW を書き換えたくないため。
//! 地図で置いた撮影地点も、EXIF を書き換えずにここに持つ（同じ理由）。
//! 設定フォルダの 1 つのデータベースにしないのは、フォルダを移動・コピーしたときにラベルを
//! 一緒に持っていけるようにするため。書き込めないフォルダ（読み取り専用のメディアなど）の
//! ラベルだけは設定フォルダの下に置く。

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, FontId};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = ".vw-labels.json";
const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    Good,
    Bad,
}

/// 地図で置いた撮影地点。EXIF の位置より優先する。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Label {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<Flag>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
}

impl Label {
    pub fn is_empty(&self) -> bool {
        self.flag.is_none() && self.tags.is_empty() && self.position.is_none()
    }
}

/// ラベルファイルの中身。
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct LabelFile {
    version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    files: BTreeMap<String, Label>,
}

impl LabelFile {
    fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// 読み込んだフォルダのラベル。
#[derive(Default)]
struct DirLabels {
    file: LabelFile,
    /// 読めなかったラベルファイル。上書きする前に `.bak` として残す。
    broken: Option<PathBuf>,
}

/// フォルダごとのラベルを、必要になったときに読んで持っておく。
pub struct LabelStore {
    /// 描画中（ツリーを辿りながら）に読むので、`&self` から読み込めるようにしておく。
    dirs: RefCell<HashMap<PathBuf, DirLabels>>,
    /// 書き込めないフォルダのラベルの置き場所。
    fallback_dir: Option<PathBuf>,
    /// 読めなかった・書けなかったときの説明。アプリがログに出す。
    problems: RefCell<Vec<String>>,
}

impl LabelStore {
    pub fn new(fallback_dir: Option<PathBuf>) -> Self {
        Self {
            dirs: RefCell::default(),
            fallback_dir,
            problems: RefCell::default(),
        }
    }

    /// フォルダに書けないときの置き場所。別のスレッドで同じラベルを読むときに使う。
    pub fn fallback_dir(&self) -> Option<&Path> {
        self.fallback_dir.as_deref()
    }

    /// 画像のラベル。
    pub fn get(&self, path: &Path) -> Label {
        let (Some(dir), Some(name)) = (path.parent(), file_key(path)) else {
            return Label::default();
        };
        self.with_dir(dir, |d| {
            d.file.files.get(&name).cloned().unwrap_or_default()
        })
    }

    /// 画像のフラグを `flag` にする。同じフラグがもう付いていたら外す。
    /// 付いたフラグを返す。
    pub fn toggle_flag(&mut self, path: &Path, flag: Flag) -> Result<Option<Flag>, String> {
        self.edit_file(path, |label| {
            label.flag = (label.flag != Some(flag)).then_some(flag);
            label.flag
        })
    }

    /// 画像のタグを付ける・外す。付いたら `true`。
    pub fn toggle_tag(&mut self, path: &Path, tag: &str) -> Result<bool, String> {
        self.edit_file(path, |label| {
            let added = !label.tags.remove(tag);
            if added {
                label.tags.insert(tag.to_owned());
            }
            added
        })
    }

    /// 地図で置いた撮影地点を `position` にする（`None` なら消す）。前の位置を返す。
    pub fn set_position(
        &mut self,
        path: &Path,
        position: Option<Position>,
    ) -> Result<Option<Position>, String> {
        self.edit_file(path, |label| {
            std::mem::replace(&mut label.position, position)
        })
    }

    /// 画像のラベルを `label` に置き換える（空なら消す）。ファイルを移動・コピーしたときに使う。
    pub fn set(&mut self, path: &Path, label: Label) -> Result<(), String> {
        self.edit_file(path, |l| *l = label)
    }

    /// フォルダのラベルファイルが外で書き換えられたときに、次に使うときに読み直させる。
    pub fn forget(&self, dir: &Path) {
        self.dirs.borrow_mut().remove(dir);
    }

    pub fn take_problems(&self) -> Vec<String> {
        std::mem::take(&mut self.problems.borrow_mut())
    }

    fn edit_file<T>(&self, path: &Path, edit: impl FnOnce(&mut Label) -> T) -> Result<T, String> {
        let (Some(dir), Some(name)) = (path.parent(), file_key(path)) else {
            return Err(format!("{}: not a file in a folder", path.display()));
        };
        self.with_dir(dir, |_| ());
        let result = {
            let mut dirs = self.dirs.borrow_mut();
            let files = &mut dirs.get_mut(dir).expect("loaded above").file.files;
            let label = files.entry(name.clone()).or_default();
            let result = edit(label);
            if label.is_empty() {
                files.remove(&name);
            }
            result
        };
        self.save(dir)?;
        Ok(result)
    }

    fn with_dir<T>(&self, dir: &Path, f: impl FnOnce(&DirLabels) -> T) -> T {
        if let Some(d) = self.dirs.borrow().get(dir) {
            return f(d);
        }
        let loaded = self.load(dir);
        let mut dirs = self.dirs.borrow_mut();
        f(dirs.entry(dir.to_path_buf()).or_insert(loaded))
    }

    fn load(&self, dir: &Path) -> DirLabels {
        let candidates = [Some(dir.join(FILE_NAME)), self.fallback_path(dir)];
        for path in candidates.into_iter().flatten() {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            return match serde_json::from_str::<LabelFile>(&text) {
                Ok(file) => DirLabels { file, broken: None },
                Err(e) => {
                    self.problems
                        .borrow_mut()
                        .push(format!("{}: cannot read labels ({e})", path.display()));
                    DirLabels {
                        file: LabelFile::default(),
                        broken: Some(path),
                    }
                }
            };
        }
        DirLabels::default()
    }

    /// フォルダのラベルを書き出す。フォルダに書けなければ設定フォルダの下に書く。
    fn save(&self, dir: &Path) -> Result<(), String> {
        let mut dirs = self.dirs.borrow_mut();
        let d = dirs.get_mut(dir).expect("saved after loading");
        if let Some(broken) = d.broken.take() {
            let _ = std::fs::rename(&broken, broken.with_extension("json.bak"));
        }
        d.file.version = VERSION;
        let in_folder = dir.join(FILE_NAME);
        let first = write_or_remove(&in_folder, &d.file);
        if first.is_ok() {
            if let Some(fallback) = self.fallback_path(dir) {
                let _ = std::fs::remove_file(fallback);
            }
            return Ok(());
        }
        let Some(fallback) = self.fallback_path(dir) else {
            return first;
        };
        if let Some(parent) = fallback.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        write_or_remove(&fallback, &d.file)
            .map_err(|e| format!("{} (also tried {})", first.unwrap_err(), e))
    }

    /// 書き込めないフォルダのラベルの置き場所。フォルダ名と、フルパスから作った番号で名前を付ける
    /// （同じ名前の別のフォルダと重ならないように）。
    fn fallback_path(&self, dir: &Path) -> Option<PathBuf> {
        let base = self.fallback_dir.as_ref()?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        dir.hash(&mut h);
        let name: String = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        Some(base.join(format!("{name}-{:016x}.json", h.finish())))
    }
}

/// 地図で置いた位置が、どこから来たかを示すメタデータの名前と値。
pub const POSITION_FROM: &str = "Position from";
pub const FROM_MAP: &str = "Map";

/// メタデータの撮影地点を、地図で置いた位置に置き換える。高度は EXIF の位置のものなので消す。
pub fn apply_position(m: &mut vw_core::Metadata, p: Position) {
    let location = vw_core::exif::LOCATION;
    m.entries.retain(|(group, _, _)| group != location);
    if let Some(gps) = vw_core::exif::Gps::new(p.latitude, p.longitude, None) {
        gps.push_to(m);
        m.push(location, POSITION_FROM, FROM_MAP.to_owned());
    }
}

/// フラグのアイコン。
pub fn flag_icon(flag: Flag) -> &'static str {
    match flag {
        Flag::Good => egui_phosphor::regular::THUMBS_UP,
        Flag::Bad => egui_phosphor::regular::THUMBS_DOWN,
    }
}

/// 名前の前にフラグのアイコン、後ろにタグを付けた表示用の文字列。
pub fn labeled_name(
    name: &str,
    label: &Label,
    color: Color32,
    weak: Color32,
    font: FontId,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let format = |color| TextFormat::simple(font.clone(), color);
    if let Some(flag) = label.flag {
        job.append(flag_icon(flag), 0.0, format(crate::theme::flag_color(flag)));
        job.append(" ", 0.0, format(color));
    }
    job.append(name, 0.0, format(color));
    for tag in &label.tags {
        job.append(&format!("  #{tag}"), 0.0, format(weak));
    }
    job
}

fn file_key(path: &Path) -> Option<String> {
    Some(path.file_name()?.to_string_lossy().into_owned())
}

/// ラベルが 1 つもなければファイルを消し、あれば書く。書きかけのファイルを残さないよう、
/// 隣に書いてから置き換える。
fn write_or_remove(path: &Path, file: &LabelFile) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: cannot write labels ({e})", path.display());
    if file.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(fail(e)),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_string_pretty(file).expect("labels always serialize");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text + "\n").map_err(fail)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        fail(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn flag_is_saved_next_to_the_image_and_read_back() {
        let dir = TempDir::new();
        let img = dir.touch("a.png");
        let mut store = LabelStore::new(None);
        assert_eq!(store.toggle_flag(&img, Flag::Good), Ok(Some(Flag::Good)));
        let fresh = LabelStore::new(None);
        assert_eq!(fresh.get(&img).flag, Some(Flag::Good));
        assert!(dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn same_flag_again_clears_it_and_removes_the_empty_file() {
        let dir = TempDir::new();
        let img = dir.touch("a.png");
        let mut store = LabelStore::new(None);
        store.toggle_flag(&img, Flag::Bad).unwrap();
        assert_eq!(store.toggle_flag(&img, Flag::Bad), Ok(None));
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn other_flag_replaces_the_current_one() {
        let dir = TempDir::new();
        let img = dir.touch("a.png");
        let mut store = LabelStore::new(None);
        store.toggle_flag(&img, Flag::Good).unwrap();
        assert_eq!(store.toggle_flag(&img, Flag::Bad), Ok(Some(Flag::Bad)));
    }

    #[test]
    fn tags_toggle_independently() {
        let dir = TempDir::new();
        let img = dir.touch("a.png");
        let mut store = LabelStore::new(None);
        assert_eq!(store.toggle_tag(&img, "cat"), Ok(true));
        assert_eq!(store.toggle_tag(&img, "dog"), Ok(true));
        assert_eq!(store.toggle_tag(&img, "cat"), Ok(false));
        let tags: Vec<_> = store.get(&img).tags.into_iter().collect();
        assert_eq!(tags, ["dog"]);
    }

    #[test]
    fn placed_position_is_saved_and_returns_the_previous_one() {
        let dir = TempDir::new();
        let img = dir.touch("a.png");
        let mut store = LabelStore::new(None);
        let sapporo = Position {
            latitude: 43.0621,
            longitude: 141.3544,
        };
        assert_eq!(store.set_position(&img, Some(sapporo)), Ok(None));
        assert_eq!(LabelStore::new(None).get(&img).position, Some(sapporo));
        assert_eq!(store.set_position(&img, None), Ok(Some(sapporo)));
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn position_alone_keeps_the_label() {
        let label = Label {
            position: Some(Position {
                latitude: 1.0,
                longitude: 2.0,
            }),
            ..Default::default()
        };
        assert!(!label.is_empty());
    }

    #[test]
    fn placed_position_replaces_the_exif_one_in_the_metadata() {
        let mut m = vw_core::Metadata::default();
        vw_core::exif::Gps::new(35.0, 139.0, Some(10.0))
            .unwrap()
            .push_to(&mut m);
        apply_position(
            &mut m,
            Position {
                latitude: 43.0621,
                longitude: 141.3544,
            },
        );
        let gps = vw_core::exif::Gps::from_metadata(&m).unwrap();
        assert!((gps.latitude - 43.0621).abs() < 1e-6);
        assert_eq!(m.get(vw_core::exif::LOCATION, "Altitude"), None);
        assert_eq!(
            m.get(vw_core::exif::LOCATION, POSITION_FROM),
            Some(FROM_MAP)
        );
    }

    #[test]
    fn broken_label_file_is_reported_and_kept_as_backup_on_save() {
        let dir = TempDir::new();
        let img = dir.touch("a.png");
        std::fs::write(dir.path().join(FILE_NAME), "{ not json").unwrap();
        let mut store = LabelStore::new(None);
        assert!(store.get(&img).is_empty());
        assert_eq!(store.take_problems().len(), 1);
        store.toggle_flag(&img, Flag::Good).unwrap();
        assert!(dir.path().join(".vw-labels.json.bak").exists());
    }

    #[test]
    fn unwritable_folder_uses_the_fallback_folder() {
        let dir = TempDir::new();
        let fallback = dir.mkdir("config");
        let missing = dir.path().join("gone").join("a.png");
        let mut store = LabelStore::new(Some(fallback.clone()));
        store.toggle_flag(&missing, Flag::Good).unwrap();
        assert_eq!(std::fs::read_dir(&fallback).unwrap().count(), 1);
        assert_eq!(
            LabelStore::new(Some(fallback)).get(&missing).flag,
            Some(Flag::Good)
        );
    }
}
