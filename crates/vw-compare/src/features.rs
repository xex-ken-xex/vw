//! 特徴点（角）を見つけて、2 枚の間で対応を取る。
//!
//! 重なった所の相関（[`crate::mosaic::link_all`]）は、重なりが狭いときや大きく回っているときに
//! 見つけられない。角の周りの小さな模様は回しても大きさを変えてもほぼ同じに見えるので、
//! それを照らし合わせれば、狭い重なりでも大きな回転でも対応が取れる。
//!
//! 角は Shi–Tomasi の値の山、向きは周りの明るさの重心、模様は向きをそろえた 2 値の比べ
//! （ORB 風、256 ビット）。大きさの違いには、縮めた画像でも探して備える。
//! OpenCV のような大きな依存は足さない（ライセンスと配布の大きさのため）。

use rayon::prelude::*;

use crate::estimate::Pair;
use crate::plane::Plane;

/// 1 つの特徴点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keypoint {
    /// 元の画像の画素の座標。
    pub at: [f64; 2],
    /// 模様を比べた向き（ラジアン）。
    pub angle: f32,
    pub bits: [u64; 4],
}

/// 1 枚から探す特徴点の数の目安。
const WANTED: usize = 1600;
/// 模様を比べる円の半径（探した段の画素）。
const RADIUS: isize = 15;
/// 縮めて探す段の倍率。
const LEVELS: [f64; 4] = [1.0, 0.71, 0.5, 0.35];
/// 2 値の比べの、いちばん近い相手との違いの上限（256 ビットのうち）。
const MAX_DISTANCE: u32 = 64;
/// いちばん近い相手が、2 番目よりこれだけ近くなければ選ばない（似た模様の繰り返しを除く）。
const RATIO: f32 = 0.8;

