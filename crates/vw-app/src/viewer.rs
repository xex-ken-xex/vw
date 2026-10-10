//! 画像フィールドの 1 枠（スロット）。ヘッダ、ズーム、パン（移動）を受け持つ。
//!
//! ズームとパンの計算は egui に依存しない関数（[`fit_scale`], [`zoom_at`]）に分けてあり、
//! テストではそちらを確かめる。

use std::path::PathBuf;

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Sense, TextureHandle, Vec2};

use crate::i18n::tr;
use crate::loader::ImageInfo;
use crate::settings::Language;

/// 表示中の画像。テクスチャと、ログ・メタデータ表示用の情報を持つ。
pub struct Shown {
    pub texture: TextureHandle,
    pub info: ImageInfo,
}

/// 画像フィールドの 1 枠。
pub struct Slot {
    /// スロットを識別する番号。並べ替えや削除で位置が変わっても読み込み結果を正しく届けるため、
    /// 配列の添字ではなくこれを使う。
    pub id: u64,
    pub path: Option<PathBuf>,
    /// 最後に出した読み込み要求の番号。これと違う番号の結果は古いので捨てる。
    /// 前後の画像へ素早く送ったとき、遅れて届いた前の画像で上書きされるのを防ぐ。
    pub request: u64,
    pub loading: bool,
    pub shown: Option<Shown>,
    pub error: Option<String>,
    /// 「枠に収まる倍率」に掛ける倍率。1.0 で全体表示。
    /// 画像の実寸に対する倍率で持たないのは、枠の大きさが変わったときに全体表示を保つため。
    zoom: f32,
    /// 枠の中心から画像の中心までのずれ（論理ピクセル）。
    pan: Vec2,
}

/// 1 フレーム分の操作結果。
#[derive(Default)]
pub struct SlotResponse {
    pub clicked: bool,
    pub close: bool,
}

/// ヘッダ（ファイル名とズーム率の帯）の高さ。
const HEADER: f32 = 20.0;
/// ズーム倍率の下限（全体表示に対して）。
const MIN_ZOOM: f32 = 0.05;
/// 実寸に対する拡大率の上限。画素を 1 つずつ確認できれば十分なので 200 倍で止める。
const MAX_PIXEL_SCALE: f32 = 200.0;
/// ホイール 1 論理ピクセル分のズーム量（指数の係数）。
const WHEEL_ZOOM_RATE: f32 = 0.002;

/// 画像を枠に収める倍率。枠より小さい画像は拡大せず 100% で表示する。
/// 小さな画像を枠いっぱいに拡大しないのは、アイコンなどがぼやけて実寸が分からなくなるため。
pub fn fit_scale(view: Vec2, image: Vec2) -> f32 {
    if image.x <= 0.0 || image.y <= 0.0 {
        return 1.0;
    }
    (view.x / image.x).min(view.y / image.y).min(1.0)
}

