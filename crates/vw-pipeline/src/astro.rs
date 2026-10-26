//! 天体写真の仕上げのノード（ノイズ除去・シャープ化・星の縮小と除去）の計算。デコンボリューションは [`crate::deconv`]。
//!
//! どれも値の大きさに依らない（雑音の強さを画像から測り、その何倍かで決める）ので、
//! 伸縮の前の線形の値（FITS の物理量など）にも、表示の 0〜1 にも同じように当てられる。
//!
//! 多重解像度はスターレット（B3 スプラインの à trous ウェーブレット、Starck & Murtagh）。
//! 非局所平均や AI を使わないのは、スターレットは星や星雲の縁を残しやすく、
//! 天文の処理（PixInsight の MultiscaleLinearTransform、Siril のウェーブレット）の定番で、
//! 計算も畳み込みだけで済むため。

use crate::nodes::Float;

/// スターレットの階層の数（1, 2, 4, 8 画素の細かさ）。
pub const LEVELS: usize = 4;

/// 白色雑音の標準偏差 1 の画像を B3 スターレットで分けたときの、各階層の係数の標準偏差
/// （Starck & Murtagh, Astronomical Image and Data Analysis, 表 2.1）。
pub(crate) const NOISE: [f32; LEVELS] = [0.889, 0.200, 0.086, 0.041];

/// ノイズ除去。階層 `j` の係数のうち、雑音の `threshold` 倍より小さいものを `strength[j]` だけ弱める。
pub fn denoise(img: &mut Float, strength: [f32; LEVELS], threshold: f32) {
    for_each_plane(img, |plane, w, h| {
        let (mut details, residual) = starlet(plane, w, h);
        let sigma = noise_sigma(&details[0]);
        for (j, d) in details.iter_mut().enumerate() {
            let s = strength[j].clamp(0.0, 1.0);
            let limit = threshold.max(0.0) * sigma * NOISE[j];
            if s == 0.0 || limit <= 0.0 {
                continue;
            }
            for c in d.iter_mut() {
                *c *= keep(c.abs() / limit, s);
            }
        }
        reconstruct(plane, &details, &residual);
    });
}

/// 雑音の大きさに対する係数の比 `a` で、残す割合。しきい値の手前で急に切ると
/// 階段のような模様が出るので、しきい値に向けてなめらかに 1 に戻す。
fn keep(a: f32, strength: f32) -> f32 {
    if a >= 1.0 {
        1.0
    } else {
        1.0 - strength * (1.0 - a * a)
    }
}

/// アンシャープマスク。`v + amount × (v − ぼかした v)`。
pub fn sharpen(img: &mut Float, amount: f32, radius: f32) {
    if amount == 0.0 || radius <= 0.0 {
        return;
    }
    for_each_plane(img, |plane, w, h| {
        let mut blurred = Float::new(w, h, 1, plane.to_vec());
        crate::nodes::blur(&mut blurred, radius);
        for (v, b) in plane.iter_mut().zip(&blurred.data) {
            *v += amount * (*v - b);
        }
    });
}

/// 星の縮小。明るさの細かい構造が雑音の `threshold` 倍を超える所を星とみなし、
/// その周りだけを半径 `size` の最小値フィルタで削って、`amount` の割合で混ぜる。
/// 星雲の細部を削らないよう、星のマスクの外は触らない（PixInsight の MorphologicalTransformation と同じ考え）。
pub fn reduce_stars(img: &mut Float, amount: f32, size: u32, threshold: f32) {
    let amount = amount.clamp(0.0, 1.0);
    let size = size.clamp(1, 8) as usize;
    if amount == 0.0 || img.data.is_empty() {
        return;
    }
    let mask = star_mask(img, threshold, size);
    for_each_plane(img, |plane, w, h| {
        let eroded = min_filter(plane, w, h, size);
        for ((v, e), m) in plane.iter_mut().zip(&eroded).zip(&mask) {
            *v += amount * m * (e - *v);
        }
    });
}

