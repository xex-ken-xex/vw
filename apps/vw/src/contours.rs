//! 灰色の画像の等高線。画素の中心を格子の点にして、マーチングスクエアで線分を出す。
//!
//! 線は元の画像の値から引く（表示の 8bit に丸めた縮小画像では、弱い水準が段になって出るため）。

use vw_core::{ColorModel, PixelData, VwImage};

/// 線を引く格子の、長い辺の点の数の上限。大きな画像は間引いて引く（毎面の引き直しを軽くする）。
const MAX_GRID: usize = 2048;

/// 水準の決め方（設定 `contours.mode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// 中央値 + 雑音（σ）の倍数。
    #[default]
    Noise,
    /// 書いた値そのもの。
    Values,
}

/// 1 つの水準の線。
#[derive(Debug, Clone, PartialEq)]
pub struct Contour {
    pub level: f64,
    /// 雑音の倍数が負（または値が中央値より下）の水準。破線で描く。
    pub below: bool,
    /// 画像の画素の座標（左上の角が 0、画素の中心は +0.5）の線分。
    pub segments: Vec<[[f32; 2]; 2]>,
    /// 線分の両端が乗る格子の辺（両隅の画素の番号の組）。隣のマスと同じ点かを、座標の
    /// 丸めに頼らずに見分ける（隣のマスは隅を逆の順に見て点を求めるので、下の桁が食い違う）。
    edges: Vec<[(usize, usize); 2]>,
}

impl Contour {
    /// 端のつながる線分をつないだ折れ線。破線の模様を線分ごとに始め直さないため（1 マスの線分は
    /// 破線の 1 つより短く、つながないと実線に見える）と、描く図形の数を減らすため。
    pub fn polylines(&self) -> Vec<Vec<[f32; 2]>> {
        use std::collections::HashMap;
        let mut ends: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (i, e) in self.edges.iter().enumerate() {
            for k in e {
                ends.entry(*k).or_default().push(i);
            }
        }
        let mut used = vec![false; self.segments.len()];
        let mut out = Vec::new();
        for start in 0..self.segments.len() {
            if used[start] {
                continue;
            }
            used[start] = true;
            let mut line = vec![self.segments[start][0], self.segments[start][1]];
            let mut keys = vec![self.edges[start][0], self.edges[start][1]];
            // 後ろへ伸ばし、行き止まったら向きを変えて前へ伸ばす。
            for _ in 0..2 {
                loop {
                    let tail = *keys.last().expect("two points");
                    let next = ends
                        .get(&tail)
                        .and_then(|list| list.iter().copied().find(|&j| !used[j]));
                    let Some(j) = next else { break };
                    used[j] = true;
                    let far = usize::from(self.edges[j][0] == tail);
                    line.push(self.segments[j][far]);
                    keys.push(self.edges[j][far]);
                }
                line.reverse();
                keys.reverse();
            }
            out.push(line);
        }
        out
    }
}

/// 灰色の画像の値。灰色でなければ `None`。
pub fn gray_values(img: &VwImage) -> Option<Vec<f32>> {
    if img.color != ColorModel::Gray {
        return None;
    }
    Some(match &img.data {
        PixelData::U8(v) => v.iter().map(|&x| f32::from(x)).collect(),
        PixelData::U16(v) => v.iter().map(|&x| f32::from(x)).collect(),
        PixelData::F32(v) => v.clone(),
    })
}

/// 中央値と雑音（中央値からの外れの中央値 × 1.4826）。間引いて数える。値がなければ `None`。
pub fn noise(values: &[f32]) -> Option<(f64, f64)> {
    let step = values.len().div_ceil(200_000).max(1);
    let mut s: Vec<f32> = values
        .iter()
        .step_by(step)
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    if s.is_empty() {
        return None;
    }
    s.sort_by(f32::total_cmp);
    let median = f64::from(s[s.len() / 2]);
    let mut d: Vec<f64> = s.iter().map(|&v| (f64::from(v) - median).abs()).collect();
    d.sort_by(f64::total_cmp);
    Some((median, 1.4826 * d[d.len() / 2]))
}

