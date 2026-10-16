//! 画像の定量的な評価。幅・高さ・f32 の画素の列と矩形を受けて数値を返すだけの関数群。
//!
//! GUI や画像形式には依存しない。vw の外（別のリポジトリや crates.io）へ切り出しても
//! 呼ぶ側は Cargo.toml の 1 行を変えるだけで済むように、この crate は std 以外に
//! 画像のクレートも持たない。

use serde::{Deserialize, Serialize};

/// グレースケールの画像。行の順に並んだ `width * height` 個の画素。
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [f32],
}

impl<'a> Image<'a> {
    pub fn new(width: u32, height: u32, pixels: &'a [f32]) -> Self {
        assert_eq!(pixels.len(), width as usize * height as usize);
        Self {
            width,
            height,
            pixels,
        }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.pixels[y * self.width as usize + x]
    }

    fn same_size(&self, other: &Image<'_>) -> bool {
        self.width == other.width && self.height == other.height
    }
}

/// 画像の中の矩形（画素の単位、`x..x+w` × `y..y+h`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Region {
    /// 2 つの角から作る。どちらの角が先でもよい。
    pub fn from_corners(a: (u32, u32), b: (u32, u32)) -> Self {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        Self {
            x: x0,
            y: y0,
            w: x1 - x0 + 1,
            h: y1 - y0 + 1,
        }
    }

    pub fn right(self) -> u32 {
        self.x + self.w
    }

    pub fn bottom(self) -> u32 {
        self.y + self.h
    }

    pub fn area(self) -> usize {
        self.w as usize * self.h as usize
    }

    /// 幅 `width` の画像の中で、この矩形に入る画素の添字。
    pub fn indices(self, width: u32) -> impl Iterator<Item = usize> {
        let (x, w, width) = (self.x as usize, self.w as usize, width as usize);
        (self.y..self.bottom())
            .map(move |row| row as usize * width + x)
            .flat_map(move |start| start..start + w)
    }

    /// `width` × `height` の画像からはみ出す部分を切り落とす。全部外なら `None`。
    pub fn clipped(self, width: u32, height: u32) -> Option<Self> {
        let right = self.right().min(width);
        let bottom = self.bottom().min(height);
        (self.x < right && self.y < bottom).then(|| Self {
            x: self.x,
            y: self.y,
            w: right - self.x,
            h: bottom - self.y,
        })
    }
}

/// 矩形の中の画素の平均と標準偏差（母標準偏差）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub mean: f64,
    pub std_dev: f64,
}

pub fn stats(img: &Image<'_>, region: Region) -> Option<Stats> {
    let region = region.clipped(img.width, img.height)?;
    let (mut sum, mut sum_sq) = (0.0f64, 0.0f64);
    for i in region.indices(img.width) {
        let v = f64::from(img.pixels[i]);
        sum += v;
        sum_sq += v * v;
    }
    let n = region.area() as f64;
    let mean = sum / n;
    let var = (sum_sq / n - mean * mean).max(0.0);
    Some(Stats {
        mean,
        std_dev: var.sqrt(),
    })
}

/// PSNR（dB）。`peak` は画素の取りうる最大値（8bit の表示なら 255）。
/// 2 枚が同じなら無限大。大きさが違えば `None`。
pub fn psnr(a: &Image<'_>, b: &Image<'_>, peak: f32) -> Option<f64> {
    if !a.same_size(b) || a.pixels.is_empty() {
        return None;
    }
    let mse = a
        .pixels
        .iter()
        .zip(b.pixels)
        .map(|(p, q)| {
            let d = f64::from(p - q);
            d * d
        })
        .sum::<f64>()
        / a.pixels.len() as f64;
    Some(if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (f64::from(peak) * f64::from(peak) / mse).log10()
    })
}

