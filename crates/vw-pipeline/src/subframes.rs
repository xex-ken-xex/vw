//! サブフレーム（天体を何枚にも分けて撮った 1 枚ずつ）の評価と位置合わせ。
//!
//! 評価は、積み重ねる前に、ぼけた（シーイングが悪い）・流れた（追尾が外れた）・雲がかかった・
//! 薄明や月で明るい 1 枚を見つけて除くため。星の形は [`crate::deconv::find_stars`] で測る。
//! 位置合わせは、明るい星の組の距離が回転やずれで変わらないことを使って、星どうしを対応させる。

use crate::deconv::{find_stars, FWHM_PER_SIGMA};
use crate::nodes::Float;

/// 1 枚の評価。
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    /// 星の FWHM の中央値（画素）。星がなければ `None`。
    pub fwhm: Option<f32>,
    /// 星の離心率の中央値（0 で丸、1 に近いほど細長い）。
    pub eccentricity: Option<f32>,
    /// 見つかった星の数。
    pub stars: usize,
    /// 背景（明るさの中央値）。値は画像の値のまま（FITS なら ADU）。
    pub background: f32,
    /// 背景の雑音（標準偏差）。
    pub noise: f32,
    /// 位置合わせに使う明るい星（明るい順）。
    pub bright: Vec<Point>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// 位置合わせに使う星の数。多いほど外れにくいが、組の数が 2 乗で増える。
const BRIGHT_STARS: usize = 30;
/// 測る値に使う星の楕円率の上限。重なった星や銀河を除く（離心率では 0.94 ほど）。
const SHAPE_LIMIT: f32 = 0.8;

/// 1 枚を評価する。
pub fn evaluate(img: &Float) -> Evaluation {
    let mut stars = find_stars(img).unwrap_or_default();
    stars.retain(|s| s.ellipticity < SHAPE_LIMIT);
    let mut fwhms: Vec<f32> = stars.iter().map(|s| s.sigma * FWHM_PER_SIGMA).collect();
    let mut eccentricities: Vec<f32> = stars.iter().map(|s| eccentricity(s.ellipticity)).collect();
    let (background, noise) = background_and_noise(img);
    stars.sort_by(|a, b| b.flux.total_cmp(&a.flux));
    Evaluation {
        fwhm: crate::astro::median(&mut fwhms),
        eccentricity: crate::astro::median(&mut eccentricities),
        stars: stars.len(),
        background,
        noise,
        bright: stars
            .iter()
            .take(BRIGHT_STARS)
            .map(|s| Point { x: s.x, y: s.y })
            .collect(),
    }
}

/// 2 次のモーメントの楕円率 (λ1 − λ2)/(λ1 + λ2) を、楕円の離心率 √(1 − 短軸²/長軸²) に直す。
/// 天体写真の処理で広く使われている尺度にそろえるため。
fn eccentricity(ellipticity: f32) -> f32 {
    let e = ellipticity.clamp(0.0, 0.999);
    (2.0 * e / (1.0 + e)).sqrt()
}

/// 明るさの中央値と、隣の画素との差から測った雑音。差を使うのは、星雲などのなだらかな明るさの
/// 違いを雑音に数えないため（隣どうしの差は、なだらかな所ではほぼ雑音だけになる）。
fn background_and_noise(img: &Float) -> (f32, f32) {
    const SAMPLES: usize = 1 << 20;
    let n = img.channels.max(1);
    let luma: Vec<f32> = img
        .data
        .chunks_exact(n)
        .map(|px| px.iter().sum::<f32>() / n as f32)
        .collect();
    let step = luma.len().div_ceil(SAMPLES).max(1);
    let mut values: Vec<f32> = luma
        .iter()
        .step_by(step)
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    let background = crate::astro::median(&mut values).unwrap_or(0.0);
    let w = img.width.max(1);
    let mut differences: Vec<f32> = (0..luma.len().saturating_sub(1))
        .step_by(step)
        .filter(|i| (i + 1) % w != 0)
        .map(|i| (luma[i + 1] - luma[i]).abs())
        .filter(|v| v.is_finite())
        .collect();
    // 差の中央値 × 1.4826 は差の σ（√2 σ）。√2 で割って 1 画素の σ にする。
    let noise =
        crate::astro::median(&mut differences).unwrap_or(0.0) * 1.4826 / std::f32::consts::SQRT_2;
    (background, noise)
}

