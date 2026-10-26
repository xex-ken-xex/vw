//! 合わせた 2 枚の違いを数にする。値はどれも表示の明るさ（0〜255、sRGB）で測る。
//!
//! 測る範囲は、2 枚が重なった所に収まる長方形。PSNR・SSIM・鮮鋭度は画質の評価
//! （vw-quality）と同じ求め方にし、評価のタブと値を見比べられるようにする。

use rayon::prelude::*;
use vw_quality::{Image, SsimMap};

use crate::align::Warped;
use crate::color::{delta_e2000, lab};
use crate::plane::{Plane, Rgb};

/// 違いの大きい場所をいくつ挙げるか。
pub const HOTSPOTS: usize = 5;
const PEAK: f32 = 255.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// 比べる前に、比べる画像の明るさを基準に合わせる（チャンネルごとのヒストグラムで）。
    pub match_brightness: bool,
}

/// 格子の中の長方形（画素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// 基準と比べる画像の、同じ尺度の値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pair {
    pub reference: f64,
    pub moving: f64,
}

impl Pair {
    /// 比べる画像 / 基準。
    pub fn ratio(&self) -> f64 {
        if self.reference == 0.0 {
            f64::NAN
        } else {
            self.moving / self.reference
        }
    }
}

/// チャンネルごとの明るさのずれ（比べる画像 − 基準）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Offset {
    pub mean: f64,
    pub median: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DeltaE {
    pub mean: f64,
    pub p95: f64,
    pub max: f64,
}

/// 違いの大きい場所と、そこの違いの平均。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hotspot {
    pub rect: Rect,
    pub mean: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// 測った長方形。
    pub region: Rect,
    pub matched: bool,
    pub psnr: Option<f64>,
    pub ssim: Option<f64>,
    /// 測った長方形の中の SSIM の地図。
    pub ssim_map: Option<SsimMap>,
    pub delta_e: DeltaE,
    /// R・G・B の明るさのずれ。明るさを合わせる前の値。
    pub offset: [Offset; 3],
    /// ラプラシアンの分散。
    pub sharpness: Option<Pair>,
    /// 平らな所の、細かい揺らぎの標準偏差。
    pub noise: Option<Pair>,
    /// 周辺の明るさ / 中心の明るさ（1 で落ちていない）。明るさを合わせる前の値。
    pub vignetting: Option<Pair>,
    /// 明るさのヒストグラム（256 段）。明るさを合わせる前の値。
    pub histograms: [Vec<u32>; 2],
    /// 格子全体の、画素ごとの違い（3 色の差の絶対値の平均）。重ならない所は `None`。
    pub difference: Vec<Option<f32>>,
    pub difference_max: f32,
    pub hotspots: Vec<Hotspot>,
}

/// 基準の格子 `reference` と、そこに写した比べる画像 `warped` を比べる。重なりがなければ `None`。
pub fn analyse(reference: &Rgb, warped: &Warped, options: Options) -> Option<Analysis> {
    assert_eq!(
        (reference.width, reference.height),
        (warped.image.width, warped.image.height)
    );
    let region = inscribed(&warped.valid, reference.width, reference.height)?;
    let r_raw = crop(reference, region);
    let m_raw = crop(&warped.image, region);
    let luts = options
        .match_brightness
        .then(|| matching_luts(&m_raw, &r_raw));
    let m = match &luts {
        Some(l) => apply_luts(&m_raw, l),
        None => m_raw.clone(),
    };
    let (rl, ml, ml_raw) = (r_raw.luma(), m.luma(), m_raw.luma());
    let (ri, mi) = (image(&rl), image(&ml));
    let ssim_map = vw_quality::ssim_map(&ri, &mi, PEAK);
    let difference = difference_map(reference, &warped.image, &warped.valid, luts.as_ref());
    let difference_max = difference.iter().flatten().fold(0.0f32, |a, &b| a.max(b));
    let hotspots = hotspots(&difference, reference.width, reference.height, region);
    Some(Analysis {
        region,
        matched: options.match_brightness,
        psnr: vw_quality::psnr(&ri, &mi, PEAK),
        ssim: ssim_map.as_ref().map(|m| m.mean),
        ssim_map,
        delta_e: delta_e(&r_raw, &m),
        offset: offsets(&r_raw, &m_raw),
        sharpness: pair(
            vw_quality::laplacian_variance(&ri),
            vw_quality::laplacian_variance(&mi),
        ),
        noise: noise(&rl, &ml),
        vignetting: pair(falloff(&rl), falloff(&ml_raw)),
        histograms: [histogram(&rl), histogram(&ml_raw)],
        difference,
        difference_max,
        hotspots,
    })
}

