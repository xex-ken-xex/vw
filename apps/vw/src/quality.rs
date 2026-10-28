//! 定量評価。アクティブなスロットの画像の PSNR・SSIM・SNR・CNR・鮮鋭度を裏で求めて持ち、
//! 解析ウインドウの「評価」タブと色導出のウインドウに出す。計算は [`vw_quality`]。
//!
//! 値はどれも表示の明るさで測る。明るさは 16bit の表示の値から求め、目盛りは 8bit の表示と
//! 同じ 0..=255 にする（PSNR の基準と、前の版で測った値とがそのまま比べられるように）。
//! 8bit に丸めてから測らないのは、10bit 以上で描くモニタでは画面に 8bit より細かい段が見えて
//! いるのに、測る値だけが 256 段に丸まってしまうため。
//!
//! 物理量（HU など）で測らないのは、
//! 「変更後」が色導出の結果として表示の色でしか存在せず、変更前と同じ土俵に乗せるには
//! 表示の明るさしかないため。ROI の統計（[`crate::roi`]）は物理量で出すので、SNR の
//! 値はそちらの mean / SD と一致しない。

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui;
use vw_core::{display, DisplayParams, Palette, VwImage};
use vw_quality::{Image, Metric, Region};

/// 表示の明るさの最大。
const PEAK: f32 = 255.0;

/// 評価する画像。毎フレーム、アクティブなスロットから作る。
pub struct Input<'a> {
    /// 表示中の画像のパス（フレームならフレームのパス）。
    pub path: &'a Path,
    /// フル解像度の画像。まだ読み込んでいなければ `None`。
    pub image: Option<&'a Arc<VwImage>>,
    /// スロットで灰色の画像に当てている色の表。変更前の明るさは、これを通した色から求める。
    pub palette: Option<&'a Arc<Palette>>,
    /// 色導出で描いた画像の明るさ（当てているときだけ。[`luma_of_derived`]）。
    pub edited: Option<&'a Arc<Vec<f32>>>,
    pub target_roi: Option<Region>,
    pub background_roi: Option<Region>,
    /// 「評価の基準にする」で選んだ別のスロットの画像。
    pub reference: Option<Reference<'a>>,
}

pub struct Reference<'a> {
    pub path: &'a Path,
    pub image: Option<&'a Arc<VwImage>>,
    pub palette: Option<&'a Arc<Palette>>,
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
    palette: Option<usize>,
    edited: Option<usize>,
    target: Option<Region>,
    background: Option<Region>,
    reference: Option<(PathBuf, Option<usize>, Option<usize>)>,
}

impl Key {
    fn of(input: &Input<'_>, image: &Arc<VwImage>) -> Self {
        Self {
            path: input.path.to_path_buf(),
            image: Arc::as_ptr(image) as usize,
            palette: input.palette.map(|p| Arc::as_ptr(p) as usize),
            edited: input.edited.map(|e| Arc::as_ptr(e) as usize),
            target: input.target_roi,
            background: input.background_roi,
            reference: input.reference.as_ref().map(|r| {
                (
                    r.path.to_path_buf(),
                    r.image.map(|i| Arc::as_ptr(i) as usize),
                    r.palette.map(|p| Arc::as_ptr(p) as usize),
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
    palette: Option<Arc<Palette>>,
    edited: Option<Arc<Vec<f32>>>,
    target: Option<Region>,
    background: Option<Region>,
    reference: Option<PendingReference>,
}

/// 基準のスロットのパス、画像（読み込み中は `None`）、色の表。
type PendingReference = (PathBuf, Option<Arc<VwImage>>, Option<Arc<Palette>>);

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
            palette: input.palette.cloned(),
            edited: input.edited.cloned(),
            target: input.target_roi,
            background: input.background_roi,
            reference: input
                .reference
                .as_ref()
                .map(|r| (r.path.to_path_buf(), r.image.cloned(), r.palette.cloned())),
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

/// 16bit の表示の値の明るさ（輝度、Rec. 709）を、0..=255 の目盛りで。
fn luma_of_rgba16(rgba: &[u16]) -> Vec<f32> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 257.0
        })
        .collect()
}

/// スロットに標準で見えている明るさ（色の表を当てていれば、その色の明るさ）。
fn shown(image: &VwImage, palette: Option<&Palette>) -> Vec<f32> {
    let mut d = display::to_display16(image, &DisplayParams::default());
    if let Some(p) = palette {
        p.recolor16(&mut d.rgba);
    }
    luma_of_rgba16(&d.rgba)
}

/// 色導出の結果（0〜1 の値）の明るさ。変更前（[`shown`]）と同じく 16bit の段に丸め、灰色なら
/// 同じように色の表を引く。手順を揃えるのは、何も変えていない色導出が変更前と同じ値になるように。
pub fn luma_of_derived(float: &crate::nodes::Float, palette: Option<&Palette>) -> Vec<f32> {
    let level = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
    let mut rgba: Vec<u16> = match float.channels {
        1 => float
            .data
            .iter()
            .flat_map(|&v| {
                let g = level(v);
                [g, g, g, u16::MAX]
            })
            .collect(),
        _ => float
            .data
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|c| [level(c[0]), level(c[1]), level(c[2]), u16::MAX])
            .collect(),
    };
    if let (1, Some(p)) = (float.channels, palette) {
        p.recolor16(&mut rgba);
    }
    luma_of_rgba16(&rgba)
}

fn evaluate(p: &Pending) -> Report {
    let (w, h) = (p.image.width, p.image.height);
    let before_px = shown(&p.image, p.palette.as_deref());
    let before = Image::new(w, h, &before_px);
    let after = p.edited.as_deref().map(|px| Image::new(w, h, px));
    let reference_px = p.reference.as_ref().map(|(_, img, palette)| {
        img.as_ref()
            .map(|img| (img.width, img.height, shown(img, palette.as_deref())))
    });
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
            palette: None,
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
        p.reference = Some(("ref.png".into(), Some(ramp(16, 8)), None));
        let r = evaluate(&p);
        assert_eq!(r.basis, Basis::Slot("ref.png".into()));
        assert_eq!(r.rows[0].before, Some(f64::INFINITY));
        p.reference = Some(("ref.png".into(), Some(ramp(8, 8)), None));
        let r = evaluate(&p);
        assert_eq!(r.rows[0].missing, Some(Missing::ReferenceSizeDiffers));
        p.reference = Some(("ref.png".into(), None, None));
        assert_eq!(
            evaluate(&p).rows[1].missing,
            Some(Missing::ReferenceLoading)
        );
    }