/// 水準の値と、下側（破線）か。`Noise` で雑音が 0 の画像（一様な画像）は線を引かない。
pub fn levels(values: &[f32], mode: Mode, written: &[f64]) -> Vec<(f64, bool)> {
    match mode {
        Mode::Values => {
            let median = noise(values).map_or(0.0, |(m, _)| m);
            written.iter().map(|&v| (v, v < median)).collect()
        }
        Mode::Noise => match noise(values) {
            Some((median, sigma)) if sigma > 0.0 => written
                .iter()
                .map(|&k| (median + k * sigma, k < 0.0))
                .collect(),
            _ => Vec::new(),
        },
    }
}

/// `values`（`width` × 高さ）の等高線を、水準ごとに引く。
pub fn trace(values: &[f32], width: usize, levels: &[(f64, bool)]) -> Vec<Contour> {
    if width == 0 {
        return Vec::new();
    }
    let height = values.len() / width;
    let step = width.max(height).div_ceil(MAX_GRID).max(1);
    levels
        .iter()
        .map(|&(level, below)| {
            let (segments, edges) = squares(values, width, height, step, level as f32);
            Contour {
                level,
                below,
                segments,
                edges,
            }
        })
        .collect()
}

/// 格子の 1 マスごとに、4 隅の値が `level` をまたぐ辺の上の点を結ぶ。値のない隅があるマスは飛ばす。
fn squares(values: &[f32], width: usize, height: usize, step: usize, level: f32) -> Segments {
    let mut out = Vec::new();
    let mut edges = Vec::new();
    if width <= step || height <= step {
        return (out, edges);
    }
    let at = |x: usize, y: usize| values[y * width + x];
    let point = |x: usize, y: usize| [x as f32 + 0.5, y as f32 + 0.5];
    // 2 隅の間で、値が `level` になる所。
    let cross = |a: [f32; 2], va: f32, b: [f32; 2], vb: f32| {
        let t = if vb != va {
            (level - va) / (vb - va)
        } else {
            0.5
        };
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    };
    let mut y = 0;
    while y + step < height {
        let mut x = 0;
        while x + step < width {
            let (x1, y1) = (x + step, y + step);
            // 左上・右上・右下・左下の順。
            let corners = [(x, y), (x1, y), (x1, y1), (x, y1)];
            let v = corners.map(|(cx, cy)| at(cx, cy));
            if v.iter().all(|v| v.is_finite()) {
                let p = corners.map(|(cx, cy)| point(cx, cy));
                let case = v
                    .iter()
                    .enumerate()
                    .fold(0, |c, (i, &val)| c | (usize::from(val >= level) << i));
                // 辺 k は隅 k と隅 k+1 の間（上・右・下・左）。
                let edge = |k: usize| cross(p[k], v[k], p[(k + 1) % 4], v[(k + 1) % 4]);
                let pairs: &[(usize, usize)] = match case {
                    0 | 15 => &[],
                    1 | 14 => &[(3, 0)],
                    2 | 13 => &[(0, 1)],
                    3 | 12 => &[(3, 1)],
                    4 | 11 => &[(1, 2)],
                    6 | 9 => &[(0, 2)],
                    7 | 8 => &[(2, 3)],
                    // 対角の 2 隅だけが上: マスの中心の値で、上の 2 隅がつながるかを決める。
                    5 | 10 => {
                        let center = v.iter().sum::<f32>() / 4.0;
                        let joined = (center >= level) == (case == 5);
                        if joined {
                            &[(3, 2), (0, 1)]
                        } else {
                            &[(3, 0), (1, 2)]
                        }
                    }
                    _ => unreachable!("four corners make sixteen cases"),
                };
                let id = |k: usize| {
                    let (a, b) = (corners[k], corners[(k + 1) % 4]);
                    let (a, b) = (a.1 * width + a.0, b.1 * width + b.0);
                    (a.min(b), a.max(b))
                };
                for &(a, b) in pairs {
                    out.push([edge(a), edge(b)]);
                    edges.push([id(a), id(b)]);
                }
            }
            x += step;
        }
        y += step;
    }
    (out, edges)
}

