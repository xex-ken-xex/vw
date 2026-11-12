//! スロットに描く表（FITS の BINTABLE・TABLE など、[`vw_core::table::Table`]）。
//!
//! 描くのは見えている行だけ。ホイールで行を、Shift+ホイールで列を送る。行をクリックして選び
//! （Shift で範囲、Ctrl で 1 行ずつ足し引き）、見出しの列の境をドラッグして幅を変える。
//!
//! egui の表の部品を使わずに描くのは、スロットの中ではホイールやドラッグをスロットが受け取って
//! いて（画像を送る・運ぶ）、部品の中のスクロールと取り合いになるため。

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Response, Stroke, Ui, pos2, vec2};
use vw_core::table::Table;

const ROW: f32 = 18.0;
const HEADER: f32 = 34.0;
const PAD: f32 = 6.0;
const BAR: f32 = 10.0;
const MIN_WIDTH: f32 = 36.0;
const MAX_START_WIDTH: f32 = 360.0;
/// 列の幅を決めるのに見る行の数。
const SAMPLE_ROWS: usize = 64;
/// 列の境をつかめる幅（左右それぞれ）。
const GRAB: f32 = 4.0;

fn font() -> FontId {
    FontId::proportional(12.0)
}

/// 選んだ行。範囲の並び（重ならず、小さい順）。数百万行を範囲で選んでも軽いように。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection(Vec<RangeInclusive<usize>>);

