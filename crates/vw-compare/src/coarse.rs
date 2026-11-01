//! 縮めた画像での大まかな位置合わせ。回転と倍率を振幅スペクトルの対数極座標で（Fourier–Mellin）、
//! ずれを位相相関で求める。
//!
//! 特徴点（角などの目印）の対応で求めないのは、依存が大きく遅いのに加え、RAW の現像と JPEG の
//! ようにトーンの違う画像でも、位相相関は明るさの違いに左右されにくいため。

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::affine::Affine;
use crate::plane::Plane;

/// 大まかに合わせるときの、基準の長辺。
const LONG_SIDE: f64 = 256.0;
/// 対数極座標の角度の段数（0〜180 度）。
const ANGLES: usize = 360;
/// 対数極座標の半径の段数。
const RADII: usize = 256;

/// 大まかな写し方と、ずれの位相相関の高さ（0〜1、高いほど確か）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coarse {
    pub transform: Affine,
    pub peak: f64,
}

/// `reference` の座標から `moving` の座標への大まかな写し方。
pub fn estimate(reference: &Plane, moving: &Plane) -> Option<Coarse> {
    if reference.width < 8 || reference.height < 8 || moving.width < 8 || moving.height < 8 {
        return None;
    }
    let (rw, rh) = (reference.width as f64, reference.height as f64);
    let (mw, mh) = (moving.width as f64, moving.height as f64);
    // 同じ範囲を写した画像なら、面積の比が倍率の 2 乗。回転していても変わらない。
    let guess = ((mw * mh) / (rw * rh)).sqrt();
    let fr = LONG_SIDE / rw.max(rh);
    let fm = fr / guess;
    let r = reference.resized(scaled(rw, fr), scaled(rh, fr));
    let m = moving.resized(scaled(mw, fm), scaled(mh, fm));
    let side = [r.width, r.height, m.width, m.height]
        .into_iter()
        .max()
        .unwrap_or(1);
    let n = (2 * side).next_power_of_two().clamp(64, 1024);
    let fft = Fft2::new(n, n);
    let (rc, r_off) = canvas(&r, n);
    let (mc, m_off) = canvas(&m, n);
    let rs = fft.forward(&rc);
    let ms = fft.forward(&mc);
    let lp_r = log_polar(&magnitude(&rs, n), n);
    let lp_m = log_polar(&magnitude(&ms, n), n);
    let lp_fft = Fft2::new(RADII, ANGLES);
    let (_, d_angle, d_radius) = {
        let (dx, dy, peak) = phase_correlate(&lp_fft, &lp_r, &lp_m);
        (peak, dy, dx)
    };
    let angle = d_angle * std::f64::consts::PI / ANGLES as f64;
    let scale = (-d_radius * radius_step(n)).exp();
    let centre = |off: [f64; 2], p: &Plane| {
        [
            off[0] + (p.width as f64 - 1.0) / 2.0,
            off[1] + (p.height as f64 - 1.0) / 2.0,
        ]
    };
    let (rcen, mcen) = (centre(r_off, &r), centre(m_off, &m));
    let reference_spectrum = rs;
    let mut best: Option<(Affine, f64)> = None;
    for turn in [angle, angle + std::f64::consts::PI] {
        let t = Affine::similarity(scale, turn, rcen, mcen);
        let warped = warp_canvas(&mc, n, &t);
        let ws = fft.forward(&warped);
        let (dx, dy, peak) = correlate_spectra(&fft, &reference_spectrum, &ws);
        // 写した画像が基準より d だけずれているなら、基準の x は写した画像の x + d。
        let shifted = Affine {
            c: dx,
            f: dy,
            ..Affine::IDENTITY
        }
        .then(&t);
        if best.is_none_or(|(_, p)| peak > p) {
            best = Some((shifted, peak));
        }
    }
    let (canvas_t, peak) = best?;
    let to_canvas = Affine {
        c: r_off[0],
        f: r_off[1],
        ..Affine::IDENTITY
    };
    let from_canvas = Affine {
        c: -m_off[0],
        f: -m_off[1],
        ..Affine::IDENTITY
    };
    let level = to_canvas.then(&canvas_t).then(&from_canvas);
    let fr_actual = r.width as f64 / rw;
    let fm_actual = m.width as f64 / mw;
    Some(Coarse {
        transform: level.between_scales(1.0 / fr_actual, 1.0 / fm_actual),
        peak,
    })
}

fn scaled(side: f64, factor: f64) -> usize {
    ((side * factor).round() as usize).max(1)
}

/// 対数極座標の半径 1 段あたりの、対数の増え方。
fn radius_step(n: usize) -> f64 {
    (n as f64 / 2.0).ln() / RADII as f64
}