/// 星の除去。星のマスク（[`star_mask`]、半径 `size` だけ広げる）の中を、周りの値から埋める。
/// 埋めた所はなめらかすぎて周りから浮くので、周りと同じ強さの雑音を足す。
///
/// AI（StarNet など）を使わないのは、重みの配布条件を確かめる必要があり、手元の計算だけで
/// 済ませたいため。明るい星の大きな光芒はマスクに入りきらず残りやすい。
pub fn remove_stars(img: &mut Float, size: u32, threshold: f32) {
    let size = size.clamp(1, 16) as usize;
    if img.data.is_empty() {
        return;
    }
    let mask = star_mask(img, threshold, size);
    if mask.iter().all(|&m| m <= 0.0) {
        return;
    }
    let known: Vec<f32> = mask
        .iter()
        .map(|&m| if m > 0.01 { 0.0 } else { 1.0 })
        .collect();
    for_each_plane(img, |plane, w, h| {
        let (details, _) = starlet(plane, w, h);
        let sigma = noise_sigma(&details[0]);
        let filled = push_pull(plane, &known, w, h);
        // チャンネルで同じ種にするのは、色の付いた粒にしないため（夜空の雑音は色が薄い）。
        let mut state = crate::nodes::splitmix(0x5EED);
        for ((v, f), m) in plane.iter_mut().zip(&filled).zip(&mask) {
            if *m > 0.0 {
                let grain = crate::nodes::gaussian(&mut state) * sigma;
                *v += m * (f + grain - *v);
            }
        }
    });
}

