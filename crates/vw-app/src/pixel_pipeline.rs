//! 画素の変換（格納値 → 物理量 → 表示の明るさ）を、手で変えた値で描き直す。
//!
//! 図の表示と操作は [`crate::pipeline_view`] にある。ここは値の組と、その値での描画だけを受け持つ。

use eframe::egui::{Color32, ColorImage};
use vw_core::{PixelData, VwImage};
use vw_dicom::pipeline::{Gray, Pipeline};

use crate::frames;

/// VOI LUT Function（DICOM PS3.3 C.11.2.1.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiFunction {
    Linear,
    LinearExact,
    Sigmoid,
}

impl VoiFunction {
    pub const ALL: [VoiFunction; 3] = [
        VoiFunction::Linear,
        VoiFunction::LinearExact,
        VoiFunction::Sigmoid,
    ];

    pub fn from_dicom(name: &str) -> Option<Self> {
        match name.trim() {
            "LINEAR" => Some(Self::Linear),
            "LINEAR_EXACT" => Some(Self::LinearExact),
            "SIGMOID" => Some(Self::Sigmoid),
            _ => None,
        }
    }

    pub fn dicom_name(self) -> &'static str {
        match self {
            Self::Linear => "LINEAR",
            Self::LinearExact => "LINEAR_EXACT",
            Self::Sigmoid => "SIGMOID",
        }
    }

    /// 物理量 `x` を 0..=1 の明るさにする。
    pub fn apply(self, x: f64, center: f64, width: f64) -> f64 {
        match self {
            Self::Linear => {
                // 幅 1 未満は規格で許されないが、手で 0 にされても割り算で壊れないようにする。
                let w = (width - 1.0).max(f64::EPSILON);
                ((x - (center - 0.5)) / w + 0.5).clamp(0.0, 1.0)
            }
            Self::LinearExact => {
                let w = width.max(f64::EPSILON);
                ((x - center) / w + 0.5).clamp(0.0, 1.0)
            }
            Self::Sigmoid => {
                let w = width.max(f64::EPSILON);
                1.0 / (1.0 + (-4.0 * (x - center) / w).exp())
            }
        }
    }
}

/// 変えられる値の組。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub slope: f64,
    pub intercept: f64,
    pub center: f64,
    pub width: f64,
    pub function: VoiFunction,
    pub invert: bool,
}

/// デコーダが画像に持たせた値の作り方。手で変えた値で描くときに、画素から格納値へ戻すのに使う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decoded {
    /// 画素は `sign × (格納値 × slope + intercept)`。U8 / U16 のときは格納値そのもの。
    pub slope: f64,
    pub intercept: f64,
    /// MONOCHROME1 の画素は符号を反転して持っている。
    pub negated: bool,
}

impl Decoded {
    pub fn of(g: &Gray) -> Self {
        Self {
            slope: g.modality.slope.value,
            intercept: g.modality.intercept.value,
            negated: g.presentation.invert,
        }
    }

    /// 画像の `i` 番目のサンプルの格納値。
    fn stored(&self, data: &PixelData, i: usize) -> f64 {
        match data {
            PixelData::U8(v) => v[i].into(),
            PixelData::U16(v) => v[i].into(),
            PixelData::F32(v) => {
                let x = f64::from(v[i]);
                let x = if self.negated { -x } else { x };
                // 傾き 0 のファイルは表示できないので、そのときは物理量をそのまま格納値とみなす。
                if self.slope == 0.0 {
                    x
                } else {
                    (x - self.intercept) / self.slope
                }
            }
        }
    }
}

/// ファイルの値の組。ウィンドウがファイルにないときは、vw が普段使う値の全範囲を当てる
/// （表示を変えずに図を開けるように）。
pub fn file_params(g: &Gray, image: &VwImage, decoded: &Decoded) -> Params {
    let file_function =
        VoiFunction::from_dicom(&g.voi.function.value).unwrap_or(VoiFunction::Linear);
    // ウィンドウがないときの全範囲は端から端まで直線なので、LINEAR（中心を 0.5 ずらす）ではなく
    // LINEAR_EXACT の式に当たる。LINEAR のままにすると標準の表示と 1 段ずれる。
    let (center, width, function) = match (&g.voi.center, &g.voi.width) {
        (Some(c), Some(w)) => (c.value, w.value, file_function),
        _ => {
            let (lo, hi) = full_range(image, decoded, g);
            (
                f64::midpoint(lo, hi),
                (hi - lo).max(1.0),
                VoiFunction::LinearExact,
            )
        }
    };
    Params {
        slope: g.modality.slope.value,
        intercept: g.modality.intercept.value,
        center,
        width,
        function,
        invert: g.presentation.invert,
    }
}

