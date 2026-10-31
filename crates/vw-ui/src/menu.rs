//! メニュー（メニューバー・右クリック・▼ の選択）の見た目と項目。
//!
//! 角を丸めた影のある窓に、指しやすい高さの行を並べ、指した行はアクセントの色で塗る。
//! egui 既定のメニューは細線で囲んだ四角に詰めて並べるだけで、古い OS の画面のように見えるため。

use egui::containers::menu::{MenuBar, MenuButton, MenuConfig, SubMenuButton};
use egui::{
    vec2, Atom, AtomExt, Button, Color32, ComboBox, CornerRadius, IntoAtoms, Popup,
    PopupCloseBehavior, Response, Shadow, Stroke, Style, Ui,
};

/// チェック欄の幅。チェックのない項目もこの幅だけ字を右へずらし、頭をそろえる。
const CHECK_WIDTH: f32 = 16.0;
const CHECK_HEIGHT: f32 = 14.0;

/// 開いたメニューの中身の見た目。egui の [`egui::containers::menu::menu_style`] の上に重ねる。
pub fn popup_style(s: &mut Style) {
    egui::containers::menu::menu_style(s);
    let dark = s.visuals.dark_mode;
    s.spacing.button_padding = vec2(10.0, 3.0);
    s.spacing.interact_size.y = 22.0;
    s.spacing.item_spacing.y = 0.0;
    s.spacing.menu_margin = egui::Margin::same(5);
    // 既定の幅では「全画面スライドショー...」とキーが並ぶと名前が切れる。
    s.spacing.menu_width = 240.0;
    s.visuals.menu_corner_radius = CornerRadius::same(7);
    s.visuals.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 140 } else { 50 }),
    };
    let accent = s.visuals.selection.stroke.color;
    let w = &mut s.visuals.widgets;
    for state in [&mut w.hovered, &mut w.open, &mut w.active] {
        state.corner_radius = CornerRadius::same(4);
        state.weak_bg_fill = accent;
        state.bg_fill = accent;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    }
}

/// メニューバーの見た目。開いているメニューの名前は、指した行と同じく角を丸めた面で示す。
fn bar_style(s: &mut Style) {
    egui::containers::menu::menu_style(s);
    s.spacing.button_padding = vec2(8.0, 3.0);
    s.spacing.interact_size.y = 24.0;
    s.spacing.item_spacing.x = 2.0;
    let w = &mut s.visuals.widgets;
    for state in [&mut w.hovered, &mut w.open, &mut w.active] {
        state.corner_radius = CornerRadius::same(4);
    }
}

/// メニューバーを描く。中には [`bar_menu`] でメニューを並べる。
pub fn bar<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
    MenuBar::new()
        .style(bar_style)
        .config(MenuConfig::new().style(popup_style))
        .ui(ui, content)
        .inner
}

/// メニューバーの 1 つのメニュー。どれかを開いている間は、名前にマウスを乗せるだけでそのメニューに
/// 切り替わる（Windows の標準のメニューと同じ）。
///
/// egui のメニューバーは、隣のメニューを開くのにもう一度クリックが要る。
/// 切り替わったメニューは次のフレームで開く（ボタンを描いたあとでないと乗っているか分からず、
/// そのときには元のメニューはもう描いてあるため）。
pub fn bar_menu<'a, R>(
    ui: &mut Ui,
    atoms: impl IntoAtoms<'a>,
    content: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let open_key = ui.id().with("vw-open-bar-menu");
    let (response, inner) = MenuButton::new(atoms).ui(ui, content);
    let ctx = ui.ctx();
    let popup_id = Popup::default_response_id(&response);
    let other_open = ui
        .data(|d| d.get_temp::<egui::Id>(open_key))
        .is_some_and(|open| open != popup_id && Popup::is_id_open(ctx, open));
    if other_open && response.hovered() {
        Popup::open_id(ctx, popup_id);
        ctx.request_repaint();
    }
    if Popup::is_id_open(ctx, popup_id) {
        ui.data_mut(|d| d.insert_temp(open_key, popup_id));
    }
    (response, inner.map(|r| r.inner))
}

