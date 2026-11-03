//! 全部の組の対応する点を使って、全体の置き方をまとめて直す（Levenberg–Marquardt）。
//!
//! [`super::tree`] は確かな組を 1 本ずつたどって置くので、枚数が多いと端で誤差がたまり、
//! 輪になった所（2 × 2 など）が閉じない。ここでは、置いた画像どうしのすべての組の点が
//! 貼り合わせの座標で重なるように、全部の置き方を少しずつ動かす。基準の 1 枚は動かさない。
//!
//! 写し方は 3 通り（[`Projection`]）。円筒は、カメラをその場で回して撮ったとみなし、画像ごとの
//! カメラの向きと、共通の焦点距離を求める。

use std::collections::HashMap;

use crate::affine::Affine;
use crate::estimate::{self, Pair};
use crate::linalg::{
    Mat3, cross3, dot3, mul3, normalized3, rotation_of, solve, symmetric_eigen, transpose3,
};
use crate::warp::{Cylinder, Homography, Warp};

use super::Link;

/// 全体の写し方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    /// ずらす・回す・大きさを変える。
    Affine,
    /// 平面への射影（遠近のゆがみを直す）。
    Projective,
    /// 円筒への投影（その場で回して撮った広いパノラマ）。
    Cylinder,
}

/// 直した結果。
#[derive(Debug, Clone, PartialEq)]
pub struct Adjusted {
    pub placed: Vec<Option<Warp>>,
    pub projection: Projection,
    /// 直す前と後の、対応する点のずれの平均（貼り合わせの座標の画素）。
    pub before: f64,
    pub after: f64,
    /// 円筒で使った焦点距離（画素）。
    pub focal: Option<f64>,
}

/// 直せなかったわけ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// 円筒にするのに要る焦点距離が分からない。
    NoFocal,
    /// 写した結果が、画像の向こう側へ回り込んだり、無限に伸びたりした。
    Unstable,
}

/// 外れた点に引きずられないように、ずれがこれより大きい点は重みを下げる（画素）。
const HUBER: f64 = 2.0;
const ITERATIONS: usize = 40;

/// 置いた画像（`tree`）を、`links` の点がそろうように `projection` で直す。`focal` は円筒の
/// 焦点距離（画素、分かれば。分からなければ組の射影変換から推定する）。
pub fn adjust(
    sizes: &[[usize; 2]],
    links: &[Link],
    tree: &[Option<(Affine, Option<usize>)>],
    projection: Projection,
    focal: Option<f64>,
) -> Result<Adjusted, Failure> {
    let n = sizes.len();
    let used: Vec<&Link> = links
        .iter()
        .filter(|l| tree[l.a].is_some() && tree[l.b].is_some() && !l.points.is_empty())
        .collect();
    let anchor = tree.iter().position(|p| matches!(p, Some((_, None))));
    let Some(anchor) = anchor else {
        return Ok(Adjusted {
            placed: vec![None; n],
            projection,
            before: 0.0,
            after: 0.0,
            focal: None,
        });
    };
    let model = match projection {
        Projection::Affine => Model::affine(tree),
        Projection::Projective => Model::projective(tree, links, anchor),
        Projection::Cylinder => {
            let f = focal
                .or_else(|| estimate_focal(sizes, links))
                .ok_or(Failure::NoFocal)?;
            Model::cylinder(sizes, tree, links, anchor, f).ok_or(Failure::NoFocal)?
        }
    };
    let mut free = vec![false; n];
    for (i, p) in tree.iter().enumerate() {
        free[i] = p.is_some() && i != anchor;
    }
    let before = mean_error(&model, &model.params, &used);
    let mut params = model.params.clone();
    optimise(&model, &mut params, &free, &used);
    let after = mean_error(&model, &params, &used);
    let placed: Vec<Option<Warp>> = match &model.kind {
        // 直したあとの向きで、もう一度水平にそろえる（初めの焦点距離が推定でずれていると、
        // 初めにそろえた水平が少し傾いているため）。
        Kind::Cylinder {
            centres,
            start,
            radius,
        } => {
            let turned: Vec<Mat3> = (0..n)
                .map(|i| {
                    let p = &params[i * model.per..(i + 1) * model.per];
                    mul3(&rotation_of([p[0], p[1], p[2]]), &start[i])
                })
                .collect();
            let level = straightened(turned);
            let f = params[model.globals_at];
            (0..n)
                .map(|i| {
                    tree[i].map(|_| Warp::Cylinder(cylinder_warp(centres[i], f, level[i], *radius)))
                })
                .collect()
        }
        Kind::Projective => recentred(
            sizes,
            (0..n)
                .map(|i| tree[i].map(|_| model.warp(i, &params)))
                .collect(),
        ),
        Kind::Affine => (0..n)
            .map(|i| tree[i].map(|_| model.warp(i, &params)))
            .collect(),
    };
    if !stable(sizes, &placed) {
        return Err(Failure::Unstable);
    }
    Ok(Adjusted {
        placed,
        projection,
        before,
        after,
        focal: match model.kind {
            Kind::Cylinder { .. } => Some(params[model.globals_at]),
            _ => None,
        },
    })
}

