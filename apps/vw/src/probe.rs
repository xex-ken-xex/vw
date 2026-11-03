//! 画素値の確認（十字の印とカーソル位置の値）。画素を読んで文字にするところだけを受け持ち、
//! 描画と操作は [`crate::viewer`] にある。
//!
//! 値は表示用の縮小画像ではなく元の画像から読む。縮小画像は表示の階調に丸めた 8bit で、
//! 元の値（16bit の生の値や CT の HU）が残っていないため。

use std::path::PathBuf;

use vw_core::geo::GeoGrid;
use vw_core::sky::{SkyFormat, SkyMap};
use vw_core::{ColorModel, Metadata, PixelData, VwImage};

use crate::quantity::Quantity;
use crate::settings::ColorFormat;

/// スロットに置いた十字の印。置いた画像（フレームならフレームのパス）にだけ出す。
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub path: PathBuf,
    pub x: u32,
    pub y: u32,
}

/// 画素 `(x, y)` の値をチャンネルごとに返す。画像の外なら `None`。
///
/// MONOCHROME1 のモノクロは、表示を揃えるために値を反転して持っている（DICOM のプラグイン）ので、
/// ここで符号を戻してファイルに書かれた値にする。
pub fn sample(img: &VwImage, meta: &Metadata, x: u32, y: u32) -> Option<Vec<f64>> {
    if x >= img.width || y >= img.height {
        return None;
    }
    let channels = img.color.channels();
    let at = (y as usize * img.width as usize + x as usize) * channels;
    let values: Vec<f64> = match &img.data {
        PixelData::U8(v) => v
            .get(at..at + channels)?
            .iter()
            .map(|&s| s.into())
            .collect(),
        PixelData::U16(v) => v
            .get(at..at + channels)?
            .iter()
            .map(|&s| s.into())
            .collect(),
        PixelData::F32(v) => {
            let sign = if inverted(meta) { -1.0 } else { 1.0 };
            v.get(at..at + channels)?
                .iter()
                .map(|&s| sign * f64::from(s))
                .collect()
        }
    };
    Some(values)
}

fn inverted(meta: &Metadata) -> bool {
    meta.get("DICOM", "Photometric")
        .is_some_and(|p| p.trim() == "MONOCHROME1")
}

/// 欠損の画素（FITS の BLANK や NaN）の値の代わりに出す文字。画面では訳して出す。
pub const MISSING: &str = "missing";

/// 画素の値を画面とコピーに出す文字にする。カラーは `format` の書き方にする（`bits` は 1 色のビット数）。
pub fn value_text(
    color: ColorModel,
    meta: &Metadata,
    values: &[f64],
    format: ColorFormat,
    bits: u32,
) -> String {
    let n = |i: usize| {
        values
            .get(i)
            .copied()
            .map(crate::histogram::number)
            .unwrap_or_default()
    };
    match color {
        ColorModel::Gray => match values.first() {
            Some(v) if v.is_nan() => MISSING.to_owned(),
            Some(&v) => Quantity::of(meta).text(v),
            None => String::new(),
        },
        ColorModel::GrayAlpha => format!("{} A {}", n(0), n(1)),
        ColorModel::Rgb | ColorModel::Rgba => crate::color_text::text(values, bits, format),
    }
}

/// 画素の位置（地球の上か空の上）と、面（時刻・高さ）の札。
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub at: Where,
    pub plane: Option<String>,
}

/// 画素の位置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Where {
    /// 気象・海洋の格子のデータの緯度・経度（度）。
    Earth { lat: f64, lon: f64 },
    /// WCS のある天体の画像の赤経・赤緯（度）と、その書き方。銀河座標で書かれた画像は、
    /// 銀河経度・銀河緯度（度）も。
    Sky {
        ra: f64,
        dec: f64,
        format: SkyFormat,
        galactic: Option<[f64; 2]>,
    },
}

