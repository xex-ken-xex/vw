//! 重なりのある何枚かの画像を、1 枚に貼り合わせる（天体のモザイク、風景のパノラマ、
//! 大きな絵を分けて撮ったもの）。
//!
//! 2 枚ずつ重なりを探し（[`link_all`]）、確かな組をたどって全部を 1 つの座標に置き（[`place`]）、
//! 置いた所に画素を重ねる（[`Canvas`]）。座標は、置く基準にした 1 枚の元の画素。
//!
//! 写し方はアフィン（ずらす・回す・大きさを変える）まで。カメラをその場で回したパノラマの
//! 遠近の歪みは扱わない。

use rayon::prelude::*;
use rustfft::num_complex::Complex32;

use crate::affine::Affine;
use crate::coarse::Fft2;
use crate::plane::Plane;
use crate::refine;

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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    /// `a` の元の画素の座標から `b` の元の画素の座標への写し方。
    pub map: Affine,
    /// 重なった所の相関（−1〜1）。
    pub correlation: f64,
    /// 重なった所の、`a` の面積に対する割合。
    pub overlap: f64,
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
    .then_some(Link {
        a,
        b,
        map,
        correlation: fitted.correlation,
        overlap: fitted.overlap,
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
    let mut placed: Vec<Option<Affine>> = vec![None; count];
    if count == 0 {
        return placed;
    }
    let mut weight = vec![0.0f64; count];
    for l in links {
        weight[l.a] += l.correlation;
        weight[l.b] += l.correlation;
    }
    let anchor = (0..count)
        .max_by(|&x, &y| weight[x].total_cmp(&weight[y]).then(y.cmp(&x)))
        .unwrap_or(0);
    placed[anchor] = Some(Affine::IDENTITY);
    loop {
        // 置いた画像と置いていない画像をつなぐ組のうち、いちばん確かなもの（Prim の方法）。
        let next = links
            .iter()
            .filter(|l| placed[l.a].is_some() != placed[l.b].is_some())
            .max_by(|x, y| x.correlation.total_cmp(&y.correlation));
        let Some(l) = next else {
            break;
        };
        let Some(to_b) = l.map.inverse() else {
            break;
        };
        match (placed[l.a], placed[l.b]) {
            (Some(pa), None) => placed[l.b] = Some(to_b.then(&pa)),
            (None, Some(pb)) => placed[l.a] = Some(l.map.then(&pb)),
            _ => break,
        }
    }
    placed
}

/// 置いた画像の 4 隅を含む範囲 `[左, 上, 右, 下]`（貼り合わせの座標）。
pub fn bounds(sizes: &[[usize; 2]], placed: &[Option<Affine>]) -> Option<[f64; 4]> {
    let mut out: Option<[f64; 4]> = None;
    for (size, p) in sizes.iter().zip(placed) {
        let Some(p) = p else {
            continue;
        };
        for corner in corners(*size) {
            let [x, y] = p.apply(corner);
            let b = out.get_or_insert([x, y, x, y]);
            *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
    }
    out
}

/// 画素の外側の縁の 4 隅（画素の中心を整数にとるので、左上は (−0.5, −0.5)）。
pub fn corners([w, h]: [usize; 2]) -> [[f64; 2]; 4] {
    let (r, b) = (w as f64 - 0.5, h as f64 - 0.5);
    [[-0.5, -0.5], [r, -0.5], [r, b], [-0.5, b]]
}

/// 2 枚の重なりの、小さいほうの画像の面積に対する割合（0〜1）。格子の点で数える。
pub fn shared(sa: [usize; 2], pa: &Affine, sb: [usize; 2], pb: &Affine) -> f64 {
    let (small, ps, big, pg) = if sa[0] * sa[1] <= sb[0] * sb[1] {
        (sa, pa, sb, pb)
    } else {
        (sb, pb, sa, pa)
    };
    let Some(to_big) = pg.inverse() else {
        return 0.0;
    };
    const STEPS: usize = 24;
    let mut inside = 0;
    for j in 0..STEPS {
        for i in 0..STEPS {
            let x = (i as f64 + 0.5) / STEPS as f64 * small[0] as f64 - 0.5;
            let y = (j as f64 + 0.5) / STEPS as f64 * small[1] as f64 - 0.5;
            let [u, v] = to_big.apply(ps.apply([x, y]));
            if u >= -0.5 && v >= -0.5 && u <= big[0] as f64 - 0.5 && v <= big[1] as f64 - 0.5 {
                inside += 1;
            }
        }
    }
    f64::from(inside) / (STEPS * STEPS) as f64
}

/// 同じ所を写した画像のまとまり（小さいほうの面積の `share` 以上が重なる画像どうしをつないだもの）。
/// どのまとまりも 2 枚以上。並びは、まとまりの中も外も画像の番号の順。
pub fn same_area(sizes: &[[usize; 2]], placed: &[Option<Affine>], share: f64) -> Vec<Vec<usize>> {
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
    /// 2 枚を直接合わせた写し方と、置いた所との食い違い（元の画素、重なりの中でいちばん大きい所）。
    /// 2 枚を直接合わせられなかったときは `None`。
    pub mismatch: Option<f64>,
    /// 重なった所の明るさの差（明るいほうに対する割合、0〜1）。
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
    placed: &[Option<Affine>],
    links: &[Link],
    used: &[bool],
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
            // a の元の座標 → b の元の座標。
            let implied = pa.then(&pb.inverse()?);
            Some(seam(pieces, links, a, b, implied, share))
        })
        .collect()
}

