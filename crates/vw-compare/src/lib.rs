//! 似た画像（同じ写真の RAW と JPEG など）を位置合わせし、違いを数で表す。
//!
//! GUI には依存しない。vw の比べる画面と、あとで vwbatch から同じものを使うため。

pub mod affine;
pub mod align;
pub mod analysis;
pub mod coarse;
pub mod color;
pub mod pairs;
pub mod plane;
pub mod refine;
pub mod render;
pub mod report;

#[cfg(test)]
mod test_util;

pub use affine::{Affine, Parts};
pub use align::{Alignment, Warped, align, warp};
pub use analysis::{Analysis, Options, Rect, analyse};
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
    finish(reference, moving, small, options)
}

/// 決まった写し方 `transform`（元の大きさどうし、基準 → 比べる画像）で比べる。手で直した
/// 写し方を使うとき。相関と重なりは、その写し方のまま測る。
pub fn compare_at(
    reference: &Prepared,
    moving: &Prepared,
    transform: Affine,
    options: Options,
) -> Compared {
    let small = transform.between_scales(reference.scale(), moving.scale());
    let (correlation, overlap) =
        refine::score(&reference.image.luma(), &moving.image.luma(), &small);
    let small = Alignment {
        transform: small,
        correlation,
        overlap,
    };
    finish(reference, moving, small, options)
}

/// 縮めた画像どうしの位置合わせ `small` から、写した画像と分析を作る。
fn finish(reference: &Prepared, moving: &Prepared, small: Alignment, options: Options) -> Compared {
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

    #[test]
    fn a_given_transform_is_used_as_is_and_scored() {
        let big = test_util::color_texture(240, 180);
        let small = big.resized(120, 90);
        let r = Prepared::new(240, 180, &big.to_rgba8());
        let m = Prepared::new(120, 90, &small.to_rgba8());
        let right = compare(&r, &m, Options::default()).alignment.transform;
        let at = compare_at(&r, &m, right, Options::default());
        assert!(at.alignment.looks_right(), "{:?}", at.alignment);
        let off = Affine {
            c: right.c + 20.0,
            ..right
        };
        let wrong = compare_at(&r, &m, off, Options::default());
        assert!(
            wrong.alignment.correlation < at.alignment.correlation - 0.2,
            "{:?}",
            wrong.alignment
        );
    }
}
