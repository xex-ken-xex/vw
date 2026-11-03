//! 対応する点の組から写し方を求める（最小二乗と RANSAC）。
//!
//! 点の組は `[a の点, b の点]` で、求めるのは a から b への写し方。

use crate::affine::Affine;
use crate::linalg::solve;
use crate::warp::Homography;

/// 対応する 1 組の点（a の点, b の点）。
pub type Pair = [[f64; 2]; 2];

/// 最小二乗のアフィン。点が 3 つ未満か、一直線に並んでいれば `None`。
pub fn affine(pairs: &[Pair]) -> Option<Affine> {
    if pairs.len() < 3 {
        return None;
    }
    // x と y の行を別々に解く（同じ係数の行列）。
    let mut h = vec![vec![0.0f64; 3]; 3];
    let (mut gx, mut gy) = (vec![0.0f64; 3], vec![0.0f64; 3]);
    let (cx, cy) = centre(pairs, 0);
    for [p, q] in pairs {
        let row = [p[0] - cx, p[1] - cy, 1.0];
        for i in 0..3 {
            for j in 0..3 {
                h[i][j] += row[i] * row[j];
            }
            gx[i] += row[i] * q[0];
            gy[i] += row[i] * q[1];
        }
    }
    let x = solve(h.clone(), gx)?;
    let y = solve(h, gy)?;
    let centred = Affine {
        a: x[0],
        b: x[1],
        c: x[2],
        d: y[0],
        e: y[1],
        f: y[2],
    };
    Some(
        Affine {
            c: -cx,
            f: -cy,
            ..Affine::IDENTITY
        }
        .then(&centred),
    )
}

/// 最小二乗の相似（ずらす・回す・大きさを変える。縦横の比やせん断は持たない）。
pub fn similarity(pairs: &[Pair]) -> Option<Affine> {
    if pairs.len() < 2 {
        return None;
    }
    let (ax, ay) = centre(pairs, 0);
    let (bx, by) = centre(pairs, 1);
    let (mut sxx, mut sxy, mut norm) = (0.0, 0.0, 0.0);
    for [p, q] in pairs {
        let (px, py) = (p[0] - ax, p[1] - ay);
        let (qx, qy) = (q[0] - bx, q[1] - by);
        sxx += px * qx + py * qy;
        sxy += px * qy - py * qx;
        norm += px * px + py * py;
    }
    if norm < 1e-12 {
        return None;
    }
    let (a, d) = (sxx / norm, sxy / norm);
    Some(Affine {
        a,
        b: -d,
        c: bx - a * ax + d * ay,
        d,
        e: a,
        f: by - d * ax - a * ay,
    })
}

/// 最小二乗の射影変換（点の座標を平均 0・大きさ √2 にそろえてから解く DLT）。
/// 点が 4 つ未満か、解けなければ `None`。
pub fn homography(pairs: &[Pair]) -> Option<Homography> {
    if pairs.len() < 4 {
        return None;
    }
    let na = normalising(pairs, 0)?;
    let nb = normalising(pairs, 1)?;
    // 右下を 1 とおいた 8 個の未知数の最小二乗（正規方程式）。
    let mut h = vec![vec![0.0f64; 8]; 8];
    let mut g = vec![0.0f64; 8];
    for [p, q] in pairs {
        let [x, y] = na.apply(*p);
        let [u, v] = nb.apply(*q);
        let rows = [
            ([x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], u),
            ([0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y], v),
        ];
        for (r, t) in rows {
            for i in 0..8 {
                for j in 0..8 {
                    h[i][j] += r[i] * r[j];
                }
                g[i] += r[i] * t;
            }
        }
    }
    let s = solve(h, g)?;
    let m = Homography([[s[0], s[1], s[2]], [s[3], s[4], s[5]], [s[6], s[7], 1.0]]);
    let to_b = Homography::from_affine(&nb.inverse()?);
    Some(Homography::from_affine(&na).then(&m).then(&to_b))
}

/// 点の平均を 0、原点からの平均の距離を √2 にするアフィン。
fn normalising(pairs: &[Pair], side: usize) -> Option<Affine> {
    let (cx, cy) = centre(pairs, side);
    let mean = pairs
        .iter()
        .map(|p| (p[side][0] - cx).hypot(p[side][1] - cy))
        .sum::<f64>()
        / pairs.len() as f64;
    if mean < 1e-12 {
        return None;
    }
    let k = std::f64::consts::SQRT_2 / mean;
    Some(Affine {
        a: k,
        b: 0.0,
        c: -k * cx,
        d: 0.0,
        e: k,
        f: -k * cy,
    })
}

fn centre(pairs: &[Pair], side: usize) -> (f64, f64) {
    let n = pairs.len().max(1) as f64;
    let (sx, sy) = pairs
        .iter()
        .fold((0.0, 0.0), |(x, y), p| (x + p[side][0], y + p[side][1]));
    (sx / n, sy / n)
}

/// 求める写し方の形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Similarity,
    Projective,
}

