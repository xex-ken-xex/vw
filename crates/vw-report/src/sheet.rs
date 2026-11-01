//! 1 枚の図（題・元の情報・グラフや画像・凡例・数値の表・説明文）の中身と並べ方。

use crate::canvas::{Anchor, Recorder};
use crate::theme::{Color, Face, LineStyle, Marker, Theme};
use crate::{Rgba, Words};

/// 線 1 本。`y` が NaN の点は欠損で、そこで線を切る。
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub name: String,
    pub points: Vec<[f64; 2]>,
    pub style: LineStyle,
}

/// 縦軸を共有する線の組。単位の違う値（画素の値と計測の値）は別の段にする。
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    /// 縦軸の名前（単位を含む）。
    pub y_label: String,
    pub series: Vec<Series>,
}

/// 横軸を共有する段の並び。
#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    pub x_label: String,
    /// 横軸の目盛りの文字（位置, 文字）。空なら値の範囲から 1・2・5 の刻みで数字を付ける。
    /// 日時や気圧の面のように、横軸の位置が番号で、見せたいのが別の文字のときに使う。
    pub x_names: Vec<(f64, String)>,
    pub panels: Vec<Panel>,
}

/// 凡例の見本。
#[derive(Debug, Clone, PartialEq)]
pub enum Swatch {
    Line(LineStyle),
    /// 色の帯（左が小さい値）。
    Ramp(Vec<Color>),
    Fill(Color),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LegendItem {
    pub label: String,
    pub swatch: Swatch,
}

/// 数値の表。`numeric_from` 列目からを数字として右に寄せる。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    pub head: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub numeric_from: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Chart(Chart),
    /// 画像と、その下の凡例。
    Image {
        image: Rgba,
        legend: Vec<LegendItem>,
    },
    Table(Table),
    /// 図の説明文。論文のテーマでは図の番号を付ける。
    Caption(String),
}

/// 1 枚の図。
#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    pub title: String,
    /// 元のファイル名・画像の大きさ・書き出した日時など。` ・ ` でつないで 1 行にする。
    pub meta: Vec<String>,
    pub blocks: Vec<Block>,
}

const PAD: f32 = 22.0;
const TITLE: f32 = 15.0;
const META: f32 = 10.0;
const BODY: f32 = 11.0;
const TICK: f32 = 10.0;
const LEGEND_ROW: f32 = 18.0;
const PLOT_HEIGHT: f32 = 190.0;
const TABLE_ROW: f32 = 17.0;

impl Sheet {
    /// 幅 `width` に並べて命令を記録し、高さを返す。
    pub fn layout(&self, r: &mut Recorder<'_>, theme: Theme, words: &Words, width: f32) -> f32 {
        let inner = width - 2.0 * PAD;
        r.rect([0.0, 0.0, width, 0.0], Some(theme.background()), None);
        let mut y = PAD + TITLE;
        for line in r.wrap(&self.title, TITLE, Face::Body, inner) {
            r.text(line, [PAD, y], TITLE, theme.heading());
            y += TITLE * 1.35;
        }
        if !self.meta.is_empty() {
            let face = if theme == Theme::Lab {
                Face::Mono
            } else {
                Face::Body
            };
            for line in r.wrap(&self.meta.join(words.separator), META, face, inner) {
                r.text(line, [PAD, y], META, theme.muted()).face(face);
                y += META * 1.5;
            }
        }
        y += 6.0;
        let mut figure = 0;
        for block in &self.blocks {
            y = match block {
                Block::Chart(c) => chart(r, theme, c, [PAD, y], inner),
                Block::Image { image, legend } => {
                    let w = inner.min(image.width as f32);
                    let h = w * image.height as f32 / image.width.max(1) as f32;
                    r.ops.push(crate::canvas::Op::Image {
                        rect: [PAD, y, w, h],
                        image: image.clone(),
                    });
                    let y = y + h + 8.0;
                    legend_rows(r, theme, legend, [PAD, y], inner)
                }
                Block::Table(t) => table(r, theme, t, [PAD, y], inner),
                Block::Caption(text) => {
                    figure += 1;
                    let text = caption(theme, words, figure, text);
                    let mut y = y + 2.0;
                    for line in r.wrap(&text, BODY, Face::Body, inner) {
                        y += BODY * 1.2;
                        r.text(line, [PAD, y], BODY, theme.muted());
                        y += BODY * 0.35;
                    }
                    y
                }
            } + 10.0;
        }
        let height = y + PAD - 10.0;
        if let Some(crate::canvas::Op::Rect { rect, .. }) = r.ops.first_mut() {
            rect[3] = height;
        }
        height
    }
}

