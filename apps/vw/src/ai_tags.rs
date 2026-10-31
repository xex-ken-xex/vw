//! 画像に自動で付けた AI タグ。フォルダごとに 1 つのファイル（[`FILE_NAME`]）に持つ。
//!
//! ラベル（`.vw-labels.json`）とは別のファイルにするのは、AI タグは数十個で確信度つき、
//! しかもモデルを変えると付け直すものなので、手で付けたラベルと混ぜると絞り込みや
//! 書き出しの意味が変わってしまうため。フォルダに書けないときは設定フォルダの下に置く
//! （ラベルと同じ理由）。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::faces::FaceMark;
use crate::tagger::Tags;

pub const FILE_NAME: &str = ".vw-ai-tags.json";
const VERSION: u32 = 1;

/// 1 枚の画像の AI タグと、付けたモデルの名前。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// タグを付けたモデル。顔だけを探した画像では空。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// タグを付けたモデルの版（[`crate::ai_models::tagger_version`]）。版を残す前に付けたものでは空。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// 顔を探したモデルの名前と版。顔が 1 つもなくても、探したら書く。版を残す前に探したものでは空。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub face_model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub face_version: String,
    #[serde(flatten)]
    pub tags: Tags,
    /// 見つけた顔の枠と人物。特徴ベクトルは個人情報なのでここには書かない（[`crate::face_store`]）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub faces: Vec<FaceMark>,
}

/// 版を残す前の記録の版。それまでに配ったのはこの版のモデルだけなので、空ならこの版で付けたとみなす。
/// 空を「古い版」として扱わないのは、前から付けてあるタグをすべて付け直させないため。
const FIRST_TAGGER_VERSION: &str = "7f6b584d";
const FIRST_FACES_VERSION: &str = "47534e27";

impl Entry {
    /// タグを付けたモデルの名前と版。タグを付けていなければ `None`。
    pub fn tagger(&self) -> Option<(&str, &str)> {
        if self.model.is_empty() {
            return None;
        }
        let version = match self.version.as_str() {
            "" if self.model == crate::ai_models::TAGGER_NAME => FIRST_TAGGER_VERSION,
            v => v,
        };
        Some((&self.model, version))
    }

    /// 顔を探したモデルの名前と版。版を残す前に探して顔があったものは、最初の版とみなす。
    pub fn face_finder(&self) -> Option<(&str, &str)> {
        if !self.face_model.is_empty() {
            Some((&self.face_model, &self.face_version))
        } else if !self.faces.is_empty() {
            Some((crate::ai_models::FACES_NAME, FIRST_FACES_VERSION))
        } else {
            None
        }
    }

    /// 今のモデルとは違う名前か版で付けたタグがある。
    pub fn tags_outdated(&self) -> bool {
        self.tagger().is_some_and(|t| {
            t != (
                crate::ai_models::TAGGER_NAME,
                crate::ai_models::tagger_version(),
            )
        })
    }

    /// 今のモデルとは違う名前か版で探した顔がある。
    pub fn faces_outdated(&self) -> bool {
        self.face_finder().is_some_and(|f| {
            f != (
                crate::ai_models::FACES_NAME,
                crate::ai_models::faces_version(),
            )
        })
    }

    /// 写っている人物の名前（重なりなし、見つけた順）。
    pub fn people(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for n in self.faces.iter().filter_map(|f| f.person.as_deref()) {
            if !names.contains(&n) {
                names.push(n);
            }
        }
        names
    }
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct TagFile {
    version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    files: BTreeMap<String, Entry>,
}

#[derive(Default)]
struct DirTags {
    file: TagFile,
    broken: Option<PathBuf>,
}

pub struct AiTagStore {
    dirs: RefCell<HashMap<PathBuf, DirTags>>,
    fallback_dir: Option<PathBuf>,
    problems: RefCell<Vec<String>>,
}

impl AiTagStore {
    pub fn new(fallback_dir: Option<PathBuf>) -> Self {
        Self {
            dirs: RefCell::default(),
            fallback_dir,
            problems: RefCell::default(),
        }
    }

