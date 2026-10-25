//! 色の表（カラーマップ）。灰色の明るさ 0〜255 を色に写す。
//!
//! 表は [`crate::display::to_display`] が作った灰色の画像に後から引く。値から明るさへの写し方
//! （黒と白の値、曲線）はそのままにして、見た目の色だけを変えるため。

use crate::palette_tables as t;

/// 256 色の表。添字が灰色の明るさ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub name: String,
    pub colors: [[u8; 3]; 256],
}

/// 組み込みの表の名前（設定に書く名前）。並びはメニューに出す順。
pub const BUILTIN: [&str; 11] = [
    "gray",
    "gray_inverted",
    "inferno",
    "magma",
    "plasma",
    "viridis",
    "hot_iron",
    "pet",
    "hot_metal_blue",
    "jet",
    "rainbow",
];

/// 灰色の表の名前。灰色の表は色を変えないので、引かずに済ませる。
pub const GRAY: &str = "gray";

/// 組み込みの表の、画面に出す名前。自分で足した表は設定に書いた名前のまま出す。
pub fn title(name: &str) -> &str {
    match name {
        "gray" => "Gray",
        "gray_inverted" => "Gray (inverted)",
        "hot_iron" => "Hot Iron",
        "pet" => "PET",
        "hot_metal_blue" => "Hot Metal Blue",
        other => other,
    }
}

impl Palette {
    /// 組み込みの表。知らない名前は `None`。
    pub fn builtin(name: &str) -> Option<Self> {
        let colors = match name {
            "gray" => std::array::from_fn(|i| [i as u8; 3]),
            "gray_inverted" => std::array::from_fn(|i| [255 - i as u8; 3]),
            "inferno" => t::INFERNO,
            "magma" => t::MAGMA,
            "plasma" => t::PLASMA,
            "viridis" => t::VIRIDIS,
            "hot_iron" => t::HOT_IRON,
            "pet" => t::PET,
            "hot_metal_blue" => t::HOT_METAL_BLUE,
            "jet" => from_fn(jet),
            "rainbow" => from_fn(rainbow),
            _ => return None,
        };
        Some(Self {
            name: name.to_owned(),
            colors,
        })
    }

    /// 色の点 `(位置 0〜1, 色)` の間を直線でつないだ表。点は位置の順でなくてよい。
    pub fn from_stops(name: &str, stops: &[(f32, [u8; 3])]) -> Result<Self, String> {
        if stops.len() < 2 {
            return Err("needs at least 2 colors".to_owned());
        }
        if let Some((p, _)) = stops.iter().find(|(p, _)| !(0.0..=1.0).contains(p)) {
            return Err(format!("position {p} is not 0 to 1"));
        }
        let mut stops = stops.to_vec();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        let colors = std::array::from_fn(|i| {
            let x = i as f32 / 255.0;
            let after = stops.iter().position(|(p, _)| *p >= x);
            let (a, b) = match after {
                Some(0) => (stops[0], stops[0]),
                Some(j) => (stops[j - 1], stops[j]),
                None => (stops[stops.len() - 1], stops[stops.len() - 1]),
            };
            let f = if b.0 > a.0 {
                (x - a.0) / (b.0 - a.0)
            } else {
                0.0
            };
            std::array::from_fn(|c| {
                (f32::from(a.1[c]) + (f32::from(b.1[c]) - f32::from(a.1[c])) * f).round() as u8
            })
        });
        Ok(Self {
            name: name.to_owned(),
            colors,
        })
    }

    /// 灰色の表か（引いても色が変わらない）。
    pub fn is_gray(&self) -> bool {
        self.colors
            .iter()
            .enumerate()
            .all(|(i, c)| *c == [i as u8; 3])
    }

    /// 位置 `t`（0〜1）の色。範囲外は端の色。
    pub fn at(&self, t: f32) -> [u8; 3] {
        self.colors[(t.clamp(0.0, 1.0) * 255.0).round() as usize]
    }

    /// 灰色の RGBA8 の画素を色にする。赤の値を明るさとして引き、アルファは残す。
    pub fn recolor(&self, rgba: &mut [u8]) {
        for px in rgba.as_chunks_mut::<4>().0 {
            let [r, g, b] = self.colors[px[0] as usize];
            px[..3].copy_from_slice(&[r, g, b]);
        }
    }
}

/// 色の点の書き方「位置 #rrggbb」（例: `0.5 #ff8000`）を読む。
pub fn parse_stop(text: &str) -> Result<(f32, [u8; 3]), String> {
    let mut parts = text.split_whitespace();
    let (Some(pos), Some(color), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("\"{text}\" is not \"position #rrggbb\""));
    };
    let pos: f32 = pos
        .parse()
        .map_err(|_| format!("\"{pos}\" is not a number"))?;
    let hex = color.strip_prefix('#').unwrap_or(color);
    let channel = |i: usize| {
        hex.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .ok_or_else(|| format!("\"{color}\" is not #rrggbb"))
    };
    if hex.len() != 6 {
        return Err(format!("\"{color}\" is not #rrggbb"));
    }
    Ok((pos, [channel(0)?, channel(2)?, channel(4)?]))
}