fn image(p: &Plane) -> Image<'_> {
    Image::new(p.width as u32, p.height as u32, &p.data)
}

fn pair(reference: Option<f64>, moving: Option<f64>) -> Option<Pair> {
    Some(Pair {
        reference: reference?,
        moving: moving?,
    })
}

/// 印の付いた画素だけでできた、なるべく大きい長方形。外接する長方形から、
/// 印のない画素がいちばん多い辺を 1 列ずつ削っていく。
pub fn inscribed(valid: &[bool], width: usize, height: usize) -> Option<Rect> {
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    for y in 0..height {
        for x in 0..width {
            if valid[y * width + x] {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    while x0 < x1 && y0 < y1 {
        let missing_row = |y: usize| (x0..x1).filter(|&x| !valid[y * width + x]).count();
        let missing_col = |x: usize| (y0..y1).filter(|&y| !valid[y * width + x]).count();
        let sides = [
            missing_row(y0) as f64 / (x1 - x0) as f64,
            missing_row(y1 - 1) as f64 / (x1 - x0) as f64,
            missing_col(x0) as f64 / (y1 - y0) as f64,
            missing_col(x1 - 1) as f64 / (y1 - y0) as f64,
        ];
        let (worst, &share) = sides
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap_or((0, &0.0));
        if share == 0.0 {
            return Some(Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            });
        }
        match worst {
            0 => y0 += 1,
            1 => y1 -= 1,
            2 => x0 += 1,
            _ => x1 -= 1,
        }
    }
    None
}

fn crop(img: &Rgb, r: Rect) -> Rgb {
    let mut data = Vec::with_capacity(r.w * r.h);
    for y in r.y..r.y + r.h {
        data.extend_from_slice(&img.data[y * img.width + r.x..y * img.width + r.x + r.w]);
    }
    Rgb {
        width: r.w,
        height: r.h,
        data,
    }
}

/// `moving` の各チャンネルの明るさの分布を、`target` の分布に合わせる表。
pub fn matching_luts(moving: &Rgb, target: &Rgb) -> [Vec<f32>; 3] {
    let mut luts: [Vec<f32>; 3] = Default::default();
    luts.par_iter_mut().enumerate().for_each(|(c, lut)| {
        *lut = matching_lut(&cdf(&moving.data, c), &cdf(&target.data, c));
    });
    luts
}

/// [`matching_luts`] の表を当てる。
pub fn apply_luts(img: &Rgb, luts: &[Vec<f32>; 3]) -> Rgb {
    Rgb {
        width: img.width,
        height: img.height,
        data: img
            .data
            .par_iter()
            .map(|p| std::array::from_fn(|c| apply_lut(&luts[c], p[c])))
            .collect(),
    }
}

/// 1 段 = 1/4 の明るさで、累積分布を対応させる表（`BINS + 1` 点）。
const BINS: usize = 1024;

/// チャンネル `c` の累積分布（`BINS + 1` 点）。
fn cdf(values: &[[f32; 3]], c: usize) -> Vec<f64> {
    let hist = values
        .par_chunks(1 << 16)
        .map(|chunk| {
            let mut h = vec![0u64; BINS];
            for v in chunk {
                h[bin(v[c])] += 1;
            }
            h
        })
        .reduce(
            || vec![0u64; BINS],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    *x += y;
                }
                a
            },
        );
    let total = values.len().max(1) as f64;
    let mut out = Vec::with_capacity(BINS + 1);
    let mut acc = 0u64;
    out.push(0.0);
    for h in hist {
        acc += h;
        out.push(acc as f64 / total);
    }
    out
}