impl Selection {
    pub fn contains(&self, row: usize) -> bool {
        self.0.iter().any(|r| r.contains(&row))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn count(&self) -> usize {
        self.0.iter().map(|r| r.end() - r.start() + 1).sum()
    }

    pub fn rows(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().flat_map(|r| r.clone())
    }

    fn only(row: usize) -> Self {
        Self(vec![row..=row])
    }

    fn range(a: usize, b: usize) -> Self {
        Self(vec![a.min(b)..=a.max(b)])
    }

    /// `row` を選んでいれば外し、選んでいなければ足す。
    fn toggle(&mut self, row: usize) {
        let mut out = Vec::new();
        let mut found = false;
        for r in self.0.drain(..) {
            if r.contains(&row) {
                found = true;
                if *r.start() < row {
                    out.push(*r.start()..=row - 1);
                }
                if row < *r.end() {
                    out.push(row + 1..=*r.end());
                }
            } else {
                out.push(r);
            }
        }
        if !found {
            out.push(row..=row);
        }
        out.sort_by_key(|r| *r.start());
        self.0 = merge(out);
    }
}

/// 隣り合う範囲をつなぐ。
fn merge(sorted: Vec<RangeInclusive<usize>>) -> Vec<RangeInclusive<usize>> {
    let mut out: Vec<RangeInclusive<usize>> = Vec::new();
    for r in sorted {
        match out.last_mut() {
            Some(last) if *r.start() <= last.end() + 1 => {
                *last = *last.start()..=(*last.end()).max(*r.end());
            }
            _ => out.push(r),
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Grab {
    /// 列の幅を変えている（列、つかんだときの幅、つかんだ x）。
    Resize(usize, f32, f32),
    /// 縦のつまみをつかんでいる（つかんだときの先頭の行、つかんだ y）。
    Rows(f32, f32),
    /// 横のつまみをつかんでいる（つかんだときの左端、つかんだ x）。
    Columns(f32, f32),
    /// 行をドラッグして選んでいる。
    Select,
}

/// スロットごとの表の見え方。表の場所が変わったら初めに戻す。
#[derive(Debug, Clone, Default)]
pub struct TableView {
    path: Option<PathBuf>,
    /// 表の代わりにスペクトルのグラフを描く（表がスペクトルとしても読めるときだけ効く）。
    pub graph: bool,
    /// いちばん上に見えている行（小数も可）。
    top: f32,
    /// 横に送った量（論理ピクセル）。
    left: f32,
    widths: Vec<f32>,
    pub selection: Selection,
    anchor: Option<usize>,
    grab: Option<Grab>,
}

/// 表を描いた枠の中の場所。
struct Layout {
    body: Rect,
    /// 行の番号の列の幅。
    number: f32,
    /// 行を描く所（見出しとつまみを除く）。
    rows: Rect,
    visible: f32,
    total_width: f32,
    vertical: Option<Rect>,
    horizontal: Option<Rect>,
}

impl TableView {
    /// 表示中の表の場所。違う表に変わったら、送り・幅・選択を初めに戻し、グラフで見るかを
    /// `graph` にする。
    pub fn follow(&mut self, path: &Path, graph: bool) {
        if self.path.as_deref() != Some(path) {
            *self = Self {
                path: Some(path.to_path_buf()),
                graph,
                ..Self::default()
            };
        }
    }

    /// 選んだ行を、見出しを先頭にしたタブ区切りの文字にする。選んでいなければ `None`。
    pub fn selected_text(&self, table: &dyn Table) -> Option<(String, usize)> {
        if self.selection.is_empty() {
            return None;
        }
        let rows = self.selection.rows().filter(|&r| r < table.rows());
        Some((
            vw_core::table::tab_separated(table, rows),
            self.selection.count(),
        ))
    }

    fn layout(&self, body: Rect, table: &dyn Table) -> Layout {
        let digits = table.rows().max(1).to_string().len() as f32;
        let number = digits * 7.0 + 2.0 * PAD;
        let total_width = number + self.widths.iter().sum::<f32>();
        let need_h = total_width > body.width();
        let rows_h = body.height() - HEADER - if need_h { BAR } else { 0.0 };
        let need_v = table.rows() as f32 * ROW > rows_h;
        let right = body.right() - if need_v { BAR } else { 0.0 };
        let bottom = body.bottom() - if need_h { BAR } else { 0.0 };
        let rows = Rect::from_min_max(pos2(body.left(), body.top() + HEADER), pos2(right, bottom));
        Layout {
            body,
            number,
            rows,
            visible: rows.height() / ROW,
            total_width,
            vertical: need_v.then(|| {
                Rect::from_min_max(pos2(right, body.top() + HEADER), pos2(body.right(), bottom))
            }),
            horizontal: need_h
                .then(|| Rect::from_min_max(pos2(body.left(), bottom), pos2(right, body.bottom()))),
        }
    }

    /// 初めの列の幅。見出しと単位と、頭の行のセルが入る幅。
    fn start_widths(ui: &Ui, table: &dyn Table) -> Vec<f32> {
        let width = |s: &str| {
            ui.painter()
                .layout_no_wrap(s.to_owned(), font(), Color32::WHITE)
                .size()
                .x
        };
        table
            .columns()
            .iter()
            .enumerate()
            .map(|(c, column)| {
                let head = width(&column.name).max(width(&sub_heading(column)));
                // 頭の行だけでは、後ろの行の桁の多い番号などが入らないので、尻の行も見る。
                let rows = table.rows();
                let tail = rows.saturating_sub(SAMPLE_ROWS).max(SAMPLE_ROWS.min(rows));
                let cells = (0..rows.min(SAMPLE_ROWS))
                    .chain(tail..rows)
                    .map(|r| width(&table.cell(r, c)))
                    .fold(0.0, f32::max);
                (head.max(cells) + 2.0 * PAD).clamp(MIN_WIDTH, MAX_START_WIDTH)
            })
            .collect()
    }

    /// 枠 `body` に表を描き、スロットの `response` のマウスとホイールを受け取る。`wheel` は
    /// `(縦, 横)` の回転量（論理ピクセル、[`super::geometry::split_wheel`]）。
    pub fn ui(
        &mut self,
        ui: &Ui,
        response: &Response,
        body: Rect,
        table: &dyn Table,
        wheel: (f32, f32),
    ) {
        if self.widths.len() != table.columns().len() {
            self.widths = Self::start_widths(ui, table);
        }
        let layout = self.layout(body, table);
        let rows = table.rows();
        let max_top = (rows as f32 - layout.visible.floor()).max(0.0);
        let max_left = (layout.total_width - layout.rows.width()).max(0.0);
        self.top = (self.top - wheel.0 / ROW).clamp(0.0, max_top);
        self.left = (self.left - wheel.1).clamp(0.0, max_left);

        let modifiers = ui.input(|i| i.modifiers);
        let pointer = response.interact_pointer_pos();
        let hover = response.hover_pos();
        let border = hover.and_then(|p| self.border_at(&layout, p));
        if border.is_some() || matches!(self.grab, Some(Grab::Resize(..))) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
        }
        // ドラッグが始まったと分かるのは少し動いた後なので、つかんだ所は押した位置で決める。
        let pressed_at = ui.input(|i| i.pointer.press_origin()).or(pointer);
        if response.drag_started()
            && let Some(p) = pressed_at
        {
            self.grab = if let Some(c) = self.border_at(&layout, p) {
                Some(Grab::Resize(c, self.widths[c], p.x))
            } else if layout.vertical.is_some_and(|r| r.contains(p)) {
                Some(Grab::Rows(self.top, p.y))
            } else if layout.horizontal.is_some_and(|r| r.contains(p)) {
                Some(Grab::Columns(self.left, p.x))
            } else if let Some(row) = self.row_at(&layout, p, rows) {
                self.press(row, modifiers);
                Some(Grab::Select)
            } else {
                None
            };
        }
        if response.dragged()
            && let Some(p) = pointer
        {
            match self.grab {
                Some(Grab::Resize(c, width, x)) => {
                    self.widths[c] = (width + p.x - x).max(MIN_WIDTH);
                }
                Some(Grab::Rows(top, y)) => {
                    let track = layout.vertical.map_or(1.0, |r| r.height());
                    self.top = (top + (p.y - y) / track * rows as f32).clamp(0.0, max_top);
                }
                Some(Grab::Columns(left, x)) => {
                    let track = layout.horizontal.map_or(1.0, |r| r.width());
                    self.left =
                        (left + (p.x - x) / track * layout.total_width).clamp(0.0, max_left);
                }
                Some(Grab::Select) => {
                    // 枠の上下の外まで引いたら、送りながら選ぶ。
                    if p.y < layout.rows.top() {
                        self.top = (self.top - 1.0).max(0.0);
                    } else if p.y > layout.rows.bottom() {
                        self.top = (self.top + 1.0).min(max_top);
                    }
                    let y = p.y.clamp(layout.rows.top(), layout.rows.bottom() - 1.0);
                    if let (Some(anchor), Some(row)) = (
                        self.anchor,
                        self.row_at(&layout, pos2(layout.rows.left() + 1.0, y), rows),
                    ) {
                        self.selection = Selection::range(anchor, row);
                    }
                    ui.ctx().request_repaint();
                }
                None => {}
            }
        }
        if response.drag_stopped() {
            self.grab = None;
        }
        if response.clicked()
            && let Some(p) = pointer
            && self.border_at(&layout, p).is_none()
        {
            match self.row_at(&layout, p, rows) {
                Some(row) => self.press(row, modifiers),
                None if p.y >= layout.rows.top() && layout.rows.contains(p) => {
                    self.selection = Selection::default();
                }
                None => {}
            }
        }
        self.paint(ui, &layout, table);
    }

    fn press(&mut self, row: usize, modifiers: egui::Modifiers) {
        match (modifiers.shift, modifiers.command, self.anchor) {
            (true, _, Some(anchor)) => self.selection = Selection::range(anchor, row),
            (_, true, _) => {
                self.selection.toggle(row);
                self.anchor = Some(row);
            }
            _ => {
                self.selection = Selection::only(row);
                self.anchor = Some(row);
            }
        }
    }

    fn row_at(&self, layout: &Layout, p: Pos2, rows: usize) -> Option<usize> {
        if !layout.rows.contains(p) {
            return None;
        }
        let row = (self.top.floor() + ((p.y - layout.rows.top()) / ROW).floor()) as usize;
        (row < rows).then_some(row)
    }

    /// 見出しの、列の右の境の上なら、その列。
    fn border_at(&self, layout: &Layout, p: Pos2) -> Option<usize> {
        let header = Rect::from_min_max(
            layout.body.min,
            pos2(layout.rows.right(), layout.body.top() + HEADER),
        );
        if !header.contains(p) {
            return None;
        }
        let mut x = layout.body.left() + layout.number - self.left;
        for (c, w) in self.widths.iter().enumerate() {
            x += w;
            if (p.x - x).abs() <= GRAB && x > layout.body.left() + layout.number {
                return Some(c);
            }
        }
        None
    }

    fn paint(&self, ui: &Ui, layout: &Layout, table: &dyn Table) {
        let visuals = ui.visuals();
        let painter = ui.painter_at(layout.body);
        let text = visuals.text_color();
        let weak = visuals.weak_text_color();
        let line = Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
        let body = layout.body;
        let first = self.top.floor() as usize;
        let shown = (layout.visible.ceil() as usize + 1).min(table.rows().saturating_sub(first));
        let left = body.left() + layout.number - self.left;
        // 行の帯と選択。
        for i in 0..shown {
            let row = first + i;
            let y = layout.rows.top() + i as f32 * ROW;
            let band = Rect::from_min_size(pos2(body.left(), y), vec2(layout.rows.width(), ROW));
            if self.selection.contains(row) {
                painter.rect_filled(band, 0.0, visuals.selection.bg_fill);
            } else if row % 2 == 1 {
                painter.rect_filled(band, 0.0, visuals.faint_bg_color);
            }
        }
        // セル。列ごとに切り抜いて、はみ出した文字を隣に重ねない。
        let mut x = left;
        for (c, &w) in self.widths.iter().enumerate() {
            let column = Rect::from_min_max(
                pos2(x.max(body.left() + layout.number), body.top()),
                pos2((x + w).min(layout.rows.right()), layout.rows.bottom()),
            );
            if column.width() > 0.0 {
                let cells = painter.with_clip_rect(column.intersect(layout.rows));
                for i in 0..shown {
                    let y = layout.rows.top() + i as f32 * ROW + ROW / 2.0;
                    let value = table.cell(first + i, c);
                    let numeric = looks_numeric(&value);
                    let galley = cell_galley(&cells, value, w - 2.0 * PAD, text);
                    // 入りきらない数は、右に寄せて頭を切ると別の数に見えるので、左に寄せて尻を「…」にする。
                    let fits = !galley.elided;
                    let left_x = if numeric && fits {
                        x + w - PAD - galley.size().x
                    } else {
                        x + PAD
                    };
                    cells.galley(pos2(left_x, y - galley.size().y / 2.0), galley, text);
                }
                let head = painter.with_clip_rect(column.intersect(Rect::from_min_max(
                    body.min,
                    pos2(layout.rows.right(), body.top() + HEADER),
                )));
                let spec = &table.columns()[c];
                head.text(
                    pos2(x + PAD, body.top() + 4.0),
                    egui::Align2::LEFT_TOP,
                    &spec.name,
                    font(),
                    text,
                );
                head.text(
                    pos2(x + PAD, body.top() + HEADER - 4.0),
                    egui::Align2::LEFT_BOTTOM,
                    sub_heading(spec),
                    FontId::proportional(11.0),
                    weak,
                );
            }
            x += w;
            if x > body.left() + layout.number && x < layout.rows.right() {
                painter.vline(x, body.top()..=layout.rows.bottom(), line);
            }
        }
        // 行の番号（横に送っても動かさない）。
        let numbers = Rect::from_min_max(
            pos2(body.left(), body.top()),
            pos2(body.left() + layout.number, layout.rows.bottom()),
        );
        painter.rect_filled(
            numbers.with_min_y(layout.rows.top()),
            0.0,
            visuals.extreme_bg_color.gamma_multiply(0.6),
        );
        let number_painter = painter.with_clip_rect(numbers.intersect(layout.rows));
        for i in 0..shown {
            let y = layout.rows.top() + i as f32 * ROW + ROW / 2.0;
            number_painter.text(
                pos2(numbers.right() - PAD, y),
                egui::Align2::RIGHT_CENTER,
                (first + i + 1).to_string(),
                font(),
                weak,
            );
        }
        painter.vline(numbers.right(), body.top()..=layout.rows.bottom(), line);
        painter.hline(body.left()..=layout.rows.right(), body.top() + HEADER, line);
        // つまみ。
        let thumb = visuals.widgets.inactive.bg_fill;
        let track = visuals.extreme_bg_color;
        if let Some(r) = layout.vertical {
            painter.rect_filled(r, 0.0, track);
            let rows = table.rows().max(1) as f32;
            let h = (layout.visible / rows * r.height())
                .max(16.0)
                .min(r.height());
            let y = r.top() + self.top / rows * r.height();
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(r.left() + 2.0, y.min(r.bottom() - h)),
                    vec2(BAR - 4.0, h),
                ),
                3.0,
                thumb,
            );
        }
        if let Some(r) = layout.horizontal {
            painter.rect_filled(r, 0.0, track);
            let total = layout.total_width.max(1.0);
            let w = (layout.rows.width() / total * r.width())
                .max(16.0)
                .min(r.width());
            let x = r.left() + self.left / total * r.width();
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(x.min(r.right() - w), r.top() + 2.0),
                    vec2(w, BAR - 4.0),
                ),
                3.0,
                thumb,
            );
        }
    }
}

