//! 見た目（配色・線・余白）。
//!
//! 画像編集ソフトのような、灰色を基調にした 1px の細線で区切る画面にする。
//! ダークは暗い部屋で目が疲れないよう真っ黒を避けた濃い灰色、ライトは明るい灰色。
//! 色相を持たない灰色にするのは、画像の色の見え方に影響させないため。

use eframe::egui::{self, Color32, CornerRadius, Stroke, Style, Theme, ThemePreference, Visuals};

use crate::settings::{Appearance, ThemeChoice};

/// 1 つのテーマの色。
struct Palette {
    /// ウィンドウの一番奥（メニュー・パネル間の地）。
    base: Color32,
    /// パネルの面。
    panel: Color32,
    /// 入力欄やボタンの面。
    field: Color32,
    /// 区切り線。
    line: Color32,
    text: Color32,
    text_strong: Color32,
    accent: Color32,
    accent_weak: Color32,
    /// 画像の背景。
    canvas: Color32,
}

const DARK: Palette = Palette {
    base: Color32::from_rgb(0x1b, 0x1c, 0x1e),
    panel: Color32::from_rgb(0x23, 0x24, 0x27),
    field: Color32::from_rgb(0x2c, 0x2d, 0x31),
    line: Color32::from_rgb(0x3a, 0x3c, 0x41),
    text: Color32::from_rgb(0xc8, 0xca, 0xce),
    text_strong: Color32::from_rgb(0xee, 0xef, 0xf1),
    accent: Color32::from_rgb(0x5b, 0x93, 0xf0),
    accent_weak: Color32::from_rgb(0x2a, 0x3a, 0x55),
    canvas: Color32::from_rgb(0x14, 0x15, 0x16),
};

const LIGHT: Palette = Palette {
    base: Color32::from_rgb(0xe4, 0xe5, 0xe7),
    panel: Color32::from_rgb(0xf1, 0xf2, 0xf3),
    field: Color32::from_rgb(0xfb, 0xfb, 0xfc),
    line: Color32::from_rgb(0xc6, 0xc9, 0xce),
    text: Color32::from_rgb(0x30, 0x33, 0x38),
    text_strong: Color32::from_rgb(0x10, 0x12, 0x14),
    accent: Color32::from_rgb(0x2f, 0x6f, 0xd6),
    accent_weak: Color32::from_rgb(0xd6, 0xe3, 0xf7),
    canvas: Color32::from_rgb(0xd2, 0xd4, 0xd7),
};

