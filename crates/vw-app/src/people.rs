//! 人物：名前と、登録した顔の特徴ベクトル・縮小画像。設定フォルダの `people.json` に置く。
//!
//! 顔の特徴ベクトルは個人を見分けられる情報なので、画像のフォルダやデータセットの
//! カタログには書かず、この PC の設定フォルダの中だけに置く。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::faces::{cosine, normalized};

pub const FILE_NAME: &str = "people.json";
const VERSION: u32 = 1;

/// 特徴ベクトルと縮小画像は、数字の並びのままだとファイルが数倍に膨らむので base64 で持つ。
mod packed {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn floats<S: Serializer>(v: &[f32], s: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn floats_de<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f32>, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(serde::de::Error::custom)?;
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect())
    }

    pub fn bytes<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }

    pub fn bytes_de<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(serde::de::Error::custom)
    }
}

/// 1 つの顔の特徴と縮小画像（[`crate::faces::THUMB_SIDE`] 角の RGB）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    #[serde(
        serialize_with = "packed::floats",
        deserialize_with = "packed::floats_de"
    )]
    pub feature: Vec<f32>,
    #[serde(
        serialize_with = "packed::bytes",
        deserialize_with = "packed::bytes_de"
    )]
    pub thumb: Vec<u8>,
    /// 顔のあった画像。人物の一覧から元の画像を開くため。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    #[serde(default)]
    pub faces: Vec<Sample>,
}

impl Person {
    /// 登録した顔の特徴の平均（長さ 1）。
    pub fn mean(&self) -> Option<Vec<f32>> {
        let first = self.faces.first()?;
        let mut sum = vec![0.0; first.feature.len()];
        for s in &self.faces {
            for (a, b) in sum.iter_mut().zip(&s.feature) {
                *a += b;
            }
        }
        Some(normalized(sum))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PeopleFile {
    version: u32,
    #[serde(default)]
    people: Vec<Person>,
}

#[derive(Default)]
pub struct People {
    path: Option<PathBuf>,
    pub list: Vec<Person>,
}

impl People {
    /// 読む。読めなかったときは空で始め、その理由を返す（壊れたファイルは保存時に .bak へ退避）。
    pub fn open(settings_dir: Option<&Path>) -> (Self, Option<String>) {
        let path = settings_dir.map(|d| d.join(FILE_NAME));
        let mut problem = None;
        let list = match path.as_ref().map(std::fs::read_to_string) {
            Some(Ok(text)) => match serde_json::from_str::<PeopleFile>(&text) {
                Ok(f) => f.people,
                Err(e) => {
                    let p = path.as_ref().expect("read above");
                    problem = Some(format!("{}: cannot read people ({e})", p.display()));
                    let _ = std::fs::rename(p, p.with_extension("json.bak"));
                    Vec::new()
                }
            },
            _ => Vec::new(),
        };
        (Self { path, list }, problem)
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Err("the settings folder is unknown".into());
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = PeopleFile {
            version: VERSION,
            people: self.list.clone(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn names(&self) -> Vec<String> {
        self.list.iter().map(|p| p.name.clone()).collect()
    }

    /// 一番似ている人物と類似度。`threshold` に届かなければ `None`。
    pub fn best_match(&self, feature: &[f32], threshold: f32) -> Option<(String, f32)> {
        self.list
            .iter()
            .filter_map(|p| Some((p.name.clone(), cosine(&p.mean()?, feature))))
            .filter(|(_, s)| *s >= threshold)
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// 顔を人物に足す。いなければ作る。
    pub fn add(&mut self, name: &str, sample: Sample) {
        match self.list.iter_mut().find(|p| p.name == name) {
            Some(p) => p.faces.push(sample),
            None => self.list.push(Person {
                name: name.to_owned(),
                faces: vec![sample],
            }),
        }
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        if from == to {
            return Ok(());
        }
        if self.list.iter().any(|p| p.name == to) {
            return Err(format!("{to} already exists"));
        }
        let p = self
            .list
            .iter_mut()
            .find(|p| p.name == from)
            .ok_or_else(|| format!("{from} not found"))?;
        p.name = to.to_owned();
        Ok(())
    }

    pub fn remove(&mut self, name: &str) {
        self.list.retain(|p| p.name != name);
    }
}

/// 縮小画像を画面に出す egui の画像にする。
pub fn thumb_image(thumb: &[u8]) -> Option<eframe::egui::ColorImage> {
    let side = crate::faces::THUMB_SIDE;
    (thumb.len() == side * side * 3)
        .then(|| eframe::egui::ColorImage::from_rgb([side, side], thumb))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(v: [f32; 2]) -> Sample {
        Sample {
            feature: normalized(v.to_vec()),
            thumb: vec![1, 2, 3],
            source: None,
        }
    }

    #[test]
    fn matches_the_closest_person_above_the_threshold() {
        let mut people = People::default();
        people.add("A", sample([1.0, 0.0]));
        people.add("B", sample([0.0, 1.0]));
        let (name, s) = people
            .best_match(&normalized(vec![0.9, 0.1]), 0.36)
            .unwrap();
        assert_eq!(name, "A");
        assert!(s > 0.9);
        assert!(people
            .best_match(&normalized(vec![-1.0, -1.0]), 0.36)
            .is_none());
    }

    #[test]
    fn mean_averages_the_registered_faces() {
        let mut people = People::default();
        people.add("A", sample([1.0, 0.0]));
        people.add("A", sample([0.0, 1.0]));
        assert_eq!(people.list.len(), 1);
        let m = people.list[0].mean().unwrap();
        assert!((m[0] - m[1]).abs() < 1e-6);
    }

    #[test]
    fn saves_features_compactly_and_reads_them_back() {
        let dir = std::env::temp_dir().join(format!("vw-people-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut people, problem) = People::open(Some(&dir));
        assert!(problem.is_none());
        people.add("A", sample([0.6, 0.8]));
        people.save().unwrap();
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(!text.contains("0.6"), "{text}");
        let (again, _) = People::open(Some(&dir));
        assert_eq!(again.list, people.list);
    }

    #[test]
    fn rename_refuses_an_existing_name() {
        let mut people = People::default();
        people.add("A", sample([1.0, 0.0]));
        people.add("B", sample([0.0, 1.0]));
        assert!(people.rename("A", "B").is_err());
        people.rename("A", "C").unwrap();
        assert_eq!(people.names(), ["C", "B"]);
    }
}
