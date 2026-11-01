//! ドラッグ中に出す、合体位置のガイド（Visual Studio のガイドに倣ったもの）。
//!
//! カーソルの下のペインの中央に上下左右の 4 つ、ウインドウの四辺の中ほどに 4 つ。
//! 中央（重ねてタブにする）を出さないのは、vw はタブを使わず、出しているペインを
//! いつも全部見せる決まりにしたため。

use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::tree::{Placed, Side, Target, preview};

/// ボタンの一辺（ポイント）。
const BUTTON: f32 = 30.0;
/// 中央の 4 つのボタンの、ペインの中心からの距離。
const SPREAD: f32 = 34.0;
/// 四辺のボタンの、ウインドウの縁からの距離。
const EDGE_INSET: f32 = 8.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub rect: Rect,
    pub side: Side,
    pub target: Target,
    /// 離したときにペインが占める範囲。
    pub preview: Rect,
}

/// `area`（ウインドウのペインを並べる範囲）で、カーソルが `pointer` にあるときに出すボタン。
pub fn buttons(area: Rect, panes: &[Placed], pointer: Pos2) -> Vec<Button> {
    let mut out = Vec::new();
    if let Some(under) = panes.iter().find(|p| p.rect.contains(pointer)) {
        let c = under.rect.center();
        for side in Side::ALL {
            let target = Target::beside(&under.pane, side);
            out.push(Button {
                rect: Rect::from_center_size(c + offset(side) * SPREAD, vec2(BUTTON, BUTTON)),
                side,
                preview: preview(under.rect, side, target.share),
                target,
            });
        }
    }
    let half = BUTTON / 2.0 + EDGE_INSET;
    for side in Side::ALL {
        let center = match side {
            Side::Left => pos2(area.left() + half, area.center().y),
            Side::Right => pos2(area.right() - half, area.center().y),
            Side::Top => pos2(area.center().x, area.top() + half),
            Side::Bottom => pos2(area.center().x, area.bottom() - half),
        };
        let target = Target::edge(side);
        out.push(Button {
            rect: Rect::from_center_size(center, vec2(BUTTON, BUTTON)),
            side,
            preview: preview(area, side, target.share),
            target,
        });
    }
    // 小さいペインでは中央の 4 つが四辺のボタンに重なる。重なったら中央のほうを残す
    // （先に積んだほうを当たりにする）。
    out
}

/// `pointer` が乗っているボタン。
pub fn hit(buttons: &[Button], pointer: Pos2) -> Option<&Button> {
    buttons.iter().find(|b| b.rect.contains(pointer))
}

fn offset(side: Side) -> egui::Vec2 {
    match side {
        Side::Left => vec2(-1.0, 0.0),
        Side::Right => vec2(1.0, 0.0),
        Side::Top => vec2(0.0, -1.0),
        Side::Bottom => vec2(0.0, 1.0),
    }
}

/// ガイドの色。vw の灰色とアクセントの青に合わせて、呼ぶ側の見た目から作る。
pub struct Look {
    pub fill: Color32,
    pub line: Color32,
    pub glyph: Color32,
    pub accent: Color32,
}

impl Look {
    pub fn from_visuals(v: &egui::Visuals) -> Self {
        Self {
            fill: v.panel_fill.gamma_multiply(0.96),
            line: v.widgets.inactive.bg_stroke.color.gamma_multiply(1.4),
            glyph: v.text_color(),
            accent: v.selection.stroke.color,
        }
    }
}

