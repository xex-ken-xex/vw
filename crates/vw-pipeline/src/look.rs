//! 灰色の画像の見た目（黒と白の決め方、その間の曲線、色の表）。ビューアではスロットごとの値で、
//! パイプラインのファイルに入れると、まとめてかけたときにもスロットで見ていたのと同じ見た目になる。

use serde::{Deserialize, Serialize};
use vw_core::{ColorModel, Curve, Palette, PixelData, VwImage};
use vw_fits::stretch::{self, Stretch};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    #[serde(skip_serializing_if = "BlackWhite::is_file")]
    pub black_white: BlackWhite,
    /// `None` はファイル（と設定）のまま。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curve: Option<CurveName>,
    /// 色の表の名前。`"off"` は色の表を当てない。`None` はかける側の既定のまま。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_map: Option<String>,
    /// 組み込みでない（設定で足した）色の表の色の点「位置 #rrggbb」。ほかの PC の設定に同じ表が
    /// なくても同じ色になるよう、名前と一緒に書く。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub color_map_stops: Vec<String>,
}

/// 色の表を当てないことを表す [`Look::color_map`] の値。
pub const COLOR_MAP_OFF: &str = "off";

impl Look {
    /// 何も決めていない（かける側の既定のまま）。
    pub fn is_empty(&self) -> bool {
        *self == Look::default()
    }

    /// 灰色の `image` に黒と白と曲線を当てる。戻り値は当てた黒と白（DICOM のファイルの段にも渡す）。
    /// 灰色でない画像と `File` のままなら何もしない。
    pub fn apply(&self, image: &mut VwImage) -> Option<(f32, f32)> {
        if !matches!(image.color, ColorModel::Gray | ColorModel::GrayAlpha) {
            return None;
        }
        if let Some(c) = self.curve {
            image.curve = c.into();
        }
        let window = self.black_white.window(&image.data)?;
        image.window = Some(window);
        Some(window)
    }

    /// 当てる色の表。`None` は灰色のまま（決めていない、`off`、灰色の表）。
    pub fn palette(&self) -> Result<Option<Palette>, String> {
        let Some(name) = self.color_map.as_deref().filter(|n| *n != COLOR_MAP_OFF) else {
            return Ok(None);
        };
        let palette = match Palette::builtin(name) {
            Some(p) => p,
            None if self.color_map_stops.is_empty() => {
                return Err(format!(
                    "color map \"{name}\" is not built in and has no colors"
                ))
            }
            None => {
                let stops = self
                    .color_map_stops
                    .iter()
                    .map(|s| vw_core::palette::parse_stop(s))
                    .collect::<Result<Vec<_>, _>>()?;
                Palette::from_stops(name, &stops)?
            }
        };
        Ok((!palette.is_gray()).then_some(palette))
    }
}

/// 黒と白の決め方。値はファイルの値の単位。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum BlackWhite {
    /// 読み込んだときの値（形式と設定の決め方）のまま。
    #[default]
    File,
    /// 値の小さいほうから `black`％ を黒、`white`％ を白にする。
    Percentile {
        black: f32,
        white: f32,
    },
    Zscale,
    MinMax,
    /// 決まった値。
    Values {
        black: f32,
        white: f32,
    },
}

impl BlackWhite {
    pub fn is_file(&self) -> bool {
        *self == BlackWhite::File
    }

    /// `data` に当てる黒と白。`File` と、値が 1 つもない画像は `None`（画像の値のまま）。
    pub fn window(&self, data: &PixelData) -> Option<(f32, f32)> {
        match *self {
            BlackWhite::File => None,
            BlackWhite::Percentile { black, white } => {
                stretch::window(data, Stretch::Percentile, black, white)
            }
            BlackWhite::Zscale => stretch::window(data, Stretch::ZScale, 0.0, 0.0),
            BlackWhite::MinMax => stretch::window(data, Stretch::MinMax, 0.0, 0.0),
            BlackWhite::Values { black, white } => Some((black, white)),
        }
    }
}

/// ファイルに書く曲線の名前（[`Curve`] は画像のクレートにあり、書き方を持たないため）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveName {
    Linear,
    Sqrt,
    Log,
    Asinh,
}

impl From<CurveName> for Curve {
    fn from(c: CurveName) -> Self {
        match c {
            CurveName::Linear => Curve::Linear,
            CurveName::Sqrt => Curve::Sqrt,
            CurveName::Log => Curve::Log,
            CurveName::Asinh => Curve::Asinh,
        }
    }
}

impl From<Curve> for CurveName {
    fn from(c: Curve) -> Self {
        match c {
            Curve::Linear => CurveName::Linear,
            Curve::Sqrt => CurveName::Sqrt,
            Curve::Log => CurveName::Log,
            Curve::Asinh => CurveName::Asinh,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white_are_chosen_by_the_saved_method() {
        let data = PixelData::F32((0..=100).map(|i| i as f32).collect());
        assert_eq!(BlackWhite::File.window(&data), None);
        assert_eq!(BlackWhite::MinMax.window(&data), Some((0.0, 100.0)));
        let fixed = BlackWhite::Values {
            black: 5.0,
            white: 7.0,
        };
        assert_eq!(fixed.window(&data), Some((5.0, 7.0)));
        let (lo, hi) = BlackWhite::Percentile {
            black: 10.0,
            white: 90.0,
        }
        .window(&data)
        .unwrap();
        assert!(
            (lo - 10.0).abs() < 1.0 && (hi - 90.0).abs() < 1.0,
            "{lo} {hi}"
        );
    }

    #[test]
    fn an_empty_look_writes_nothing() {
        assert_eq!(toml::to_string(&Look::default()).unwrap(), "");
        let look = Look {
            black_white: BlackWhite::Percentile {
                black: 0.5,
                white: 99.5,
            },
            curve: Some(CurveName::Asinh),
            color_map: Some("inferno".into()),
            color_map_stops: Vec::new(),
        };
        let text = toml::to_string(&look).unwrap();
        assert_eq!(toml::from_str::<Look>(&text).unwrap(), look, "{text}");
    }

    #[test]
    fn a_color_map_not_built_in_is_made_from_its_saved_stops() {
        let mut look = Look {
            color_map: Some("mine".into()),
            ..Look::default()
        };
        assert!(look.palette().is_err());
        look.color_map_stops = vec!["0 #000000".into(), "1 #ff0000".into()];
        assert_eq!(look.palette().unwrap().unwrap().colors[255], [255, 0, 0]);
        look.color_map = Some(COLOR_MAP_OFF.into());
        assert!(look.palette().unwrap().is_none());
        look.color_map = Some("gray".into());
        assert!(look.palette().unwrap().is_none());
    }

    #[test]
    fn the_look_sets_black_white_and_curve_only_on_gray_images() {
        let look = Look {
            black_white: BlackWhite::Values {
                black: 1.0,
                white: 2.0,
            },
            curve: Some(CurveName::Sqrt),
            ..Look::default()
        };
        let mut gray = VwImage::new(2, 1, ColorModel::Gray, PixelData::U8(vec![0, 9])).unwrap();
        assert_eq!(look.apply(&mut gray), Some((1.0, 2.0)));
        assert_eq!((gray.window, gray.curve), (Some((1.0, 2.0)), Curve::Sqrt));
        let mut rgb = VwImage::new(1, 1, ColorModel::Rgb, PixelData::U8(vec![0, 9, 9])).unwrap();
        assert_eq!(look.apply(&mut rgb), None);
        assert_eq!(rgb.window, None);
    }
}