    /// フォルダに書けないときの置き場所。別のスレッドで同じ記録を読むときに使う。
    pub fn fallback_dir(&self) -> Option<&Path> {
        self.fallback_dir.as_deref()
    }

    pub fn get(&self, path: &Path) -> Option<Entry> {
        let (dir, name) = split(path)?;
        self.with_dir(dir, |d| d.file.files.get(&name).cloned())
    }

    /// 画像の記録の一部を書き換えて保存する。タグ付けと顔探しは別々に走るので、
    /// 片方の結果でもう片方を消さないよう、丸ごと置き換えずにここを通す。
    pub fn update(&self, path: &Path, change: impl FnOnce(&mut Entry)) -> Result<(), String> {
        let (dir, name) =
            split(path).ok_or_else(|| format!("{}: not a file in a folder", path.display()))?;
        self.with_dir(dir, |_| ());
        {
            let mut dirs = self.dirs.borrow_mut();
            let files = &mut dirs.get_mut(dir).expect("loaded above").file.files;
            change(files.entry(name).or_default());
        }
        self.save(dir)
    }

    /// 画像の顔の枠。
    pub fn faces(&self, path: &Path) -> Vec<FaceMark> {
        split(path)
            .and_then(|(dir, name)| {
                self.with_dir(dir, |d| d.file.files.get(&name).map(|e| e.faces.clone()))
            })
            .unwrap_or_default()
    }

    /// 画像の記録（タグと顔の枠）を消す。フォルダごとに 1 回だけ書く。
    /// 消して空になったフォルダのファイルは、ファイルごと消す。返すのは消した画像の数。
    pub fn remove(&self, paths: &[PathBuf]) -> Result<usize, String> {
        let mut by_dir: BTreeMap<&Path, Vec<String>> = BTreeMap::new();
        for p in paths {
            if let Some((dir, name)) = split(p) {
                by_dir.entry(dir).or_default().push(name);
            }
        }
        let mut removed = 0;
        for (dir, names) in by_dir {
            let before = removed;
            let empty = {
                self.with_dir(dir, |_| ());
                let mut dirs = self.dirs.borrow_mut();
                let files = &mut dirs.get_mut(dir).expect("loaded above").file.files;
                for n in &names {
                    removed += usize::from(files.remove(n).is_some());
                }
                files.is_empty()
            };
            if removed == before {
                continue;
            }
            if empty {
                for path in [Some(dir.join(FILE_NAME)), self.fallback_path(dir)]
                    .into_iter()
                    .flatten()
                {
                    let _ = std::fs::remove_file(path);
                }
            } else {
                self.save(dir)?;
            }
        }
        Ok(removed)
    }

    /// 画像の記録を書き換え、変わったものをフォルダごとに 1 回だけ書く。
    /// `change` は変えたら true を返す。返すのは変わった画像の数。
    pub fn edit_all(
        &self,
        paths: &[PathBuf],
        mut change: impl FnMut(&mut Entry) -> bool,
    ) -> Result<usize, String> {
        let mut by_dir: BTreeMap<&Path, Vec<String>> = BTreeMap::new();
        for p in paths {
            if let Some((dir, name)) = split(p) {
                by_dir.entry(dir).or_default().push(name);
            }
        }
        let mut changed = 0;
        for (dir, names) in by_dir {
            self.with_dir(dir, |_| ());
            let n = {
                let mut dirs = self.dirs.borrow_mut();
                let files = &mut dirs.get_mut(dir).expect("loaded above").file.files;
                let mut n = 0;
                for name in &names {
                    if let Some(e) = files.get_mut(name) {
                        n += usize::from(change(e));
                    }
                }
                n
            };
            if n > 0 {
                self.save(dir)?;
                changed += n;
            }
        }
        Ok(changed)
    }

    pub fn forget(&self, dir: &Path) {
        self.dirs.borrow_mut().remove(dir);
    }

