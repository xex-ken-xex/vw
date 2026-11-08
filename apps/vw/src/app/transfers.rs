//! ドラッグ＆ドロップ、移動・コピー、ごみ箱への移動と、その確認ダイアログ。

use std::path::{Path, PathBuf};

use eframe::egui::{self, vec2};
use egui_phosphor::regular as icon;

use crate::archive::{self};
use crate::discard::{self, TrashJob};
use crate::export::{self, Filter, Matcher};
use crate::i18n::{tr, tr1};
use crate::labels::{Flag, Label};
use crate::transfer::{Job as TransferJob, Op, Transfer};
use crate::viewer::DraggedFile;

use super::*;

impl VwApp {
    /// 読み込み中のフォルダの確認（サブフォルダも読むか、次の区切りまで続けるか）。
    /// スロットからドラッグした画像 `file` がフォルダ `dir` にドロップされた。Ctrl を押していれば
    /// コピーしてすぐ終える。移動は確認してから行う。
    pub(super) fn drop_file(&mut self, ctx: &egui::Context, file: PathBuf, dir: PathBuf) {
        // ドロップした瞬間の Ctrl だけで決めないのは、ボタンと Ctrl をほぼ同時に離すと
        // Ctrl を先に離したことになり、コピーと見せていたものが移動になるため。
        self.drop_taken = true;
        let copy = self.drag_copy || ctx.input(|i| i.modifiers.command);
        let op = if copy { Op::Copy } else { Op::Move };
        let t = Transfer {
            file,
            to_dir: dir,
            op,
        };
        if t.is_noop() {
            self.log.info(format!(
                "Not moved: {} is already in {}",
                t.file.display(),
                t.to_dir.display()
            ));
            return;
        }
        // 展開した ZIP は一時フォルダの写しなので、中へ入れても ZIP には入らず、中から移しても
        // ZIP からは消えない。コピーで取り出すことだけ許す。
        let into_zip = self.archives.mount_of(&t.to_dir).is_some();
        let out_of_zip = op == Op::Move && self.archives.mount_of(&t.file).is_some();
        if into_zip || out_of_zip {
            self.log.warn(format!(
                "{}: files in a ZIP are read-only (Ctrl+drag copies them out)",
                t.file.display()
            ));
            return;
        }
        match op {
            Op::Copy => self.run_transfer(ctx, t),
            Op::Move => {
                self.move_at = ctx.pointer_latest_pos().map(|at| (ctx.viewport_id(), at));
                self.pending_move = Some(t);
            }
        }
    }

    /// 移動・コピーを裏で始める。数 GB のファイルを写す間も画面を止めないため。
    pub(super) fn run_transfer(&mut self, ctx: &egui::Context, t: Transfer) {
        let ctx = ctx.clone();
        self.transfer_jobs
            .push(TransferJob::start(t, move || ctx.request_repaint()));
    }

    /// 移し終えたもののラベルを移し、結果をログに出す。
    pub(super) fn finish_transfer_jobs(&mut self) {
        let mut i = 0;
        while i < self.transfer_jobs.len() {
            let Some(result) = self.transfer_jobs[i].poll() else {
                i += 1;
                continue;
            };
            let t = self.transfer_jobs.remove(i).transfer;
            let verb = match t.op {
                Op::Move => "Moved",
                Op::Copy => "Copied",
            };
            match result {
                Ok(dest) => {
                    self.log
                        .done(verb, format!("{} to {}", t.file.display(), dest.display()));
                    if let Some(e) = t.carry_labels(&self.labels, &dest) {
                        self.log.error(format!("Labels were not carried over: {e}"));
                    }
                }
                Err(e) => self.log.error(format!("Cannot move or copy: {e}")),
            }
        }
    }

    /// 今いるフォルダの悪いフラグの画像を集め、ごみ箱へ送ってよいか尋ねる。
    /// サブフォルダまで辿らないのは、見えていない画像まで消してしまわないため。
    pub(super) fn ask_trash_bad(&mut self) {
        let lang = self.lang();
        let Some(dir) = self.current.clone() else {
            self.log
                .warn(tr(lang, "Select a folder in the tree first."));
            return;
        };
        if self.archives.mount_of(&dir).is_some() {
            self.log
                .warn(format!("{}: files in a ZIP are read-only", dir.display()));
            return;
        }
        let bad = Filter::Flag(Some(Flag::Bad));
        let matcher = Matcher {
            filter: &bad,
            labels: &self.labels,
        };
        let order = self.settings().tree.order();
        let files = export::collect(&dir, false, self.loader.registry(), order, &matcher).images;
        if files.is_empty() {
            self.log.info(format!("No bad images in {}", dir.display()));
            return;
        }
        self.pending_trash = Some(discard::Pending { dir, files });
        self.focus_trash_cancel = true;
    }

