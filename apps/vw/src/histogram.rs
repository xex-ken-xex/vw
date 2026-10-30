//! 画像全体のヒストグラムを数える。描くのは [`crate::histogram_view`]。
//!
//! 横軸は 2 通り: 表示の明るさ（0..=255、どの形式でも）と、物理量（DICOM の HU など）。

use vw_core::VwImage;

use crate::pixel_pipeline::{self, Decoded};
use crate::roi::Region;

/// 数える画素の添字。`region` がなければ画像全体。
/// 画像からはみ出す ROI は切り落とす。色導出の途中の縮めた絵のように、ROI を引いた画像より
/// 小さい画像を数えることがあるため。
fn indices(width: u32, height: u32, region: Option<Region>) -> Box<dyn Iterator<Item = usize>> {
    match region.map(|r| r.clipped(width, height)) {
        Some(Some(r)) => Box::new(r.indices(width)),
        Some(None) => Box::new(std::iter::empty()),
        None => Box::new(0..width as usize * height as usize),
    }
}

/// 整数の値は小数点を付けずに、そうでない値は小数第 2 位まで出す。
/// 一律に小数で出すと、ほとんどが整数の HU や 8bit の値が `-35.00` のように読みにくくなるため。
pub fn number(v: f64) -> String {
    if (v - v.round()).abs() < 0.005 {
        format!("{:.0}", v.round())
    } else {
        format!("{v:.2}")
    }
}

/// 明るさの段の数。
pub const LEVELS: usize = 256;
/// 物理量のヒストグラムの区間の数。
pub const BINS: usize = 256;

/// 数えるチャンネル。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Luma,
    Red,
    Green,
    Blue,
}

impl Channel {
    pub fn title(self) -> &'static str {
        match self {
            Channel::Luma => "Luminance",
            Channel::Red => "Red",
            Channel::Green => "Green",
            Channel::Blue => "Blue",
        }
    }
}

/// 表示の明るさのヒストグラム。チャンネルごとに [`LEVELS`] 段。
#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    pub luma: Vec<u32>,
    pub red: Vec<u32>,
    pub green: Vec<u32>,
    pub blue: Vec<u32>,
    /// R・G・B が同じでない画素がある。
    pub color: bool,
}

impl Levels {
    fn empty() -> Self {
        Self {
            luma: vec![0; LEVELS],
            red: vec![0; LEVELS],
            green: vec![0; LEVELS],
            blue: vec![0; LEVELS],
            color: false,
        }
    }

    fn add(&mut self, r: u8, g: u8, b: u8) {
        self.luma[luma(r, g, b) as usize] += 1;
        self.red[r as usize] += 1;
        self.green[g as usize] += 1;
        self.blue[b as usize] += 1;
        self.color |= r != g || g != b;
    }

    pub fn channel(&self, c: Channel) -> &[u32] {
        match c {
            Channel::Luma => &self.luma,
            Channel::Red => &self.red,
            Channel::Green => &self.green,
            Channel::Blue => &self.blue,
        }
    }
}

/// 輝度（sRGB の係数、Rec. 709）。
pub fn luma(r: u8, g: u8, b: u8) -> u8 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)).round() as u8
}

/// 表示用に変換した画像（RGBA、幅 `width`）の明るさを数える。透明度は見ない。
pub fn levels_of_rgba(rgba: &[u8], width: u32, region: Option<Region>) -> Levels {
    let px = rgba.as_chunks::<4>().0;
    let height = (px.len() / width.max(1) as usize) as u32;
    let mut levels = Levels::empty();
    for i in indices(width, height, region) {
        let p = px[i];
        levels.add(p[0], p[1], p[2]);
    }
    levels
}

/// 物理量のヒストグラム。変更前と変更後で同じ区間に数え、重ねて比べられるようにする。
#[derive(Debug, Clone, PartialEq)]
pub struct Physical {
    pub lo: f64,
    pub hi: f64,
    pub before: Vec<u32>,
    pub after: Option<Vec<u32>>,
    pub before_stats: Stats,
    pub after_stats: Option<Stats>,
}

