//! ズームマップの別ウインドウ。画像全体の縮小図に、アクティブなスロットで今見えている範囲を
//! 枠で出し、枠やつまみでスロットの表示位置と倍率を動かす。
//!
//! ピクセルパイプラインで値を変えているときは、変更前と変更後の縮小図を並べ、同じ枠を両方に出す。

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Sense, Stroke, TextureHandle};

use crate::i18n::tr;
use crate::settings::Language;
use crate::sub_window::SubWindow;
use crate::viewer::ViewWindow;

/// ウインドウに渡す、アクティブなスロットの表示。
pub struct Target<'a> {
    pub title: String,
    /// 変更前（ファイルの値での表示）。
    pub before: &'a TextureHandle,
    /// 変更後。ピクセルパイプラインで値を変えていなければ `None`。
    pub after: Option<&'a TextureHandle>,
    /// 画像の縦横の画素数。
    pub size: [u32; 2],
    pub view: ViewWindow,
}

/// スロットへの操作。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// 画像のこの点（左上 (0, 0)、右下 (1, 1) の割合）を枠の中心にする。
    CenterAt(Pos2),
    /// 実寸に対する表示倍率。
    SetScale(f32),
    Fit,
    ActualSize,
}

#[derive(Default)]
pub struct ZoomMap {
    pub open: bool,
    pub window: SubWindow,
    /// 枠の中をつかんでドラッグしている。つかんだ点と枠の中心のずれ（割合）。
    grab: Option<egui::Vec2>,
}

impl ZoomMap {
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        lang: Language,
        target: Option<Target<'_>>,
    ) -> Vec<Command> {
        if !self.open {
            return Vec::new();
        }
        let title = match &target {
            Some(t) => format!("{} - {}", tr(lang, "Zoom map"), t.title),
            None => tr(lang, "Zoom map").to_owned(),
        };
        let mut out = Vec::new();
        let mut close = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("zoom_map"),
            self.window.builder(title, [640.0, 380.0]),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                self.window.track(ctx);
                self.window.pin_overlay(ctx, lang);
                egui::CentralPanel::default().show(ctx, |ui| match &target {
                    None => {
                        ui.centered_and_justified(|ui| {
                            ui.weak(tr(lang, "Select an image in a slot."))
                        });
                    }
                    Some(t) => out = self.contents(ui, lang, t),
                });
            },
        );
        if close {
            self.open = false;
        }
        out
    }

    fn contents(&mut self, ui: &mut egui::Ui, lang: Language, t: &Target<'_>) -> Vec<Command> {
        let mut out = Vec::new();
        let bar_h = 30.0;
        let area = ui
            .available_rect_before_wrap()
            .with_max_y(ui.max_rect().max.y - bar_h);
        let maps: Vec<(&TextureHandle, &str)> = match t.after {
            Some(after) => vec![(t.before, "Before"), (after, "After")],
            None => vec![(t.before, "No change in the pixel pipeline")],
        };
        let cells = split(area, maps.len(), area.width() >= area.height());
        for ((texture, caption), cell) in maps.into_iter().zip(cells) {
            let caption_h = 16.0;
            let map_area = cell.with_max_y(cell.max.y - caption_h).shrink(4.0);
            let aspect = vec2(t.size[0].max(1) as f32, t.size[1].max(1) as f32);
            let fit = (map_area.width() / aspect.x).min(map_area.height() / aspect.y);
            let map = Rect::from_center_size(map_area.center(), aspect * fit);
            let id = ui.id().with(("zoom_map", caption));
            let response = ui.interact(map, id, Sense::click_and_drag());
            let painter = ui.painter_at(cell);
            painter.image(
                texture.id(),
                map,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            painter.rect_stroke(
                map,
                0.0,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Outside,
            );
            let frame = Rect::from_min_max(
                map.min + t.view.visible.min.to_vec2() * map.size(),
                map.min + t.view.visible.max.to_vec2() * map.size(),
            );
            draw_frame(&painter, frame);
            painter.text(
                pos2(cell.center().x, cell.max.y - 2.0),
                egui::Align2::CENTER_BOTTOM,
                tr(lang, caption),
                egui::FontId::proportional(12.0),
                ui.visuals().weak_text_color(),
            );
            let to_fraction = |p: Pos2| ((p - map.min) / map.size()).to_pos2();
            if let Some(p) = response.interact_pointer_pos() {
                if response.drag_started() || response.clicked() {
                    self.grab = frame
                        .contains(p)
                        .then(|| to_fraction(p) - t.view.visible.center());
                }
                let at = to_fraction(p) - self.grab.unwrap_or_default();
                if response.dragged() || response.clicked() {
                    out.push(Command::CenterAt(pos2(
                        at.x.clamp(0.0, 1.0),
                        at.y.clamp(0.0, 1.0),
                    )));
                }
            }
            if response.drag_stopped() {
                self.grab = None;
            }
        }
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_max(
                pos2(ui.max_rect().min.x, ui.max_rect().max.y - bar_h),
                ui.max_rect().max,
            )),
            |ui| {
                ui.horizontal_centered(|ui| out.extend(zoom_bar(ui, lang, &t.view)));
            },
        );
        out
    }
}

