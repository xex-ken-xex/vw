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
            let found = consistent(&pieces[b], &keypoints[a], &keypoints[b]);
            progress();
            found.map(|p| ((a, b), p))
        })
        .collect()
}

/// [`feature_pairs`] が呼ぶ `progress` の回数。
pub fn feature_steps(pieces: usize) -> usize {
    pieces * pieces.saturating_sub(1) / 2
}

fn consistent(pb: &Piece, ka: &[Keypoint], kb: &[Keypoint]) -> Option<Vec<Pair>> {
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
    Some(inliers.into_iter().map(|i| candidates[i]).collect())
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