/// 射影で置いた画像を、全体がいちばん小さくなる 1 枚を正面にして置き直す。基準の 1 枚は
/// 組の確かさで選ぶので、端の画像が基準になると、反対の端が遠近で大きく引き伸ばされるため。
fn recentred(sizes: &[[usize; 2]], placed: Vec<Option<Warp>>) -> Vec<Option<Warp>> {
    let homography = |w: &Option<Warp>| match w {
        Some(Warp::Projective(h)) => Some(*h),
        Some(Warp::Affine(t)) => Some(Homography::from_affine(t)),
        _ => None,
    };
    let area = |p: &[Option<Warp>]| {
        let whole = p
            .iter()
            .zip(sizes)
            .all(|(w, s)| w.as_ref().is_none_or(|w| w.outline(*s, 8).len() == 32));
        super::bounds(sizes, p)
            .filter(|_| whole)
            .map(|b| (b[2] - b[0]) * (b[3] - b[1]))
    };
    let mut best = (area(&placed), placed.clone());
    for centre in placed.iter().filter_map(homography) {
        let Some(back) = centre.inverse() else {
            continue;
        };
        let moved: Vec<Option<Warp>> = placed
            .iter()
            .map(|w| homography(w).map(|h| Warp::Projective(h.then(&back).normalized())))
            .collect();
        let a = area(&moved);
        if a.is_some_and(|a| best.0.is_none_or(|b| a < b)) {
            best = (a, moved);
        }
    }
    best.1
}

/// 写した画像がどれも、縁を全部写せて、ありえない大きさに伸びていないか。
fn stable(sizes: &[[usize; 2]], placed: &[Option<Warp>]) -> bool {
    let mut area = 0.0;
    for (s, p) in sizes.iter().zip(placed) {
        let Some(p) = p else {
            continue;
        };
        let outline = p.outline(*s, 8);
        let want = if matches!(p, Warp::Affine(_)) { 4 } else { 32 };
        if outline.len() < want
            || outline
                .iter()
                .any(|q| !q[0].is_finite() || !q[1].is_finite())
        {
            return false;
        }
        area += (s[0] * s[1]) as f64;
    }
    super::bounds(sizes, placed).is_some_and(|b| (b[2] - b[0]) * (b[3] - b[1]) <= 50.0 * area)
}

/// 写し方の形と、画像ごとの値の並べ方。
struct Model {
    kind: Kind,
    /// 画像ごとの値の数。
    per: usize,
    /// 全画像に共通の値の始まり（`per × 枚数` の後ろ）。
    globals_at: usize,
    /// 初めの値。
    params: Vec<f64>,
    /// 値ごとの、微分を数値で求める刻み。
    steps: Vec<f64>,
}

enum Kind {
    Affine,
    Projective,
    Cylinder {
        centres: Vec<[f64; 2]>,
        /// 初めの向き（値は、これに掛ける小さな回転）。
        start: Vec<Mat3>,
        /// 貼り合わせの 1 ラジアンの画素（初めの焦点距離。直す間は変えない。変えると、
        /// 全体を縮めるだけでずれが小さく見えてしまうため）。
        radius: f64,
    },
}