    pub fn take_problems(&self) -> Vec<String> {
        std::mem::take(&mut self.problems.borrow_mut())
    }

    fn with_dir<T>(&self, dir: &Path, f: impl FnOnce(&DirTags) -> T) -> T {
        if let Some(d) = self.dirs.borrow().get(dir) {
            return f(d);
        }
        let loaded = self.load(dir);
        let mut dirs = self.dirs.borrow_mut();
        f(dirs.entry(dir.to_path_buf()).or_insert(loaded))
    }

    fn load(&self, dir: &Path) -> DirTags {
        let candidates = [Some(dir.join(FILE_NAME)), self.fallback_path(dir)];
        for path in candidates.into_iter().flatten() {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            return match serde_json::from_str::<TagFile>(&text) {
                Ok(file) => DirTags { file, broken: None },
                Err(e) => {
                    self.problems
                        .borrow_mut()
                        .push(format!("{}: cannot read AI tags ({e})", path.display()));
                    DirTags {
                        file: TagFile::default(),
                        broken: Some(path),
                    }
                }
            };
        }
        DirTags::default()
    }

    fn save(&self, dir: &Path) -> Result<(), String> {
        let mut dirs = self.dirs.borrow_mut();
        let d = dirs.get_mut(dir).expect("saved after loading");
        if let Some(broken) = d.broken.take() {
            let _ = std::fs::rename(&broken, broken.with_extension("json.bak"));
        }
        d.file.version = VERSION;
        let text = serde_json::to_string_pretty(&d.file).map_err(|e| e.to_string())?;
        let in_folder = dir.join(FILE_NAME);
        let first =
            std::fs::write(&in_folder, &text).map_err(|e| format!("{}: {e}", in_folder.display()));
        if first.is_ok() {
            return Ok(());
        }
        let Some(fallback) = self.fallback_path(dir) else {
            return first;
        };
        if let Some(parent) = fallback.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&fallback, &text).map_err(|e| {
            format!(
                "{} (also tried {}: {e})",
                first.unwrap_err(),
                fallback.display()
            )
        })
    }

    fn fallback_path(&self, dir: &Path) -> Option<PathBuf> {
        Some(self.fallback_dir.as_ref()?.join(folder_file_name(dir)))
    }
}

/// フォルダの外（設定フォルダの下）にフォルダごとの記録を置くときのファイル名。
/// 同じ名前のフォルダが別の場所にあっても重ならないよう、パスのハッシュを添える。
pub fn folder_file_name(dir: &Path) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dir.hash(&mut h);
    let name: String = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    format!("{name}-{:016x}.json", h.finish())
}

