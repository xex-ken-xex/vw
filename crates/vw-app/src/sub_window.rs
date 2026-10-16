//! ヒストグラムやズームマップのような別ウインドウの、位置と大きさと「常に最前面」の記憶。

use eframe::egui;
use egui_phosphor::regular as icon;

use crate::frames;
use crate::i18n::tr;
use crate::settings::Language;

/// 画像に追従するウインドウのタイトル。「名前 - ファイル名  Frame n」。
pub fn title(lang: Language, name: &'static str, path: Option<&std::path::Path>) -> String {
    let name = tr(lang, name);
    let Some(path) = path else {
        return name.to_owned();
    };
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

/// 開くときに渡す位置と大きさは前回の終了時のものに固定し、動かした後の値は渡し直さない。
/// egui は渡した値が前のフレームと変わるとウインドウを動かすので、毎回今の値を渡すと
/// 利用者が動かしている最中のウインドウと取り合いになるため。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubWindow {
    initial: Option<[f32; 4]>,
    current: Option<[f32; 4]>,
    on_top: bool,
    /// 最前面の切り替えを押した。次のフレームでウインドウに送る。
    level_changed: bool,
}

impl SubWindow {
    pub fn restored(saved: Saved) -> Self {
        Self {
            initial: saved.rect,
            current: saved.rect,
            on_top: saved.on_top,
            level_changed: false,
        }
    }

    /// 終了時に残す値。
    pub fn saved(&self) -> Saved {
        Saved {
            rect: self.current,
            on_top: self.on_top,
        }
    }

    pub fn builder(&self, title: String, default_size: [f32; 2]) -> egui::ViewportBuilder {
        let b = egui::ViewportBuilder::default().with_title(title);
        let b = match self.initial {
            Some([x, y, w, h]) => b.with_position([x, y]).with_inner_size([w, h]),
            None => b.with_inner_size(default_size),
        };
        if self.on_top {
            b.with_always_on_top()
        } else {
            b
        }
    }

    /// ウインドウの中（そのビューポートの `ctx`）で毎フレーム呼ぶ。
    pub fn track(&mut self, ctx: &egui::Context) {
        let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
        if let (Some(outer), Some(inner)) = (outer, inner) {
            self.current = Some([outer.min.x, outer.min.y, inner.width(), inner.height()]);
        }
        if std::mem::take(&mut self.level_changed) {
            let level = if self.on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        }
    }

    /// 「常に最前面」の切り替えボタンをウインドウの右上に重ねる。
    /// パネルの中に置くと中身のレイアウトごとに位置を合わせる必要があるので、独立した領域にする。
    pub fn pin_overlay(&mut self, ctx: &egui::Context, lang: Language) {
        egui::Area::new(egui::Id::new("sub_window_pin"))
            .anchor(egui::Align2::RIGHT_TOP, [-6.0, 4.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| self.pin_button(ui, lang));
    }

    fn pin_button(&mut self, ui: &mut egui::Ui, lang: Language) {
        let text = egui::RichText::new(icon::PUSH_PIN).size(14.0);
        let button = egui::Button::new(text).selected(self.on_top).frame(false);
        let hint = if self.on_top {
            "Always on top (click to release)"
        } else {
            "Keep this window on top"
        };
        if ui.add(button).on_hover_text(tr(lang, hint)).clicked() {
            self.on_top = !self.on_top;
            self.level_changed = true;
        }
    }
}
