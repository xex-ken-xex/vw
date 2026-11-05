//! 重なりのある何枚かの画像を、1 枚に貼り合わせる（天体のモザイク、風景のパノラマ、
//! 大きな絵を分けて撮ったもの）。
//!
//! 2 枚ずつ重なりを探し（[`link_all`]）、確かな組をたどって全部を 1 つの座標に置き（[`place`]）、
//! 置いた所に画素を重ねる（[`Canvas`]）。座標は、置く基準にした 1 枚の元の画素。
//!
//! 2 枚ずつの組はアフィン（ずらす・回す・大きさを変える）で見つけ、対応する点を残す。
//! 全体の置き方は、その点を使って [`adjust`] でアフィン・射影・円筒のどれかに直す。

use rayon::prelude::*;
use rustfft::num_complex::Complex32;

use crate::affine::Affine;
use crate::coarse::Fft2;
use crate::estimate::Pair;
use crate::linalg::solve;
use crate::plane::Plane;
use crate::refine;
pub use crate::warp::corners;
use crate::warp::{Warp, local_affine};

pub mod adjust;
pub mod more;

/// 位置合わせに使う 1 枚。
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// 縮めた明るさ。
    pub plane: Plane,
    /// 元の画像の大きさ（画素）。
    pub full: [usize; 2],
}

impl Piece {
    /// 縮めた画像の 1 画素が、元の画像の何画素ぶんか（の逆数。1 以下）。
    pub fn scale(&self) -> f64 {
        self.plane.width as f64 / self.full[0] as f64
    }
}

/// 重なりの見つかった 2 枚。
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    /// `a` の元の画素の座標から `b` の元の画素の座標への写し方。
    pub map: Affine,
    /// 重なった所の相関（−1〜1）。
    pub correlation: f64,
    /// 重なった所の、`a` の面積に対する割合。
    pub overlap: f64,
    pub kind: LinkKind,
    /// 対応する点（a の元の画素, b の元の画素）。全体を直す（[`adjust`]）のに使う。
    /// 重なりの絵や WCS で合わせた組は、写し方で重なりの中の格子の点を写したもの。
    pub points: Vec<Pair>,
}

/// どうやって組を見つけたか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// 重なった所の絵の相関で。
    Overlap,
    /// 特徴点（角）の対応で。
    Features,
    /// 星の並びで。
    Stars,
    /// ヘッダの WCS で。
    Wcs,
    /// 利用者が大まかに置いた所から。
    ByHand,
}

impl Link {
    /// 写し方 `map` で決まる組（対応する点は、重なりの中の格子の点を写したもの）。
    pub fn by_map(
        a: (usize, [usize; 2]),
        b: (usize, [usize; 2]),
        map: Affine,
        correlation: f64,
        overlap: f64,
        kind: LinkKind,
    ) -> Link {
        Link {
            a: a.0,
            b: b.0,
            map,
            correlation,
            overlap,
            kind,
            points: grid_pairs(a.1, b.1, &map),
        }
    }
}

/// `a` の中で `b` にも写る、格子の点と写した先（全体を直すときの 1 組の重みをそろえるため、
/// どの組も同じくらいの数にする）。
pub fn grid_pairs(sa: [usize; 2], sb: [usize; 2], map: &Affine) -> Vec<Pair> {
    const STEPS: usize = 6;
    let mut out = Vec::new();
    // 重なりが狭くても点が残るように、細かい格子から重なりの中の点を間引く。
    let fine = STEPS * 4;
    for j in 0..=fine {
        for i in 0..=fine {
            let p = [
                i as f64 / fine as f64 * (sa[0] - 1) as f64,
                j as f64 / fine as f64 * (sa[1] - 1) as f64,
            ];
            let q = map.apply(p);
            if q[0] >= 0.0
                && q[1] >= 0.0
                && q[0] <= (sb[0] - 1) as f64
                && q[1] <= (sb[1] - 1) as f64
            {
                out.push([p, q]);
            }
        }
    }
    let keep = (STEPS + 1) * (STEPS + 1);
    if out.len() > keep {
        let step = out.len() as f64 / keep as f64;
        out = (0..keep).map(|k| out[(k as f64 * step) as usize]).collect();
    }
    out
}

/// 大まかに探すときの、いちばん大きな画像の長辺。
const COARSE_SIDE: usize = 256;
/// 確かな重なりとみなす相関の下限。
pub const MIN_CORRELATION: f64 = 0.6;
/// 重なりとみなす面積の割合の下限。これより狭いと、たまたま似た所でも合ってしまう。
pub const MIN_OVERLAP: f64 = 0.05;
/// 細かく直す候補にする、大まかな相関の下限。
const MIN_COARSE: f64 = 0.3;
/// 1 組ごとに、細かく直してみる大まかな候補の数。
const CANDIDATES: usize = 3;

/// 探し方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Search {
    /// 90 度ずつ回したものも探す（望遠鏡の子午線越えで 180 度回った天体の画像など）。
    /// 写真では、回して偶然似た所を拾うおそれのほうが大きいので探さない。
    pub quarter_turns: bool,
}

/// 大まかに探すための、縮めた画像の FFT（回したもの、回さないときは 1 つ）。
struct Spectra {
    /// `turns[k]` は時計回りに `k` × 90 度回したもの。
    turns: Vec<Masked>,
}

/// 縮めた画像を `n` × `n` の左上に置いた値・値の 2 乗・写っている所の印の FFT。
struct Masked {
    value: Vec<Complex32>,
    square: Vec<Complex32>,
    mask: Vec<Complex32>,
    size: [usize; 2],
}

