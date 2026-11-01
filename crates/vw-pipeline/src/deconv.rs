//! デコンボリューション（光学系と大気で星がにじんだのを戻す）と、写っている星からの PSF の測定。
//!
//! PSF はガウスとみなす。Moffat などの裾の長い形や、星の切り出しをそのまま PSF にするやり方を
//! 採らないのは、裾は背景の雑音に埋もれて測りにくく、切り出しは雑音ごと戻してしまうため。
//! ガウスなら畳み込みを横と縦に分けて速くでき（[`crate::nodes::blur`]）、Wiener も式で書ける。

use rustfft::FftPlanner;
use rustfft::num_complex::Complex32;
use serde::{Deserialize, Serialize};

use crate::astro::{NOISE, fill_nan, for_each_plane, noise_sigma, starlet};
use crate::nodes::{Float, blur};

/// ガウスの σ に対する FWHM の比（2√(2 ln 2)）。
pub const FWHM_PER_SIGMA: f32 = 2.354_82;

/// PSF を測る星を見つけるしきい値（雑音の何倍か）。星の縮小・除去より高めにして、
/// 雑音の山を星と取り違えないようにする。
const MEASURE_THRESHOLD: f32 = 5.0;
/// PSF を決めるのに要る星の数。これより少なければ手で入れた FWHM を使う。
const MIN_STARS: usize = 3;

/// デコンボリューションのやり方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    /// 繰り返しで、値が負にならない。星の芯をよく締める。全変動で正則化できる。
    #[default]
    RichardsonLucy,
    /// 周波数の上で一度に割る。速いが、明るい星の周りに輪が出やすい。
    Wiener,
    /// 残差を足していく繰り返し。RL より穏やかで、背景の雑音を増やしにくい。
    Landweber,
}

impl Algorithm {
    pub const ALL: [Algorithm; 3] = [
        Algorithm::RichardsonLucy,
        Algorithm::Wiener,
        Algorithm::Landweber,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Algorithm::RichardsonLucy => "Richardson–Lucy",
            Algorithm::Wiener => "Wiener",
            Algorithm::Landweber => "Landweber",
        }
    }

    /// 繰り返しの回数を使うか。
    pub fn iterative(self) -> bool {
        !matches!(self, Algorithm::Wiener)
    }
}

/// デコンボリューションの値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Deconvolution {
    pub algorithm: Algorithm,
    /// PSF のガウスの σ（画素）。
    pub sigma: f32,
    pub iterations: u32,
    /// Richardson–Lucy の全変動の重み（0 で正則化なし）。
    pub tv: f32,
    /// Wiener の雑音と信号の比。大きいほど控えめに戻す。
    pub noise_ratio: f32,
}

/// チャンネルごとにデコンボリューションをかける。
pub fn deconvolve(img: &mut Float, d: Deconvolution) {
    if d.sigma <= 0.05 || img.data.is_empty() {
        return;
    }
    for_each_plane(img, |plane, w, h| match d.algorithm {
        Algorithm::RichardsonLucy => richardson_lucy(plane, w, h, d.sigma, d.iterations, d.tv),
        Algorithm::Wiener => wiener(plane, w, h, d.sigma, d.noise_ratio),
        Algorithm::Landweber => landweber(plane, w, h, d.sigma, d.iterations),
    });
}

fn blurred(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let mut f = Float::new(w, h, 1, plane.to_vec());
    blur(&mut f, sigma);
    f.data
}

fn range(plane: &[f32]) -> (f32, f32) {
    plane
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        })
}

/// Richardson–Lucy。`u ← u × H(d / Hu)`（ガウスは左右対称なので転置も同じぼかし）。
/// 全変動の項（Dey ほか 2006）は `u` を `1 − λ div(∇u / |∇u|)` で割る。
fn richardson_lucy(plane: &mut [f32], w: usize, h: usize, sigma: f32, iterations: u32, tv: f32) {
    let (lo, hi) = range(plane);
    let span = (hi - lo).max(f32::MIN_POSITIVE);
    // RL は正の値でしか成り立たないので、いちばん暗い所が少しだけ正になるよう持ち上げる。
    // 線形の値（FITS の物理量）は負や大きな背景を持つことがあるため。
    let offset = lo - span * 1e-3;
    let tiny = span * 1e-6;
    let data: Vec<f32> = plane.iter().map(|v| v - offset).collect();
    let mut u = data.clone();
    for _ in 0..iterations {
        let estimate = blurred(&u, w, h, sigma);
        let ratio: Vec<f32> = data
            .iter()
            .zip(&estimate)
            .map(|(d, e)| d / e.max(tiny))
            .collect();
        let correction = blurred(&ratio, w, h, sigma);
        let divergence = (tv > 0.0).then(|| tv_divergence(&u, w, h, span * 1e-3));
        for (i, v) in u.iter_mut().enumerate() {
            let mut f = correction[i];
            if let Some(div) = &divergence {
                f /= (1.0 - tv * div[i]).max(0.1);
            }
            *v = (*v * f).max(tiny);
        }
    }
    for (p, v) in plane.iter_mut().zip(&u) {
        *p = v + offset;
    }
}

