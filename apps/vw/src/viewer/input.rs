//! スロットの本体へのマウス操作の解釈。egui の `Response` から読んだ事実（[`Pointer`]）と
//! ツール（[`Tool`]）から、何をするかを決める。egui の `Ui` には触らないので、
//! ツール × 操作の決まりはテストで確かめる。
//!
//! ツールごとの振る舞いは trait ではなく `match` で書く。ツールはアプリで決まっていて
//! 外から足さないので、動的ディスパッチの型は要らず、「ツールごとに 1 か所」で足りる。

use eframe::egui::{CursorIcon, Pos2, Response, Vec2};

use super::geometry::KEY_ZOOM_FACTOR;
use crate::roi;

/// 左端のツール列で選んでいるツールのうち、スロットの本体の操作に関わるもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// 矢印。クリックで選び、左ドラッグで画像を運ぶ（見出しのドラッグと同じ）。
    #[default]
    Select,
    /// 手のひら。左ドラッグで表示位置を動かす。
    Hand,
    /// 左ドラッグの上下で拡大縮小、クリックで 1 段拡大、Alt+クリックで 1 段縮小。
    Zoom,
    /// 画素値の確認。クリックで十字を置き、左ドラッグで表示位置を動かす。
    Probe,
    /// 左ドラッグで矩形の ROI を描く（Shift で背景）。
    Roi,
}

/// 左ボタンのドラッグでやること。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drag {
    /// 表示位置を動かす。
    Pan,
    /// 上下で拡大縮小。
    Zoom,
    DrawRoi,
    /// 画像を運ぶ（ほかのスロットやフォルダへ）。
    CarryFile,
}

/// 左クリックでやること。拡大は [`Tool::zoom_on_click`] で別に決める（描く前に倍率を変えるため）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Nothing,
    /// 選んでいた十字と ROI を外す。
    ClearSelection,
    SelectMark(usize),
    /// 画素 `(x, y)` に十字を置く。
    AddMark(u32, u32),
    /// `None` なら ROI の外をクリックしたので選択を外す。
    SelectRoi(Option<usize>),
}

/// クリックした位置にあったもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Hits {
    /// 表示中の画像の十字の番号。
    pub mark: Option<usize>,
    /// 表示中の画像の ROI の番号。
    pub roi: Option<usize>,
    /// 画像の画素。画像の外なら `None`。
    pub pixel: Option<(u32, u32)>,
}

impl Tool {
    pub fn drag(self) -> Drag {
        match self {
            Tool::Select => Drag::CarryFile,
            Tool::Hand | Tool::Probe => Drag::Pan,
            Tool::Zoom => Drag::Zoom,
            Tool::Roi => Drag::DrawRoi,
        }
    }

    /// ズームのツールでは続けてクリックして拡大するので、ダブルクリックを最大化にしない。
    pub fn maximizes_on_double_click(self) -> bool {
        self != Tool::Zoom
    }

    /// クリックで倍率に掛ける値。ズームのツールだけ。Alt で縮小。
    pub fn zoom_on_click(self, alt: bool) -> Option<f32> {
        match self {
            Tool::Zoom if alt => Some(1.0 / KEY_ZOOM_FACTOR),
            Tool::Zoom => Some(KEY_ZOOM_FACTOR),
            _ => None,
        }
    }

    pub fn click(self, hits: Hits) -> Click {
        match self {
            Tool::Roi => Click::SelectRoi(hits.roi),
            Tool::Probe => match (hits.mark, hits.pixel) {
                (Some(i), _) => Click::SelectMark(i),
                (None, Some((x, y))) => Click::AddMark(x, y),
                (None, None) => Click::Nothing,
            },
            // 拡大のクリックでも、矢印と同じく選んでいた十字や ROI は外す。
            Tool::Select | Tool::Hand | Tool::Zoom => Click::ClearSelection,
        }
    }