/// 重み 0 の画素を、周りの重みのある画素から埋める（push-pull）。半分に縮めながら
/// 重み付きで平均し、いちばん粗い所から拡げ戻して穴だけに入れる。
/// 穴の縁から少しずつしみ込ませるやり方（拡散）より、大きな穴でも速く埋まる。
fn push_pull(values: &[f32], weights: &[f32], w: usize, h: usize) -> Vec<f32> {
    if weights.iter().all(|&k| k >= 1.0) || (w <= 1 && h <= 1) {
        return values.to_vec();
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut coarse = vec![0.0f32; cw * ch];
    let mut coarse_weights = vec![0.0f32; cw * ch];
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut sum, mut weight) = (0.0f32, 0.0f32);
            for y in 2 * cy..(2 * cy + 2).min(h) {
                for x in 2 * cx..(2 * cx + 2).min(w) {
                    let k = weights[y * w + x];
                    sum += k * values[y * w + x];
                    weight += k;
                }
            }
            let i = cy * cw + cx;
            if weight > 0.0 {
                coarse[i] = sum / weight;
            }
            coarse_weights[i] = weight.min(1.0);
        }
    }
    let coarse = push_pull(&coarse, &coarse_weights, cw, ch);
    let sample = |x: f32, y: f32| -> f32 {
        let x = x.clamp(0.0, (cw - 1) as f32);
        let y = y.clamp(0.0, (ch - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let top = coarse[y0 * cw + x0] * (1.0 - fx) + coarse[y0 * cw + x1] * fx;
        let bottom = coarse[y1 * cw + x0] * (1.0 - fx) + coarse[y1 * cw + x1] * fx;
        top * (1.0 - fy) + bottom * fy
    };
    (0..w * h)
        .map(|i| {
            let k = weights[i];
            if k >= 1.0 {
                return values[i];
            }
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let up = sample((x + 0.5) / 2.0 - 0.5, (y + 0.5) / 2.0 - 0.5);
            k * values[i] + (1.0 - k) * up
        })
        .collect()
}

/// 星の柔らかいマスク（0〜1）。明るさ（チャンネルの平均）の細かい 3 階層の和で星を見つけ、
/// 削る半径より少し広げてからぼかす（星の縁で段差が出ないように）。
pub fn star_mask(img: &Float, threshold: f32, size: usize) -> Vec<f32> {
    let (w, h, n) = (img.width, img.height, img.channels);
    let mut luma: Vec<f32> = img
        .data
        .chunks_exact(n)
        .map(|px| px.iter().sum::<f32>() / n as f32)
        .collect();
    let mut hole = vec![false; luma.len()];
    for i in fill_nan(&mut luma) {
        hole[i] = true;
    }
    let (details, _) = starlet(&luma, w, h);
    let sigma = noise_sigma(&details[0]);
    let noise = (NOISE[0] * NOISE[0] + NOISE[1] * NOISE[1] + NOISE[2] * NOISE[2]).sqrt();
    let limit = threshold.max(0.0) * sigma * noise;
    let mut mask: Vec<f32> = (0..w * h)
        .map(|i| {
            let detail = details[0][i] + details[1][i] + details[2][i];
            if detail > limit && !hole[i] {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    mask = max_filter(&mask, w, h, size + 1);
    let mut soft = Float::new(w, h, 1, mask);
    crate::nodes::blur(&mut soft, 1.0);
    soft.data
}

/// チャンネルごとの面に分けて `f(面, 幅, 高さ)` を当て、戻す。NaN は面の中央値で埋めてから当て、後で NaN に戻す
/// （NaN のまま畳み込むと、周りの画素まで NaN になるため）。
pub(crate) fn for_each_plane(img: &mut Float, f: impl Fn(&mut [f32], usize, usize) + Sync) {
    let (w, h, n) = (img.width, img.height, img.channels);
    if w == 0 || h == 0 {
        return;
    }
    let mut planes: Vec<Vec<f32>> = (0..n)
        .map(|c| img.data.iter().skip(c).step_by(n).copied().collect())
        .collect();
    std::thread::scope(|scope| {
        for plane in &mut planes {
            let f = &f;
            scope.spawn(move || {
                let holes = fill_nan(plane);
                f(plane, w, h);
                for i in holes {
                    plane[i] = f32::NAN;
                }
            });
        }
    });
    for (c, plane) in planes.iter().enumerate() {
        for (i, v) in plane.iter().enumerate() {
            img.data[i * n + c] = *v;
        }
    }
}

/// NaN を中央値で埋め、埋めた位置を返す。
pub(crate) fn fill_nan(plane: &mut [f32]) -> Vec<usize> {
    let holes: Vec<usize> = (0..plane.len()).filter(|&i| plane[i].is_nan()).collect();
    if !holes.is_empty() {
        let mut finite: Vec<f32> = plane.iter().copied().filter(|v| v.is_finite()).collect();
        let fill = median(&mut finite).unwrap_or(0.0);
        for &i in &holes {
            plane[i] = fill;
        }
    }
    holes
}

pub(crate) fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mid = values.len() / 2;
    let (_, m, _) = values.select_nth_unstable_by(mid, f32::total_cmp);
    Some(*m)
}

/// 画像そのものの雑音の標準偏差。いちばん細かい階層の係数の中央絶対偏差から見積もる
/// （星や縁の大きな係数に引っ張られない）。
pub(crate) fn noise_sigma(finest: &[f32]) -> f32 {
    // 全部を並べ替えると大きな画像で遅いので、間引いて使う。
    let step = (finest.len() / 1_000_000).max(1);
    let mut values: Vec<f32> = finest.iter().step_by(step).copied().collect();
    let center = median(&mut values).unwrap_or(0.0);
    for v in &mut values {
        *v = (*v - center).abs();
    }
    median(&mut values).unwrap_or(0.0) / 0.6745 / NOISE[0]
}

/// スターレット変換。`(各階層の係数, 残りのなめらかな面)`。係数を全部足して残りを足すと元に戻る。
pub fn starlet(plane: &[f32], w: usize, h: usize) -> (Vec<Vec<f32>>, Vec<f32>) {
    let mut details = Vec::with_capacity(LEVELS);
    let mut current = plane.to_vec();
    for j in 0..LEVELS {
        let smooth = b3_smooth(&current, w, h, 1 << j);
        details.push(current.iter().zip(&smooth).map(|(c, s)| c - s).collect());
        current = smooth;
    }
    (details, current)
}

fn reconstruct(plane: &mut [f32], details: &[Vec<f32>], residual: &[f32]) {
    plane.copy_from_slice(residual);
    for d in details {
        for (v, c) in plane.iter_mut().zip(d) {
            *v += c;
        }
    }
}

/// B3 スプライン（1, 4, 6, 4, 1）/ 16 を、`step` 画素おきに横と縦にかける。端は鏡に映して折り返す。
fn b3_smooth(plane: &[f32], w: usize, h: usize, step: usize) -> Vec<f32> {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let along = |line: &[f32], out: &mut [f32]| {
        let len = line.len() as isize;
        for (i, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for (k, weight) in K.iter().enumerate() {
                let at = i as isize + (k as isize - 2) * step as isize;
                acc += line[mirror(at, len)] * weight;
            }
            *o = acc;
        }
    };
    let mut rows = vec![0.0f32; w * h];
    by_rows(&mut rows, w, |y, out| {
        along(&plane[y * w..(y + 1) * w], out)
    });
    let columns = transpose(&rows, w, h);
    let mut out = vec![0.0f32; w * h];
    by_rows(&mut out, h, |x, o| along(&columns[x * h..(x + 1) * h], o));
    transpose(&out, h, w)
}

fn mirror(i: isize, len: isize) -> usize {
    if len == 1 {
        return 0;
    }
    let period = 2 * (len - 1);
    let m = i.rem_euclid(period);
    (if m >= len { period - m } else { m }) as usize
}

fn transpose(plane: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            out[x * h + y] = plane[y * w + x];
        }
    }
    out
}

/// 幅 `w` の行に分けて並列に `f(行番号, 行)` を当てる。
fn by_rows(out: &mut [f32], w: usize, f: impl Fn(usize, &mut [f32]) + Sync) {
    if w == 0 {
        return;
    }
    let rows = out.len() / w;
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(out.len().div_ceil(1 << 16))
        .max(1);
    let per = rows.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (band, chunk) in out.chunks_mut(per * w).enumerate() {
            let f = &f;
            scope.spawn(move || {
                for (i, row) in chunk.chunks_exact_mut(w).enumerate() {
                    f(band * per + i, row);
                }
            });
        }
    });
}