/// `div(∇u / |∇u|)`。前進差分で勾配を取り、後退差分で発散を取る。`eps` は平らな所で割り算が
/// 暴れないための下駄。
fn tv_divergence(u: &[f32], w: usize, h: usize, eps: f32) -> Vec<f32> {
    let mut nx = vec![0.0f32; w * h];
    let mut ny = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let gx = if x + 1 < w { u[i + 1] - u[i] } else { 0.0 };
            let gy = if y + 1 < h { u[i + w] - u[i] } else { 0.0 };
            let norm = (gx * gx + gy * gy + eps * eps).sqrt();
            nx[i] = gx / norm;
            ny[i] = gy / norm;
        }
    }
    let mut div = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let dx = nx[i] - if x > 0 { nx[i - 1] } else { 0.0 };
            let dy = ny[i] - if y > 0 { ny[i - w] } else { 0.0 };
            div[i] = dx + dy;
        }
    }
    div
}

/// Landweber。`u ← u + τ H(d − Hu)`。暗い側はもとの最小値で止める（値を負に振らせない）。
fn landweber(plane: &mut [f32], w: usize, h: usize, sigma: f32, iterations: u32) {
    // ガウスの H は固有値が 0〜1 なので、τ < 2 なら収束する。速さと安定の間をとって 1.5。
    const STEP: f32 = 1.5;
    let (lo, _) = range(plane);
    let data = plane.to_vec();
    for _ in 0..iterations {
        let estimate = blurred(plane, w, h, sigma);
        let residual: Vec<f32> = data.iter().zip(&estimate).map(|(d, e)| d - e).collect();
        let back = blurred(&residual, w, h, sigma);
        for (v, r) in plane.iter_mut().zip(&back) {
            *v = (*v + STEP * r).max(lo);
        }
    }
}

/// Wiener。周波数の上で `D × H / (H² + K)`。端の折り返しで縁に縞が出ないよう、鏡に映して
/// 広げてから変換する。平均を引いてから割るのは、K で全体が暗くならないようにするため。
fn wiener(plane: &mut [f32], w: usize, h: usize, sigma: f32, noise_ratio: f32) {
    let k = noise_ratio.max(1e-6);
    let pad = (sigma * 4.0).ceil() as usize + 1;
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let mean = plane.iter().map(|&v| f64::from(v)).sum::<f64>() / plane.len() as f64;
    let mean = mean as f32;
    let at = |i: usize, len: usize| -> usize {
        let i = i as isize - pad as isize;
        mirror(i, len as isize)
    };
    let mut grid: Vec<Complex32> = (0..pw * ph)
        .map(|i| {
            let (x, y) = (at(i % pw, w), at(i / pw, h));
            Complex32::new(plane[y * w + x] - mean, 0.0)
        })
        .collect();
    let mut planner = FftPlanner::<f32>::new();
    fft_2d(&mut grid, pw, ph, &mut planner, false);
    let freq = |k: usize, n: usize| -> f32 {
        let k = if k > n / 2 {
            k as f32 - n as f32
        } else {
            k as f32
        };
        k / n as f32
    };
    let two_pi2_s2 = 2.0 * std::f32::consts::PI.powi(2) * sigma * sigma;
    for (i, c) in grid.iter_mut().enumerate() {
        let (fx, fy) = (freq(i % pw, pw), freq(i / pw, ph));
        let otf = (-two_pi2_s2 * (fx * fx + fy * fy)).exp();
        *c *= otf / (otf * otf + k);
    }
    fft_2d(&mut grid, pw, ph, &mut planner, true);
    let scale = 1.0 / (pw * ph) as f32;
    for y in 0..h {
        for x in 0..w {
            plane[y * w + x] = grid[(y + pad) * pw + x + pad].re * scale + mean;
        }
    }
}

fn mirror(i: isize, len: isize) -> usize {
    if len <= 1 {
        return 0;
    }
    let period = 2 * (len - 1);
    let m = i.rem_euclid(period);
    (if m >= len { period - m } else { m }) as usize
}