    pub(super) fn trash_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending_trash else {
            return;
        };
        let lang = self.lang();
        let (names, rest) = pending.preview(10);
        let mut answer = None;
        let modal = vw_ui::dialog::Dialog::new("trash_question")
            .tone(vw_ui::dialog::Tone::Caution)
            .max_width(480.0)
            .show(ctx, |ui| {
                ui.label(tr1(
                    lang,
                    "Move {} bad images to the trash?",
                    &pending.files.len().to_string(),
                ));
                ui.label(egui::RichText::new(pending.dir.display().to_string()).monospace());
                ui.add_space(4.0);
                for name in names {
                    ui.label(name);
                }
                if rest > 0 {
                    ui.label(tr1(lang, "...and {} more", &rest.to_string()));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    // 取り消しを先に置き、Enter で押されるようにする。うっかり Enter で送らないため。
                    let cancel = ui.button(tr(lang, "Cancel"));
                    if std::mem::take(&mut self.focus_trash_cancel) {
                        cancel.request_focus();
                    }
                    if cancel.clicked() {
                        answer = Some(false);
                    }
                    if ui.button(tr(lang, "Move to trash")).clicked() {
                        answer = Some(true);
                    }
                });
            });
        if modal.should_close && answer.is_none() {
            answer = Some(false);
        }
        let Some(yes) = answer else {
            return;
        };
        let Some(pending) = self.pending_trash.take() else {
            return;
        };
        if !yes {
            self.log.info(format!(
                "Did not move the bad images in {} to the trash",
                pending.dir.display()
            ));
            return;
        }
        let ctx = ctx.clone();
        self.trash_job = Some(TrashJob::start(pending.files, move || {
            ctx.request_repaint()
        }));
    }

    /// ごみ箱へ送り終えた画像のラベルを消し、結果をログに出す。スロットは
    /// フォルダの監視が消えた画像として閉じる。
    pub(super) fn finish_trash_job(&mut self) {
        let Some(outcomes) = self.trash_job.as_ref().and_then(TrashJob::poll) else {
            return;
        };
        self.trash_job = None;
        let mut moved = Vec::new();
        for (file, result) in outcomes {
            match result {
                Ok(()) => {
                    // 消す前のラベルを取っておき、元に戻したときに付け直す。
                    let label = self.labels.get(&file);
                    if let Err(e) = self.labels.set(&file, Label::default()) {
                        self.log.error(format!("{}: {e}", file.display()));
                    }
                    moved.push((file, label));
                }
                Err(e) => self
                    .log
                    .error(format!("Cannot move {} to the trash: {e}", file.display())),
            }
        }
        let count = moved.len();
        if discard::CAN_RESTORE && !moved.is_empty() {
            self.remember_undo(super::undo::Undo::Trashed(moved));
            self.log.info(format!(
                "Moved {count} bad images to the trash (Ctrl+Z to undo)"
            ));
        } else {
            self.log
                .info(format!("Moved {count} bad images to the trash"));
        }
    }

    /// ごみ箱から戻し終えた画像にラベルを付け直し、結果をログに出す。
    pub(super) fn finish_restore_job(&mut self) {
        let Some((outcomes, labels)) = self.restore_job.as_mut().and_then(|job| {
            let outcomes = job.job.poll()?;
            Some((outcomes, std::mem::take(&mut job.labels)))
        }) else {
            return;
        };
        self.restore_job = None;
        let mut restored = 0;
        for (file, result) in outcomes {
            match result {
                Ok(()) => {
                    restored += 1;
                    let label = labels
                        .iter()
                        .find(|(f, _)| *f == file)
                        .map(|(_, l)| l.clone())
                        .unwrap_or_default();
                    if let Err(e) = self.labels.set(&file, label) {
                        self.log.error(format!("{}: {e}", file.display()));
                    }
                }
                Err(e) => self.log.error(format!(
                    "Cannot restore {} from the trash: {e}",
                    file.display()
                )),
            }
        }
        self.log
            .info(format!("Restored {restored} images from the trash"));
        self.apply_filter_to_slots();
    }

    /// ドロップした画像を移してよいか尋ねる。
    pub(super) fn move_dialog(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.pending_move else {
            return;
        };
        let lang = self.lang();
        let name = t.file.file_name().unwrap_or_default().to_string_lossy();
        let message = tr1(lang, "Move \"{}\" to", &name);
        let mut answer = None;
        let mut dialog = vw_ui::dialog::Dialog::new("move_question")
            .tone(vw_ui::dialog::Tone::Caution)
            .max_width(480.0);
        // 落とした所の近くに出す。中央だと、落とした所を見ている目に入らず、何も起きなかったように見える。
        if let Some((viewport, at)) = self.move_at {
            dialog = dialog.near(viewport, at);
        }
        let modal = dialog.show(ctx, |ui| {
            ui.label(message);
            ui.label(egui::RichText::new(t.to_dir.display().to_string()).monospace());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr(lang, "Move")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr(lang, "Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close && answer.is_none() {
            answer = Some(false);
        }
        let Some(yes) = answer else {
            return;
        };
        let Some(t) = self.pending_move.take() else {
            return;
        };
        if yes {
            self.run_transfer(ctx, t);
        } else {
            self.log.info(format!("Did not move {}", t.file.display()));
        }
    }

    /// ドラッグ中の画像の名前と、移動かコピーかをマウスの横に出す。
    pub(super) fn drag_preview(&mut self, ctx: &egui::Context) {
        let taken = std::mem::take(&mut self.drop_taken);
        let Some(file) = egui::DragAndDrop::payload::<DraggedFile>(ctx) else {
            self.drag_copy = false;
            self.reorder_hover = false;
            return;
        };
        if !taken && ctx.input(|i| i.pointer.any_released()) {
            self.log.info(format!(
                "Not moved: drop {} on a folder in the tree or a folder slot",
                file.0.file_name().unwrap_or_default().to_string_lossy()
            ));
            return;
        }
        self.drag_copy = ctx.input(|i| i.modifiers.command);
        let Some(pointer) = ctx.pointer_latest_pos() else {
            return;
        };
        let lang = self.lang();
        // 地図の上では地図が印と説明を出す。ここで出すと、メインのウインドウの端に押し込まれる。
        if self.map.drop_hover.is_some() {
            return;
        }
        let (icon, verb) = if self.reorder_hover {
            (icon::SQUARES_FOUR, tr(lang, "Place here"))
        } else if self.drag_copy {
            (icon::COPY, tr(lang, "Copy"))
        } else {
            (icon::ARROW_RIGHT, tr(lang, "Move"))
        };
        let name = file.0.file_name().unwrap_or_default().to_string_lossy();
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        egui::Area::new(egui::Id::unique("drag_preview"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pointer + vec2(14.0, 14.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(format!("{icon} {verb}: {name}"));
                });
            });
    }

    /// エクスプローラなどからドロップされたパスへ移る。ファイルならそのフォルダへ移って、その画像を選ぶ。
    /// ZIP は展開してその中へ移る。複数ドロップされたときは先頭だけを使う。
    pub(super) fn receive_os_drop(&mut self, ctx: &egui::Context) {
        let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()))
        else {
            return;
        };
        if path.is_dir() || archive::is_archive(&path) {
            self.open_folder(ctx, path);
            return;
        }
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        self.open_folder(ctx, dir);
        self.select_when_loaded = Some(path);
    }

    pub(super) fn select_dropped_image(&mut self) {
        let Some(path) = &self.select_when_loaded else {
            return;
        };
        if let Some(i) = self
            .slots
            .iter()
            .position(|s| s.path.as_ref() == Some(path))
        {
            self.active = i;
            self.select_when_loaded = None;
            if self.slots[i].loading {
                self.loader.prioritize(self.slots[i].request);
            }
        } else if self.folder_load.is_none() {
            // 読み込みを終えても出てこない（対応していない形式など）。
            self.select_when_loaded = None;
        }
    }

    /// 終わったコピーの結果をログに出す。
    pub(super) fn finish_copy_job(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.export else {
            return;
        };
        let Some(job) = &dialog.job else {
            return;
        };
        if !job.finished.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let failed = std::mem::take(&mut *job.failed.lock().unwrap_or_else(|e| e.into_inner()));
        let copied = job.copied.load(std::sync::atomic::Ordering::Relaxed);
        let located = job.located.load(std::sync::atomic::Ordering::Relaxed);
        let strip = dialog.strip_location;
        dialog.job = None;
        // ログの欄はこの画面より先に描いているので、結果を出すにはもう一度描く。
        ctx.request_repaint();
        self.log.done(
            "Exported",
            format!("{copied} images ({} not copied)", failed.len()),
        );
        if strip {
            self.log.info(format!(
                "Export: removed the location from {located} images"
            ));
        }
        for e in failed {
            self.log.error(format!("Export: {e}"));
        }
    }
}
