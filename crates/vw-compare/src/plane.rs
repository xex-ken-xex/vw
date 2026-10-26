//! 画素の並び（灰色 1 面と、RGB）と、縮小・サンプリング。

use rayon::prelude::*;

/// 灰色の 1 面。行の順に `width * height` 個。
#[derive(Debug, Clone, PartialEq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

/// RGB の画像。値は 0〜255（表示の明るさ）。
#[derive(Debug, Clone, PartialEq)]
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub data: Vec<[f32; 3]>,
}

/// 表示の明るさ。画質の評価（vw の quality.rs）と同じ重み。
pub fn luma([r, g, b]: [f32; 3]) -> f32 {
    0.299 * r + 0.587 * g + 0.114 * b
}

impl Plane {
    pub fn new(width: usize, height: usize, data: Vec<f32>) -> Self {
        assert_eq!(data.len(), width * height);
        Self {
            width,
            height,
            data,
        }
    }

    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// 画素の中心を整数の座標とした位置の値（双一次補間）。画像の外は `None`。
    /// 端の半画素は端の画素の値にする。
    pub fn sample(&self, x: f64, y: f64) -> Option<f32> {
        sample(&self.data, self.width, self.height, x, y)
    }

    /// `width` × `height` に縮める（面積の平均）か、広げる（双一次補間）。
    pub fn resized(&self, width: usize, height: usize) -> Plane {
        Plane::new(
            width,
            height,
            resize(&self.data, self.width, self.height, width, height),
        )
    }
}

impl Rgb {
    /// RGBA8 の並びから（アルファは見ない）。
    pub fn from_rgba8(width: usize, height: usize, rgba: &[u8]) -> Self {
        assert_eq!(rgba.len(), width * height * 4);
        let data = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
            .collect();
        Self {
            width,
            height,
            data,
        }
    }

    /// RGBA8 の並びを、長辺 `long` 以下に縮めながら読む。元の大きさの浮動小数の並びを
    /// 作らないのは、2400 万画素で 300 MB 近くになるため。
    pub fn from_rgba8_fit(width: usize, height: usize, rgba: &[u8], long: usize) -> Self {
        assert_eq!(rgba.len(), width * height * 4);
        let (w, h, _) = fit_long_side(width, height, long);
        let ratio = width as f64 / w as f64;
        let rows: Vec<Vec<[f32; 3]>> = (0..height)
            .into_par_iter()
            .map(|y| {
                let line: Vec<[f32; 3]> = rgba[y * width * 4..(y + 1) * width * 4]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
                    .collect();
                (0..w).map(|i| resample(&line, i, ratio)).collect()
            })
            .collect();
        let mut columns = vec![[0.0f32; 3]; w * height];
        columns
            .par_chunks_mut(height)
            .enumerate()
            .for_each(|(i, column)| {
                for (y, v) in column.iter_mut().enumerate() {
                    *v = rows[y][i];
                }
            });
        drop(rows);
        Self {
            width: w,
            height: h,
            data: resize_axis(&columns, height, w, h),
        }
    }

    pub fn luma(&self) -> Plane {
        Plane::new(
            self.width,
            self.height,
            self.data.par_iter().map(|&p| luma(p)).collect(),
        )
    }

    pub fn channel(&self, c: usize) -> Plane {
        Plane::new(
            self.width,
            self.height,
            self.data.iter().map(|p| p[c]).collect(),
        )
    }

    pub fn from_channels(planes: [&Plane; 3]) -> Self {
        let (width, height) = (planes[0].width, planes[0].height);
        let data = (0..width * height)
            .map(|i| [planes[0].data[i], planes[1].data[i], planes[2].data[i]])
            .collect();
        Self {
            width,
            height,
            data,
        }
    }

    pub fn resized(&self, width: usize, height: usize) -> Rgb {
        Rgb {
            width,
            height,
            data: resize(&self.data, self.width, self.height, width, height),
        }
    }

    /// [`Plane::sample`] の 3 色版。
    pub fn sample(&self, x: f64, y: f64) -> Option<[f32; 3]> {
        sample(&self.data, self.width, self.height, x, y)
    }

    /// 0〜255 に丸めた RGBA8（不透明）。
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.data
            .iter()
            .flat_map(|p| {
                let [r, g, b] = p.map(|v| v.round().clamp(0.0, 255.0) as u8);
                [r, g, b, 255]
            })
            .collect()
    }
}

/// 長辺を `long` 以下にする大きさ（広げはしない）と、その倍率。
pub fn fit_long_side(width: usize, height: usize, long: usize) -> (usize, usize, f64) {
    let side = width.max(height);
    if side <= long {
        return (width, height, 1.0);
    }
    let scale = long as f64 / side as f64;
    let w = ((width as f64 * scale).round() as usize).max(1);
    let h = ((height as f64 * scale).round() as usize).max(1);
    (w, h, scale)
}

/// 灰色の値と 3 色の値を、同じ縮小と補間で扱うための足し算と掛け算。
pub trait Px: Copy + Send + Sync {
    const ZERO: Self;
    fn add(self, o: Self) -> Self;
    fn mul(self, k: f32) -> Self;
}

impl Px for f32 {
    const ZERO: f32 = 0.0;
    fn add(self, o: f32) -> f32 {
        self + o
    }
    fn mul(self, k: f32) -> f32 {
        self * k
    }
}