/// 図の番号を付けた説明文。
pub fn caption(theme: Theme, words: &Words, number: usize, text: &str) -> String {
    if theme.numbers_figures() {
        format!("{} {number}{}{text}", words.figure, words.number_gap)
    } else {
        text.to_owned()
    }
}

/// グラフ（凡例・各段・横軸）を描き、下の端の y を返す。
pub fn chart(r: &mut Recorder<'_>, theme: Theme, c: &Chart, at: [f32; 2], width: f32) -> f32 {
    let x_range = range(
        c.panels
            .iter()
            .flat_map(|p| p.series.iter())
            .flat_map(|s| s.points.iter().map(|p| p[0])),
    );
    let x_ticks = if c.x_names.is_empty() {
        ticks(x_range, 7)
    } else {
        c.x_names.clone()
    };
    let x_range = span(&x_ticks, x_range);
    let mut y = at[1];
    let left_room = c
        .panels
        .iter()
        .map(|p| {
            let yr = range(p.series.iter().flat_map(|s| s.points.iter().map(|q| q[1])));
            ticks(yr, 5)
                .iter()
                .map(|t| r.width(&t.1, TICK, Face::Body))
                .fold(0.0, f32::max)
        })
        .fold(0.0, f32::max);
    let plot_left = at[0] + 18.0 + left_room + 8.0;
    // 右端の目盛りの数字は中央揃えなので、その半分が図の外にはみ出さないだけ空ける。
    let right_room = x_ticks
        .last()
        .map_or(0.0, |t| r.width(&t.1, TICK, Face::Body) / 2.0);
    let plot_right = at[0] + width - right_room.max(8.0) - 2.0;
    for (i, p) in c.panels.iter().enumerate() {
        let items: Vec<LegendItem> = p
            .series
            .iter()
            .map(|s| LegendItem {
                label: s.name.clone(),
                swatch: Swatch::Line(s.style),
            })
            .collect();
        y = legend_rows(r, theme, &items, [plot_left, y], plot_right - plot_left) + 2.0;
        let plot = [plot_left, y, plot_right - plot_left, PLOT_HEIGHT];
        let y_raw = range(p.series.iter().flat_map(|s| s.points.iter().map(|q| q[1])));
        let y_ticks = ticks(y_raw, 5);
        let y_range = span(&y_ticks, y_raw);
        axes(r, theme, plot, &x_ticks, x_range, &y_ticks, y_range);
        let sx = |v: f64| plot[0] + ((v - x_range.0) / (x_range.1 - x_range.0)) as f32 * plot[2];
        let sy =
            |v: f64| plot[1] + (1.0 - ((v - y_range.0) / (y_range.1 - y_range.0)) as f32) * plot[3];
        for s in &p.series {
            let mut run: Vec<[f32; 2]> = Vec::new();
            for q in &s.points {
                if q[1].is_finite() {
                    run.push([sx(q[0]), sy(q[1])]);
                } else {
                    r.line(
                        std::mem::take(&mut run),
                        s.style.color,
                        theme.line_width(),
                        s.style.dash,
                    );
                }
            }
            r.line(run, s.style.color, theme.line_width(), s.style.dash);
            if let Some(m) = s.style.marker {
                let every = (s.points.len() / 12).max(1);
                for q in s.points.iter().step_by(every).filter(|q| q[1].is_finite()) {
                    marker(r, theme, m, [sx(q[0]), sy(q[1])], s.style.color);
                }
            }
        }
        r.text(
            p.y_label.clone(),
            [at[0] + 10.0, plot[1] + plot[3] / 2.0],
            BODY * 0.95,
            theme.ink(),
        )
        .anchor(Anchor::Middle)
        .vertical();
        y = plot[1] + plot[3] + TICK + 8.0;
        if i + 1 < c.panels.len() {
            y += 10.0;
        }
    }
    r.text(
        c.x_label.clone(),
        [(plot_left + plot_right) / 2.0, y + BODY + 2.0],
        BODY * 0.95,
        theme.ink(),
    )
    .anchor(Anchor::Middle);
    y + BODY + 6.0
}

