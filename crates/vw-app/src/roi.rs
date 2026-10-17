//! ROI（関心領域）。スロットに描いた矩形と、その中の画素の統計。描画と操作は [`crate::viewer`]。
//!
//! 値は十字の印と同じく、表示用の縮小画像ではなく元の画像から読む。

use std::path::PathBuf;

use vw_core::{ColorModel, Metadata, PixelData, VwImage};

use crate::histogram::{self, Accumulator, Stats};

pub use vw_quality::Region;

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

/// カラー画像の `region` の中の、R・G・B それぞれの平均。モノクロや空の範囲では `None`。
pub fn color_means(img: &VwImage, region: Region) -> Option<[f64; 3]> {
    if !matches!(img.color, ColorModel::Rgb | ColorModel::Rgba) {
        return None;
    }
    let channels = img.color.channels();
    let mut sums = [0.0f64; 3];
    let mut n = 0usize;
    for i in region.indices(img.width) {
        let at = i * channels;
        for (k, sum) in sums.iter_mut().enumerate() {
            *sum += match &img.data {
                PixelData::U8(v) => f64::from(v[at + k]),
                PixelData::U16(v) => f64::from(v[at + k]),
                PixelData::F32(v) => f64::from(v[at + k]),
            };
        }
        n += 1;
    }
    (n > 0).then(|| sums.map(|s| s / n as f64))
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
/// `color` はカラー画像の平均の色（[`color_means`] を書いたもの）。あれば輝度の平均の代わりに出す。
pub fn label(
    kind: &str,
    region: Region,
    stats: Option<&Stats>,
    unit: Option<&str>,
    color: Option<&str>,
) -> String {
    let size = format!("{kind}  {}×{}", region.w, region.h);
    match stats {
        Some(s) => {
            let mean = match color {
                Some(c) => c.to_owned(),
                None => format!(
                    "{}{}",
                    histogram::number(s.mean),
                    unit.map(|u| format!(" {u}")).unwrap_or_default()
                ),
            };
            format!("{size}  mean {mean}  SD {}", histogram::number(s.std_dev))
        }
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
            label("Target", r, Some(&s), Some("HU"), None),
            "Target  10×20  mean 12.50 HU  SD 3"
        );
        assert_eq!(label("Target", r, None, None, None), "Target  10×20");
        assert_eq!(
            clipboard_text("Target", r, Some(&s)),
            "Target, 3, 4, 10, 20, 200, 12.50, 3, -10, 40"
        );
    }

    #[test]
    fn color_images_show_the_mean_of_each_channel() {
        let img = VwImage::new(
            2,
            1,
            ColorModel::Rgb,
            PixelData::U8(vec![10, 20, 30, 20, 40, 60]),
        )
        .unwrap();
        let r = Region {
            x: 0,
            y: 0,
            w: 2,
            h: 1,
        };
        assert_eq!(color_means(&img, r), Some([15.0, 30.0, 45.0]));
        let s = Stats {
            min: 0.0,
            max: 0.0,
            mean: 0.0,
            std_dev: 2.0,
        };
        assert_eq!(
            label("Target", r, Some(&s), None, Some("#0F1E2D")),
            "Target  2×1  mean #0F1E2D  SD 2"
        );
    }

    #[test]
    fn gray_images_have_no_color_mean() {
        let img = VwImage::new(1, 1, ColorModel::Gray, PixelData::U8(vec![5])).unwrap();
        let r = Region {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        };
        assert_eq!(color_means(&img, r), None);
    }
}
