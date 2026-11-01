//! 描く命令の記録と、それを SVG にすること。画像にするのは [`crate::raster`]。
//!
//! 図の並べ方は 1 か所（[`crate::sheet`]）で決め、命令として記録してから SVG と画像の両方に
//! 書き出す。SVG と画像で別々に並べると、同じテーマでも HTML とクリップボードで見た目がずれるため。

use std::fmt::Write as _;

use crate::Rgba;
use crate::theme::{Color, Face};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// 1 つの描く命令。座標は論理の単位（SVG の viewBox と同じ）。文字の `y` は字の並びの基準線。
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Line {
        points: Vec<[f32; 2]>,
        color: Color,
        width: f32,
        dash: &'static [f32],
    },
    Rect {
        rect: [f32; 4],
        fill: Option<Color>,
        stroke: Option<(Color, f32)>,
    },
    Polygon {
        points: Vec<[f32; 2]>,
        fill: Option<Color>,
        stroke: Option<(Color, f32)>,
    },
    Circle {
        center: [f32; 2],
        radius: f32,
        fill: Option<Color>,
        stroke: Option<(Color, f32)>,
    },
    Text {
        text: String,
        at: [f32; 2],
        size: f32,
        color: Color,
        anchor: Anchor,
        face: Face,
        /// 左に 90° 回す（縦軸の名前）。
        vertical: bool,
    },
    Image {
        rect: [f32; 4],
        image: Rgba,
    },
}

/// 文字の幅を測る。画像にするときは字体の送り幅、SVG では見積もり。
pub trait Measure {
    fn width(&self, text: &str, size: f32, face: Face) -> f32;
}

/// 字体を読まずに見積もる幅。SVG は開く側の字体で描かれ、正確な幅は分からないので、
/// 和文は 1 文字 1 em、欧文は 0.56 em（等幅は 0.6 em）と見て、少し広めに取る。
pub struct Estimate;

impl Measure for Estimate {
    fn width(&self, text: &str, size: f32, face: Face) -> f32 {
        text.chars()
            .map(|c| {
                if is_wide(c) {
                    1.0
                } else if face == Face::Mono {
                    0.6
                } else {
                    0.56
                }
            })
            .sum::<f32>()
            * size
    }
}

pub fn is_wide(c: char) -> bool {
    matches!(u32::from(c), 0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6)
}

/// 命令を貯める。
pub struct Recorder<'a> {
    pub ops: Vec<Op>,
    pub measure: &'a dyn Measure,
}

impl<'a> Recorder<'a> {
    pub fn new(measure: &'a dyn Measure) -> Self {
        Self {
            ops: Vec::new(),
            measure,
        }
    }

    pub fn width(&self, text: &str, size: f32, face: Face) -> f32 {
        self.measure.width(text, size, face)
    }

    pub fn line(&mut self, points: Vec<[f32; 2]>, color: Color, width: f32, dash: &'static [f32]) {
        if points.len() >= 2 {
            self.ops.push(Op::Line {
                points,
                color,
                width,
                dash,
            });
        }
    }

    pub fn rect(&mut self, rect: [f32; 4], fill: Option<Color>, stroke: Option<(Color, f32)>) {
        self.ops.push(Op::Rect { rect, fill, stroke });
    }

    pub fn text(
        &mut self,
        text: impl Into<String>,
        at: [f32; 2],
        size: f32,
        color: Color,
    ) -> TextOp<'_, 'a> {
        self.ops.push(Op::Text {
            text: text.into(),
            at,
            size,
            color,
            anchor: Anchor::Start,
            face: Face::Body,
            vertical: false,
        });
        TextOp { recorder: self }
    }

    /// `text` を `width` に収まるように折り返した行。和文は文字ごと、欧文は空白で切る。
    pub fn wrap(&self, text: &str, size: f32, face: Face, width: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.split('\n') {
            let mut line = String::new();
            let mut last_break: Option<usize> = None;
            for c in paragraph.chars() {
                line.push(c);
                if c == ' ' || is_wide(c) {
                    last_break = Some(line.len());
                }
                if self.width(&line, size, face) > width && line.chars().count() > 1 {
                    let cut = match last_break {
                        Some(at) if at < line.len() && at > 0 => at,
                        _ => line.len() - c.len_utf8(),
                    };
                    let rest = line.split_off(cut);
                    lines.push(line.trim_end().to_owned());
                    line = rest.trim_start().to_owned();
                    last_break = None;
                }
            }
            lines.push(line);
        }
        lines
    }
}

/// 最後に足した文字の向きや字体を変える。
pub struct TextOp<'r, 'a> {
    recorder: &'r mut Recorder<'a>,
}

impl TextOp<'_, '_> {
    fn last(&mut self) -> &mut Op {
        self.recorder
            .ops
            .last_mut()
            .expect("a text op was just pushed")
    }

    pub fn anchor(mut self, a: Anchor) -> Self {
        if let Op::Text { anchor, .. } = self.last() {
            *anchor = a;
        }
        self
    }

    pub fn face(mut self, f: Face) -> Self {
        if let Op::Text { face, .. } = self.last() {
            *face = f;
        }
        self
    }

    pub fn vertical(mut self) -> Self {
        if let Op::Text { vertical, .. } = self.last() {
            *vertical = true;
        }
        self
    }
}

