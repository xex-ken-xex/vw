//! 裏で走っている仕事の一覧と、画面下の活動ラインとトースト。

use std::time::{Duration, Instant};

use eframe::egui::{self, vec2};

use crate::i18n::{tr, tr1};
use crate::status::Task;
use crate::theme;

use super::*;

impl VwApp {
    /// 進めている処理 1 件ずつと、その進み具合。ZIP の展開・ドラッグでの移動やコピー・データセットの
    /// 探し出し・読み込みや取り出し・書き出し・送信・AI・AI のモデルのダウンロード・RAW の現像。
    pub(super) fn tasks(&self) -> Vec<Task> {
        let lang = self.lang();
        let task = |kind: &'static str, name: String, (done, total): (usize, usize), bytes| Task {
            kind: tr(lang, kind).to_owned(),
            name,
            done,
            total,
            bytes,
            note: String::new(),
        };
        let mut tasks = Vec::new();
        for (root, folders, found, _) in self.datasets.progress().into_iter().filter(|p| !p.3) {
            tasks.push(Task {
                note: format!(
                    "{}, {}",
                    tr1(lang, "{} folders", &thousands(folders)),
                    tr1(lang, "{} datasets", &thousands(found))
                ),
                ..task("Looking for datasets", file_name(root), (0, 0), false)
            });
        }
        for (name, done, total) in self.archives.progress() {
            tasks.push(task("Extracting ZIP", name, (done, total), true));
        }
        for job in &self.transfer_jobs {
            let kind = match job.transfer.op {
                crate::transfer::Op::Move => "Moving",
                crate::transfer::Op::Copy => "Copying",
            };
            tasks.push(task(
                kind,
                file_name(&job.transfer.file),
                job.progress(),
                true,
            ));
        }
        for (name, done, total) in self.ai_models.progress() {
            tasks.push(task("Downloading", name.to_owned(), (done, total), true));
        }
        if let Some(d) = &self.dataset_dialog
            && let Some(p) = d.progress() {
                tasks.push(task("Dataset job", file_name(d.dir()), p, false));
            }
        if let Some(job) = self.export.as_ref().and_then(|d| d.job.as_ref()) {
            let done = job.done.load(std::sync::atomic::Ordering::Relaxed);
            tasks.push(task("Exporting", String::new(), (done, job.total), false));
        }
        if let Some(job) = &self.send_job
            && let Some(p) = job.progress() {
                tasks.push(task("Sending", job.destination.name.clone(), p, false));
            }
        if let Some(job) = &self.ai_job {
            tasks.push(task(
                self.ai_job_name(),
                String::new(),
                job.progress(),
                false,
            ));
        }
        for shown in self.slots.iter().filter_map(|s| s.shown.as_ref()) {
            if shown.developing() {
                tasks.push(task(
                    "Developing RAW",
                    file_name(&shown.path),
                    (0, 0),
                    false,
                ));
            }
        }
        tasks
    }

    /// 進めている処理の進み具合。何もしていなければ `None`。
    pub(super) fn activity(&self) -> Option<Activity> {
        let tasks = self.tasks();
        if tasks.is_empty() {
            return None;
        }
        if tasks.iter().any(|t| t.total == 0) {
            return Some(Activity::Unknown);
        }
        // 一番遅れているものに合わせる。合計で出すと、終わりかけのものに隠れて
        // 長い処理がもうすぐ終わるように見えてしまうため。
        tasks
            .iter()
            .map(|t| (t.done as f32 / t.total as f32).clamp(0.0, 1.0))
            .reduce(f32::min)
            .map(Activity::Fraction)
    }

    /// ウィンドウの一番下の細い線。処理中は灰色の上を赤い線が右へ伸び、普段は背景と同じ色で見えない。
    ///
    /// 状態バーや進み具合のダイアログにしないのは、ZIP の展開は裏で頻繁に動くので、
    /// そのたびに画面の配置が変わったり作業を遮ったりしないようにするため。
    pub(super) fn activity_line(&self, ui: &mut egui::Ui) {
        let activity = self.activity();
        egui::Panel::bottom("activity")
            .frame(egui::Frame::NONE)
            .exact_size(ACTIVITY_LINE)
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| {
                let rect = ui.max_rect();
                let painter = ui.painter();
                let Some(activity) = activity else {
                    painter.rect_filled(rect, 0.0, ui.visuals().panel_fill);
                    return;
                };
                painter.rect_filled(rect, 0.0, theme::ACTIVITY_TRACK);
                let (from, to) = match activity {
                    Activity::Fraction(f) => (0.0, f),
                    Activity::Unknown => {
                        let t = ui.input(|i| i.time) as f32;
                        sweep(t)
                    }
                };
                let x = |f: f32| rect.left() + rect.width() * f;
                let done = egui::Rect::from_x_y_ranges(x(from)..=x(to), rect.y_range());
                painter.rect_filled(done, 0.0, theme::ACTIVITY);
            });
        if activity.is_some() {
            // 展開は終わるまで知らせてこないので、線を進めるために描き直す。
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    /// 進み具合の短い言い方（「ZIP の展開 37%」）。いくつもあれば最初のものに数を添える。
    pub(super) fn activity_summary(&self) -> Option<String> {
        let tasks = self.tasks();
        let first = tasks.first()?;
        let mut text = first.kind.clone();
        if first.total > 0 {
            let percent = 100.0 * first.done as f32 / first.total as f32;
            text.push_str(&format!(" {percent:.0}%"));
        }
        if tasks.len() > 1 {
            text.push_str(&format!(" (+{})", tasks.len() - 1));
        }
        Some(text)
    }

    pub(super) fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some(toast) = &self.toast else {
            return;
        };
        let now = Instant::now();
        if now >= toast.until {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::new("copied_toast"))
            .order(egui::Order::Tooltip)
            .fixed_pos(toast.pos + vec2(12.0, 12.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(tr(self.lang(), "Copied"));
                });
            });
        ctx.request_repaint_after(toast.until - now);
    }
}

/// 処理の行に出すファイルやフォルダの名前。
fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// 下の線の進み具合。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Activity {
    /// 済んだ割合（0〜1）。
    Fraction(f32),
    /// 全部の量がまだ分からない（ZIP の目次を読んでいるなど）。
    Unknown,
}

/// 全部の量が分からない間、線の上を往復する短い区間（左端, 右端、0〜1）。`t` は秒。
fn sweep(t: f32) -> (f32, f32) {
    const WIDTH: f32 = 0.2;
    const PERIOD: f32 = 2.0;
    let phase = (t / PERIOD).fract() * 2.0;
    let travel = if phase < 1.0 { phase } else { 2.0 - phase };
    let from = travel * (1.0 - WIDTH);
    (from, from + WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_progress_sweeps_a_short_segment_back_and_forth() {
        assert_eq!(sweep(0.0), (0.0, 0.2));
        let (from, to) = sweep(1.0);
        assert!((from - 0.8).abs() < 1e-6 && (to - 1.0).abs() < 1e-6);
        assert_eq!(sweep(2.0), (0.0, 0.2));
    }
}
