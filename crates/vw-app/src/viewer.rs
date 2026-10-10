//! 画像フィールドの 1 枠（スロット）。ヘッダ、ズーム、パン（移動）を受け持つ。
//!
//! ズームとパンの計算は egui に依存しない関数（[`fit_scale`], [`zoom_at`]）に分けてあり、
//! テストではそちらを確かめる。

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, pos2, vec2, Color32, MouseWheelUnit, PointerButton, Pos2, Rect, Sense, TextureHandle,
    Vec2,
};
use vw_core::VwImage;

use crate::i18n::tr;
use crate::loader::ImageInfo;
use crate::settings::Language;

/// 表示中の画像。縮小画像のテクスチャと、ログ・メタデータ表示用の情報を持つ。
pub struct Shown {
    /// 表示中の画像のファイル。次の画像を読み込み中は [`Slot::path`] と食い違う。
    pub path: PathBuf,
    pub thumb: TextureHandle,
    pub info: ImageInfo,
    /// フル解像度の画像。縮小画像では足りない大きさで表示するときだけ持つ（[`crate::memory`]）。
    pub full: Option<Full>,
}

pub struct Full {
    pub image: Arc<VwImage>,
    pub texture: TextureHandle,
    pub bytes: usize,
    /// 最後に必要とされたフレーム番号。
    pub last_used: u64,
}

impl Shown {
    /// 描画に使うテクスチャ。フル解像度があればそれ、なければ縮小画像。
    pub fn texture(&self) -> &TextureHandle {
        self.full.as_ref().map_or(&self.thumb, |f| &f.texture)
    }
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
    /// 読み込み中のフル解像度の画像の要求番号と、見込みバイト数。
    pub full_request: Option<(u64, usize)>,
    /// フル解像度の画像が届いたらコピーする（コピーしようとしたときに捨ててあった）。
    pub copy_when_full: bool,
    /// 直前のフレームで、縮小画像では足りない大きさで表示した。
    pub wants_full_now: bool,
    /// 「枠に収まる倍率」に掛ける倍率。1.0 で全体表示。
    /// 画像の実寸に対する倍率で持たないのは、枠の大きさが変わったときに全体表示を保つため。
    zoom: f32,
    /// 枠の中心から画像の中心までのずれ（論理ピクセル）。
    pan: Vec2,
    /// まだ 1 段に満たないホイールの回転量（論理ピクセル）。
    wheel: f32,
    /// 直前に描いた画像部分の大きさ。キー操作で倍率を変えるときに使う。
    last_body: Option<Vec2>,
}

/// 1 フレーム分の操作結果。
#[derive(Default)]
pub struct SlotResponse {
    pub clicked: bool,
    pub close: bool,
    /// 右クリックメニューで「コピー」が選ばれた。
    pub copy: bool,
    /// 縮小画像では足りない大きさで表示している。
    pub wants_full: bool,
    /// ダブルクリックされた（最大化の切り替え）。
    pub toggle_maximize: bool,
    /// ホイールで送る枚数。負なら前へ。
    pub step: isize,
}

/// ヘッダ（ファイル名とズーム率の帯）の高さ。
const HEADER: f32 = 20.0;
/// ズーム倍率の下限（全体表示に対して）。
const MIN_ZOOM: f32 = 0.05;
/// 実寸に対する拡大率の上限。画素を 1 つずつ確認できれば十分なので 200 倍で止める。
const MAX_PIXEL_SCALE: f32 = 200.0;
/// 拡大縮小のドラッグ 1 論理ピクセル分のズーム量（指数の係数）。100 px で約 2.7 倍。
const DRAG_ZOOM_RATE: f32 = 0.01;
/// ホイールで 1 枚送るのに要る回転量（論理ピクセル）。egui がホイール 1 目盛りを
/// 40 論理ピクセルに換算するので、1 目盛りで 1 枚になる。
pub const WHEEL_STEP: f32 = 40.0;
/// キーで拡大縮小するときの 1 回分の倍率。
pub const KEY_ZOOM_FACTOR: f32 = 1.25;
/// パンに使うマウスボタン。
const PAN_BUTTON: PointerButton = PointerButton::Primary;
/// 上下のドラッグで拡大縮小するマウスボタン。
const ZOOM_BUTTON: PointerButton = PointerButton::Secondary;