/// `plane`（縮めた明るさ）の特徴点。`to_full` は `plane` の 1 画素が元の画像の何画素か。
pub fn detect(plane: &Plane, to_full: f64) -> Vec<Keypoint> {
    let pattern = pattern();
    let per_level = WANTED / LEVELS.len();
    LEVELS
        .par_iter()
        .flat_map(|&k| {
            let (w, h) = (
                ((plane.width as f64 * k).round() as usize).max(1),
                ((plane.height as f64 * k).round() as usize).max(1),
            );
            if w < 4 * RADIUS as usize || h < 4 * RADIUS as usize {
                return Vec::new();
            }
            let level = if k == 1.0 {
                plane.clone()
            } else {
                plane.resized(w, h)
            };
            let smooth = blurred(&level, 2.0);
            let corners = strongest(&level, per_level);
            let back = plane.width as f64 / w as f64 * to_full;
            corners
                .into_iter()
                .map(|(x, y)| {
                    let angle = orientation(&smooth, x, y);
                    Keypoint {
                        at: [(x as f64 + 0.5) * back - 0.5, (y as f64 + 0.5) * back - 0.5],
                        angle,
                        bits: describe(&smooth, x, y, angle, &pattern),
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `a` と `b` の特徴点のうち、互いにいちばん近い相手どうしの組（元の画像の画素の座標）。
pub fn matches(a: &[Keypoint], b: &[Keypoint]) -> Vec<Pair> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let nearest = |from: &Keypoint, to: &[Keypoint]| -> (usize, u32, u32) {
        let (mut best, mut first, mut second) = (0, u32::MAX, u32::MAX);
        for (j, k) in to.iter().enumerate() {
            let d = distance(&from.bits, &k.bits);
            if d < first {
                (second, first, best) = (first, d, j);
            } else if d < second {
                second = d;
            }
        }
        (best, first, second)
    };
    let back: Vec<usize> = b.par_iter().map(|k| nearest(k, a).0).collect();
    a.par_iter()
        .enumerate()
        .filter_map(|(i, k)| {
            let (j, first, second) = nearest(k, b);
            (first <= MAX_DISTANCE && (first as f32) < RATIO * second as f32 && back[j] == i)
                .then_some([k.at, b[j].at])
        })
        .collect()
}

fn distance(a: &[u64; 4], b: &[u64; 4]) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

/// 角の強さ（Shi–Tomasi の小さいほうの固有値）の山のうち強いもの。画面に散らすため、
/// 区画ごとに強い順で数を限る（模様の多い所に点が固まると、狭い重なりに点が残らない）。
fn strongest(p: &Plane, wanted: usize) -> Vec<(usize, usize)> {
    let (w, h) = (p.width, p.height);
    let mut gx = vec![0.0f32; w * h];
    let mut gy = vec![0.0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let v = |dx: isize, dy: isize| {
                p.data[((y as isize + dy) as usize) * w + (x as isize + dx) as usize]
            };
            gx[y * w + x] =
                (v(1, -1) + 2.0 * v(1, 0) + v(1, 1)) - (v(-1, -1) + 2.0 * v(-1, 0) + v(-1, 1));
            gy[y * w + x] =
                (v(-1, 1) + 2.0 * v(0, 1) + v(1, 1)) - (v(-1, -1) + 2.0 * v(0, -1) + v(1, -1));
        }
    }
    let xx = blurred(&Plane::new(w, h, gx.iter().map(|g| g * g).collect()), 1.5);
    let yy = blurred(&Plane::new(w, h, gy.iter().map(|g| g * g).collect()), 1.5);
    let xy = blurred(
        &Plane::new(w, h, gx.iter().zip(&gy).map(|(a, b)| a * b).collect()),
        1.5,
    );
    let score: Vec<f32> = (0..w * h)
        .map(|i| {
            let (a, b, c) = (xx.data[i], yy.data[i], xy.data[i]);
            0.5 * (a + b) - (0.25 * (a - b) * (a - b) + c * c).sqrt()
        })
        .collect();
    let top = score.iter().copied().fold(0.0f32, f32::max);
    if top <= 0.0 {
        return Vec::new();
    }
    let floor = top * 1e-3;
    let margin = (RADIUS as f64 * std::f64::consts::SQRT_2).ceil() as usize + 1;
    let mut peaks: Vec<(f32, usize, usize)> = Vec::new();
    for y in margin..h.saturating_sub(margin) {
        for x in margin..w.saturating_sub(margin) {
            let s = score[y * w + x];
            if s < floor {
                continue;
            }
            let is_peak = (-2isize..=2).all(|dy| {
                (-2isize..=2).all(|dx| {
                    (dx == 0 && dy == 0)
                        || score[((y as isize + dy) as usize) * w + (x as isize + dx) as usize] < s
                })
            });
            if is_peak {
                peaks.push((s, x, y));
            }
        }
    }
    peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
    const CELLS: usize = 8;
    let per_cell = wanted.div_ceil(CELLS * CELLS).max(1) * 2;
    let mut count = vec![0usize; CELLS * CELLS];
    let mut out = Vec::new();
    for (_, x, y) in peaks {
        let cell = (y * CELLS / h) * CELLS + x * CELLS / w;
        if count[cell] >= per_cell {
            continue;
        }
        count[cell] += 1;
        out.push((x, y));
        if out.len() >= wanted {
            break;
        }
    }
    out
}

/// 周りの円の明るさの重心の向き。
fn orientation(p: &Plane, x: usize, y: usize) -> f32 {
    let (mut mx, mut my) = (0.0f32, 0.0f32);
    for dy in -RADIUS..=RADIUS {
        for dx in -RADIUS..=RADIUS {
            if dx * dx + dy * dy > RADIUS * RADIUS {
                continue;
            }
            let v = p.at((x as isize + dx) as usize, (y as isize + dy) as usize);
            mx += dx as f32 * v;
            my += dy as f32 * v;
        }
    }
    my.atan2(mx)
}

/// 2 値の比べに使う 256 組の点（決まった種から。どの画像でも同じ組を使う）。
fn pattern() -> Vec<[[f32; 2]; 2]> {
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut uniform = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let sigma = (2 * RADIUS + 1) as f64 / 5.0;
    let mut gauss = move || {
        let (u, v) = (uniform().max(1e-12), uniform());
        let r = (-2.0 * u.ln()).sqrt() * sigma;
        let t = 2.0 * std::f64::consts::PI * v;
        let lim = (RADIUS - 1) as f64 / std::f64::consts::SQRT_2;
        [
            (r * t.cos()).clamp(-lim, lim) as f32,
            (r * t.sin()).clamp(-lim, lim) as f32,
        ]
    };
    (0..256).map(|_| [gauss(), gauss()]).collect()
}

fn describe(p: &Plane, x: usize, y: usize, angle: f32, pattern: &[[[f32; 2]; 2]]) -> [u64; 4] {
    let (s, c) = angle.sin_cos();
    let at = |q: [f32; 2]| {
        let u = (c * q[0] - s * q[1]).round() as isize;
        let v = (s * q[0] + c * q[1]).round() as isize;
        p.at((x as isize + u) as usize, (y as isize + v) as usize)
    };
    let mut bits = [0u64; 4];
    for (i, [a, b]) in pattern.iter().enumerate() {
        if at(*a) < at(*b) {
            bits[i / 64] |= 1 << (i % 64);
        }
    }
    bits
}

/// ガウスのぼかし（縦と横に分けて、端は端の値を伸ばす）。
fn blurred(p: &Plane, sigma: f64) -> Plane {
    let r = (sigma * 3.0).ceil() as isize;
    let kernel: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp() as f32)
        .collect();
    let sum: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.iter().map(|k| k / sum).collect();
    let (w, h) = (p.width, p.height);
    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, out) in row.iter_mut().enumerate() {
            *out = kernel
                .iter()
                .enumerate()
                .map(|(k, kv)| {
                    let xx = (x as isize + k as isize - r).clamp(0, w as isize - 1) as usize;
                    kv * p.data[y * w + xx]
                })
                .sum();
        }
    });
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            *o = kernel
                .iter()
                .enumerate()
                .map(|(k, kv)| {
                    let yy = (y as isize + k as isize - r).clamp(0, h as isize - 1) as usize;
                    kv * tmp[yy * w + x]
                })
                .sum();
        }
    });
    Plane::new(w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::affine::Affine;
    use crate::estimate::{Model, ransac};

    /// ぼけた点を散らした絵（どこを切り出しても似た所のない絵）。
    fn scene(w: usize, h: usize) -> Plane {
        let mut seed = 0x1234_5678_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            f64::from(seed) / f64::from(u32::MAX)
        };
        let mut v = vec![100.0f32; w * h];
        for _ in 0..w * h / 300 {
            let (bx, by, r, amp) = (
                next() * w as f64,
                next() * h as f64,
                1.5 + next() * 5.0,
                (next() * 2.0 - 1.0) * 120.0,
            );
            let reach = (r * 3.0) as isize;
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    let (x, y) = (bx as isize + dx, by as isize + dy);
                    if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
                        continue;
                    }
                    let d2 = (x as f64 - bx).powi(2) + (y as f64 - by).powi(2);
                    v[y as usize * w + x as usize] += (amp * (-d2 / (2.0 * r * r)).exp()) as f32;
                }
            }
        }
        Plane::new(w, h, v)
    }

    /// `t`（元の絵の座標 → 新しい絵の座標）で写した `w` × `h` の絵。
    fn warped(src: &Plane, t: &Affine, w: usize, h: usize) -> Plane {
        let back = t.inverse().unwrap();
        let data = (0..w * h)
            .map(|i| {
                let [x, y] = back.apply([(i % w) as f64, (i / w) as f64]);
                src.sample(x, y).unwrap_or(100.0)
            })
            .collect();
        Plane::new(w, h, data)
    }

    #[test]
    fn a_turned_and_shrunk_copy_is_matched_point_to_point() {
        let src = scene(500, 400);
        let t = Affine::similarity(0.8, 0.6, [250.0, 200.0], [220.0, 170.0]);
        let moved = warped(&src, &t, 440, 340);
        let pairs = matches(&detect(&src, 1.0), &detect(&moved, 1.0));
        let (h, inliers) = ransac(&pairs, Model::Similarity, 2.0).expect("a fit");
        assert!(inliers.len() >= 30, "{} of {}", inliers.len(), pairs.len());
        let q = h.apply([250.0, 200.0]).unwrap();
        assert!((q[0] - 220.0).hypot(q[1] - 170.0) < 1.0, "{q:?}");
    }

    #[test]
    fn unrelated_pictures_give_no_consistent_matches() {
        let a = scene(400, 300);
        let b = Plane::new(
            400,
            300,
            scene(700, 500)
                .data
                .chunks(700)
                .flat_map(|r| r[300..700].to_vec())
                .take(400 * 300)
                .collect(),
        );
        let pairs = matches(&detect(&a, 1.0), &detect(&b, 1.0));
        let found = ransac(&pairs, Model::Similarity, 2.0).map_or(0, |(_, i)| i.len());
        assert!(found < 10, "{found} of {}", pairs.len());
    }
}