fn axes(
    r: &mut Recorder<'_>,
    theme: Theme,
    plot: [f32; 4],
    x_ticks: &[(f64, String)],
    x_range: (f64, f64),
    y_ticks: &[(f64, String)],
    y_range: (f64, f64),
) {
    let [x0, y0, w, h] = plot;
    let (x1, y1) = (x0 + w, y0 + h);
    let sx = |v: f64| x0 + ((v - x_range.0) / (x_range.1 - x_range.0)) as f32 * w;
    let sy = |v: f64| y0 + (1.0 - ((v - y_range.0) / (y_range.1 - y_range.0)) as f32) * h;
    for (v, label) in y_ticks {
        let y = sy(*v);
        if let Some(g) = theme.grid() {
            r.line(vec![[x0, y], [x1, y]], g, 1.0, &[]);
        }
        r.line(vec![[x0 - 4.0, y], [x0, y]], theme.axis(), 1.0, &[]);
        r.text(label.clone(), [x0 - 7.0, y + 3.5], TICK, theme.muted())
            .anchor(Anchor::End);
    }
    for (v, label) in x_ticks {
        let x = sx(*v);
        if let Some(g) = theme.grid() {
            r.line(vec![[x, y0], [x, y1]], g, 1.0, &[]);
        }
        r.line(vec![[x, y1], [x, y1 + 4.0]], theme.axis(), 1.0, &[]);
        r.text(label.clone(), [x, y1 + 4.0 + TICK], TICK, theme.muted())
            .anchor(Anchor::Middle);
    }
    if theme.framed() {
        r.rect(plot, None, Some((theme.axis(), 1.0)));
    } else {
        r.line(vec![[x0, y1], [x1, y1]], theme.axis(), 1.0, &[]);
        r.line(vec![[x0, y0], [x0, y1]], theme.axis(), 1.0, &[]);
    }
}

fn marker(r: &mut Recorder<'_>, theme: Theme, m: Marker, [x, y]: [f32; 2], color: Color) {
    let open = Some(theme.background());
    let op = match m {
        Marker::Circle => crate::canvas::Op::Circle {
            center: [x, y],
            radius: 2.6,
            fill: open,
            stroke: Some((color, 1.1)),
        },
        Marker::Square => crate::canvas::Op::Rect {
            rect: [x - 2.4, y - 2.4, 4.8, 4.8],
            fill: Some(color),
            stroke: None,
        },
        Marker::Triangle => crate::canvas::Op::Polygon {
            points: vec![[x, y - 3.0], [x + 2.8, y + 2.2], [x - 2.8, y + 2.2]],
            fill: open,
            stroke: Some((color, 1.0)),
        },
        Marker::Diamond => crate::canvas::Op::Polygon {
            points: vec![[x, y - 3.2], [x + 3.0, y], [x, y + 3.2], [x - 3.0, y]],
            fill: Some(color),
            stroke: None,
        },
    };
    r.ops.push(op);
}

