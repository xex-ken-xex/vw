//! 位置合わせ（大まかに合わせて、細かく直す）と、合わせた画像を基準の格子に写すこと。

use rayon::prelude::*;

use crate::affine::Affine;
use crate::coarse;
use crate::plane::{Plane, Rgb};
use crate::refine;

/// 合わなかったとみなす相関。これより低ければ、画面とレポートで「合っていないかもしれない」と出す。
pub const WEAK_CORRELATION: f64 = 0.5;
/// 合わなかったとみなす重なり（基準の面積に対する割合）。
pub const WEAK_OVERLAP: f64 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alignment {
    /// 基準の画素 → 比べる画像の画素。
    pub transform: Affine,
    pub correlation: f64,
    pub overlap: f64,
}

impl Alignment {
    /// 相関も重なりも足りている。
    pub fn looks_right(&self) -> bool {
        self.correlation >= WEAK_CORRELATION && self.overlap >= WEAK_OVERLAP
    }
}

/// `moving` を `reference` に合わせる。
pub fn align(reference: &Rgb, moving: &Rgb) -> Alignment {
    let r = reference.luma();
    let m = moving.luma();
    let centred = centred(&r, &m);
    let first = match coarse::estimate(&r, &m) {
        Some(c) => refine::refine(&r, &m, c.transform),
        None => refine::refine(&r, &m, centred),
    };
    let best = if first.correlation >= WEAK_CORRELATION {
        first
    } else {
        // 位相相関が外れたときに備え、中心を合わせただけの所からも直してみる。
        let second = refine::refine(&r, &m, centred);
        if second.correlation > first.correlation {
            second
        } else {
            first
        }
    };
    Alignment {
        transform: best.transform,
        correlation: best.correlation,
        overlap: best.overlap,
    }
}

/// 中心どうしを合わせ、面積の比で倍率を揃えただけの写し方。
pub fn centred(r: &Plane, m: &Plane) -> Affine {
    let (rw, rh) = (r.width as f64, r.height as f64);
    let (mw, mh) = (m.width as f64, m.height as f64);
    let scale = ((mw * mh) / (rw * rh)).sqrt();
    Affine::similarity(
        scale,
        0.0,
        [(rw - 1.0) / 2.0, (rh - 1.0) / 2.0],
        [(mw - 1.0) / 2.0, (mh - 1.0) / 2.0],
    )
}

/// 基準の格子に写した画像と、写った画素の印。
#[derive(Debug, Clone, PartialEq)]
pub struct Warped {
    pub image: Rgb,
    pub valid: Vec<bool>,
}

/// `moving` を、基準（元の大きさ `reference_size`）を倍率 `scale` で縮めた格子
/// （`width` × `height`）に写す。縮めて写すときは、先に比べる画像も縮めて折り返しを防ぐ。
pub fn warp(
    moving: &Rgb,
    transform: &Affine,
    reference_size: [usize; 2],
    width: usize,
    height: usize,
) -> Warped {
    let s = width as f64 / reference_size[0] as f64;
    let ratio = (transform.a * transform.e - transform.b * transform.d)
        .abs()
        .sqrt()
        .max(1e-6);
    let g = (s / ratio).min(1.0);
    let small;
    let (src, g) = if g < 0.999 {
        let w = ((moving.width as f64 * g).round() as usize).max(1);
        let h = ((moving.height as f64 * g).round() as usize).max(1);
        small = moving.resized(w, h);
        (&small, w as f64 / moving.width as f64)
    } else {
        (moving, 1.0)
    };
    let t = transform.between_scales(s, g);
    let rows: Vec<(Vec<[f32; 3]>, Vec<bool>)> = (0..height)
        .into_par_iter()
        .map(|y| {
            let mut px = Vec::with_capacity(width);
            let mut ok = Vec::with_capacity(width);
            for x in 0..width {
                let [sx, sy] = t.apply([x as f64, y as f64]);
                match src.sample(sx, sy) {
                    Some(p) => {
                        px.push(p);
                        ok.push(true);
                    }
                    None => {
                        px.push([0.0; 3]);
                        ok.push(false);
                    }
                }
            }
            (px, ok)
        })
        .collect();
    let mut data = Vec::with_capacity(width * height);
    let mut valid = Vec::with_capacity(width * height);
    for (p, v) in rows {
        data.extend(p);
        valid.extend(v);
    }
    Warped {
        image: Rgb {
            width,
            height,
            data,
        },
        valid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{color_texture, transformed};

    fn color_transformed(src: &Rgb, w: usize, h: usize, t: &Affine) -> Rgb {
        let planes = [0, 1, 2].map(|c| transformed(&src.channel(c), w, h, t));
        Rgb::from_channels([&planes[0], &planes[1], &planes[2]])
    }

    #[test]
    fn a_bigger_turned_copy_is_aligned_and_warped_back() {
        let r = color_texture(240, 180);
        let t = Affine::similarity(1.25, 3f64.to_radians(), [120.0, 90.0], [152.0, 110.0]);
        let m = color_transformed(&r, 300, 225, &t);
        let a = align(&r, &m);
        assert!(a.looks_right(), "{a:?}");
        let w = warp(&m, &a.transform, [240, 180], 240, 180);
        let i = 90 * 240 + 120;
        assert!(w.valid[i]);
        for c in 0..3 {
            assert!((w.image.data[i][c] - r.data[i][c]).abs() < 4.0);
        }
    }

    #[test]
    fn an_unrelated_image_is_reported_as_not_aligned() {
        let r = color_texture(240, 180);
        let flat = Rgb {
            width: 200,
            height: 150,
            data: (0..200 * 150)
                .map(|i| {
                    let v = ((i * 7919) % 251) as f32;
                    [v, v, v]
                })
                .collect(),
        };
        assert!(!align(&r, &flat).looks_right());
    }

    #[test]
    fn warping_to_a_smaller_grid_marks_what_is_outside() {
        let m = color_texture(100, 100);
        let shift = Affine {
            c: 50.0,
            ..Affine::IDENTITY
        };
        let w = warp(&m, &shift, [100, 100], 50, 50);
        assert_eq!(w.image.width, 50);
        assert!(w.valid[25 * 50 + 10]);
        assert!(!w.valid[25 * 50 + 40]);
    }
}