/// 基準の画像の画素 (x, y) を、別の画像の画素 (x', y') に写す 1 次の式。
/// `x' = m[0][0] x + m[0][1] y + m[0][2]`、`y' = m[1][0] x + m[1][1] y + m[1][2]`。
pub type Map = [[f64; 3]; 2];

/// 同じ星とみなす距離（画素）。
const MATCH_PIXELS: f64 = 2.5;
/// 組の距離がこれより近い星は使わない（距離の誤差に対して角度が大きく振れるため）。
const MIN_PAIR_DISTANCE: f64 = 20.0;
/// 合ったとみなすのに要る、対応の付いた星の数の下限。星の多い画像では、少ない方の星の数の
/// 3 分の 1 も要る（星が混んでいると、関係のない並びでも数個は偶然重なるため）。
const MIN_MATCHES: usize = 4;

/// `reference` の星を `other` の星に重ねる式（ずれ・回転・わずかな倍率）。合わなければ `None`。
///
/// よく使われる三角形の形で対応を探す方法より簡単な、2 つの星の距離の一致で候補を作り、
/// 候補ごとに重なる星を数える方法にした。サブフレームどうしは倍率がほぼ同じなので距離がそのまま
/// 使え、30 個の星なら組は 435 で、全部を当たっても一瞬で済むため。
pub fn align(reference: &[Point], other: &[Point]) -> Option<Map> {
    let r: Vec<[f64; 2]> = reference.iter().map(|p| [p.x.into(), p.y.into()]).collect();
    let o: Vec<[f64; 2]> = other.iter().map(|p| [p.x.into(), p.y.into()]).collect();
    if r.len() < MIN_MATCHES || o.len() < MIN_MATCHES {
        return None;
    }
    let mut other_pairs: Vec<(f64, usize, usize)> = pairs(&o)
        .filter(|(d, _, _)| *d >= MIN_PAIR_DISTANCE)
        .collect();
    other_pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut best: Option<(usize, Vec<(usize, usize)>)> = None;
    for (d, i, j) in pairs(&r).filter(|(d, _, _)| *d >= MIN_PAIR_DISTANCE) {
        let tolerance = MATCH_PIXELS * 2.0;
        let from = other_pairs.partition_point(|p| p.0 < d - tolerance);
        for &(_, k, l) in other_pairs[from..]
            .iter()
            .take_while(|p| p.0 <= d + tolerance)
        {
            for (a, b) in [(k, l), (l, k)] {
                let m = similarity_from_two(r[i], r[j], o[a], o[b]);
                let matched = matches(&r, &o, &m);
                if best.as_ref().is_none_or(|(n, _)| matched.len() > *n) {
                    best = Some((matched.len(), matched));
                }
            }
        }
    }
    let (count, matched) = best?;
    if count < MIN_MATCHES.max(r.len().min(o.len()) / 3) {
        return None;
    }
    let pairs: Vec<Pair> = matched.iter().map(|&(i, k)| (r[i], o[k])).collect();
    let mut m = fit_similarity(&pairs)?;
    // 当て直した式で対応を取り直し、もう一度当てる（初めの 2 つの星の位置の誤差を薄めるため）。
    let again: Vec<Pair> = matches(&r, &o, &m)
        .iter()
        .map(|&(i, k)| (r[i], o[k]))
        .collect();
    if again.len() >= MIN_MATCHES {
        m = fit_similarity(&again)?;
    }
    Some(m)
}

fn pairs(points: &[[f64; 2]]) -> impl Iterator<Item = (f64, usize, usize)> + '_ {
    (0..points.len()).flat_map(move |i| {
        (i + 1..points.len()).map(move |j| (distance(points[i], points[j]), i, j))
    })
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

pub fn apply(m: &Map, p: [f64; 2]) -> [f64; 2] {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2],
    ]
}

/// 基準の星 (r1, r2) を (o1, o2) に重ねる、回転と倍率とずれ。
fn similarity_from_two(r1: [f64; 2], r2: [f64; 2], o1: [f64; 2], o2: [f64; 2]) -> Map {
    let v = [r2[0] - r1[0], r2[1] - r1[1]];
    let u = [o2[0] - o1[0], o2[1] - o1[1]];
    // 複素数の割り算 u / v。
    let len = v[0] * v[0] + v[1] * v[1];
    let a = (u[0] * v[0] + u[1] * v[1]) / len;
    let b = (u[1] * v[0] - u[0] * v[1]) / len;
    [
        [a, -b, o1[0] - (a * r1[0] - b * r1[1])],
        [b, a, o1[1] - (b * r1[0] + a * r1[1])],
    ]
}

