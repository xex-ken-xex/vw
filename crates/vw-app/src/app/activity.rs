//! 裏で走っている仕事の一覧と、画面下の活動ラインとトースト。

use std::time::{Duration, Instant};

use eframe::egui::{self, vec2};

use crate::i18n::tr;
use crate::theme;

use super::*;

impl VwApp {
    /// 進めている処理の名前と (済んだ数, 全部の数)。ZIP の展開・データセットの読み込みや取り出し・書き出し・送信。
    pub(super) fn jobs(&self) -> Vec<(&'static str, usize, usize)> {
        let jobs = [
            ("Extracting ZIP", self.archives.progress()),
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
        jobs.into_iter()
            .filter_map(|(name, p)| p.map(|(done, total)| (name, done, total)))
            .collect()
    }

    /// 進めている処理の進み具合（0〜1）。何もしていなければ `None`。
    pub(super) fn activity(&self) -> Option<f32> {
        let jobs = self.jobs();
        // 一番遅れているものに合わせる。合計で出すと、終わりかけのものに隠れて
        // 長い処理がもうすぐ終わるように見えてしまうため。
        jobs.iter()
            .map(|&(_, done, total)| (done as f32 / total.max(1) as f32).clamp(0.0, 1.0))
            .reduce(f32::min)
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
                let Some(fraction) = activity else {
                    painter.rect_filled(rect, 0.0, ui.visuals().panel_fill);
                    return;
                };
                painter.rect_filled(rect, 0.0, theme::ACTIVITY_TRACK);
                let mut done = rect;
                done.set_width(rect.width() * fraction);
                painter.rect_filled(done, 0.0, theme::ACTIVITY);
            });
        if activity.is_some() {
            // 展開は終わるまで知らせてこないので、線を進めるために描き直す。
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
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
