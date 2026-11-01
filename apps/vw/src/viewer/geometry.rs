//! 矩形と倍率の計算。egui の `Ui` には触らず、数だけを受けて数だけを返すので、
//! テストはこちらで振る舞いを確かめる。

use eframe::egui::{self, MouseWheelUnit, Pos2, Rect, Vec2, pos2, vec2};

use crate::probe::Mark;
use crate::roi::Region;

/// ズーム倍率の下限（全体表示に対して）。
pub const MIN_ZOOM: f32 = 0.05;
/// 実寸に対する拡大率の上限。画素を 1 つずつ確認できれば十分なので 200 倍で止める。
pub const MAX_PIXEL_SCALE: f32 = 200.0;
/// 拡大縮小のドラッグ 1 論理ピクセル分のズーム量（指数の係数）。100 px で約 2.7 倍。
pub const DRAG_ZOOM_RATE: f32 = 0.01;
/// ホイールで 1 枚送るのに要る回転量（論理ピクセル）。egui がホイール 1 目盛りを
/// 40 論理ピクセルに換算するので、1 目盛りで 1 枚になる。
pub const WHEEL_STEP: f32 = 40.0;
/// キーで拡大縮小するときの 1 回分の倍率。
pub const KEY_ZOOM_FACTOR: f32 = 1.25;
/// 十字をクリックで選べる距離（論理ピクセル）。
pub const MARK_HIT: f32 = 8.0;

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

/// 画像を枠いっぱいに収める倍率。枠より小さい画像も拡大する。
///
/// 小さな画像を 100% のまま止めないのは、1x1 や最大化でスロットを広げても画像が
/// 広がらず、「枠に合わせる」が効いていないように見えるため。実寸で見たいときは
/// 100% 表示の操作がある。
pub fn fit_scale(view: Vec2, image: Vec2) -> f32 {
    if image.x <= 0.0 || image.y <= 0.0 {
        return 1.0;
    }
    (view.x / image.x).min(view.y / image.y)
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

/// 枠（大きさ `body`）に見えている画像の範囲。画像の左上を (0, 0)、右下を (1, 1) とした
/// 割合で、画像の外にはみ出す分は切り取る。`scale` は実寸に対する表示倍率、
/// `pan` は枠の中心から画像の中心までのずれ。
pub fn visible_part(body: Vec2, image: Vec2, scale: f32, pan: Vec2) -> Rect {
    let shown = image * scale;
    if shown.x <= 0.0 || shown.y <= 0.0 {
        return Rect::NOTHING;
    }
    let to_image = |q: Vec2| ((q - pan) / shown + vec2(0.5, 0.5)).to_pos2();
    let r = Rect::from_min_max(to_image(-body / 2.0), to_image(body / 2.0));
    r.intersect(Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)))
}

/// 画像の `at`（[`visible_part`] と同じ割合）を枠の中心に持ってくるパン。
pub fn pan_to_center(at: Pos2, image: Vec2, scale: f32) -> Vec2 {
    -(at.to_vec2() - vec2(0.5, 0.5)) * image * scale
}

/// ズームマップに渡す、スロットの今の表示。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewWindow {
    /// 見えている範囲（[`visible_part`]）。
    pub visible: Rect,
    /// 実寸に対する表示倍率。
    pub scale: f32,
    /// 選べる倍率の範囲。
    pub min_scale: f32,
    pub max_scale: f32,
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
pub fn wheel_delta(events: &[egui::Event], page_height: f32) -> f32 {
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

/// このフレームのホイールの回転量を、Shift なしと Shift 付きに分ける（[`wheel_delta`] と同じ向きと単位）。
/// Shift 付きはページ送りに使う。
///
/// Shift 付きは横の回転量も数える。macOS などでは OS が Shift+ホイールを横スクロールに
/// 変えて届けるため、縦だけ見ると回しても何も起きない。
pub fn split_wheel(events: &[egui::Event], page_height: f32) -> (f32, f32) {
    let (mut plain, mut shifted) = (0.0, 0.0);
    for e in events {
        let egui::Event::MouseWheel {
            unit,
            delta,
            modifiers,
            ..
        } = e
        else {
            continue;
        };
        if modifiers.command {
            continue;
        }
        let scale = match unit {
            MouseWheelUnit::Point => 1.0,
            MouseWheelUnit::Line => WHEEL_STEP,
            MouseWheelUnit::Page => page_height,
        };
        if modifiers.shift {
            shifted += (delta.y + delta.x) * scale;
        } else {
            plain += delta.y * scale;
        }
    }
    (plain, shifted)
}

/// 枠の中に置いた画像。画素と画面の位置の変換はすべてここを通す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// 画像を描く矩形（枠からはみ出す分も含む）。
    pub rect: Rect,
    /// 実寸に対する表示倍率。
    pub scale: f32,
    pub width: u32,
    pub height: u32,
}