/// ウィンドウがないときに vw-core が当てる範囲を、物理量で表したもの。
fn full_range(image: &VwImage, decoded: &Decoded, g: &Gray) -> (f64, f64) {
    match &image.data {
        PixelData::U8(_) => (0.0, 255.0),
        PixelData::U16(_) => (
            0.0,
            ((1u32 << image.significant_bits.clamp(1, 16)) - 1).into(),
        ),
        PixelData::F32(_) => {
            let modality = |v: f64| v * g.modality.slope.value + g.modality.intercept.value;
            let (lo, hi) = stored_range(image, decoded);
            let (a, b) = (modality(lo), modality(hi));
            (a.min(b), a.max(b))
        }
    }
}

/// 格納値の最小と最大。
fn stored_range(image: &VwImage, decoded: &Decoded) -> (f64, f64) {
    let n = image.width as usize * image.height as usize;
    (0..n)
        .map(|i| decoded.stored(&image.data, i))
        .filter(|v| v.is_finite())
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)))
}

/// 画素 `i` の各段の値: 格納値、物理量、表示の明るさ（0..=255）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trace {
    pub stored: f64,
    pub modality: f64,
    pub display: u8,
}

pub fn trace(image: &VwImage, decoded: &Decoded, p: &Params, x: u32, y: u32) -> Option<Trace> {
    if x >= image.width || y >= image.height {
        return None;
    }
    let i = y as usize * image.width as usize + x as usize;
    let stored = decoded.stored(&image.data, i);
    let modality = stored * p.slope + p.intercept;
    Some(Trace {
        stored,
        modality,
        display: brightness(modality, p),
    })
}

/// 画像の `i` 番目のサンプルの格納値。
pub fn stored_value(image: &VwImage, decoded: &Decoded, i: usize) -> f64 {
    decoded.stored(&image.data, i)
}

/// 画像の `i` 番目のサンプルを `p` の値で描いたときの明るさ。
pub fn display_value(image: &VwImage, decoded: &Decoded, p: &Params, i: usize) -> u8 {
    brightness(decoded.stored(&image.data, i) * p.slope + p.intercept, p)
}

fn brightness(modality: f64, p: &Params) -> u8 {
    let y = p.function.apply(modality, p.center, p.width);
    let y = if p.invert { 1.0 - y } else { y };
    (y * 255.0).round() as u8
}

/// `p` の値で描いた画像。
pub fn render(image: &VwImage, decoded: &Decoded, p: &Params) -> ColorImage {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut pixels = vec![Color32::BLACK; w * h];
    let rows_per = h.div_ceil(threads(w * h)).max(1);
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in pixels.chunks_mut(rows_per * w).enumerate() {
            scope.spawn(move || {
                let first = chunk_index * rows_per * w;
                for (k, px) in chunk.iter_mut().enumerate() {
                    let m = decoded.stored(&image.data, first + k) * p.slope + p.intercept;
                    let g = brightness(m, p);
                    *px = Color32::from_gray(g);
                }
            });
        }
    });
    ColorImage::new([w, h], pixels)
}

/// 小さな画像はスレッドを分けない（立ち上げの方が高くつく）。
fn threads(pixels: usize) -> usize {
    if pixels < 512 * 512 {
        1
    } else {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    }
}

/// 物理量のヒストグラム。グラフの後ろに出す。
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    pub lo: f64,
    pub hi: f64,
    pub bins: Vec<u32>,
}

