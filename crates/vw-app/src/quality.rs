//! 定量評価。アクティブなスロットの画像の PSNR・SSIM・SNR・CNR・鮮鋭度を裏で求めて持ち、
//! 解析ウインドウの「評価」タブと色導出のウインドウに出す。計算は [`vw_quality`]。
//!
//! 値はどれも表示の明るさ（0..=255）で測る。物理量（HU など）で測らないのは、
//! 「変更後」が色導出の結果として表示の色でしか存在せず、変更前と同じ土俵に乗せるには
//! 表示の明るさしかないため。ROI の統計（[`crate::roi`]）は物理量で出すので、SNR の
//! 値はそちらの mean / SD と一致しない。

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui;
use vw_core::{display, DisplayParams, VwImage};
use vw_quality::{Image, Metric, Region};

use crate::pixel_pipeline::{self, Decoded, Params};

/// 表示の明るさの最大。
const PEAK: f32 = 255.0;

/// 評価する画像。毎フレーム、アクティブなスロットから作る。
pub struct Input<'a> {
    /// 表示中の画像のパス（フレームならフレームのパス）。
    pub path: &'a Path,
    /// フル解像度の画像。まだ読み込んでいなければ `None`。
    pub image: Option<&'a Arc<VwImage>>,
    /// 色導出で手で変えている値と、その画像のデコードの仕方（変えているときだけ）。
    pub edited: Option<(Params, Decoded)>,
    pub target_roi: Option<Region>,
    pub background_roi: Option<Region>,
    /// 「評価の基準にする」で選んだ別のスロットの画像。
    pub reference: Option<Reference<'a>>,
}

pub struct Reference<'a> {
    pub path: &'a Path,
    pub image: Option<&'a Arc<VwImage>>,
}

/// 値が出せない理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// 基準と比べる指標なのに、変更後も基準のスロットもない。
    NoReference,
    ReferenceLoading,
    ReferenceSizeDiffers,
    NeedsTarget,
    NeedsBackground,
}

impl Missing {
    pub fn text(self) -> &'static str {
        match self {
            Missing::NoReference => {
                "Change values in the pixel pipeline, or choose a reference image in a slot."
            }
            Missing::ReferenceLoading => "The reference image is still loading.",
            Missing::ReferenceSizeDiffers => "The reference image has a different size.",
            Missing::NeedsTarget => "Draw a target ROI with the ROI tool.",
            Missing::NeedsBackground => "Draw a background ROI (Shift + drag) with the ROI tool.",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub metric: Metric,
    /// 変更前（標準の表示）の値。
    pub before: Option<f64>,
    /// 変更後（色導出の結果）の値。変えていなければ `None`。
    pub after: Option<f64>,
    pub missing: Option<Missing>,
}

impl Row {
    /// 判定に使う値。変更後があればそれ、なければ変更前。
    pub fn judged(&self) -> Option<f64> {
        self.after.or(self.before)
    }
}

/// 基準にした画像。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Basis {
    /// 変更前（標準の表示）。
    Before,
    Slot(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub rows: Vec<Row>,
    /// 変更後の列がある。
    pub changed: bool,
    pub basis: Basis,
    pub target_roi: Option<Region>,
    pub background_roi: Option<Region>,
}

impl Report {
    /// 表計算に貼る形。
    pub fn csv(&self) -> String {
        let mut lines = vec!["metric,before,after".to_owned()];
        for r in &self.rows {
            let cell = |v: Option<f64>| v.map(|v| r.metric.format(v)).unwrap_or_default();
            lines.push(format!(
                "{},{},{}",
                r.metric.key(),
                cell(r.before),
                cell(r.after)
            ));
        }
        lines.join("\n")
    }
}

/// 求め直すかどうかを決める、求めた条件。
#[derive(Debug, Clone, PartialEq)]
struct Key {
    path: PathBuf,
    image: usize,
    edited: Option<Params>,
    target: Option<Region>,
    background: Option<Region>,
    reference: Option<(PathBuf, Option<usize>)>,
}

impl Key {
    fn of(input: &Input<'_>, image: &Arc<VwImage>) -> Self {
        Self {
            path: input.path.to_path_buf(),
            image: Arc::as_ptr(image) as usize,
            edited: input.edited.map(|(p, _)| p),
            target: input.target_roi,
            background: input.background_roi,
            reference: input.reference.as_ref().map(|r| {
                (
                    r.path.to_path_buf(),
                    r.image.map(|i| Arc::as_ptr(i) as usize),
                )
            }),
        }
    }
}

#[derive(Default)]
pub struct Evaluation {
    report: Option<(Key, Report)>,
    computing: Option<(Key, Receiver<Report>)>,
}

/// 裏のスレッドに渡す、`Input` の写し。
struct Pending {
    image: Arc<VwImage>,
    edited: Option<(Params, Decoded)>,
    target: Option<Region>,
    background: Option<Region>,
    reference: Option<(PathBuf, Option<Arc<VwImage>>)>,
}

impl Evaluation {
    /// 解析か色導出のウインドウが開いている間、毎フレーム呼ぶ。条件が変わっていたら裏で求め直す。
    /// 同時に 2 つは走らせない。求めている間に条件が変わっても、届いた次のフレームに
    /// そのときの条件で求め直すので、ここで覚えておく必要はない。
    pub fn update(&mut self, ctx: &egui::Context, input: Option<&Input<'_>>) {
        if let Some((key, rx)) = &self.computing {
            if let Ok(report) = rx.try_recv() {
                self.report = Some((key.clone(), report));
                self.computing = None;
            }
        }
        let Some(input) = input else {
            self.clear();
            return;
        };
        let Some(image) = input.image else { return };
        let key = Key::of(input, image);
        let done = self.report.as_ref().is_some_and(|(k, _)| *k == key);
        if done || self.computing.is_some() {
            return;
        }
        let pending = Pending {
            image: image.clone(),
            edited: input.edited,
            target: input.target_roi,
            background: input.background_roi,
            reference: input
                .reference
                .as_ref()
                .map(|r| (r.path.to_path_buf(), r.image.cloned())),
        };
        let rx = crate::background::compute(ctx, move || evaluate(&pending));
        self.computing = Some((key, rx));
    }