/// 平均を引き、縁を Hann 窓で落として、`n` × `n` の真ん中に置く。左上の置き場所も返す。
fn canvas(p: &Plane, n: usize) -> (Vec<f32>, [f64; 2]) {
    let mean = p.data.iter().sum::<f32>() / p.data.len() as f32;
    let (ox, oy) = ((n - p.width.min(n)) / 2, (n - p.height.min(n)) / 2);
    let mut out = vec![0.0f32; n * n];
    let hann = |i: usize, len: usize| {
        if len <= 1 {
            1.0
        } else {
            0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (len - 1) as f32).cos()
        }
    };
    for y in 0..p.height.min(n) {
        let wy = hann(y, p.height);
        for x in 0..p.width.min(n) {
            out[(oy + y) * n + ox + x] = (p.at(x, y) - mean) * wy * hann(x, p.width);
        }
    }
    (out, [ox as f64, oy as f64])
}

/// 置いた画像 `c` を写し方 `t`（基準の置き場所の座標 → この置き場所の座標）で基準の置き場所に写す。
fn warp_canvas(c: &[f32], n: usize, t: &Affine) -> Vec<f32> {
    let plane = Plane::new(n, n, c.to_vec());
    let mut out = vec![0.0f32; n * n];
    for y in 0..n {
        for x in 0..n {
            let [sx, sy] = t.apply([x as f64, y as f64]);
            out[y * n + x] = plane.sample(sx, sy).unwrap_or(0.0);
        }
    }
    out
}

/// 中心に寄せた対数振幅に、低い周波数を落とす重みをかけたもの（`n` × `n`）。
fn magnitude(spectrum: &[Complex32], n: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; n * n];
    let half = n / 2;
    for v in 0..n {
        for u in 0..n {
            let su = (u + half) % n;
            let sv = (v + half) % n;
            let xi = (su as f32 - half as f32) / n as f32;
            let eta = (sv as f32 - half as f32) / n as f32;
            let x = (std::f32::consts::PI * xi).cos() * (std::f32::consts::PI * eta).cos();
            let high_pass = (1.0 - x) * (2.0 - x);
            out[sv * n + su] = (1.0 + spectrum[v * n + u].norm()).ln() * high_pass;
        }
    }
    out
}

/// 振幅を、横に半径の対数（`RADII`）、縦に角度（0〜180 度、`ANGLES`）で並べ直す。
fn log_polar(mag: &[f32], n: usize) -> Vec<f32> {
    let plane = Plane::new(n, n, mag.to_vec());
    let centre = (n / 2) as f64;
    let step = radius_step(n);
    let mut out = vec![0.0f32; RADII * ANGLES];
    for a in 0..ANGLES {
        let theta = a as f64 * std::f64::consts::PI / ANGLES as f64;
        let (s, c) = theta.sin_cos();
        for r in 0..RADII {
            let radius = (r as f64 * step).exp();
            out[a * RADII + r] = plane
                .sample(centre + radius * c, centre + radius * s)
                .unwrap_or(0.0);
        }
    }
    out
}

/// `b` が `a` を `(dx, dy)` ずらしたもの（`b(x) ≈ a(x − d)`）とみたときの `d` と、相関の高さ。
fn phase_correlate(fft: &Fft2, a: &[f32], b: &[f32]) -> (f64, f64, f64) {
    let fa = fft.forward(a);
    let fb = fft.forward(b);
    correlate_spectra(fft, &fa, &fb)
}

fn correlate_spectra(fft: &Fft2, fa: &[Complex32], fb: &[Complex32]) -> (f64, f64, f64) {
    let cross: Vec<Complex32> = fa
        .iter()
        .zip(fb)
        .map(|(a, b)| {
            let c = b * a.conj();
            let norm = c.norm();
            if norm > 1e-12 {
                c / norm
            } else {
                Complex32::new(0.0, 0.0)
            }
        })
        .collect();
    let (w, h) = (fft.width, fft.height);
    let surface: Vec<f32> = fft
        .inverse(&cross)
        .iter()
        .map(|c| c.re / (w * h) as f32)
        .collect();
    let (best, &peak) = surface
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap_or((0, &0.0));
    let (px, py) = (best % w, best / w);
    let at = |x: isize, y: isize| {
        let x = x.rem_euclid(w as isize) as usize;
        let y = y.rem_euclid(h as isize) as usize;
        surface[y * w + x]
    };
    // 頂点の前後 3 点に放物線を当てて、画素より細かく求める。
    let refine = |l: f32, c: f32, r: f32| {
        let den = l - 2.0 * c + r;
        if den.abs() < 1e-12 {
            0.0
        } else {
            f64::from(0.5 * (l - r) / den).clamp(-0.5, 0.5)
        }
    };
    let (ix, iy) = (px as isize, py as isize);
    let fx = refine(at(ix - 1, iy), peak, at(ix + 1, iy));
    let fy = refine(at(ix, iy - 1), peak, at(ix, iy + 1));
    let wrap = |p: usize, len: usize| {
        if p > len / 2 {
            p as f64 - len as f64
        } else {
            p as f64
        }
    };
    (
        wrap(px, w) + fx,
        wrap(py, h) + fy,
        f64::from(peak).clamp(0.0, 1.0),
    )
}