/// SSIM（Wang ら 2004 の式、11×11 のガウス窓、σ = 1.5）。大きさが違えば `None`。
///
/// 元の実装と同じく、短辺が 256 画素程度になるまで整数倍で縮小してから求める。
/// 画素ごとに 5 枚の写像を持つと大きな画像でメモリを食う上に、SSIM は縮小した
/// 解像度で見るのが本来の使い方（視距離に合わせる）だから。
pub fn ssim(a: &Image<'_>, b: &Image<'_>, peak: f32) -> Option<f64> {
    if !a.same_size(b) || a.width == 0 || a.height == 0 {
        return None;
    }
    let factor = (a.width.min(a.height) / 256).max(1);
    let (a, w, h) = shrink(a, factor);
    let (b, _, _) = shrink(b, factor);
    let blur = |v: &[f32]| gaussian(v, w, h);
    let mu_a = blur(&a);
    let mu_b = blur(&b);
    let aa = blur(&a.iter().map(|p| p * p).collect::<Vec<_>>());
    let bb = blur(&b.iter().map(|p| p * p).collect::<Vec<_>>());
    let ab = blur(&a.iter().zip(&b).map(|(p, q)| p * q).collect::<Vec<_>>());
    let c1 = (0.01 * f64::from(peak)).powi(2);
    let c2 = (0.03 * f64::from(peak)).powi(2);
    let margin = GAUSSIAN.len() / 2;
    if w <= 2 * margin || h <= 2 * margin {
        // 窓が収まらないほど小さい。中心の 1 画素ぶんの窓で見る。
        return Some(ssim_at(mu_a[0], mu_b[0], aa[0], bb[0], ab[0], c1, c2));
    }
    let mut sum = 0.0;
    let mut n = 0usize;
    for y in margin..h - margin {
        for x in margin..w - margin {
            let i = y * w + x;
            sum += ssim_at(mu_a[i], mu_b[i], aa[i], bb[i], ab[i], c1, c2);
            n += 1;
        }
    }
    Some(sum / n as f64)
}

fn ssim_at(mu_a: f32, mu_b: f32, aa: f32, bb: f32, ab: f32, c1: f64, c2: f64) -> f64 {
    let (mu_a, mu_b) = (f64::from(mu_a), f64::from(mu_b));
    let var_a = f64::from(aa) - mu_a * mu_a;
    let var_b = f64::from(bb) - mu_b * mu_b;
    let cov = f64::from(ab) - mu_a * mu_b;
    ((2.0 * mu_a * mu_b + c1) * (2.0 * cov + c2))
        / ((mu_a * mu_a + mu_b * mu_b + c1) * (var_a + var_b + c2))
}

/// σ = 1.5 の 11 点のガウス窓（正規化済み）。
const GAUSSIAN: [f32; 11] = [
    0.001_028_4,
    0.007_598_8,
    0.036_000_8,
    0.109_359_3,
    0.213_005_2,
    0.266_015_2,
    0.213_005_2,
    0.109_359_3,
    0.036_000_8,
    0.007_598_8,
    0.001_028_4,
];

/// 縦横に分けたガウスぼかし。端は画像の外を 0 として扱う（縁は SSIM の平均から除く）。
fn gaussian(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    let r = GAUSSIAN.len() / 2;
    let mut rows = vec![0.0f32; w * h];
    for y in 0..h {
        let row = &v[y * w..(y + 1) * w];
        for x in 0..w {
            let mut s = 0.0;
            for (k, g) in GAUSSIAN.iter().enumerate() {
                if let Some(px) = (x + k).checked_sub(r).filter(|&px| px < w) {
                    s += row[px] * g;
                }
            }
            rows[y * w + x] = s;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for (k, g) in GAUSSIAN.iter().enumerate() {
            let Some(py) = (y + k).checked_sub(r).filter(|&py| py < h) else {
                continue;
            };
            let src = &rows[py * w..(py + 1) * w];
            let dst = &mut out[y * w..(y + 1) * w];
            for (d, s) in dst.iter_mut().zip(src) {
                *d += s * g;
            }
        }
    }
    out
}

/// `factor` × `factor` の平均で縮小する。
fn shrink(img: &Image<'_>, factor: u32) -> (Vec<f32>, usize, usize) {
    if factor <= 1 {
        return (img.pixels.to_vec(), img.width as usize, img.height as usize);
    }
    let (w, h) = (
        (img.width / factor) as usize,
        (img.height / factor) as usize,
    );
    let f = factor as usize;
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for dy in 0..f {
                for dx in 0..f {
                    s += img.at(x * f + dx, y * f + dy);
                }
            }
            out.push(s / (f * f) as f32);
        }
    }
    (out, w, h)
}

