//! ファイルメニューの「フォルダを開く…」と「最近開いたフォルダ」、ヘルプの「使い方（Web）」。

use eframe::egui;

use crate::i18n::tr;
use crate::settings::Language;

use super::*;

/// 使い方のページ。言葉の設定に合わせて開く。
fn user_guide_url(lang: Language) -> &'static str {
    match lang {
        Language::Japanese => "https://xex-ken-xex.github.io/vw/manual/",
        Language::English => "https://xex-ken-xex.github.io/vw/en/manual/",
    }
}

impl VwApp {
    /// フォルダを選ぶ窓を出す。最初は今のフォルダ（ZIP の中なら ZIP のあるフォルダ）を開いて見せる。
    pub(super) fn ask_folder_to_open(&mut self) {
        let start = self
            .current
            .as_ref()
            .map(|c| match self.archives.mount_of(c) {
                Some(mount) => mount
                    .zip
                    .parent()
                    .map_or_else(|| c.clone(), Path::to_path_buf),
                None => c.clone(),
            });
        let lang = self.lang();
        self.folder_picker = Some(Box::new(vw_tree::picker::Picker::folder(
            tr(lang, "Open folder"),
            start.as_deref(),
            crate::settings_ui::picker_labels(lang),
        )));
    }

    pub(super) fn folder_picker_ui(&mut self, ctx: &egui::Context) {
        let Some(picker) = &mut self.folder_picker else {
            return;
        };
        match picker.show(ctx) {
            vw_tree::picker::Outcome::Open => {}
            vw_tree::picker::Outcome::Cancelled => self.folder_picker = None,
            vw_tree::picker::Outcome::Chosen(folder) => {
                self.folder_picker = None;
                self.open_folder(ctx, folder);
            }
        }
    }

    /// 開いたフォルダを最近の一覧に入れる。展開した ZIP の中は終わると消えるので、入れない
    /// （ZIP そのものは [`VwApp::open_folder`] が入れる）。
    pub(super) fn remember_folder(&mut self, folder: &Path) {
        if self.archives.mount_of(folder).is_none() {
            crate::recent::remember(&mut self.recent_folders, folder);
        }
    }

    /// 「最近開いたフォルダ」の中身。選んだフォルダを返す。今はないフォルダは押せない。
    pub(super) fn recent_menu(&self, ui: &mut egui::Ui) -> Option<PathBuf> {
        let lang = self.lang();
        if self.recent_folders.is_empty() {
            ui.add_enabled(false, egui::Button::new(tr(lang, "No folders yet")));
            return None;
        }
        let mut chosen = None;
        for folder in &self.recent_folders {
            let label = folder.display().to_string();
            let here = folder.exists();
            let item = ui.add_enabled(here, vw_ui::menu::item(false, label, ""));
            let item = if here {
                item
            } else {
                item.on_disabled_hover_text(tr(lang, "Folder not found"))
            };
            if item.clicked() {
                chosen = Some(folder.clone());
                ui.close();
            }
        }
        chosen
    }

    pub(super) fn open_user_guide(&mut self) {
        let url = user_guide_url(self.lang());
        match vw_launch::open_url(url) {
            Ok(()) => self.log.info(format!("Help: opened {url}")),
            Err(e) => self.log.error(format!("Help: cannot open {url}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_guide_follows_the_language() {
        assert!(user_guide_url(Language::Japanese).ends_with("/vw/manual/"));
        assert!(user_guide_url(Language::English).ends_with("/vw/en/manual/"));
    }
}
