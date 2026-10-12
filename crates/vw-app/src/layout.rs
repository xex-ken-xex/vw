//! 画像領域にスロットを並べるマス目（レイアウト）と、ページ分け。
//!
//! 設定ファイルには `"auto"` または `"列x行"`（例: `"2x2"`）の文字列で保存する。

use std::fmt;
use std::ops::Range;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// スロットの並べ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum SlotLayout {
    /// スロットの数から、正方形に近いマス目を毎回決める。全スロットが 1 ページに入る。
    #[default]
    Auto,
    /// 決まった列数と行数。入りきらないスロットは次のページに回す。
    Grid { cols: usize, rows: usize },
}

impl SlotLayout {
    /// 1 辺の上限。これより細かく割ると、1 枠が小さすぎて画像が判別できない。
    pub const MAX_SIDE: usize = 8;

    /// メニューに並べる既定のマス目。
    pub const PRESETS: [SlotLayout; 7] = [
        SlotLayout::Auto,
        SlotLayout::grid(1, 1),
        SlotLayout::grid(1, 2),
        SlotLayout::grid(2, 1),
        SlotLayout::grid(2, 2),
        SlotLayout::grid(3, 3),
        SlotLayout::grid(4, 4),
    ];

    pub const fn grid(cols: usize, rows: usize) -> Self {
        SlotLayout::Grid { cols, rows }
    }

    /// `n` 個のスロットを並べるときの (列数, 行数)。
    pub fn dims(self, n: usize) -> (usize, usize) {
        match self {
            SlotLayout::Auto => auto_dims(n),
            SlotLayout::Grid { cols, rows } => (cols, rows),
        }
    }

    /// 1 ページに入るスロットの数。
    pub fn per_page(self, n: usize) -> usize {
        let (cols, rows) = self.dims(n);
        cols * rows
    }

    pub fn page_count(self, n: usize) -> usize {
        n.div_ceil(self.per_page(n)).max(1)
    }

    /// `index` 番目のスロットが載るページ。
    pub fn page_of(self, index: usize, n: usize) -> usize {
        index / self.per_page(n)
    }

    /// `page` ページに載るスロットの添字。
    pub fn page_range(self, page: usize, n: usize) -> Range<usize> {
        let per_page = self.per_page(n);
        let start = (page * per_page).min(n);
        start..(start + per_page).min(n)
    }
}

/// 画面に出すスロットとマス目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// 描くスロットの添字。
    pub range: Range<usize>,
    pub cols: usize,
    pub rows: usize,
    /// 今のページ（0 から）とページ数。最大化中は 1 ページとみなす。
    pub page: usize,
    pub pages: usize,
}

/// `n` 個のスロットのうち、画面に出すもの。アクティブなスロットが載るページを出す。
/// `maximized` があれば、そのスロットだけを 1 マスで出す。
///
/// ページ番号を別に持たずアクティブなスロットから決めるのは、キー操作やツリーから
/// 開いた画像のスロットが、いつも見えているようにするため。
pub fn view(layout: SlotLayout, n: usize, active: usize, maximized: Option<usize>) -> View {
    if let Some(i) = maximized.filter(|i| *i < n) {
        return View {
            range: i..i + 1,
            cols: 1,
            rows: 1,
            page: 0,
            pages: 1,
        };
    }
    let (cols, rows) = layout.dims(n);
    let page = layout.page_of(active.min(n.saturating_sub(1)), n);
    View {
        range: layout.page_range(page, n),
        cols,
        rows,
        page,
        pages: layout.page_count(n),
    }
}

/// ページを `delta` だけ送ったときに、アクティブにするスロット（そのページの先頭）。
/// 端まで行ったら反対側に回り込む。
pub fn page_step(layout: SlotLayout, n: usize, active: usize, delta: isize) -> usize {
    let pages = layout.page_count(n) as isize;
    let page = layout.page_of(active, n) as isize;
    let target = (page + delta).rem_euclid(pages) as usize;
    layout.page_range(target, n).start
}

/// `start` から長さ `len` を `n` 等分した `i` 番目の範囲（始まり, 終わり）。隣との間だけ
/// 1 物理ピクセル（`pixel` ポイント）空け、両端には空けない。境目は物理ピクセルにそろえる。
///
/// 等分した幅をそのまま足していくと、境目が画素の途中に来て、間が 1px にならずににじむため、
/// 境目ごとに丸める。
pub fn cell_span(start: f32, len: f32, n: usize, i: usize, pixel: f32) -> (f32, f32) {
    let edge = |k: usize| {
        if k == n {
            start + len
        } else {
            ((start + len * k as f32 / n as f32) / pixel).round() * pixel
        }
    };
    let gap = if i + 1 < n { pixel } else { 0.0 };
    (edge(i), (edge(i + 1) - gap).max(edge(i)))
}

/// `n` 枚を並べる、正方形に近く横長を優先したマス目の (列数, 行数)。
pub fn auto_dims(n: usize) -> (usize, usize) {
    let n = n.max(1);
    let cols = (n as f64).sqrt().ceil() as usize;
    (cols, n.div_ceil(cols))
}

impl fmt::Display for SlotLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlotLayout::Auto => f.write_str("auto"),
            SlotLayout::Grid { cols, rows } => write!(f, "{cols}x{rows}"),
        }
    }
}

impl FromStr for SlotLayout {
    type Err = String;

    /// `auto` か `列x行` を読む。`x` は大文字や `×` でもよい。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("auto") {
            return Ok(SlotLayout::Auto);
        }
        let side = |t: &str| {
            t.trim()
                .parse::<usize>()
                .ok()
                .filter(|v| (1..=Self::MAX_SIDE).contains(v))
        };
        s.split_once(['x', 'X', '×'])
            .and_then(|(c, r)| Some(SlotLayout::grid(side(c)?, side(r)?)))
            .ok_or_else(|| {
                format!(
                    "{s:?} is not \"auto\" or COLSxROWS (1 to {} each)",
                    Self::MAX_SIDE
                )
            })
    }
}