/// RANSAC で外れた組を除き、残った組で最小二乗した写し方と、残った組の番号。
/// 点の選び方は決まった種から（同じ入力なら同じ結果にするため）。
pub fn ransac(pairs: &[Pair], model: Model, threshold: f64) -> Option<(Homography, Vec<usize>)> {
    let need = match model {
        Model::Similarity => 2,
        Model::Projective => 4,
    };
    if pairs.len() < need + 2 {
        return None;
    }
    let fit = |sample: &[Pair]| -> Option<Homography> {
        match model {
            Model::Similarity => similarity(sample).map(|t| Homography::from_affine(&t)),
            Model::Projective => homography(sample),
        }
    };
    let inliers = |h: &Homography| -> Vec<usize> {
        (0..pairs.len())
            .filter(|&i| {
                let [p, q] = pairs[i];
                h.apply(p)
                    .is_some_and(|m| (m[0] - q[0]).hypot(m[1] - q[1]) <= threshold)
            })
            .collect()
    };
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    let iterations = match model {
        Model::Similarity => 300,
        Model::Projective => 800,
    };
    let mut best: Vec<usize> = Vec::new();
    for _ in 0..iterations {
        let mut picked: Vec<usize> = Vec::with_capacity(need);
        while picked.len() < need {
            let i = next(pairs.len());
            if !picked.contains(&i) {
                picked.push(i);
            }
        }
        let sample: Vec<Pair> = picked.iter().map(|&i| pairs[i]).collect();
        let Some(h) = fit(&sample) else {
            continue;
        };
        let found = inliers(&h);
        if found.len() > best.len() {
            best = found;
            if best.len() == pairs.len() {
                break;
            }
        }
    }
    if best.len() < need + 2 {
        return None;
    }
    // 残った組で直し、もう一度数え直す（最初の数点だけで決めた写し方より確か）。
    let mut h = fit(&best.iter().map(|&i| pairs[i]).collect::<Vec<_>>())?;
    for _ in 0..2 {
        let again = inliers(&h);
        if again.len() < best.len() {
            break;
        }
        best = again;
        h = fit(&best.iter().map(|&i| pairs[i]).collect::<Vec<_>>())?;
    }
    Some((h, best))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Vec<[f64; 2]> {
        (0..6)
            .flat_map(|j| (0..6).map(move |i| [i as f64 * 37.0, j as f64 * 29.0]))
            .collect()
    }

    #[test]
    fn an_exact_affine_is_recovered_by_least_squares() {
        let t = Affine {
            a: 1.02,
            b: 0.1,
            c: 15.0,
            d: -0.05,
            e: 0.97,
            f: -8.0,
        };
        let pairs: Vec<Pair> = grid().into_iter().map(|p| [p, t.apply(p)]).collect();
        let got = affine(&pairs).unwrap();
        for p in grid() {
            let (u, v) = (got.apply(p), t.apply(p));
            assert!((u[0] - v[0]).abs() < 1e-9 && (u[1] - v[1]).abs() < 1e-9);
        }
    }

    #[test]
    fn an_exact_similarity_is_recovered() {
        let t = Affine::similarity(1.3, -0.7, [10.0, 20.0], [100.0, -40.0]);
        let pairs: Vec<Pair> = grid().into_iter().map(|p| [p, t.apply(p)]).collect();
        let got = similarity(&pairs).unwrap();
        assert!((got.a - t.a).abs() < 1e-12 && (got.f - t.f).abs() < 1e-9);
    }

    #[test]
    fn an_exact_homography_is_recovered() {
        let h = Homography([[0.9, 0.05, 40.0], [0.02, 1.1, -10.0], [3e-4, -1e-4, 1.0]]);
        let pairs: Vec<Pair> = grid()
            .into_iter()
            .map(|p| [p, h.apply(p).unwrap()])
            .collect();
        let got = homography(&pairs).unwrap();
        for p in grid() {
            let (u, v) = (got.apply(p).unwrap(), h.apply(p).unwrap());
            assert!((u[0] - v[0]).abs() < 1e-6 && (u[1] - v[1]).abs() < 1e-6);
        }
    }

    #[test]
    fn ransac_finds_the_shift_although_half_of_the_pairs_are_wrong() {
        let t = Affine::similarity(1.0, 0.2, [0.0, 0.0], [50.0, 10.0]);
        let mut pairs: Vec<Pair> = grid().into_iter().map(|p| [p, t.apply(p)]).collect();
        // 半分の組を、でたらめな相手にする。
        let n = pairs.len();
        for i in (0..n).step_by(2) {
            pairs[i][1] = [(i * 53 % 211) as f64, (i * 97 % 157) as f64];
        }
        let (h, inliers) = ransac(&pairs, Model::Projective, 1.0).unwrap();
        assert_eq!(inliers.len(), n / 2);
        let q = h.apply([100.0, 100.0]).unwrap();
        let want = t.apply([100.0, 100.0]);
        assert!((q[0] - want[0]).abs() < 1e-6 && (q[1] - want[1]).abs() < 1e-6);
    }
}
