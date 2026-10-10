//! One image slot of the image field: header, zoom and pan.

use std::path::PathBuf;

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Sense, TextureHandle, Vec2};

use crate::loader::ImageInfo;

pub struct Shown {
    pub texture: TextureHandle,
    pub info: ImageInfo,
}

pub struct Slot {
    pub id: u64,
    pub path: Option<PathBuf>,
    /// Id of the newest load request; older results are dropped.
    pub request: u64,
    pub loading: bool,
    pub shown: Option<Shown>,
    pub error: Option<String>,
    /// Multiplier on the fit-to-slot scale.
    zoom: f32,
    /// Offset of the image center from the slot center, in points.
    pan: Vec2,
}

#[derive(Default)]
pub struct SlotResponse {
    pub clicked: bool,
    pub close: bool,
}

const HEADER: f32 = 20.0;

impl Slot {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            path: None,
            request: 0,
            loading: false,
            shown: None,
            error: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
        }
    }

    pub fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    fn base_scale(&self, body: Rect) -> f32 {
        let Some(s) = &self.shown else { return 1.0 };
        let (w, h) = (s.info.image.width as f32, s.info.image.height as f32);
        // Fit large images; show small ones at 100%.
        (body.width() / w).min(body.height() / h).min(1.0)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, rect: Rect, active: bool) -> SlotResponse {
        let mut out = SlotResponse::default();
        let visuals = ui.visuals().clone();
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
        let body = Rect::from_min_max(pos2(rect.min.x, rect.min.y + HEADER), rect.max);
        let painter = ui.painter_at(rect);

        painter.rect_filled(body, 0.0, Color32::from_gray(24));
        painter.rect_filled(header, 0.0, visuals.faint_bg_color);

        let response = ui.interact(
            body,
            ui.id().with(("slot", self.id)),
            Sense::click_and_drag(),
        );
        if response.clicked() || response.drag_started() {
            out.clicked = true;
        }
        if response.double_clicked() {
            self.reset_view();
        }

        let base = self.base_scale(body);
        if let Some(shown) = &self.shown {
            if response.dragged() {
                self.pan += response.drag_delta();
            }
            if response.hovered() {
                let (scroll, pinch, pointer) = ui.input(|i| {
                    (
                        i.smooth_scroll_delta.y,
                        i.zoom_delta(),
                        i.pointer.hover_pos(),
                    )
                });
                let factor = pinch * (scroll * 0.002).exp();
                if factor != 1.0 {
                    let old = self.zoom;
                    self.zoom = (self.zoom * factor).clamp(0.05, 200.0 / base.max(1e-6));
                    // Keep the point under the cursor fixed.
                    if let Some(p) = pointer {
                        let c = body.center() + self.pan;
                        self.pan += (p - c) * (1.0 - self.zoom / old);
                    }
                }
            }
            let scale = base * self.zoom;
            let size = vec2(
                shown.info.image.width as f32,
                shown.info.image.height as f32,
            ) * scale;
            let img_rect = Rect::from_center_size(body.center() + self.pan, size);
            ui.painter_at(body).image(
                shown.texture.id(),
                img_rect,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            let msg = if self.loading {
                "Loading..."
            } else if self.error.is_some() {
                "Failed to load (see log)"
            } else {
                "Select an image in the folder tree (Ctrl+click adds a slot)"
            };
            painter.text(
                body.center(),
                egui::Align2::CENTER_CENTER,
                msg,
                egui::FontId::proportional(14.0),
                Color32::GRAY,
            );
        }

        // Header: file name, zoom, close button.
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(empty)".into());
        let label = if self.shown.is_some() {
            format!("{name}   {:.0}%", base * self.zoom * 100.0)
        } else {
            name
        };
        let text_color = if active {
            visuals.strong_text_color()
        } else {
            visuals.weak_text_color()
        };
        painter.text(
            header.left_center() + vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.0),
            text_color,
        );
        if ui
            .interact(header, ui.id().with(("header", self.id)), Sense::click())
            .clicked()
        {
            out.clicked = true;
        }
        let close_rect = Rect::from_center_size(
            header.right_center() - vec2(HEADER / 2.0, 0.0),
            Vec2::splat(HEADER - 4.0),
        );
        let close = ui.interact(close_rect, ui.id().with(("close", self.id)), Sense::click());
        let close_color = if close.hovered() {
            visuals.strong_text_color()
        } else {
            visuals.weak_text_color()
        };
        painter.text(
            close_rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(14.0),
            close_color,
        );
        out.close = close.on_hover_text("Close slot").clicked();

        let border = if active {
            visuals.selection.stroke.color
        } else {
            visuals.widgets.noninteractive.bg_stroke.color
        };
        painter.rect_stroke(
            rect.shrink(0.5),
            0.0,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        out
    }
}