/// 凡例を左から並べ、入らなければ次の行に送る。下の端の y を返す。
pub fn legend_rows(
    r: &mut Recorder<'_>,
    theme: Theme,
    items: &[LegendItem],
    at: [f32; 2],
    width: f32,
) -> f32 {
    if items.is_empty() {
        return at[1];
    }
    let (mut x, mut y) = (at[0], at[1] + LEGEND_ROW * 0.5);
    for item in items {
        let w = 22.0 + 6.0 + r.width(&item.label, BODY, Face::Body);
        if x > at[0] && x + w > at[0] + width {
            x = at[0];
            y += LEGEND_ROW;
        }
        match &item.swatch {
            Swatch::Line(s) => {
                r.line(
                    vec![[x, y], [x + 22.0, y]],
                    s.color,
                    theme.line_width() + 0.4,
                    s.dash,
                );
                if let Some(m) = s.marker {
                    marker(r, theme, m, [x + 11.0, y], s.color);
                }
            }
            Swatch::Fill(c) => r.rect(
                [x + 4.0, y - 5.0, 14.0, 10.0],
                Some(*c),
                Some((theme.rule(), 0.75)),
            ),
            Swatch::Ramp(colors) => {
                let step = 22.0 / colors.len().max(1) as f32;
                for (i, c) in colors.iter().enumerate() {
                    r.rect(
                        [x + i as f32 * step, y - 5.0, step + 0.3, 10.0],
                        Some(*c),
                        None,
                    );
                }
                r.rect([x, y - 5.0, 22.0, 10.0], None, Some((theme.rule(), 0.75)));
            }
        }
        r.text(item.label.clone(), [x + 28.0, y + 3.8], BODY, theme.ink());
        x += w + 16.0;
    }
    y + LEGEND_ROW * 0.5
}

/// 表を描き、下の端の y を返す。列の幅は中身の幅に合わせ、余りは最初の列に足す。
pub fn table(r: &mut Recorder<'_>, theme: Theme, t: &Table, at: [f32; 2], width: f32) -> f32 {
    let size = BODY * 0.95;
    let columns = t
        .head
        .len()
        .max(t.rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return at[1];
    }
    let face_of = |c: usize| {
        if c >= t.numeric_from && theme.mono_numbers() {
            Face::Mono
        } else {
            Face::Body
        }
    };
    let mut widths = vec![0.0f32; columns];
    for row in std::iter::once(&t.head).chain(&t.rows) {
        for (c, cell) in row.iter().enumerate() {
            widths[c] = widths[c].max(r.width(cell, size, face_of(c)) + 14.0);
        }
    }
    let total: f32 = widths.iter().sum();
    if total < width {
        widths[0] += width - total;
    } else {
        let k = width / total;
        widths.iter_mut().for_each(|w| *w *= k);
    }
    let right = at[0] + width;
    let mut y = at[1];
    let heavy = if theme == Theme::Paper { 1.5 } else { 1.0 };
    if theme == Theme::Paper {
        r.line(vec![[at[0], y], [right, y]], theme.ink(), heavy, &[]);
    }
    if let Some(fill) = theme.table_head() {
        r.rect([at[0], y, width, TABLE_ROW], Some(fill), None);
    }
    let cells = |r: &mut Recorder<'_>, row: &[String], y: f32, color: Color, head: bool| {
        let mut x = at[0];
        for (c, cell) in row.iter().enumerate() {
            let numeric = c >= t.numeric_from && c > 0;
            let face = if head { Face::Body } else { face_of(c) };
            let (px, anchor) = if numeric {
                (x + widths[c] - 6.0, Anchor::End)
            } else {
                (x + 6.0, Anchor::Start)
            };
            r.text(cell.clone(), [px, y + TABLE_ROW - 5.0], size, color)
                .anchor(anchor)
                .face(face);
            x += widths[c];
        }
    };
    let head_color = if theme == Theme::Lab {
        theme.heading()
    } else if theme == Theme::Dark {
        theme.muted()
    } else {
        theme.ink()
    };
    cells(r, &t.head, y, head_color, true);
    y += TABLE_ROW;
    let head_rule = match theme {
        Theme::Paper => Some((theme.ink(), 0.75, &[][..])),
        Theme::Mono | Theme::Dark => Some((theme.rule(), 1.0, &[][..])),
        Theme::Lab => None,
    };
    if let Some((c, w, d)) = head_rule {
        r.line(vec![[at[0], y], [right, y]], c, w, d);
    }
    for row in &t.rows {
        cells(r, row, y, theme.ink(), false);
        y += TABLE_ROW;
        match theme {
            Theme::Lab | Theme::Dark => {
                let c = if theme == Theme::Dark {
                    Color::rgb(0x2C2F33)
                } else {
                    theme.rule()
                };
                r.line(vec![[at[0], y], [right, y]], c, 1.0, &[]);
            }
            Theme::Mono => r.line(
                vec![[at[0], y], [right, y]],
                Color::rgb(0x888888),
                0.5,
                &[1.0, 1.5],
            ),
            Theme::Paper => {}
        }
    }
    if theme == Theme::Paper {
        r.line(vec![[at[0], y], [right, y]], theme.ink(), heavy, &[]);
    }
    y
}