fn bin(v: f32) -> usize {
    ((v / PEAK * BINS as f32) as usize).min(BINS - 1)
}

fn matching_lut(cf: &[f64], ct: &[f64]) -> Vec<f32> {
    let step = PEAK / BINS as f32;
    // 段の境目 i の明るさ（i · step）を、累積が同じになる目標の明るさに写す。
    let mut lut = Vec::with_capacity(BINS + 1);
    let mut j = 0usize;
    for &q in cf {
        while j + 1 < ct.len() && ct[j + 1] < q {
            j += 1;
        }
        let (lo, hi) = (ct[j], ct[(j + 1).min(BINS)]);
        let frac = if hi > lo {
            ((q - lo) / (hi - lo)) as f32
        } else {
            0.0
        };
        lut.push((j as f32 + frac.clamp(0.0, 1.0)) * step);
    }
    lut
}

fn apply_lut(lut: &[f32], v: f32) -> f32 {
    let pos = (v / PEAK * BINS as f32).clamp(0.0, BINS as f32);
    let i = (pos.floor() as usize).min(BINS - 1);
    let f = pos - i as f32;
    lut[i] * (1.0 - f) + lut[i + 1] * f
}

fn offsets(r: &Rgb, m: &Rgb) -> [Offset; 3] {
    std::array::from_fn(|c| {
        let mut d: Vec<f32> = r
            .data
            .iter()
            .zip(&m.data)
            .map(|(a, b)| b[c] - a[c])
            .collect();
        if d.is_empty() {
            return Offset::default();
        }
        let mean = d.iter().map(|&v| f64::from(v)).sum::<f64>() / d.len() as f64;
        let mid = d.len() / 2;
        d.select_nth_unstable_by(mid, f32::total_cmp);
        Offset {
            mean,
            median: f64::from(d[mid]),
        }
    })
}

fn delta_e(r: &Rgb, m: &Rgb) -> DeltaE {
    let mut d: Vec<f32> = r
        .data
        .par_iter()
        .zip(&m.data)
        .map(|(&a, &b)| delta_e2000(lab(a), lab(b)) as f32)
        .collect();
    if d.is_empty() {
        return DeltaE::default();
    }
    let mean = d.iter().map(|&v| f64::from(v)).sum::<f64>() / d.len() as f64;
    let max = d.iter().fold(0.0f32, |a, &b| a.max(b));
    let k = ((d.len() as f64 * 0.95) as usize).min(d.len() - 1);
    d.select_nth_unstable_by(k, f32::total_cmp);
    DeltaE {
        mean,
        p95: f64::from(d[k]),
        max: f64::from(max),
    }
}

/// 格子全体の画素ごとの違い。明るさを合わせるときは、測った長方形で作った表を全体に当てる。
fn difference_map(
    reference: &Rgb,
    moving: &Rgb,
    valid: &[bool],
    luts: Option<&[Vec<f32>; 3]>,
) -> Vec<Option<f32>> {
    reference
        .data
        .par_iter()
        .zip(&moving.data)
        .zip(valid)
        .map(|((a, b), &ok)| {
            ok.then(|| {
                (0..3)
                    .map(|c| {
                        let v = match luts {
                            Some(l) => apply_lut(&l[c], b[c]),
                            None => b[c],
                        };
                        (v - a[c]).abs()
                    })
                    .sum::<f32>()
                    / 3.0
            })
        })
        .collect()
}

