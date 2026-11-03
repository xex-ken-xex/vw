//! 星の並びで、2 枚の星の写真の対応を取る。
//!
//! 近い 3 つの星が作る三角形の辺の比は、写真を回しても大きさを変えても変わらない。
//! 両方の三角形の辺の比を照らし合わせ、合いそうな組から写し方の候補を作り、
//! いちばん多くの星を重ねる候補を選ぶ。WCS のない星の写真や、望遠鏡の違う写真（倍率が違う）、
//! 子午線越えで半回転した写真でも合う。

use crate::affine::Affine;
use crate::estimate::{Pair, similarity};

/// 使う星の数（明るい順）。
const STARS: usize = 40;
/// 三角形を作る近い星の数。
const NEIGHBOURS: usize = 5;
/// 辺の比の違いの上限。
const TOLERANCE: f64 = 0.01;
/// 星を重ねたとみなす距離（b の画素）。
const MATCH_RADIUS: f64 = 3.0;
/// 合ったとみなす重なった星の数の下限。
pub const MIN_STARS: usize = 6;

struct Triangle {
    /// 頂点（短い辺の向かいから順に）。
    vertices: [usize; 3],
    /// 短い辺 / 長い辺、中の辺 / 長い辺。
    shape: [f64; 2],
    /// 頂点の回る向き（鏡に映した三角形を取り違えないため）。
    clockwise: bool,
}

