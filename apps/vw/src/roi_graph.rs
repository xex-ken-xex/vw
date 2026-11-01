//! ROI（関心領域）の値のグラフ: 帯のプロファイル、ヒストグラム、フレームごとの平均。
//! 欄は [`crate::app`] にある。
//!
//! 値は計測線（[`crate::line_profile`]）と同じく元の画像から読み、画素の値と計測の値の関係も
//! 計測線の [`Values`] を使う。ROI の札（[`crate::roi::label`]）はカラーを輝度 1 つにまとめるが、
//! グラフは R・G・B を分けて出す（色ごとのムラや偏りを見たいので）。

use vw_core::{ColorModel, Metadata, VwImage};
use vw_report::{Chart, Panel, Series, Table, Theme};

use crate::histogram;
use crate::line_profile::{Values, channel_names, stats};
use crate::probe;
use crate::roi::{Kind, Region};

/// 欄で選ぶグラフの形。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// 矩形の幅（高さ）で平均を取りながら、横（縦）に沿った値。
    #[default]
    Band,
    Histogram,
    /// 並び（フレームやフォルダの画像）の 1 枚ごとの ROI の平均。
    Frames,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Band, Mode::Histogram, Mode::Frames];

    pub fn title(self) -> &'static str {
        match self {
            Mode::Band => "Band profile",
            Mode::Histogram => "Histogram",
            Mode::Frames => "Mean per frame",
        }
    }
}

/// 帯のプロファイルの向き。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// 左から右へ。各列で縦に平均する。
    #[default]
    Horizontal,
    /// 上から下へ。各行で横に平均する。
    Vertical,
}

/// グラフにする ROI。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub kind: Kind,
    pub region: Region,
}

/// 画像に収まる部分だけにした ROI。画像の外にはみ出す部分は読まない。
fn clip(region: Region, img: &VwImage) -> Option<Region> {
    let right = region.right().min(img.width);
    let bottom = region.bottom().min(img.height);
    (region.x < right && region.y < bottom).then(|| Region {
        x: region.x,
        y: region.y,
        w: right - region.x,
        h: bottom - region.y,
    })
}

/// 画素の読んだ値（チャンネルごと）。欠損は NaN。
fn read(img: &VwImage, meta: &Metadata, x: u32, y: u32, channels: usize) -> Vec<f64> {
    probe::sample(img, meta, x, y).unwrap_or_else(|| vec![f64::NAN; channels])
}

/// 平均を取る入れ物。欠損（NaN）は数えない。
#[derive(Clone, Default)]
struct Mean {
    sum: f64,
    n: usize,
}

impl Mean {
    fn add(&mut self, v: f64) {
        if v.is_finite() {
            self.sum += v;
            self.n += 1;
        }
    }

    fn get(&self) -> f64 {
        if self.n == 0 {
            f64::NAN
        } else {
            self.sum / self.n as f64
        }
    }
}

/// 画素の値と計測の値（モノクロで 2 種類あるときだけ）に直した、ひと組の値。
/// `read` はチャンネルごとの読んだ値。
fn split(read: &[f64], values: &Values) -> (Vec<f64>, Option<f64>) {
    let mono = read.len() == 1;
    let pixel = read
        .iter()
        .map(|&r| if mono { values.pixel(r) } else { r })
        .collect();
    let measured = (mono && values.two_kinds()).then(|| values.measured(read[0]));
    (pixel, measured)
}

/// 1 つの ROI の帯のプロファイル。
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub area: Area,
    pub direction: Direction,
    /// 横なら x、縦なら y（画素の番号）。
    pub positions: Vec<u32>,
    /// 色ごとの画素の値の平均。
    pub pixel: Vec<Vec<f64>>,
    pub measured: Option<Vec<f64>>,
}