/// 2 次元の FFT（行ごとに変換し、転置して列も変換する）。
fn fft_2d(
    grid: &mut Vec<Complex32>,
    w: usize,
    h: usize,
    planner: &mut FftPlanner<f32>,
    inverse: bool,
) {
    let plan = |n: usize, planner: &mut FftPlanner<f32>| {
        if inverse {
            planner.plan_fft_inverse(n)
        } else {
            planner.plan_fft_forward(n)
        }
    };
    let rows = plan(w, planner);
    by_lines(grid, w, |line| rows.process(line));
    let mut columns = transpose(grid, w, h);
    let cols = plan(h, planner);
    by_lines(&mut columns, h, |line| cols.process(line));
    *grid = transpose(&columns, h, w);
}

fn by_lines(data: &mut [Complex32], len: usize, f: impl Fn(&mut [Complex32]) + Sync) {
    let lines = data.len() / len;
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(data.len().div_ceil(1 << 16))
        .max(1);
    let per = lines.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for chunk in data.chunks_mut(per * len) {
            let f = &f;
            scope.spawn(move || {
                for line in chunk.chunks_exact_mut(len) {
                    f(line);
                }
            });
        }
    });
}

fn transpose<T: Copy + Default>(data: &[T], w: usize, h: usize) -> Vec<T> {
    let mut out = vec![T::default(); w * h];
    for y in 0..h {
        for x in 0..w {
            out[x * h + y] = data[y * w + x];
        }
    }
    out
}

/// 星から測った PSF。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measured {
    /// 画素。
    pub fwhm: f32,
    /// 測るのに使った星の数。
    pub stars: usize,
}

/// 写っている星から PSF の FWHM（画素）を測る。星が [`MIN_STARS`] 個より少なければ `None`。
///
/// 星ごとに、ガウスの重みを星の幅に合わせながら 2 次のモーメントを取る（適応モーメント）。
/// 重みなしのモーメントは裾の雑音で大きく振れ、半値の面積は画素の粗さに引っ張られるため。
/// 重みの幅が星の σ と一致したとき、重み付きの分散は σ² / 2 になる。
pub fn measure_fwhm(img: &Float) -> Option<Measured> {
    let mut sigmas: Vec<f32> = find_stars(img)?
        .into_iter()
        .filter(|s| s.ellipticity < 0.3)
        .map(|s| s.sigma)
        .collect();
    if sigmas.len() < MIN_STARS {
        return None;
    }
    let stars = sigmas.len();
    crate::astro::median(&mut sigmas).map(|s| Measured {
        fwhm: s * FWHM_PER_SIGMA,
        stars,
    })
}

/// 1 つの星の形（適応モーメントで測ったもの）。位置は画素の中心を整数にした座標。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub x: f32,
    pub y: f32,
    /// ガウスの σ（画素）。FWHM はこれに [`FWHM_PER_SIGMA`] を掛けたもの。
    pub sigma: f32,
    /// (長軸² − 短軸²) / (長軸² + 短軸²)。丸い星で 0。
    pub ellipticity: f32,
    /// 背景を引いた明るさの和（重み付き）。明るい星を選ぶのに使う。
    pub flux: f32,
}

/// 写っている星を探して形を測る。画像が小さすぎるか、星らしいものが 1 つもなければ `None`。
/// 端にかかるもの、測る途中で外れたものは除く。丸くないもの（重なった星や流れた星）は残す。
pub fn find_stars(img: &Float) -> Option<Vec<Shape>> {
    let (w, h, n) = (img.width, img.height, img.channels);
    if w < 16 || h < 16 {
        return None;
    }
    let mut luma: Vec<f32> = img
        .data
        .chunks_exact(n)
        .map(|px| px.iter().sum::<f32>() / n as f32)
        .collect();
    fill_nan(&mut luma);
    let (details, _) = starlet(&luma, w, h);
    let detail: Vec<f32> = (0..w * h)
        .map(|i| details[0][i] + details[1][i] + details[2][i])
        .collect();
    let sigma = noise_sigma(&details[0]);
    let noise = (NOISE[0] * NOISE[0] + NOISE[1] * NOISE[1] + NOISE[2] * NOISE[2]).sqrt();
    // 雑音のない画像（作った画像など）でも、星の裾の小さな揺れを星にしないための下限。
    let peak = detail.iter().copied().fold(0.0f32, f32::max);
    let limit = (MEASURE_THRESHOLD * sigma * noise).max(peak * 0.05);
    if limit <= 0.0 {
        return None;
    }
    Some(
        components(&detail, w, h, limit)
            .iter()
            .filter_map(|c| star_shape(&luma, w, h, c))
            .collect(),
    )
}

