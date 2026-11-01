//! 画像や星図の上に重ねる枠と名札の描き方。顔の枠と、星図の写った範囲で同じ見た目にする。

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// 枠の下に敷く黒い縁。明るい画像でも暗い画像でも枠が見えるように。
fn backing() -> Stroke {
    Stroke::new(3.0, Color32::from_black_alpha(140))
}

/// `points` を結んだ線（`closed` なら最後と最初もつなぐ）を、黒い縁の上に `color` で描く。
/// `dashed` なら点線にする（確かでない範囲）。
pub fn outline(
    painter: &Painter,
    points: &[Pos2],
    closed: bool,
    color: Color32,
    width: f32,
    dashed: bool,
) {
    if points.len() < 2 {
        return;
    }
    let mut line = points.to_vec();
    if closed {
        line.push(points[0]);
    }
    let stroke = Stroke::new(width, color);
    if dashed {
        painter.extend(Shape::dashed_line(&line, backing(), 6.0, 4.0));
        painter.extend(Shape::dashed_line(&line, stroke, 6.0, 4.0));
    } else {
        painter.add(Shape::line(line.clone(), backing()));
        painter.add(Shape::line(line, stroke));
    }
}

pub fn rect_outline(painter: &Painter, rect: Rect, color: Color32, width: f32, dashed: bool) {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    outline(painter, &corners, true, color, width, dashed);
}

/// 中心に打つ十字（範囲の分からない位置）。
pub fn cross(painter: &Painter, at: Pos2, color: Color32, arm: f32) {
    for stroke in [
        Stroke::new(4.0, Color32::from_black_alpha(140)),
        Stroke::new(2.0, color),
    ] {
        painter.line_segment([at - vec2(arm, 0.0), at + vec2(arm, 0.0)], stroke);
        painter.line_segment([at - vec2(0.0, arm), at + vec2(0.0, arm)], stroke);
    }
}

/// 枠の外接矩形 `around` の下（入らなければ内側の下端）に、黒地の名札を出す。
/// `bounds` からはみ出さないように左右を寄せる。
pub fn name_tag(painter: &Painter, around: Rect, bounds: Rect, text: &str, color: Color32) {
    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.0), color);
    let pad = vec2(4.0, 1.0);
    let size = galley.size() + 2.0 * pad;
    let y = if around.bottom() + size.y + 2.0 <= bounds.bottom() {
        around.bottom() + 2.0
    } else {
        around.bottom() - size.y - 2.0
    };
    let x = around
        .left()
        .max(bounds.left())
        .min(bounds.right() - size.x);
    let pill = Rect::from_min_size(pos2(x, y), size);
    painter.rect_filled(pill, 2.0, Color32::from_black_alpha(160));
    painter.galley(pill.min + pad, galley, color);
}