/// 写した基準の星のそばに相手の星がある組（基準の番号, 相手の番号）。相手の星は 1 度だけ使う。
fn matches(r: &[[f64; 2]], o: &[[f64; 2]], m: &Map) -> Vec<(usize, usize)> {
    let mut used = vec![false; o.len()];
    let mut out = Vec::new();
    for (i, p) in r.iter().enumerate() {
        let q = apply(m, *p);
        let nearest = o
            .iter()
            .enumerate()
            .filter(|(k, _)| !used[*k])
            .map(|(k, s)| (distance(q, *s), k))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((d, k)) = nearest
            && d <= MATCH_PIXELS {
                used[k] = true;
                out.push((i, k));
            }
    }
    out
}

/// 基準の星と、それに対応するもう 1 枚の星。
type Pair = ([f64; 2], [f64; 2]);

/// 対応の付いた星の組に、最小二乗で回転と倍率とずれを当てる。
fn fit_similarity(pairs: &[Pair]) -> Option<Map> {
    let n = pairs.len() as f64;
    let mean = |f: &dyn Fn(&Pair) -> f64| pairs.iter().map(f).sum::<f64>() / n;
    let (rx, ry) = (mean(&|p| p.0[0]), mean(&|p| p.0[1]));
    let (ox, oy) = (mean(&|p| p.1[0]), mean(&|p| p.1[1]));
    let (mut sa, mut sb, mut ss) = (0.0, 0.0, 0.0);
    for (r, o) in pairs {
        let (x, y) = (r[0] - rx, r[1] - ry);
        let (u, v) = (o[0] - ox, o[1] - oy);
        sa += x * u + y * v;
        sb += x * v - y * u;
        ss += x * x + y * y;
    }
    if ss <= 0.0 {
        return None;
    }
    let (a, b) = (sa / ss, sb / ss);
    Some([
        [a, -b, ox - (a * rx - b * ry)],
        [b, a, oy - (b * rx + a * ry)],
    ])
}

/// 除くかを決める値。比は全部の枚数の中央値に対するもの。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// FWHM が中央値のこの倍を超えたら除く。
    pub fwhm_ratio: f32,
    /// 離心率がこれを超えたら除く。
    pub eccentricity: f32,
    /// 星の数が中央値のこの倍より少なければ除く（雲）。
    pub stars_ratio: f32,
    /// 背景が中央値から雑音の中央値のこの倍より明るければ除く（薄明・月・街明かり）。
    pub background_sigmas: f32,
}

/// 除く理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Blurred,
    Elongated,
    FewStars,
    BrightSky,
    NoStars,
}

impl Reason {
    pub fn title(self) -> &'static str {
        match self {
            Reason::Blurred => "FWHM",
            Reason::Elongated => "Eccentricity",
            Reason::FewStars => "Stars",
            Reason::BrightSky => "Background",
            Reason::NoStars => "No stars",
        }
    }
}

/// 1 枚ずつの、除く理由（なければ空）。
pub fn reasons(evaluations: &[Evaluation], limits: Limits) -> Vec<Vec<Reason>> {
    let median_of = |f: &dyn Fn(&Evaluation) -> Option<f32>| {
        let mut v: Vec<f32> = evaluations.iter().filter_map(f).collect();
        crate::astro::median(&mut v)
    };
    let fwhm = median_of(&|e| e.fwhm);
    let stars = median_of(&|e| Some(e.stars as f32));
    let background = median_of(&|e| Some(e.background));
    let noise = median_of(&|e| Some(e.noise));
    evaluations
        .iter()
        .map(|e| {
            let mut out = Vec::new();
            if e.stars == 0 {
                out.push(Reason::NoStars);
            }
            if let (Some(f), Some(m)) = (e.fwhm, fwhm)
                && f > m * limits.fwhm_ratio {
                    out.push(Reason::Blurred);
                }
            if e.eccentricity.is_some_and(|x| x > limits.eccentricity) {
                out.push(Reason::Elongated);
            }
            if let Some(m) = stars
                && e.stars > 0 && (e.stars as f32) < m * limits.stars_ratio {
                    out.push(Reason::FewStars);
                }
            if let (Some(b), Some(n)) = (background, noise)
                && e.background > b + limits.background_sigmas * n.max(f32::EPSILON) {
                    out.push(Reason::BrightSky);
                }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests;
