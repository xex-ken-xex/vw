//! 画素の変換（格納値 → 物理量 → 表示の明るさ）を、手で変えた値で描き直す。
//!
//! 図の表示と操作は [`crate::pipeline_view`] にある。ここは値の組と、その値での描画だけを受け持つ。

use eframe::egui::{Color32, ColorImage};
use vw_core::{PixelData, VwImage};
use vw_dicom::pipeline::{Gray, Pipeline};

use crate::frames;
use crate::nodes::Float;

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

/// 物理量 `modality` を `p` の窓で 0〜1 に写した表示の値。
fn unit_brightness(modality: f64, p: &Params) -> f64 {
    let y = p.function.apply(modality, p.center, p.width);
    if p.invert {
        1.0 - y
    } else {
        y
    }
}

fn brightness(modality: f64, p: &Params) -> u8 {
    (unit_brightness(modality, p) * 255.0).round() as u8
}

/// `p` の値で描いた表示の値（0〜1、グレー 1 チャンネル）。自由なノードはこの後ろに当てる。
pub fn float_of_params(image: &VwImage, decoded: &Decoded, p: &Params) -> Float {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut data = vec![0.0f32; w * h];
    let rows_per = h.div_ceil(threads(w * h)).max(1);
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in data.chunks_mut(rows_per * w).enumerate() {
            scope.spawn(move || {
                let first = chunk_index * rows_per * w;
                for (k, out) in chunk.iter_mut().enumerate() {
                    let m = decoded.stored(&image.data, first + k) * p.slope + p.intercept;
                    *out = unit_brightness(m, p) as f32;
                }
            });
        }
    });
    Float::new(w, h, 1, data)
}

/// 標準の写し方（[`vw_core::display::to_display`] と同じ式、曲線も含む）で 0〜1 にした表示の値。
/// DICOM 以外の画像と、カラーの DICOM に使う。U16 は有効ビットの全範囲、F32 は
/// 画像の窓か最小〜最大を 0〜1 に写すので、8bit に落とす前の階調が残る。
pub fn float_of_display(image: &VwImage) -> Float {
    let (w, h) = (image.width as usize, image.height as usize);
    let samples = image.color.channels();
    let channels = if samples >= 3 { 3 } else { 1 };
    let n = w * h;
    let mut data = Vec::with_capacity(n * channels);
    let mut push = |sample: &dyn Fn(usize) -> f32| {
        for i in 0..n {
            for c in 0..channels {
                data.push(sample(i * samples + c));
            }
        }
    };
    match &image.data {
        PixelData::U8(v) => match image.window {
            None => push(&|i| f32::from(v[i]) * (1.0 / 255.0)),
            Some((lo, hi)) => {
                let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
                push(&|i| ((f32::from(v[i]) - lo) * scale).clamp(0.0, 255.0) / 255.0);
            }
        },
        PixelData::U16(v) => {
            let max = ((1u32 << image.significant_bits.clamp(1, 16)) - 1) as f32;
            let (lo, hi) = image.window.unwrap_or((0.0, max));
            let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
            push(&|i| ((f32::from(v[i]) - lo) * scale).clamp(0.0, 255.0) / 255.0);
        }
        PixelData::F32(v) => {
            let (lo, hi) = image.window.unwrap_or_else(|| {
                v.iter()
                    .filter(|x| x.is_finite())
                    .fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)))
            });
            let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
            push(&|i| ((v[i] - lo) * scale).clamp(0.0, 255.0) / 255.0);
        }
    }
    if image.curve != vw_core::Curve::Linear {
        for v in &mut data {
            *v = image.curve.apply(*v);
        }
    }
    Float::new(w, h, channels, data)
}

/// 標準の表示（RGBA）。
pub fn standard_display(image: &VwImage) -> Vec<u8> {
    // 大きな画像も縮めない（ノードは全画素に当てるので、標準も同じ大きさで要る）。
    let params = vw_core::DisplayParams {
        max_side: u32::MAX,
        ..Default::default()
    };
    vw_core::display::to_display(image, &params).rgba
}