fn seam(pieces: &[Piece], links: &[Link], a: usize, b: usize, implied: Affine, share: f64) -> Seam {
    let (pa, pb) = (&pieces[a], &pieces[b]);
    let small = implied.between_scales(pa.scale(), pb.scale());
    let (correlation, _) = refine::score(&pa.plane, &pb.plane, &small);
    let direct = links.iter().find_map(|l| match (l.a, l.b) {
        (x, y) if x == a && y == b => Some(l.map),
        (x, y) if x == b && y == a => l.map.inverse(),
        _ => None,
    });
    let mismatch = direct.map(|d| {
        overlap_points(pa, pb, &implied)
            .into_iter()
            .map(|p| {
                let (u, v) = (implied.apply(p), d.apply(p));
                (u[0] - v[0]).hypot(u[1] - v[1])
            })
            .fold(0.0, f64::max)
    });
    let (brightness, sharpness) = looks(pa, pb, &small);
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

/// `a` の元の座標で、`b` にも写る格子の点。
fn overlap_points(pa: &Piece, pb: &Piece, implied: &Affine) -> Vec<[f64; 2]> {
    const STEPS: usize = 16;
    let mut out = Vec::new();
    for j in 0..=STEPS {
        for i in 0..=STEPS {
            let p = [
                i as f64 / STEPS as f64 * (pa.full[0] - 1) as f64,
                j as f64 / STEPS as f64 * (pa.full[1] - 1) as f64,
            ];
            let [u, v] = implied.apply(p);
            if u >= 0.0 && v >= 0.0 && u <= (pb.full[0] - 1) as f64 && v <= (pb.full[1] - 1) as f64
            {
                out.push(p);
            }
        }
    }
    out
}

/// 重なった所の明るさの差と、鮮鋭度の比。`small` は縮めた画像どうしの写し方。
fn looks(pa: &Piece, pb: &Piece, small: &Affine) -> (f64, f64) {
    let (a, b) = (&pa.plane, &pb.plane);
    let (mut sum_a, mut sum_b, mut grad_a, mut grad_b, mut count) = (0.0, 0.0, 0.0, 0.0, 0usize);
    // b の勾配は、a の 1 画素ぶん離れた点との差で測る。縮めた倍率が違っても、同じ広さで比べるため。
    for y in 1..a.height.saturating_sub(1) {
        for x in 1..a.width.saturating_sub(1) {
            let [u, v] = small.apply([x as f64, y as f64]);
            let (Some(vb), Some(vbx), Some(vby)) = (
                b.sample(u, v),
                b.sample(u + small.a, v + small.d),
                b.sample(u + small.b, v + small.e),
            ) else {
                continue;
            };
            let va = a.at(x, y);
            sum_a += f64::from(va);
            sum_b += f64::from(vb);
            grad_a += f64::from((a.at(x + 1, y) - va).powi(2) + (a.at(x, y + 1) - va).powi(2));
            grad_b += f64::from((vbx - vb).powi(2) + (vby - vb).powi(2));
            count += 1;
        }
    }
    if count == 0 {
        return (0.0, 1.0);
    }
    let (ma, mb) = (sum_a / count as f64, sum_b / count as f64);
    let brightness = if ma.max(mb) > 0.0 {
        (ma - mb).abs() / ma.max(mb)
    } else {
        0.0
    };
    let sharpness = if grad_a.max(grad_b) > 0.0 {
        grad_a.min(grad_b) / grad_a.max(grad_b)
    } else {
        1.0
    };
    (brightness, sharpness)
}

/// 重なった所の明るさがそろうように、画像ごとに掛ける比（使わない画像と置けなかった画像は 1）。
///
/// つなぎ目ごとの明るさの差の 2 乗と、比の 1 からの離れの 2 乗の和を最小にする
/// （比をすべて 0 にすれば差は消えるので、1 から離れることにも重みを付ける）。
pub fn gains(pieces: &[Piece], placed: &[Option<Affine>], used: &[bool]) -> Vec<f64> {
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
            let Some(implied) = pb.inverse().map(|inv| pa.then(&inv)) else {
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

/// 連立 1 次方程式（ガウスの消去法、部分ピボット）。
fn solve(mut h: Vec<Vec<f64>>, mut g: Vec<f64>) -> Option<Vec<f64>> {
    let n = g.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&x, &y| h[x][col].abs().total_cmp(&h[y][col].abs()))?;
        if h[pivot][col].abs() < 1e-12 {
            return None;
        }
        h.swap(col, pivot);
        g.swap(col, pivot);
        let (top, below) = h.split_at_mut(col + 1);
        let pivot_row = &top[col];
        for (row, r) in below.iter_mut().enumerate() {
            let k = r[col] / pivot_row[col];
            for (v, p) in r[col..].iter_mut().zip(&pivot_row[col..]) {
                *v -= k * p;
            }
            g[col + 1 + row] -= k * g[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|c| h[row][c] * x[c]).sum();
        x[row] = (g[row] - s) / h[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
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
    pub placed: Affine,
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
        let Some(to_layer) = layer.placed.inverse() else {
            return;
        };
        let back = self.grid.to_mosaic.then(&to_layer);
        let Some(forward) = back.inverse() else {
            return;
        };
        // 重ねる画像の 4 隅を囲む、出す画像の行と列だけを回る。
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for c in corners([layer.width, layer.height]) {
            let [x, y] = forward.apply(c);
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
                    let [u, v] = back.apply([i as f64, y]);
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