/// すべての組の重なりを探す。`progress` は 1 枚を下ごしらえするか、1 組を調べるたびに呼ぶ。
pub fn link_all(pieces: &[Piece], search: Search, progress: &(dyn Fn() + Sync)) -> Vec<Link> {
    let Some(side) = pieces.iter().map(|p| p.full[0].max(p.full[1])).max() else {
        return Vec::new();
    };
    let factor = COARSE_SIDE as f64 / side as f64;
    let n = (2 * COARSE_SIDE).next_power_of_two();
    let fft = Fft2::new(n, n);
    let spectra: Vec<Spectra> = pieces
        .par_iter()
        .map(|p| {
            let s = spectra(p, factor, n, &fft, search);
            progress();
            s
        })
        .collect();
    let pairs: Vec<(usize, usize)> = (0..pieces.len())
        .flat_map(|a| (a + 1..pieces.len()).map(move |b| (a, b)))
        .collect();
    let mut links: Vec<Link> = pairs
        .par_iter()
        .filter_map(|&(a, b)| {
            let found = link_pair(pieces, &spectra, factor, &fft, a, b);
            progress();
            found
        })
        .collect();
    links.sort_by_key(|l| (l.a, l.b));
    links
}

/// [`link_all`] が呼ぶ `progress` の回数。
pub fn link_steps(pieces: usize) -> usize {
    pieces + pieces * pieces.saturating_sub(1) / 2
}

fn spectra(p: &Piece, factor: f64, n: usize, fft: &Fft2, search: Search) -> Spectra {
    let w = ((p.full[0] as f64 * factor).round() as usize).clamp(1, n / 2);
    let h = ((p.full[1] as f64 * factor).round() as usize).clamp(1, n / 2);
    let small = p.plane.resized(w, h);
    let mean = small.data.iter().sum::<f32>() / small.data.len().max(1) as f32;
    let turns = if search.quarter_turns { 0..4 } else { 0..1 };
    Spectra {
        turns: turns
            .map(|k| {
                let t = quarter_turn(&small, k);
                let place = |f: &dyn Fn(f32) -> f32| {
                    let mut out = vec![0.0f32; n * n];
                    for y in 0..t.height {
                        for x in 0..t.width {
                            out[y * n + x] = f(t.at(x, y) - mean);
                        }
                    }
                    fft.forward(&out)
                };
                Masked {
                    value: place(&|v| v),
                    square: place(&|v| v * v),
                    mask: place(&|_| 1.0),
                    size: [t.width, t.height],
                }
            })
            .collect(),
    }
}

fn link_pair(
    pieces: &[Piece],
    spectra: &[Spectra],
    factor: f64,
    fft: &Fft2,
    a: usize,
    b: usize,
) -> Option<Link> {
    let fa = &spectra[a].turns[0];
    // 重なりがこれより狭いずれは数えない（縮めた画像の画素の数）。
    let smaller = |s: [usize; 2]| (s[0] * s[1]).min(fa.size[0] * fa.size[1]) as f64;
    let mut candidates: Vec<(f64, Affine)> = spectra[b]
        .turns
        .iter()
        .enumerate()
        .flat_map(|(k, fb)| {
            let least = smaller(fb.size) * MIN_OVERLAP;
            peaks(&masked_correlation(fft, fa, fb), fft.width, least)
                .into_iter()
                .map(move |(d, score)| (score, k, d, fb.size))
        })
        .map(|(score, k, d, size)| {
            // 縮めた a の点 p は、縮めて回した b の p + d に写る。
            let shift = Affine {
                c: d[0],
                f: d[1],
                ..Affine::IDENTITY
            };
            let unturn = turn_map(k, size).inverse().unwrap_or(Affine::IDENTITY);
            let small = shift.then(&unturn);
            (score, small.between_scales(1.0 / factor, 1.0 / factor))
        })
        .collect();
    candidates.sort_by(|x, y| y.0.total_cmp(&x.0));
    candidates
        .into_iter()
        .take(CANDIDATES)
        .filter_map(|(_, start)| link_from(pieces, a, b, start))
        .max_by(|x, y| x.correlation.total_cmp(&y.correlation))
}

/// `start`（`a` の元の座標 → `b` の元の座標）から細かく直す。確かな重なりにならなければ `None`。
/// 手で大まかに置いた画像を、そこから合わせるのにも使う。
pub fn link_from(pieces: &[Piece], a: usize, b: usize, start: Affine) -> Option<Link> {
    let (pa, pb) = (&pieces[a], &pieces[b]);
    let (sa, sb) = (pa.scale(), pb.scale());
    let fitted = refine::refine(&pa.plane, &pb.plane, start.between_scales(sa, sb));
    let map = fitted.transform.between_scales(1.0 / sa, 1.0 / sb);
    (fitted.correlation >= MIN_CORRELATION
        && fitted.overlap >= MIN_OVERLAP
        && plausible(&start, &map, pa.full, pb.full))
    .then(|| {
        Link::by_map(
            (a, pa.full),
            (b, pb.full),
            map,
            fitted.correlation,
            fitted.overlap,
            LinkKind::Overlap,
        )
    })
}

/// 細かく直した写し方が、同じ撮り方で分けて撮った画像どうしとしてありうるか。狭い重なりから
/// 直すと、平らな所に引きずられて大きく回ったり縦横に伸びたりした写し方に落ちることがあるので、
/// 大まかに見つけた向きから大きく離れたもの、縦横の倍率やせん断の大きいものは捨てる。
fn plausible(start: &Affine, map: &Affine, a: [usize; 2], b: [usize; 2]) -> bool {
    let size = |s: [usize; 2]| [s[0] as f64, s[1] as f64];
    let (from, to) = (start.parts(size(a), size(b)), map.parts(size(a), size(b)));
    let turned = ((to.rotation - from.rotation + 540.0).rem_euclid(360.0) - 180.0).abs();
    turned <= 10.0
        && (0.8..1.25).contains(&(to.scale / from.scale))
        && (to.aspect - 1.0).abs() <= 0.05
        && to.shear.abs() <= 3.0
}