/// 違いの平均が大きいマス（重なりの中に収まるもの）を、隣り合わないように上から選ぶ。
fn hotspots(diff: &[Option<f32>], width: usize, height: usize, region: Rect) -> Vec<Hotspot> {
    let side = (width.max(height) / 32).max(16);
    let (cols, rows) = (region.w / side, region.h / side);
    let mut cells = Vec::new();
    for j in 0..rows {
        for i in 0..cols {
            let rect = Rect {
                x: region.x + i * side,
                y: region.y + j * side,
                w: side,
                h: side,
            };
            let mut sum = 0.0f64;
            for y in rect.y..rect.y + side {
                for x in rect.x..rect.x + side {
                    sum += f64::from(diff[y * width + x].unwrap_or(0.0));
                }
            }
            cells.push((i, j, (sum / (side * side) as f64) as f32, rect));
        }
    }
    cells.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut picked: Vec<(usize, usize, f32, Rect)> = Vec::new();
    for c in cells {
        if picked.len() == HOTSPOTS || c.2 <= 0.0 {
            break;
        }
        let near = picked
            .iter()
            .any(|p| p.0.abs_diff(c.0) <= 1 && p.1.abs_diff(c.1) <= 1);
        if !near {
            picked.push(c);
        }
    }
    picked
        .into_iter()
        .map(|(_, _, mean, rect)| Hotspot { rect, mean })
        .collect()
}

/// 平らな所（勾配の小さい 1 割のマス）で、3×3 の平均からの揺らぎの標準偏差。
fn noise(r: &Plane, m: &Plane) -> Option<Pair> {
    const SIDE: usize = 16;
    let (cols, rows) = (r.width / SIDE, r.height / SIDE);
    if cols * rows < 4 {
        return None;
    }
    let mut cells: Vec<(f64, usize, usize)> = Vec::new();
    for j in 0..rows {
        for i in 0..cols {
            let mut e = 0.0f64;
            for y in j * SIDE + 1..(j + 1) * SIDE - 1 {
                for x in i * SIDE + 1..(i + 1) * SIDE - 1 {
                    let gx = r.at(x + 1, y) - r.at(x - 1, y);
                    let gy = r.at(x, y + 1) - r.at(x, y - 1);
                    e += f64::from(gx.abs() + gy.abs());
                }
            }
            cells.push((e, i, j));
        }
    }
    cells.sort_by(|a, b| a.0.total_cmp(&b.0));
    let take = (cells.len() / 10).max(4).min(cells.len());
    let flat = &cells[..take];
    let wobble = |p: &Plane| {
        let (mut sum, mut n) = (0.0f64, 0usize);
        for &(_, i, j) in flat {
            let mut vals = Vec::with_capacity(SIDE * SIDE);
            for y in j * SIDE + 1..(j + 1) * SIDE - 1 {
                for x in i * SIDE + 1..(i + 1) * SIDE - 1 {
                    let mut box_sum = 0.0;
                    for dy in 0..3 {
                        for dx in 0..3 {
                            box_sum += p.at(x + dx - 1, y + dy - 1);
                        }
                    }
                    vals.push(f64::from(p.at(x, y) - box_sum / 9.0));
                }
            }
            let mean = vals.iter().sum::<f64>() / vals.len() as f64;
            sum += vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>();
            n += vals.len();
        }
        (sum / n.max(1) as f64).sqrt()
    };
    Some(Pair {
        reference: wobble(r),
        moving: wobble(m),
    })
}

/// 隅（中心からの距離が対角の半分の 8 割より遠い所）の明るさ / 中心（2 割より近い所）の明るさ。
fn falloff(p: &Plane) -> Option<f64> {
    let (cx, cy) = ((p.width as f64 - 1.0) / 2.0, (p.height as f64 - 1.0) / 2.0);
    let half = cx.hypot(cy);
    if half < 4.0 {
        return None;
    }
    let (mut centre, mut cn, mut edge, mut en) = (0.0f64, 0usize, 0.0f64, 0usize);
    for y in 0..p.height {
        for x in 0..p.width {
            let r = (x as f64 - cx).hypot(y as f64 - cy) / half;
            if r < 0.2 {
                centre += f64::from(p.at(x, y));
                cn += 1;
            } else if r > 0.8 {
                edge += f64::from(p.at(x, y));
                en += 1;
            }
        }
    }
    if cn == 0 || en == 0 || centre == 0.0 {
        return None;
    }
    Some((edge / en as f64) / (centre / cn as f64))
}