    /// 画像の上に乗せたときのマウスカーソル。`None` は既定のまま。
    pub fn cursor(self, alt: bool, over_pixel: bool, dragging: bool) -> Option<CursorIcon> {
        match self {
            Tool::Select => None,
            Tool::Hand => Some(grab(dragging)),
            Tool::Zoom if alt => Some(CursorIcon::ZoomOut),
            Tool::Zoom => Some(CursorIcon::ZoomIn),
            Tool::Probe if over_pixel && !dragging => Some(CursorIcon::Crosshair),
            Tool::Probe => Some(grab(dragging)),
            Tool::Roi => Some(CursorIcon::Crosshair),
        }
    }
}

fn grab(dragging: bool) -> CursorIcon {
    if dragging {
        CursorIcon::Grabbing
    } else {
        CursorIcon::Grab
    }
}

/// ROI のドラッグで描く種類。Shift で背景。
pub fn roi_kind(shift: bool) -> roi::Kind {
    if shift {
        roi::Kind::Background
    } else {
        roi::Kind::Target
    }
}

/// 1 フレームの間にスロットの本体で起きたマウスの事実。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pointer {
    /// 押している（押した）位置。
    pub pos: Option<Pos2>,
    /// 乗せている位置。
    pub hover: Option<Pos2>,
    pub hovered: bool,
    pub clicked: bool,
    pub double_clicked: bool,
    pub secondary_clicked: bool,
    /// 左ボタンのドラッグ。
    pub drag_started: bool,
    pub dragged: bool,
    pub drag_stopped: bool,
    /// 右ボタンのドラッグ（どのツールでも上下で拡大縮小）。
    pub dragged_secondary: bool,
    /// このフレームのドラッグの移動量。
    pub drag_delta: Vec2,
    /// ドラッグを始めた位置。
    pub press_origin: Option<Pos2>,
    pub alt: bool,
    pub shift: bool,
}