pub fn band(
    img: &VwImage,
    meta: &Metadata,
    area: Area,
    direction: Direction,
    values: &Values,
) -> Band {
    let channels = channel_names(img.color).len();
    let mut positions = Vec::new();
    let mut pixel = vec![Vec::new(); channels];
    let mut measured = Vec::new();
    if let Some(r) = clip(area.region, img) {
        let (along, across) = match direction {
            Direction::Horizontal => (r.x..r.right(), r.y..r.bottom()),
            Direction::Vertical => (r.y..r.bottom(), r.x..r.right()),
        };
        for p in along {
            let mut means = vec![Mean::default(); channels];
            for q in across.clone() {
                let (x, y) = match direction {
                    Direction::Horizontal => (p, q),
                    Direction::Vertical => (q, p),
                };
                for (m, v) in means.iter_mut().zip(read(img, meta, x, y, channels)) {
                    m.add(v);
                }
            }
            let read: Vec<f64> = means.iter().map(Mean::get).collect();
            let (px, ms) = split(&read, values);
            for (column, v) in pixel.iter_mut().zip(px) {
                column.push(v);
            }
            measured.extend(ms);
            positions.push(p);
        }
    }
    let two = channels == 1 && values.two_kinds();
    Band {
        area,
        direction,
        positions,
        pixel,
        measured: two.then_some(measured),
    }
}

/// ROI の中の全部の画素（色ごと）。画素の値と、2 種類ある画像では計測の値。
#[derive(Debug, Clone, PartialEq)]
pub struct Pixels {
    pub pixel: Vec<Vec<f64>>,
    pub measured: Option<Vec<f64>>,
}

pub fn pixels(img: &VwImage, meta: &Metadata, region: Region, values: &Values) -> Pixels {
    let channels = channel_names(img.color).len();
    let mut pixel = vec![Vec::new(); channels];
    let mut measured = Vec::new();
    if let Some(r) = clip(region, img) {
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                let (px, ms) = split(&read(img, meta, x, y, channels), values);
                for (column, v) in pixel.iter_mut().zip(px) {
                    column.push(v);
                }
                measured.extend(ms);
            }
        }
    }
    Pixels {
        pixel,
        measured: (channels == 1 && values.two_kinds()).then_some(measured),
    }
}

/// ヒストグラムの区切り。`edges` は区間の境（区間の数 + 1 個）。
#[derive(Debug, Clone, PartialEq)]
pub struct Bins {
    pub edges: Vec<f64>,
}

impl Bins {
    /// `min`〜`max` を `count` 個に分ける。整数の値で幅が `count` 以下なら、1 つの値に 1 区間
    /// （境は値の ±0.5）にする。整数を幅 1 未満で区切ると、空の区間と山が交互に並んでしまうため。
    pub fn new(min: f64, max: f64, count: usize, integers: bool) -> Self {
        let count = count.max(1);
        if !(min.is_finite() && max.is_finite()) {
            return Self { edges: vec![] };
        }
        let span = max - min;
        let edges = if integers && span + 1.0 <= count as f64 {
            (0..=span as usize + 1)
                .map(|i| min - 0.5 + i as f64)
                .collect()
        } else if span == 0.0 {
            vec![min - 0.5, min + 0.5]
        } else {
            (0..=count)
                .map(|i| min + span * i as f64 / count as f64)
                .collect()
        };
        Self { edges }
    }

    pub fn len(&self) -> usize {
        self.edges.len().saturating_sub(1)
    }

    /// 値の入る区間。範囲の外と欠損は `None`。最後の区間は右の境も含む。
    pub fn index(&self, v: f64) -> Option<usize> {
        let n = self.len();
        if n == 0 || !v.is_finite() || v < self.edges[0] || v > self.edges[n] {
            return None;
        }
        let i = self.edges.partition_point(|&e| e <= v);
        Some(i.saturating_sub(1).min(n - 1))
    }

    pub fn count(&self, values: &[f64]) -> Vec<u64> {
        let mut counts = vec![0; self.len()];
        for &v in values {
            if let Some(i) = self.index(v) {
                counts[i] += 1;
            }
        }
        counts
    }
}

/// 全部の ROI に共通の区切り（ROI どうしを同じ横軸で比べるため）。
pub fn common_bins(sets: &[&[f64]], count: usize) -> Bins {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut integers = true;
    for &v in sets.iter().flat_map(|s| s.iter()).filter(|v| v.is_finite()) {
        min = min.min(v);
        max = max.max(v);
        integers &= v.fract() == 0.0;
    }
    Bins::new(min, max, count, integers)
}

/// 1 枚（フレーム）の ROI の平均。
#[derive(Debug, Clone, PartialEq)]
pub struct FrameMean {
    /// 並びの中の位置（0 から）。
    pub index: usize,
    pub pixel: Vec<f64>,
    pub measured: Option<f64>,
}