fn triangles(stars: &[[f64; 2]]) -> Vec<Triangle> {
    let n = stars.len();
    let d = |i: usize, j: usize| (stars[i][0] - stars[j][0]).hypot(stars[i][1] - stars[j][1]);
    let mut out = Vec::new();
    for i in 0..n {
        let mut near: Vec<usize> = (0..n).filter(|&j| j != i).collect();
        near.sort_by(|&x, &y| d(i, x).total_cmp(&d(i, y)));
        near.truncate(NEIGHBOURS);
        for (k, &j) in near.iter().enumerate() {
            for &l in &near[k + 1..] {
                // 同じ三角形を何度も数えないように、いちばん小さい番号の星から作ったものだけ残す。
                if i > j || i > l {
                    continue;
                }
                let mut sides = [(d(j, l), i), (d(i, l), j), (d(i, j), l)];
                sides.sort_by(|a, b| a.0.total_cmp(&b.0));
                let longest = sides[2].0;
                if longest < 8.0 || sides[0].0 / longest < 0.05 {
                    continue;
                }
                let v = [sides[0].1, sides[1].1, sides[2].1];
                let (p, q, r) = (stars[v[0]], stars[v[1]], stars[v[2]]);
                let cross = (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
                out.push(Triangle {
                    vertices: v,
                    shape: [sides[0].0 / longest, sides[1].0 / longest],
                    clockwise: cross > 0.0,
                });
            }
        }
    }
    out
}

/// 星の位置（明るい順、元の画像の画素）`a` と `b` から、a から b への写し方と重なった星の組。
/// 合わなければ `None`。
pub fn match_stars(a: &[[f64; 2]], b: &[[f64; 2]]) -> Option<(Affine, Vec<Pair>)> {
    let a = &a[..a.len().min(STARS)];
    let b = &b[..b.len().min(STARS)];
    if a.len() < MIN_STARS || b.len() < MIN_STARS {
        return None;
    }
    let (ta, tb) = (triangles(a), triangles(b));
    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (i, x) in ta.iter().enumerate() {
        for (j, y) in tb.iter().enumerate() {
            if x.clockwise != y.clockwise {
                continue;
            }
            let diff = (x.shape[0] - y.shape[0]).hypot(x.shape[1] - y.shape[1]);
            if diff < TOLERANCE {
                candidates.push((diff, i, j));
            }
        }
    }
    candidates.sort_by(|x, y| x.0.total_cmp(&y.0));
    candidates.truncate(3000);
    let mut best: Option<(usize, f64, Affine)> = None;
    for (_, i, j) in candidates {
        let pairs: Vec<Pair> = (0..3)
            .map(|k| [a[ta[i].vertices[k]], b[tb[j].vertices[k]]])
            .collect();
        let Some(t) = similarity(&pairs) else {
            continue;
        };
        let (count, spread) = overlap(a, b, &t);
        if best.is_none_or(|(c, s, _)| count > c || (count == c && spread < s)) {
            best = Some((count, spread, t));
        }
    }
    let (_, _, t) = best?;
    // 重なった星だけで直し、もう一度数える。
    let mut pairs = nearest_pairs(a, b, &t);
    let mut t = similarity(&pairs)?;
    for _ in 0..2 {
        let again = nearest_pairs(a, b, &t);
        if again.len() < pairs.len() {
            break;
        }
        pairs = again;
        t = similarity(&pairs)?;
    }
    (pairs.len() >= MIN_STARS).then_some((t, pairs))
}

/// `t` で写した a の星のうち、b の星に重なった数と、ずれの 2 乗の平均。
fn overlap(a: &[[f64; 2]], b: &[[f64; 2]], t: &Affine) -> (usize, f64) {
    let pairs = nearest_pairs(a, b, t);
    let spread = pairs
        .iter()
        .map(|[p, q]| {
            let m = t.apply(*p);
            (m[0] - q[0]).powi(2) + (m[1] - q[1]).powi(2)
        })
        .sum::<f64>()
        / pairs.len().max(1) as f64;
    (pairs.len(), spread)
}

/// `t` で写した a の星と、いちばん近い b の星の組（[`MATCH_RADIUS`] 以内、b の星は 1 度だけ）。
fn nearest_pairs(a: &[[f64; 2]], b: &[[f64; 2]], t: &Affine) -> Vec<Pair> {
    let mut taken = vec![false; b.len()];
    let mut out = Vec::new();
    for p in a {
        let m = t.apply(*p);
        let near = b
            .iter()
            .enumerate()
            .filter(|(j, _)| !taken[*j])
            .map(|(j, q)| (j, (q[0] - m[0]).hypot(q[1] - m[1])))
            .min_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((j, d)) = near
            && d <= MATCH_RADIUS
        {
            taken[j] = true;
            out.push([*p, b[j]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(n: usize, w: f64, h: f64) -> Vec<[f64; 2]> {
        let mut seed = 0x51ed_270b_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            f64::from(seed) / f64::from(u32::MAX)
        };
        (0..n).map(|_| [next() * w, next() * h]).collect()
    }

    #[test]
    fn a_half_turned_shrunk_star_field_is_matched() {
        let sky = field(80, 1200.0, 900.0);
        let t = Affine::similarity(0.7, std::f64::consts::PI, [600.0, 450.0], [400.0, 300.0]);
        // b は a の右半分ほどを写し、星をいくつか取りこぼしている。
        let a: Vec<[f64; 2]> = sky.iter().copied().filter(|p| p[0] < 900.0).collect();
        let b: Vec<[f64; 2]> = sky
            .iter()
            .enumerate()
            .filter(|(i, p)| p[0] > 400.0 && i % 7 != 0)
            .map(|(_, p)| t.apply(*p))
            .collect();
        let (got, pairs) = match_stars(&a, &b).expect("matched");
        assert!(pairs.len() >= 10, "{}", pairs.len());
        let q = got.apply([700.0, 400.0]);
        let want = t.apply([700.0, 400.0]);
        assert!(
            (q[0] - want[0]).hypot(q[1] - want[1]) < 0.01,
            "{q:?} {want:?}"
        );
    }

    #[test]
    fn two_different_star_fields_do_not_match() {
        let a = field(40, 1000.0, 800.0);
        let b: Vec<[f64; 2]> = field(80, 1000.0, 800.0)[40..].to_vec();
        assert!(match_stars(&a, &b).is_none());
    }
}