/// 倍率を `factor` 倍し、`cursor`（画像中心からのずれではなく枠中心からの位置）の下にある
/// 点が動かないようにパンを補正した `(zoom, pan)` を返す。
pub fn zoom_at(zoom: f32, pan: Vec2, factor: f32, base: f32, cursor: Option<Vec2>) -> (f32, Vec2) {
    let new_zoom = (zoom * factor).clamp(MIN_ZOOM, MAX_PIXEL_SCALE / base.max(1e-6));
    let pan = match cursor {
        Some(c) => pan + (c - pan) * (1.0 - new_zoom / zoom),
        None => pan,
    };
    (new_zoom, pan)
}

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

    /// 全体表示・中央に戻す。
    pub fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    fn base_scale(&self, body: Rect) -> f32 {
        let Some(s) = &self.shown else { return 1.0 };
        // テクスチャの大きさではなく元画像の大きさを使う。テクスチャは上限に合わせて
        // 縮小されていることがあり、それで計算すると表示倍率が実寸とずれるため。
        fit_scale(
            body.size(),
            vec2(s.info.image.width as f32, s.info.image.height as f32),
        )
    }

    /// `rect` の範囲にスロットを描き、操作を処理する。`background` は画像の背後の色。
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        active: bool,
        background: Color32,
        lang: Language,
    ) -> SlotResponse {
        let mut out = SlotResponse::default();
        let visuals = ui.visuals().clone();
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
        let body = Rect::from_min_max(pos2(rect.min.x, rect.min.y + HEADER), rect.max);
        let painter = ui.painter_at(rect);

        painter.rect_filled(body, 0.0, background);
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
                // ホイールはスクロールではなくズームに使う。画像ビューワでは
                // スクロールより拡大縮小の頻度が高く、移動はドラッグで足りるため。
                let factor = pinch * (scroll * WHEEL_ZOOM_RATE).exp();
                if factor != 1.0 {
                    let cursor = pointer.map(|p| p - body.center());
                    (self.zoom, self.pan) = zoom_at(self.zoom, self.pan, factor, base, cursor);
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
            let msg = tr(lang, msg);
            painter.text(
                body.center(),
                egui::Align2::CENTER_CENTER,
                msg,
                egui::FontId::proportional(14.0),
                Color32::GRAY,
            );
        }

        // ヘッダ: ファイル名、表示倍率、閉じるボタン。
        // ヘッダ全体のクリック判定を閉じるボタンより先に登録する。egui では後から登録した
        // 判定が上に来るので、逆にすると閉じるボタンが押せなくなる。
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| tr(lang, "(empty)").into());
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
        out.close = close.on_hover_text(tr(lang, "Close slot")).clicked();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_image_fits_the_limiting_side() {
        assert_eq!(fit_scale(vec2(100.0, 100.0), vec2(400.0, 200.0)), 0.25);
        assert_eq!(fit_scale(vec2(100.0, 100.0), vec2(200.0, 400.0)), 0.25);
    }

    #[test]
    fn small_image_is_not_enlarged() {
        assert_eq!(fit_scale(vec2(1000.0, 1000.0), vec2(10.0, 10.0)), 1.0);
    }

    #[test]
    fn empty_image_does_not_divide_by_zero() {
        assert_eq!(fit_scale(vec2(100.0, 100.0), vec2(0.0, 0.0)), 1.0);
    }

    #[test]
    fn zoom_keeps_point_under_cursor_fixed() {
        let (zoom, pan, base) = (1.0, vec2(10.0, -5.0), 0.5);
        let cursor = vec2(40.0, 30.0);
        // カーソル位置にある画像上の点（画像中心からの距離を倍率で割った値）
        let image_point = (cursor - pan) / (base * zoom);
        let (z2, p2) = zoom_at(zoom, pan, 2.0, base, Some(cursor));
        assert_eq!(z2, 2.0);
        let after = p2 + image_point * (base * z2);
        assert!((after - cursor).length() < 1e-4, "{after:?}");
    }

    #[test]
    fn zoom_without_cursor_keeps_pan() {
        let (z, p) = zoom_at(1.0, vec2(3.0, 4.0), 1.5, 1.0, None);
        assert_eq!((z, p), (1.5, vec2(3.0, 4.0)));
    }

    #[test]
    fn zoom_is_clamped() {
        assert_eq!(zoom_at(1.0, Vec2::ZERO, 1e-6, 1.0, None).0, MIN_ZOOM);
        // 全体表示が 50% なら、実寸 200 倍は全体表示の 400 倍。
        assert_eq!(zoom_at(1.0, Vec2::ZERO, 1e6, 0.5, None).0, 400.0);
    }

    #[test]
    fn new_slot_is_empty_and_fitted() {
        let slot = Slot::new(7);
        assert_eq!(slot.id, 7);
        assert!(slot.path.is_none() && slot.shown.is_none() && !slot.loading);
        assert_eq!((slot.zoom, slot.pan), (1.0, Vec2::ZERO));
    }
}