fn histogram(p: &Plane) -> Vec<u32> {
    let mut h = vec![0u32; 256];
    for &v in &p.data {
        h[(v.round().clamp(0.0, 255.0)) as usize] += 1;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::color_texture;

    fn all_valid(img: &Rgb) -> Warped {
        Warped {
            image: img.clone(),
            valid: vec![true; img.width * img.height],
        }
    }

    #[test]
    fn the_same_image_has_no_difference() {
        let r = color_texture(128, 96);
        let a = analyse(&r, &all_valid(&r), Options::default()).unwrap();
        assert_eq!(
            a.region,
            Rect {
                x: 0,
                y: 0,
                w: 128,
                h: 96
            }
        );
        assert_eq!(a.psnr, Some(f64::INFINITY));
        assert!((a.ssim.unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(a.delta_e.max, 0.0);
        assert_eq!(a.difference_max, 0.0);
        assert!(a.hotspots.is_empty());
        assert!((a.sharpness.unwrap().ratio() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn matching_brightness_removes_a_tone_change_but_keeps_the_offset() {
        let r = color_texture(128, 96);
        let mut m = r.clone();
        for p in &mut m.data {
            *p = p.map(|v| (v * 0.8 + 20.0).min(255.0));
        }
        let plain = analyse(&r, &all_valid(&m), Options::default()).unwrap();
        let matched = analyse(
            &r,
            &all_valid(&m),
            Options {
                match_brightness: true,
            },
        )
        .unwrap();
        assert!(matched.delta_e.mean < plain.delta_e.mean / 4.0);
        assert!(matched.psnr.unwrap() > plain.psnr.unwrap() + 10.0);
        // ずれは合わせる前の値。
        assert_eq!(matched.offset, plain.offset);
        assert!(plain.offset[0].mean.abs() > 1.0);
    }

    #[test]
    fn a_changed_patch_is_the_first_hotspot() {
        let r = color_texture(128, 128);
        let mut m = r.clone();
        for y in 70..86 {
            for x in 20..36 {
                m.data[y * 128 + x] = [255.0, 0.0, 255.0];
            }
        }
        let a = analyse(&r, &all_valid(&m), Options::default()).unwrap();
        let top = a.hotspots[0].rect;
        assert!(top.x <= 36 && top.x + top.w >= 20 && top.y <= 86 && top.y + top.h >= 70);
        assert!(a.delta_e.max > 30.0);
    }

    #[test]
    fn the_inscribed_rectangle_leaves_out_the_missing_corner() {
        let (w, h) = (10, 8);
        let mut valid = vec![true; w * h];
        for y in 0..3 {
            for x in 0..2 {
                valid[y * w + x] = false;
            }
        }
        let r = inscribed(&valid, w, h).unwrap();
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                assert!(valid[y * w + x]);
            }
        }
        assert!(r.w * r.h >= 8 * 7 - 1, "{r:?}");
        assert_eq!(inscribed(&[false; 4], 2, 2), None);
    }

    #[test]
    fn darker_corners_lower_the_falloff() {
        let (w, h) = (100, 80);
        let flat = Plane::new(w, h, vec![200.0; w * h]);
        assert!((falloff(&flat).unwrap() - 1.0).abs() < 1e-9);
        let (cx, cy) = (49.5, 39.5);
        let dark = Plane::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let r = ((i % w) as f64 - cx).hypot((i / w) as f64 - cy) / 63.6;
                    (200.0 * (1.0 - 0.5 * r * r)) as f32
                })
                .collect(),
        );
        assert!(falloff(&dark).unwrap() < 0.75);
    }

    #[test]
    fn added_grain_raises_the_noise() {
        let r = color_texture(160, 160);
        let mut m = r.clone();
        for (i, p) in m.data.iter_mut().enumerate() {
            let d = if (i * 2_654_435_761) % 7 < 3 {
                6.0
            } else {
                -4.5
            };
            *p = p.map(|v| (v + d).clamp(0.0, 255.0));
        }
        let n = noise(&r.luma(), &m.luma()).unwrap();
        assert!(n.ratio() > 2.0, "{n:?}");
    }
}
