//! 「vw Image Viewer について」のウインドウ。名前・版・著作権・ライセンス・Issue の画面へのリンクと、
//! 同梱のサードパーティのライセンス一覧を開くボタンを出す。

use std::path::PathBuf;

use eframe::egui;

use crate::i18n::tr;
use crate::settings::{Language, MapSettings};

/// 窓のタイトルや About に出す名前。ファイル名・コマンド名・設定フォルダは短い `vw` のまま。
pub const NAME: &str = "vw Image Viewer";
pub const COPYRIGHT: &str = "Copyright (c) 2026 vw contributors";
pub const AUTHOR: &str = "xex (xex-ken-xex)";
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// 不具合の報告と要望のテンプレートを選ぶ画面。
pub const ISSUES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues/new/choose");
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
    /// 地図の出典は、地図の絵の取り先を設定で変えられるので、いまの設定のものを出す。
    pub fn ui(&mut self, ctx: &egui::Context, lang: Language, map: &MapSettings) -> Option<String> {
        if !self.open {
            return None;
        }
        let mut error = None;
        let mut open = self.open;
        egui::Window::new(tr(lang, "About vw Image Viewer"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.heading(format!("{NAME} {VERSION}"));
                ui.label(tr(lang, "Fast image viewer"));
                ui.add_space(6.0);
                ui.label(COPYRIGHT);
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Author"));
                    ui.label(AUTHOR);
                });
                ui.hyperlink(REPOSITORY);
                ui.hyperlink_to(tr(lang, "Report a bug or request a feature..."), ISSUES);
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "License"));
                    ui.label(LICENSE);
                });
                ui.hyperlink_to(
                    tr(lang, "Place names: GeoNames, CC BY 4.0"),
                    crate::geonames::CREDIT_URL,
                )
                .on_hover_text(crate::geonames::CREDIT);
                ui.hyperlink_to(
                    tr(lang, "Digital source type names: IPTC NewsCodes, CC BY 4.0"),
                    "https://cv.iptc.org/newscodes/digitalsourcetype/",
                )
                .on_hover_text("Digital source type names: IPTC NewsCodes (Copyright IPTC), licensed under CC BY 4.0.");
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Map tiles:"));
                    ui.hyperlink_to(&map.attribution, &map.attribution_url);
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
                if button.clicked()
                    && let Some(path) = path
                        && let Err(e) = vw_launch::open_file(&path) {
                            error = Some(format!("{}: {e}", path.display()));
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

    #[test]
    fn issue_link_opens_the_template_chooser() {
        assert_eq!(
            ISSUES,
            "https://github.com/xex-ken-xex/vw/issues/new/choose"
        );
    }
}