/// 幅 `width` に 1 行で入れた文字。入らなければ尻を「…」にする。
fn cell_galley(
    painter: &egui::Painter,
    value: String,
    width: f32,
    color: Color32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(value, font(), color);
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

/// 見出しの 2 行目（単位と型）。
fn sub_heading(column: &vw_core::table::Column) -> String {
    match (column.unit.is_empty(), column.format.is_empty()) {
        (true, true) => String::new(),
        (true, false) => column.format.clone(),
        (false, true) => column.unit.clone(),
        (false, false) => format!("{} · {}", column.unit, column.format),
    }
}

/// 数のセルは右に寄せる（桁が揃うように）。
fn looks_numeric(s: &str) -> bool {
    !s.is_empty() && (s == "NaN" || s.parse::<f64>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_clicks_add_and_remove_single_rows() {
        let mut s = Selection::range(2, 6);
        s.toggle(4);
        assert_eq!(s.rows().collect::<Vec<_>>(), [2, 3, 5, 6]);
        s.toggle(4);
        assert_eq!(s, Selection::range(2, 6));
        s.toggle(7);
        s.toggle(0);
        assert_eq!(s.rows().collect::<Vec<_>>(), [0, 2, 3, 4, 5, 6, 7]);
        assert_eq!(s.count(), 7);
    }

    #[test]
    fn a_range_of_millions_of_rows_stays_one_range() {
        let s = Selection::range(5_000_000, 0);
        assert_eq!(s.count(), 5_000_001);
        assert!(s.contains(4_999_999));
        assert!(!s.contains(5_000_001));
    }

    #[test]
    fn another_table_starts_from_the_top_with_nothing_selected() {
        let mut v = TableView::default();
        v.follow(Path::new("/a.fits"), true);
        v.selection = Selection::only(3);
        v.top = 10.0;
        v.graph = false;
        v.follow(Path::new("/a.fits"), true);
        assert_eq!((v.top, v.graph), (10.0, false), "same table keeps its view");
        v.follow(Path::new("/b.fits"), true);
        assert_eq!((v.top, v.graph, v.selection.is_empty()), (0.0, true, true));
    }
}