impl Place {
    /// 画素 `(x, y)` の位置。緯度・経度も空の座標も分からない画像や、格子・投影の外なら `None`。
    pub fn at(
        geo: Option<&GeoGrid>,
        sky: Option<&dyn SkyMap>,
        format: SkyFormat,
        meta: &Metadata,
        x: u32,
        y: u32,
    ) -> Option<Self> {
        let (px, py) = (f64::from(x), f64::from(y));
        let at = match (geo, sky) {
            (Some(geo), _) => {
                let (lat, lon) = geo.at(px, py)?;
                Where::Earth { lat, lon }
            }
            (None, Some(sky)) => {
                let [ra, dec] = sky.at(px, py)?;
                Where::Sky {
                    ra,
                    dec,
                    format,
                    galactic: sky.galactic(px, py),
                }
            }
            (None, None) => return None,
        };
        let plane = meta
            .get(vw_netcdf::GROUP, vw_fits::PLANE)
            .or_else(|| meta.get(vw_fits::GROUP, vw_fits::PLANE))
            .map(str::to_owned);
        Some(Self { at, plane })
    }

    /// 位置と面の札を 1 行にした文字。
    pub fn label(&self) -> String {
        let at = match self.at {
            Where::Earth { lat, lon } => vw_core::geo::format_lat_lon(lat, lon),
            Where::Sky {
                ra,
                dec,
                format,
                galactic,
            } => {
                let radec = vw_core::sky::format_radec(ra, dec, format);
                // 画像の座標系（銀河座標）を先に、星図やほかの画像と照らすための赤経・赤緯を後に。
                match galactic {
                    Some([l, b]) => format!("{}  ({radec})", vw_core::sky::format_galactic(l, b)),
                    None => radec,
                }
            }
        };
        match &self.plane {
            Some(plane) => format!("{at}  {plane}"),
            None => at,
        }
    }

    /// 計算に使える、符号付きの度の小数（緯度・経度、または赤経・赤緯と、あれば銀河経度・銀河緯度）。
    fn degrees(&self) -> Vec<f64> {
        match self.at {
            Where::Earth { lat, lon } => vec![lat, lon],
            Where::Sky {
                ra, dec, galactic, ..
            } => {
                let mut v = vec![ra.rem_euclid(360.0), dec];
                if let Some([l, b]) = galactic {
                    v.extend([l.rem_euclid(360.0), b]);
                }
                v
            }
        }
    }
}

/// 十字の横に出す文字。値がまだ読めない（元の画像を読み込み中）ときは位置だけ。
pub fn mark_label(x: u32, y: u32, value: Option<&str>, place: Option<&Place>) -> String {
    let at = match value {
        Some(v) => format!("{v}  (x {x}, y {y})"),
        None => format!("(x {x}, y {y})"),
    };
    match place {
        Some(place) => format!("{at}\n{}", place.label()),
        None => at,
    }
}

