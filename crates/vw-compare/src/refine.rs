//! 大まかな写し方を、明るさの勾配で細かく直す（Lucas–Kanade、6 つの値のアフィン）。
//! 縮めた画像から始めて、元の大きさに近づけながら繰り返す。
//!
//! 比べる画像の明るさに `gain · 値 + bias` を一緒に当てはめる。RAW の現像と JPEG のように
//! 明るさやコントラストが違っても、ずれと取り違えないため。外れた画素（動いた物、
//! 片方だけの飽和）に引きずられないよう、残差の大きい画素は Huber の重みで軽くする。

use rayon::prelude::*;

use crate::affine::Affine;
use crate::plane::{fit_long_side, Plane};

/// 直す段の基準の長辺。最後の段は基準の長辺（この上限まで）。
const LEVELS: [usize; 4] = [256, 512, 1024, 2048];
/// 1 段の繰り返しの上限。
const ITERATIONS: usize = 40;
/// 1 回の計算に使う画素の数のおよその上限（多い段は間引く）。
const SAMPLES: usize = 250_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Refined {
    pub transform: Affine,
    /// 重なった所での、基準と写した比べる画像の相関（−1〜1）。
    pub correlation: f64,
    /// 重なった所の割合（基準の面積に対して、0〜1）。
    pub overlap: f64,
}

/// `start`（基準 → 比べる画像）を細かく直す。
pub fn refine(reference: &Plane, moving: &Plane, start: Affine) -> Refined {
    let (rw, rh) = (reference.width, reference.height);
    let top = rw.max(rh);
    let mut sides: Vec<usize> = LEVELS.iter().copied().filter(|&l| l < top).collect();
    sides.push(top.min(*LEVELS.last().unwrap_or(&2048)));
    sides.dedup();
    let mut t = start;
    let mut last = None;
    for side in sides {
        let (w, h, _) = fit_long_side(rw, rh, side);
        let r = if (w, h) == (rw, rh) {
            reference.clone()
        } else {
            reference.resized(w, h)
        };
        let s = w as f64 / rw as f64;
        // 比べる画像も、基準の 1 画素がおよそ 1 画素になる大きさにする（縮めすぎで勾配を失わない）。
        let scale = (t.a * t.e - t.b * t.d).abs().sqrt().max(1e-6);
        let g = (s / scale).min(1.0);
        let (mw, mh) = (
            ((moving.width as f64 * g).round() as usize).max(1),
            ((moving.height as f64 * g).round() as usize).max(1),
        );
        let m = if (mw, mh) == (moving.width, moving.height) {
            moving.clone()
        } else {
            moving.resized(mw, mh)
        };
        let g = mw as f64 / moving.width as f64;
        let level = t.between_scales(s, g);
        let (fitted, score) = fit(&r, &m, level);
        t = fitted.between_scales(1.0 / s, 1.0 / g);
        last = Some(score);
    }
    let (correlation, overlap) = last.unwrap_or((0.0, 0.0));
    Refined {
        transform: t,
        correlation,
        overlap,
    }
}

/// 写し方 `t` のままで、重なった所の相関と重なりの割合を測る（手で直した写し方を確かめる）。
pub fn score(reference: &Plane, moving: &Plane, t: &Affine) -> (f64, f64) {
    let stride = ((reference.width * reference.height) as f64 / SAMPLES as f64)
        .sqrt()
        .ceil()
        .max(1.0) as usize;
    correlation(reference, moving, t, stride)
}

/// 1 段ぶんの繰り返し。直した写し方と、そのときの (相関, 重なりの割合)。
fn fit(r: &Plane, m: &Plane, start: Affine) -> (Affine, (f64, f64)) {
    let stride = ((r.width * r.height) as f64 / SAMPLES as f64)
        .sqrt()
        .ceil()
        .max(1.0) as usize;
    let mut p = [
        start.a, start.b, start.c, start.d, start.e, start.f, 1.0, 0.0,
    ];
    // 初めは明るさの比と差を平均で合わせておく。
    if let Some((gain, bias)) = photometric(r, m, &affine(&p), stride) {
        p[6] = gain;
        p[7] = bias;
    }
    let mut sigma = f64::INFINITY;
    for _ in 0..ITERATIONS {
        let Some(acc) = accumulate(r, m, &p, stride, sigma) else {
            break;
        };
        sigma = (acc.median_abs() * 1.4826).max(0.5);
        let Some(delta) = solve(acc.h, acc.g) else {
            break;
        };
        for (v, d) in p.iter_mut().zip(delta) {
            *v -= d;
        }
        // 画像の隅がどれだけ動いたか（画素）で止める。
        let moved = [0.0, r.width as f64]
            .into_iter()
            .flat_map(|x| [0.0, r.height as f64].map(|y| (x, y)))
            .map(|(x, y)| {
                let dx = delta[0] * x + delta[1] * y + delta[2];
                let dy = delta[3] * x + delta[4] * y + delta[5];
                dx.hypot(dy)
            })
            .fold(0.0f64, f64::max);
        if moved < 0.01 {
            break;
        }
    }
    let t = affine(&p);
    (t, correlation(r, m, &t, stride))
}

