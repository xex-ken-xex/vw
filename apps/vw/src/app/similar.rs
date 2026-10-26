//! 似た画像の組。スロットに出ている画像から組を探し（[`vw_compare::pairs`]）、見出しに
//! 印を出す。印のクリックで、その組を比べる。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

use eframe::egui::Color32;
use vw_compare::pairs::{self, Item};

use super::VwApp;
use crate::viewer::{SimilarMark, Slot};

/// 組の印の色。隣り合う組が見分けやすいよう、色相を離して並べる。
const COLORS: [Color32; 6] = [
    Color32::from_rgb(0, 200, 230),
    Color32::from_rgb(240, 160, 40),
    Color32::from_rgb(170, 120, 255),
    Color32::from_rgb(90, 210, 90),
    Color32::from_rgb(255, 110, 150),
    Color32::from_rgb(220, 220, 80),
];

#[derive(Default)]
pub(super) struct Similar {
    /// 前に組を探したときのスロットの中身。変わったときだけ探し直す。
    signature: u64,
    /// スロットの番号（[`Slot::id`]）ごとの印と、同じ組のスロット。
    marks: HashMap<u64, (SimilarMark, Vec<u64>)>,
}

impl Similar {
    pub(super) fn mark(&self, slot: u64) -> Option<&SimilarMark> {
        self.marks.get(&slot).map(|(m, _)| m)
    }

    /// `slot` と同じ組のスロット（`slot` を含む）。
    pub(super) fn group(&self, slot: u64) -> Option<&[u64]> {
        self.marks.get(&slot).map(|(_, g)| g.as_slice())
    }
}

impl VwApp {
    /// スロットの中身が変わっていれば、似た画像の組を探し直す。描く前に毎フレーム呼ぶ。
    pub(super) fn update_similar(&mut self) {
        let candidates: Vec<(&Slot, &Path)> = self
            .slots
            .iter()
            .filter(|s| s.folder.is_none())
            .filter_map(|s| Some((s, s.shown.as_ref()?.path.as_path())))
            .collect();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (slot, path) in &candidates {
            slot.id.hash(&mut h);
            path.hash(&mut h);
            slot.shown.as_ref().map(|s| s.info.fingerprint).hash(&mut h);
        }
        let signature = h.finish();
        if signature == self.similar.signature {
            return;
        }
        let items: Vec<Item> = candidates
            .iter()
            .map(|(slot, path)| item(slot, path))
            .collect();
        let mut marks = HashMap::new();
        for (n, group) in pairs::groups(&items).into_iter().enumerate() {
            let ids: Vec<u64> = group.iter().map(|&i| candidates[i].0.id).collect();
            for &i in &group {
                let others: Vec<String> = group
                    .iter()
                    .filter(|&&j| j != i)
                    .map(|&j| crate::compare::file_name(candidates[j].1))
                    .collect();
                let mark = SimilarMark {
                    number: n + 1,
                    color: COLORS[n % COLORS.len()],
                    others: others.join(", "),
                };
                marks.insert(candidates[i].0.id, (mark, ids.clone()));
            }
        }
        self.similar = Similar { signature, marks };
    }
}

/// 組を探すための、スロットに出ている画像の名前・撮影日時・カメラ・指紋。
fn item(slot: &Slot, path: &Path) -> Item {
    let file = crate::frames::file_of(path);
    let meta = slot.shown.as_ref().map(|s| &s.info.metadata);
    let get = |group: &str, key: &str| {
        meta.and_then(|m| m.get(group, key))
            .map(str::to_owned)
            .filter(|v| !v.trim().is_empty())
    };
    // フレームを指すパスは、同じファイルのフレームどうしが名前で組にならないよう、
    // フレームの番号も名前に入れる。
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = match crate::frames::split(path) {
        Some((_, frame)) => format!("{stem}#{frame}"),
        None => stem,
    };
    Item {
        folder: file
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        stem,
        taken: get("EXIF", "DateTimeOriginal").or_else(|| get("RAW", "Date taken")),
        camera: get("EXIF", "Model").or_else(|| get("RAW", "Model")),
        fingerprint: slot.shown.as_ref().and_then(|s| s.info.fingerprint),
    }
}
