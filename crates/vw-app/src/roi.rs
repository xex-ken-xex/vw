//! ROI（関心領域）。スロットに描いた矩形と、その中の画素の統計。描画と操作は [`crate::viewer`]。
//!
//! 値は十字の印と同じく、表示用の縮小画像ではなく元の画像から読む。

use std::path::PathBuf;

use vw_core::{ColorModel, Metadata, PixelData, VwImage};

use crate::histogram::{self, Accumulator, Stats};

/// 画像の中の矩形（画素の単位、`x..x+w` × `y..y+h`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Region {
    /// 2 つの角から作る。どちらの角が先でもよい。
    pub fn from_corners(a: (u32, u32), b: (u32, u32)) -> Self {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        Self {
            x: x0,
            y: y0,
            w: x1 - x0 + 1,
            h: y1 - y0 + 1,
        }
    }

    pub fn right(self) -> u32 {
        self.x + self.w
    }

    pub fn bottom(self) -> u32 {
        self.y + self.h
    }

    pub fn area(self) -> usize {
        self.w as usize * self.h as usize
    }

    /// 幅 `width` の画像の中で、この矩形に入る画素の添字。
    pub fn indices(self, width: u32) -> impl Iterator<Item = usize> {
        let (x, w, width) = (self.x as usize, self.w as usize, width as usize);
        (self.y..self.bottom())
            .map(move |row| row as usize * width + x)
            .flat_map(move |start| start..start + w)
    }
}

/// 対象と背景の 2 種類。SNR は対象だけ、CNR は両方から求める（後の品質評価のため）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Target,
    Background,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::Target => "Target",
            Kind::Background => "Background",
        }
    }
}

/// スロットに描いた ROI。置いた画像（フレームならフレームのパス）にだけ出す。
#[derive(Debug, Clone, PartialEq)]
pub struct Roi {
    pub path: PathBuf,
    pub kind: Kind,
    pub region: Region,
}

/// `region` の中の画素の統計。モノクロは画素の値、カラーは輝度（Rec. 709）。
///
/// MONOCHROME1 のモノクロは表示を揃えるために値を反転して持っている（DICOM のプラグイン）ので、
/// ここで符号を戻してファイルに書かれた値にする。
pub fn stats(img: &VwImage, meta: &Metadata, region: Region) -> Stats {
    let channels = img.color.channels();
    let sign = if inverted(meta) { -1.0 } else { 1.0 };
    let value = |i: usize| -> f64 {
        let at = i * channels;
        let c = |k: usize| -> f64 {
            match &img.data {
                PixelData::U8(v) => v[at + k].into(),
                PixelData::U16(v) => v[at + k].into(),
                PixelData::F32(v) => sign * f64::from(v[at + k]),
            }
        };
        match img.color {
            ColorModel::Gray | ColorModel::GrayAlpha => c(0),
            ColorModel::Rgb | ColorModel::Rgba => 0.2126 * c(0) + 0.7152 * c(1) + 0.0722 * c(2),
        }
    };
    let mut acc = Accumulator::default();
    for i in region.indices(img.width) {
        acc.add(value(i));
    }
    acc.stats()
}

fn inverted(meta: &Metadata) -> bool {
    meta.get("DICOM", "Photometric")
        .is_some_and(|p| p.trim() == "MONOCHROME1")
}

/// CT の値は HU（Rescale を当てた値）なので単位を付ける。ほかは単位のない生の値。
pub fn unit(meta: &Metadata) -> Option<&'static str> {
    meta.get("DICOM", "Modality")
        .is_some_and(|m| m.trim() == "CT")
        .then_some("HU")
}

/// 矩形の横に出す文字。統計がまだ読めない（元の画像を読み込み中）ときは大きさだけ。
pub fn label(kind: &str, region: Region, stats: Option<&Stats>, unit: Option<&str>) -> String {
    let size = format!("{kind}  {}×{}", region.w, region.h);
    match stats {
        Some(s) => format!(
            "{size}  mean {}{}  SD {}",
            histogram::number(s.mean),
            unit.map(|u| format!(" {u}")).unwrap_or_default(),
            histogram::number(s.std_dev)
        ),
        None => size,
    }
}

/// ROI をコピーしたときの文字。表計算に貼れるよう、カンマで区切る。
pub fn clipboard_text(kind: &str, region: Region, stats: Option<&Stats>) -> String {
    let head = format!(
        "{kind}, {}, {}, {}, {}",
        region.x, region.y, region.w, region.h
    );
    match stats {
        Some(s) => format!(
            "{head}, {}, {}, {}, {}, {}",
            region.area(),
            histogram::number(s.mean),
            histogram::number(s.std_dev),
            histogram::number(s.min),
            histogram::number(s.max)
        ),
        None => head,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(entries: &[(&str, &str)]) -> Metadata {
        let mut m = Metadata::default();
        for (k, v) in entries {
            m.push("DICOM", *k, *v);
        }
        m
    }

    #[test]
    fn corners_in_any_order_give_the_same_inclusive_region() {
        let a = Region::from_corners((5, 7), (2, 3));
        let b = Region::from_corners((2, 3), (5, 7));
        assert_eq!(a, b);
        assert_eq!((a.x, a.y, a.w, a.h), (2, 3, 4, 5));
        assert_eq!((a.right(), a.bottom(), a.area()), (6, 8, 20));
    }

    #[test]
    fn indices_walk_the_rows_inside_the_region() {
        let r = Region {
            x: 1,
            y: 1,
            w: 2,
            h: 2,
        };
        assert_eq!(r.indices(4).collect::<Vec<_>>(), vec![5, 6, 9, 10]);
    }

    #[test]
    fn gray_stats_use_the_pixel_values() {
        let img =
            VwImage::new(2, 2, ColorModel::Gray, PixelData::U16(vec![10, 20, 30, 40])).unwrap();
        let s = stats(
            &img,
            &meta(&[]),
            Region {
                x: 0,
                y: 0,
                w: 2,
                h: 1,
            },
        );
        assert_eq!((s.min, s.max, s.mean, s.std_dev), (10.0, 20.0, 15.0, 5.0));
    }

    #[test]
    fn monochrome1_values_are_read_back_unflipped() {
        let img = VwImage::new(1, 1, ColorModel::Gray, PixelData::F32(vec![-7.0])).unwrap();
        let s = stats(
            &img,
            &meta(&[("Photometric", "MONOCHROME1")]),
            Region {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
        );
        assert_eq!(s.mean, 7.0);
    }

    #[test]
    fn color_stats_use_luminance() {
        let img = VwImage::new(1, 1, ColorModel::Rgb, PixelData::U8(vec![255, 0, 0])).unwrap();
        let s = stats(
            &img,
            &meta(&[]),
            Region {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
        );
        assert!((s.mean - 54.21).abs() < 0.01);
    }

    #[test]
    fn label_and_clipboard_text_carry_the_unit_and_the_size() {
        let r = Region {
            x: 3,
            y: 4,
            w: 10,
            h: 20,
        };
        let s = Stats {
            min: -10.0,
            max: 40.0,
            mean: 12.5,
            std_dev: 3.0,
        };
        assert_eq!(
            label("Target", r, Some(&s), Some("HU")),
            "Target  10×20  mean 12.50 HU  SD 3"
        );
        assert_eq!(label("Target", r, None, None), "Target  10×20");
        assert_eq!(
            clipboard_text("Target", r, Some(&s)),
            "Target, 3, 4, 10, 20, 200, 12.50, 3, -10, 40"
        );
    }
}
