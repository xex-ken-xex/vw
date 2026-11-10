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

/// 1 つの部品の、文字ありとアイコンだけの幅。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartWidth {
    pub full: f32,
    pub icons: f32,
}

/// 帯の部品をどう詰めるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    /// 左寄せ・右寄せを左から通して数えて、前からいくつをアイコンだけにするか。
    pub icons: usize,
    /// 左寄せの部品のうち、帯に出す数。残りは「…」のメニューにしまう。
    pub left_shown: usize,
}

/// 帯の幅の測り方。部品の幅は、それぞれのあとの隙間を含めて測る。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    pub available: f32,
    /// 部品の間の区切りの幅。
    pub separator: f32,
    /// 左寄せと右寄せの間の空き。
    pub gap: f32,
    /// 「…」のボタンの幅。
    pub more: f32,
}

/// 帯に入るよう、左の部品から順にアイコンだけにし、それでも入らなければ左寄せの部品を後ろから
/// 「…」にしまう。右寄せの部品（ページ）はしまわない。
pub fn fit(left: &[PartWidth], right: &[PartWidth], room: Room) -> Fit {
    let all = left.len() + right.len();
    let width = |icons: usize, left_shown: usize| {
        let pick = |i: usize, w: &PartWidth| if i < icons { w.icons } else { w.full };
        let more = left_shown < left.len();
        let left_items = left_shown + usize::from(more);
        let left_width: f32 = left[..left_shown]
            .iter()
            .enumerate()
            .map(|(i, w)| pick(i, w))
            .sum::<f32>()
            + if more { room.more } else { 0.0 }
            + left_items.saturating_sub(1) as f32 * room.separator;
        let right_width: f32 = right
            .iter()
            .enumerate()
            .map(|(i, w)| pick(left.len() + i, w))
            .sum::<f32>()
            + right.len().saturating_sub(1) as f32 * room.separator;
        let gap = if left_items > 0 && !right.is_empty() {
            room.gap
        } else {
            0.0
        };
        left_width + gap + right_width
    };
    for icons in 0..=all {
        if width(icons, left.len()) <= room.available {
            return Fit {
                icons,
                left_shown: left.len(),
            };
        }
    }
    let left_shown = (0..left.len())
        .rev()
        .find(|&shown| width(all, shown) <= room.available)
        .unwrap_or(0);
    Fit {
        icons: all,
        left_shown,
    }
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

    const ROOM: Room = Room {
        available: 100.0,
        separator: 5.0,
        gap: 10.0,
        more: 10.0,
    };

    fn widths(list: &[(f32, f32)]) -> Vec<PartWidth> {
        list.iter()
            .map(|&(full, icons)| PartWidth { full, icons })
            .collect()
    }

    #[test]
    fn everything_fits_with_text_when_there_is_room() {
        let left = widths(&[(30.0, 10.0), (30.0, 10.0)]);
        let right = widths(&[(20.0, 10.0)]);
        // 30 + 5 + 30 + 10 + 20 = 95
        assert_eq!(
            fit(&left, &right, ROOM),
            Fit {
                icons: 0,
                left_shown: 2
            }
        );
    }

    #[test]
    fn narrow_bar_turns_parts_into_icons_from_the_left() {
        let left = widths(&[(40.0, 10.0), (40.0, 10.0)]);
        let right = widths(&[(20.0, 10.0)]);
        // 文字ありは 40 + 5 + 40 + 10 + 20 = 115、先頭をアイコンにすると 85
        assert_eq!(
            fit(&left, &right, ROOM),
            Fit {
                icons: 1,
                left_shown: 2
            }
        );
    }

    #[test]
    fn parts_that_do_not_fit_even_as_icons_go_into_the_more_menu() {
        let left = widths(&[(40.0, 30.0), (40.0, 30.0), (40.0, 30.0)]);
        let right = widths(&[(20.0, 20.0)]);
        // アイコンだけで 3 つは 30 * 3 + 5 * 2 + 10 + 20 = 130。2 つと「…」で 30 * 2 + 10 + 5 * 2 + 10 + 20 = 110、
        // 1 つと「…」で 30 + 10 + 5 + 10 + 20 = 75
        assert_eq!(
            fit(&left, &right, ROOM),
            Fit {
                icons: 4,
                left_shown: 1
            }
        );
    }

    #[test]
    fn right_parts_are_never_put_away() {
        let left = widths(&[(40.0, 40.0)]);
        let right = widths(&[(200.0, 150.0)]);
        assert_eq!(
            fit(&left, &right, ROOM),
            Fit {
                icons: 2,
                left_shown: 0
            }
        );
    }
}