/// 物理量（格納値 × 傾き + 切片）を数える。`after` は手で変えた傾きと切片で、
/// ファイルの値と同じなら数えない（変更前と同じ山になるだけなので）。
pub fn physical(
    image: &VwImage,
    decoded: &Decoded,
    before: (f64, f64),
    after: Option<(f64, f64)>,
    region: Option<Region>,
) -> Physical {
    let after = after.filter(|a| *a != before);
    let values = |(slope, intercept): (f64, f64)| {
        indices(image.width, image.height, region)
            .map(move |i| pixel_pipeline::stored_value(image, decoded, i) * slope + intercept)
            .filter(|v| v.is_finite())
    };
    let range =
        |r: (f64, f64)| values(r).fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let (mut lo, mut hi) = range(before);
    if let Some(a) = after {
        let (alo, ahi) = range(a);
        (lo, hi) = (lo.min(alo), hi.max(ahi));
    }
    if lo > hi {
        (lo, hi) = (0.0, 0.0);
    }
    let count = |r| {
        let mut bins = vec![0u32; BINS];
        let mut acc = Accumulator::default();
        for v in values(r) {
            let i = if hi > lo {
                (((v - lo) / (hi - lo)) * BINS as f64) as usize
            } else {
                0
            };
            bins[i.min(BINS - 1)] += 1;
            acc.add(v);
        }
        (bins, acc.stats())
    };
    let (before, before_stats) = count(before);
    let (after, after_stats) = match after.map(count) {
        Some((b, s)) => (Some(b), Some(s)),
        None => (None, None),
    };
    Physical {
        lo,
        hi,
        before,
        after,
        before_stats,
        after_stats,
    }
}

/// 最小・最大・平均・標準偏差（母標準偏差）。画素がなければすべて 0。
/// 区間ごとの数から、`lo`〜`hi` を等分した区間の真ん中の値として統計を出す。
pub fn bin_stats(counts: &[u32], lo: f64, hi: f64) -> Stats {
    let mut acc = Accumulator::default();
    let step = (hi - lo) / counts.len().max(1) as f64;
    for (i, &n) in counts.iter().enumerate() {
        acc.add_many(lo + step * (i as f64 + 0.5), n);
    }
    acc.stats()
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub std_dev: f64,
}

#[derive(Default)]
pub struct Accumulator {
    n: u64,
    sum: f64,
    sum_sq: f64,
    min: f64,
    max: f64,
}

impl Accumulator {
    pub fn add(&mut self, v: f64) {
        if self.n == 0 {
            (self.min, self.max) = (v, v);
        }
        self.n += 1;
        self.sum += v;
        self.sum_sq += v * v;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
    }

    fn add_many(&mut self, v: f64, count: u32) {
        if count == 0 {
            return;
        }
        if self.n == 0 {
            (self.min, self.max) = (v, v);
        }
        let c = f64::from(count);
        self.n += u64::from(count);
        self.sum += v * c;
        self.sum_sq += v * v * c;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
    }

    pub fn stats(&self) -> Stats {
        if self.n == 0 {
            return Stats::default();
        }
        let n = self.n as f64;
        let mean = self.sum / n;
        Stats {
            min: self.min,
            max: self.max,
            mean,
            std_dev: (self.sum_sq / n - mean * mean).max(0.0).sqrt(),
        }
    }
}

/// 明るさのヒストグラムの統計。段の番号がそのまま値なので、数えた結果から正確に求まる。
pub fn level_stats(counts: &[u32]) -> Stats {
    let mut acc = Accumulator::default();
    for (v, &c) in counts.iter().enumerate() {
        acc.add_many(v as f64, c);
    }
    acc.stats()
}

#[cfg(test)]
mod tests {

    #[test]
    fn bin_stats_take_each_bin_at_its_middle() {
        // 0〜100 を 4 区間に分け、2 番目（25〜50、真ん中 37.5）に 1、4 番目（真ん中 87.5）に 1。
        let s = bin_stats(&[0, 1, 0, 1], 0.0, 100.0);
        assert_eq!((s.min, s.max, s.mean), (37.5, 87.5, 62.5));
    }