/// ずれ d ごとの、重なった所だけで測った正規化相互相関と、重なった画素の数（Padfield の方法）。
///
/// 位相相関（比べる画面の大まかな合わせ方）を使わないのは、重なりが狭いと、重ならない所の模様が
/// 作る偶然の山に本当の山が埋もれるため。重なった所だけで平均と分散を取り直せば、
/// 重なりの広さに左右されない。
fn masked_correlation(fft: &Fft2, a: &Masked, b: &Masked) -> Vec<(f32, f32)> {
    // (x ⋆ y)(d) = Σ x(p) y(p + d) = F⁻¹(conj(X) · Y)。
    let cross = |x: &[Complex32], y: &[Complex32]| -> Vec<f32> {
        let product: Vec<Complex32> = x.iter().zip(y).map(|(p, q)| p.conj() * q).collect();
        let scale = (fft.width * fft.height) as f32;
        fft.inverse(&product).iter().map(|c| c.re / scale).collect()
    };
    let count = cross(&a.mask, &b.mask);
    let sum_a = cross(&a.value, &b.mask);
    let sum_b = cross(&a.mask, &b.value);
    let square_a = cross(&a.square, &b.mask);
    let square_b = cross(&a.mask, &b.square);
    let product = cross(&a.value, &b.value);
    (0..count.len())
        .map(|i| {
            let n = count[i].round();
            if n < 16.0 {
                return (0.0, n);
            }
            let cov = product[i] - sum_a[i] * sum_b[i] / n;
            let va = square_a[i] - sum_a[i] * sum_a[i] / n;
            let vb = square_b[i] - sum_b[i] * sum_b[i] / n;
            let den = (va * vb).max(0.0).sqrt();
            (if den > 1e-6 { cov / den } else { 0.0 }, n)
        })
        .collect()
}

/// 相関の面の、高い山いくつか（ずれ, 相関）。いちばん高い山が偶然の模様の重なりで、
/// 本当の重なりが 2 番目にあることがあるので、いくつかを細かく直して比べる。
/// 重なった画素が `least` より少ないずれは見ない。
fn peaks(surface: &[(f32, f32)], n: usize, least: f64) -> Vec<([f64; 2], f64)> {
    let wrap = |p: usize| {
        if p > n / 2 {
            p as f64 - n as f64
        } else {
            p as f64
        }
    };
    let value = |x: usize, y: usize| {
        let (v, c) = surface[(y % n) * n + x % n];
        if f64::from(c) >= least { v } else { f32::MIN }
    };
    let mut order: Vec<usize> = (0..surface.len())
        .filter(|&i| f64::from(surface[i].1) >= least && f64::from(surface[i].0) >= MIN_COARSE)
        .collect();
    order.sort_unstable_by(|&x, &y| surface[y].0.total_cmp(&surface[x].0));
    let mut found: Vec<([f64; 2], f64)> = Vec::new();
    for i in order {
        if found.len() >= CANDIDATES {
            break;
        }
        let (px, py) = (i % n, i / n);
        let d = [wrap(px), wrap(py)];
        // 同じ山の裾は数えない。
        if found
            .iter()
            .any(|(p, _)| (p[0] - d[0]).hypot(p[1] - d[1]) < 4.0)
        {
            continue;
        }
        // 頂点の前後 3 点に放物線を当てて、画素より細かく求める。
        let fit = |l: f32, c: f32, r: f32| {
            let den = l - 2.0 * c + r;
            if l == f32::MIN || r == f32::MIN || den.abs() < 1e-12 {
                0.0
            } else {
                f64::from(0.5 * (l - r) / den).clamp(-0.5, 0.5)
            }
        };
        let c = surface[i].0;
        let fx = fit(value(px + n - 1, py), c, value(px + 1, py));
        let fy = fit(value(px, py + n - 1), c, value(px, py + 1));
        found.push(([d[0] + fx, d[1] + fy], f64::from(c)));
    }
    found
}

/// 時計回りに `k` × 90 度回した画像。
fn quarter_turn(p: &Plane, k: usize) -> Plane {
    let size = [p.width, p.height];
    let t = turn_map(k, size);
    let (w, h) = if k % 2 == 1 {
        (p.height, p.width)
    } else {
        (p.width, p.height)
    };
    let back = t.inverse().unwrap_or(Affine::IDENTITY);
    let data = (0..w * h)
        .map(|i| {
            let [x, y] = back.apply([(i % w) as f64, (i / w) as f64]);
            p.at(x.round() as usize, y.round() as usize)
        })
        .collect();
    Plane::new(w, h, data)
}

/// 大きさ `size` の画像を時計回りに `k` × 90 度回すときの、元の座標から回した座標への写し方。
fn turn_map(k: usize, [w, h]: [usize; 2]) -> Affine {
    let (w, h) = ((w - 1) as f64, (h - 1) as f64);
    match k % 4 {
        1 => Affine {
            a: 0.0,
            b: -1.0,
            c: h,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        },
        2 => Affine {
            a: -1.0,
            b: 0.0,
            c: w,
            d: 0.0,
            e: -1.0,
            f: h,
        },
        3 => Affine {
            a: 0.0,
            b: 1.0,
            c: 0.0,
            d: -1.0,
            e: 0.0,
            f: w,
        },
        _ => Affine::IDENTITY,
    }
}