/// ボタンと、乗っているボタンの落ちる場所を描く。角は丸めない（vw の見出しや枠と同じ、四角くシャープな見た目）。
pub fn paint(painter: &Painter, buttons: &[Button], hot: Option<&Button>, look: &Look) {
    if let Some(hot) = hot {
        painter.rect(
            hot.preview,
            0.0,
            look.accent.gamma_multiply(0.16),
            Stroke::new(1.0, look.accent),
            StrokeKind::Inside,
        );
    }
    for b in buttons {
        let is_hot = hot.is_some_and(|h| h.rect == b.rect);
        let (fill, glyph, line) = if is_hot {
            (look.accent, Color32::WHITE, look.accent)
        } else {
            (look.fill, look.glyph, look.line)
        };
        painter.rect_filled(
            b.rect.translate(vec2(0.0, 2.0)),
            0.0,
            Color32::from_black_alpha(70),
        );
        painter.rect(
            b.rect,
            0.0,
            fill,
            Stroke::new(1.0, line),
            StrokeKind::Inside,
        );
        // 「どこに入るか」の図: 外枠と、入る側を塗った半分。
        let frame = Rect::from_center_size(b.rect.center(), vec2(18.0, 18.0));
        painter.rect_stroke(frame, 0.0, Stroke::new(1.2, glyph), StrokeKind::Inside);
        painter.rect_filled(preview(frame, b.side, 0.45), 0.0, glyph);
    }
}

/// ドラッグしているペインの名前の札。引き出したウインドウがカーソルに付いてこないときに、
/// カーソルの下のウインドウに描く。
pub fn paint_chip(painter: &Painter, at: Pos2, title: &str, visuals: &egui::Visuals, look: &Look) {
    let galley = painter.layout_no_wrap(
        title.to_owned(),
        egui::FontId::proportional(12.0),
        visuals.strong_text_color(),
    );
    let rect = Rect::from_min_size(at, galley.size() + vec2(16.0, 6.0));
    painter.rect(
        rect,
        0.0,
        visuals.panel_fill,
        Stroke::new(1.0, look.accent),
        StrokeKind::Inside,
    );
    painter.galley(
        rect.min + vec2(8.0, 3.0),
        galley,
        visuals.strong_text_color(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(pane: &str, rect: Rect) -> Placed {
        Placed {
            pane: pane.to_owned(),
            rect,
        }
    }

    fn area() -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0))
    }

    fn panes() -> Vec<Placed> {
        vec![
            placed(
                "tree",
                Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 600.0)),
            ),
            placed(
                "images",
                Rect::from_min_max(pos2(201.0, 0.0), pos2(1000.0, 600.0)),
            ),
        ]
    }

    #[test]
    fn shows_four_guides_on_the_pane_under_the_cursor_and_four_on_the_window_edges() {
        let b = buttons(area(), &panes(), pos2(600.0, 300.0));
        assert_eq!(b.len(), 8);
        let beside: Vec<_> = b.iter().filter(|b| b.target.anchor == ["images"]).collect();
        assert_eq!(beside.len(), 4);
        assert!(
            beside
                .iter()
                .all(|b| panes()[1].rect.contains(b.rect.center()))
        );
    }

    #[test]
    fn offers_no_center_guide() {
        let b = buttons(area(), &panes(), pos2(600.0, 300.0));
        let center = panes()[1].rect.center();
        assert!(hit(&b, center).is_none());
    }

    #[test]
    fn the_left_guide_of_a_pane_drops_into_its_left_half() {
        let b = buttons(area(), &panes(), pos2(600.0, 300.0));
        let c = panes()[1].rect.center();
        let hot = hit(&b, c + vec2(-SPREAD, 0.0)).unwrap();
        assert_eq!(hot.target, Target::beside("images", Side::Left));
        assert_eq!(hot.preview.left(), 201.0);
        assert!((hot.preview.right() - 600.5).abs() < 1.0);
    }

    #[test]
    fn an_edge_guide_drops_along_the_whole_window_side() {
        let b = buttons(area(), &panes(), pos2(600.0, 300.0));
        let hot = hit(&b, pos2(500.0, 600.0 - EDGE_INSET - 2.0)).unwrap();
        assert_eq!(hot.target, Target::edge(Side::Bottom));
        assert_eq!(hot.preview.width(), 1000.0);
        assert_eq!(hot.preview.bottom(), 600.0);
    }

    #[test]
    fn an_empty_window_still_offers_its_edge_guides() {
        let b = buttons(area(), &[], pos2(500.0, 300.0));
        assert_eq!(b.len(), 4);
    }
}