/// `img` の `region` の平均。フレームごとに `values` を作り直す（PET はフレームごとに SUV の
/// 換算が変わりうる）。
pub fn frame_mean(img: &VwImage, meta: &Metadata, region: Region, index: usize) -> FrameMean {
    let values = Values::of(meta);
    let channels = channel_names(img.color).len();
    let mut means = vec![Mean::default(); channels];
    if let Some(r) = clip(region, img) {
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                for (m, v) in means.iter_mut().zip(read(img, meta, x, y, channels)) {
                    m.add(v);
                }
            }
        }
    }
    let read: Vec<f64> = means.iter().map(Mean::get).collect();
    let (pixel, measured) = split(&read, &values);
    FrameMean {
        index,
        pixel,
        measured,
    }
}

/// 図と表の言葉（画面の言語）。
pub struct Labels<'a> {
    pub kind_name: fn(Kind) -> &'a str,
    pub position_x: &'a str,
    pub position_y: &'a str,
    pub pixel_value: &'a str,
    pub measured_value: &'a str,
    pub mean_pixel: &'a str,
    pub mean_measured: &'a str,
    pub count: &'a str,
    pub frame: &'a str,
    pub roi: &'a str,
    pub place: &'a str,
    pub frames: &'a str,
    pub kind: &'a str,
    pub min: &'a str,
    pub max: &'a str,
    pub mean: &'a str,
    pub sd: &'a str,
}

fn with_unit(name: &str, unit: &str) -> String {
    if unit.is_empty() {
        name.to_owned()
    } else {
        format!("{name} ({unit})")
    }
}

fn series_name(labels: &Labels<'_>, kind: Kind, channel: &str) -> String {
    let name = (labels.kind_name)(kind);
    if channel.is_empty() {
        name.to_owned()
    } else {
        format!("{name} {channel}")
    }
}

/// ROI の色の番号。対象を 1 本目、背景を 2 本目の色にする（ROI の数で色がずれないように）。
fn style_index(kind: Kind) -> usize {
    match kind {
        Kind::Target => 0,
        Kind::Background => 1,
    }
}

/// ROI ごとの 1 本ぶんの値: 種類, 横軸, 色ごとの画素の値, 計測の値。
type Line = (Kind, Vec<f64>, Vec<Vec<f64>>, Option<Vec<f64>>);

/// ROI ごとの表の 1 行ぶん: 種類, 2 列目の文字, 色ごとの画素の値, 計測の値。
pub type Row = (Kind, String, Vec<Vec<f64>>, Option<Vec<f64>>);

/// 画素の値の段と、2 種類ある画像では計測の値の段。
fn two_panels(
    series: &[Line],
    color: ColorModel,
    values: &Values,
    labels: &Labels<'_>,
    theme: Theme,
    (pixel_label, measured_label): (&str, &str),
) -> Vec<Panel> {
    let names = channel_names(color);
    let points = |x: &[f64], y: &[f64]| -> Vec<[f64; 2]> {
        x.iter().zip(y).map(|(&x, &y)| [x, y]).collect()
    };
    let mut pixel = Vec::new();
    for (kind, x, columns, _) in series {
        let i = style_index(*kind);
        for (c, column) in columns.iter().enumerate() {
            pixel.push(Series {
                name: series_name(labels, *kind, names[c]),
                points: points(x, column),
                style: if names.len() > 1 {
                    theme.channel(c, i)
                } else {
                    theme.line(i)
                },
            });
        }
    }
    let mut panels = vec![Panel {
        y_label: pixel_label.to_owned(),
        series: pixel,
    }];
    if values.two_kinds() && names.len() == 1 {
        panels.push(Panel {
            y_label: with_unit(measured_label, &values.unit),
            series: series
                .iter()
                .filter_map(|(kind, x, _, m)| {
                    Some(Series {
                        name: series_name(labels, *kind, ""),
                        points: points(x, m.as_ref()?),
                        style: theme.line(style_index(*kind)),
                    })
                })
                .collect(),
        });
    } else if !values.unit.is_empty() {
        panels[0].y_label = with_unit(pixel_label, &values.unit);
    }
    panels
}