/// 倍率のつまみ（対数目盛り）・％の欄・フィット・1:1。
fn zoom_bar(ui: &mut egui::Ui, lang: Language, view: &ViewWindow) -> Vec<Command> {
    let mut out = Vec::new();
    let (lo, hi) = (view.min_scale * 100.0, view.max_scale * 100.0);
    if ui.small_button("−").clicked() {
        out.push(Command::SetScale(
            view.scale / crate::viewer::KEY_ZOOM_FACTOR,
        ));
    }
    let mut percent = view.scale * 100.0;
    let slider = egui::Slider::new(&mut percent, lo..=hi)
        .logarithmic(true)
        .show_value(false);
    if ui.add_sized([220.0, 18.0], slider).changed() {
        out.push(Command::SetScale(percent / 100.0));
    }
    if ui.small_button("+").clicked() {
        out.push(Command::SetScale(
            view.scale * crate::viewer::KEY_ZOOM_FACTOR,
        ));
    }
    let mut typed = view.scale * 100.0;
    let field = egui::DragValue::new(&mut typed)
        .range(lo..=hi)
        .suffix(" %")
        .max_decimals(0)
        .speed(1.0);
    if ui.add(field).changed() {
        out.push(Command::SetScale(typed / 100.0));
    }
    if ui.button(tr(lang, "Fit")).clicked() {
        out.push(Command::Fit);
    }
    if ui.button("1:1").clicked() {
        out.push(Command::ActualSize);
    }
    out
}

/// 明るい画像でも暗い画像でも見えるよう、黒い縁を付けて描く。
fn draw_frame(painter: &egui::Painter, frame: Rect) {
    for (width, color) in [
        (3.0, Color32::from_black_alpha(180)),
        (1.0, Color32::from_rgb(255, 210, 0)),
    ] {
        painter.rect_stroke(
            frame,
            0.0,
            Stroke::new(width, color),
            egui::StrokeKind::Middle,
        );
    }
}

/// `area` を `n` 個に等分する。`side_by_side` なら左右、そうでなければ上下。
fn split(area: Rect, n: usize, side_by_side: bool) -> Vec<Rect> {
    let n = n.max(1);
    (0..n)
        .map(|i| {
            let (a, b) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
            if side_by_side {
                Rect::from_x_y_ranges(
                    area.lerp_inside(vec2(a, 0.0)).x..=area.lerp_inside(vec2(b, 0.0)).x,
                    area.y_range(),
                )
            } else {
                Rect::from_x_y_ranges(
                    area.x_range(),
                    area.lerp_inside(vec2(0.0, a)).y..=area.lerp_inside(vec2(0.0, b)).y,
                )
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_area_is_split_left_and_right() {
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 100.0));
        let r = split(area, 2, true);
        assert_eq!(r[0], Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)));
        assert_eq!(
            r[1],
            Rect::from_min_max(pos2(100.0, 0.0), pos2(200.0, 100.0))
        );
    }

    #[test]
    fn tall_area_is_split_top_and_bottom() {
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 200.0));
        let r = split(area, 2, false);
        assert_eq!(
            r[1],
            Rect::from_min_max(pos2(0.0, 100.0), pos2(100.0, 200.0))
        );
    }
}