/// 十字をコピーしたときの文字。表計算に貼れるよう、カンマで区切る。
/// 緯度・経度と赤経・赤緯は、計算に使えるよう度の小数で書く（南・西・南の赤緯は負）。
pub fn clipboard_text(x: u32, y: u32, value: Option<&str>, place: Option<&Place>) -> String {
    let mut text = match value {
        Some(v) => format!("{x}, {y}, {v}"),
        None => format!("{x}, {y}"),
    };
    if let Some(place) = place {
        for v in place.degrees() {
            text += &format!(", {v:.6}");
        }
        if let Some(plane) = &place.plane {
            text += &format!(", {plane}");
        }
    }
    text
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

    fn gray_f32(values: Vec<f32>) -> VwImage {
        VwImage::new(2, 1, ColorModel::Gray, PixelData::F32(values)).unwrap()
    }

    #[test]
    fn reads_the_pixel_at_the_given_position() {
        let img = VwImage::new(
            2,
            2,
            ColorModel::Gray,
            PixelData::U16(vec![10, 20, 30, 4000]),
        )
        .unwrap();
        assert_eq!(sample(&img, &meta(&[]), 1, 1), Some(vec![4000.0]));
        assert_eq!(sample(&img, &meta(&[]), 0, 1), Some(vec![30.0]));
    }

    #[test]
    fn outside_the_image_has_no_value() {
        let img = gray_f32(vec![1.0, 2.0]);
        assert_eq!(sample(&img, &meta(&[]), 2, 0), None);
        assert_eq!(sample(&img, &meta(&[]), 0, 1), None);
    }

    #[test]
    fn color_pixels_give_one_value_per_channel() {
        let img = VwImage::new(1, 1, ColorModel::Rgb, PixelData::U8(vec![12, 34, 56])).unwrap();
        let values = sample(&img, &meta(&[]), 0, 0).unwrap();
        assert_eq!(values, vec![12.0, 34.0, 56.0]);
        assert_eq!(
            value_text(ColorModel::Rgb, &meta(&[]), &values, ColorFormat::Rgb, 16),
            "R 12 G 34 B 56"
        );
        assert_eq!(
            value_text(ColorModel::Rgb, &meta(&[]), &values, ColorFormat::Hex, 8),
            "#0C2238"
        );
    }

    #[test]
    fn ct_values_are_shown_in_hu() {
        let m = meta(&[("Modality", "CT")]);
        let img = gray_f32(vec![-1000.0, -35.0]);
        let values = sample(&img, &m, 1, 0).unwrap();
        assert_eq!(
            value_text(ColorModel::Gray, &m, &values, ColorFormat::Rgb, 16),
            "-35 HU"
        );
    }

    #[test]
    fn non_ct_values_have_no_unit() {
        let m = meta(&[("Modality", "MR")]);
        assert_eq!(
            value_text(ColorModel::Gray, &m, &[812.0], ColorFormat::Rgb, 16),
            "812"
        );
    }

    #[test]
    fn monochrome1_values_are_read_back_with_their_stored_sign() {
        let m = meta(&[("Photometric", "MONOCHROME1")]);
        let img = gray_f32(vec![-120.0, 0.0]);
        assert_eq!(sample(&img, &m, 0, 0), Some(vec![120.0]));
    }

    #[test]
    fn fractional_values_keep_two_decimals() {
        assert_eq!(
            value_text(ColorModel::Gray, &meta(&[]), &[0.126], ColorFormat::Rgb, 16),
            "0.13"
        );
        assert_eq!(
            value_text(ColorModel::Gray, &meta(&[]), &[7.0], ColorFormat::Rgb, 16),
            "7"
        );
    }

    #[test]
    fn missing_pixels_say_so_instead_of_a_number() {
        assert_eq!(
            value_text(
                ColorModel::Gray,
                &meta(&[]),
                &[f64::NAN],
                ColorFormat::Rgb,
                32
            ),
            MISSING
        );
    }

    #[test]
    fn label_shows_value_and_position() {
        assert_eq!(
            mark_label(120, 88, Some("-35 HU"), None),
            "-35 HU  (x 120, y 88)"
        );
        assert_eq!(mark_label(120, 88, None, None), "(x 120, y 88)");
    }

    #[test]
    fn copied_text_is_comma_separated() {
        assert_eq!(
            clipboard_text(120, 88, Some("-35 HU"), None),
            "120, 88, -35 HU"
        );
    }

    fn tokyo() -> Place {
        Place {
            at: Where::Earth {
                lat: 35.68,
                lon: 139.77,
            },
            plane: Some("time = 2020-01-01".to_owned()),
        }
    }

    #[test]
    fn label_adds_latitude_longitude_and_plane_on_a_second_line() {
        assert_eq!(
            mark_label(3, 4, Some("1.5"), Some(&tokyo())),
            "1.5  (x 3, y 4)\n35.68°N 139.77°E  time = 2020-01-01"
        );
    }

    #[test]
    fn copied_text_adds_signed_latitude_longitude_and_plane() {
        let place = Place {
            at: Where::Earth {
                lat: -12.5,
                lon: -45.25,
            },
            plane: None,
        };
        assert_eq!(
            clipboard_text(3, 4, Some("1.5"), Some(&place)),
            "3, 4, 1.5, -12.500000, -45.250000"
        );
        assert_eq!(
            clipboard_text(3, 4, None, Some(&tokyo())),
            "3, 4, 35.680000, 139.770000, time = 2020-01-01"
        );
    }

    #[test]
    fn place_is_read_from_the_grid_and_the_plane_label() {
        let grid = GeoGrid {
            width: 2,
            height: 2,
            cells: vw_core::geo::Cells::Axes {
                longitudes: vec![10.0, 20.0],
                latitudes: vec![50.0, 40.0],
            },
        };
        let m = meta(&[]);
        let mut with_plane = Metadata::default();
        with_plane.push(vw_netcdf::GROUP, vw_fits::PLANE, "level = 500 hPa");
        let at = |grid, meta, x, y| Place::at(grid, None, SkyFormat::Degrees, meta, x, y);
        assert_eq!(
            at(Some(&grid), &with_plane, 1, 1),
            Some(Place {
                at: Where::Earth {
                    lat: 40.0,
                    lon: 20.0
                },
                plane: Some("level = 500 hPa".to_owned())
            })
        );
        assert_eq!(
            at(Some(&grid), &m, 1, 0).map(|p| p.degrees()[0]),
            Some(50.0)
        );
        assert_eq!(at(Some(&grid), &m, 5, 0), None);
        assert_eq!(at(None, &m, 0, 0), None);
    }

    /// 1 画素が 1″ で、(0, 0) が赤経 83.82°・赤緯 -5.39° の空。
    #[derive(Debug)]
    struct Orion;

    impl SkyMap for Orion {
        fn at(&self, x: f64, y: f64) -> Option<[f64; 2]> {
            (x < 10.0).then(|| [83.82 - x / 3600.0, -5.39 - y / 3600.0])
        }
    }

    #[test]
    fn sky_images_show_right_ascension_and_declination() {
        let m = meta(&[]);
        let place = Place::at(None, Some(&Orion), SkyFormat::Sexagesimal, &m, 0, 0).unwrap();
        assert_eq!(
            mark_label(0, 0, Some("1.5"), Some(&place)),
            "1.5  (x 0, y 0)\nα 05h35m16.80s  δ -05°23′24.0″"
        );
        assert_eq!(
            clipboard_text(0, 0, Some("1.5"), Some(&place)),
            "0, 0, 1.5, 83.820000, -5.390000"
        );
        assert_eq!(
            Place::at(None, Some(&Orion), SkyFormat::Degrees, &m, 0, 0)
                .unwrap()
                .label(),
            "α 83.820000°  δ -5.390000°"
        );
    }

    /// (0, 0) が銀河中心の、銀河座標の画像。
    #[derive(Debug)]
    struct GalacticCentre;

    impl SkyMap for GalacticCentre {
        fn at(&self, _x: f64, _y: f64) -> Option<[f64; 2]> {
            Some([266.404_988, -28.936_178])
        }

        fn galactic(&self, _x: f64, _y: f64) -> Option<[f64; 2]> {
            Some([0.0, 0.0])
        }
    }

    #[test]
    fn galactic_images_show_their_own_coordinates_first() {
        let m = meta(&[]);
        let place = Place::at(
            None,
            Some(&GalacticCentre),
            SkyFormat::Sexagesimal,
            &m,
            0,
            0,
        )
        .unwrap();
        assert_eq!(
            place.label(),
            "l 0.000000°  b +0.000000°  (α 17h45m37.20s  δ -28°56′10.2″)"
        );
        assert_eq!(
            clipboard_text(0, 0, None, Some(&place)),
            "0, 0, 266.404988, -28.936178, 0.000000, 0.000000"
        );
    }

    #[test]
    fn outside_the_projection_has_no_sky_position() {
        let m = meta(&[]);
        assert_eq!(
            Place::at(None, Some(&Orion), SkyFormat::Degrees, &m, 20, 0),
            None
        );
    }

    #[test]
    fn cube_planes_of_sky_images_follow_the_position() {
        let mut m = Metadata::default();
        m.push(vw_fits::GROUP, vw_fits::PLANE, "FREQ = 1.420 GHz");
        let place = Place::at(None, Some(&Orion), SkyFormat::Degrees, &m, 0, 0).unwrap();
        assert_eq!(
            place.label(),
            "α 83.820000°  δ -5.390000°  FREQ = 1.420 GHz"
        );
    }
}
