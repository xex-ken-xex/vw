//! 重なった所の相関で見つからなかった組を、特徴点や星の並びで探す。
//!
//! 相関はぼけた写真や模様の少ない所に強く、特徴点は大きな回転や狭い重なりに強い。
//! そこで相関で見つかった組はそのまま使い（結果を変えないため）、見つからなかった組だけを
//! 特徴点（星の写真は星の並び）で探す。

use std::collections::HashMap;

use rayon::prelude::*;

use crate::affine::Affine;
use crate::estimate::{self, Model, Pair};
use crate::features::{self, Keypoint};
use crate::refine;
use crate::stars;

use super::{Link, LinkKind, MIN_OVERLAP, Piece, link_from, shared};

/// 特徴点の組をいつ使うか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeatureUse {
    /// 相関で見つからなかった組だけ。
    #[default]
    Fallback,
    /// どの組も両方で探し、確かなほうを使う。
    Always,
    /// 使わない。
    Never,
}

/// 対応する特徴点の組がこれより少なければ、組とみなさない。
const MIN_FEATURES: usize = 15;
/// 細かく直せなかったとき、特徴点だけで組とみなす対応の数。
const SURE_FEATURES: usize = 30;
/// 星の並びだけで組とみなす星の数。
const SURE_STARS: usize = 8;
/// 特徴点の位置の誤差の見込み（縮めた画像の画素）。
const FEATURE_PIXELS: f64 = 3.0;

/// 画像ごとの特徴点（並べた画像と同じ順）。
pub fn keypoints(pieces: &[Piece]) -> Vec<Vec<Keypoint>> {
    pieces
        .iter()
        .map(|p| features::detect(&p.plane, 1.0 / p.scale()))
        .collect()
}

/// どの 2 枚についても、外れを除いた特徴点の組（a の元の画素, b の元の画素）。
/// 射影や円筒で全体を直すのに使う（相関で見つけた組の点は、アフィンで写した格子の点なので
/// 遠近のゆがみを持たないため）。組の見つからない 2 枚は入れない。
pub fn feature_pairs(
    pieces: &[Piece],
    keypoints: &[Vec<Keypoint>],
    progress: &(dyn Fn() + Sync),
) -> HashMap<(usize, usize), Vec<Pair>> {
    let n = pieces.len();
    let pairs: Vec<(usize, usize)> = (0..n)
        .flat_map(|a| (a + 1..n).map(move |b| (a, b)))
        .collect();
    pairs
        .par_iter()
        .filter_map(|&(a, b)| {
            let found = consistent(&pieces[a], &pieces[b], &keypoints[a], &keypoints[b]);
            progress();
            found.map(|p| ((a, b), p))
        })
        .collect()
}

/// [`feature_pairs`] が呼ぶ `progress` の回数。
pub fn feature_steps(pieces: usize) -> usize {
    pieces * pieces.saturating_sub(1) / 2
}

fn consistent(pa: &Piece, pb: &Piece, ka: &[Keypoint], kb: &[Keypoint]) -> Option<Vec<Pair>> {
    let candidates = features::matches(ka, kb);
    if candidates.len() < MIN_FEATURES {
        return None;
    }
    let threshold = FEATURE_PIXELS / pb.scale();
    let (h, inliers) = estimate::ransac(&candidates, Model::Projective, threshold)?;
    if inliers.len() < MIN_FEATURES {
        return None;
    }
    // 同じ撮り方で分けて撮った画像どうしとしてありえない写し方（裏返し、極端な伸び縮み）は捨てる。
    let a = estimate::affine(&inliers.iter().map(|&i| candidates[i]).collect::<Vec<_>>())?;
    let det = a.a * a.e - a.b * a.d;
    if !(0.1..10.0).contains(&det) || h.as_affine().is_none() && h.inverse().is_none() {
        return None;
    }
    let refined: Vec<Pair> = inliers
        .into_iter()
        .filter_map(|i| sharpen(pa, pb, &h, candidates[i]))
        .collect();
    (refined.len() >= MIN_FEATURES).then_some(refined)
}

/// 模様を比べる窓の半径（縮めた画像の画素）。
const PATCH: isize = 8;

