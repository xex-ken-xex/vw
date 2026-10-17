//! 裏で走っている仕事の一覧と、画面下の活動ラインとトースト。

use std::time::{Duration, Instant};

use eframe::egui::{self, vec2};

use crate::i18n::tr;
use crate::status::Job;
use crate::theme;

use super::*;

impl VwApp {
    /// 進めている処理と、その進み具合。ZIP の展開・ドラッグでの移動やコピー・データセットの
    /// 読み込みや取り出し・書き出し・送信・AI。名前は訳す前のもの。
    pub(super) fn jobs(&self) -> Vec<Job> {
        let transfers = (!self.transfer_jobs.is_empty()).then(|| {
            self.transfer_jobs
                .iter()
                .map(|j| j.progress())
                .fold((0, 0), |(d, t), (done, total)| (d + done, t + total))
        });
        let in_bytes = [
            ("Extracting ZIP", self.archives.progress()),
            ("Moving or copying", transfers),
        ];
        let in_items = [
            (
                "Dataset job",
                self.dataset_dialog.as_ref().and_then(|d| d.progress()),
            ),
            (
                "Exporting",
                self.export
                    .as_ref()
                    .and_then(|d| d.job.as_ref())
                    .map(|j| (j.done.load(std::sync::atomic::Ordering::Relaxed), j.total)),
            ),
            ("Sending", self.send_job.as_ref().and_then(|j| j.progress())),
            (
                self.ai_job_name(),
                self.ai_job.as_ref().map(|j| j.progress()),
            ),
            ("Downloading AI models", self.ai_models.progress()),
        ];
        let job = |bytes| {
            move |(name, p): (&'static str, Option<(usize, usize)>)| {
                p.map(|(done, total)| Job {
                    name,
                    done,
                    total,
                    bytes,
                })
            }
        };
        in_bytes
            .into_iter()
            .filter_map(job(true))
            .chain(in_items.into_iter().filter_map(job(false)))
            .collect()
    }

    /// 進めている処理の進み具合。何もしていなければ `None`。
    pub(super) fn activity(&self) -> Option<Activity> {
        let jobs = self.jobs();
        if jobs.is_empty() {
            return None;
        }
        if jobs.iter().any(|j| j.total == 0) {
            return Some(Activity::Unknown);
        }
        // 一番遅れているものに合わせる。合計で出すと、終わりかけのものに隠れて
        // 長い処理がもうすぐ終わるように見えてしまうため。
        jobs.iter()
            .map(|j| (j.done as f32 / j.total as f32).clamp(0.0, 1.0))
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
        let jobs = self.jobs();
        let first = jobs.first()?;
        let lang = self.lang();
        let mut text = tr(lang, first.name).to_owned();
        if first.total > 0 {
            let percent = 100.0 * first.done as f32 / first.total as f32;
            text.push_str(&format!(" {percent:.0}%"));
        }
        if jobs.len() > 1 {
            text.push_str(&format!(" (+{})", jobs.len() - 1));
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