impl TryFrom<String> for SlotLayout {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<SlotLayout> for String {
    fn from(layout: SlotLayout) -> Self {
        layout.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_grid_is_close_to_square() {
        assert_eq!(auto_dims(1), (1, 1));
        assert_eq!(auto_dims(2), (2, 1));
        assert_eq!(auto_dims(4), (2, 2));
        assert_eq!(auto_dims(5), (3, 2));
        assert_eq!(auto_dims(10), (4, 3));
    }

    #[test]
    fn auto_grid_always_has_room_for_every_slot() {
        for n in 1..200 {
            let (c, r) = auto_dims(n);
            assert!(c * r >= n, "{n} slots in {c}x{r}");
        }
    }

    #[test]
    fn zero_slots_still_gives_one_cell() {
        assert_eq!(auto_dims(0), (1, 1));
    }

    #[test]
    fn auto_layout_fits_every_slot_on_one_page() {
        for n in [1, 7, 500] {
            assert_eq!(SlotLayout::Auto.page_count(n), 1);
            assert_eq!(SlotLayout::Auto.page_range(0, n), 0..n);
        }
    }

    #[test]
    fn fixed_grid_splits_slots_into_pages() {
        let l = SlotLayout::grid(2, 2);
        assert_eq!(l.page_count(9), 3);
        assert_eq!(l.page_range(0, 9), 0..4);
        assert_eq!(l.page_range(1, 9), 4..8);
        assert_eq!(l.page_range(2, 9), 8..9);
    }

    #[test]
    fn page_of_a_slot_is_where_it_is_drawn() {
        let l = SlotLayout::grid(1, 2);
        assert_eq!(l.page_of(0, 5), 0);
        assert_eq!(l.page_of(1, 5), 0);
        assert_eq!(l.page_of(2, 5), 1);
        assert_eq!(l.page_of(4, 5), 2);
    }

    #[test]
    fn fewer_slots_than_cells_is_one_page() {
        let l = SlotLayout::grid(3, 3);
        assert_eq!(l.page_count(2), 1);
        assert_eq!(l.page_range(0, 2), 0..2);
    }

    #[test]
    fn view_shows_the_page_of_the_active_slot() {
        let v = view(SlotLayout::grid(2, 2), 10, 5, None);
        assert_eq!((v.range, v.page, v.pages), (4..8, 1, 3));
        assert_eq!((v.cols, v.rows), (2, 2));
    }

    #[test]
    fn maximized_slot_is_shown_alone() {
        let v = view(SlotLayout::grid(2, 2), 10, 5, Some(7));
        assert_eq!((v.range, v.cols, v.rows, v.pages), (7..8, 1, 1, 1));
    }

    #[test]
    fn maximized_slot_that_no_longer_exists_is_ignored() {
        let v = view(SlotLayout::Auto, 3, 0, Some(3));
        assert_eq!(v.range, 0..3);
    }

    #[test]
    fn auto_view_shows_every_slot() {
        let v = view(SlotLayout::Auto, 5, 4, None);
        assert_eq!((v.range, v.cols, v.rows), (0..5, 3, 2));
    }

    #[test]
    fn page_step_moves_to_the_first_slot_of_the_next_page() {
        let l = SlotLayout::grid(2, 2);
        assert_eq!(page_step(l, 10, 1, 1), 4);
        assert_eq!(page_step(l, 10, 5, -1), 0);
    }

    #[test]
    fn page_step_wraps_around_both_ends() {
        let l = SlotLayout::grid(2, 2);
        assert_eq!(page_step(l, 10, 9, 1), 0);
        assert_eq!(page_step(l, 10, 0, -1), 8);
    }

    #[test]
    fn page_step_on_a_single_page_stays_on_it() {
        assert_eq!(page_step(SlotLayout::Auto, 6, 3, 1), 0);
    }

    #[test]
    fn layout_text_round_trips() {
        for l in SlotLayout::PRESETS {
            assert_eq!(l.to_string().parse::<SlotLayout>(), Ok(l));
        }
    }

    #[test]
    fn layout_accepts_upper_case_and_multiplication_sign() {
        assert_eq!("AUTO".parse(), Ok(SlotLayout::Auto));
        assert_eq!("3X2".parse(), Ok(SlotLayout::grid(3, 2)));
        assert_eq!(" 1 × 4 ".parse(), Ok(SlotLayout::grid(1, 4)));
    }

    #[test]
    fn layout_rejects_empty_or_oversized_grids() {
        for bad in ["", "0x2", "2x0", "9x1", "2x", "x2", "2*2", "grid"] {
            assert!(bad.parse::<SlotLayout>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn cells_leave_one_pixel_between_them_and_none_at_the_edges() {
        let spans: Vec<_> = (0..3).map(|i| cell_span(10.0, 300.0, 3, i, 1.0)).collect();
        assert_eq!(spans, [(10.0, 109.0), (110.0, 209.0), (210.0, 310.0)]);
    }

    #[test]
    fn cell_edges_land_on_physical_pixels_when_scaled() {
        let pixel = 1.0 / 1.25;
        for i in 0..3 {
            let (from, to) = cell_span(0.0, 100.0, 3, i, pixel);
            for x in [from, to] {
                let physical = x / pixel;
                assert!((physical - physical.round()).abs() < 1e-3, "{x}");
            }
        }
        assert_eq!(cell_span(0.0, 100.0, 3, 2, pixel).1, 100.0);
    }
}