    #[test]
    fn the_before_column_reads_the_color_map_the_slot_shows() {
        let mut p = pending(ramp(16, 8));
        // 明るさを逆にする表を当てると、基準（表なし）との差が出る。
        p.palette = Some(Arc::new(Palette::builtin("gray_inverted").unwrap()));
        p.reference = Some(("ref.png".into(), Some(ramp(16, 8)), None));
        let r = evaluate(&p);
        assert!(r.rows[0].before.unwrap() < 10.0, "{:?}", r.rows[0]);
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
        assert!((cnr.before.unwrap() - 17.0).abs() < 1e-4, "{cnr:?}");
        assert_eq!(cnr.missing, None);
    }

    fn gray16(w: u32, h: u32, px: Vec<u16>) -> Arc<VwImage> {
        Arc::new(VwImage::new(w, h, ColorModel::Gray, PixelData::U16(px)).unwrap())
    }

    /// 画像の値をそのまま 0〜1 にした、何も変えていない色導出。
    fn unchanged(image: &VwImage) -> crate::nodes::Float {
        let PixelData::U16(v) = &image.data else {
            panic!("16-bit image expected")
        };
        crate::nodes::Float::new(
            image.width as usize,
            image.height as usize,
            1,
            v.iter().map(|&v| f32::from(v) / 65535.0).collect(),
        )
    }

    #[test]
    fn differences_finer_than_an_8_bit_step_are_measured() {
        // 0 と 100 の市松模様。8bit に丸めるとどちらも 0 になり、鮮鋭度が 0 になっていた。
        let px = (0..64)
            .map(|i| if (i % 8 + i / 8) % 2 == 0 { 0 } else { 100 })
            .collect();
        let r = evaluate(&pending(gray16(8, 8, px)));
        let sharp = r
            .rows
            .iter()
            .find(|r| r.metric == Metric::Sharpness)
            .unwrap();
        assert!(sharp.before.unwrap() > 0.0, "{sharp:?}");
    }

    #[test]
    fn brightness_stays_on_the_0_to_255_scale() {
        let px = shown(&gray16(2, 1, vec![0, 65535]), None);
        assert_eq!(px, [0.0, 255.0]);
    }

    #[test]
    fn an_unchanged_derivation_matches_the_before_column_exactly() {
        let image = gray16(64, 4, (0..256).map(|i| (i * 257 + i % 7) as u16).collect());
        let mut p = pending(image.clone());
        p.edited = Some(Arc::new(luma_of_derived(&unchanged(&image), None)));
        let r = evaluate(&p);
        assert_eq!(r.rows[0].after, Some(f64::INFINITY), "{:?}", r.rows[0]);
    }

    #[test]
    fn a_derivation_reads_the_color_map_the_same_way_as_the_before_column() {
        let image = gray16(64, 4, (0..256).map(|i| (i * 257 + 100) as u16).collect());
        let palette = Palette::builtin("viridis").unwrap();
        assert_eq!(
            luma_of_derived(&unchanged(&image), Some(&palette)),
            shown(&image, Some(&palette))
        );
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
