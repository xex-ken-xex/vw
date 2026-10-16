//! 定量評価の表（解析ウインドウの「評価」タブ）と、色導出のウインドウに出す 1 行。
//! 数値は [`crate::quality`] が裏で求めたもの。判定は表示のたびに設定の閾値で決めるので、
//! 閾値を変えると求め直さずに判定だけ変わる。

use eframe::egui::{self, Color32, Sense};
use vw_quality::{Metric, Verdict};

use crate::i18n::tr;
use crate::quality::{Basis, Report};
use crate::settings::{Language, Quality};

/// 表から頼まれたこと。
pub enum Command {
    /// 表を CSV にしてコピーする。
    Copy(String),
    /// 基準を変更前に戻す。
    ClearReference,
}

fn metric_title(m: Metric) -> &'static str {
    match m {
        Metric::Psnr => "PSNR (vs reference)",
        Metric::Ssim => "SSIM (vs reference)",
        Metric::Snr => "SNR (target ROI)",
        Metric::Cnr => "CNR (target / background ROI)",
        Metric::Sharpness => "Sharpness (Laplacian variance)",
    }
}

fn verdict_title(v: Verdict) -> &'static str {
    match v {
        Verdict::Good => "Acceptable",
        Verdict::Caution => "Caution",
        Verdict::Degraded => "Degraded",
    }
}

/// 判定の色。vw の暗い画面に合わせて彩度を抑える。
fn verdict_color(v: Verdict) -> Color32 {
    match v {
        Verdict::Good => Color32::from_rgb(110, 180, 120),
        Verdict::Caution => Color32::from_rgb(210, 170, 70),
        Verdict::Degraded => Color32::from_rgb(210, 90, 90),
    }
}

fn value_text(m: Metric, v: f64) -> String {
    match m.unit() {
        Some(u) => format!("{} {u}", m.format(v)),
        None => m.format(v),
    }
}

/// 「評価」タブの中身。
pub fn table(
    ui: &mut egui::Ui,
    lang: Language,
    report: Option<&Report>,
    thresholds: &Quality,
    computing: bool,
) -> Option<Command> {
    let mut out = None;
    let Some(report) = report else {
        ui.centered_and_justified(|ui| ui.spinner());
        return None;
    };
    ui.horizontal(|ui| {
        ui.label(tr(lang, "Reference"));
        match &report.basis {
            Basis::Before => {
                ui.label(tr(lang, "Before (standard display)"));
            }
            Basis::Slot(path) => {
                let file = crate::frames::file_of(path);
                ui.label(file.file_name().unwrap_or_default().to_string_lossy());
                if ui
                    .small_button(tr(lang, "Back to before"))
                    .on_hover_text(tr(lang, "Compare with the standard display again."))
                    .clicked()
                {
                    out = Some(Command::ClearReference);
                }
            }
        }
        ui.weak(tr(
            lang,
            "Right-click a slot to use its image as the reference.",
        ));
        if computing {
            ui.spinner();
        }
    });
    egui::Grid::new("quality_table")
        .num_columns(5)
        .spacing([18.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.weak(tr(lang, "Metric"));
            ui.weak(tr(lang, "Before"));
            ui.weak(tr(lang, "After"));
            ui.weak(tr(lang, "Verdict"));
            ui.weak("");
            ui.end_row();
            for row in &report.rows {
                ui.label(tr(lang, metric_title(row.metric)));
                for v in [row.before, row.after] {
                    match v {
                        Some(v) => {
                            ui.add(egui::Label::new(value_text(row.metric, v)).selectable(true));
                        }
                        None => {
                            ui.weak("—");
                        }
                    }
                }
                match row.judged() {
                    Some(v) => {
                        let t = thresholds.get(row.metric);
                        let verdict = t.verdict(v);
                        ui.colored_label(verdict_color(verdict), tr(lang, verdict_title(verdict)));
                        bar(ui, t.bar(v), verdict_color(verdict));
                    }
                    None => {
                        ui.weak("—");
                        ui.weak(row.missing.map_or("", |m| tr(lang, m.text())));
                    }
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        let roi = |name: &'static str, r: Option<vw_quality::Region>| match r {
            Some(r) => format!(
                "{} ({},{})-({},{})",
                tr(lang, name),
                r.x,
                r.y,
                r.right() - 1,
                r.bottom() - 1
            ),
            None => format!("{} —", tr(lang, name)),
        };
        ui.weak(format!(
            "ROI: {}   {}",
            roi("Target", report.target_roi),
            roi("Background", report.background_roi)
        ));
        if ui.button(tr(lang, "Copy as CSV")).clicked() {
            out = Some(Command::Copy(report.csv()));
        }
    });
    ui.weak(tr(
        lang,
        "All values are measured on the display brightness (0 to 255).",
    ));
    out
}

/// 判定の帯。
fn bar(ui: &mut egui::Ui, fraction: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(90.0, 10.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let filled = rect.with_max_x(rect.min.x + rect.width() * fraction.clamp(0.0, 1.0));
    painter.rect_filled(filled, 2.0, color);
}

/// 色導出のウインドウに出す 1 行。変更後（なければ変更前）の値と判定を並べる。
pub fn summary(ui: &mut egui::Ui, lang: Language, report: Option<&Report>, thresholds: &Quality) {
    ui.horizontal(|ui| {
        ui.weak(tr(lang, "Evaluation"));
        let Some(report) = report else {
            ui.spinner();
            return;
        };
        for row in &report.rows {
            ui.label(tr(lang, short_title(row.metric)));
            match row.judged() {
                Some(v) => {
                    let verdict = thresholds.get(row.metric).verdict(v);
                    ui.colored_label(verdict_color(verdict), value_text(row.metric, v))
                        .on_hover_text(tr(lang, verdict_title(verdict)));
                }
                None => {
                    ui.weak("—")
                        .on_hover_text(row.missing.map_or("", |m| tr(lang, m.text())));
                }
            }
        }
    });
}

fn short_title(m: Metric) -> &'static str {
    match m {
        Metric::Psnr => "PSNR",
        Metric::Ssim => "SSIM",
        Metric::Snr => "SNR",
        Metric::Cnr => "CNR",
        Metric::Sharpness => "Sharpness",
    }
}