/// 値の範囲。値がなければ 0〜1、1 つの値だけなら前後に 1 ずつ広げる。
fn range(values: impl Iterator<Item = f64>) -> (f64, f64) {
    let (lo, hi) = values
        .filter(|v| v.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    if lo > hi {
        (0.0, 1.0)
    } else if lo == hi {
        (lo - 1.0, hi + 1.0)
    } else {
        (lo, hi)
    }
}

/// 目盛りの範囲（最初と最後の目盛り）を、値の範囲が入るように広げたもの。
fn span(ticks: &[(f64, String)], raw: (f64, f64)) -> (f64, f64) {
    match (ticks.first(), ticks.last()) {
        (Some(a), Some(b)) if a.0 < b.0 => (a.0.min(raw.0), b.0.max(raw.1)),
        _ => raw,
    }
}

/// 1・2・5 の刻みの目盛りを、およそ `count` 本。値の範囲を外から挟む。
pub fn ticks((lo, hi): (f64, f64), count: usize) -> Vec<(f64, String)> {
    let raw = (hi - lo) / count.max(1) as f64;
    if !(raw.is_finite() && raw > 0.0) {
        return Vec::new();
    }
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|k| k * mag)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * mag);
    let decimals = (-step.log10().floor()).max(0.0) as usize;
    let first = (lo / step).floor() as i64;
    let last = (hi / step).ceil() as i64;
    // 桁区切りは目盛りのどれかが 1 万を超えるときに全部へ付け、5000 と 10,000 が並ばないようにする。
    let group = (first.abs().max(last.abs()) as f64 * step) >= 10_000.0;
    (first..=last)
        .map(|k| {
            let v = k as f64 * step;
            (v, number(v, decimals, group))
        })
        .collect()
}

fn number(v: f64, decimals: usize, group: bool) -> String {
    let s = format!("{:.*}", decimals, if v == 0.0 { 0.0 } else { v });
    if decimals == 0 && group {
        group_thousands(&s)
    } else {
        s
    }
}