/// 一辺 `2r + 1` の正方形の最小値。横と縦に分けてかける。
fn min_filter(plane: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    extreme_filter(plane, w, h, r, f32::min)
}

pub(crate) fn max_filter(plane: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    extreme_filter(plane, w, h, r, f32::max)
}

fn extreme_filter(
    plane: &[f32],
    w: usize,
    h: usize,
    r: usize,
    pick: impl Fn(f32, f32) -> f32 + Sync,
) -> Vec<f32> {
    let along = |line: &[f32], out: &mut [f32]| {
        let len = line.len();
        for (i, o) in out.iter_mut().enumerate() {
            let (from, to) = (i.saturating_sub(r), (i + r).min(len - 1));
            *o = line[from..=to].iter().copied().fold(line[i], &pick);
        }
    };
    let mut rows = vec![0.0f32; w * h];
    by_rows(&mut rows, w, |y, out| {
        along(&plane[y * w..(y + 1) * w], out)
    });
    let columns = transpose(&rows, w, h);
    let mut out = vec![0.0f32; w * h];
    by_rows(&mut out, h, |x, o| along(&columns[x * h..(x + 1) * h], o));
    transpose(&out, h, w)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 決まった種の、標準偏差 `sigma` のほぼ正規分布の乱数（一様乱数 12 個の和から 6 を引く）。
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

    /// 背景 `sky` に、`(x, y)` を中心とした σ `width` の星を置いた灰色の画像。
    fn sky_with_star(w: usize, h: usize, sky: f32, star: (f32, f32, f32), seed: u64) -> Float {
        let n = noise(w * h, seed, 0.3);
        let data = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let d2 = (x - star.0).powi(2) + (y - star.1).powi(2);
                sky + n[i] + 50.0 * (-d2 / (2.0 * star.2 * star.2)).exp()
            })
            .collect();
        Float::new(w, h, 1, data)
    }

    fn spread(values: &[f32]) -> f32 {
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt()
    }

    #[test]
    fn starlet_coefficients_add_back_to_the_image() {
        let plane = noise(37 * 23, 7, 1.0);
        let (details, residual) = starlet(&plane, 37, 23);
        let mut back = vec![0.0; plane.len()];
        reconstruct(&mut back, &details, &residual);
        for (a, b) in plane.iter().zip(&back) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn noise_sigma_is_measured_from_the_finest_level() {
        let plane = noise(256 * 256, 3, 2.0);
        let (details, _) = starlet(&plane, 256, 256);
        let sigma = noise_sigma(&details[0]);
        assert!((sigma - 2.0).abs() < 0.2, "{sigma}");
    }

    #[test]
    fn denoise_flattens_the_sky_and_keeps_the_star_peak() {
        let mut img = sky_with_star(96, 96, 1000.0, (48.0, 48.0, 1.5), 11);
        let before_sky = spread(&img.data[..96 * 20]);
        let before_peak = img.data[48 * 96 + 48];
        denoise(&mut img, [1.0, 1.0, 0.5, 0.0], 3.0);
        let after_sky = spread(&img.data[..96 * 20]);
        assert!(after_sky < before_sky * 0.5, "{before_sky} -> {after_sky}");
        let after_peak = img.data[48 * 96 + 48];
        assert!(
            after_peak > before_peak - 5.0,
            "{before_peak} -> {after_peak}"
        );
    }

    #[test]
    fn sharpen_raises_the_contrast_of_an_edge() {
        let mut img = Float::new(8, 1, 1, vec![0.2, 0.2, 0.2, 0.2, 0.8, 0.8, 0.8, 0.8]);
        sharpen(&mut img, 1.0, 1.0);
        assert!(img.data[3] < 0.2 && img.data[4] > 0.8, "{:?}", img.data);
        assert!((img.data[0] - 0.2).abs() < 0.01);
    }

    #[test]
    fn star_reduction_shrinks_the_star_and_leaves_the_sky() {
        let mut img = sky_with_star(64, 64, 100.0, (32.0, 32.0, 2.5), 5);
        let sky_before: Vec<f32> = img.data[..64 * 10].to_vec();
        let ring = |img: &Float| img.data[32 * 64 + 36];
        let before = ring(&img);
        reduce_stars(&mut img, 1.0, 2, 4.0);
        assert!(ring(&img) < before - 10.0, "{before} -> {}", ring(&img));
        assert_eq!(&img.data[..64 * 10], &sky_before[..]);
    }

    #[test]
    fn star_removal_fills_the_star_with_the_sky() {
        let mut img = sky_with_star(64, 64, 100.0, (32.0, 32.0, 2.0), 5);
        let far: Vec<f32> = img.data[..64 * 8].to_vec();
        remove_stars(&mut img, 3, 4.0);
        let core = img.data[32 * 64 + 32];
        assert!((core - 100.0).abs() < 3.0, "{core}");
        assert_eq!(&img.data[..64 * 8], &far[..]);
    }

    #[test]
    fn push_pull_fills_a_hole_from_its_surroundings() {
        let (w, h) = (16, 16);
        let values: Vec<f32> = (0..w * h).map(|i| (i % w) as f32).collect();
        let mut weights = vec![1.0; w * h];
        let mut holed = values.clone();
        for y in 6..10 {
            for x in 6..10 {
                weights[y * w + x] = 0.0;
                holed[y * w + x] = 1000.0;
            }
        }
        let filled = push_pull(&holed, &weights, w, h);
        for y in 6..10 {
            for x in 6..10 {
                let v = filled[y * w + x];
                assert!((v - x as f32).abs() < 2.5, "({x}, {y}) {v}");
            }
        }
        assert_eq!(filled[0], 0.0);
    }

    #[test]
    fn nan_pixels_stay_nan_and_do_not_spread() {
        let mut img = sky_with_star(32, 32, 10.0, (16.0, 16.0, 1.5), 9);
        img.data[0] = f32::NAN;
        denoise(&mut img, [1.0; LEVELS], 3.0);
        assert!(img.data[0].is_nan());
        assert!(img.data[1..].iter().all(|v| v.is_finite()));
    }

    #[test]
    fn mirror_folds_indices_back_into_the_line() {
        let folded: Vec<usize> = (-3..8).map(|i| mirror(i, 5)).collect();
        assert_eq!(folded, [3, 2, 1, 0, 1, 2, 3, 4, 3, 2, 1]);
    }
}