pub fn histogram(image: &VwImage, decoded: &Decoded, slope: f64, intercept: f64) -> Histogram {
    const BINS: usize = 256;
    let n = image.width as usize * image.height as usize;
    let values = || {
        (0..n)
            .map(|i| decoded.stored(&image.data, i) * slope + intercept)
            .filter(|v| v.is_finite())
    };
    let (lo, hi) = values().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let mut bins = vec![0u32; BINS];
    if hi > lo {
        let scale = (BINS - 1) as f64 / (hi - lo);
        for v in values() {
            bins[((v - lo) * scale) as usize] += 1;
        }
    }
    Histogram {
        lo: lo.min(hi),
        hi,
        bins,
    }
}

/// ファイルの値で描いた画像が、標準の表示（[`vw_core::display::to_display`]）と画素単位で
/// 食い違ったときの、食い違った画素の数と最大の差。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mismatch {
    pub pixels: usize,
    pub max: u8,
}

/// 標準の表示の明るさ（グレースケールなので R だけ）。
pub fn standard_gray(image: &VwImage) -> Vec<u8> {
    let shown = vw_core::display::to_display(image, &vw_core::DisplayParams::default());
    shown.rgba.iter().step_by(4).copied().collect()
}

/// ファイルの値 `file` で描いた画像が標準の表示 `standard` と同じか確かめる。
///
/// 色導出は「標準と同じ画像から、値を変えたところだけが変わる」ことが前提なので、
/// 食い違うファイル（VOI LUT Function が SIGMOID など、標準が表せない変換）では使わない。
pub fn check_standard(
    image: &VwImage,
    decoded: &Decoded,
    file: &Params,
    standard: &[u8],
) -> Result<(), Mismatch> {
    let n = image.width as usize * image.height as usize;
    let (mut pixels, mut max) = (0usize, 0u8);
    for (i, &s) in standard.iter().enumerate().take(n) {
        let d = display_value(image, decoded, file, i).abs_diff(s);
        if d > 0 {
            pixels += 1;
            max = max.max(d);
        }
    }
    if pixels == 0 {
        Ok(())
    } else {
        Err(Mismatch { pixels, max })
    }
}

/// 変更後と標準の明るさの差 |変更後 − 標準| のヒートマップと、差の最大。
/// 色の目盛りは差の最大に合わせる（差が小さくても見えるように）。
pub fn difference(derived: &ColorImage, standard: &[u8]) -> (ColorImage, u8) {
    let diffs: Vec<u8> = derived
        .pixels
        .iter()
        .zip(standard)
        .map(|(c, &s)| c.r().abs_diff(s))
        .collect();
    let max = diffs.iter().copied().max().unwrap_or(0);
    let pixels = diffs
        .iter()
        .map(|&d| {
            heat(if max == 0 {
                0.0
            } else {
                f32::from(d) / f32::from(max)
            })
        })
        .collect();
    (ColorImage::new(derived.size, pixels), max)
}

/// 0..=1 を 黒 → 赤 → 黄 → 白 の色にする。
pub fn heat(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0) * 3.0;
    let ramp = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(ramp(t), ramp(t - 1.0), ramp(t - 2.0))
}

/// ファイルの段（DICOM のヘッダ）を裏のスレッドで読む。画像の画素は別に届くので、ここでは読まない。
pub fn read_in_background(
    ctx: &eframe::egui::Context,
    path: &std::path::Path,
) -> std::sync::mpsc::Receiver<Result<Pipeline, String>> {
    let (file, frame) = frames::split(path).unwrap_or_else(|| (path.to_path_buf(), 0));
    crate::background::compute(ctx, move || {
        std::fs::read(&file)
            .map_err(|e| e.to_string())
            .and_then(|bytes| vw_dicom::pipeline::read(&bytes, frame).map_err(|e| e.to_string()))
    })
}