impl Px for [f32; 3] {
    const ZERO: [f32; 3] = [0.0; 3];
    fn add(self, o: [f32; 3]) -> [f32; 3] {
        [self[0] + o[0], self[1] + o[1], self[2] + o[2]]
    }
    fn mul(self, k: f32) -> [f32; 3] {
        self.map(|v| v * k)
    }
}

fn sample<T: Px>(data: &[T], width: usize, height: usize, x: f64, y: f64) -> Option<T> {
    let (w, h) = (width as f64, height as f64);
    if !(x >= -0.5 && y >= -0.5 && x <= w - 0.5 && y <= h - 0.5) {
        return None;
    }
    let x = x.clamp(0.0, w - 1.0);
    let y = y.clamp(0.0, h - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let at = |x: usize, y: usize| data[y * width + x];
    let top = at(x0, y0).mul(1.0 - fx).add(at(x1, y0).mul(fx));
    let bottom = at(x0, y1).mul(1.0 - fx).add(at(x1, y1).mul(fx));
    Some(top.mul(1.0 - fy).add(bottom.mul(fy)))
}

fn resize<T: Px>(data: &[T], from_w: usize, from_h: usize, w: usize, h: usize) -> Vec<T> {
    // 1 回目で横を縮めて転置した並びになり、2 回目で縦を縮めて元の向きに戻る。
    let columns = resize_axis(data, from_w, from_h, w);
    resize_axis(&columns, from_h, w, h)
}

/// 行ごとに長さ `from` を `to` にする（`rows` 行）。結果は転置した並び（`to` 行 × `rows` 列）。
/// 縦横を同じ関数で縮めるため、1 回ごとに転置しておく。
fn resize_axis<T: Px>(data: &[T], from: usize, rows: usize, to: usize) -> Vec<T> {
    let ratio = from as f64 / to as f64;
    let lines: Vec<Vec<T>> = (0..rows)
        .into_par_iter()
        .map(|r| {
            let line = &data[r * from..(r + 1) * from];
            (0..to).map(|i| resample(line, i, ratio)).collect()
        })
        .collect();
    let mut out = vec![T::ZERO; to * rows];
    out.par_chunks_mut(rows)
        .enumerate()
        .for_each(|(i, column)| {
            for (r, v) in column.iter_mut().enumerate() {
                *v = lines[r][i];
            }
        });
    out
}

/// 長さ `line.len()` の並びを `ratio` 分の 1 にしたときの、`i` 番目の値。
fn resample<T: Px>(line: &[T], i: usize, ratio: f64) -> T {
    let from = line.len();
    if ratio > 1.0 {
        // 縮める: 元の [a, b) の面積の平均。
        let a = i as f64 * ratio;
        let b = a + ratio;
        let mut sum = T::ZERO;
        let mut k = a.floor() as usize;
        while (k as f64) < b && k < from {
            let lo = a.max(k as f64);
            let hi = b.min(k as f64 + 1.0);
            sum = sum.add(line[k].mul(((hi - lo) / ratio) as f32));
            k += 1;
        }
        sum
    } else {
        // 広げる: 画素の中心どうしの双一次補間。
        let x = ((i as f64 + 0.5) * ratio - 0.5).clamp(0.0, (from - 1) as f64);
        let x0 = x.floor() as usize;
        let x1 = (x0 + 1).min(from - 1);
        let f = (x - x0 as f64) as f32;
        line[x0].mul(1.0 - f).add(line[x1].mul(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinking_averages_the_covered_pixels() {
        let p = Plane::new(4, 2, vec![0.0, 2.0, 4.0, 6.0, 10.0, 12.0, 14.0, 16.0]);
        let s = p.resized(2, 1);
        assert_eq!((s.width, s.height), (2, 1));
        assert_eq!(s.data, [6.0, 10.0]);
        let third = Plane::new(3, 1, vec![0.0, 3.0, 6.0]).resized(2, 1);
        assert_eq!(third.data, [1.0, 5.0]);
    }

    #[test]
    fn widening_interpolates_between_pixel_centres() {
        let p = Plane::new(2, 1, vec![0.0, 10.0]).resized(4, 1);
        assert_eq!(p.data, [0.0, 2.5, 7.5, 10.0]);
    }

    #[test]
    fn sampling_is_bilinear_inside_and_none_outside() {
        let p = Plane::new(2, 2, vec![0.0, 10.0, 20.0, 30.0]);
        assert_eq!(p.sample(0.5, 0.5), Some(15.0));
        assert_eq!(p.sample(-0.4, 0.0), Some(0.0));
        assert_eq!(p.sample(-0.6, 0.0), None);
        assert_eq!(p.sample(0.0, 1.6), None);
    }

    #[test]
    fn reading_and_shrinking_at_once_matches_shrinking_after_reading() {
        let rgba: Vec<u8> = (0..40 * 30 * 4).map(|i| (i * 37 % 256) as u8).collect();
        let a = Rgb::from_rgba8_fit(40, 30, &rgba, 16);
        let b = Rgb::from_rgba8(40, 30, &rgba).resized(16, 12);
        assert_eq!((a.width, a.height), (16, 12));
        for (p, q) in a.data.iter().zip(&b.data) {
            for c in 0..3 {
                assert!((p[c] - q[c]).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn the_long_side_is_fitted_without_widening() {
        assert_eq!(fit_long_side(400, 300, 1000), (400, 300, 1.0));
        assert_eq!(fit_long_side(4000, 3000, 1000), (1000, 750, 0.25));
    }
}