/// 2 次元の FFT（行ごと、列ごと）。
struct Fft2 {
    width: usize,
    height: usize,
    rows: Arc<dyn Fft<f32>>,
    cols: Arc<dyn Fft<f32>>,
    rows_inv: Arc<dyn Fft<f32>>,
    cols_inv: Arc<dyn Fft<f32>>,
}

impl Fft2 {
    fn new(width: usize, height: usize) -> Self {
        let mut planner = FftPlanner::new();
        Self {
            width,
            height,
            rows: planner.plan_fft_forward(width),
            cols: planner.plan_fft_forward(height),
            rows_inv: planner.plan_fft_inverse(width),
            cols_inv: planner.plan_fft_inverse(height),
        }
    }

    fn forward(&self, data: &[f32]) -> Vec<Complex32> {
        let c: Vec<Complex32> = data.iter().map(|&v| Complex32::new(v, 0.0)).collect();
        self.run(c, false)
    }

    fn inverse(&self, data: &[Complex32]) -> Vec<Complex32> {
        self.run(data.to_vec(), true)
    }

    fn run(&self, mut data: Vec<Complex32>, inverse: bool) -> Vec<Complex32> {
        let (w, h) = (self.width, self.height);
        let (rows, cols) = match inverse {
            true => (&self.rows_inv, &self.cols_inv),
            false => (&self.rows, &self.cols),
        };
        rows.process(&mut data);
        let mut t = vec![Complex32::new(0.0, 0.0); w * h];
        for y in 0..h {
            for x in 0..w {
                t[x * h + y] = data[y * w + x];
            }
        }
        cols.process(&mut t);
        for x in 0..w {
            for y in 0..h {
                data[y * w + x] = t[x * h + y];
            }
        }
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{texture, transformed};

    #[test]
    fn phase_correlation_finds_a_whole_pixel_shift() {
        let a = texture(128, 128);
        let b = transformed(
            &a,
            128,
            128,
            &Affine::similarity(1.0, 0.0, [0.0, 0.0], [-5.0, 3.0]),
        );
        let fft = Fft2::new(128, 128);
        let (dx, dy, peak) = phase_correlate(&fft, &a.data, &b.data);
        assert!(
            (dx + 5.0).abs() < 0.3 && (dy - 3.0).abs() < 0.3,
            "{dx} {dy}"
        );
        assert!(peak > 0.3, "{peak}");
    }

    fn check(t: &Affine, size: (usize, usize)) {
        let r = texture(200, 150);
        let m = transformed(&r, size.0, size.1, t);
        let found = estimate(&r, &m).unwrap();
        for p in [[20.0, 20.0], [180.0, 30.0], [100.0, 130.0]] {
            let want = t.apply(p);
            let got = found.transform.apply(p);
            let err = (want[0] - got[0]).hypot(want[1] - got[1]);
            assert!(
                err < 2.5,
                "{p:?}: want {want:?} got {got:?} (peak {})",
                found.peak
            );
        }
    }

    #[test]
    fn a_shifted_copy_is_found() {
        check(
            &Affine::similarity(1.0, 0.0, [100.0, 75.0], [92.0, 81.0]),
            (200, 150),
        );
    }

    #[test]
    fn a_turned_and_shrunk_copy_is_found() {
        check(
            &Affine::similarity(0.8, 7f64.to_radians(), [100.0, 75.0], [84.0, 58.0]),
            (170, 125),
        );
    }

    #[test]
    fn a_copy_turned_upside_down_is_found() {
        check(
            &Affine::similarity(1.0, 180f64.to_radians(), [100.0, 75.0], [99.5, 74.5]),
            (200, 150),
        );
    }

    #[test]
    fn a_copy_turned_a_quarter_is_found() {
        check(
            &Affine::similarity(1.0, 90f64.to_radians(), [100.0, 75.0], [74.5, 99.5]),
            (150, 200),
        );
    }
}