fn affine(p: &[f64; 8]) -> Affine {
    Affine {
        a: p[0],
        b: p[1],
        c: p[2],
        d: p[3],
        e: p[4],
        f: p[5],
    }
}

/// 写した比べる画像の明るさを基準に合わせる比と差（平均と標準偏差で）。
fn photometric(r: &Plane, m: &Plane, t: &Affine, stride: usize) -> Option<(f64, f64)> {
    let pairs = pairs(r, m, t, stride);
    if pairs.len() < 16 {
        return None;
    }
    let n = pairs.len() as f64;
    let (mr, mm) = pairs
        .iter()
        .fold((0.0, 0.0), |(a, b), &(x, y)| (a + x, b + y));
    let (mr, mm) = (mr / n, mm / n);
    let (vr, vm) = pairs.iter().fold((0.0, 0.0), |(a, b), &(x, y)| {
        (a + (x - mr).powi(2), b + (y - mm).powi(2))
    });
    if vm <= 1e-9 {
        return None;
    }
    let gain = (vr / vm).sqrt();
    Some((gain, mr - gain * mm))
}

/// 重なった所の (基準の値, 比べる画像の値) の組。
fn pairs(r: &Plane, m: &Plane, t: &Affine, stride: usize) -> Vec<(f64, f64)> {
    (0..r.height)
        .into_par_iter()
        .step_by(stride)
        .flat_map_iter(|y| {
            (0..r.width).step_by(stride).filter_map(move |x| {
                let [sx, sy] = t.apply([x as f64, y as f64]);
                let v = m.sample(sx, sy)?;
                Some((f64::from(r.at(x, y)), f64::from(v)))
            })
        })
        .collect()
}