impl Pointer {
    // `Response` の各メソッドは中で egui の状態をロックするので、`ctx.input` の閉包の中では
    // 呼べない（デッドロックする）。修飾キーと押した位置だけを別に読む。
    pub fn read(response: &Response) -> Self {
        use eframe::egui::PointerButton::{Primary, Secondary};
        let (press_origin, alt, shift) = response
            .ctx
            .input(|i| (i.pointer.press_origin(), i.modifiers.alt, i.modifiers.shift));
        Self {
            pos: response.interact_pointer_pos(),
            hover: response.hover_pos(),
            hovered: response.hovered(),
            clicked: response.clicked(),
            double_clicked: response.double_clicked(),
            secondary_clicked: response.secondary_clicked(),
            drag_started: response.drag_started_by(Primary),
            dragged: response.dragged_by(Primary),
            drag_stopped: response.drag_stopped_by(Primary),
            dragged_secondary: response.dragged_by(Secondary),
            drag_delta: response.drag_delta(),
            press_origin,
            alt,
            shift,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(mark: Option<usize>, roi: Option<usize>, pixel: Option<(u32, u32)>) -> Hits {
        Hits { mark, roi, pixel }
    }

    #[test]
    fn the_arrow_drags_the_file_and_the_hand_pans() {
        assert_eq!(Tool::Select.drag(), Drag::CarryFile);
        assert_eq!(Tool::Hand.drag(), Drag::Pan);
    }

    #[test]
    fn the_probe_pans_so_marks_can_be_placed_on_a_zoomed_image() {
        assert_eq!(Tool::Probe.drag(), Drag::Pan);
    }

    #[test]
    fn the_zoom_tool_zooms_by_dragging_and_the_roi_tool_draws() {
        assert_eq!(Tool::Zoom.drag(), Drag::Zoom);
        assert_eq!(Tool::Roi.drag(), Drag::DrawRoi);
    }

    #[test]
    fn only_the_zoom_tool_keeps_double_click_from_maximizing() {
        assert!(!Tool::Zoom.maximizes_on_double_click());
        for tool in [Tool::Select, Tool::Hand, Tool::Probe, Tool::Roi] {
            assert!(tool.maximizes_on_double_click(), "{tool:?}");
        }
    }

    #[test]
    fn a_zoom_click_enlarges_and_alt_click_shrinks_by_one_step() {
        assert_eq!(Tool::Zoom.zoom_on_click(false), Some(KEY_ZOOM_FACTOR));
        assert_eq!(Tool::Zoom.zoom_on_click(true), Some(1.0 / KEY_ZOOM_FACTOR));
    }

    #[test]
    fn other_tools_do_not_zoom_on_click() {
        for tool in [Tool::Select, Tool::Hand, Tool::Probe, Tool::Roi] {
            assert_eq!(tool.zoom_on_click(false), None, "{tool:?}");
            assert_eq!(tool.zoom_on_click(true), None, "{tool:?}");
        }
    }

    #[test]
    fn a_probe_click_on_a_pixel_adds_a_mark() {
        assert_eq!(
            Tool::Probe.click(hits(None, None, Some((3, 4)))),
            Click::AddMark(3, 4)
        );
    }

    #[test]
    fn a_probe_click_on_a_mark_selects_it_instead_of_adding_another() {
        assert_eq!(
            Tool::Probe.click(hits(Some(1), None, Some((3, 4)))),
            Click::SelectMark(1)
        );
    }

    #[test]
    fn a_probe_click_outside_the_image_does_nothing() {
        assert_eq!(Tool::Probe.click(hits(None, None, None)), Click::Nothing);
    }

    #[test]
    fn an_roi_click_selects_the_roi_under_the_pointer_or_clears_the_selection() {
        assert_eq!(
            Tool::Roi.click(hits(None, Some(2), Some((0, 0)))),
            Click::SelectRoi(Some(2))
        );
        assert_eq!(
            Tool::Roi.click(hits(Some(0), None, Some((0, 0)))),
            Click::SelectRoi(None)
        );
    }

    #[test]
    fn arrow_hand_and_zoom_clicks_clear_the_selection_even_on_a_mark() {
        for tool in [Tool::Select, Tool::Hand, Tool::Zoom] {
            assert_eq!(
                tool.click(hits(Some(0), Some(0), Some((1, 1)))),
                Click::ClearSelection,
                "{tool:?}"
            );
        }
    }

    #[test]
    fn shift_draws_a_background_roi() {
        assert_eq!(roi_kind(false), roi::Kind::Target);
        assert_eq!(roi_kind(true), roi::Kind::Background);
    }

    #[test]
    fn the_hand_shows_a_grab_cursor_and_grabbing_while_dragging() {
        assert_eq!(
            Tool::Hand.cursor(false, true, false),
            Some(CursorIcon::Grab)
        );
        assert_eq!(
            Tool::Hand.cursor(false, true, true),
            Some(CursorIcon::Grabbing)
        );
    }

    #[test]
    fn the_zoom_cursor_follows_alt() {
        assert_eq!(
            Tool::Zoom.cursor(false, true, false),
            Some(CursorIcon::ZoomIn)
        );
        assert_eq!(
            Tool::Zoom.cursor(true, true, false),
            Some(CursorIcon::ZoomOut)
        );
    }

    #[test]
    fn the_probe_shows_a_crosshair_over_a_pixel_and_a_hand_elsewhere() {
        assert_eq!(
            Tool::Probe.cursor(false, true, false),
            Some(CursorIcon::Crosshair)
        );
        assert_eq!(
            Tool::Probe.cursor(false, false, false),
            Some(CursorIcon::Grab)
        );
        assert_eq!(
            Tool::Probe.cursor(false, true, true),
            Some(CursorIcon::Grabbing)
        );
    }

    #[test]
    fn the_roi_tool_always_shows_a_crosshair_and_the_arrow_keeps_the_default() {
        assert_eq!(
            Tool::Roi.cursor(false, false, false),
            Some(CursorIcon::Crosshair)
        );
        assert_eq!(Tool::Select.cursor(false, true, false), None);
    }
}
