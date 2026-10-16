//! 見つけた顔の特徴ベクトルと縮小画像。フォルダごとに 1 つのファイルを、
//! 設定フォルダの `faces` の下に置く。
//!
//! 画像のフォルダに置かないのは、顔の特徴ベクトルが個人を見分けられる情報で、
//! フォルダごと人に渡したり共有ドライブに置いたりしたときに一緒に出ていかないようにするため。
//! 枠と人物名は `.vw-ai-tags.json` にあり、ここの並びはその枠の並びと同じ。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::people::Sample;

const VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
struct FaceFile {
    version: u32,
    /// 元のフォルダ。ファイル名のハッシュだけではどのフォルダか分からないため。
    #[serde(default)]
    folder: PathBuf,
    #[serde(default)]
    files: BTreeMap<String, Vec<Sample>>,
}

pub struct FaceStore {
    dir: Option<PathBuf>,
    cache: RefCell<HashMap<PathBuf, FaceFile>>,
}

impl FaceStore {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            cache: RefCell::default(),
        }
    }

    fn path_of(&self, folder: &Path) -> Option<PathBuf> {
        Some(
            self.dir
                .as_ref()?
                .join(crate::ai_tags::folder_file_name(folder)),
        )
    }

    fn with<T>(&self, folder: &Path, f: impl FnOnce(&mut FaceFile) -> T) -> T {
        let mut cache = self.cache.borrow_mut();
        let file = cache.entry(folder.to_path_buf()).or_insert_with(|| {
            self.path_of(folder)
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        });
        f(file)
    }

    /// 画像の `index` 番目の顔。
    pub fn get(&self, image: &Path, index: usize) -> Option<Sample> {
        let (folder, name) = (image.parent()?, image.file_name()?.to_str()?.to_owned());
        self.with(folder, |f| {
            f.files.get(&name).and_then(|v| v.get(index).cloned())
        })
    }

    /// 画像の顔を置き換えて保存する。
    pub fn set(&self, image: &Path, samples: Vec<Sample>) -> Result<(), String> {
        let folder = image
            .parent()
            .ok_or_else(|| format!("{}: not in a folder", image.display()))?;
        let name = image
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("{}: no file name", image.display()))?
            .to_owned();
        let path = self
            .path_of(folder)
            .ok_or("the settings folder is unknown")?;
        let text = self.with(folder, |f| {
            f.version = VERSION;
            f.folder = folder.to_path_buf();
            if samples.is_empty() {
                f.files.remove(&name);
            } else {
                f.files.insert(name, samples);
            }
            serde_json::to_string(f).map_err(|e| e.to_string())
        })?;
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_are_kept_outside_the_image_folder() {
        let base = std::env::temp_dir().join(format!("vw-face-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let images = base.join("photos");
        std::fs::create_dir_all(&images).unwrap();
        let store = FaceStore::new(Some(base.join("faces")));
        let sample = Sample {
            feature: vec![1.0, 0.0],
            thumb: vec![9; 3],
            source: None,
        };
        store
            .set(&images.join("a.jpg"), vec![sample.clone()])
            .unwrap();
        assert_eq!(std::fs::read_dir(&images).unwrap().count(), 0);
        let again = FaceStore::new(Some(base.join("faces")));
        assert_eq!(again.get(&images.join("a.jpg"), 0), Some(sample));
        assert_eq!(again.get(&images.join("a.jpg"), 1), None);
    }
}