fn from_fn(f: fn(f32) -> [f32; 3]) -> [[u8; 3]; 256] {
    std::array::from_fn(|i| f(i as f32 / 255.0).map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
}

/// Matplotlib の `jet` と同じ折れ線（`_jet_data`）。表を写さず式にしたのは、折れ線を
/// 直線でつなぐだけで表と同じ値になるため。
fn jet(x: f32) -> [f32; 3] {
    let ramp = |points: &[(f32, f32)]| {
        let j = points
            .iter()
            .position(|p| p.0 >= x)
            .unwrap_or(points.len() - 1);
        if j == 0 {
            return points[0].1;
        }
        let (a, b) = (points[j - 1], points[j]);
        a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0)
    };
    [
        ramp(&[
            (0.0, 0.0),
            (0.35, 0.0),
            (0.66, 1.0),
            (0.89, 1.0),
            (1.0, 0.5),
        ]),
        ramp(&[
            (0.0, 0.0),
            (0.125, 0.0),
            (0.375, 1.0),
            (0.64, 1.0),
            (0.91, 0.0),
            (1.0, 0.0),
        ]),
        ramp(&[
            (0.0, 0.5),
            (0.11, 1.0),
            (0.34, 1.0),
            (0.65, 0.0),
            (1.0, 0.0),
        ]),
    ]
}

/// Matplotlib の `rainbow`（`_rainbow_data`）の式。
fn rainbow(x: f32) -> [f32; 3] {
    use std::f32::consts::PI;
    [(2.0 * x - 0.5).abs(), (PI * x).sin(), (PI * x / 2.0).cos()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_name_has_a_table() {
        for name in BUILTIN {
            assert!(Palette::builtin(name).is_some(), "{name}");
        }
        assert!(Palette::builtin("unknown").is_none());
    }

    #[test]
    fn perceptual_tables_match_matplotlib_at_both_ends_and_the_middle() {
        // Matplotlib の値（0〜1）を 255 倍して丸めたもの。
        let cases = [
            ("inferno", [[0, 0, 4], [188, 55, 84], [252, 255, 164]]),
            ("viridis", [[68, 1, 84], [33, 145, 140], [253, 231, 37]]),
            ("magma", [[0, 0, 4], [183, 55, 121], [252, 253, 191]]),
            ("plasma", [[13, 8, 135], [204, 71, 120], [240, 249, 33]]),
        ];
        for (name, [lo, mid, hi]) in cases {
            let p = Palette::builtin(name).unwrap();
            assert_eq!(
                [p.colors[0], p.colors[128], p.colors[255]],
                [lo, mid, hi],
                "{name}"
            );
        }
    }

    #[test]
    fn dicom_palettes_go_from_black_to_white() {
        for name in ["hot_iron", "pet"] {
            let p = Palette::builtin(name).unwrap();
            assert_eq!(p.colors[0], [0, 0, 0], "{name}");
            assert_eq!(p.colors[255], [255, 255, 255], "{name}");
        }
    }

    #[test]
    fn hot_iron_follows_the_standard_red_then_green_then_blue_ramps() {
        // PS3.6 B.1.1: 赤は 0〜127 で 0→254、緑は 128〜255 で 0→254、青は 192〜255 で 0→252。
        let p = Palette::builtin("hot_iron").unwrap();
        assert_eq!(p.colors[64], [128, 0, 0]);
        assert_eq!(p.colors[127], [254, 0, 0]);
        assert_eq!(p.colors[191], [255, 126, 0]);
    }

    #[test]
    fn jet_runs_from_dark_blue_through_green_to_dark_red() {
        let p = Palette::builtin("jet").unwrap();
        assert_eq!(p.colors[0], [0, 0, 128]);
        assert_eq!(p.at(0.5), [125, 255, 122]);
        assert_eq!(p.colors[255], [128, 0, 0]);
    }

    #[test]
    fn gray_tables_keep_or_invert_the_brightness() {
        assert!(Palette::builtin("gray").unwrap().is_gray());
        let inverted = Palette::builtin("gray_inverted").unwrap();
        assert!(!inverted.is_gray());
        assert_eq!(inverted.colors[0], [255; 3]);
        assert_eq!(inverted.colors[255], [0; 3]);
    }

    #[test]
    fn stops_are_joined_by_straight_lines_and_ends_are_held() {
        let p = Palette::from_stops("mine", &[(1.0, [255, 255, 255]), (0.2, [0, 0, 200])]).unwrap();
        assert_eq!(p.colors[0], [0, 0, 200]);
        assert_eq!(p.colors[51], [0, 0, 200]);
        assert_eq!(p.colors[255], [255, 255, 255]);
        let mid = p.at(0.6);
        assert!((126..=129).contains(&mid[0]), "{mid:?}");
    }

    #[test]
    fn stops_need_two_colors_inside_zero_to_one() {
        assert!(Palette::from_stops("a", &[(0.0, [0; 3])]).is_err());
        assert!(Palette::from_stops("a", &[(0.0, [0; 3]), (1.5, [0; 3])]).is_err());
    }

    #[test]
    fn stop_text_is_a_position_and_a_hex_color() {
        assert_eq!(parse_stop("0.5 #ff8000"), Ok((0.5, [255, 128, 0])));
        assert_eq!(parse_stop(" 1  00FF00 "), Ok((1.0, [0, 255, 0])));
        assert!(parse_stop("0.5").is_err());
        assert!(parse_stop("half #ff8000").is_err());
        assert!(parse_stop("0.5 #ff80").is_err());
        assert!(parse_stop("0.5 #gg8000").is_err());
    }

    #[test]
    fn recolor_uses_red_as_brightness_and_keeps_alpha() {
        let p = Palette::builtin("hot_iron").unwrap();
        let mut px = vec![127, 127, 127, 40, 0, 0, 0, 255];
        p.recolor(&mut px);
        assert_eq!(px, vec![254, 0, 0, 40, 0, 0, 0, 255]);
    }
}