    use super::*;
    use vw_core::{ColorModel, PixelData};

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

    #[test]
    fn a_region_reaching_past_a_smaller_picture_counts_only_what_is_inside() {
        // 2x2 の絵に、元の画像（もっと大きい）の座標で引いた ROI を当てる。
        let rgba = [10, 10, 10, 255].repeat(4);
        let region = Region {
            x: 1,
            y: 1,
            w: 100,
            h: 100,
        };
        let l = levels_of_rgba(&rgba, 2, Some(region));
        assert_eq!(l.channel(Channel::Luma)[10], 1);
        let outside = Region { x: 5, ..region };
        assert_eq!(
            levels_of_rgba(&rgba, 2, Some(outside))
                .luma
                .iter()
                .sum::<u32>(),
            0
        );
    }

    #[test]
    fn gray_pixels_count_the_same_in_every_channel_and_are_not_color() {
        let l = levels_of_rgba(&[10, 10, 10, 255, 200, 200, 200, 0], 2, None);
        for c in [Channel::Luma, Channel::Red, Channel::Green, Channel::Blue] {
            assert_eq!(l.channel(c)[10], 1);
            assert_eq!(l.channel(c)[200], 1);
        }
        assert!(!l.color);
    }

    #[test]
    fn color_pixels_are_counted_per_channel_and_by_luminance() {
        let l = levels_of_rgba(&[255, 0, 0, 255], 1, None);
        assert!(l.color);
        assert_eq!(l.red[255], 1);
        assert_eq!(l.green[0], 1);
        assert_eq!(l.luma[54], 1);
    }

    #[test]
    fn level_stats_are_exact() {
        let mut counts = vec![0; LEVELS];
        counts[0] = 1;
        counts[255] = 1;
        let s = level_stats(&counts);
        assert_eq!(
            (s.min, s.max, s.mean, s.std_dev),
            (0.0, 255.0, 127.5, 127.5)
        );
    }

    #[test]
    fn physical_histogram_spans_both_before_and_after_on_one_axis() {
        let image = ct(&[-1000.0, 0.0, 1000.0]);
        let p = physical(&image, &CT, (1.0, -1024.0), Some((1.0, -24.0)), None);
        assert_eq!((p.lo, p.hi), (-1000.0, 2000.0));
        assert_eq!(p.before.iter().sum::<u32>(), 3);
        assert_eq!(p.after.as_ref().unwrap().iter().sum::<u32>(), 3);
        assert_eq!(p.before_stats.mean, 0.0);
        assert_eq!(p.after_stats.unwrap().mean, 1000.0);
    }

    #[test]
    fn unchanged_rescale_has_no_after_histogram() {
        let image = ct(&[0.0, 1.0]);
        let p = physical(&image, &CT, (1.0, -1024.0), Some((1.0, -1024.0)), None);
        assert!(p.after.is_none());
    }

    #[test]
    fn region_limits_the_count_to_its_pixels() {
        let image = ct(&[0.0, 10.0, 20.0, 30.0]);
        let region = Region {
            x: 1,
            y: 0,
            w: 2,
            h: 1,
        };
        let p = physical(&image, &CT, (1.0, -1024.0), None, Some(region));
        assert_eq!((p.lo, p.hi), (10.0, 20.0));
        assert_eq!(p.before.iter().sum::<u32>(), 2);
        let l = levels_of_rgba(
            &[0, 0, 0, 255, 9, 9, 9, 255, 1, 1, 1, 255, 2, 2, 2, 255],
            4,
            Some(region),
        );
        assert_eq!((l.luma[9], l.luma[1], l.luma[0]), (1, 1, 0));
    }

    #[test]
    fn single_valued_image_lands_in_one_bin() {
        let image = ct(&[5.0, 5.0]);
        let p = physical(&image, &CT, (1.0, -1024.0), None, None);
        assert_eq!(p.before[0], 2);
        assert_eq!(p.before_stats.std_dev, 0.0);
    }
}
