//! 出力の見た目のテーマ 4 つ。レポートの HTML、グラフ、クリップボードの画像で同じものを使う。

/// 8bit の RGBA。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub const fn rgb(hex: u32) -> Self {
        Self([(hex >> 16) as u8, (hex >> 8) as u8, hex as u8, 255])
    }

    /// `#rrggbb`（不透明でなければ `rgba(...)`）。
    pub fn css(self) -> String {
        let [r, g, b, a] = self.0;
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("rgba({r},{g},{b},{:.3})", f32::from(a) / 255.0)
        }
    }
}

/// 線の上に打つ印。印刷モノクロのテーマで、線の形と合わせて見分けるのに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Circle,
    Square,
    Triangle,
    Diamond,
}

/// 1 本の線の描き方。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineStyle {
    pub color: Color,
    /// 点線の長さ（線, 間, 線, 間…）。空なら実線。
    pub dash: &'static [f32],
    pub marker: Option<Marker>,
}

/// 文字の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// 本文（論文は明朝、ほかはゴシック）。
    Body,
    /// 数字と表の値。
    Mono,
}

/// テーマ。設定の `report.theme` の値（`paper`・`lab`・`dark`・`mono`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// 白地・明朝、図の番号、罫線 3 本の表。
    Paper,
    /// ゴシック・等幅の数字・薄い方眼、色覚の違いでも見分けやすい配色。
    #[default]
    Lab,
    /// 画面と同じ暗い灰色と細い線。
    Dark,
    /// 黒だけ。線の形と印で見分ける。
    Mono,
}

const SOLID: &[f32] = &[];
const DASHES: [&[f32]; 4] = [SOLID, &[6.0, 3.0], &[1.5, 2.5], &[8.0, 3.0, 2.0, 3.0]];
const MARKERS: [Marker; 4] = [
    Marker::Circle,
    Marker::Square,
    Marker::Triangle,
    Marker::Diamond,
];

/// 色覚の違いでも見分けやすい Okabe–Ito の配色（黄は白地で読めないので外す）。
const OKABE_ITO: [u32; 6] = [0x0072B2, 0xD55E00, 0x009E73, 0xCC79A7, 0xE69F00, 0x56B4E9];
const DARK_LINES: [u32; 6] = [0x6CB6FF, 0xF0A35E, 0x7ED39B, 0xE88FD0, 0xE8D36C, 0x9FD8FF];

impl Theme {
    pub const ALL: [Theme; 4] = [Theme::Paper, Theme::Lab, Theme::Dark, Theme::Mono];