fn group_thousands(s: &str) -> String {
    let (sign, digits) = s.strip_prefix('-').map_or(("", s), |d| ("-", d));
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{Estimate, Op};

    fn sheet() -> Sheet {
        let series = |name: &str, i: usize| Series {
            name: name.to_owned(),
            points: (0..50).map(|x| [f64::from(x), f64::from(x * x)]).collect(),
            style: Theme::Lab.line(i),
        };
        Sheet {
            title: "Values along the lines".to_owned(),
            meta: vec!["a.png".to_owned(), "64 × 64".to_owned()],
            blocks: vec![
                Block::Chart(Chart {
                    x_label: "Distance from the start (px)".to_owned(),
                    x_names: Vec::new(),
                    panels: vec![Panel {
                        y_label: "Pixel value".to_owned(),
                        series: vec![series("Line 1", 0), series("Line 2", 1)],
                    }],
                }),
                Block::Table(Table {
                    head: vec!["Line".into(), "Min".into()],
                    rows: vec![vec!["Line 1".into(), "0".into()]],
                    numeric_from: 1,
                }),
                Block::Caption("Values from start to end.".to_owned()),
            ],
        }
    }

    fn texts(ops: &[Op]) -> Vec<&str> {
        ops.iter()
            .filter_map(|o| match o {
                Op::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ticks_step_by_one_two_or_five() {
        let t: Vec<String> = ticks((0.0, 45000.0), 5).into_iter().map(|t| t.1).collect();
        assert_eq!(t, ["0", "10,000", "20,000", "30,000", "40,000", "50,000"]);
        let t: Vec<String> = ticks((0.0, 1.0), 5).into_iter().map(|t| t.1).collect();
        assert_eq!(t, ["0.0", "0.2", "0.4", "0.6", "0.8", "1.0"]);
        let t: Vec<String> = ticks((-1000.0, 200.0), 5)
            .into_iter()
            .map(|t| t.1)
            .collect();
        assert_eq!(t, ["-1000", "-500", "0", "500"]);
        let t: Vec<String> = ticks((-15000.0, 5000.0), 4)
            .into_iter()
            .map(|t| t.1)
            .collect();
        assert_eq!(t, ["-15,000", "-10,000", "-5,000", "0", "5,000"]);
    }

    #[test]
    fn a_sheet_carries_title_meta_legend_axis_names_table_and_caption() {
        let mut r = Recorder::new(&Estimate);
        let h = sheet().layout(&mut r, Theme::Lab, &Words::ENGLISH, 640.0);
        assert!(h > PLOT_HEIGHT);
        let t = texts(&r.ops);
        for want in [
            "Values along the lines",
            "a.png · 64 × 64",
            "Line 1",
            "Line 2",
            "Pixel value",
            "Distance from the start (px)",
            "Min",
            "Values from start to end.",
        ] {
            assert!(t.contains(&want), "{want} in {t:?}");
        }
    }

    #[test]
    fn the_paper_theme_numbers_figures() {
        let mut r = Recorder::new(&Estimate);
        sheet().layout(&mut r, Theme::Paper, &Words::JAPANESE, 640.0);
        assert!(texts(&r.ops).contains(&"図 1　Values from start to end."));
    }

    #[test]
    fn the_background_covers_the_whole_sheet() {
        let mut r = Recorder::new(&Estimate);
        let h = sheet().layout(&mut r, Theme::Dark, &Words::ENGLISH, 640.0);
        assert_eq!(
            r.ops[0],
            Op::Rect {
                rect: [0.0, 0.0, 640.0, h],
                fill: Some(Theme::Dark.background()),
                stroke: None
            }
        );
    }

    #[test]
    fn missing_values_break_the_line() {
        let mut r = Recorder::new(&Estimate);
        let chart_with_gap = Chart {
            x_label: String::new(),
            x_names: Vec::new(),
            panels: vec![Panel {
                y_label: String::new(),
                series: vec![Series {
                    name: "a".into(),
                    points: vec![
                        [0.0, 1.0],
                        [1.0, 2.0],
                        [2.0, f64::NAN],
                        [3.0, 1.0],
                        [4.0, 3.0],
                    ],
                    style: Theme::Paper.line(0),
                }],
            }],
        };
        chart(&mut r, Theme::Paper, &chart_with_gap, [0.0, 0.0], 400.0);
        let color = Theme::Paper.line(0).color;
        let runs = r
            .ops
            .iter()
            .filter(|o| matches!(o, Op::Line { color: c, width, .. } if *c == color && *width == Theme::Paper.line_width()))
            .count();
        assert_eq!(runs, 2);
    }

    #[test]
    fn monochrome_lines_get_markers() {
        let mut r = Recorder::new(&Estimate);
        let mut s = sheet();
        if let Block::Chart(c) = &mut s.blocks[0] {
            for (i, series) in c.panels[0].series.iter_mut().enumerate() {
                series.style = Theme::Mono.line(i);
            }
        }
        s.layout(&mut r, Theme::Mono, &Words::ENGLISH, 640.0);
        assert!(r.ops.iter().any(|o| matches!(o, Op::Circle { .. })));
    }
}
