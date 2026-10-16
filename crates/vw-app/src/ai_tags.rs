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

use crate::tagger::Tags;

pub const FILE_NAME: &str = ".vw-ai-tags.json";
const VERSION: u32 = 1;

/// 1 枚の画像の AI タグと、付けたモデルの名前。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub model: String,
    #[serde(flatten)]
    pub tags: Tags,
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

    pub fn get(&self, path: &Path) -> Option<Entry> {
        let (dir, name) = split(path)?;
        self.with_dir(dir, |d| d.file.files.get(&name).cloned())
    }

    pub fn set(&mut self, path: &Path, entry: Entry) -> Result<(), String> {
        let (dir, name) =
            split(path).ok_or_else(|| format!("{}: not a file in a folder", path.display()))?;
        self.with_dir(dir, |_| ());
        {
            let mut dirs = self.dirs.borrow_mut();
            let files = &mut dirs.get_mut(dir).expect("loaded above").file.files;
            files.insert(name, entry);
        }
        self.save(dir)
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
        }
    }

    #[test]
    fn tags_are_saved_next_to_the_image_and_read_back() {
        let dir = tempfile_dir();
        let img = dir.join("a.png");
        let mut store = AiTagStore::new(None);
        store.set(&img, entry("1girl", 0.9)).unwrap();
        assert!(dir.join(FILE_NAME).exists());
        let fresh = AiTagStore::new(None);
        assert_eq!(fresh.get(&img), Some(entry("1girl", 0.9)));
        assert_eq!(fresh.get(&dir.join("b.png")), None);
    }

    #[test]
    fn broken_file_is_reported_and_kept_as_backup() {
        let dir = tempfile_dir();
        std::fs::write(dir.join(FILE_NAME), "{not json").unwrap();
        let mut store = AiTagStore::new(None);
        assert_eq!(store.get(&dir.join("a.png")), None);
        assert_eq!(store.take_problems().len(), 1);
        store.set(&dir.join("a.png"), entry("x", 0.5)).unwrap();
        assert!(dir.join(".vw-ai-tags.json.bak").exists());
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
}
