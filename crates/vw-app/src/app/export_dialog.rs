//! エクスポートのダイアログと実行。

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::export::{self, CopyJob, Filter, ListFormat};
use crate::i18n::tr;
use crate::labels::Flag;

use super::*;

impl VwApp {
    /// cd したフォルダ（なければアクティブな画像のフォルダ）を対象に、書き出しの画面を開く。
    pub(super) fn open_export(&mut self) {
        let root = self.current.clone().or_else(|| {
            self.active_image()
                .and_then(|p| p.parent().map(Path::to_path_buf))
        });
        let Some(root) = root else {
            self.log
                .warn(tr(self.lang(), "Select a folder in the tree first."));
            return;
        };
        // ZIP の中は終了時に消える一時フォルダなので、出力先は ZIP の隣に置く。
        let (name, beside) = match self.archives.mount_of(&root) {
            Some(m) if m.dir == root => (
                zip_stem(&m.zip),
                m.zip.parent().unwrap_or(&m.zip).to_owned(),
            ),
            Some(m) => (
                folder_name(&root),
                m.zip.parent().unwrap_or(&m.zip).to_owned(),
            ),
            None => (
                folder_name(&root),
                root.parent().unwrap_or(&root).to_owned(),
            ),
        };
        let suffix = match &self.filter {
            Filter::All => "export".to_owned(),
            Filter::Flag(Some(Flag::Good)) => "good".to_owned(),
            Filter::Flag(Some(Flag::Bad)) => "bad".to_owned(),
            Filter::Flag(None) => "unflagged".to_owned(),
            Filter::Tag(t) => t.clone(),
        };
        let root_label = match self.archives.mount_of(&root) {
            Some(m) => match root.strip_prefix(&m.dir) {
                Ok(rel) if !rel.as_os_str().is_empty() => m.zip.join(rel),
                _ => m.zip.clone(),
            }
            .display()
            .to_string(),
            None => root.display().to_string(),
        };
        self.export = Some(ExportDialog {
            root_label,
            dest: beside
                .join(format!("{name}-{suffix}"))
                .display()
                .to_string(),
            list_path: beside
                .join(format!("{name}-{suffix}.csv"))
                .display()
                .to_string(),
            root,
            recursive: true,
            kind: ExportKind::Copy,
            keep_structure: true,
            job: None,
        });
    }