/// `float` を丸めた表示と、標準の表示のアルファを合わせた画像。
pub fn to_color_image(float: &Float, standard_rgba: &[u8]) -> ColorImage {
    let rgb = float.to_rgb8();
    let pixels = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .zip(standard_rgba.as_chunks::<4>().0)
        .map(|(c, s)| Color32::from_rgba_premultiplied(c[0], c[1], c[2], s[3]))
        .collect();
    ColorImage::new([float.width, float.height], pixels)
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

/// 何も変えていない `float` を丸めた表示が、標準の表示 `standard_rgba` と同じか確かめる。
///
/// 色導出は「標準と同じ画像から、変えたところだけが変わる」ことが前提なので、
/// 食い違うファイル（VOI LUT Function が SIGMOID など、標準が表せない変換）では使わない。
pub fn check_standard(float: &Float, standard_rgba: &[u8]) -> Result<(), Mismatch> {
    let rgb = float.to_rgb8();
    let (mut pixels, mut max) = (0usize, 0u8);
    for (c, s) in rgb
        .as_chunks::<3>()
        .0
        .iter()
        .zip(standard_rgba.as_chunks::<4>().0)
    {
        let d = (0..3).map(|k| c[k].abs_diff(s[k])).max().unwrap_or(0);
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

/// 変更後と標準の明るさの差 |変更後 − 標準|（RGB の最大）のヒートマップと、差の最大。
/// 色の目盛りは差の最大に合わせる（差が小さくても見えるように）。
pub fn difference(derived: &ColorImage, standard_rgba: &[u8]) -> (ColorImage, u8) {
    let diffs: Vec<u8> = derived
        .pixels
        .iter()
        .zip(standard_rgba.as_chunks::<4>().0)
        .map(|(c, s)| {
            c.r()
                .abs_diff(s[0])
                .max(c.g().abs_diff(s[1]))
                .max(c.b().abs_diff(s[2]))
        })
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
        let float = float_of_params(&image, &CT, &params(40.0, 400.0));
        assert_eq!(float.to_rgb8()[..7], [0, 0, 0, 128, 128, 128, 255]);
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
        check_standard(
            &float_of_params(&image, &decoded, &file),
            &standard_display(&image),
        )
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

    fn plain_matches(image: &VwImage) -> Result<(), Mismatch> {
        check_standard(&float_of_display(image), &standard_display(image))
    }

    #[test]
    fn plain_images_of_every_sample_type_reproduce_the_standard_display() {
        use vw_core::ColorModel;
        let gray8 =
            VwImage::new(256, 1, ColorModel::Gray, PixelData::U8((0..=255).collect())).unwrap();
        assert_eq!(plain_matches(&gray8), Ok(()));
        let rgba8: Vec<u8> = (0..1024).map(|i| (i * 7 % 256) as u8).collect();
        let rgba = VwImage::new(256, 1, ColorModel::Rgba, PixelData::U8(rgba8)).unwrap();
        assert_eq!(plain_matches(&rgba), Ok(()));
        for bits in [9u32, 10, 12, 14, 16] {
            let max = (1u32 << bits) - 1;
            // 標準の表示は長辺を縮めるので、全値を並べても縮まない幅に折り返す。
            let width = (max + 1).min(4096);
            let mut img = VwImage::new(
                width,
                (max + 1) / width,
                ColorModel::Gray,
                PixelData::U16((0..=max as u16).collect()),
            )
            .unwrap();
            img.significant_bits = bits;
            assert_eq!(plain_matches(&img), Ok(()), "{bits} bit");
            img.window = Some((100.0, max as f32 * 0.6));
            assert_eq!(plain_matches(&img), Ok(()), "{bits} bit, windowed");
        }
        let values: Vec<f32> = (0..2000).map(|i| i as f32 * 0.37 - 100.0).collect();
        let mut f = VwImage::new(2000, 1, ColorModel::Gray, PixelData::F32(values)).unwrap();
        assert_eq!(plain_matches(&f), Ok(()));
        f.window = Some((-50.0, 300.0));
        assert_eq!(plain_matches(&f), Ok(()));
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
        let standard = |g: [u8; 3]| -> Vec<u8> { g.iter().flat_map(|&v| [v, v, v, 255]).collect() };
        let (heatmap, max) = difference(&derived, &standard([10, 40, 140]));
        assert_eq!(max, 40);
        assert_eq!(heatmap.pixels[0], Color32::BLACK);
        assert_eq!(heatmap.pixels[2], Color32::WHITE);
        let (none, zero) = difference(&derived, &standard([10, 60, 100]));
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
