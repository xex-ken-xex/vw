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
    /// ログ欄の見出しの右に、折り返し・見た目の切り替えとログファイル・フォルダを開くボタンと、
    /// 裏で進めている処理（くるくると名前・割合）を出す。
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
            if ui
                .add_enabled(has_file, egui::Button::new(icon::FOLDER_OPEN).small())
                .on_hover_text(tr(lang, "Open the log folder"))
                .on_disabled_hover_text(tr(
                    lang,
                    "The log is not written to a file (Settings > Log).",
                ))
                .clicked()
            {
                action = Some(LogAction::OpenFolder);
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
            // ターミナル風では状態の行に出ているので、普通の見た目のときだけ。
            if style == LogStyle::Normal {
                if let Some(summary) = self.activity_summary() {
                    ui.add_space(8.0);
                    ui.weak(summary);
                    ui.spinner();
                }
            }
        });
        action
    }

    /// ログの下の状態の行。動いているものがあれば回るしるしのために細かく描き直し、
    /// 何もなくても波形が流れるよう 1 秒ごとに描き直す。
    pub(super) fn status_lines(&mut self, ctx: &egui::Context) -> Vec<Vec<crate::status::Segment>> {
        use crate::status::{self, Snapshot, Words};
        let lang = self.lang();
        let now = Instant::now();
        let counts = self.loader.counts();
        let tasks = self.tasks();
        let tally = self.tally.update(&tasks);
        let busy = counts.waiting + counts.running > 0 || !tasks.is_empty();
        ctx.request_repaint_after(Duration::from_millis(if busy { 125 } else { 1000 }));
        let snapshot = Snapshot {
            waiting: counts.waiting,
            running: counts.running,
            workers: counts.workers,
            sparkline: self.throughput.sparkline(now),
            rate: self.throughput.rate(now),
            resident: self.full_states().iter().filter_map(|f| f.resident).sum(),
            limit: self.settings().viewer.full_image_limit_bytes(),
            tasks,
            tally,
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
            working: tr(lang, "Working"),
            more: tr(lang, "more"),
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
                    if let Err(e) = vw_launch::open_file(&path) {
                        self.log.error(format!("{}: {e}", path.display()));
                    }
                }
            }
            LogAction::OpenFolder => self.open_log_folder(),
            LogAction::CopyAll => {
                let text = self.log.all_text();
                self.copy_text(ctx, text, "log");
            }
        }
    }

    /// 今書いているログのファイルのフォルダを開く。ファイルに書いていないときは、
    /// 設定ファイルの隣の logs フォルダ（以前の起動のログがあるところ）を開く。
    pub(super) fn open_log_folder(&mut self) {
        let dir = self
            .log
            .file_path()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.settings.path.as_deref().map(crate::log::log_dir));
        let Some(dir) = dir else {
            return;
        };
        if let Err(e) = vw_launch::open_folder(&dir) {
            self.log.error(format!("{}: {e}", dir.display()));
        }
    }

    /// 同梱のヘルプページを、表示の言語で開く。書き出し先は設定のフォルダの help
    /// （設定のフォルダがなければ一時フォルダ）。
    pub(super) fn open_help(&mut self, page: crate::help::Page) {
        let dir = self
            .settings
            .path
            .as_deref()
            .and_then(Path::parent)
            .map_or_else(|| std::env::temp_dir().join("vw-help"), |d| d.join("help"));
        let opened = crate::help::write_page(&dir, page, self.lang())
            .and_then(|path| vw_launch::open_file(&path).map(|()| path));
        match opened {
            Ok(path) => self.log.info(format!("Help: opened {}", path.display())),
            Err(e) => self.log.error(format!("Help: {}: {e}", dir.display())),
        }
    }

    /// ログ欄の中身。文字はドラッグで行をまたいで範囲選択でき、Ctrl+C でコピーする
    /// （コピーは egui のラベル選択に任せる）。
    pub(super) fn log_ui(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let terminal = self.settings().log.style == LogStyle::Terminal;
        let wrap = self.settings().log.wrap;
        let mut action = None;
        // 集計の行はターミナル風だけに出す。普通の見た目はログファイルの中身と同じ行だけにして、
        // 欄の下の数行が絶えず書き換わって目に入らないようにする（読み込み中かはスロットで分かる）。
        let status = if terminal {
            self.status_lines(ui.ctx())
        } else {
            Vec::new()
        };
        let vw_log::view::Shown {
            menu_target,
            selecting,
        } = vw_log::view::show(ui, &self.log, &status, terminal, wrap);
        let Some(menu_target) = menu_target else {
            return;
        };
        vw_ui::menu::context(&menu_target, |ui| {
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
