//! ヒストグラムやズームマップのような道具の欄の、見出しとキー操作。
//! 欄にする前の別ウインドウの位置（`Saved`）は、初めて欄として出すときの置き場所に使う。

use eframe::egui;

use crate::frames;
use crate::i18n::tr;
use crate::settings::Language;

/// 画像に追従するウインドウのタイトル。「名前 - ファイル名  Frame n」。
pub fn title(lang: Language, name: &'static str, path: Option<&std::path::Path>) -> String {
    let name = tr(lang, name);
    let Some(path) = path else {
        return name.to_owned();
    };
    if let Some(title) = crate::compose::title(path) {
        return format!("{name} - {title}");
    }
    let file = frames::file_of(path);
    let file = file.file_name().unwrap_or_default().to_string_lossy();
    match frames::split(path) {
        Some((_, frame)) => format!("{name} - {file}  {} {}", tr(lang, "Frame"), frame + 1),
        None => format!("{name} - {file}"),
    }
}

/// 起動をまたいで残す値。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Saved {
    /// 論理ピクセルの `[左, 上, 幅, 高さ]`。
    pub rect: Option<[f32; 4]>,
    pub on_top: bool,
}

/// 欄が自分のキー操作（コピー・削除など）を受けてよいか。本体に並べた欄は、本体の
/// ホットキーとぶつからないよう、マウスが欄の上にあるときだけ受ける。別ウインドウに
/// 出した欄は、そのウインドウにキーが届いている限り受ける。
pub fn owns_keys(ui: &egui::Ui) -> bool {
    ui.ctx().viewport_id() != egui::ViewportId::ROOT || ui.rect_contains_pointer(ui.max_rect())
}