pub fn band_chart(
    bands: &[Band],
    color: ColorModel,
    values: &Values,
    labels: &Labels<'_>,
    theme: Theme,
) -> Chart {
    let series: Vec<_> = bands
        .iter()
        .map(|b| {
            let x = b.positions.iter().map(|&p| f64::from(p)).collect();
            (b.area.kind, x, b.pixel.clone(), b.measured.clone())
        })
        .collect();
    let direction = bands.first().map_or(Direction::Horizontal, |b| b.direction);
    Chart {
        x_label: format!(
            "{} (px)",
            match direction {
                Direction::Horizontal => labels.position_x,
                Direction::Vertical => labels.position_y,
            }
        ),
        panels: two_panels(
            &series,
            color,
            values,
            labels,
            theme,
            (labels.pixel_value, labels.measured_value),
        ),
    }
}

/// ROI ごとの画素の数の段。横軸は画素の値（`measured` なら計測の値）。
pub fn histogram_chart(
    areas: &[(Area, Pixels)],
    bins: &Bins,
    color: ColorModel,
    values: &Values,
    labels: &Labels<'_>,
    theme: Theme,
    measured: bool,
) -> Chart {
    let names = channel_names(color);
    let mut series = Vec::new();
    for (area, px) in areas {
        let i = style_index(area.kind);
        let columns: Vec<(&str, &[f64])> = if measured {
            px.measured
                .as_deref()
                .map(|m| vec![("", m)])
                .unwrap_or_default()
        } else {
            px.pixel
                .iter()
                .enumerate()
                .map(|(c, v)| (names[c], v.as_slice()))
                .collect()
        };
        for (c, (channel, v)) in columns.into_iter().enumerate() {
            let counts = bins.count(v);
            series.push(Series {
                name: series_name(labels, area.kind, channel),
                points: steps(bins, &counts),
                style: if names.len() > 1 && !measured {
                    theme.channel(c, i)
                } else {
                    theme.line(i)
                },
            });
        }
    }
    let x_label = if measured {
        with_unit(labels.measured_value, &values.unit)
    } else if values.two_kinds() || values.unit.is_empty() {
        labels.pixel_value.to_owned()
    } else {
        with_unit(labels.pixel_value, &values.unit)
    };
    Chart {
        x_label,
        panels: vec![Panel {
            y_label: labels.count.to_owned(),
            series,
        }],
    }
}

/// 区間ごとの数を、段の形の折れ線にする（区間の幅が分かるように）。
fn steps(bins: &Bins, counts: &[u64]) -> Vec<[f64; 2]> {
    let mut points = Vec::with_capacity(counts.len() * 2);
    for (i, &c) in counts.iter().enumerate() {
        points.push([bins.edges[i], c as f64]);
        points.push([bins.edges[i + 1], c as f64]);
    }
    points
}

/// 計測の値の段の区切り。画素の値の区切りの境を、計測の値に直したもの。
pub fn measured_bins(pixel: &Bins, values: &Values) -> Bins {
    let mut edges: Vec<f64> = pixel
        .edges
        .iter()
        .map(|&e| values.measured(values.read_of(e)))
        .collect();
    if edges.first() > edges.last() {
        edges.reverse();
    }
    Bins { edges }
}

pub fn frames_chart(
    series: &[(Kind, Vec<FrameMean>)],
    color: ColorModel,
    values: &Values,
    labels: &Labels<'_>,
    theme: Theme,
) -> Chart {
    let channels = channel_names(color).len();
    let rows: Vec<_> = series
        .iter()
        .map(|(kind, means)| {
            let x = means.iter().map(|m| (m.index + 1) as f64).collect();
            let pixel = (0..channels)
                .map(|c| {
                    means
                        .iter()
                        .map(|m| m.pixel.get(c).copied().unwrap_or(f64::NAN))
                        .collect()
                })
                .collect();
            let measured = means
                .iter()
                .map(|m| m.measured)
                .collect::<Option<Vec<f64>>>();
            (*kind, x, pixel, measured)
        })
        .collect();
    Chart {
        x_label: labels.frame.to_owned(),
        panels: two_panels(
            &rows,
            color,
            values,
            labels,
            theme,
            (labels.mean_pixel, labels.mean_measured),
        ),
    }
}