impl Model {
    fn affine(tree: &[Option<(Affine, Option<usize>)>]) -> Model {
        let per = 6;
        let mut params = Vec::with_capacity(tree.len() * per);
        for p in tree {
            let t = p.map_or(Affine::IDENTITY, |(t, _)| t);
            params.extend([t.a, t.b, t.c, t.d, t.e, t.f]);
        }
        let steps = (0..tree.len())
            .flat_map(|_| [1e-6, 1e-6, 1e-3, 1e-6, 1e-6, 1e-3])
            .collect();
        Model {
            kind: Kind::Affine,
            per,
            globals_at: params.len(),
            params,
            steps,
        }
    }

    /// 基準から組をたどり、組ごとの射影変換をつないで初めの置き方にする。
    fn projective(
        tree: &[Option<(Affine, Option<usize>)>],
        links: &[Link],
        anchor: usize,
    ) -> Model {
        let n = tree.len();
        let mut placed: Vec<Option<Homography>> = vec![None; n];
        placed[anchor] = tree[anchor].map(|(t, _)| Homography::from_affine(&t));
        let mut changed = true;
        while changed {
            changed = false;
            for i in 0..n {
                let (None, Some((_, Some(l)))) = (placed[i], tree[i]) else {
                    continue;
                };
                let link = &links[l];
                let pair = pair_homography(link);
                let done = if link.b == i {
                    // b の画素 → a の画素 → 貼り合わせ。
                    placed[link.a]
                        .zip(pair.inverse())
                        .map(|(pa, back)| back.then(&pa))
                } else {
                    placed[link.b].map(|pb| pair.then(&pb))
                };
                if let Some(h) = done {
                    placed[i] = Some(h);
                    changed = true;
                }
            }
        }
        let per = 8;
        let mut params = Vec::with_capacity(n * per);
        for (i, p) in placed.iter().enumerate() {
            let h = p
                .or_else(|| tree[i].map(|(t, _)| Homography::from_affine(&t)))
                .unwrap_or(Homography::IDENTITY)
                .normalized()
                .0;
            params.extend([
                h[0][0], h[0][1], h[0][2], h[1][0], h[1][1], h[1][2], h[2][0], h[2][1],
            ]);
        }
        let steps = (0..n)
            .flat_map(|_| [1e-6, 1e-6, 1e-3, 1e-6, 1e-6, 1e-3, 1e-10, 1e-10])
            .collect();
        Model {
            kind: Kind::Projective,
            per,
            globals_at: params.len(),
            params,
            steps,
        }
    }

    fn cylinder(
        sizes: &[[usize; 2]],
        tree: &[Option<(Affine, Option<usize>)>],
        links: &[Link],
        anchor: usize,
        focal: f64,
    ) -> Option<Model> {
        let n = sizes.len();
        let centres: Vec<[f64; 2]> = sizes
            .iter()
            .map(|s| [(s[0] as f64 - 1.0) / 2.0, (s[1] as f64 - 1.0) / 2.0])
            .collect();
        let start = straightened(rotations(tree, links, anchor, &centres, focal)?);
        let per = 3;
        let mut params = vec![0.0; n * per];
        params.push(focal);
        let mut steps = vec![1e-6; n * per];
        steps.push(1e-3 * focal.max(1.0));
        Some(Model {
            kind: Kind::Cylinder {
                centres,
                start,
                radius: focal,
            },
            per,
            globals_at: n * per,
            params,
            steps,
        })
    }

    fn warp(&self, i: usize, params: &[f64]) -> Warp {
        let p = &params[i * self.per..(i + 1) * self.per];
        match &self.kind {
            Kind::Affine => Warp::Affine(Affine {
                a: p[0],
                b: p[1],
                c: p[2],
                d: p[3],
                e: p[4],
                f: p[5],
            }),
            Kind::Projective => Warp::Projective(Homography([
                [p[0], p[1], p[2]],
                [p[3], p[4], p[5]],
                [p[6], p[7], 1.0],
            ])),
            Kind::Cylinder {
                centres,
                start,
                radius,
            } => {
                let f = params[self.globals_at];
                Warp::Cylinder(cylinder_warp(
                    centres[i],
                    f,
                    mul3(&rotation_of([p[0], p[1], p[2]]), &start[i]),
                    *radius,
                ))
            }
        }
    }
}

