//! 左端の道具の列のボタン。

use egui::{vec2, CornerRadius, Rect, Response, Sense, Ui, WidgetText};

/// 道具の列の 1 ボタン。選んでいる道具は、面をアクセントの淡い色で塗り、左端にアクセントの線を引き、
/// アイコンをアクセントの色にする。押せない道具は、押せるものよりはっきり薄くする。
///
/// egui の selectable のボタンは面の色を変えるだけで、並んだアイコンの中でどれを選んでいるかが
/// 一目で分かりにくいため、自分で描く。
pub fn button(
    ui: &mut Ui,
    selected: bool,
    enabled: bool,
    icon: impl Into<WidgetText>,
    side: f32,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), sense);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals();
    let accent = visuals.selection.stroke.color;
    let corner = CornerRadius::same(4);
    if selected {
        ui.painter()
            .rect_filled(rect, corner, visuals.selection.bg_fill);
        let bar = Rect::from_min_max(rect.left_top(), rect.left_bottom() + vec2(2.0, 0.0));
        ui.painter().rect_filled(bar, 1.0, accent);
    } else if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, corner, visuals.widgets.hovered.weak_bg_fill);
    }
    let color = if !enabled {
        visuals.weak_text_color().gamma_multiply(0.45)
    } else if selected {
        accent
    } else if response.hovered() {
        visuals.strong_text_color()
    } else {
        visuals.text_color()
    };
    let galley = icon.into().into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(pos, galley, color);
    response
}
