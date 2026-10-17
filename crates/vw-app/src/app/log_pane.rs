//! ログ欄と状態行。

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_phosphor::regular as icon;

use crate::copy::Focus;
use crate::i18n::tr;
use crate::settings::LogStyle;

use super::*;

impl VwApp {
    /// ログ欄の見出しの右に、折り返し・見た目の切り替えとログファイルを開くボタンを出す。
    pub(super) fn log_header(&self, ui: &mut egui::Ui) -> Option<LogAction> {
        let lang = self.lang();
        let mut action = None;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let has_file = self.log.file_path().is_some();
            if ui
                .add_enabled(has_file, egui::Button::new(icon::FILE_TEXT).small())
                .on_hover_text(tr(lang, "Open the log file"))
                .on_disabled_hover_text(tr(
                    lang,
                    "The log is not written to a file (Settings > Log).",
                ))
                .clicked()
            {
                action = Some(LogAction::OpenFile);
            }
            let wrap = self.settings().log.wrap;
            if ui
                .add(
                    egui::Button::new(icon::TEXT_ALIGN_LEFT)
                        .small()
                        .selected(wrap),
                )
                .on_hover_text(tr(
                    lang,
                    if wrap {
                        "Wrap long lines: on"
                    } else {
                        "Wrap long lines: off"
                    },
                ))
                .clicked()
            {
                action = Some(LogAction::Wrap(!wrap));
            }
            let style = self.settings().log.style;
            for choice in LogStyle::ALL.into_iter().rev() {
                if ui
                    .selectable_label(style == choice, tr(lang, choice.title()))
                    .clicked()
                    && style != choice
                {
                    action = Some(LogAction::Style(choice));
                }
            }
        });
        action
    }

    /// ログの上の状態の行。動いているものがあれば回るしるしのために細かく描き直し、
    /// 何もなくても波形が流れるよう 1 秒ごとに描き直す。
    pub(super) fn status_lines(&self, ctx: &egui::Context) -> Vec<Vec<crate::status::Segment>> {
        use crate::status::{self, Snapshot, Words};
        let lang = self.lang();
        let now = Instant::now();
        let counts = self.loader.counts();
        let jobs: Vec<status::Job> = self
            .jobs()
            .into_iter()
            .map(|(name, done, total)| status::Job {
                name: tr(lang, name),
                done,
                total,
            })
            .collect();
        let busy = counts.waiting + counts.running > 0 || !jobs.is_empty();
        ctx.request_repaint_after(Duration::from_millis(if busy { 125 } else { 1000 }));
        let snapshot = Snapshot {
            waiting: counts.waiting,
            running: counts.running,
            workers: counts.workers,
            sparkline: self.throughput.sparkline(now),
            rate: self.throughput.rate(now),
            resident: self.full_states().iter().filter_map(|f| f.resident).sum(),
            limit: self.settings().viewer.full_image_limit_bytes(),
            jobs,
            spinner: status::spinner(now.duration_since(self.started).as_secs_f64()),
            files: self.load_stats.files,
            failed: self.load_stats.failed,
            averages: self.load_stats.averages(),
            last: self.load_stats.last.clone(),
            last_failed: self.load_stats.last_failed.clone(),
        };
        let words = Words {
            load: tr(lang, "Loading"),
            waiting: tr(lang, "waiting"),
            per_second: tr(lang, "files/s"),
            full: tr(lang, "Full-res"),
            idle: tr(lang, "Idle"),
        };
        status::lines(&snapshot, &words)
    }

    pub(super) fn log_action(&mut self, ctx: &egui::Context, action: LogAction) {
        match action {
            LogAction::Style(style) => {
                self.settings.settings.log.style = style;
                self.settings_changed(ctx);
            }
            LogAction::Wrap(wrap) => {
                self.settings.settings.log.wrap = wrap;
                self.settings_changed(ctx);
            }
            LogAction::OpenFile => {
                if let Some(path) = self.log.file_path().map(Path::to_path_buf) {
                    if let Err(e) = crate::launch::open_file(&path) {
                        self.log.error(format!("{}: {e}", path.display()));
                    }
                }
            }
            LogAction::OpenFolder => {
                if let Some(dir) = self
                    .log
                    .file_path()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                {
                    if let Err(e) = crate::launch::open_folder(&dir) {
                        self.log.error(format!("{}: {e}", dir.display()));
                    }
                }
            }
            LogAction::CopyAll => {
                let text = self.log.all_text();
                self.copy_text(ctx, text, "log");
            }
        }
    }

    /// ログを新しいものが下に来る順で出し、末尾に自動で追従する。
    /// 文字はドラッグで行をまたいで範囲選択でき、Ctrl+C でコピーする（コピーは egui のラベル選択に任せる）。
    ///
    /// 行ごとに別のラベルにして 1 つの文字列にまとめないのは、行ごとに色を変えるのと、
    /// 新しい行が増えても選んだ範囲が行のラベルに付いたまま動かないようにするため。
    pub(super) fn log_ui(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let terminal = self.settings().log.style == LogStyle::Terminal;
        let wrap = self.settings().log.wrap;
        let mut action = None;
        let mut selecting = false;
        let frame = if terminal {
            egui::Frame::new()
                .fill(TERMINAL_BACKGROUND)
                .inner_margin(egui::Margin::symmetric(4, 2))
        } else {
            egui::Frame::new()
        };
        // 状態の行はどちらの見た目でも出す。1 枚ごとの読み込みはログ欄に出さないので、
        // 読み込みの進み具合はここでしか見えないため。
        let status = self.status_lines(ui.ctx());
        let mut menu_target: Option<egui::Response> = None;
        frame.show(ui, |ui| {
            let visuals = ui.visuals().clone();
            for line in &status {
                // 折り返さないときは状態の行を切り詰める。ログと違って横に送る場所がないため。
                let label =
                    egui::Label::new(status_job(line, terminal, &visuals)).selectable(false);
                ui.add(if wrap { label.wrap() } else { label.truncate() });
            }
            ui.add_space(2.0);
            if !terminal {
                ui.separator();
            }
            let scroll = if wrap {
                egui::ScrollArea::vertical()
            } else {
                egui::ScrollArea::both()
            };
            scroll
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.set_min_width(ui.available_width());
                    for (i, line) in self.log.lines.iter().enumerate() {
                        let label = if terminal {
                            terminal_line(line, wrap)
                        } else {
                            normal_line(ui, line, wrap)
                        };
                        let r = ui.push_id(i, |ui| ui.add(label)).inner;
                        selecting |= r.drag_started();
                        menu_target = Some(match menu_target.take() {
                            Some(m) => m.union(r),
                            None => r,
                        });
                    }
                    let rest = ui.allocate_response(ui.available_size(), egui::Sense::click());
                    menu_target = Some(match menu_target.take() {
                        Some(m) => m.union(rest),
                        None => rest,
                    });
                })
        });
        let Some(menu_target) = menu_target else {
            return;
        };
        menu_target.context_menu(|ui| {
            if ui.button(tr(lang, "Copy all")).clicked() {
                action = Some(LogAction::CopyAll);
                ui.close();
            }
            let has_file = self.log.file_path().is_some();
            if ui
                .add_enabled(has_file, egui::Button::new(tr(lang, "Open the log file")))
                .clicked()
            {
                action = Some(LogAction::OpenFile);
                ui.close();
            }
            if ui
                .add_enabled(has_file, egui::Button::new(tr(lang, "Open the log folder")))
                .clicked()
            {
                action = Some(LogAction::OpenFolder);
                ui.close();
            }
        });
        let ctx = ui.ctx().clone();
        if selecting {
            self.set_focus(&ctx, Focus::Log);
        }
        if let Some(a) = action {
            self.log_action(&ctx, a);
        }
    }
}
