//! スロットの領域の下の外側に置く「外壁の帯」の部品と並び（設定 `[wall_bar]`）。
//!
//! 外壁の帯には、1 つのスロットではなくフォルダ全体のスロットに効くコマンド（並び順、
//! 混ざり方、レイアウト、ページ）と、選んでいるスロットの再生を集める。

use serde::{Deserialize, Serialize};

use crate::layout::SlotLayout;

/// 外壁の帯に置く部品。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallPart {
    /// 再生／止める、速さ、最後まで行ったら（選んでいるスロット）。
    Play,
    /// ファイルの並びの基準と向き。
    Sort,
    /// 種類が混ざったときの並べ方（1 列／種類ごと）。
    Mix,
    /// レイアウト（決まった形と n×m のマス目）。
    Layout,
    /// スロットの数とページ。
    Pages,
}

impl WallPart {
    pub const ALL: [WallPart; 5] = [
        WallPart::Play,
        WallPart::Sort,
        WallPart::Mix,
        WallPart::Layout,
        WallPart::Pages,
    ];

    pub fn title(self) -> &'static str {
        match self {
            WallPart::Play => "Playback",
            WallPart::Sort => "File order",
            WallPart::Mix => "When file types are mixed",
            WallPart::Layout => "Layout",
            WallPart::Pages => "Pages",
        }
    }

    pub fn id(self) -> String {
        match toml::Value::try_from(self) {
            Ok(toml::Value::String(s)) => s,
            _ => unreachable!("WallPart serializes as a string"),
        }
    }

    pub fn from_id(id: &str) -> Option<WallPart> {
        toml::Value::String(id.to_owned()).try_into().ok()
    }
}

/// 帯の左寄せと右寄せ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// `[wall_bar]`: 外壁の帯を出すかと、部品の並び。左寄せも右寄せも、左から右へ読んだ順
/// （設定ファイルと見た目の順を同じにするため）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WallBar {
    pub show: bool,
    pub left: Vec<WallPart>,
    pub right: Vec<WallPart>,
}

impl Default for WallBar {
    fn default() -> Self {
        Self {
            show: true,
            left: vec![
                WallPart::Play,
                WallPart::Sort,
                WallPart::Mix,
                WallPart::Layout,
            ],
            right: vec![WallPart::Pages],
        }
    }
}

impl WallBar {
    pub fn side(&self, side: Side) -> &[WallPart] {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    fn side_mut(&mut self, side: Side) -> &mut Vec<WallPart> {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }

    /// 帯に出している部品か。帯を隠していれば、どれも出していない。
    pub fn shows(&self, part: WallPart) -> bool {
        self.show && (self.left.contains(&part) || self.right.contains(&part))
    }

    /// どちらにも置いていない部品。
    pub fn hidden(&self) -> Vec<WallPart> {
        WallPart::ALL
            .into_iter()
            .filter(|p| !self.left.contains(p) && !self.right.contains(p))
            .collect()
    }

    /// 同じ部品を 2 回置かない。外したものを返す（設定の報告用）。
    pub fn tidy(&mut self) -> Vec<WallPart> {
        let mut seen = Vec::new();
        let mut dropped = Vec::new();
        for side in [Side::Left, Side::Right] {
            self.side_mut(side).retain(|&p| {
                let keep = !seen.contains(&p);
                if keep {
                    seen.push(p);
                } else {
                    dropped.push(p);
                }
                keep
            });
        }
        dropped
    }

    /// `part` を `side` の `index` 番目に置く（別の場所にあれば外してから）。
    pub fn place(&mut self, part: WallPart, side: Side, index: usize) {
        self.remove(part);
        let list = self.side_mut(side);
        list.insert(index.min(list.len()), part);
    }

    pub fn remove(&mut self, part: WallPart) {
        self.left.retain(|&p| p != part);
        self.right.retain(|&p| p != part);
    }
}

/// レイアウトを選ぶマス目で、左上の角から `offset`（マスの大きさ `cell` の単位）の所を指したときの
/// 列数と行数。マス目の外を指したら端のマスに丸める。
pub fn grid_at(offset: (f32, f32), cell: f32) -> SlotLayout {
    let side =
        |v: f32| ((v / cell).floor() as i64 + 1).clamp(1, SlotLayout::MAX_SIDE as i64) as usize;
    SlotLayout::grid(side(offset.0), side(offset.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_puts_commands_left_and_pages_right() {
        let bar = WallBar::default();
        assert!(bar.show);
        assert_eq!(bar.right, [WallPart::Pages]);
        assert!(bar.hidden().is_empty());
    }

    #[test]
    fn placing_a_part_moves_it_from_its_old_place() {
        let mut bar = WallBar::default();
        bar.place(WallPart::Play, Side::Right, 0);
        assert_eq!(bar.right, [WallPart::Play, WallPart::Pages]);
        assert!(!bar.left.contains(&WallPart::Play));
    }

    #[test]
    fn removed_part_is_listed_as_hidden_and_not_shown() {
        let mut bar = WallBar::default();
        bar.remove(WallPart::Pages);
        assert_eq!(bar.hidden(), [WallPart::Pages]);
        assert!(!bar.shows(WallPart::Pages));
    }

    #[test]
    fn hiding_the_bar_shows_no_part() {
        let bar = WallBar {
            show: false,
            ..WallBar::default()
        };
        assert!(WallPart::ALL.iter().all(|&p| !bar.shows(p)));
    }

    #[test]
    fn tidy_drops_a_part_placed_twice() {
        let mut bar = WallBar {
            show: true,
            left: vec![WallPart::Sort, WallPart::Sort],
            right: vec![WallPart::Sort, WallPart::Pages],
        };
        assert_eq!(bar.tidy(), [WallPart::Sort, WallPart::Sort]);
        assert_eq!(bar.left, [WallPart::Sort]);
        assert_eq!(bar.right, [WallPart::Pages]);
    }

    #[test]
    fn part_ids_round_trip() {
        for part in WallPart::ALL {
            assert_eq!(WallPart::from_id(&part.id()), Some(part));
        }
        assert_eq!(WallPart::from_id("nope"), None);
    }

    #[test]
    fn grid_cell_under_the_pointer_gives_columns_and_rows() {
        assert_eq!(grid_at((0.0, 0.0), 10.0), SlotLayout::grid(1, 1));
        assert_eq!(grid_at((25.0, 9.9), 10.0), SlotLayout::grid(3, 1));
        assert_eq!(grid_at((10.0, 30.0), 10.0), SlotLayout::grid(2, 4));
    }

    #[test]
    fn grid_outside_the_cells_is_clamped_to_the_edge() {
        assert_eq!(grid_at((-5.0, 500.0), 10.0), SlotLayout::grid(1, 8));
    }
}