    /// 設定ファイルに書く名前。
    pub fn key(self) -> &'static str {
        match self {
            Theme::Paper => "paper",
            Theme::Lab => "lab",
            Theme::Dark => "dark",
            Theme::Mono => "mono",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.key() == key)
    }

    pub fn background(self) -> Color {
        Color::rgb(match self {
            Theme::Paper | Theme::Mono => 0xFFFFFF,
            Theme::Lab => 0xFBFCFD,
            Theme::Dark => 0x1E1F22,
        })
    }

    /// 本文と題の色。
    pub fn ink(self) -> Color {
        Color::rgb(match self {
            Theme::Paper => 0x111111,
            Theme::Lab => 0x1C2430,
            Theme::Dark => 0xD6D8DC,
            Theme::Mono => 0x000000,
        })
    }

    /// 題の色。技術ノートだけ紺にする。
    pub fn heading(self) -> Color {
        match self {
            Theme::Lab => Color::rgb(0x12324F),
            Theme::Dark => Color::rgb(0xECEEF1),
            _ => self.ink(),
        }
    }

    /// 目盛りの数字・説明文などの控えめな文字。
    pub fn muted(self) -> Color {
        Color::rgb(match self {
            Theme::Paper => 0x444444,
            Theme::Lab => 0x5A6878,
            Theme::Dark => 0x8C939C,
            Theme::Mono => 0x333333,
        })
    }

    /// 方眼の色。論文は方眼を引かない。
    pub fn grid(self) -> Option<Color> {
        match self {
            Theme::Paper => None,
            Theme::Lab => Some(Color::rgb(0xE3E8EE)),
            Theme::Dark => Some(Color::rgb(0x2C2F33)),
            Theme::Mono => Some(Color::rgb(0xDDDDDD)),
        }
    }

    pub fn axis(self) -> Color {
        Color::rgb(match self {
            Theme::Paper | Theme::Mono => 0x111111,
            Theme::Lab => 0x7A8796,
            Theme::Dark => 0x5B6068,
        })
    }

    /// 表の罫線の色。
    pub fn rule(self) -> Color {
        match self {
            Theme::Paper | Theme::Mono => self.ink(),
            Theme::Lab => Color::rgb(0xE3E8EE),
            Theme::Dark => Color::rgb(0x3A3D42),
        }
    }

    /// 表の見出しの地の色。技術ノートだけ塗る。
    pub fn table_head(self) -> Option<Color> {
        (self == Theme::Lab).then_some(Color::rgb(0xE8EEF5))
    }

    /// グラフの周りを枠で囲むか（囲まないテーマは左と下の軸だけ）。
    pub fn framed(self) -> bool {
        matches!(self, Theme::Paper | Theme::Mono)
    }

    pub fn line_width(self) -> f32 {
        match self {
            Theme::Paper => 1.4,
            Theme::Lab => 1.8,
            Theme::Dark => 1.6,
            Theme::Mono => 1.3,
        }
    }

    /// 図の番号を付けるか（論文だけ）。
    pub fn numbers_figures(self) -> bool {
        self == Theme::Paper
    }

    /// `i` 本目の線。
    pub fn line(self, i: usize) -> LineStyle {
        match self {
            Theme::Paper => {
                const PAPER: [(u32, usize); 4] =
                    [(0x111111, 0), (0x111111, 1), (0xB0342B, 0), (0x111111, 2)];
                let (color, dash) = PAPER[i % PAPER.len()];
                LineStyle {
                    color: Color::rgb(color),
                    dash: DASHES[(dash + i / PAPER.len()) % DASHES.len()],
                    marker: None,
                }
            }
            Theme::Lab => colored(OKABE_ITO[i % OKABE_ITO.len()], i / OKABE_ITO.len()),
            Theme::Dark => colored(DARK_LINES[i % DARK_LINES.len()], i / DARK_LINES.len()),
            Theme::Mono => LineStyle {
                color: Color::rgb(0x000000),
                dash: DASHES[i % DASHES.len()],
                marker: Some(MARKERS[(i + i / DASHES.len()) % MARKERS.len()]),
            },
        }
    }

    /// カラー画像の色 `channel`（0 = R、1 = G、2 = B）の、`line` 本目の計測線の線。
    /// 色は色の名前に合わせ、何本目かは線の形で見分ける。
    pub fn channel(self, channel: usize, line: usize) -> LineStyle {
        let colors: [u32; 3] = match self {
            Theme::Paper => [0xB0342B, 0x2E7D32, 0x1F4E9C],
            Theme::Lab => [0xD55E00, 0x009E73, 0x0072B2],
            Theme::Dark => [0xFF7B72, 0x7ED39B, 0x6CB6FF],
            Theme::Mono => [0x000000; 3],
        };
        LineStyle {
            color: Color::rgb(colors[channel % 3]),
            dash: DASHES[line % DASHES.len()],
            marker: (self == Theme::Mono).then_some(MARKERS[channel % MARKERS.len()]),
        }
    }

    /// HTML の `font-family`。書き出した HTML は手元で開くので、Web フォントは読まず OS の字体を並べる。
    pub fn font_family(self, face: Face) -> &'static str {
        match (self, face) {
            (_, Face::Mono) => {
                "ui-monospace, \"Cascadia Mono\", Consolas, \"DejaVu Sans Mono\", monospace"
            }
            (Theme::Paper, Face::Body) => {
                "\"Yu Mincho\", YuMincho, \"Hiragino Mincho ProN\", \"Noto Serif CJK JP\", \"Noto Serif JP\", serif"
            }
            (_, Face::Body) => {
                "\"Yu Gothic UI\", \"Yu Gothic\", Meiryo, \"Hiragino Sans\", \"Noto Sans CJK JP\", \"Noto Sans JP\", system-ui, sans-serif"
            }
        }
    }

    /// 表の数字を等幅にするか。
    pub fn mono_numbers(self) -> bool {
        matches!(self, Theme::Lab | Theme::Dark)
    }
}

fn colored(hex: u32, round: usize) -> LineStyle {
    LineStyle {
        color: Color::rgb(hex),
        dash: DASHES[round % DASHES.len()],
        marker: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_round_trip() {
        for t in Theme::ALL {
            assert_eq!(Theme::from_key(t.key()), Some(t));
        }
        assert_eq!(Theme::from_key("neon"), None);
    }

    #[test]
    fn the_technical_note_is_the_default() {
        assert_eq!(Theme::default(), Theme::Lab);
    }

    #[test]
    fn neighbouring_lines_look_different_in_every_theme() {
        for t in Theme::ALL {
            for i in 0..8 {
                let (a, b) = (t.line(i), t.line(i + 1));
                assert!(
                    a.color != b.color || a.dash != b.dash || a.marker != b.marker,
                    "{t:?} {i}"
                );
            }
        }
    }

    #[test]
    fn monochrome_lines_are_black_and_carry_markers() {
        for i in 0..6 {
            let l = Theme::Mono.line(i);
            assert_eq!(l.color, Color::rgb(0));
            assert!(l.marker.is_some());
        }
    }

    #[test]
    fn color_channels_keep_their_hue_and_lines_differ_by_dash() {
        let r1 = Theme::Lab.channel(0, 0);
        let r2 = Theme::Lab.channel(0, 1);
        assert_eq!(r1.color, r2.color);
        assert_ne!(r1.dash, r2.dash);
        assert_ne!(Theme::Lab.channel(1, 0).color, r1.color);
    }

    #[test]
    fn css_colors_are_hex_when_opaque() {
        assert_eq!(Color::rgb(0x0072B2).css(), "#0072b2");
        assert_eq!(Color([0, 0, 0, 128]).css(), "rgba(0,0,0,0.502)");
    }
}