/// 右クリックで開くメニュー。
pub fn context(response: &Response, content: impl FnOnce(&mut Ui)) {
    Popup::context_menu(response)
        .style(popup_style)
        .show(content);
}

/// 書き込む欄のある、右クリックで開くメニュー。項目を押したら呼ぶ側で `ui.close()` する。
///
/// 普通のメニューは中を押しても閉じるので、欄を押した途端に閉じて書き込めない。
/// すべてのメニューをこの閉じ方にしないのは、どの項目にも `ui.close()` が要るようになるため。
pub fn context_with_fields(response: &Response, content: impl FnOnce(&mut Ui)) {
    Popup::context_menu(response)
        .style(popup_style)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(content);
}

/// 書き込む欄のある、押すと下に開くメニュー。閉じ方は [`context_with_fields`] と同じ。
pub fn button_with_fields<'a, R>(
    ui: &mut Ui,
    atoms: impl IntoAtoms<'a>,
    content: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let config = MenuConfig::new()
        .style(popup_style)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside);
    let (response, inner) = MenuButton::new(atoms).config(config).ui(ui, content);
    (response, inner.map(|r| r.inner))
}

/// 押すと下にメニューを開くボタン（▼ の選択など）。メニューの中で使えば横に開く下の段になる。
pub fn button<'a, R>(
    ui: &mut Ui,
    atoms: impl IntoAtoms<'a>,
    content: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let (response, inner) = MenuButton::new(atoms)
        .config(MenuConfig::new().style(popup_style))
        .ui(ui, content);
    (response, inner.map(|r| r.inner))
}

/// ▼ で一覧を開く選択。一覧はメニューと同じ見た目にする。
pub fn combo(id_salt: impl egui::AsIdSalt) -> ComboBox {
    ComboBox::from_id_salt(id_salt).popup_style(popup_style.into())
}

/// チェック欄とキーの欄のあるメニューの 1 項目。押せないときの説明を付けられるよう、
/// 置く前のボタンを返す。
pub fn item<'a>(checked: bool, text: impl Into<String>, shortcut: impl Into<String>) -> Button<'a> {
    Button::new((check_atom(checked), text.into())).shortcut_text(shortcut.into())
}

/// チェック欄の幅だけ字をずらした、横に開く下の段。
pub fn submenu<R>(
    ui: &mut Ui,
    text: impl Into<String>,
    content: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let button =
        Button::new((check_atom(false), text.into())).right_text(SubMenuButton::RIGHT_ARROW);
    SubMenuButton::from_button(button)
        .ui(ui, content)
        .1
        .map(|r| r.inner)
}

fn check_atom<'a>(checked: bool) -> Atom<'a> {
    let mark = if checked { "\u{2713}" } else { "" };
    // 高さを 0 にすると、印が字の並びより下にずれる。
    Atom::from(mark)
        .atom_size(vec2(CHECK_WIDTH, CHECK_HEIGHT))
        .atom_align(egui::Align2::LEFT_CENTER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_menus_have_rounded_corners_and_a_shadow() {
        let mut s = Style::default();
        popup_style(&mut s);
        assert!(s.visuals.menu_corner_radius.nw > 0);
        assert!(s.visuals.popup_shadow.blur > 0);
    }

    #[test]
    fn the_pointed_row_is_filled_with_the_accent_and_drawn_in_white() {
        let mut s = Style::default();
        let accent = s.visuals.selection.stroke.color;
        popup_style(&mut s);
        assert_eq!(s.visuals.widgets.hovered.bg_fill, accent);
        assert_eq!(s.visuals.widgets.hovered.fg_stroke.color, Color32::WHITE);
    }

    #[test]
    fn menu_rows_are_taller_than_the_compact_default() {
        let mut s = Style::default();
        s.spacing.interact_size.y = 16.0;
        popup_style(&mut s);
        assert!(s.spacing.interact_size.y >= 22.0);
    }
}