/// 命令を `width` × `height` の SVG にする。`fonts` は本文と等幅の `font-family`。
pub fn svg(ops: &[Op], width: f32, height: f32, fonts: [&str; 2], label: &str) -> String {
    let mut s = String::new();
    let _ = write!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"{}\" height=\"{}\" role=\"img\" aria-label=\"{}\" font-family=\"{}\">",
        n(width),
        n(height),
        n(width),
        n(height),
        esc(label),
        esc(fonts[0])
    );
    for op in ops {
        match op {
            Op::Line {
                points,
                color,
                width,
                dash,
            } => {
                let _ = write!(
                    s,
                    "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" stroke-linejoin=\"round\"{}/>",
                    path(points, false),
                    color.css(),
                    n(*width),
                    dash_attr(dash)
                );
            }
            Op::Rect { rect, fill, stroke } => {
                let [x, y, w, h] = *rect;
                let _ = write!(
                    s,
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
                    n(x),
                    n(y),
                    n(w),
                    n(h),
                    paint(*fill, *stroke)
                );
            }
            Op::Polygon {
                points,
                fill,
                stroke,
            } => {
                let _ = write!(
                    s,
                    "<path d=\"{}\"{}/>",
                    path(points, true),
                    paint(*fill, *stroke)
                );
            }
            Op::Circle {
                center,
                radius,
                fill,
                stroke,
            } => {
                let _ = write!(
                    s,
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"{}/>",
                    n(center[0]),
                    n(center[1]),
                    n(*radius),
                    paint(*fill, *stroke)
                );
            }
            Op::Text {
                text,
                at,
                size,
                color,
                anchor,
                face,
                vertical,
            } => {
                let anchor = match anchor {
                    Anchor::Start => "start",
                    Anchor::Middle => "middle",
                    Anchor::End => "end",
                };
                let place = if *vertical {
                    format!(
                        "transform=\"translate({} {}) rotate(-90)\"",
                        n(at[0]),
                        n(at[1])
                    )
                } else {
                    format!("x=\"{}\" y=\"{}\"", n(at[0]), n(at[1]))
                };
                let family = match face {
                    Face::Mono => format!(" font-family=\"{}\"", esc(fonts[1])),
                    Face::Body => String::new(),
                };
                let _ = write!(
                    s,
                    "<text {place} font-size=\"{}\" text-anchor=\"{anchor}\" fill=\"{}\"{family}>{}</text>",
                    n(*size),
                    color.css(),
                    esc(text)
                );
            }
            Op::Image { rect, image } => {
                let [x, y, w, h] = *rect;
                let _ = write!(
                    s,
                    "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" href=\"{}\"/>",
                    n(x),
                    n(y),
                    n(w),
                    n(h),
                    crate::data_url(image)
                );
            }
        }
    }
    s.push_str("</svg>");
    s
}

/// 小数 1 桁まで。SVG を軽くするため。
fn n(v: f32) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r:.1}")
    }
}

fn path(points: &[[f32; 2]], closed: bool) -> String {
    let mut d = String::new();
    for (i, p) in points.iter().enumerate() {
        let _ = write!(
            d,
            "{}{} {}",
            if i == 0 { "M" } else { "L" },
            n(p[0]),
            n(p[1])
        );
    }
    if closed {
        d.push('Z');
    }
    d
}

fn dash_attr(dash: &[f32]) -> String {
    if dash.is_empty() {
        String::new()
    } else {
        let parts: Vec<String> = dash.iter().map(|&d| n(d)).collect();
        format!(" stroke-dasharray=\"{}\"", parts.join(" "))
    }
}

fn paint(fill: Option<Color>, stroke: Option<(Color, f32)>) -> String {
    let mut s = match fill {
        Some(c) => format!(" fill=\"{}\"", c.css()),
        None => " fill=\"none\"".to_owned(),
    };
    if let Some((c, w)) = stroke {
        let _ = write!(s, " stroke=\"{}\" stroke-width=\"{}\"", c.css(), n(w));
    }
    s
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japanese_counts_as_one_em_and_latin_as_about_half() {
        assert_eq!(Estimate.width("あい", 10.0, Face::Body), 20.0);
        assert!((Estimate.width("ab", 10.0, Face::Body) - 11.2).abs() < 1e-4);
    }

    #[test]
    fn long_japanese_text_wraps_by_character() {
        let r = Recorder::new(&Estimate);
        let lines = r.wrap("あいうえおかきくけこ", 10.0, Face::Body, 45.0);
        assert_eq!(lines, ["あいうえ", "おかきく", "けこ"]);
    }

    #[test]
    fn latin_text_wraps_at_spaces() {
        let r = Recorder::new(&Estimate);
        let lines = r.wrap("one two three", 10.0, Face::Body, 45.0);
        assert_eq!(lines, ["one two", "three"]);
    }

    #[test]
    fn svg_escapes_text_and_draws_dashes() {
        let mut r = Recorder::new(&Estimate);
        r.text("a < b", [1.0, 2.0], 10.0, Color::rgb(0));
        r.line(
            vec![[0.0, 0.0], [10.0, 0.5]],
            Color::rgb(0xFF0000),
            1.5,
            &[6.0, 3.0],
        );
        let s = svg(&r.ops, 20.0, 10.0, ["serif", "monospace"], "test");
        assert!(s.contains(">a &lt; b</text>"));
        assert!(s.contains("stroke-dasharray=\"6 3\""));
        assert!(s.contains("d=\"M0 0L10 0.5\""));
    }
}
