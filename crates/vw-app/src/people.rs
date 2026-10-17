//! 人物：名前と、登録した顔の特徴ベクトル・縮小画像。設定フォルダの `people.json` に置く。
//!
//! 顔の特徴ベクトルは個人を見分けられる情報なので、画像のフォルダやデータセットの
//! カタログには書かず、この PC の設定フォルダの中だけに置く。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::faces::{cosine, normalized, FaceMark};

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
    /// 最初に登録した時刻（UNIX 秒）。この項目のない古いファイルでは 0。
    #[serde(default)]
    pub added: i64,
    /// 一覧に出す代表の顔（`faces` の番号）。
    #[serde(default)]
    pub cover: usize,
}

impl Person {
    pub fn cover(&self) -> Option<&Sample> {
        self.faces.get(self.cover).or_else(|| self.faces.first())
    }

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

/// 人物の一覧の並べ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    #[default]
    OldestFirst,
    NewestFirst,
    FewestImages,
    MostImages,
}

impl Order {
    pub const ALL: [Order; 4] = [
        Order::OldestFirst,
        Order::NewestFirst,
        Order::FewestImages,
        Order::MostImages,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Order::OldestFirst => "Registered: oldest first",
            Order::NewestFirst => "Registered: newest first",
            Order::FewestImages => "Images: fewest first",
            Order::MostImages => "Images: most first",
        }
    }
}