impl Placed {
    /// 枠 `body` の中心から `pan` だけずらして、`width` x `height` の画像を `scale` 倍で置く。
    pub fn new(body: Rect, width: u32, height: u32, scale: f32, pan: Vec2) -> Self {
        let size = vec2(width as f32, height as f32) * scale;
        Self {
            rect: Rect::from_center_size(body.center() + pan, size),
            scale,
            width,
            height,
        }
    }

    pub fn size(&self) -> Vec2 {
        self.rect.size()
    }

    /// 画面の位置にある画素。画像の外なら `None`。
    pub fn pixel_at(&self, p: Pos2) -> Option<(u32, u32)> {
        let v = (p - self.rect.min) / self.scale;
        let inside =
            v.x >= 0.0 && v.y >= 0.0 && (v.x as u32) < self.width && (v.y as u32) < self.height;
        inside.then_some((v.x as u32, v.y as u32))
    }

    /// 画面の位置にいちばん近い画素。画像の外にはみ出したドラッグも端の画素に丸めて矩形にするため。
    pub fn pixel_clamped(&self, p: Pos2) -> (u32, u32) {
        let v = (p - self.rect.min) / self.scale;
        (
            (v.x.max(0.0) as u32).min(self.width.saturating_sub(1)),
            (v.y.max(0.0) as u32).min(self.height.saturating_sub(1)),
        )
    }

    /// 画素の中心の画面の位置。
    pub fn pixel_center(&self, x: u32, y: u32) -> Pos2 {
        self.rect.min + vec2(x as f32 + 0.5, y as f32 + 0.5) * self.scale
    }

    pub fn mark_pos(&self, m: &Mark) -> Pos2 {
        self.pixel_center(m.x, m.y)
    }

    /// 画素の矩形の画面の矩形。
    pub fn region_rect(&self, r: Region) -> Rect {
        Rect::from_min_max(
            self.rect.min + vec2(r.x as f32, r.y as f32) * self.scale,
            self.rect.min + vec2(r.right() as f32, r.bottom() as f32) * self.scale,
        )
    }

    /// 画像の幅と高さに対する割合 `[x, y, w, h]` で表した矩形（顔の枠）の画面の矩形。
    pub fn fraction_rect(&self, f: [f32; 4]) -> Rect {
        let (w, h) = (self.width as f32, self.height as f32);
        Rect::from_min_size(
            self.rect.min + vec2(f[0] * w, f[1] * h) * self.scale,
            vec2(f[2] * w, f[3] * h) * self.scale,
        )
    }
}

