//! 「vw について」のウインドウ。名前・版・著作権・ライセンスと、同梱の
//! サードパーティのライセンス一覧を開くボタンを出す。

use std::path::PathBuf;

use eframe::egui;

use crate::i18n::tr;
use crate::settings::Language;

pub const COPYRIGHT: &str = "Copyright (c) 2026 vw contributors";
pub const AUTHOR: &str = "xex (xex-ken-xex)";
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
pub const LICENSE: &str = env!("CARGO_PKG_LICENSE");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 配布物で vw の実行ファイルと同じフォルダに置かれる、依存クレートのライセンス一覧。
const THIRD_PARTY: &str = "THIRD-PARTY-LICENSES.txt";

#[derive(Default)]
pub struct About {
    pub open: bool,
}

impl About {
    /// 開いていれば描く。一覧を開けなかったときは、その理由を返す（ログに出す）。
    pub fn ui(&mut self, ctx: &egui::Context, lang: Language) -> Option<String> {
        if !self.open {
            return None;
        }
        let mut error = None;
        let mut open = self.open;
        egui::Window::new(tr(lang, "About vw"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.heading(format!("vw {VERSION}"));
                ui.label(tr(lang, "Fast image viewer"));
                ui.add_space(6.0);
                ui.label(COPYRIGHT);
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Author"));
                    ui.label(AUTHOR);
                });
                ui.hyperlink(REPOSITORY);
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "License"));
                    ui.label(LICENSE);
                });
                ui.add_space(6.0);
                let path = third_party_path();
                let exists = path.as_ref().is_some_and(|p| p.exists());
                let button = ui
                    .add_enabled(
                        exists,
                        egui::Button::new(tr(lang, "Third-party licenses...")),
                    )
                    .on_disabled_hover_text(tr(
                        lang,
                        "THIRD-PARTY-LICENSES.txt is not next to vw (it comes with the release zip).",
                    ));
                if button.clicked() {
                    if let Some(path) = path {
                        if let Err(e) = crate::launch::open_file(&path) {
                            error = Some(format!("{}: {e}", path.display()));
                        }
                    }
                }
            });
        self.open = open;
        error
    }
}

fn third_party_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join(THIRD_PARTY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_shows_the_dual_license_and_the_repository() {
        assert_eq!(LICENSE, "MIT OR Apache-2.0");
        assert_eq!(REPOSITORY, "https://github.com/xex-ken-xex/vw");
    }
}