fn split(path: &Path) -> Option<(&Path, String)> {
    let dir = path.parent()?;
    let name = path.file_name()?.to_str()?.to_owned();
    Some((dir, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tagger::Scored;

    fn entry(name: &str, score: f32) -> Entry {
        Entry {
            model: "test".into(),
            tags: Tags {
                general: vec![Scored {
                    name: name.into(),
                    score,
                }],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn tags_are_saved_next_to_the_image_and_read_back() {
        let dir = tempfile_dir();
        let img = dir.join("a.png");
        let store = AiTagStore::new(None);
        store.update(&img, |e| *e = entry("1girl", 0.9)).unwrap();
        assert!(dir.join(FILE_NAME).exists());
        let fresh = AiTagStore::new(None);
        assert_eq!(fresh.get(&img), Some(entry("1girl", 0.9)));
        assert_eq!(fresh.get(&dir.join("b.png")), None);
    }

    #[test]
    fn updating_faces_keeps_the_tags() {
        let dir = tempfile_dir();
        let img = dir.join("a.png");
        let store = AiTagStore::new(None);
        store.update(&img, |e| *e = entry("1girl", 0.9)).unwrap();
        store
            .update(&img, |e| {
                e.faces = vec![FaceMark {
                    rect: [0.1, 0.1, 0.2, 0.2],
                    score: 0.95,
                    person: Some("A".into()),
                    similarity: 0.5,
                    unlinked: false,
                    hidden: false,
                }]
            })
            .unwrap();
        let got = AiTagStore::new(None).get(&img).unwrap();
        assert_eq!(got.tags.general[0].name, "1girl");
        assert_eq!(got.people(), ["A"]);
    }

    #[test]
    fn broken_file_is_reported_and_kept_as_backup() {
        let dir = tempfile_dir();
        std::fs::write(dir.join(FILE_NAME), "{not json").unwrap();
        let store = AiTagStore::new(None);
        assert_eq!(store.get(&dir.join("a.png")), None);
        assert_eq!(store.take_problems().len(), 1);
        store
            .update(&dir.join("a.png"), |e| *e = entry("x", 0.5))
            .unwrap();
        assert!(dir.join(".vw-ai-tags.json.bak").exists());
    }

    #[test]
    fn removing_every_entry_deletes_the_file() {
        let dir = tempfile_dir();
        let (a, b) = (dir.join("a.png"), dir.join("b.png"));
        let store = AiTagStore::new(None);
        store.update(&a, |e| *e = entry("cat", 0.9)).unwrap();
        store.update(&b, |e| *e = entry("dog", 0.9)).unwrap();
        assert_eq!(store.remove(std::slice::from_ref(&a)).unwrap(), 1);
        assert_eq!(AiTagStore::new(None).get(&a), None);
        assert!(AiTagStore::new(None).get(&b).is_some());
        assert_eq!(store.remove(&[a, b]).unwrap(), 1);
        assert!(!dir.join(FILE_NAME).exists());
    }

    #[test]
    fn removing_a_tag_from_many_images_writes_each_folder_once() {
        let dir = tempfile_dir();
        let (a, b, c) = (dir.join("a.png"), dir.join("b.png"), dir.join("c.png"));
        let store = AiTagStore::new(None);
        store.update(&a, |e| *e = entry("cat", 0.9)).unwrap();
        store.update(&b, |e| *e = entry("dog", 0.9)).unwrap();
        let names = ["cat".to_owned()];
        let n = store
            .edit_all(&[a.clone(), b.clone(), c], |e| e.tags.remove(&names))
            .unwrap();
        assert_eq!(n, 1);
        let again = AiTagStore::new(None);
        assert!(again.get(&a).unwrap().tags.general.is_empty());
        assert_eq!(again.get(&b).unwrap().tags.general[0].name, "dog");
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vw-ai-tags-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn tags_and_faces_made_before_versions_were_kept_count_as_the_first_version() {
        use crate::ai_models::{faces_version, tagger_version, FACES_NAME, TAGGER_NAME};
        let old = Entry {
            model: TAGGER_NAME.into(),
            faces: vec![FaceMark {
                rect: [0.1, 0.1, 0.2, 0.2],
                score: 0.9,
                person: None,
                similarity: 0.0,
                unlinked: false,
                hidden: false,
            }],
            ..Default::default()
        };
        assert_eq!(old.tagger(), Some((TAGGER_NAME, tagger_version())));
        assert_eq!(old.face_finder(), Some((FACES_NAME, faces_version())));
        assert!(!old.tags_outdated());
        assert!(!old.faces_outdated());
        assert_eq!(Entry::default().tagger(), None);
        assert_eq!(Entry::default().face_finder(), None);
    }

    #[test]
    fn another_model_or_version_is_outdated() {
        use crate::ai_models::{tagger_version, FACES_NAME, TAGGER_NAME};
        let other_version = Entry {
            model: TAGGER_NAME.into(),
            version: "00000000".into(),
            ..Default::default()
        };
        assert!(other_version.tags_outdated());
        let other_model = Entry {
            model: "wd-eva02-large-tagger-v3".into(),
            version: tagger_version().into(),
            ..Default::default()
        };
        assert!(other_model.tags_outdated());
        let faces = Entry {
            face_model: FACES_NAME.into(),
            face_version: "00000000".into(),
            ..Default::default()
        };
        assert!(faces.faces_outdated());
        assert!(!faces.tags_outdated());
    }
}