fn palette(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

/// フラグの色。灰色基調の画面の中で、ダーク・ライトのどちらでも見分けやすい彩度にする。
pub fn flag_color(flag: crate::labels::Flag) -> Color32 {
    match flag {
        crate::labels::Flag::Good => Color32::from_rgb(76, 175, 80),
        crate::labels::Flag::Bad => Color32::from_rgb(229, 83, 75),
    }
}

/// フラグのない画像で絞り込んでいるときの色。
pub const UNFLAGGED: Color32 = Color32::from_rgb(158, 158, 158);

/// 設定の `index` 番目のタグの色。フラグの緑・赤と見分けやすい色から順に使い、足りなければ繰り返す。
pub fn tag_color(index: usize) -> Color32 {
    const COLORS: [Color32; 8] = [
        Color32::from_rgb(66, 165, 245),
        Color32::from_rgb(255, 193, 7),
        Color32::from_rgb(171, 71, 188),
        Color32::from_rgb(38, 198, 218),
        Color32::from_rgb(255, 112, 67),
        Color32::from_rgb(236, 64, 122),
        Color32::from_rgb(156, 204, 101),
        Color32::from_rgb(121, 134, 203),
    ];
    COLORS[index % COLORS.len()]
}

/// テーマの既定の画像背景色。
pub fn canvas_color(theme: Theme) -> Color32 {
    palette(theme).canvas
}

/// 隣の色より少し明るい（ライトでは暗い）色。ホバーなどの段差に使う。
fn step(c: Color32, theme: Theme, amount: u8) -> Color32 {
    let f = |v: u8| match theme {
        Theme::Dark => v.saturating_add(amount),
        Theme::Light => v.saturating_sub(amount),
    };
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

fn visuals(theme: Theme) -> Visuals {
    let p = palette(theme);
    let mut v = match theme {
        Theme::Dark => Visuals::dark(),
        Theme::Light => Visuals::light(),
    };
    let square = CornerRadius::same(2);
    let hairline = Stroke::new(1.0, p.line);

    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.window_stroke = hairline;
    v.window_corner_radius = square;
    v.menu_corner_radius = square;
    // 影は奥行きを出す装飾で、細線で区切る方針と合わないので消す。
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    v.extreme_bg_color = p.base;
    // 入力欄は一段沈んだ色にして、枠線なしでも入力できる場所だと分かるようにする。
    v.text_edit_bg_color = Some(p.base);
    v.faint_bg_color = step(p.panel, theme, 6);
    v.code_bg_color = p.field;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent_weak;
    v.selection.stroke = Stroke::new(1.0, p.accent);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = hairline;
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = square;

    w.inactive.bg_fill = p.field;
    w.inactive.weak_bg_fill = p.field;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = square;

    w.hovered.bg_fill = step(p.field, theme, 12);
    w.hovered.weak_bg_fill = step(p.field, theme, 12);
    w.hovered.bg_stroke = hairline;
    w.hovered.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.hovered.corner_radius = square;
    // egui 既定ではホバーで少し膨らむが、細線のレイアウトが揺れて見えるのでやめる。
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.accent_weak;
    w.active.weak_bg_fill = p.accent_weak;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.active.corner_radius = square;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.field;
    w.open.weak_bg_fill = p.field;
    w.open.bg_stroke = hairline;
    w.open.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.open.corner_radius = square;
    v
}

fn style(theme: Theme, compact: bool) -> Style {
    let mut s = Style {
        visuals: visuals(theme),
        ..Style::default()
    };
    if compact {
        let sp = &mut s.spacing;
        sp.item_spacing = egui::vec2(6.0, 2.0);
        sp.button_padding = egui::vec2(5.0, 1.0);
        sp.menu_margin = egui::Margin::same(4);
        sp.window_margin = egui::Margin::same(4);
        sp.indent = 14.0;
        sp.interact_size.y = 16.0;
        sp.icon_width = 12.0;
        sp.icon_width_inner = 7.0;
        sp.icon_spacing = 4.0;
    }
    s
}

/// 外観の設定を egui に反映する。ダーク用とライト用の両方を登録しておき、
/// どちらを使うかは egui に任せる（System なら OS の設定が変わった時点で切り替わる）。
pub fn apply(ctx: &egui::Context, appearance: &Appearance) {
    ctx.set_style_of(Theme::Dark, style(Theme::Dark, appearance.compact));
    ctx.set_style_of(Theme::Light, style(Theme::Light, appearance.compact));
    ctx.set_theme(match appearance.theme {
        ThemeChoice::System => ThemePreference::System,
        ThemeChoice::Dark => ThemePreference::Dark,
        ThemeChoice::Light => ThemePreference::Light,
    });
    ctx.set_zoom_factor(appearance.clamped_scale());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma(c: Color32) -> u32 {
        (c.r() as u32 + c.g() as u32 + c.b() as u32) / 3
    }

    #[test]
    fn dark_theme_avoids_pure_black_and_white() {
        let v = visuals(Theme::Dark);
        assert!(luma(v.panel_fill) > 0x10 && luma(v.panel_fill) < 0x40);
        assert_ne!(v.widgets.noninteractive.fg_stroke.color, Color32::WHITE);
    }

    #[test]
    fn light_theme_is_light() {
        assert!(luma(visuals(Theme::Light).panel_fill) > 0xc0);
        assert!(!visuals(Theme::Light).dark_mode);
    }

    #[test]
    fn surfaces_are_neutral_gray_so_images_are_not_tinted() {
        for theme in [Theme::Dark, Theme::Light] {
            let p = palette(theme);
            for c in [p.base, p.panel, p.field, p.line, p.canvas] {
                let spread = c.r().max(c.g()).max(c.b()) - c.r().min(c.g()).min(c.b());
                assert!(spread <= 8, "{c:?}");
            }
        }
    }

    #[test]
    fn separators_are_one_pixel_hairlines() {
        for theme in [Theme::Dark, Theme::Light] {
            assert_eq!(visuals(theme).widgets.noninteractive.bg_stroke.width, 1.0);
        }
    }

    #[test]
    fn compact_style_has_tighter_spacing_than_default() {
        let compact = style(Theme::Dark, true).spacing;
        let normal = style(Theme::Dark, false).spacing;
        assert!(compact.item_spacing.y < normal.item_spacing.y);
        assert!(compact.interact_size.y < normal.interact_size.y);
    }
}