/// 名前に `query` を含む人物を `order` で並べる（大文字小文字は区別しない）。
/// `images` は人物ごとの写っている画像の枚数。同じ値のときは名前順。
pub fn arrange<'a>(
    list: &'a [Person],
    query: &str,
    images: &HashMap<String, usize>,
    order: Order,
) -> Vec<&'a Person> {
    let query = query.trim().to_lowercase();
    let mut out: Vec<&Person> = list
        .iter()
        .filter(|p| p.name.to_lowercase().contains(&query))
        .collect();
    let count = |p: &Person| images.get(&p.name).copied().unwrap_or(0);
    out.sort_by(|a, b| {
        let by = match order {
            Order::OldestFirst => a.added.cmp(&b.added),
            Order::NewestFirst => b.added.cmp(&a.added),
            Order::FewestImages => count(a).cmp(&count(b)),
            Order::MostImages => count(b).cmp(&count(a)),
        };
        by.then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// 顔の枠から、人物ごとの写っている画像の枚数を数える。1 枚に同じ人が 2 回写っていても 1 枚。
pub fn image_counts<'a>(
    images: impl IntoIterator<Item = &'a [FaceMark]>,
) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for marks in images {
        let mut seen: Vec<&str> = Vec::new();
        for name in marks.iter().filter_map(|m| m.person.as_deref()) {
            if !seen.contains(&name) {
                seen.push(name);
                *counts.entry(name.to_owned()).or_insert(0) += 1;
            }
        }
    }
    counts
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
                added: chrono::Local::now().timestamp(),
                cover: 0,
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

    pub fn set_cover(&mut self, name: &str, index: usize) {
        if let Some(p) = self.list.iter_mut().find(|p| p.name == name) {
            if index < p.faces.len() {
                p.cover = index;
            }
        }
    }

    /// 前に見つけた顔の枠を、今の登録に合わせ直したものと、新しく名前が付いた数。
    /// `samples` は枠と同じ並びの特徴。`renamed` は (古い名前, 新しい名前)。
    pub fn rematch(
        &self,
        marks: &[FaceMark],
        samples: &[Sample],
        threshold: f32,
        renamed: Option<(&str, &str)>,
    ) -> (Vec<FaceMark>, usize) {
        let names = self.names();
        let mut matched = 0;
        let mut out = marks.to_vec();
        for (i, mark) in out.iter_mut().enumerate() {
            if let (Some((from, to)), Some(person)) = (renamed, &mark.person) {
                if person == from {
                    mark.person = Some(to.to_owned());
                }
            }
            if mark.person.as_ref().is_some_and(|p| !names.contains(p)) {
                mark.person = None;
                mark.similarity = 0.0;
            }
            if mark.person.is_none() && !mark.unlinked {
                let found = samples
                    .get(i)
                    .and_then(|s| self.best_match(&s.feature, threshold));
                if let Some((name, similarity)) = found {
                    mark.person = Some(name);
                    mark.similarity = similarity;
                    matched += 1;
                }
            }
        }
        (out, matched)
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

    fn mark(person: Option<&str>, unlinked: bool) -> FaceMark {
        FaceMark {
            rect: [0.0, 0.0, 0.1, 0.1],
            score: 0.9,
            person: person.map(str::to_owned),
            similarity: if person.is_some() { 1.0 } else { 0.0 },
            unlinked,
            hidden: false,
        }
    }

    #[test]
    fn rematch_names_earlier_unknown_faces_but_not_unlinked_ones() {
        let mut people = People::default();
        people.add("A", sample([1.0, 0.0]));
        let marks = [mark(None, false), mark(None, true), mark(None, false)];
        let samples = [sample([0.9, 0.1]), sample([1.0, 0.0]), sample([0.0, 1.0])];
        let (out, n) = people.rematch(&marks, &samples, 0.36, None);
        assert_eq!(n, 1);
        assert_eq!(out[0].person.as_deref(), Some("A"));
        assert_eq!(out[1].person, None, "the person chose to unlink it");
        assert_eq!(out[2].person, None, "not similar enough");
    }

    #[test]
    fn rematch_follows_renames_and_drops_deleted_people() {
        let mut people = People::default();
        people.add("C", sample([1.0, 0.0]));
        let marks = [mark(Some("A"), false), mark(Some("Gone"), false)];
        let samples = [sample([1.0, 0.0]), sample([0.0, 1.0])];
        let (out, n) = people.rematch(&marks, &samples, 0.36, Some(("A", "C")));
        assert_eq!(out[0].person.as_deref(), Some("C"));
        assert_eq!(
            out[0].similarity, 1.0,
            "a hand-registered face keeps its similarity"
        );
        assert_eq!(out[1].person, None);
        assert_eq!(n, 0);
    }

    fn person(name: &str, added: i64) -> Person {
        Person {
            name: name.into(),
            faces: vec![sample([1.0, 0.0])],
            added,
            cover: 0,
        }
    }

    #[test]
    fn arrange_filters_by_name_and_sorts_by_date_or_images() {
        let list = [
            person("Taro", 30),
            person("Hanako", 10),
            person("taichi", 20),
        ];
        let images = HashMap::from([("Taro".to_owned(), 5), ("Hanako".to_owned(), 9)]);
        let names = |v: Vec<&Person>| v.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(
            names(arrange(&list, "ta", &images, Order::OldestFirst)),
            ["taichi", "Taro"]
        );
        assert_eq!(
            names(arrange(&list, "", &images, Order::NewestFirst)),
            ["Taro", "taichi", "Hanako"]
        );
        assert_eq!(
            names(arrange(&list, "", &images, Order::MostImages)),
            ["Hanako", "Taro", "taichi"]
        );
        assert_eq!(
            names(arrange(&list, "", &images, Order::FewestImages)),
            ["taichi", "Taro", "Hanako"]
        );
    }

    #[test]
    fn an_image_counts_once_per_person() {
        let a = [
            mark(Some("A"), false),
            mark(Some("A"), false),
            mark(Some("B"), false),
        ];
        let b = [mark(Some("A"), false), mark(None, false)];
        let counts = image_counts([&a[..], &b[..]]);
        assert_eq!(counts["A"], 2);
        assert_eq!(counts["B"], 1);
    }

    #[test]
    fn the_cover_falls_back_to_the_first_face() {
        let mut people = People::default();
        people.add("A", sample([1.0, 0.0]));
        people.add("A", sample([0.0, 1.0]));
        people.set_cover("A", 1);
        assert_eq!(
            people.list[0].cover().unwrap().feature,
            sample([0.0, 1.0]).feature
        );
        people.set_cover("A", 9);
        assert_eq!(people.list[0].cover, 1, "an index out of range is ignored");
        people.list[0].faces.truncate(1);
        assert_eq!(
            people.list[0].cover().unwrap().feature,
            sample([1.0, 0.0]).feature
        );
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
