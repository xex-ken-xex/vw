//! vw が書き出すレポートとグラフの見た目。
//!
//! - テーマ 4 つ（[`Theme`]）: 論文・技術ノート・vw ダーク・印刷モノクロ
//! - 1 枚の図（[`Sheet`]: 題・元の情報・グラフや画像・凡例・数値の表・説明文）を 1 か所で並べ、
//!   SVG（HTML のレポートの中）と画像（クリップボード・画像だけの書き出し）の両方に描く
//! - レポートの HTML の型（[`html::Page`]）
//!
//! 文の言語は呼ぶ側が決める（[`Words`] と、図に入れる文字）。

use base64::Engine;

pub mod canvas;
pub mod html;
pub mod raster;
pub mod sheet;
pub mod theme;

pub use raster::Fonts;
pub use sheet::{Block, Chart, LegendItem, Panel, Series, Sheet, Swatch, Table};
pub use theme::{Color, Face, LineStyle, Marker, Theme};

/// RGBA8 の画像。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// 図と表の言葉。
#[derive(Debug, Clone, Copy)]
pub struct Words {
    pub figure: &'static str,
    pub table: &'static str,
    /// 図の番号と説明文の間。
    pub number_gap: &'static str,
    /// 元の情報を 1 行につなぐ区切り。
    pub separator: &'static str,
    pub lang: &'static str,
}

impl Words {
    pub const ENGLISH: Words = Words {
        figure: "Figure",
        table: "Table",
        number_gap: ". ",
        separator: " · ",
        lang: "en",
    };
    pub const JAPANESE: Words = Words {
        figure: "図",
        table: "表",
        number_gap: "\u{3000}",
        separator: " ・ ",
        lang: "ja",
    };
}

/// 論理の幅。画像はこの幅の [`IMAGE_SCALE`] 倍の画素にする。
pub const SHEET_WIDTH: f32 = 720.0;
/// 画像にするときの倍率。高い画素密度の画面や印刷でも文字がつぶれないように 2 倍にする。
pub const IMAGE_SCALE: f32 = 2.0;

impl Sheet {
    /// 1 枚の画像にする。
    pub fn image(&self, theme: Theme, words: &Words, fonts: &Fonts) -> Rgba {
        let mut r = canvas::Recorder::new(fonts);
        let height = self.layout(&mut r, theme, words, SHEET_WIDTH);
        raster::render(&r.ops, SHEET_WIDTH, height, IMAGE_SCALE, fonts)
    }
}

impl Chart {
    /// HTML に入れる SVG（凡例・軸・線。題と説明文は HTML の側に書く）。
    pub fn svg(&self, theme: Theme, width: f32, label: &str) -> String {
        let mut r = canvas::Recorder::new(&canvas::Estimate);
        r.rect([0.0, 0.0, width, 0.0], Some(theme.background()), None);
        let height = sheet::chart(&mut r, theme, self, [0.0, 6.0], width) + 4.0;
        if let Some(canvas::Op::Rect { rect, .. }) = r.ops.first_mut() {
            rect[3] = height;
        }
        canvas::svg(
            &r.ops,
            width,
            height,
            [theme.font_family(Face::Body), theme.font_family(Face::Mono)],
            label,
        )
    }
}

/// 凡例だけの SVG（HTML の画像の下に置く）。
pub fn legend_svg(theme: Theme, items: &[LegendItem], width: f32) -> String {
    let mut r = canvas::Recorder::new(&canvas::Estimate);
    let height = sheet::legend_rows(&mut r, theme, items, [0.0, 0.0], width);
    canvas::svg(
        &r.ops,
        width,
        height,
        [theme.font_family(Face::Body), theme.font_family(Face::Mono)],
        "",
    )
}

pub fn png(img: &Rgba) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width as u32, img.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("PNG header to a Vec");
        writer
            .write_image_data(&img.data)
            .expect("PNG data to a Vec");
    }
    out
}

pub fn data_url(img: &Rgba) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png(img))
    )
}

/// CSV の 1 つの値。カンマ・引用符・改行を含むときだけ囲む。
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_fields_are_quoted_only_when_needed() {
        assert_eq!(csv_field("a"), "a");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn a_chart_becomes_an_svg_with_the_theme_background() {
        let c = Chart {
            x_label: "x".into(),
            panels: vec![Panel {
                y_label: "y".into(),
                series: vec![Series {
                    name: "a".into(),
                    points: vec![[0.0, 0.0], [1.0, 1.0]],
                    style: Theme::Dark.line(0),
                }],
            }],
        };
        let s = c.svg(Theme::Dark, 600.0, "chart");
        assert!(s.starts_with("<svg"));
        assert!(s.contains("fill=\"#1e1f22\""));
    }
}