/// ROI ごと・値の種類ごとの最小・最大・平均・標準偏差の表。`rows` は
/// （種類, 2 列目の文字, 色ごとの画素の値, 計測の値）。
pub fn table(
    rows: &[Row],
    color: ColorModel,
    values: &Values,
    labels: &Labels<'_>,
    second: &str,
) -> Table {
    let names = channel_names(color);
    let n = histogram::number;
    let mut out = Vec::new();
    for (kind, place, pixel, measured) in rows {
        let mut kinds: Vec<(String, &[f64])> = pixel
            .iter()
            .enumerate()
            .map(|(c, v)| {
                let name = if names[c].is_empty() {
                    if values.two_kinds() {
                        labels.pixel_value.to_owned()
                    } else {
                        with_unit(labels.pixel_value, &values.unit)
                    }
                } else {
                    format!("{} {}", labels.pixel_value, names[c])
                };
                (name, v.as_slice())
            })
            .collect();
        if let Some(m) = measured {
            kinds.push((with_unit(labels.measured_value, &values.unit), m.as_slice()));
        }
        for (name, v) in kinds {
            let s = stats(v);
            let cell = |i: usize| s.map_or_else(String::new, |s| n(s[i]));
            out.push(vec![
                (labels.kind_name)(*kind).to_owned(),
                place.clone(),
                name,
                cell(0),
                cell(1),
                cell(2),
                cell(3),
            ]);
        }
    }
    Table {
        head: vec![
            labels.roi.to_owned(),
            second.to_owned(),
            labels.kind.to_owned(),
            labels.min.to_owned(),
            labels.max.to_owned(),
            labels.mean.to_owned(),
            labels.sd.to_owned(),
        ],
        rows: out,
        numeric_from: 3,
    }
}

/// ROI の場所の文字（左上の x, y と幅×高さ）。
pub fn place(region: Region) -> String {
    format!("{}, {}  {}×{}", region.x, region.y, region.w, region.h)
}

/// 英語の ROI の名前（CSV の値。言語によらない）。
fn kind_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Target => "target",
        Kind::Background => "background",
    }
}

fn pixel_columns(color: ColorModel) -> Vec<String> {
    channel_names(color)
        .iter()
        .map(|c| {
            if c.is_empty() {
                "pixel_value".to_owned()
            } else {
                format!("pixel_{}", c.to_lowercase())
            }
        })
        .collect()
}

fn value(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        String::new()
    }
}

/// 表の文字の並びを CSV（`,`）かタブ区切りにする。列名は言語によらず英語。
struct Writer {
    separator: char,
    out: String,
}

impl Writer {
    fn new(separator: char, head: &[String]) -> Self {
        let mut w = Self {
            separator,
            out: String::new(),
        };
        w.row(head.to_vec());
        w
    }

    fn row(&mut self, cells: Vec<String>) {
        let cells: Vec<String> = if self.separator == ',' {
            cells.iter().map(|c| vw_report::csv_field(c)).collect()
        } else {
            cells
        };
        self.out.push_str(&cells.join(&self.separator.to_string()));
        self.out.push('\n');
    }
}

fn head(first: &[&str], color: ColorModel, measured: bool, last: &[&str]) -> Vec<String> {
    let mut h: Vec<String> = first.iter().map(|s| (*s).to_owned()).collect();
    h.extend(pixel_columns(color));
    if measured {
        h.extend(["measured_value".to_owned(), "measured_unit".to_owned()]);
    }
    h.extend(last.iter().map(|s| (*s).to_owned()));
    h
}

/// 帯のプロファイルの点ごとの値。
pub fn band_rows(bands: &[Band], color: ColorModel, values: &Values, separator: char) -> String {
    let measured = values.two_kinds() && channel_names(color).len() == 1;
    let mut w = Writer::new(
        separator,
        &head(&["roi", "direction", "position_px"], color, measured, &[]),
    );
    for b in bands {
        let direction = match b.direction {
            Direction::Horizontal => "x",
            Direction::Vertical => "y",
        };
        for (i, p) in b.positions.iter().enumerate() {
            let mut row = vec![
                kind_key(b.area.kind).to_owned(),
                direction.to_owned(),
                p.to_string(),
            ];
            row.extend(b.pixel.iter().map(|c| value(c[i])));
            if measured {
                row.push(
                    b.measured
                        .as_ref()
                        .map_or_else(String::new, |m| value(m[i])),
                );
                row.push(values.unit.clone());
            }
            w.row(row);
        }
    }
    w.out
}