    /// 最後に求めた結果。条件が変わって求め直している間も、前の結果を出しておく。
    pub fn report(&self) -> Option<&Report> {
        self.report.as_ref().map(|(_, r)| r)
    }

    pub fn computing(&self) -> bool {
        self.computing.is_some()
    }

    pub fn clear(&mut self) {
        self.report = None;
        self.computing = None;
    }
}

/// 表示の明るさ（輝度、Rec. 709）。
fn luma_of_rgba(rgba: &[u8]) -> Vec<f32> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
        .collect()
}

fn luma_of_color_image(img: &egui::ColorImage) -> Vec<f32> {
    img.pixels
        .iter()
        .map(|p| 0.2126 * f32::from(p.r()) + 0.7152 * f32::from(p.g()) + 0.0722 * f32::from(p.b()))
        .collect()
}

fn shown(image: &VwImage) -> Vec<f32> {
    luma_of_rgba(&display::to_display(image, &DisplayParams::default()).rgba)
}

fn evaluate(p: &Pending) -> Report {
    let (w, h) = (p.image.width, p.image.height);
    let before_px = shown(&p.image);
    let before = Image::new(w, h, &before_px);
    let after_px = p.edited.map(|(params, decoded)| {
        luma_of_color_image(&pixel_pipeline::render(&p.image, &decoded, &params))
    });
    let after = after_px.as_deref().map(|px| Image::new(w, h, px));
    let reference_px = p
        .reference
        .as_ref()
        .map(|(_, img)| img.as_ref().map(|img| (img.width, img.height, shown(img))));
    // 基準: 別のスロットがあればその画像。なければ変更前で、そのとき変更前の列は空になる。
    let (basis, reference, missing_reference) = match &reference_px {
        Some(Some((rw, rh, px))) if (*rw, *rh) == (w, h) => (
            Basis::Slot(p.reference.as_ref().unwrap().0.clone()),
            Some(Image::new(*rw, *rh, px)),
            None,
        ),
        Some(Some(_)) => (
            Basis::Slot(p.reference.as_ref().unwrap().0.clone()),
            None,
            Some(Missing::ReferenceSizeDiffers),
        ),
        Some(None) => (
            Basis::Slot(p.reference.as_ref().unwrap().0.clone()),
            None,
            Some(Missing::ReferenceLoading),
        ),
        None => (Basis::Before, None, None),
    };
    let rows = Metric::ALL
        .iter()
        .map(|&metric| {
            let mut missing = None;
            let compare = |a: &Image<'_>, b: &Image<'_>| match metric {
                Metric::Psnr => vw_quality::psnr(a, b, PEAK),
                Metric::Ssim => vw_quality::ssim(a, b, PEAK),
                _ => None,
            };
            let single = |img: &Image<'_>| match metric {
                Metric::Snr => p.target.and_then(|t| vw_quality::snr(img, t)),
                Metric::Cnr => p
                    .target
                    .zip(p.background)
                    .and_then(|(t, b)| vw_quality::cnr(img, t, b)),
                Metric::Sharpness => vw_quality::laplacian_variance(img),
                _ => None,
            };
            let (before_v, after_v) = if metric.needs_reference() {
                match (&reference, missing_reference) {
                    (Some(r), _) => (
                        compare(&before, r),
                        after.as_ref().and_then(|a| compare(a, r)),
                    ),
                    (None, Some(m)) => {
                        missing = Some(m);
                        (None, None)
                    }
                    (None, None) => {
                        let v = after.as_ref().and_then(|a| compare(a, &before));
                        if v.is_none() {
                            missing = Some(Missing::NoReference);
                        }
                        (None, v)
                    }
                }
            } else {
                if metric.needs_target() && p.target.is_none() {
                    missing = Some(Missing::NeedsTarget);
                } else if metric.needs_background() && p.background.is_none() {
                    missing = Some(Missing::NeedsBackground);
                }
                (single(&before), after.as_ref().and_then(single))
            };
            Row {
                metric,
                before: before_v,
                after: after_v,
                missing,
            }
        })
        .collect();
    Report {
        rows,
        changed: after.is_some(),
        basis,
        target_roi: p.target,
        background_roi: p.background,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_core::{ColorModel, PixelData};

    fn gray(w: u32, h: u32, px: Vec<u8>) -> Arc<VwImage> {
        Arc::new(VwImage::new(w, h, ColorModel::Gray, PixelData::U8(px)).unwrap())
    }

    fn ramp(w: u32, h: u32) -> Arc<VwImage> {
        gray(
            w,
            h,
            (0..w * h).map(|i| (i % w * 255 / (w - 1)) as u8).collect(),
        )
    }

    fn pending(image: Arc<VwImage>) -> Pending {
        Pending {
            image,
            edited: None,
            target: None,
            background: None,
            reference: None,
        }
    }

    #[test]
    fn without_a_reference_or_a_change_the_compared_metrics_say_why_they_are_empty() {
        let r = evaluate(&pending(ramp(16, 8)));
        assert_eq!(r.basis, Basis::Before);
        assert!(!r.changed);
        let psnr = &r.rows[0];
        assert_eq!((psnr.before, psnr.after), (None, None));
        assert_eq!(psnr.missing, Some(Missing::NoReference));
        let snr = r.rows.iter().find(|r| r.metric == Metric::Snr).unwrap();
        assert_eq!(snr.missing, Some(Missing::NeedsTarget));
        let sharp = r
            .rows
            .iter()
            .find(|r| r.metric == Metric::Sharpness)
            .unwrap();
        assert!(sharp.before.is_some() && sharp.after.is_none());
    }

    #[test]
    fn a_reference_slot_fills_the_before_column_and_a_different_size_is_refused() {
        let mut p = pending(ramp(16, 8));
        p.reference = Some(("ref.png".into(), Some(ramp(16, 8))));
        let r = evaluate(&p);
        assert_eq!(r.basis, Basis::Slot("ref.png".into()));
        assert_eq!(r.rows[0].before, Some(f64::INFINITY));
        p.reference = Some(("ref.png".into(), Some(ramp(8, 8))));
        let r = evaluate(&p);
        assert_eq!(r.rows[0].missing, Some(Missing::ReferenceSizeDiffers));
        p.reference = Some(("ref.png".into(), None));
        assert_eq!(
            evaluate(&p).rows[1].missing,
            Some(Missing::ReferenceLoading)
        );
    }

    #[test]
    fn rois_give_snr_and_cnr_in_display_brightness() {
        let mut p = pending(gray(4, 1, vec![10, 20, 100, 100]));
        p.target = Some(Region {
            x: 2,
            y: 0,
            w: 2,
            h: 1,
        });
        p.background = Some(Region {
            x: 0,
            y: 0,
            w: 2,
            h: 1,
        });
        let r = evaluate(&p);
        let snr = r.rows.iter().find(|r| r.metric == Metric::Snr).unwrap();
        assert_eq!(snr.before, Some(f64::INFINITY));
        let cnr = r.rows.iter().find(|r| r.metric == Metric::Cnr).unwrap();
        assert_eq!(cnr.before, Some(17.0));
        assert_eq!(cnr.missing, None);
    }

    #[test]
    fn csv_lists_every_metric_with_empty_cells_for_missing_values() {
        let r = evaluate(&pending(ramp(16, 8)));
        let csv = r.csv();
        assert!(
            csv.starts_with("metric,before,after\npsnr,,\nssim,,\n"),
            "{csv}"
        );
        let sharpness = csv.lines().find(|l| l.starts_with("sharpness,")).unwrap();
        let cells: Vec<&str> = sharpness.split(',').collect();
        assert!(!cells[1].is_empty() && cells[2].is_empty(), "{sharpness}");
    }
}