/// 上下のドラッグ `dy`（論理ピクセル、下が正）に対する倍率。上へ引くと拡大。
pub fn drag_zoom_factor(dy: f32) -> f32 {
    (-dy * DRAG_ZOOM_RATE).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_image_is_visible_when_it_fits() {
        let r = visible_part(vec2(200.0, 100.0), vec2(100.0, 100.0), 1.0, Vec2::ZERO);
        assert_eq!(r, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)));
    }

    #[test]
    fn zoomed_in_view_shows_the_middle_part_shifted_by_the_pan() {
        let r = visible_part(vec2(100.0, 100.0), vec2(100.0, 100.0), 4.0, Vec2::ZERO);
        assert_eq!(
            r,
            Rect::from_min_max(pos2(0.375, 0.375), pos2(0.625, 0.625))
        );
        // 画像を右へずらすと、見えるのは画像の左寄り。
        let r = visible_part(
            vec2(100.0, 100.0),
            vec2(100.0, 100.0),
            4.0,
            vec2(100.0, 0.0),
        );
        assert_eq!(r.min.x, 0.125);
    }

    #[test]
    fn centering_a_point_makes_it_the_middle_of_the_visible_part() {
        let (body, image, scale) = (vec2(100.0, 80.0), vec2(300.0, 200.0), 2.0);
        let at = pos2(0.2, 0.7);
        let r = visible_part(body, image, scale, pan_to_center(at, image, scale));
        assert!((r.center() - at).length() < 1e-5, "{r:?}");
    }

    #[test]
    fn large_image_fits_the_limiting_side() {
        assert_eq!(fit_scale(vec2(100.0, 100.0), vec2(400.0, 200.0)), 0.25);
        assert_eq!(fit_scale(vec2(100.0, 100.0), vec2(200.0, 400.0)), 0.25);
    }

    #[test]
    fn small_image_is_enlarged_to_fill_the_slot() {
        assert_eq!(fit_scale(vec2(1000.0, 500.0), vec2(10.0, 10.0)), 50.0);
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
    fn wheel_with_shift_turns_pages_instead_of_images() {
        let wheel = |modifiers: egui::Modifiers, delta: Vec2| egui::Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta,
            phase: egui::TouchPhase::Move,
            modifiers,
        };
        let down = vec2(0.0, -1.0);
        assert_eq!(
            split_wheel(&[wheel(egui::Modifiers::NONE, down)], 500.0),
            (-WHEEL_STEP, 0.0)
        );
        assert_eq!(
            split_wheel(&[wheel(egui::Modifiers::SHIFT, down)], 500.0),
            (0.0, -WHEEL_STEP)
        );
        // OS が横スクロールに変えて届けても、ページ送りとして数える。
        assert_eq!(
            split_wheel(&[wheel(egui::Modifiers::SHIFT, vec2(-1.0, 0.0))], 500.0),
            (0.0, -WHEEL_STEP)
        );
        let ctrl_shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        assert_eq!(split_wheel(&[wheel(ctrl_shift, down)], 500.0), (0.0, 0.0));
    }

    #[test]
    fn pixels_are_found_only_inside_the_placed_image() {
        // 10 x 4 の画像を 2 倍で、枠 (0,0)-(100,100) の中心に置く: 矩形は (40,46)-(60,54)。
        let p = Placed::new(
            Rect::from_min_max(Pos2::ZERO, pos2(100.0, 100.0)),
            10,
            4,
            2.0,
            Vec2::ZERO,
        );
        assert_eq!(
            p.rect,
            Rect::from_min_max(pos2(40.0, 46.0), pos2(60.0, 54.0))
        );
        assert_eq!(p.pixel_at(pos2(40.0, 46.0)), Some((0, 0)));
        assert_eq!(p.pixel_at(pos2(59.9, 53.9)), Some((9, 3)));
        assert_eq!(p.pixel_at(pos2(60.0, 50.0)), None);
        assert_eq!(p.pixel_at(pos2(39.9, 50.0)), None);
    }

    #[test]
    fn dragging_outside_the_image_is_rounded_to_the_edge_pixel() {
        let p = Placed::new(
            Rect::from_min_max(Pos2::ZERO, pos2(100.0, 100.0)),
            10,
            4,
            2.0,
            Vec2::ZERO,
        );
        assert_eq!(p.pixel_clamped(pos2(-50.0, -50.0)), (0, 0));
        assert_eq!(p.pixel_clamped(pos2(500.0, 500.0)), (9, 3));
    }

    #[test]
    fn a_pixel_center_maps_back_to_the_same_pixel() {
        let p = Placed::new(
            Rect::from_min_max(Pos2::ZERO, pos2(100.0, 100.0)),
            10,
            4,
            2.0,
            vec2(3.0, -7.0),
        );
        assert_eq!(p.pixel_at(p.pixel_center(7, 2)), Some((7, 2)));
        assert_eq!(p.pixel_center(0, 0), p.rect.min + vec2(1.0, 1.0));
    }

    #[test]
    fn region_and_fraction_rects_follow_the_scale() {
        let p = Placed::new(
            Rect::from_min_max(Pos2::ZERO, pos2(100.0, 100.0)),
            10,
            4,
            2.0,
            Vec2::ZERO,
        );
        let r = p.region_rect(Region {
            x: 1,
            y: 1,
            w: 2,
            h: 2,
        });
        assert_eq!(r, Rect::from_min_max(pos2(42.0, 48.0), pos2(46.0, 52.0)));
        let f = p.fraction_rect([0.5, 0.0, 0.5, 1.0]);
        assert_eq!(f, Rect::from_min_max(pos2(50.0, 46.0), pos2(60.0, 54.0)));
    }

    #[test]
    fn pulling_the_drag_up_zooms_in_and_down_zooms_out() {
        assert!(drag_zoom_factor(-100.0) > 2.0);
        assert!(drag_zoom_factor(100.0) < 0.5);
        assert_eq!(drag_zoom_factor(0.0), 1.0);
    }
}