/// 確かな組をたどって、全部の画像を 1 つの座標に置く。`placed[i]` は `i` の元の座標から
/// 貼り合わせの座標への写し方。どの組からもたどれない画像は `None`。
///
/// 基準は、確かな組の相関の和がいちばん大きい画像（真ん中にある画像ほど多くの画像と重なる）。
/// たどる組は、相関の高い順に木を作って選ぶ（全部の組の食い違いを最小二乗でならす方法は、
/// 1 組の取り違えが全体をゆがめるので、まずは確かな組だけをつなぐ）。
pub fn place(count: usize, links: &[Link]) -> Vec<Option<Affine>> {
    tree(count, links)
        .into_iter()
        .map(|p| p.map(|(t, _)| t))
        .collect()
}

/// [`tree`] の置き方と食い違う組を外す。繰り返しの多い絵（草地や空）では、重なっていない
/// 2 枚が相関で結ばれることがあり、そのまま全体の直しに入れると、正しい組まで引っ張られるため。
/// 置き方をたどった組は残す（どれが誤りかは、ほかの組との食い違いでしか分からないので）。
pub fn without_disagreeing(count: usize, links: &mut Vec<Link>) {
    let placed = tree(count, links);
    let in_tree: Vec<usize> = placed
        .iter()
        .flatten()
        .filter_map(|(_, via)| *via)
        .collect();
    let mut index = 0;
    links.retain(|l| {
        let i = index;
        index += 1;
        if in_tree.contains(&i) {
            return true;
        }
        let (Some((pa, _)), Some((pb, _))) = (placed[l.a], placed[l.b]) else {
            return true;
        };
        disagreement(l, &pa, &pb).is_none_or(|(off, extent)| off <= (0.25 * extent).max(30.0))
    });
}

/// 組の点を置き方で写したずれの中央値と、点の広がり（a の画素の対角）。点がなければ `None`。
fn disagreement(l: &Link, pa: &Affine, pb: &Affine) -> Option<(f64, f64)> {
    let mut offsets: Vec<f64> = l
        .points
        .iter()
        .map(|[p, q]| {
            let (x, y) = (pa.apply(*p), pb.apply(*q));
            (x[0] - y[0]).hypot(x[1] - y[1])
        })
        .collect();
    if offsets.is_empty() {
        return None;
    }
    offsets.sort_by(f64::total_cmp);
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for [p, _] in &l.points {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    Some((
        offsets[offsets.len() / 2],
        (hi[0] - lo[0]).hypot(hi[1] - lo[1]),
    ))
}

/// [`place`] と同じ置き方と、それぞれを置くのにたどった組（`links` の番号。基準は `None`）。
pub fn tree(count: usize, links: &[Link]) -> Vec<Option<(Affine, Option<usize>)>> {
    let mut placed: Vec<Option<(Affine, Option<usize>)>> = vec![None; count];
    if count == 0 {
        return placed;
    }
    let mut weight = vec![0.0f64; count];
    for l in links {
        weight[l.a] += l.correlation;
        weight[l.b] += l.correlation;
    }
    let anchor = first_clear_max((0..count).map(|i| (i, weight[i]))).unwrap_or(0);
    placed[anchor] = Some((Affine::IDENTITY, None));
    loop {
        // 置いた画像と置いていない画像をつなぐ組のうち、いちばん確かなもの（Prim の方法）。
        let next = first_clear_max(
            links
                .iter()
                .enumerate()
                .filter(|(_, l)| placed[l.a].is_some() != placed[l.b].is_some())
                .map(|(i, l)| (i, l.correlation)),
        );
        let Some(i) = next else {
            break;
        };
        let l = &links[i];
        let Some(to_b) = l.map.inverse() else {
            break;
        };
        match (placed[l.a], placed[l.b]) {
            (Some((pa, _)), None) => placed[l.b] = Some((to_b.then(&pa), Some(i))),
            (None, Some((pb, _))) => placed[l.a] = Some((l.map.then(&pb), Some(i))),
            _ => break,
        }
    }
    level(&mut placed);
    placed
}

/// 値がこれより近いものは同じとみなす。相関は画素の並べ方や足す順で末尾の桁が変わるので、
/// そのまま大小を比べると、ほぼ同じ候補のどれを選ぶかがわずかな計算の違いで入れ替わる。
const TIE: f64 = 1e-9;

/// いちばん大きい値の番号。`TIE` 未満の差しかないものは、先に出てきた（番号の小さい）方を選ぶ。
fn first_clear_max(values: impl Iterator<Item = (usize, f64)>) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, v) in values {
        if best.is_none_or(|(_, b)| v > b + TIE) {
            best = Some((i, v));
        }
    }
    best.map(|(i, _)| i)
}