    /// 書き出しの画面。
    pub(super) fn export_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.export else {
            return;
        };
        let lang = self.settings.settings.appearance.language;
        let filter_text = filter_name(&self.filter, lang);
        let mut open = true;
        let mut run = false;
        let mut cancel_job = false;
        egui::Window::new(tr(lang, "Export images"))
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("export-grid")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label(tr(lang, "Folder"));
                        ui.label(&dialog.root_label);
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut dialog.recursive, tr(lang, "Include subfolders"));
                        ui.end_row();
                        ui.label(tr(lang, "Images shown"));
                        ui.label(format!("{}: {filter_text}", tr(lang, "Filter")));
                        ui.end_row();
                    });
                ui.separator();
                for (kind, title) in [
                    (ExportKind::Copy, "Copy files to a folder"),
                    (ExportKind::List(ListFormat::Csv), "CSV list"),
                    (ExportKind::List(ListFormat::Jsonl), "JSONL list"),
                ] {
                    if ui
                        .radio_value(&mut dialog.kind, kind, tr(lang, title))
                        .changed()
                    {
                        let ext = match kind {
                            ExportKind::List(ListFormat::Jsonl) => "jsonl",
                            _ => "csv",
                        };
                        dialog.list_path = Path::new(&dialog.list_path)
                            .with_extension(ext)
                            .display()
                            .to_string();
                    }
                }
                // ファイル選択ダイアログを使わず文字で入力させるのは、Linux で OS のダイアログを
                // 出すには GTK などへの依存が増え、起動の速さと配布の手軽さを損なうため。
                match dialog.kind {
                    ExportKind::Copy => {
                        ui.label(tr(lang, "Destination folder"));
                        ui.add(egui::TextEdit::singleline(&mut dialog.dest).desired_width(420.0));
                        ui.checkbox(
                            &mut dialog.keep_structure,
                            tr(lang, "Keep folder structure"),
                        );
                    }
                    ExportKind::List(_) => {
                        ui.label(tr(lang, "Output file"));
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.list_path).desired_width(420.0),
                        );
                    }
                }
                ui.separator();
                match &dialog.job {
                    Some(job) if !job.finished.load(std::sync::atomic::Ordering::Relaxed) => {
                        let done = job.done.load(std::sync::atomic::Ordering::Relaxed);
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / job.total.max(1) as f32)
                                    .text(format!("{done} / {}", job.total))
                                    .desired_width(300.0),
                            );
                            cancel_job = ui.button(tr(lang, "Cancel")).clicked();
                        });
                    }
                    _ => {
                        run = ui.button(tr(lang, "Export")).clicked();
                    }
                }
            });
        if cancel_job {
            if let Some(job) = &dialog.job {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if !open {
            if let Some(job) = &dialog.job {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.export = None;
            return;
        }
        if run {
            self.run_export(ctx);
        }
        self.finish_copy_job(ctx);
    }

    /// 書き出しを始める。一覧はその場で書き、コピーは別スレッドで行う。
    pub(super) fn run_export(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.export else {
            return;
        };
        let order = self.settings().tree.order();
        let files = export::collect(
            &dialog.root,
            dialog.recursive,
            self.loader.registry(),
            order,
            &self.matcher(),
        );
        let dialog = self.export.as_mut().expect("checked above");
        match dialog.kind {
            ExportKind::List(format) => {
                if !files.archives.is_empty() {
                    self.log.warn(format!(
                        "Export: the list leaves out {} ZIP / 7z files (open them to list their images)",
                        files.archives.len()
                    ));
                }
                let items: Vec<_> = files
                    .images
                    .into_iter()
                    .map(|f| {
                        let label = self.labels.get(&f);
                        (f, label)
                    })
                    .collect();
                let out = PathBuf::from(dialog.list_path.trim());
                match export::write_list(&out, format, &items) {
                    Ok(()) => self.log.done(
                        "Exported",
                        format!("{} images to {}", items.len(), out.display()),
                    ),
                    Err(e) => self.log.error(format!("Export: {e}")),
                }
            }
            ExportKind::Copy => {
                let dest = PathBuf::from(dialog.dest.trim());
                let mut taken = HashSet::new();
                let mut items: Vec<_> = files
                    .images
                    .into_iter()
                    .map(|f| {
                        let to = export::copy_target(
                            &dialog.root,
                            &f,
                            &dest,
                            dialog.keep_structure,
                            &mut taken,
                        );
                        export::Copy::File { from: f, to }
                    })
                    .collect();
                let images = items.len();
                let archives = if self.filter.is_all() {
                    files.archives.len()
                } else {
                    0
                };
                if self.filter.is_all() {
                    // ファイルを先に写す。ZIP の中身を直下に置くときの番号付けで、
                    // 先に決めておいたファイルの名前を横取りしないため。
                    items.extend(files.archives.into_iter().map(|zip| {
                        let (dir, flat) = match zip.strip_prefix(&dialog.root) {
                            Ok(rel) if dialog.keep_structure => (dest.join(rel), false),
                            _ => (dest.clone(), true),
                        };
                        export::Copy::Archive {
                            archive: zip,
                            dir,
                            flat,
                        }
                    }));
                } else if !files.archives.is_empty() {
                    self.log.warn(format!(
                        "Export: skipped {} ZIP / 7z files (images in them have no labels)",
                        files.archives.len()
                    ));
                }
                self.log.info(format!(
                    "Export: copying {images} images and {archives} ZIP / 7z files to {}",
                    dest.display()
                ));
                let ctx = ctx.clone();
                dialog.job = Some(CopyJob::start(
                    items,
                    self.loader.registry().clone(),
                    move || ctx.request_repaint(),
                ));
            }
        }
    }

    /// 送信先の名前（右クリックの「送信 ▸」に並べる）。
    pub(super) fn destination_names(&self) -> Vec<String> {
        self.settings
            .settings
            .dicom_send
            .destinations
            .iter()
            .map(|d| d.name.clone())
            .collect()
    }
}