/// 重なった所の相関と、重なりの割合。
fn correlation(r: &Plane, m: &Plane, t: &Affine, stride: usize) -> (f64, f64) {
    let pairs = pairs(r, m, t, stride);
    let total = r.height.div_ceil(stride) * r.width.div_ceil(stride);
    let overlap = pairs.len() as f64 / total.max(1) as f64;
    if pairs.len() < 16 {
        return (0.0, overlap);
    }
    let n = pairs.len() as f64;
    let (mr, mm) = pairs
        .iter()
        .fold((0.0, 0.0), |(a, b), &(x, y)| (a + x, b + y));
    let (mr, mm) = (mr / n, mm / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for &(x, y) in &pairs {
        sxy += (x - mr) * (y - mm);
        sxx += (x - mr).powi(2);
        syy += (y - mm).powi(2);
    }
    let den = (sxx * syy).sqrt();
    (if den > 0.0 { sxy / den } else { 0.0 }, overlap)
}

#[derive(Clone)]
struct Acc {
    h: [[f64; 8]; 8],
    g: [f64; 8],
    /// 残差の絶対値の一部（中央値を見積もる）。
    sample: Vec<f32>,
    n: usize,
}

impl Acc {
    fn new() -> Self {
        Self {
            h: [[0.0; 8]; 8],
            g: [0.0; 8],
            sample: Vec::new(),
            n: 0,
        }
    }

    fn merge(mut self, o: Acc) -> Acc {
        for i in 0..8 {
            self.g[i] += o.g[i];
            for j in 0..8 {
                self.h[i][j] += o.h[i][j];
            }
        }
        self.sample.extend(o.sample);
        self.n += o.n;
        self
    }

    fn median_abs(&self) -> f64 {
        let mut s = self.sample.clone();
        if s.is_empty() {
            return 0.0;
        }
        let mid = s.len() / 2;
        s.select_nth_unstable_by(mid, f32::total_cmp);
        f64::from(s[mid])
    }
}

/// ガウス・ニュートンの正規方程式（`h δ = g`）の項を足し合わせる。
fn accumulate(r: &Plane, m: &Plane, p: &[f64; 8], stride: usize, sigma: f64) -> Option<Acc> {
    let t = affine(p);
    let (gain, bias) = (p[6], p[7]);
    let k = 1.345 * sigma;
    let acc = (0..r.height)
        .into_par_iter()
        .step_by(stride)
        .fold(Acc::new, |mut acc, y| {
            for x in (0..r.width).step_by(stride) {
                let (fx, fy) = (x as f64, y as f64);
                let [sx, sy] = t.apply([fx, fy]);
                let (Some(v), Some(l), Some(rt), Some(u), Some(d)) = (
                    m.sample(sx, sy),
                    m.sample(sx - 1.0, sy),
                    m.sample(sx + 1.0, sy),
                    m.sample(sx, sy - 1.0),
                    m.sample(sx, sy + 1.0),
                ) else {
                    continue;
                };
                let v = f64::from(v);
                let gx = gain * f64::from(rt - l) / 2.0;
                let gy = gain * f64::from(d - u) / 2.0;
                let res = gain * v + bias - f64::from(r.at(x, y));
                let w = if res.abs() <= k { 1.0 } else { k / res.abs() };
                let j = [gx * fx, gx * fy, gx, gy * fx, gy * fy, gy, v, 1.0];
                for a in 0..8 {
                    acc.g[a] += w * j[a] * res;
                    for b in a..8 {
                        acc.h[a][b] += w * j[a] * j[b];
                    }
                }
                if acc.n % 7 == 0 {
                    acc.sample.push(res.abs() as f32);
                }
                acc.n += 1;
            }
            acc
        })
        .reduce(Acc::new, Acc::merge);
    if acc.n < 64 {
        return None;
    }
    let mut acc = acc;
    for a in 0..8 {
        for b in 0..a {
            acc.h[a][b] = acc.h[b][a];
        }
    }
    Some(acc)
}

/// `h δ = g` を解く。値の桁が違うので、対角で正規化してから掃き出す。
fn solve(h: [[f64; 8]; 8], g: [f64; 8]) -> Option<[f64; 8]> {
    let d: Vec<f64> = (0..8).map(|i| h[i][i].abs().sqrt().max(1e-12)).collect();
    let mut a = [[0.0f64; 9]; 8];
    for i in 0..8 {
        for j in 0..8 {
            a[i][j] = h[i][j] / (d[i] * d[j]);
        }
        // 少しだけ減衰をかけ、平らな画像で発散しないようにする。
        a[i][i] += 1e-6;
        a[i][8] = g[i] / d[i];
    }
    for col in 0..8 {
        let pivot = (col..8).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        let pivot_row = a[col];
        for (row, line) in a.iter_mut().enumerate() {
            if row != col {
                let f = line[col] / pivot_row[col];
                for (x, p) in line.iter_mut().zip(pivot_row).skip(col) {
                    *x -= f * p;
                }
            }
        }
    }
    let mut out = [0.0; 8];
    for i in 0..8 {
        out[i] = a[i][8] / a[i][i] / d[i];
        if !out[i].is_finite() {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{texture, transformed};

    fn error(t: &Affine, found: &Affine, size: (f64, f64)) -> f64 {
        [[0.0, 0.0], [size.0, 0.0], [0.0, size.1], [size.0, size.1]]
            .into_iter()
            .map(|p| {
                let (a, b) = (t.apply(p), found.apply(p));
                (a[0] - b[0]).hypot(a[1] - b[1])
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn a_rough_start_is_brought_within_a_tenth_of_a_pixel() {
        let r = texture(240, 180);
        let t = Affine {
            a: 0.97,
            b: 0.03,
            c: 6.3,
            d: -0.02,
            e: 1.01,
            f: -4.6,
        };
        let m = transformed(&r, 240, 180, &t);
        let start = Affine {
            c: 4.0,
            f: -3.0,
            ..Affine::IDENTITY
        };
        let found = refine(&r, &m, start);
        let err = error(&t, &found.transform, (200.0, 150.0));
        assert!(err < 0.1, "{err} {:?}", found.transform);
        assert!(found.correlation > 0.99, "{}", found.correlation);
    }

    #[test]
    fn a_brighter_lower_contrast_copy_is_still_aligned() {
        let r = texture(240, 180);
        let t = Affine::similarity(1.0, 0.02, [120.0, 90.0], [123.0, 88.0]);
        let mut m = transformed(&r, 240, 180, &t);
        for v in &mut m.data {
            *v = *v * 0.6 + 70.0;
        }
        let found = refine(&r, &m, Affine::IDENTITY);
        let err = error(&t, &found.transform, (200.0, 150.0));
        assert!(err < 0.15, "{err}");
    }

    #[test]
    fn the_equations_are_solved_exactly() {
        let mut h = [[0.0; 8]; 8];
        for (i, row) in h.iter_mut().enumerate() {
            row[i] = (i + 1) as f64 * 10.0;
        }
        h[0][1] = 1.0;
        h[1][0] = 1.0;
        let x = [1.0, -2.0, 3.0, 0.5, 0.0, 4.0, -1.0, 2.0];
        let g: [f64; 8] = std::array::from_fn(|i| (0..8).map(|j| h[i][j] * x[j]).sum());
        let got = solve(h, g).unwrap();
        for (a, b) in got.iter().zip(x) {
            assert!((a - b).abs() < 1e-4, "{got:?}");
        }
    }
}