fn cylinder_warp(centre: [f64; 2], focal: f64, rotation: Mat3, radius: f64) -> Cylinder {
    Cylinder {
        to_camera: Affine {
            a: 1.0 / focal,
            b: 0.0,
            c: -centre[0] / focal,
            d: 0.0,
            e: 1.0 / focal,
            f: -centre[1] / focal,
        },
        rotation,
        to_mosaic: Affine {
            a: radius,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: radius,
            f: 0.0,
        },
    }
}

/// 組の点から求めた、a の画素から b の画素への射影変換（求まらなければ組のアフィン）。
fn pair_homography(link: &Link) -> Homography {
    estimate::homography(&link.points).unwrap_or_else(|| Homography::from_affine(&link.map))
}

/// 1 組の点の、貼り合わせの座標でのずれ。写せない点は `None`。
fn residual(wa: &Warp, wb: &Warp, [p, q]: &Pair) -> Option<[f64; 2]> {
    let (u, v) = (wa.apply(*p)?, wb.apply(*q)?);
    Some([u[0] - v[0], u[1] - v[1]])
}

fn mean_error(model: &Model, params: &[f64], links: &[&Link]) -> f64 {
    let (mut sum, mut count) = (0.0, 0usize);
    for l in links {
        let (wa, wb) = (model.warp(l.a, params), model.warp(l.b, params));
        for p in &l.points {
            if let Some(r) = residual(&wa, &wb, p) {
                sum += r[0].hypot(r[1]);
                count += 1;
            }
        }
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

fn huber(e: f64) -> f64 {
    if e <= HUBER {
        0.5 * e * e
    } else {
        HUBER * (e - 0.5 * HUBER)
    }
}

fn cost(model: &Model, params: &[f64], links: &[&Link]) -> f64 {
    links
        .iter()
        .map(|l| {
            let (wa, wb) = (model.warp(l.a, params), model.warp(l.b, params));
            l.points
                .iter()
                .map(|p| residual(&wa, &wb, p).map_or(HUBER * 50.0, |r| huber(r[0].hypot(r[1]))))
                .sum::<f64>()
        })
        .sum()
}

fn optimise(model: &Model, params: &mut [f64], free: &[bool], links: &[&Link]) {
    // 動かす値の番号（動かさない画像の値は数えない）。共通の値はいつも動かす。
    let mut index: Vec<Option<usize>> = vec![None; params.len()];
    let mut count = 0;
    for (i, &f) in free.iter().enumerate() {
        if f {
            for k in 0..model.per {
                index[i * model.per + k] = Some(count);
                count += 1;
            }
        }
    }
    for slot in index.iter_mut().skip(model.globals_at) {
        *slot = Some(count);
        count += 1;
    }
    if count == 0 || links.is_empty() {
        return;
    }
    let mut lambda = 1e-3;
    let mut current = cost(model, params, links);
    for _ in 0..ITERATIONS {
        let (h, g) = normal_equations(model, params, &index, count, links);
        let mut improved = false;
        for _ in 0..8 {
            let mut damped = h.clone();
            for (i, row) in damped.iter_mut().enumerate() {
                row[i] += lambda * row[i].max(1e-12);
            }
            let Some(step) = solve(damped, g.iter().map(|v| -v).collect()) else {
                lambda *= 10.0;
                continue;
            };
            let mut trial = params.to_vec();
            for (k, slot) in index.iter().enumerate() {
                if let Some(j) = slot {
                    trial[k] += step[*j];
                }
            }
            let next = cost(model, &trial, links);
            if next < current {
                let gain = (current - next) / current.max(1e-300);
                params.copy_from_slice(&trial);
                current = next;
                lambda = (lambda / 3.0).max(1e-12);
                improved = gain > 1e-9;
                break;
            }
            lambda *= 4.0;
        }
        if !improved {
            break;
        }
    }
}

/// 1 つの値を少し上げた・下げたときの、組の両端の置き方と、動かした量。
type Nudged = ((Warp, Warp), (Warp, Warp), f64);

/// 重みを付けた正規方程式（JᵀWJ と JᵀWr）。微分は、組の両端の画像の値と共通の値についてだけ、
/// 中心差分で求める（ほかの値には触れないので 0）。
fn normal_equations(
    model: &Model,
    params: &[f64],
    index: &[Option<usize>],
    count: usize,
    links: &[&Link],
) -> (Vec<Vec<f64>>, Vec<f64>) {
    let mut h = vec![vec![0.0f64; count]; count];
    let mut g = vec![0.0f64; count];
    for l in links {
        let local: Vec<usize> = (l.a * model.per..(l.a + 1) * model.per)
            .chain(l.b * model.per..(l.b + 1) * model.per)
            .chain(model.globals_at..params.len())
            .filter(|&k| index[k].is_some())
            .collect();
        if local.is_empty() {
            continue;
        }
        let warps = |p: &[f64]| (model.warp(l.a, p), model.warp(l.b, p));
        let (wa, wb) = warps(params);
        // 値ごとに、少し動かした置き方。
        let moved: Vec<Nudged> = local
            .iter()
            .map(|&k| {
                let d = model.steps[k];
                let mut up = params.to_vec();
                up[k] += d;
                let mut down = params.to_vec();
                down[k] -= d;
                (warps(&up), warps(&down), d)
            })
            .collect();
        for pair in &l.points {
            let Some(r) = residual(&wa, &wb, pair) else {
                continue;
            };
            let e = r[0].hypot(r[1]);
            let w = if e <= HUBER { 1.0 } else { HUBER / e };
            let mut jac: Vec<[f64; 2]> = Vec::with_capacity(local.len());
            let mut ok = true;
            for ((ua, ub), (da, db), d) in &moved {
                match (residual(ua, ub, pair), residual(da, db, pair)) {
                    (Some(u), Some(v)) => {
                        jac.push([(u[0] - v[0]) / (2.0 * d), (u[1] - v[1]) / (2.0 * d)]);
                    }
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            for (x, &kx) in local.iter().enumerate() {
                let jx = index[kx].expect("free");
                g[jx] += w * (jac[x][0] * r[0] + jac[x][1] * r[1]);
                for (y, &ky) in local.iter().enumerate() {
                    let jy = index[ky].expect("free");
                    h[jx][jy] += w * (jac[x][0] * jac[y][0] + jac[x][1] * jac[y][1]);
                }
            }
        }
    }
    (h, g)
}

/// 組の射影変換から推定した焦点距離（画素、中央値）。その場で回して撮った組の射影変換は
/// `K R K⁻¹` の形になることから求める（Szeliski と Shum の式）。求まる組がなければ `None`。
pub fn estimate_focal(sizes: &[[usize; 2]], links: &[Link]) -> Option<f64> {
    let mut found: Vec<f64> = Vec::new();
    for l in links {
        if l.points.len() < 8 {
            continue;
        }
        let (ca, cb) = (centre_of(sizes[l.a]), centre_of(sizes[l.b]));
        let centred: Vec<Pair> = l
            .points
            .iter()
            .map(|[p, q]| [[p[0] - ca[0], p[1] - ca[1]], [q[0] - cb[0], q[1] - cb[1]]])
            .collect();
        let Some(h) = estimate::homography(&centred) else {
            continue;
        };
        if let Some(f) = focal_of(&h) {
            found.push(f);
        }
    }
    if found.is_empty() {
        return None;
    }
    found.sort_by(f64::total_cmp);
    Some(found[found.len() / 2])
}

fn centre_of(s: [usize; 2]) -> [f64; 2] {
    [(s[0] as f64 - 1.0) / 2.0, (s[1] as f64 - 1.0) / 2.0]
}

fn focal_of(h: &Homography) -> Option<f64> {
    let m = h.normalized().0;
    let h = [
        m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2], m[2][0], m[2][1], m[2][2],
    ];
    let pick = |d1: f64, d2: f64, v1: f64, v2: f64| -> Option<f64> {
        let (v1, v2) = if v1 < v2 { (v2, v1) } else { (v1, v2) };
        if v1 > 0.0 && v2 > 0.0 {
            Some(if d1.abs() > d2.abs() { v1 } else { v2 }.sqrt())
        } else if v1 > 0.0 {
            Some(v1.sqrt())
        } else {
            None
        }
    };
    let d1 = h[6] * h[7];
    let d2 = (h[7] - h[6]) * (h[7] + h[6]);
    let f1 = pick(
        d1,
        d2,
        -(h[0] * h[1] + h[3] * h[4]) / d1,
        (h[0] * h[0] + h[3] * h[3] - h[1] * h[1] - h[4] * h[4]) / d2,
    );
    let d1 = h[0] * h[3] + h[1] * h[4];
    let d2 = h[0] * h[0] + h[1] * h[1] - h[3] * h[3] - h[4] * h[4];
    let f0 = pick(d1, d2, -h[2] * h[5] / d1, (h[5] * h[5] - h[2] * h[2]) / d2);
    let f = match (f0, f1) {
        (Some(a), Some(b)) => (a * b).sqrt(),
        _ => return None,
    };
    f.is_finite().then_some(f)
}

/// 基準から組をたどって、画像ごとのカメラの向き（世界 → カメラ）を求める。
fn rotations(
    tree: &[Option<(Affine, Option<usize>)>],
    links: &[Link],
    anchor: usize,
    centres: &[[f64; 2]],
    focal: f64,
) -> Option<Vec<Mat3>> {
    let n = tree.len();
    let identity = crate::linalg::IDENTITY3;
    let mut out: Vec<Option<Mat3>> = vec![None; n];
    out[anchor] = Some(identity);
    let ray = |c: [f64; 2], p: [f64; 2]| {
        normalized3([(p[0] - c[0]) / focal, (p[1] - c[1]) / focal, 1.0]).unwrap_or([0.0, 0.0, 1.0])
    };
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..n {
            let (None, Some((_, Some(l)))) = (out[i], tree[i]) else {
                continue;
            };
            let link = &links[l];
            let pairs: Vec<[[f64; 3]; 2]> = link
                .points
                .iter()
                .map(|[p, q]| [ray(centres[link.a], *p), ray(centres[link.b], *q)])
                .collect();
            // a のカメラの向き → b のカメラの向き。
            let Some(ab) = turn_between(&pairs) else {
                continue;
            };
            let next = if link.b == i {
                out[link.a].map(|ra| mul3(&ab, &ra))
            } else {
                out[link.b].map(|rb| mul3(&transpose3(&ab), &rb))
            };
            if next.is_some() {
                out[i] = next;
                changed = true;
            }
        }
    }
    Some(out.into_iter().map(|r| r.unwrap_or(identity)).collect())
}

/// `v ≈ R u` となる回転 R（Horn の四元数の方法）。
pub fn turn_between(pairs: &[[[f64; 3]; 2]]) -> Option<Mat3> {
    if pairs.len() < 2 {
        return None;
    }
    let mut s = [[0.0f64; 3]; 3];
    for [u, v] in pairs {
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += u[i] * v[j];
            }
        }
    }
    let [[sxx, sxy, sxz], [syx, syy, syz], [szx, szy, szz]] = s;
    let n = vec![
        vec![sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        vec![syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        vec![szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        vec![sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    let (_, q) = symmetric_eigen(n).pop()?;
    let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
    Some([
        [
            w * w + x * x - y * y - z * z,
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (y * x + w * z),
            w * w - x * x + y * y - z * z,
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (z * x - w * y),
            2.0 * (z * y + w * x),
            w * w - x * x - y * y + z * z,
        ],
    ])
}

/// 世界の向きを、カメラの左右の向きがそろって水平になるように回す（基準の 1 枚が傾いて
/// いても、パノラマが波打たないように）。前は、カメラの前の向きの平均にする。
fn straightened(rotations: Vec<Mat3>) -> Vec<Mat3> {
    // カメラの右の向き（世界の座標）は回転の 1 行目、下は 2 行目、前は 3 行目。
    let mut m = vec![vec![0.0f64; 3]; 3];
    let (mut down, mut ahead) = ([0.0f64; 3], [0.0f64; 3]);
    for r in &rotations {
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += r[0][i] * r[0][j];
            }
            down[i] += r[1][i];
            ahead[i] += r[2][i];
        }
    }
    let eigen = symmetric_eigen(m);
    // 右の向きがほぼ 1 つに決まっている（縦にだけ振った）なら、上下が決まらないのでそのまま。
    if eigen[1].0 < 1e-3 * rotations.len() as f64 {
        return rotations;
    }
    let mut up = [eigen[0].1[0], eigen[0].1[1], eigen[0].1[2]];
    if dot3(up, down) < 0.0 {
        up = up.map(|v| -v);
    }
    let along = dot3(ahead, up);
    let Some(z) = normalized3([0, 1, 2].map(|i| ahead[i] - along * up[i])) else {
        return rotations;
    };
    let x = cross3(up, z);
    let g = [x, up, z];
    let back = transpose3(&g);
    rotations.iter().map(|r| mul3(r, &back)).collect()
}

/// 円筒にしたとき、全体が横に何度に広がるか（画像の縁の角度の幅）。求まらなければ `None`。
pub fn cylinder_span(
    sizes: &[[usize; 2]],
    links: &[Link],
    tree: &[Option<(Affine, Option<usize>)>],
    focal: f64,
) -> Option<f64> {
    let anchor = tree.iter().position(|p| matches!(p, Some((_, None))))?;
    let model = Model::cylinder(sizes, tree, links, anchor, focal)?;
    let Kind::Cylinder { radius, .. } = model.kind else {
        return None;
    };
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for i in 0..sizes.len() {
        if tree[i].is_none() {
            continue;
        }
        for q in model.warp(i, &model.params).outline(sizes[i], 8) {
            let theta = q[0] / radius;
            lo = lo.min(theta);
            hi = hi.max(theta);
        }
    }
    (hi > lo).then(|| (hi - lo).to_degrees())
}

/// アフィンでは直せない遠近のゆがみが、組の点にどれだけあるか（画素）。組ごとに、点を囲む
/// 四角の 4 隅で、射影変換とアフィンの写した所の違いのいちばん大きいものを測り、中央値を取る。
/// 射影変換にしても点のずれがあまり減らない組は、ゆがみ 0 と数える（特徴点の位置の誤差だけで、
/// 平らに撮った画像を射影にしてしまわないため）。測れる組がなければ `None`。
pub fn perspective(points: &HashMap<(usize, usize), Vec<Pair>>) -> Option<f64> {
    let rms = |pairs: &[Pair], map: &dyn Fn([f64; 2]) -> Option<[f64; 2]>| -> Option<f64> {
        let mut sum = 0.0;
        for [p, q] in pairs {
            let m = map(*p)?;
            sum += (m[0] - q[0]).powi(2) + (m[1] - q[1]).powi(2);
        }
        Some((sum / pairs.len() as f64).sqrt())
    };
    let mut found: Vec<f64> = points
        .values()
        .filter(|p| p.len() >= 12)
        .filter_map(|pairs| {
            let h = estimate::homography(pairs)?;
            let a = estimate::affine(pairs)?;
            let by_h = rms(pairs, &|p| h.apply(p))?;
            let by_a = rms(pairs, &|p| Some(a.apply(p)))?;
            if by_h > 0.8 * by_a {
                return Some(0.0);
            }
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for [p, _] in pairs {
                (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
            }
            [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
                .iter()
                .map(|c| {
                    let (u, v) = (h.apply(*c)?, a.apply(*c));
                    Some((u[0] - v[0]).hypot(u[1] - v[1]))
                })
                .try_fold(0.0f64, |m, d| d.map(|d| m.max(d)))
        })
        .collect();
    if found.is_empty() {
        return None;
    }
    found.sort_by(f64::total_cmp);
    Some(found[found.len() / 2])
}

#[cfg(test)]
mod tests;