/// 置いた画像の多くがまっすぐ・等倍に見えるように、全体を回して縮める。基準の 1 枚が
/// 傾いて撮れていると、そのままでは全体が傾くため（回転と倍率の中央値を打ち消す）。
fn level(placed: &mut [Option<(Affine, Option<usize>)>]) {
    let mut angles: Vec<f64> = placed
        .iter()
        .flatten()
        .map(|(p, _)| p.d.atan2(p.a))
        .collect();
    let mut scales: Vec<f64> = placed
        .iter()
        .flatten()
        .map(|(p, _)| (p.a * p.e - p.b * p.d).abs().sqrt())
        .collect();
    if angles.len() < 3 {
        return;
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let (angle, scale) = (median(&mut angles), median(&mut scales));
    // ほぼまっすぐなら触らない（基準の 1 枚を元の画素のまま残すため）。
    if scale <= 0.0 || (angle.abs().to_degrees() < 0.05 && (scale - 1.0).abs() < 1e-3) {
        return;
    }
    let (c, s) = (angle.cos() / scale, angle.sin() / scale);
    let undo = Affine {
        a: c,
        b: s,
        c: 0.0,
        d: -s,
        e: c,
        f: 0.0,
    };
    for (p, _) in placed.iter_mut().flatten() {
        *p = p.then(&undo);
    }
}

/// 縁を写して範囲を測るときの、辺ごとの点の数（射影や円筒では縁が曲がって写るため）。
const OUTLINE_STEPS: usize = 16;

/// 置いた画像の縁を含む範囲 `[左, 上, 右, 下]`（貼り合わせの座標）。
pub fn bounds(sizes: &[[usize; 2]], placed: &[Option<Warp>]) -> Option<[f64; 4]> {
    let mut out: Option<[f64; 4]> = None;
    for (size, p) in sizes.iter().zip(placed) {
        let Some(p) = p else {
            continue;
        };
        for [x, y] in p.outline(*size, OUTLINE_STEPS) {
            let b = out.get_or_insert([x, y, x, y]);
            *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
    }
    out
}

/// 2 枚の重なりの、小さいほうの画像の面積に対する割合（0〜1）。格子の点で数える。
pub fn shared(sa: [usize; 2], pa: &Warp, sb: [usize; 2], pb: &Warp) -> f64 {
    let (small, ps, big, pg) = if sa[0] * sa[1] <= sb[0] * sb[1] {
        (sa, pa, sb, pb)
    } else {
        (sb, pb, sa, pa)
    };
    let Some(to_big) = pg.back() else {
        return 0.0;
    };
    const STEPS: usize = 24;
    let mut inside = 0;
    for j in 0..STEPS {
        for i in 0..STEPS {
            let x = (i as f64 + 0.5) / STEPS as f64 * small[0] as f64 - 0.5;
            let y = (j as f64 + 0.5) / STEPS as f64 * small[1] as f64 - 0.5;
            let Some([u, v]) = ps.apply([x, y]).and_then(|m| to_big.apply(m)) else {
                continue;
            };
            if u >= -0.5 && v >= -0.5 && u <= big[0] as f64 - 0.5 && v <= big[1] as f64 - 0.5 {
                inside += 1;
            }
        }
    }
    f64::from(inside) / (STEPS * STEPS) as f64
}

/// 同じ所を写した画像のまとまり（小さいほうの面積の `share` 以上が重なる画像どうしをつないだもの）。
/// どのまとまりも 2 枚以上。並びは、まとまりの中も外も画像の番号の順。
pub fn same_area(sizes: &[[usize; 2]], placed: &[Option<Warp>], share: f64) -> Vec<Vec<usize>> {
    let n = sizes.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for a in 0..n {
        for b in a + 1..n {
            let (Some(pa), Some(pb)) = (&placed[a], &placed[b]) else {
                continue;
            };
            if shared(sizes[a], pa, sizes[b], pb) >= share {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in 0..n {
        let r = root(&mut parent, i);
        match groups.iter_mut().find(|g| g[0] == r) {
            Some(g) => g.push(i),
            None => groups.push(vec![i]),
        }
    }
    groups.retain(|g| g.len() > 1);
    groups
}

/// 1 か所のつなぎ目の診断。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seam {
    pub a: usize,
    pub b: usize,
    /// 小さいほうの画像の面積に対する重なりの割合。
    pub shared: f64,
    /// 置いたとおりに重ねたときの、重なった所の相関。
    pub correlation: f64,
    /// 2 枚を直接合わせた組の対応する点と、置いた所との食い違い（元の画素、大きいほうから 1 割の所。
    /// 特徴点の位置のわずかな誤差で、いちばん大きな 1 点が目立ちすぎないように）。
    /// 2 枚を直接合わせられなかったときは `None`。
    pub mismatch: Option<f64>,
    /// 重なった所の明るさの差（明るいほうに対する割合、0〜1）。明るさをそろえたときは、そろえたあとの差。
    pub brightness: f64,
    /// 重なった所の鮮鋭度の比（ぼけたほう / くっきりしたほう、0〜1）。
    pub sharpness: f64,
}

/// つなぎ目の見込み。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    Clean,
    Visible,
    Misaligned,
}

/// 相関がこれより低いと、合っていない。
const MISALIGNED_CORRELATION: f64 = 0.5;
/// 食い違いがこれより大きいと合っていない（星や細い線が二重に見える）。
const MISALIGNED_PIXELS: f64 = 3.0;
/// これより相関が低い・食い違いが大きい・明るさや鮮鋭度が違うと、つなぎ目が見えやすい。
const CLEAN_CORRELATION: f64 = 0.8;
const CLEAN_PIXELS: f64 = 1.0;
const CLEAN_BRIGHTNESS: f64 = 0.1;
const CLEAN_SHARPNESS: f64 = 0.7;

impl Seam {
    pub fn verdict(&self) -> Verdict {
        let off = self.mismatch.unwrap_or(0.0);
        if self.correlation < MISALIGNED_CORRELATION || off > MISALIGNED_PIXELS {
            Verdict::Misaligned
        } else if self.correlation < CLEAN_CORRELATION
            || off > CLEAN_PIXELS
            || self.brightness > CLEAN_BRIGHTNESS
            || self.sharpness < CLEAN_SHARPNESS
        {
            Verdict::Visible
        } else {
            Verdict::Clean
        }
    }
}

/// 重なりの面積の割合がこれより小さい組は、つなぎ目として調べない（角が触れているだけの組）。
const SEAM_SHARE: f64 = 0.02;

/// 使う画像（`used`）どうしの、重なったつなぎ目を調べる。
pub fn seams(
    pieces: &[Piece],
    placed: &[Option<Warp>],
    links: &[Link],
    used: &[bool],
    gains: &[f64],
) -> Vec<Seam> {
    let n = pieces.len();
    let pairs: Vec<(usize, usize)> = (0..n)
        .flat_map(|a| (a + 1..n).map(move |b| (a, b)))
        .filter(|&(a, b)| used[a] && used[b])
        .collect();
    pairs
        .par_iter()
        .filter_map(|&(a, b)| {
            let (pa, pb) = (placed[a]?, placed[b]?);
            let share = shared(pieces[a].full, &pa, pieces[b].full, &pb);
            if share < SEAM_SHARE {
                return None;
            }
            // a の元の座標 → b の元の座標（重なりの中でいちばん近いアフィン）。
            let implied = local_affine(&pa, pieces[a].full, &pb, pieces[b].full)?;
            let back = pb.back()?;
            let through = |p: [f64; 2]| pa.apply(p).and_then(|m| back.apply(m));
            let mismatch = direct_mismatch(links, [a, b], &through);
            let correlation = correlation_through(&pieces[a], &pieces[b], &through);
            Some(seam(
                pieces,
                [a, b],
                implied,
                correlation,
                mismatch,
                share,
                [gains[a], gains[b]],
            ))
        })
        .collect()
}

/// 2 枚を直接合わせた組（`links` のうち、`a` と `b` の組でいちばん確かなもの）の対応する点が、
/// 置いた所（`implied`: a の元の座標 → b の元の座標）でどれだけ食い違うか。
fn direct_mismatch(
    links: &[Link],
    [a, b]: [usize; 2],
    implied: &dyn Fn([f64; 2]) -> Option<[f64; 2]>,
) -> Option<f64> {
    let direct = links
        .iter()
        .filter(|l| (l.a, l.b) == (a, b) || (l.a, l.b) == (b, a))
        .filter(|l| l.kind != LinkKind::ByHand || l.correlation > 0.0)
        .max_by(|x, y| x.correlation.total_cmp(&y.correlation))?;
    let mut off: Vec<f64> = direct
        .points
        .iter()
        .filter_map(|&[p, q]| {
            let (p, q) = if direct.a == a { (p, q) } else { (q, p) };
            let u = implied(p)?;
            Some((u[0] - q[0]).hypot(u[1] - q[1]))
        })
        .collect();
    if off.is_empty() {
        return None;
    }
    off.sort_by(f64::total_cmp);
    Some(off[(off.len() * 9 / 10).min(off.len() - 1)])
}

/// 重なった所の相関。`through` は a の元の画素から b の元の画素へ（射影や円筒でも、置いたとおり
/// に写す。重なりの中でアフィンに近づけると、広い重なりの端で数画素ずれて相関が下がるため）。
fn correlation_through(
    pa: &Piece,
    pb: &Piece,
    through: &dyn Fn([f64; 2]) -> Option<[f64; 2]>,
) -> f64 {
    let (sa, sb) = (pa.scale(), pb.scale());
    let (a, b) = (&pa.plane, &pb.plane);
    const SAMPLES: usize = 40_000;
    let stride = ((a.width * a.height) as f64 / SAMPLES as f64)
        .sqrt()
        .ceil()
        .max(1.0) as usize;
    let (mut n, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0);
    for y in (0..a.height).step_by(stride) {
        for x in (0..a.width).step_by(stride) {
            let full = [(x as f64 + 0.5) / sa - 0.5, (y as f64 + 0.5) / sa - 0.5];
            let Some(q) = through(full) else {
                continue;
            };
            let Some(vb) = b.sample((q[0] + 0.5) * sb - 0.5, (q[1] + 0.5) * sb - 0.5) else {
                continue;
            };
            let (va, vb) = (f64::from(a.at(x, y)), f64::from(vb));
            n += 1.0;
            sx += va;
            sy += vb;
            sxx += va * va;
            syy += vb * vb;
            sxy += va * vb;
        }
    }
    if n < 16.0 {
        return 0.0;
    }
    let cov = sxy - sx * sy / n;
    let den = ((sxx - sx * sx / n) * (syy - sy * sy / n)).max(0.0).sqrt();
    if den > 1e-9 { cov / den } else { 0.0 }
}

#[allow(clippy::too_many_arguments)]
fn seam(
    pieces: &[Piece],
    [a, b]: [usize; 2],
    implied: Affine,
    correlation: f64,
    mismatch: Option<f64>,
    share: f64,
    gains: [f64; 2],
) -> Seam {
    let (pa, pb) = (&pieces[a], &pieces[b]);
    let small = implied.between_scales(pa.scale(), pb.scale());
    let (brightness, sharpness) = looks(pa, pb, &small, gains);
    Seam {
        a,
        b,
        shared: share,
        correlation,
        mismatch,
        brightness,
        sharpness,
    }
}

/// 重なった所の明るさの差と、鮮鋭度の比。`small` は縮めた画像どうしの写し方（a → b）。
/// 明るさは、それぞれ `gains` を掛けたあとの差（そろえたあとに残る段差）。
fn looks(pa: &Piece, pb: &Piece, small: &Affine, gains: [f64; 2]) -> (f64, f64) {
    let (a, b) = (&pa.plane, &pb.plane);
    let (mut sum_a, mut sum_b, mut count) = (0.0, 0.0, 0usize);
    for y in 0..a.height {
        for x in 0..a.width {
            let [u, v] = small.apply([x as f64, y as f64]);
            if let Some(vb) = b.sample(u, v) {
                sum_a += f64::from(a.at(x, y));
                sum_b += f64::from(vb);
                count += 1;
            }
        }
    }
    if count == 0 {
        return (0.0, 1.0);
    }
    let (ma, mb) = (sum_a / count as f64, sum_b / count as f64);
    let (ga, gb) = (ma * gains[0], mb * gains[1]);
    let brightness = if ga.max(gb) > 0.0 {
        (ga - gb).abs() / ga.max(gb)
    } else {
        0.0
    };
    // 勾配はそれぞれ自分の画素の上で測る。相手の格子へ写して測ると、1 画素より細かいずれの
    // 補間でぼけて、同じくっきりさの画像でも鋭さが半分ほどに見えてしまうため。
    let sharpness = match (small.inverse(), small.parts([1.0, 1.0], [1.0, 1.0]).scale) {
        (Some(back), k) if k > 0.0 => {
            // 明るさで割る。露出が暗いだけの写真は勾配も暗さの 2 乗だけ小さく、ぼけて見えてしまうため。
            let ga = gradient_energy(a, b, small) / (ma * ma).max(1e-12);
            // 縮めた倍率が違えば、同じ広さの勾配は倍率の 2 乗だけ違って見えるので、そろえる。
            let gb = gradient_energy(b, a, &back) * k * k / (mb * mb).max(1e-12);
            if ga.max(gb) > 0.0 {
                ga.min(gb) / ga.max(gb)
            } else {
                1.0
            }
        }
        _ => 1.0,
    };
    (brightness, sharpness)
}

/// `own` の画素のうち、`to_other` で写して `other` にも写る所の、勾配の 2 乗の平均。
fn gradient_energy(own: &Plane, other: &Plane, to_other: &Affine) -> f64 {
    let (mut sum, mut count) = (0.0f64, 0usize);
    for y in 0..own.height.saturating_sub(1) {
        for x in 0..own.width.saturating_sub(1) {
            let [u, v] = to_other.apply([x as f64, y as f64]);
            if other.sample(u, v).is_none() {
                continue;
            }
            let c = own.at(x, y);
            sum += f64::from((own.at(x + 1, y) - c).powi(2) + (own.at(x, y + 1) - c).powi(2));
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// 重なった所の明るさがそろうように、画像ごとに掛ける比（使わない画像と置けなかった画像は 1）。
///
/// つなぎ目ごとの明るさの差の 2 乗と、比の 1 からの離れの 2 乗の和を最小にする
/// （比をすべて 0 にすれば差は消えるので、1 から離れることにも重みを付ける）。
pub fn gains(pieces: &[Piece], placed: &[Option<Warp>], used: &[bool]) -> Vec<f64> {
    let n = pieces.len();
    // 差の重みに対する、1 から離れることの重み（明るさは 0〜255 の目盛り）。
    const NOISE: f64 = 10.0;
    const SPREAD: f64 = 0.5;
    let mut h = vec![vec![0.0f64; n]; n];
    let mut g = vec![0.0f64; n];
    let mut any = false;
    for a in 0..n {
        for b in a + 1..n {
            if !(used[a] && used[b]) {
                continue;
            }
            let (Some(pa), Some(pb)) = (placed[a], placed[b]) else {
                continue;
            };
            let Some(implied) = local_affine(&pa, pieces[a].full, &pb, pieces[b].full) else {
                continue;
            };
            let Some((ma, mb, count)) = means(&pieces[a], &pieces[b], &implied) else {
                continue;
            };
            any = true;
            let w = count as f64;
            let (kn, kg) = (1.0 / (NOISE * NOISE), 1.0 / (SPREAD * SPREAD));
            h[a][a] += w * (ma * ma * kn + kg);
            h[b][b] += w * (mb * mb * kn + kg);
            h[a][b] -= w * ma * mb * kn;
            h[b][a] -= w * ma * mb * kn;
            g[a] += w * kg;
            g[b] += w * kg;
        }
    }
    if !any {
        return vec![1.0; n];
    }
    // どのつなぎ目にも入らない画像は、比を 1 のままにする。
    for i in 0..n {
        if h[i][i] <= 0.0 {
            h[i][i] = 1.0;
            g[i] = 1.0;
        }
    }
    solve(h, g).unwrap_or_else(|| vec![1.0; n])
}

/// 重なった所の、a と b の明るさの平均と、数えた点の数。
fn means(pa: &Piece, pb: &Piece, implied: &Affine) -> Option<(f64, f64, usize)> {
    let small = implied.between_scales(pa.scale(), pb.scale());
    let (a, b) = (&pa.plane, &pb.plane);
    let (mut sa, mut sb, mut n) = (0.0, 0.0, 0usize);
    for y in 0..a.height {
        for x in 0..a.width {
            let [u, v] = small.apply([x as f64, y as f64]);
            if let Some(vb) = b.sample(u, v) {
                sa += f64::from(a.at(x, y));
                sb += f64::from(vb);
                n += 1;
            }
        }
    }
    (n >= 16).then(|| (sa / n as f64, sb / n as f64, n))
}

/// 重なった所の混ぜ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blend {
    /// 縁に近いほど薄くして重ねる。つなぎ目が目立たない。
    #[default]
    Feather,
    /// 後の画像を上に置くだけ。値はどれか 1 枚の値のまま。
    Top,
}

/// 貼り合わせた画像の格子。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub width: usize,
    pub height: usize,
    /// 出す画像の画素の座標から、貼り合わせの座標への写し方。
    pub to_mosaic: Affine,
}

impl Grid {
    /// `bounds` を、画素の数が `max_pixels` 以下になるように縮めて（広げはしない）覆う格子。
    pub fn covering(bounds: [f64; 4], max_pixels: usize) -> Option<Self> {
        let (w, h) = (bounds[2] - bounds[0], bounds[3] - bounds[1]);
        if !(w > 0.0 && h > 0.0) || max_pixels == 0 {
            return None;
        }
        let scale = (max_pixels as f64 / (w * h)).sqrt().min(1.0);
        let width = ((w * scale).ceil() as usize).max(1);
        let height = ((h * scale).ceil() as usize).max(1);
        // 出す画像の画素 (i, j) の中心は、貼り合わせの座標で左上の縁から (i + 0.5) / scale。
        let to_mosaic = Affine {
            a: 1.0 / scale,
            b: 0.0,
            c: bounds[0] + 0.5 / scale,
            d: 0.0,
            e: 1.0 / scale,
            f: bounds[1] + 0.5 / scale,
        };
        Some(Self {
            width,
            height,
            to_mosaic,
        })
    }

    /// 出す画像の 1 画素が、貼り合わせの座標の何画素ぶんか。
    pub fn step(&self) -> f64 {
        self.to_mosaic.a.hypot(self.to_mosaic.d)
    }
}

/// 重ねる 1 枚（値は画素ごとに色を並べた順、`channels` 個ずつ）。値が NaN の所は写っていないとみなす。
pub struct Layer<'a> {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub data: &'a [f32],
    /// 元の座標から貼り合わせの座標への写し方。
    pub placed: Warp,
    /// 値に掛ける比。
    pub gain: f32,
}

/// 重ねている途中の画像。1 枚ずつ足せるので、全部の画像を同時にメモリに持たずに済む。
pub struct Canvas {
    pub grid: Grid,
    pub channels: usize,
    blend: Blend,
    sums: Vec<f32>,
    weights: Vec<f32>,
}

impl Canvas {
    pub fn new(grid: Grid, channels: usize, blend: Blend) -> Self {
        let n = grid.width * grid.height;
        Self {
            grid,
            channels,
            blend,
            sums: vec![0.0; n * channels],
            weights: vec![0.0; n],
        }
    }

    /// 1 枚を重ねる。
    pub fn add(&mut self, layer: &Layer<'_>) {
        let Some(to_grid) = self.grid.to_mosaic.inverse() else {
            return;
        };
        let forward = layer.placed.then(&to_grid);
        let Some(back) = forward.back() else {
            return;
        };
        // 重ねる画像の縁を囲む、出す画像の行と列だけを回る。
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for [x, y] in forward.outline([layer.width, layer.height], OUTLINE_STEPS) {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let (gw, gh) = (self.grid.width, self.grid.height);
        let cols =
            (x0.floor().max(0.0) as usize)..((x1.ceil() + 1.0).clamp(0.0, gw as f64) as usize);
        let rows =
            (y0.floor().max(0.0) as usize)..((y1.ceil() + 1.0).clamp(0.0, gh as f64) as usize);
        if cols.is_empty() || rows.is_empty() {
            return;
        }
        let ch = self.channels;
        let blend = self.blend;
        let (half_w, half_h) = (layer.width as f64 / 2.0, layer.height as f64 / 2.0);
        let y_start = rows.start;
        let sums = &mut self.sums[rows.start * gw * ch..rows.end * gw * ch];
        let weights = &mut self.weights[rows.start * gw..rows.end * gw];
        sums.par_chunks_mut(gw * ch)
            .zip(weights.par_chunks_mut(gw))
            .enumerate()
            .for_each(|(j, (sum_row, weight_row))| {
                let y = (y_start + j) as f64;
                let mut value = vec![0.0f32; ch];
                for i in cols.clone() {
                    let Some([u, v]) = back.apply([i as f64, y]) else {
                        continue;
                    };
                    if !sample(layer, u, v, &mut value) {
                        continue;
                    }
                    let w = match blend {
                        Blend::Top => 1.0,
                        Blend::Feather => {
                            let dx = (u + 0.5).min(layer.width as f64 - 0.5 - u) / half_w;
                            let dy = (v + 0.5).min(layer.height as f64 - 0.5 - v) / half_h;
                            (dx.clamp(0.0, 1.0) * dy.clamp(0.0, 1.0)).max(1e-6) as f32
                        }
                    };
                    let out = &mut sum_row[i * ch..(i + 1) * ch];
                    match blend {
                        Blend::Top => {
                            for (o, v) in out.iter_mut().zip(&value) {
                                *o = v * layer.gain;
                            }
                            weight_row[i] = 1.0;
                        }
                        Blend::Feather => {
                            for (o, v) in out.iter_mut().zip(&value) {
                                *o += w * v * layer.gain;
                            }
                            weight_row[i] += w;
                        }
                    }
                }
            });
    }

    /// 重ね終えた値（画素ごとに色を並べた順）。どの画像も写っていない所は NaN。
    pub fn finish(self) -> Vec<f32> {
        let ch = self.channels;
        let mut out = self.sums;
        for (px, &w) in out.chunks_exact_mut(ch).zip(&self.weights) {
            for v in px {
                *v = if w > 0.0 { *v / w } else { f32::NAN };
            }
        }
        out
    }
}

/// `layer` の (x, y) の値（双一次補間）を `out` に入れる。画像の外か、近くに写っていない画素が
/// あれば `false`。
fn sample(layer: &Layer<'_>, x: f64, y: f64, out: &mut [f32]) -> bool {
    let (w, h) = (layer.width, layer.height);
    if x < -0.5 || y < -0.5 || x > w as f64 - 0.5 || y > h as f64 - 0.5 {
        return false;
    }
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let ch = layer.channels;
    let at = |x: usize, y: usize, c: usize| layer.data[(y * w + x) * ch + c];
    for (c, o) in out.iter_mut().enumerate().take(ch) {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
        let bottom = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
        let v = top * (1.0 - fy) + bottom * fy;
        if !v.is_finite() {
            return false;
        }
        *o = v;
    }
    true
}

#[cfg(test)]
mod tests;
