//! 画素の変換（格納値 → 物理量 → 表示の明るさ）を、手で変えた値で描き直す。
//!
//! 図の表示と操作は [`crate::pipeline_view`] にある。ここは値の組と、その値での描画だけを受け持つ。

use eframe::egui::{Color32, ColorImage};
use vw_core::{PixelData, VwImage};
use vw_dicom::pipeline::Gray;

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
    let (center, width) = match (&g.voi.center, &g.voi.width) {
        (Some(c), Some(w)) => (c.value, w.value),
        _ => {
            let (lo, hi) = full_range(image, decoded, g);
            ((lo + hi) / 2.0, (hi - lo).max(1.0))
        }
    };
    Params {
        slope: g.modality.slope.value,
        intercept: g.modality.intercept.value,
        center,
        width,
        function: VoiFunction::from_dicom(&g.voi.function.value).unwrap_or(VoiFunction::Linear),
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

    #[test]
    fn histogram_spans_the_physical_values() {
        let image = ct(&[-1000.0, 0.0, 1000.0]);
        let h = histogram(&image, &CT, 1.0, -1024.0);
        assert_eq!((h.lo, h.hi), (-1000.0, 1000.0));
        assert_eq!(h.bins.iter().sum::<u32>(), 3);
        assert_eq!(h.bins[255], 1);
    }
}
