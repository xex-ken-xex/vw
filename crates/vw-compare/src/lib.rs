//! 似た画像（同じ写真の RAW と JPEG など）を位置合わせし、違いを数で表す。
//!
//! GUI には依存しない。vw の比べる画面と、あとで vwbatch から同じものを使うため。

pub mod affine;
pub mod align;
pub mod analysis;
pub mod coarse;
pub mod color;
pub mod plane;
pub mod refine;
pub mod render;
pub mod report;

#[cfg(test)]
mod test_util;

pub use affine::{Affine, Parts};
pub use align::{align, warp, Alignment, Warped};
pub use analysis::{analyse, Analysis, Options, Rect};
pub use plane::{Plane, Rgb};

/// 違いを測る格子の長辺。元の大きさで測らないのは、2400 万画素の画像を何枚も
/// 浮動小数で持つとメモリを食い、計算も遅くなるため。鮮鋭度とノイズは 2 枚の比で見るので、
/// 同じ格子で測れば比べられる。位置合わせもこの大きさで行う（細かく直すのは長辺 2048 まで）。
pub const ANALYSIS_LONG_SIDE: usize = 3072;

/// 長辺 [`ANALYSIS_LONG_SIDE`] 以下に縮めた画像と、元の大きさ。
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub image: Rgb,
    pub size: [usize; 2],
}

impl Prepared {
    /// RGBA8 の画像から。
    pub fn new(width: usize, height: usize, rgba: &[u8]) -> Self {
        Self {
            image: Rgb::from_rgba8_fit(width, height, rgba, ANALYSIS_LONG_SIDE),
            size: [width, height],
        }
    }

    /// 縮めた画素 / 元の画素。
    pub fn scale(&self) -> f64 {
        self.image.width as f64 / self.size[0] as f64
    }
}

/// 比べた 1 枚。
#[derive(Debug, Clone, PartialEq)]
pub struct Compared {
    /// 元の大きさどうしの写し方（基準の画素 → この画像の画素）。
    pub alignment: Alignment,
    /// 基準の格子（縮めた基準）に写したもの。
    pub warped: Warped,
    /// 重ならなければ `None`。
    pub analysis: Option<Analysis>,
}

/// `moving` を `reference` に合わせて比べる。
pub fn compare(reference: &Prepared, moving: &Prepared, options: Options) -> Compared {
    let small = align(&reference.image, &moving.image);
    let (sr, sm) = (reference.scale(), moving.scale());
    let alignment = Alignment {
        transform: small.transform.between_scales(1.0 / sr, 1.0 / sm),
        ..small
    };
    let grid = &reference.image;
    let warped = warp(
        &moving.image,
        &small.transform,
        [grid.width, grid.height],
        grid.width,
        grid.height,
    );
    let analysis = analyse(grid, &warped, options);
    Compared {
        alignment,
        warped,
        analysis,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_size_copy_is_found_at_twice_the_scale_in_original_pixels() {
        let big = test_util::color_texture(240, 180);
        let rgba = |img: &Rgb| img.to_rgba8();
        let small = big.resized(120, 90);
        let r = Prepared::new(240, 180, &rgba(&big));
        let m = Prepared::new(120, 90, &rgba(&small));
        let c = compare(&r, &m, Options::default());
        assert!(c.alignment.looks_right(), "{:?}", c.alignment);
        let p = c.alignment.transform.parts([240.0, 180.0], [120.0, 90.0]);
        assert!((p.scale - 0.5).abs() < 0.01, "{p:?}");
        assert!(p.shift[0].abs() < 0.5 && p.shift[1].abs() < 0.5, "{p:?}");
        let a = c.analysis.unwrap();
        assert!(a.ssim.unwrap() > 0.8, "{:?}", a.ssim);
    }
}