/// 物理量の単位。CT は HU、PET は Units の値。
pub fn unit_of(g: &Gray) -> Option<String> {
    match g.modality.modality.as_deref().map(str::trim) {
        Some("CT") => Some("HU".into()),
        _ => g.modality.units.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_core::ColorModel;

    fn ct(values: &[f32]) -> VwImage {
        VwImage::new(
            values.len() as u32,
            1,
            ColorModel::Gray,
            PixelData::F32(values.to_vec()),
        )
        .unwrap()
    }

    const CT: Decoded = Decoded {
        slope: 1.0,
        intercept: -1024.0,
        negated: false,
    };

    fn params(center: f64, width: f64) -> Params {
        Params {
            slope: 1.0,
            intercept: -1024.0,
            center,
            width,
            function: VoiFunction::Linear,
            invert: false,
        }
    }

    #[test]
    fn linear_window_maps_its_edges_to_black_and_white() {
        let f = VoiFunction::Linear;
        assert_eq!(f.apply(-200.0, 40.0, 400.0), 0.0);
        assert_eq!(f.apply(300.0, 40.0, 400.0), 1.0);
        assert!((f.apply(40.0, 40.0, 400.0) - 0.5).abs() < 0.01);
    }

    #[test]
    fn sigmoid_is_half_at_the_center() {
        assert_eq!(VoiFunction::Sigmoid.apply(40.0, 40.0, 400.0), 0.5);
    }

    #[test]
    fn trace_goes_from_stored_value_through_hounsfield_units_to_brightness() {
        let image = ct(&[40.0]);
        let t = trace(&image, &CT, &params(40.0, 400.0), 0, 0).unwrap();
        assert_eq!(t.stored, 1064.0);
        assert_eq!(t.modality, 40.0);
        assert_eq!(t.display, 128);
    }

    #[test]
    fn changed_intercept_shifts_the_physical_values() {
        let image = ct(&[40.0]);
        let mut p = params(40.0, 400.0);
        p.intercept = -1000.0;
        assert_eq!(trace(&image, &CT, &p, 0, 0).unwrap().modality, 64.0);
    }

    #[test]
    fn monochrome1_pixels_are_turned_back_into_stored_values() {
        let decoded = Decoded {
            slope: 1.0,
            intercept: 0.0,
            negated: true,
        };
        let image = ct(&[-100.0]);
        let mut p = params(100.0, 200.0);
        p.intercept = 0.0;
        p.invert = true;
        let t = trace(&image, &decoded, &p, 0, 0).unwrap();
        assert_eq!(t.stored, 100.0);
        assert_eq!(t.display, 127);
    }

    #[test]
    fn render_uses_the_changed_window() {
        let image = ct(&[-1000.0, 40.0, 1000.0]);
        let img = render(&image, &CT, &params(40.0, 400.0));
        let gray: Vec<u8> = img.pixels.iter().map(|c| c.r()).collect();
        assert_eq!(gray, vec![0, 128, 255]);
    }

    #[test]
    fn missing_window_falls_back_to_the_full_range() {
        let bytes = vw_dicom::synth::Synth::ct(2, 2)
            .rescale(1.0, -1024.0)
            .bytes();
        let vw_dicom::pipeline::Pipeline::Gray(g) = vw_dicom::pipeline::read(&bytes, 0).unwrap()
        else {
            panic!()
        };
        let image = ct(&[-1024.0, 0.0, 976.0, 0.0]);
        let p = file_params(&g, &image, &CT);
        assert_eq!((p.center, p.width), (-24.0, 2000.0));
    }

    #[test]
    fn per_frame_rescale_is_undone_the_way_the_decoder_applied_it() {
        use vw_core::DecoderPlugin;
        let bytes = vw_dicom::synth::Synth::ct(1, 1)
            .frames_i16(&[&[100], &[100]])
            .per_frame_values(&[(1.0, -1024.0, 40.0, 400.0), (2.0, -2048.0, 40.0, 400.0)])
            .bytes();
        let image = vw_dicom::DicomPlugin.decode_frame(&bytes, 1).unwrap();
        let vw_dicom::pipeline::Pipeline::Gray(g) = vw_dicom::pipeline::read(&bytes, 1).unwrap()
        else {
            panic!()
        };
        let decoded = Decoded::of(&g);
        let p = file_params(&g, &image, &decoded);
        let t = trace(&image, &decoded, &p, 0, 0).unwrap();
        assert_eq!((t.stored, t.modality), (100.0, -1848.0));
    }

    fn synth_image(bytes: &[u8]) -> (VwImage, Gray) {
        use vw_core::DecoderPlugin;
        let image = vw_dicom::DicomPlugin.decode_frame(bytes, 0).unwrap();
        let vw_dicom::pipeline::Pipeline::Gray(g) = vw_dicom::pipeline::read(bytes, 0).unwrap()
        else {
            panic!()
        };
        (image, *g)
    }

    fn file_values_match(bytes: &[u8]) -> Result<(), Mismatch> {
        let (image, g) = synth_image(bytes);
        let decoded = Decoded::of(&g);
        let file = file_params(&g, &image, &decoded);
        check_standard(&image, &decoded, &file, &standard_gray(&image))
    }

    fn ramp_i16(n: usize, from: i16, step: i16) -> Vec<i16> {
        (0..n).map(|i| from + step * i as i16).collect()
    }

    #[test]
    fn file_values_reproduce_the_standard_display_pixel_by_pixel() {
        use vw_dicom::synth::{Synth, CT_IMAGE};
        let ct = || {
            Synth::ct(16, 16)
                .pixels_i16(&ramp_i16(256, -100, 9))
                .rescale(1.0, -1024.0)
        };
        assert_eq!(file_values_match(&ct().window(40.0, 400.0).bytes()), Ok(()));
        assert_eq!(
            file_values_match(&ct().window(-600.0, 1501.0).bytes()),
            Ok(())
        );
        assert_eq!(
            file_values_match(
                &ct()
                    .window(40.0, 400.0)
                    .voi_function("LINEAR_EXACT")
                    .bytes()
            ),
            Ok(())
        );
        assert_eq!(file_values_match(&ct().bytes()), Ok(()));
        let unsigned: Vec<u16> = (0..256).map(|i| i * 16).collect();
        let plain = || Synth::new(CT_IMAGE, 16, 16).pixels_u16(&unsigned, 12);
        assert_eq!(file_values_match(&plain().bytes()), Ok(()));
        assert_eq!(
            file_values_match(&plain().window(2048.0, 1000.0).bytes()),
            Ok(())
        );
        let mono1 = plain().monochrome1().window(1000.0, 3000.0);
        assert_eq!(file_values_match(&mono1.bytes()), Ok(()));
    }

    #[test]
    fn sigmoid_files_do_not_match_the_linear_standard_display() {
        let bytes = vw_dicom::synth::Synth::ct(16, 16)
            .pixels_i16(&ramp_i16(256, -100, 9))
            .rescale(1.0, -1024.0)
            .window(40.0, 400.0)
            .voi_function("SIGMOID")
            .bytes();
        let m = file_values_match(&bytes).unwrap_err();
        assert!(m.pixels > 0 && m.max > 1, "{m:?}");
    }

    #[test]
    fn difference_scales_the_heat_to_the_largest_change() {
        let derived = ColorImage::new(
            [3, 1],
            vec![
                Color32::from_gray(10),
                Color32::from_gray(60),
                Color32::from_gray(100),
            ],
        );
        let (heatmap, max) = difference(&derived, &[10, 40, 140]);
        assert_eq!(max, 40);
        assert_eq!(heatmap.pixels[0], Color32::BLACK);
        assert_eq!(heatmap.pixels[2], Color32::WHITE);
        let (none, zero) = difference(&derived, &[10, 60, 100]);
        assert_eq!(zero, 0);
        assert!(none.pixels.iter().all(|&c| c == Color32::BLACK));
    }

    #[test]
    fn heat_goes_from_black_through_red_and_yellow_to_white() {
        assert_eq!(heat(0.0), Color32::BLACK);
        assert_eq!(heat(1.0 / 3.0), Color32::from_rgb(255, 0, 0));
        assert_eq!(heat(2.0 / 3.0), Color32::from_rgb(255, 255, 0));
        assert_eq!(heat(1.0), Color32::WHITE);
    }

    #[test]
    fn histogram_spans_the_physical_values() {
        let image = ct(&[-1000.0, 0.0, 1000.0]);
        let h = histogram(&image, &CT, 1.0, -1024.0);
        assert_eq!((h.lo, h.hi), (-1000.0, 1000.0));
        assert_eq!(h.bins.iter().sum::<u32>(), 3);
        assert_eq!(h.bins[255], 1);
    }
}
