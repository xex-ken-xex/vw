//! vw の画面に固有の色と、外観の設定の反映。灰色の地や線などの共通の見た目は [`vw_ui::theme`]。

use eframe::egui::{self, Color32, ThemePreference};

use crate::settings::{Appearance, ThemeChoice};

pub use vw_ui::theme::canvas_color;

/// フラグの色。灰色基調の画面の中で、ダーク・ライトのどちらでも見分けやすい彩度にする。
pub fn flag_color(flag: crate::labels::Flag) -> Color32 {
    match flag {
        crate::labels::Flag::Good => Color32::from_rgb(76, 175, 80),
        crate::labels::Flag::Bad => Color32::from_rgb(229, 83, 75),
    }
}

/// データセット（検索カタログのあるフォルダ）の名前の色。ダーク・ライトのどちらでも読める濃さの青緑。
pub const DATASET: Color32 = Color32::from_rgb(38, 166, 154);

/// 処理中を示す線の、進んだ部分と残りの部分の色。
pub const ACTIVITY: Color32 = Color32::from_rgb(229, 57, 53);
pub const ACTIVITY_TRACK: Color32 = Color32::from_rgb(110, 110, 110);

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

/// 外観の設定を egui に反映する。
pub fn apply(ctx: &egui::Context, appearance: &Appearance) {
    let preference = match appearance.theme {
        ThemeChoice::System => ThemePreference::System,
        ThemeChoice::Dark => ThemePreference::Dark,
        ThemeChoice::Light => ThemePreference::Light,
    };
    vw_ui::theme::apply(
        ctx,
        preference,
        appearance.compact,
        appearance.clamped_scale(),
    );
}