/// しきい値を超えた画素のつながり（8 近傍）。`(画素の数, いちばん明るい細部の位置)`。
fn components(detail: &[f32], w: usize, h: usize, limit: f32) -> Vec<(usize, usize)> {
    // 大きすぎるものは星雲や銀河の明るい所。PSF を測る相手ではない。
    const MAX_AREA: usize = 2000;
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if seen[start] || detail[start] <= limit {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut area, mut best) = (0usize, start);
        while let Some(i) = stack.pop() {
            area += 1;
            if detail[i] > detail[best] {
                best = i;
            }
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !seen[j] && detail[j] > limit {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if (2..=MAX_AREA).contains(&area) {
            out.push((area, best));
        }
    }
    out
}

/// 1 つの星の形。端にかかる、収まらないものは `None`。
fn star_shape(luma: &[f32], w: usize, h: usize, &(area, at): &(usize, usize)) -> Option<Shape> {
    let mut s = ((area as f32 / std::f32::consts::PI).sqrt() / 1.5).max(0.8);
    let (mut cx, mut cy) = ((at % w) as f32, (at / w) as f32);
    let ring = ((s * 5.0).ceil() as usize + 3).min(40);
    let background = ring_median(luma, w, h, cx as usize, cy as usize, ring)?;
    let (mut mxx, mut myy, mut mxy, mut flux) = (0.0, 0.0, 0.0, 0.0);
    for _ in 0..30 {
        let r = (s * 4.0).ceil() as isize + 1;
        let (x0, y0) = (cx.round() as isize, cy.round() as isize);
        if x0 - r < 0 || y0 - r < 0 || x0 + r >= w as isize || y0 + r >= h as isize {
            return None;
        }
        let (mut sum, mut sx, mut sy, mut sxx, mut syy, mut sxy) =
            (0.0f32, 0.0, 0.0, 0.0, 0.0, 0.0);
        for y in y0 - r..=y0 + r {
            for x in x0 - r..=x0 + r {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let weight = (-(dx * dx + dy * dy) / (2.0 * s * s)).exp();
                let v = (luma[y as usize * w + x as usize] - background) * weight;
                sum += v;
                sx += v * dx;
                sy += v * dy;
                sxx += v * dx * dx;
                syy += v * dy * dy;
                sxy += v * dx * dy;
            }
        }
        if sum <= 0.0 {
            return None;
        }
        flux = sum;
        let (ox, oy) = (sx / sum, sy / sum);
        mxx = sxx / sum - ox * ox;
        myy = syy / sum - oy * oy;
        mxy = sxy / sum - ox * oy;
        cx += ox;
        cy += oy;
        let next = (mxx + myy).max(0.0).sqrt();
        if !next.is_finite() || !(0.3..=30.0).contains(&next) {
            return None;
        }
        let done = (next - s).abs() < 1e-3;
        s = next;
        if done {
            break;
        }
    }
    let ellipticity = ((mxx - myy).powi(2) + 4.0 * mxy * mxy).sqrt() / (mxx + myy);
    Some(Shape {
        x: cx,
        y: cy,
        sigma: s,
        ellipticity,
        flux,
    })
}

/// `(x, y)` を中心とする一辺 `2r + 1` の正方形の縁の画素の中央値（星の周りの背景）。
fn ring_median(luma: &[f32], w: usize, h: usize, x: usize, y: usize, r: usize) -> Option<f32> {
    if x < r || y < r || x + r >= w || y + r >= h {
        return None;
    }
    let mut values = Vec::with_capacity(8 * r);
    for i in 0..=2 * r {
        values.push(luma[(y - r) * w + x - r + i]);
        values.push(luma[(y + r) * w + x - r + i]);
        values.push(luma[(y - r + i) * w + x - r]);
        values.push(luma[(y - r + i) * w + x + r]);
    }
    crate::astro::median(&mut values)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 決まった種の、ほぼ正規分布の乱数（一様乱数 12 個の和から 6 を引く）。
    fn noise(n: usize, seed: u64, sigma: f32) -> Vec<f32> {
        let mut s = seed | 1;
        let mut uniform = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 40) as f32 / (1u32 << 24) as f32
        };
        (0..n)
            .map(|_| ((0..12).map(|_| uniform()).sum::<f32>() - 6.0) * sigma)
            .collect()
    }

    /// 背景 `sky` に σ `width` の星を格子状に並べた灰色の画像。
    fn star_field(size: usize, width: f32, sky: f32, noise_sigma: f32) -> Float {
        let n = noise(size * size, 17, noise_sigma);
        let stars: Vec<(f32, f32, f32)> = (0..5)
            .flat_map(|j| {
                (0..5).map(move |i| {
                    let x = 20.0 + i as f32 * (size as f32 - 40.0) / 4.0 + 0.3 * j as f32;
                    let y = 20.0 + j as f32 * (size as f32 - 40.0) / 4.0 + 0.2 * i as f32;
                    (x, y, 30.0 + 20.0 * ((i + j) % 3) as f32)
                })
            })
            .collect();
        let data = (0..size * size)
            .map(|i| {
                let (x, y) = ((i % size) as f32, (i / size) as f32);
                let light: f32 = stars
                    .iter()
                    .map(|&(sx, sy, a)| {
                        let d2 = (x - sx).powi(2) + (y - sy).powi(2);
                        a * (-d2 / (2.0 * width * width)).exp()
                    })
                    .sum();
                sky + light + n[i]
            })
            .collect();
        Float::new(size, size, 1, data)
    }

    /// 星の芯の明るさと、芯から 3 画素離れた所の明るさの比（大きいほど締まっている）。
    fn sharpness(img: &Float, x: usize, y: usize) -> f32 {
        let at = |dx: usize| img.data[y * img.width + x + dx];
        (at(0) - 100.0) / (at(3) - 100.0).max(0.01)
    }

    #[test]
    fn the_fwhm_is_measured_from_the_stars() {
        for width in [1.5f32, 2.5] {
            let img = star_field(240, width, 100.0, 0.5);
            let fwhm = measure_fwhm(&img).unwrap().fwhm;
            let expected = width * FWHM_PER_SIGMA;
            assert!(
                (fwhm - expected).abs() < expected * 0.1,
                "σ {width}: {fwhm} vs {expected}"
            );
        }
    }

    #[test]
    fn an_image_without_stars_has_no_fwhm() {
        let img = Float::new(64, 64, 1, noise(64 * 64, 3, 1.0));
        assert_eq!(measure_fwhm(&img), None);
    }

    #[test]
    fn every_algorithm_tightens_the_stars_and_keeps_the_sky() {
        for algorithm in Algorithm::ALL {
            let mut img = star_field(160, 2.0, 100.0, 0.05);
            let before = sharpness(&img, 20, 20);
            let sky_before = img.data[35 * 160 + 35];
            deconvolve(
                &mut img,
                Deconvolution {
                    algorithm,
                    sigma: 2.0,
                    iterations: 20,
                    tv: 0.0,
                    noise_ratio: 0.01,
                },
            );
            let after = sharpness(&img, 20, 20);
            assert!(after > before * 1.3, "{algorithm:?}: {before} -> {after}");
            let sky_after = img.data[35 * 160 + 35];
            assert!(
                (sky_after - sky_before).abs() < 1.0,
                "{algorithm:?}: {sky_before} -> {sky_after}"
            );
        }
    }

    #[test]
    fn richardson_lucy_keeps_the_total_light() {
        let mut img = star_field(120, 2.0, 10.0, 0.0);
        let total: f32 = img.data.iter().sum();
        deconvolve(
            &mut img,
            Deconvolution {
                algorithm: Algorithm::RichardsonLucy,
                sigma: 2.0,
                iterations: 10,
                tv: 0.0,
                noise_ratio: 0.0,
            },
        );
        let after: f32 = img.data.iter().sum();
        assert!((after - total).abs() < total * 0.01, "{total} -> {after}");
    }

    #[test]
    fn total_variation_keeps_the_background_noise_from_growing() {
        let run = |tv: f32| {
            let mut img = star_field(120, 2.0, 100.0, 1.0);
            deconvolve(
                &mut img,
                Deconvolution {
                    algorithm: Algorithm::RichardsonLucy,
                    sigma: 2.0,
                    iterations: 20,
                    tv,
                    noise_ratio: 0.0,
                },
            );
            let sky: Vec<f32> = img.data[..120 * 8].to_vec();
            let mean = sky.iter().sum::<f32>() / sky.len() as f32;
            sky.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / sky.len() as f32
        };
        assert!(run(0.01) < run(0.0), "{} vs {}", run(0.01), run(0.0));
    }

    #[test]
    fn nan_pixels_stay_nan() {
        let mut img = star_field(64, 2.0, 100.0, 0.1);
        img.data[5] = f32::NAN;
        deconvolve(
            &mut img,
            Deconvolution {
                algorithm: Algorithm::Wiener,
                sigma: 2.0,
                iterations: 0,
                tv: 0.0,
                noise_ratio: 0.01,
            },
        );
        assert!(img.data[5].is_nan());
        assert!(img.data[6..].iter().all(|v| v.is_finite()));
    }
}