/// ホイールの回転量 `delta`（論理ピクセル、上に回すと正）を `acc` に足し、
/// 1 段分たまるごとに送る枚数を返す。上に回すと前（負）、下に回すと次（正）。
///
/// タッチパッドは細かい量を何度も送ってくるので、1 回のイベントを 1 枚にはしない。
/// 回す向きが変わったら、それまでの端数は捨てる。逆向きに回し始めたのに、
/// 前の向きの端数を打ち消すまで反応しないのを避けるため。
pub fn wheel_steps(acc: &mut f32, delta: f32) -> isize {
    if delta == 0.0 {
        return 0;
    }
    if acc.signum() != delta.signum() {
        *acc = 0.0;
    }
    *acc += delta;
    let steps = (*acc / WHEEL_STEP).trunc();
    *acc -= steps * WHEEL_STEP;
    -(steps as isize)
}

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

/// 長辺 `shown_side`（物理ピクセル）で表示するのに、長辺 `thumb_side` の縮小画像では
/// 画素が足りないか。元画像（長辺 `image_side`）が縮小されていない小さな画像なら、
/// 縮小画像がそのまま元の画素なので要らない。
pub fn needs_full(shown_side: f32, thumb_side: usize, image_side: u32) -> bool {
    image_side as usize > thumb_side && shown_side > thumb_side as f32
}