/// ヒストグラムの区間ごとの数。画素の値の区間と、2 種類ある画像では計測の値の区間。
pub fn histogram_rows(
    areas: &[(Area, Pixels)],
    bins: &Bins,
    color: ColorModel,
    values: &Values,
    separator: char,
) -> String {
    let names = channel_names(color);
    let measured = values.two_kinds() && names.len() == 1;
    let mut h = vec![
        "roi".to_owned(),
        "bin".to_owned(),
        "pixel_from".to_owned(),
        "pixel_to".to_owned(),
    ];
    if measured {
        h.extend([
            "measured_from".to_owned(),
            "measured_to".to_owned(),
            "measured_unit".to_owned(),
        ]);
    }
    h.extend(names.iter().map(|c| {
        if c.is_empty() {
            "count".to_owned()
        } else {
            format!("count_{}", c.to_lowercase())
        }
    }));
    let mut w = Writer::new(separator, &h);
    for (area, px) in areas {
        let counts: Vec<Vec<u64>> = px.pixel.iter().map(|v| bins.count(v)).collect();
        for i in 0..bins.len() {
            let mut row = vec![
                kind_key(area.kind).to_owned(),
                i.to_string(),
                value(bins.edges[i]),
                value(bins.edges[i + 1]),
            ];
            if measured {
                let (a, b) = (
                    values.measured(values.read_of(bins.edges[i])),
                    values.measured(values.read_of(bins.edges[i + 1])),
                );
                row.extend([value(a.min(b)), value(a.max(b)), values.unit.clone()]);
            }
            row.extend(counts.iter().map(|c| c[i].to_string()));
            w.row(row);
        }
    }
    w.out
}

