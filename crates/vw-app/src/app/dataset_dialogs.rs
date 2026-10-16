//! データセット画面のダイアログ（取り出し先の確認、外側のカタログの更新）。

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::i18n::{tr, tr1};
use crate::viewer::Slot;

use super::*;

impl VwApp {
    pub(super) fn open_dataset_dialog(
        &mut self,
        ctx: &egui::Context,
        request: crate::dataset_ui::Request,
    ) {
        if self.dataset_dialog.is_some() {
            self.log
                .warn(tr(self.lang(), "Finish the open dataset dialog first."));
            return;
        }
        self.dataset_dialog = Some(crate::dataset_ui::DatasetDialog::open(
            request,
            &self.settings().dataset.fields,
            self.datasets.index(),
            &self.extract_base(),
            self.loader.registry(),
            ctx,
        ));
    }

    /// 取り出しの出力先を置く場所。前に取り出した場所がまだあればそこ、なければユーザーフォルダ。
    pub(super) fn extract_base(&self) -> PathBuf {
        self.last_extract_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(std::env::home_dir)
            .unwrap_or_default()
    }

    pub(super) fn dataset_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.dataset_dialog else {
            return;
        };
        let lang = self.settings.settings.appearance.language;
        if !dialog.show(ctx, lang, self.loader.registry(), &mut self.log) {
            let dir = dialog.dir().to_path_buf();
            self.outer_to_update = dialog.outer().map(Path::to_path_buf);
            if let Some(dest) = dialog.extracted_to().map(Path::to_path_buf) {
                self.datasets.refresh(&dest);
                self.tree.forget_peek(&dest);
                if let Some(parent) = dest.parent() {
                    self.tree.refresh(parent, self.loader.registry());
                    self.last_extract_dir = Some(parent.to_path_buf());
                }
            }
            self.dataset_dialog = None;
            self.datasets.refresh(&dir);
            self.tree.forget_peek(&dir);
            self.next_queued_update(ctx);
        }
    }

    /// 取り出した先を含むデータセットを更新するか聞く。外側のデータセットは、更新するまで
    /// 取り出したファイルを含まないため。
    pub(super) fn outer_update_dialog(&mut self, ctx: &egui::Context) {
        let Some(outer) = &self.outer_to_update else {
            return;
        };
        if self.dataset_dialog.is_some() {
            return;
        }
        let lang = self.settings().appearance.language;
        let name = crate::catalog::load(outer)
            .map(|c| c.name)
            .unwrap_or_else(|_| outer.display().to_string());
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("outer_dataset_update")).show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.label(tr1(
                lang,
                "The files were extracted into the dataset \"{}\". Update it now?",
                &name,
            ));
            ui.weak(tr(
                lang,
                "Until it is updated, its search catalog does not include the extracted files.",
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr(lang, "Update")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr(lang, "Later")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(update) = answer {
            if let Some(outer) = self.outer_to_update.take() {
                if update {
                    self.open_dataset_dialog(ctx, crate::dataset_ui::Request::Update(outer));
                }
            }
        }
    }

    /// Datasets 画面から、データセット `dir` の検索で当たったファイル（カタログのパス）だけを
    /// 1 件 1 スロットで並べる。
    ///
    /// フォルダへは移らない（cd すると、当たらなかったファイルも並んでしまう）。ZIP / 7z の中の
    /// ファイルは並べない。開くには展開が要り、当たったファイルのためにアーカイブをいくつも展開すると
    /// 待たされるため。その数はログに出し、取り出せば見られることを伝える。
    pub(super) fn show_dataset_files(&mut self, dir: &Path, files: &[String]) {
        self.show_datasets = false;
        self.finish_folder_load();
        self.close_all_slots();
        self.tree.reveal(dir, &self.loader.registry().clone());
        self.current = None;
        let (archived, on_disk): (Vec<&String>, Vec<&String>) = files
            .iter()
            .partition(|f| f.contains(crate::catalog::IN_ARCHIVE));
        let paths: Vec<PathBuf> = on_disk
            .into_iter()
            .filter_map(|f| crate::dataset_extract::enclosed(f))
            .map(|f| dir.join(f))
            .collect();
        for _ in 1..paths.len() {
            self.slots.push(Slot::new(self.next_id));
            self.next_id += 1;
        }
        // 先にスロットを全部足してから枠の大きさを見積もる（cd と同じ理由）。
        let full = self.cell_needs_full();
        for (i, path) in paths.into_iter().enumerate() {
            self.start_load(i, path, full);
        }
        self.active = 0;
        self.log.info(format!(
            "Showing {} files found in the dataset {}",
            files.len() - archived.len(),
            dir.display()
        ));
        if !archived.is_empty() {
            self.log.warn(format!(
                "{} files found are inside ZIP / 7z files and are not shown. Extract them to see them.",
                archived.len()
            ));
        }
    }

    /// Datasets 画面から、データセットのフォルダへ移ってファイルを読み込む。検索カタログにあるのに
    /// 見つからないファイルはログに出し、更新を促す。
    pub(super) fn show_dataset(&mut self, ctx: &egui::Context, dir: PathBuf) {
        self.show_datasets = false;
        let into = self.settings().tree.load_into;
        self.cd(ctx, dir.clone(), into);
        let Some(entry) = self
            .datasets
            .index()
            .and_then(|i| i.entries.iter().find(|e| e.dir == dir))
        else {
            return;
        };
        let missing = crate::dataset_extract::missing(&dir, &entry.catalog);
        if missing.is_empty() {
            return;
        }
        const SHOWN: usize = 5;
        let mut list = missing[..missing.len().min(SHOWN)].join(", ");
        if missing.len() > SHOWN {
            list.push_str(", ...");
        }
        let name = entry.catalog.name.clone();
        self.log.warn(format!(
            "Dataset \"{name}\": {} files in the search catalog are missing ({list}). Update the dataset.",
            missing.len()
        ));
    }

    /// 「すべて更新」で残っているデータセットを、前のものが終わるたびに 1 つずつ更新する。
    /// 同時に走らせないのは、パスワードを聞くダイアログが重ならないようにするため。
    pub(super) fn next_queued_update(&mut self, ctx: &egui::Context) {
        if self.dataset_dialog.is_some() || self.dataset_queue.is_empty() {
            return;
        }
        let dir = self.dataset_queue.remove(0);
        self.open_dataset_dialog(ctx, crate::dataset_ui::Request::Update(dir));
    }
}