/// このフレームのホイールの縦の回転量（論理ピクセル、上に回すと正）。
///
/// egui の `smooth_scroll_delta` を使わないのは、回転量を数フレームに分けて届けるので、
/// 1 目盛りで 1 枚送る判定がフレームの区切りでぶれるため。Ctrl 付きは egui が
/// ズームとして扱うので数えない。
fn wheel_delta(events: &[egui::Event], page_height: f32) -> f32 {
    events
        .iter()
        .filter_map(|e| match e {
            egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } if !modifiers.command => Some(match unit {
                MouseWheelUnit::Point => delta.y,
                MouseWheelUnit::Line => delta.y * WHEEL_STEP,
                MouseWheelUnit::Page => delta.y * page_height,
            }),
            _ => None,
        })
        .sum()
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
            full_request: None,
            copy_when_full: false,
            wants_full_now: false,
            zoom: 1.0,
            pan: Vec2::ZERO,
            wheel: 0.0,
            last_body: None,
        }
    }

    /// 枠の中心を動かさずに `factor` 倍する。
    pub fn zoom_by(&mut self, factor: f32) {
        let base = self.last_base();
        (self.zoom, self.pan) = zoom_at(self.zoom, self.pan, factor, base, Some(Vec2::ZERO));
    }

    /// 実寸（表示倍率 100%）にする。枠の中心にある点はそのまま。
    pub fn actual_size(&mut self) {
        let base = self.last_base();
        let factor = 1.0 / (base * self.zoom);
        self.zoom_by(factor);
    }

    fn last_base(&self) -> f32 {
        match self.last_body {
            Some(size) => self.base_scale(Rect::from_min_size(Pos2::ZERO, size)),
            None => 1.0,
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
        fit_scale(body.size(), vec2(s.info.width as f32, s.info.height as f32))
    }

    /// `rect` の範囲にスロットを描き、操作を処理する。`background` は画像の背後の色。
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        active: bool,
        background: Color32,
        lang: Language,
        empty_hint: &'static str,
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
        if response.clicked() || response.drag_started() || response.secondary_clicked() {
            out.clicked = true;
        }
        if self.shown.is_some() {
            response.context_menu(|ui| {
                if ui.button(tr(lang, "Copy")).clicked() {
                    out.copy = true;
                    ui.close();
                }
            });
        }
        out.toggle_maximize = response.double_clicked();
        if response.hovered() {
            let wheel = ui.input(|i| wheel_delta(&i.events, i.viewport_rect().height()));
            out.step = wheel_steps(&mut self.wheel, wheel);
        }

        self.last_body = Some(body.size());
        let base = self.base_scale(body);
        if let Some(shown) = &self.shown {
            if response.dragged_by(PAN_BUTTON) {
                self.pan += response.drag_delta();
            }
            if response.dragged_by(ZOOM_BUTTON) {
                let factor = (-response.drag_delta().y * DRAG_ZOOM_RATE).exp();
                let anchor = ui.input(|i| i.pointer.press_origin());
                let cursor = anchor.map(|p| p - body.center());
                (self.zoom, self.pan) = zoom_at(self.zoom, self.pan, factor, base, cursor);
            }
            if response.hovered() {
                // タッチパッドのピンチと Ctrl+ホイール（egui がズームとして届ける）。
                let (pinch, pointer) = ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
                if pinch != 1.0 {
                    let cursor = pointer.map(|p| p - body.center());
                    (self.zoom, self.pan) = zoom_at(self.zoom, self.pan, pinch, base, cursor);
                }
            }
            let scale = base * self.zoom;
            let size = vec2(shown.info.width as f32, shown.info.height as f32) * scale;
            let img_rect = Rect::from_center_size(body.center() + self.pan, size);
            let [tw, th] = shown.thumb.size();
            out.wants_full = needs_full(
                size.max_elem() * ui.ctx().pixels_per_point(),
                tw.max(th),
                shown.info.width.max(shown.info.height),
            );
            ui.painter_at(body).image(
                shown.texture().id(),
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
                empty_hint
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
    fn full_resolution_is_needed_only_when_shown_larger_than_the_thumbnail() {
        assert!(!needs_full(400.0, 512, 4000));
        assert!(!needs_full(512.0, 512, 4000));
        assert!(needs_full(513.0, 512, 4000));
    }

    #[test]
    fn images_smaller_than_the_thumbnail_never_need_full_resolution() {
        // 元が 300 x 200 なら縮小画像は元の画素そのもの。拡大表示しても読み直さない。
        assert!(!needs_full(3000.0, 300, 300));
    }

    #[test]
    fn image_just_over_the_thumbnail_size_needs_full_resolution() {
        // 513 px は半分の 257 px に縮小されるので、それより大きく出すなら元が要る。
        assert!(needs_full(300.0, 257, 513));
    }

    #[test]
    fn one_wheel_notch_steps_one_image() {
        let mut acc = 0.0;
        assert_eq!(wheel_steps(&mut acc, -WHEEL_STEP), 1);
        assert_eq!(wheel_steps(&mut acc, WHEEL_STEP), -1);
    }

    #[test]
    fn small_touchpad_deltas_add_up_to_one_step() {
        let mut acc = 0.0;
        assert_eq!(wheel_steps(&mut acc, -15.0), 0);
        assert_eq!(wheel_steps(&mut acc, -15.0), 0);
        assert_eq!(wheel_steps(&mut acc, -15.0), 1);
        assert_eq!(wheel_steps(&mut acc, -15.0), 0);
    }

    #[test]
    fn fast_spin_steps_several_images_at_once() {
        let mut acc = 0.0;
        assert_eq!(wheel_steps(&mut acc, -3.0 * WHEEL_STEP), 3);
    }

    #[test]
    fn reversing_the_wheel_drops_the_leftover() {
        let mut acc = 0.0;
        assert_eq!(wheel_steps(&mut acc, -30.0), 0);
        assert_eq!(wheel_steps(&mut acc, WHEEL_STEP), -1);
    }

    #[test]
    fn wheel_with_ctrl_is_not_counted_as_image_steps() {
        let wheel = |ctrl: bool| egui::Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            modifiers: if ctrl {
                egui::Modifiers::COMMAND
            } else {
                egui::Modifiers::NONE
            },
        };
        assert_eq!(wheel_delta(&[wheel(false)], 500.0), -WHEEL_STEP);
        assert_eq!(wheel_delta(&[wheel(true)], 500.0), 0.0);
    }

    #[test]
    fn new_slot_is_empty_and_fitted() {
        let slot = Slot::new(7);
        assert_eq!(slot.id, 7);
        assert!(slot.path.is_none() && slot.shown.is_none() && !slot.loading);
        assert_eq!((slot.zoom, slot.pan), (1.0, Vec2::ZERO));
    }
}