/// 線分と、その両端の乗る格子の辺。
type Segments = (Vec<[[f32; 2]; 2]>, Vec<[(usize, usize); 2]>);

#[cfg(test)]
mod tests {
    use super::*;

    /// 中心からの距離の画像。
    fn cone(n: usize) -> Vec<f32> {
        let c = (n as f32 - 1.0) / 2.0;
        (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as f32, (i / n) as f32);
                ((x - c).powi(2) + (y - c).powi(2)).sqrt()
            })
            .collect()
    }

    #[test]
    fn a_contour_of_a_cone_is_a_circle_of_that_radius() {
        let n = 41;
        let lines = trace(&cone(n), n, &[(10.0, false)]);
        let centre = n as f32 / 2.0;
        assert!(lines[0].segments.len() > 40);
        for s in &lines[0].segments {
            for p in s {
                let r = ((p[0] - centre).powi(2) + (p[1] - centre).powi(2)).sqrt();
                assert!((r - 10.0).abs() < 0.1, "{p:?} at {r}");
            }
        }
    }

    #[test]
    fn the_pieces_of_a_closed_contour_join_into_one_ring() {
        let n = 41;
        let lines = trace(&cone(n), n, &[(10.0, false)]);
        let rings = lines[0].polylines();
        assert_eq!(rings.len(), 1);
        let ring = &rings[0];
        assert_eq!(ring.len(), lines[0].segments.len() + 1);
        let (a, b) = (ring[0], ring[ring.len() - 1]);
        assert!((a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3);
    }

    #[test]
    fn missing_pixels_leave_a_gap_instead_of_a_line() {
        let n = 21;
        let mut v = cone(n);
        let whole = trace(&v, n, &[(5.0, false)])[0].segments.len();
        for y in 0..n {
            v[y * n + 10] = f32::NAN;
        }
        let cut = trace(&v, n, &[(5.0, false)])[0].segments.len();
        assert!(cut < whole);
    }

    #[test]
    fn noise_levels_are_multiples_of_the_spread_above_the_median() {
        // 中央値 10、外れの中央値 1 → σ = 1.4826。
        let v: Vec<f32> = (0..1001).map(|i| [9.0, 10.0, 11.0][i % 3]).collect();
        let l = levels(&v, Mode::Noise, &[-3.0, 3.0]);
        assert!((l[0].0 - (10.0 - 3.0 * 1.4826)).abs() < 1e-6 && l[0].1);
        assert!((l[1].0 - (10.0 + 3.0 * 1.4826)).abs() < 1e-6 && !l[1].1);
    }

    #[test]
    fn a_flat_image_has_no_noise_contours_but_takes_written_values() {
        let v = vec![5.0f32; 100];
        assert!(levels(&v, Mode::Noise, &[3.0]).is_empty());
        assert_eq!(
            levels(&v, Mode::Values, &[4.0, 6.0]),
            vec![(4.0, true), (6.0, false)]
        );
    }

    #[test]
    fn large_images_are_traced_on_a_thinned_grid() {
        let (w, h) = (MAX_GRID * 2 + 2, 4);
        let v: Vec<f32> = (0..w * h).map(|i| (i % w) as f32).collect();
        // 縦の線 1 本。間引いた格子でも 1 マスに 1 本で、高さの分だけ出る。
        let lines = trace(&v, w, &[(1000.5, false)]);
        assert_eq!(lines[0].segments.len(), 1);
    }

    #[test]
    fn colour_images_have_no_contours() {
        let img = VwImage::new(1, 1, ColorModel::Rgb, PixelData::U8(vec![1, 2, 3])).unwrap();
        assert!(gray_values(&img).is_none());
    }
}