/// SNR = 対象 ROI の平均 / 標準偏差。ばらつきがなければ無限大。
pub fn snr(img: &Image<'_>, target: Region) -> Option<f64> {
    let s = stats(img, target)?;
    Some(ratio(s.mean.abs(), s.std_dev))
}

/// CNR = |対象の平均 − 背景の平均| / 背景の標準偏差。
pub fn cnr(img: &Image<'_>, target: Region, background: Region) -> Option<f64> {
    let t = stats(img, target)?;
    let b = stats(img, background)?;
    Some(ratio((t.mean - b.mean).abs(), b.std_dev))
}

fn ratio(num: f64, den: f64) -> f64 {
    if den == 0.0 {
        f64::INFINITY
    } else {
        num / den
    }
}

/// 鮮鋭度 = ラプラシアン（4 近傍）の分散。縁の 1 画素は除く。ぼけるほど小さい。
pub fn laplacian_variance(img: &Image<'_>) -> Option<f64> {
    let (w, h) = (img.width as usize, img.height as usize);
    if w < 3 || h < 3 {
        return None;
    }
    let (mut sum, mut sum_sq, mut n) = (0.0f64, 0.0f64, 0usize);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let lap = 4.0 * img.at(x, y)
                - img.at(x - 1, y)
                - img.at(x + 1, y)
                - img.at(x, y - 1)
                - img.at(x, y + 1);
            let lap = f64::from(lap);
            sum += lap;
            sum_sq += lap * lap;
            n += 1;
        }
    }
    let mean = sum / n as f64;
    Some((sum_sq / n as f64 - mean * mean).max(0.0))
}

/// 評価する指標。どれも値が大きいほど良い。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Metric {
    Psnr,
    Ssim,
    Snr,
    Cnr,
    Sharpness,
}

impl Metric {
    pub const ALL: [Metric; 5] = [
        Metric::Psnr,
        Metric::Ssim,
        Metric::Snr,
        Metric::Cnr,
        Metric::Sharpness,
    ];

    /// 設定ファイルや CSV で使う名前。
    pub fn key(self) -> &'static str {
        match self {
            Metric::Psnr => "psnr",
            Metric::Ssim => "ssim",
            Metric::Snr => "snr",
            Metric::Cnr => "cnr",
            Metric::Sharpness => "sharpness",
        }
    }

    pub fn unit(self) -> Option<&'static str> {
        match self {
            Metric::Psnr => Some("dB"),
            _ => None,
        }
    }

    /// 基準の画像と比べる指標（基準がないと出せない）。
    pub fn needs_reference(self) -> bool {
        matches!(self, Metric::Psnr | Metric::Ssim)
    }

    /// 対象 ROI が要る。
    pub fn needs_target(self) -> bool {
        matches!(self, Metric::Snr | Metric::Cnr)
    }

    /// 背景 ROI が要る。
    pub fn needs_background(self) -> bool {
        matches!(self, Metric::Cnr)
    }

    /// 画面に出すときの桁数。
    pub fn format(self, v: f64) -> String {
        if v.is_infinite() {
            return "∞".to_owned();
        }
        match self {
            Metric::Ssim => format!("{v:.4}"),
            Metric::Psnr | Metric::Snr | Metric::Cnr => format!("{v:.2}"),
            Metric::Sharpness => format!("{v:.0}"),
        }
    }
}