/// 特徴点の対応 `[p, q]` の `q` を、`p` のまわりの模様が `b` でいちばんよく重なる所へ、画素より
/// 細かく動かす（Lucas–Kanade のずらしだけ。窓の形は `h` で写す）。特徴点は縮めた段の画素の上で
/// 見つけるので、そのままでは数画素の誤差があり、全体を直してもつなぎ目にずれが残るため。
/// 合わない点は `None`。
fn sharpen(pa: &Piece, pb: &Piece, h: &crate::warp::Homography, [p, _]: Pair) -> Option<Pair> {
    let (sa, sb) = (pa.scale(), pb.scale());
    let small = |v: [f64; 2], s: f64| [(v[0] + 0.5) * s - 0.5, (v[1] + 0.5) * s - 0.5];
    let pa_small = small(p, sa);
    // 窓の 1 画素（a の縮めた画素）が、b の縮めた画素でどう伸びるか（h の微分）。
    let step = 1.0 / sa;
    let at = |dx: f64, dy: f64| {
        h.apply([p[0] + dx * step, p[1] + dy * step])
            .map(|v| small(v, sb))
    };
    let (centre, right, down) = (at(0.0, 0.0)?, at(1.0, 0.0)?, at(0.0, 1.0)?);
    let jx = [right[0] - centre[0], right[1] - centre[1]];
    let jy = [down[0] - centre[0], down[1] - centre[1]];
    // RANSAC で残った点は h のそばにあるので、見つけた点ではなく h の予想から始める
    // （見つけた点は縮めた段の画素に丸まっている）。
    let mut offset = centre;
    let template: Vec<(f64, f64, f32)> = (-PATCH..=PATCH)
        .flat_map(|v| (-PATCH..=PATCH).map(move |u| (u as f64, v as f64)))
        .filter_map(|(u, v)| {
            let t = pa.plane.sample(pa_small[0] + u, pa_small[1] + v)?;
            Some((u, v, t))
        })
        .collect();
    if template.len() < ((2 * PATCH + 1) * (2 * PATCH + 1)) as usize {
        return None;
    }
    let mean_t = template.iter().map(|t| f64::from(t.2)).sum::<f64>() / template.len() as f64;
    for _ in 0..10 {
        let (mut hxx, mut hxy, mut hyy, mut gx, mut gy) = (0.0, 0.0, 0.0, 0.0, 0.0);
        let mut values = Vec::with_capacity(template.len());
        for &(u, v, _) in &template {
            let x = offset[0] + jx[0] * u + jy[0] * v;
            let y = offset[1] + jx[1] * u + jy[1] * v;
            values.push((x, y, pb.plane.sample(x, y)?));
        }
        let mean_b = values.iter().map(|v| f64::from(v.2)).sum::<f64>() / values.len() as f64;
        for (&(_, _, t), &(x, y, b)) in template.iter().zip(&values) {
            let dx = f64::from(pb.plane.sample(x + 0.5, y)? - pb.plane.sample(x - 0.5, y)?);
            let dy = f64::from(pb.plane.sample(x, y + 0.5)? - pb.plane.sample(x, y - 0.5)?);
            let e = (f64::from(b) - mean_b) - (f64::from(t) - mean_t);
            hxx += dx * dx;
            hxy += dx * dy;
            hyy += dy * dy;
            gx += dx * e;
            gy += dy * e;
        }
        let det = hxx * hyy - hxy * hxy;
        if det.abs() < 1e-9 {
            return None;
        }
        let ddx = -(hyy * gx - hxy * gy) / det;
        let ddy = -(hxx * gy - hxy * gx) / det;
        offset = [offset[0] + ddx, offset[1] + ddy];
        if (offset[0] - centre[0]).hypot(offset[1] - centre[1]) > 3.0 {
            return None;
        }
        if ddx.hypot(ddy) < 0.01 {
            break;
        }
    }
    // 重ねた窓の相関が低ければ、模様が違う（遠近で隠れた所など）。
    let mut values = Vec::with_capacity(template.len());
    for &(u, v, _) in &template {
        let x = offset[0] + jx[0] * u + jy[0] * v;
        let y = offset[1] + jx[1] * u + jy[1] * v;
        values.push(f64::from(pb.plane.sample(x, y)?));
    }
    let t: Vec<f64> = template.iter().map(|t| f64::from(t.2)).collect();
    if pearson(&t, &values) < 0.8 {
        return None;
    }
    let full = [(offset[0] + 0.5) / sb - 0.5, (offset[1] + 0.5) / sb - 0.5];
    Some([p, full])
}

fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        ab += (x - ma) * (y - mb);
        aa += (x - ma) * (x - ma);
        bb += (y - mb) * (y - mb);
    }
    let den = (aa * bb).sqrt();
    if den > 1e-12 { ab / den } else { 0.0 }
}

/// 星の写真の、画像ごとの星の位置（明るい順、元の画素）。星の写真でなければ `None`。
pub type Stars = Option<Vec<[f64; 2]>>;

/// 相関で見つからなかった組（`use_features` が `Always` ならどの組も）を、星の並びと特徴点で
/// 探して `links` に足す。
pub fn link_more(
    pieces: &[Piece],
    stars: &[Stars],
    features: &HashMap<(usize, usize), Vec<Pair>>,
    use_features: FeatureUse,
    links: &mut Vec<Link>,
) {
    let n = pieces.len();
    let linked = |links: &[Link], a: usize, b: usize| {
        links
            .iter()
            .position(|l| (l.a, l.b) == (a, b) || (l.a, l.b) == (b, a))
    };
    let pairs: Vec<(usize, usize)> = (0..n)
        .flat_map(|a| (a + 1..n).map(move |b| (a, b)))
        .filter(|&(a, b)| match linked(links, a, b) {
            None => true,
            // WCS の組は、星や特徴点より確か。
            Some(i) => use_features == FeatureUse::Always && links[i].kind == LinkKind::Overlap,
        })
        .collect();
    let found: Vec<Link> = pairs
        .par_iter()
        .filter_map(|&(a, b)| {
            let by_stars = match (&stars[a], &stars[b]) {
                (Some(sa), Some(sb)) => star_link(pieces, a, b, sa, sb),
                _ => None,
            };
            by_stars.or_else(|| {
                if use_features == FeatureUse::Never {
                    return None;
                }
                feature_link(pieces, a, b, features.get(&(a, b))?)
            })
        })
        .collect();
    for l in found {
        match linked(links, l.a, l.b) {
            Some(i) if links[i].correlation >= l.correlation => {}
            Some(i) => links[i] = l,
            None => links.push(l),
        }
    }
}

fn star_link(
    pieces: &[Piece],
    a: usize,
    b: usize,
    sa: &[[f64; 2]],
    sb: &[[f64; 2]],
) -> Option<Link> {
    let (t, points) = stars::match_stars(sa, sb)?;
    settle(pieces, a, b, t, points, SURE_STARS, LinkKind::Stars)
}

fn feature_link(pieces: &[Piece], a: usize, b: usize, points: &[Pair]) -> Option<Link> {
    let start = estimate::affine(points)?;
    settle(
        pieces,
        a,
        b,
        start,
        points.to_vec(),
        SURE_FEATURES,
        LinkKind::Features,
    )
}

/// 星や特徴点で見つけた写し方 `start` を、重なりの絵で細かく直す。直せなければ、対応が
/// `sure` 以上あるときだけ `start` のまま組にする（遠近のゆがみがあると、アフィンでは
/// 重なりの絵の相関が上がりきらないため）。
fn settle(
    pieces: &[Piece],
    a: usize,
    b: usize,
    start: Affine,
    points: Vec<Pair>,
    sure: usize,
    kind: LinkKind,
) -> Option<Link> {
    if let Some(mut l) = link_from(pieces, a, b, start) {
        l.kind = kind;
        l.points = points;
        return Some(l);
    }
    if points.len() < sure {
        return None;
    }
    let (pa, pb) = (&pieces[a], &pieces[b]);
    let overlap = shared(
        pa.full,
        &Affine::IDENTITY.into(),
        pb.full,
        &start.inverse()?.into(),
    );
    if overlap < MIN_OVERLAP {
        return None;
    }
    let small = start.between_scales(pa.scale(), pb.scale());
    let (correlation, _) = refine::score(&pa.plane, &pb.plane, &small);
    Some(Link {
        a,
        b,
        map: start,
        correlation: correlation.max(0.0),
        overlap,
        kind,
        points,
    })
}