/// フレームごとの平均。`paths` は並びの中の位置ごとのファイル（フレーム）の名前。
pub fn frames_rows(
    series: &[(Kind, Vec<FrameMean>)],
    names: &[String],
    color: ColorModel,
    values: &Values,
    separator: char,
) -> String {
    let measured = values.two_kinds() && channel_names(color).len() == 1;
    let mut h = vec!["roi".to_owned(), "frame".to_owned()];
    h.extend(pixel_columns(color).iter().map(|c| format!("mean_{c}")));
    if measured {
        h.extend(["mean_measured_value".to_owned(), "measured_unit".to_owned()]);
    }
    h.push("file".to_owned());
    let mut w = Writer::new(separator, &h);
    for (kind, means) in series {
        for m in means {
            let mut row = vec![kind_key(*kind).to_owned(), (m.index + 1).to_string()];
            row.extend(m.pixel.iter().map(|&v| value(v)));
            if measured {
                row.push(m.measured.map_or_else(String::new, value));
                row.push(values.unit.clone());
            }
            row.push(names.get(m.index).cloned().unwrap_or_default());
            w.row(row);
        }
    }
    w.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_core::PixelData;

    fn target(x: u32, y: u32, w: u32, h: u32) -> Area {
        Area {
            kind: Kind::Target,
            region: Region { x, y, w, h },
        }
    }

    fn meta(group: &str, entries: &[(&str, &str)]) -> Metadata {
        let mut m = Metadata::default();
        for (k, v) in entries {
            m.push(group, *k, *v);
        }
        m
    }

    fn kind_name(kind: Kind) -> &'static str {
        kind.title()
    }

    const LABELS: Labels<'static> = Labels {
        kind_name,
        position_x: "Position x",
        position_y: "Position y",
        pixel_value: "Pixel value",
        measured_value: "Measured value",
        mean_pixel: "Mean pixel value",
        mean_measured: "Mean measured value",
        count: "Pixels",
        frame: "Frame",
        roi: "ROI",
        place: "Place",
        frames: "Frames",
        kind: "Values",
        min: "Min",
        max: "Max",
        mean: "Mean",
        sd: "SD",
    };

    /// 3×2 の 16bit。上の行 0,10,20、下の行 100,110,120。
    fn small() -> VwImage {
        VwImage::new(
            3,
            2,
            ColorModel::Gray,
            PixelData::U16(vec![0, 10, 20, 100, 110, 120]),
        )
        .unwrap()
    }

    #[test]
    fn a_horizontal_band_averages_each_column() {
        let m = Metadata::default();
        let b = band(
            &small(),
            &m,
            target(0, 0, 3, 2),
            Direction::Horizontal,
            &Values::of(&m),
        );
        assert_eq!(b.positions, [0, 1, 2]);
        assert_eq!(b.pixel, vec![vec![50.0, 60.0, 70.0]]);
        assert_eq!(b.measured, None);
    }

    #[test]
    fn a_vertical_band_averages_each_row() {
        let m = Metadata::default();
        let b = band(
            &small(),
            &m,
            target(0, 0, 3, 2),
            Direction::Vertical,
            &Values::of(&m),
        );
        assert_eq!(b.positions, [0, 1]);
        assert_eq!(b.pixel, vec![vec![10.0, 110.0]]);
    }

    #[test]
    fn the_part_of_a_roi_outside_the_image_is_left_out() {
        let m = Metadata::default();
        let b = band(
            &small(),
            &m,
            target(2, 0, 5, 5),
            Direction::Horizontal,
            &Values::of(&m),
        );
        assert_eq!(b.positions, [2]);
        assert_eq!(b.pixel, vec![vec![70.0]]);
    }

    #[test]
    fn missing_pixels_are_left_out_of_the_band_average() {
        let img =
            VwImage::new(1, 2, ColorModel::Gray, PixelData::F32(vec![f32::NAN, 4.0])).unwrap();
        let m = Metadata::default();
        let b = band(
            &img,
            &m,
            target(0, 0, 1, 2),
            Direction::Horizontal,
            &Values::of(&m),
        );
        assert_eq!(b.pixel, vec![vec![4.0]]);
    }

    #[test]
    fn ct_bands_give_pixel_values_and_hu() {
        let m = meta(
            "DICOM",
            &[
                ("Modality", "CT"),
                ("Rescale slope", "1"),
                ("Rescale intercept", "-1024"),
            ],
        );
        let img =
            VwImage::new(2, 1, ColorModel::Gray, PixelData::F32(vec![-1000.0, 40.0])).unwrap();
        let v = Values::of(&m);
        let b = band(&img, &m, target(0, 0, 2, 1), Direction::Horizontal, &v);
        assert_eq!(b.pixel, vec![vec![24.0, 1064.0]]);
        assert_eq!(b.measured, Some(vec![-1000.0, 40.0]));
        let c = band_chart(
            std::slice::from_ref(&b),
            ColorModel::Gray,
            &v,
            &LABELS,
            Theme::Lab,
        );
        assert_eq!(c.panels.len(), 2);
        assert_eq!(c.panels[1].y_label, "Measured value (HU)");
        assert_eq!(c.x_label, "Position x (px)");
        let csv = band_rows(&[b], ColorModel::Gray, &v, ',');
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "roi,direction,position_px,pixel_value,measured_value,measured_unit"
        );
        assert_eq!(lines[1], "target,x,0,24,-1000,HU");
    }

    #[test]
    fn color_bands_give_red_green_and_blue() {
        let img = VwImage::new(
            1,
            2,
            ColorModel::Rgb,
            PixelData::U8(vec![10, 20, 30, 30, 40, 50]),
        )
        .unwrap();
        let m = Metadata::default();
        let v = Values::of(&m);
        let b = band(&img, &m, target(0, 0, 1, 2), Direction::Horizontal, &v);
        assert_eq!(b.pixel, vec![vec![20.0], vec![30.0], vec![40.0]]);
        let c = band_chart(&[b], ColorModel::Rgb, &v, &LABELS, Theme::Lab);
        let names: Vec<&str> = c.panels[0].series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Target R", "Target G", "Target B"]);
    }

    #[test]
    fn small_integer_ranges_get_one_bin_per_value() {
        let b = Bins::new(3.0, 5.0, 64, true);
        assert_eq!(b.edges, [2.5, 3.5, 4.5, 5.5]);
        assert_eq!(b.count(&[3.0, 5.0, 5.0, f64::NAN, 9.0]), [1, 0, 2]);
    }

    #[test]
    fn wide_ranges_are_split_into_the_set_number_of_bins() {
        let b = Bins::new(0.0, 100.0, 4, true);
        assert_eq!(b.edges, [0.0, 25.0, 50.0, 75.0, 100.0]);
        assert_eq!(b.count(&[0.0, 24.9, 25.0, 100.0]), [2, 1, 0, 1]);
    }

    #[test]
    fn a_single_value_gets_one_bin_around_it() {
        let b = Bins::new(7.5, 7.5, 16, false);
        assert_eq!(b.edges, [7.0, 8.0]);
    }

    #[test]
    fn all_rois_share_the_same_bins() {
        let b = common_bins(&[&[0.0, 1.0], &[4.0]], 64);
        assert_eq!(b.edges.first(), Some(&-0.5));
        assert_eq!(b.edges.last(), Some(&4.5));
    }

    #[test]
    fn fits_histograms_give_measured_bin_edges() {
        let m = meta(
            vw_fits::GROUP,
            &[("BSCALE", "0.5"), ("BZERO", "100"), ("BUNIT", "'Jy/beam'")],
        );
        let v = Values::of(&m);
        let pixel = Bins {
            edges: vec![0.0, 10.0],
        };
        assert_eq!(measured_bins(&pixel, &v).edges, [100.0, 105.0]);
        let img = VwImage::new(2, 1, ColorModel::Gray, PixelData::F32(vec![100.0, 105.0])).unwrap();
        let px = pixels(
            &img,
            &m,
            Region {
                x: 0,
                y: 0,
                w: 2,
                h: 1,
            },
            &v,
        );
        assert_eq!(px.pixel, vec![vec![0.0, 10.0]]);
        assert_eq!(px.measured, Some(vec![100.0, 105.0]));
        let bins = common_bins(&[&px.pixel[0]], 2);
        let csv = histogram_rows(
            &[(target(0, 0, 2, 1), px)],
            &bins,
            ColorModel::Gray,
            &v,
            ',',
        );
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "roi,bin,pixel_from,pixel_to,measured_from,measured_to,measured_unit,count"
        );
        assert_eq!(lines[1], "target,0,0,5,100,102.5,Jy/beam,1");
    }

    #[test]
    fn frame_means_use_each_frames_own_conversion() {
        let m = meta(
            "DICOM",
            &[
                ("Modality", "PT"),
                ("Units", "BQML"),
                (vw_dicom::SUV_FACTOR, "0.001"),
            ],
        );
        let img =
            VwImage::new(2, 1, ColorModel::Gray, PixelData::F32(vec![1000.0, 3000.0])).unwrap();
        let f = frame_mean(
            &img,
            &m,
            Region {
                x: 0,
                y: 0,
                w: 2,
                h: 1,
            },
            4,
        );
        assert_eq!(f.index, 4);
        assert_eq!(f.pixel, vec![2000.0]);
        assert_eq!(f.measured, Some(2.0));
    }

    #[test]
    fn frame_rows_number_frames_from_one_and_name_the_file() {
        let m = Metadata::default();
        let v = Values::of(&m);
        let series = vec![(
            Kind::Target,
            vec![FrameMean {
                index: 0,
                pixel: vec![5.0],
                measured: None,
            }],
        )];
        let csv = frames_rows(&series, &["a.dcm".to_owned()], ColorModel::Gray, &v, ',');
        assert_eq!(csv, "roi,frame,mean_pixel_value,file\ntarget,1,5,a.dcm\n");
        let c = frames_chart(&series, ColorModel::Gray, &v, &LABELS, Theme::Lab);
        assert_eq!(c.panels[0].series[0].points, vec![[1.0, 5.0]]);
    }

    #[test]
    fn the_table_lists_each_roi_and_kind_of_value() {
        let m = Metadata::default();
        let v = Values::of(&m);
        let t = table(
            &[(
                Kind::Background,
                place(Region {
                    x: 1,
                    y: 2,
                    w: 3,
                    h: 4,
                }),
                vec![vec![1.0, 3.0]],
                None,
            )],
            ColorModel::Gray,
            &v,
            &LABELS,
            LABELS.place,
        );
        assert_eq!(
            t.rows,
            vec![vec![
                "Background".to_owned(),
                "1, 2  3×4".to_owned(),
                "Pixel value".to_owned(),
                "1".to_owned(),
                "3".to_owned(),
                "2".to_owned(),
                "1".to_owned(),
            ]]
        );
    }
}