/// 判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Good,
    Caution,
    Degraded,
}

/// 判定の境目。`good` 以上なら適合、`caution` 以上なら注意、それ未満は劣化。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    pub good: f64,
    pub caution: f64,
}

impl Thresholds {
    pub fn valid(&self) -> bool {
        self.good.is_finite() && self.caution.is_finite() && self.good >= self.caution
    }

    pub fn verdict(&self, value: f64) -> Verdict {
        if value >= self.good {
            Verdict::Good
        } else if value >= self.caution {
            Verdict::Caution
        } else {
            Verdict::Degraded
        }
    }

    /// 帯の長さ（0..=1）。適合の境目で 3/4、その倍で満杯になるようにし、閾値の違う指標を同じ帯で見せる。
    pub fn bar(&self, value: f64) -> f32 {
        if value.is_infinite() {
            return 1.0;
        }
        let full = self.good * 1.5;
        if full <= 0.0 {
            return if value >= self.good { 1.0 } else { 0.0 };
        }
        (value / full).clamp(0.0, 1.0) as f32
    }
}

impl Metric {
    /// 既定の境目。8bit の表示の明るさ（0..=255）で測ったときの目安。
    pub fn default_thresholds(self) -> Thresholds {
        match self {
            Metric::Psnr => Thresholds {
                good: 35.0,
                caution: 25.0,
            },
            Metric::Ssim => Thresholds {
                good: 0.95,
                caution: 0.85,
            },
            Metric::Snr => Thresholds {
                good: 10.0,
                caution: 5.0,
            },
            Metric::Cnr => Thresholds {
                good: 3.0,
                caution: 1.5,
            },
            Metric::Sharpness => Thresholds {
                good: 500.0,
                caution: 100.0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Vec<f32> {
        (0..w * h)
            .map(|i| (i % w) as f32 * 255.0 / (w - 1) as f32)
            .collect()
    }

    #[test]
    fn corners_in_any_order_give_the_same_inclusive_region() {
        let a = Region::from_corners((5, 7), (2, 3));
        let b = Region::from_corners((2, 3), (5, 7));
        assert_eq!(a, b);
        assert_eq!((a.x, a.y, a.w, a.h), (2, 3, 4, 5));
        assert_eq!((a.right(), a.bottom(), a.area()), (6, 8, 20));
    }

    #[test]
    fn indices_walk_the_rows_inside_the_region() {
        let r = Region {
            x: 1,
            y: 1,
            w: 2,
            h: 2,
        };
        assert_eq!(r.indices(4).collect::<Vec<_>>(), vec![5, 6, 9, 10]);
    }

    #[test]
    fn region_outside_the_image_is_clipped_or_dropped() {
        let r = Region {
            x: 3,
            y: 2,
            w: 10,
            h: 10,
        };
        assert_eq!(
            r.clipped(5, 4),
            Some(Region {
                x: 3,
                y: 2,
                w: 2,
                h: 2
            })
        );
        assert_eq!(r.clipped(3, 4), None);
    }

    #[test]
    fn stats_give_mean_and_population_std_dev() {
        let img = Image::new(2, 2, &[10.0, 20.0, 30.0, 40.0]);
        let s = stats(
            &img,
            Region {
                x: 0,
                y: 0,
                w: 2,
                h: 1,
            },
        )
        .unwrap();
        assert_eq!((s.mean, s.std_dev), (15.0, 5.0));
    }

    #[test]
    fn identical_images_have_infinite_psnr_and_ssim_one() {
        let px = gradient(64, 48);
        let img = Image::new(64, 48, &px);
        assert_eq!(psnr(&img, &img, 255.0), Some(f64::INFINITY));
        let s = ssim(&img, &img, 255.0).unwrap();
        assert!((s - 1.0).abs() < 1e-6, "{s}");
    }

    #[test]
    fn psnr_matches_the_mean_squared_error_formula() {
        let a = Image::new(2, 1, &[0.0, 0.0]);
        let b = Image::new(2, 1, &[3.0, 4.0]);
        // MSE = (9 + 16) / 2 = 12.5, PSNR = 10 log10(255² / 12.5)
        let expected = 10.0 * (255.0f64 * 255.0 / 12.5).log10();
        assert!((psnr(&a, &b, 255.0).unwrap() - expected).abs() < 1e-9);
    }

    #[test]
    fn noise_lowers_ssim_and_different_sizes_give_none() {
        let px = gradient(64, 48);
        let noisy: Vec<f32> = px
            .iter()
            .enumerate()
            .map(|(i, p)| p + if i % 2 == 0 { 20.0 } else { -20.0 })
            .collect();
        let a = Image::new(64, 48, &px);
        let b = Image::new(64, 48, &noisy);
        let s = ssim(&a, &b, 255.0).unwrap();
        assert!(s < 0.9, "{s}");
        let c = Image::new(48, 64, &px);
        assert_eq!(ssim(&a, &c, 255.0), None);
        assert_eq!(psnr(&a, &c, 255.0), None);
    }

    #[test]
    fn big_images_are_shrunk_before_ssim_but_identical_ones_still_score_one() {
        let px = gradient(600, 520);
        let img = Image::new(600, 520, &px);
        assert!((ssim(&img, &img, 255.0).unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn snr_and_cnr_follow_the_roi_statistics() {
        let px = [10.0, 20.0, 100.0, 100.0];
        let img = Image::new(4, 1, &px);
        let background = Region {
            x: 0,
            y: 0,
            w: 2,
            h: 1,
        };
        let target = Region {
            x: 2,
            y: 0,
            w: 2,
            h: 1,
        };
        assert_eq!(snr(&img, background), Some(3.0));
        assert_eq!(snr(&img, target), Some(f64::INFINITY));
        assert_eq!(cnr(&img, target, background), Some(17.0));
        assert_eq!(
            snr(
                &img,
                Region {
                    x: 9,
                    y: 0,
                    w: 1,
                    h: 1
                }
            ),
            None
        );
    }

    #[test]
    fn blurred_image_has_lower_laplacian_variance() {
        let sharp: Vec<f32> = (0..16 * 16)
            .map(|i| if (i % 16) < 8 { 0.0 } else { 255.0 })
            .collect();
        let soft: Vec<f32> = (0..16 * 16)
            .map(|i| (i % 16) as f32 * 255.0 / 15.0)
            .collect();
        let a = Image::new(16, 16, &sharp);
        let b = Image::new(16, 16, &soft);
        let (va, vb) = (
            laplacian_variance(&a).unwrap(),
            laplacian_variance(&b).unwrap(),
        );
        assert!(va > vb * 10.0, "{va} {vb}");
        assert_eq!(laplacian_variance(&Image::new(2, 2, &[0.0; 4])), None);
    }

    #[test]
    fn thresholds_split_values_into_three_verdicts() {
        let t = Thresholds {
            good: 30.0,
            caution: 20.0,
        };
        assert_eq!(t.verdict(30.0), Verdict::Good);
        assert_eq!(t.verdict(29.9), Verdict::Caution);
        assert_eq!(t.verdict(19.9), Verdict::Degraded);
        assert_eq!(t.verdict(f64::INFINITY), Verdict::Good);
        assert!(!Thresholds {
            good: 1.0,
            caution: 2.0
        }
        .valid());
        assert_eq!(t.bar(45.0), 1.0);
        assert_eq!(t.bar(0.0), 0.0);
        assert!((t.bar(30.0) - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn metrics_format_with_their_own_precision() {
        assert_eq!(Metric::Ssim.format(0.98765), "0.9877");
        assert_eq!(Metric::Psnr.format(f64::INFINITY), "∞");
        assert_eq!(Metric::Sharpness.format(2246.6), "2247");
        for m in Metric::ALL {
            assert!(m.default_thresholds().valid(), "{}", m.key());
        }
    }
}
