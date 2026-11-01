//! 計測線（計測ツールで引いた線）と、その線に沿った値（ラインプロファイル）。描画と操作は
//! [`crate::viewer`]、欄は [`crate::app`] にある。
//!
//! 値は十字の印と同じく、表示用の縮小画像ではなく元の画像から読む（16bit・実数・FITS は元の値）。
//! 線の上の点は、始点から 1 画素ずつ進んだ位置に最も近い画素の値にする。隣の画素から補間しないのは、
//! ファイルに無い値（16bit の整数の間の値や、欠損と隣の平均）を作ってしまうため。

use std::path::PathBuf;

use vw_core::{ColorModel, Metadata, VwImage};
use vw_report::{Chart, Panel, Series, Table, Theme};

use crate::histogram;
use crate::probe;
use crate::quantity::Quantity;

/// スロットに引いた計測線。置いた画像（フレームならフレームのパス）にだけ出す。
/// 端は画素の番号で、線は画素の中心どうしを結ぶ。
#[derive(Debug, Clone, PartialEq)]
pub struct MeasureLine {
    pub path: PathBuf,
    pub start: (u32, u32),
    pub end: (u32, u32),
    /// 画面とグラフに出す番号。消しても番号を詰めない（グラフと画像の対応が変わらないように）。
    pub number: usize,
}

impl MeasureLine {
    /// 始点から終点までの長さ（画素）。
    pub fn length(&self) -> f64 {
        let dx = f64::from(self.end.0) - f64::from(self.start.0);
        let dy = f64::from(self.end.1) - f64::from(self.start.1);
        dx.hypot(dy)
    }

    /// 線の上の点（始点からの距離, 画素）。始点と終点を含み、間は 1 画素ずつ。
    pub fn points(&self) -> Vec<(f64, (u32, u32))> {
        let length = self.length();
        let steps = length.ceil().max(0.0) as usize;
        let (x0, y0) = (f64::from(self.start.0), f64::from(self.start.1));
        let (x1, y1) = (f64::from(self.end.0), f64::from(self.end.1));
        (0..=steps)
            .map(|i| {
                let t = if steps == 0 {
                    0.0
                } else {
                    i as f64 / steps as f64
                };
                let x = (x0 + (x1 - x0) * t).round() as u32;
                let y = (y0 + (y1 - y0) * t).round() as u32;
                (length * t, (x, y))
            })
            .collect()
    }
}

/// 次に引く線の番号。その画像の線の番号の最大の次。
pub fn next_number(lines: &[MeasureLine], path: &std::path::Path) -> usize {
    lines
        .iter()
        .filter(|l| l.path == path)
        .map(|l| l.number)
        .max()
        .unwrap_or(0)
        + 1
}

/// 線の始点から見た終点を、Shift で 45° 刻みの向きに寄せる。長さはそのまま。
pub fn snap((x0, y0): (u32, u32), (x1, y1): (u32, u32), w: u32, h: u32) -> (u32, u32) {
    let (dx, dy) = (f64::from(x1) - f64::from(x0), f64::from(y1) - f64::from(y0));
    let length = dx.hypot(dy);
    if length == 0.0 {
        return (x1, y1);
    }
    let step = std::f64::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    // 斜めは x と y を同じだけ動かす（丸めで 45° からずれないように）。
    let (ux, uy) = (angle.cos(), angle.sin());
    let reach = if ux.abs() > 0.1 && uy.abs() > 0.1 {
        (dx.abs() + dy.abs()) / 2.0
    } else {
        length
    };
    let clamp = |v: f64, max: u32| v.round().clamp(0.0, f64::from(max.saturating_sub(1))) as u32;
    let sign = |v: f64| if v.abs() < 0.1 { 0.0 } else { v.signum() };
    (
        clamp(f64::from(x0) + sign(ux) * reach, w),
        clamp(f64::from(y0) + sign(uy) * reach, h),
    )
}

/// 画素の値（ファイルに書かれた値）と、計測の値（物理量に直した値）の関係。
///
/// 読み込んだ画像は、DICOM なら Rescale を、FITS なら BSCALE・BZERO を当てた値で持っている
/// （[`vw_dicom`]・[`vw_fits`]）。画素の値はそこから式を逆にたどって出す。
#[derive(Debug, Clone, PartialEq)]
pub struct Values {
    /// 画素の値 = (読んだ値 − intercept) / slope。`None` は読んだ値がそのまま画素の値。
    stored: Option<(f64, f64)>,
    /// 計測の値 = 読んだ値 × これ（PET の SUVbw）。
    suv: Option<f64>,
    /// 計測の値の単位。
    pub unit: String,
}

impl Values {
    pub fn of(meta: &Metadata) -> Self {
        let quantity = Quantity::of(meta);
        let suv = match quantity {
            Quantity::Pet(Ok(f)) => Some(f),
            _ => None,
        };
        let unit = match &quantity {
            Quantity::Raw => String::new(),
            Quantity::Unit(u) => (*u).to_owned(),
            Quantity::Named(u) => u.clone(),
            Quantity::Pet(Ok(_)) => "SUVbw".to_owned(),
            Quantity::Pet(Err(_)) => "Bq/ml".to_owned(),
        };
        Self {
            stored: dicom_rescale(meta).or_else(|| fits_scale(meta)),
            suv,
            unit,
        }
    }

    /// 画素の値と計測の値が違う画像か（グラフ・表・CSV に 2 種類とも出す）。
    pub fn two_kinds(&self) -> bool {
        self.stored.is_some() || self.suv.is_some()
    }

    pub fn pixel(&self, read: f64) -> f64 {
        match self.stored {
            Some((slope, intercept)) => (read - intercept) / slope,
            None => read,
        }
    }

    /// [`Values::pixel`] の逆。画素の値から読んだ値に戻す。
    pub fn read_of(&self, pixel: f64) -> f64 {
        match self.stored {
            Some((slope, intercept)) => pixel * slope + intercept,
            None => pixel,
        }
    }

    pub fn measured(&self, read: f64) -> f64 {
        read * self.suv.unwrap_or(1.0)
    }
}

fn number_of(meta: &Metadata, group: &str, key: &str) -> Option<f64> {
    meta.entries
        .iter()
        .rev()
        .find(|(g, k, _)| g.starts_with(group) && k == key)
        .and_then(|(_, _, v)| v.trim().trim_matches('\'').trim().parse::<f64>().ok())
}

/// DICOM の Rescale。何もしない式（1 倍、0 足す）なら `None`。
fn dicom_rescale(meta: &Metadata) -> Option<(f64, f64)> {
    let slope = number_of(meta, "DICOM", "Rescale slope");
    let intercept = number_of(meta, "DICOM", "Rescale intercept");
    if slope.is_none() && intercept.is_none() {
        return None;
    }
    let (slope, intercept) = (slope.unwrap_or(1.0), intercept.unwrap_or(0.0));
    (slope != 0.0 && (slope, intercept) != (1.0, 0.0)).then_some((slope, intercept))
}

/// FITS の BSCALE・BZERO。符号なし 16bit の決まり（BSCALE 1、BZERO 32768）は、整数の型の都合で
/// 書く値を 32768 ずらしただけで物理量への換算ではないので、1 種類として扱う。
fn fits_scale(meta: &Metadata) -> Option<(f64, f64)> {
    let scale = number_of(meta, vw_fits::GROUP, "BSCALE").unwrap_or(1.0);
    let zero = number_of(meta, vw_fits::GROUP, "BZERO").unwrap_or(0.0);
    let plain = scale == 1.0 && (zero == 0.0 || zero == 32768.0);
    (scale != 0.0 && !plain).then_some((scale, zero))
}

/// 1 本の線に沿った値。
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub number: usize,
    pub start: (u32, u32),
    pub end: (u32, u32),
    pub length: f64,
    pub distance: Vec<f64>,
    pub positions: Vec<(u32, u32)>,
    /// 色ごとの画素の値（モノクロは 1 つ、カラーは R・G・B）。
    pub pixel: Vec<Vec<f64>>,
    /// 計測の値（モノクロで 2 種類ある画像だけ）。
    pub measured: Option<Vec<f64>>,
}

/// 色の名前。モノクロは空。
pub fn channel_names(color: ColorModel) -> &'static [&'static str] {
    match color {
        ColorModel::Rgb | ColorModel::Rgba => &["R", "G", "B"],
        ColorModel::Gray | ColorModel::GrayAlpha => &[""],
    }
}

pub fn profile(img: &VwImage, meta: &Metadata, line: &MeasureLine, values: &Values) -> Profile {
    let channels = channel_names(img.color).len();
    let points = line.points();
    let mut pixel = vec![Vec::with_capacity(points.len()); channels];
    let mut read = Vec::with_capacity(points.len());
    for &(_, (x, y)) in &points {
        let v = probe::sample(img, meta, x, y);
        for (c, column) in pixel.iter_mut().enumerate() {
            column.push(
                v.as_ref()
                    .and_then(|v| v.get(c).copied())
                    .map_or(
                        f64::NAN,
                        |r| if channels == 1 { values.pixel(r) } else { r },
                    ),
            );
        }
        read.push(v.and_then(|v| v.first().copied()).unwrap_or(f64::NAN));
    }
    let measured = (channels == 1 && values.two_kinds())
        .then(|| read.iter().map(|&r| values.measured(r)).collect());
    Profile {
        number: line.number,
        start: line.start,
        end: line.end,
        length: line.length(),
        distance: points.iter().map(|p| p.0).collect(),
        positions: points.iter().map(|p| p.1).collect(),
        pixel,
        measured,
    }
}

/// 最小・最大・平均・標準偏差。欠損（NaN）は数えない。
pub fn stats(values: &[f64]) -> Option<[f64; 4]> {
    let mut acc = histogram::Accumulator::default();
    let mut any = false;
    for &v in values.iter().filter(|v| v.is_finite()) {
        acc.add(v);
        any = true;
    }
    let s = acc.stats();
    any.then_some([s.min, s.max, s.mean, s.std_dev])
}

/// 図と表の言葉（画面の言語）。
pub struct Labels<'a> {
    pub line: &'a str,
    pub distance: &'a str,
    pub pixel_value: &'a str,
    pub measured_value: &'a str,
    pub start: &'a str,
    pub end: &'a str,
    pub length: &'a str,
    pub min: &'a str,
    pub max: &'a str,
    pub mean: &'a str,
    pub sd: &'a str,
    pub kind: &'a str,
}

/// 線の名前（凡例）。カラーは色の名前を付ける。
fn series_name(labels: &Labels<'_>, number: usize, channel: &str) -> String {
    if channel.is_empty() {
        format!("{} {number}", labels.line)
    } else {
        format!("{} {number} {channel}", labels.line)
    }
}

fn with_unit(name: &str, unit: &str) -> String {
    if unit.is_empty() {
        name.to_owned()
    } else {
        format!("{name} ({unit})")
    }
}

/// 全部の線のグラフ。画素の値の段と、2 種類ある画像では計測の値の段。
pub fn chart(
    profiles: &[Profile],
    color: ColorModel,
    values: &Values,
    labels: &Labels<'_>,
    theme: Theme,
) -> Chart {
    let names = channel_names(color);
    let points = |p: &Profile, v: &[f64]| -> Vec<[f64; 2]> {
        p.distance.iter().zip(v).map(|(&d, &v)| [d, v]).collect()
    };
    let mut pixel = Vec::new();
    for (i, p) in profiles.iter().enumerate() {
        for (c, column) in p.pixel.iter().enumerate() {
            let style = if names.len() > 1 {
                theme.channel(c, i)
            } else {
                theme.line(i)
            };
            pixel.push(Series {
                name: series_name(labels, p.number, names[c]),
                points: points(p, column),
                style,
            });
        }
    }
    let mut panels = vec![Panel {
        y_label: labels.pixel_value.to_owned(),
        series: pixel,
    }];
    if values.two_kinds() {
        panels.push(Panel {
            y_label: with_unit(labels.measured_value, &values.unit),
            series: profiles
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    Some(Series {
                        name: series_name(labels, p.number, ""),
                        points: points(p, p.measured.as_ref()?),
                        style: theme.line(i),
                    })
                })
                .collect(),
        });
    } else if !values.unit.is_empty() {
        panels[0].y_label = with_unit(labels.pixel_value, &values.unit);
    }
    Chart {
        x_label: format!("{} (px)", labels.distance),
        panels,
    }
}

/// 線ごとの位置・長さと、値の種類ごとの最小・最大・平均・標準偏差の表。
pub fn table(
    profiles: &[Profile],
    color: ColorModel,
    values: &Values,
    labels: &Labels<'_>,
) -> Table {
    let names = channel_names(color);
    let mut rows = Vec::new();
    let n = histogram::number;
    for p in profiles {
        let mut kinds: Vec<(String, &[f64])> = p
            .pixel
            .iter()
            .enumerate()
            .map(|(c, v)| {
                let kind = if names[c].is_empty() {
                    labels.pixel_value.to_owned()
                } else {
                    format!("{} {}", labels.pixel_value, names[c])
                };
                (kind, v.as_slice())
            })
            .collect();
        if let Some(m) = &p.measured {
            kinds.push((with_unit(labels.measured_value, &values.unit), m.as_slice()));
        }
        for (kind, v) in kinds {
            let s = stats(v);
            let cell = |i: usize| s.map_or_else(String::new, |s| n(s[i]));
            rows.push(vec![
                p.number.to_string(),
                format!("{}, {}", p.start.0, p.start.1),
                format!("{}, {}", p.end.0, p.end.1),
                format!("{:.1}", p.length),
                kind,
                cell(0),
                cell(1),
                cell(2),
                cell(3),
            ]);
        }
    }
    Table {
        head: vec![
            labels.line.to_owned(),
            labels.start.to_owned(),
            labels.end.to_owned(),
            format!("{} (px)", labels.length),
            labels.kind.to_owned(),
            labels.min.to_owned(),
            labels.max.to_owned(),
            labels.mean.to_owned(),
            labels.sd.to_owned(),
        ],
        rows,
        numeric_from: 5,
    }
}

/// 点ごとの値の表（CSV・タブ区切り）。列名は言語によらず英語。
pub fn rows(profiles: &[Profile], color: ColorModel, values: &Values, separator: char) -> String {
    let names = channel_names(color);
    let mut head = vec!["line", "index", "distance_px", "x", "y"];
    let pixel_columns: Vec<String> = names
        .iter()
        .map(|c| {
            if c.is_empty() {
                "pixel_value".to_owned()
            } else {
                format!("pixel_{}", c.to_lowercase())
            }
        })
        .collect();
    head.extend(pixel_columns.iter().map(String::as_str));
    let measured = values.two_kinds() && names.len() == 1;
    if measured {
        head.extend(["measured_value", "measured_unit"]);
    }
    let field = |s: String| {
        if separator == ',' {
            vw_report::csv_field(&s)
        } else {
            s
        }
    };
    let sep = separator.to_string();
    let mut out = head.join(&sep);
    out.push('\n');
    let value = |v: f64| {
        if v.is_finite() {
            format!("{v}")
        } else {
            String::new()
        }
    };
    for p in profiles {
        for i in 0..p.distance.len() {
            let mut row = vec![
                p.number.to_string(),
                i.to_string(),
                format!("{:.3}", p.distance[i]),
                p.positions[i].0.to_string(),
                p.positions[i].1.to_string(),
            ];
            row.extend(p.pixel.iter().map(|c| value(c[i])));
            if measured {
                row.push(
                    p.measured
                        .as_ref()
                        .map_or_else(String::new, |m| value(m[i])),
                );
                row.push(field(values.unit.clone()));
            }
            out.push_str(&row.join(&sep));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_core::PixelData;

    fn line(start: (u32, u32), end: (u32, u32)) -> MeasureLine {
        MeasureLine {
            path: PathBuf::from("a.png"),
            start,
            end,
            number: 1,
        }
    }

    fn meta(group: &str, entries: &[(&str, &str)]) -> Metadata {
        let mut m = Metadata::default();
        for (k, v) in entries {
            m.push(group, *k, *v);
        }
        m
    }

    const LABELS: Labels<'static> = Labels {
        line: "Line",
        distance: "Distance from the start",
        pixel_value: "Pixel value",
        measured_value: "Measured value",
        start: "Start",
        end: "End",
        length: "Length",
        min: "Min",
        max: "Max",
        mean: "Mean",
        sd: "SD",
        kind: "Values",
    };

    #[test]
    fn a_line_is_sampled_once_per_pixel_from_start_to_end() {
        let pts = line((0, 0), (4, 0)).points();
        let xs: Vec<u32> = pts.iter().map(|p| p.1.0).collect();
        assert_eq!(xs, [0, 1, 2, 3, 4]);
        assert_eq!(pts.last().unwrap().0, 4.0);
    }

    #[test]
    fn a_diagonal_line_reaches_its_end_and_measures_its_length() {
        let l = line((0, 0), (3, 4));
        assert_eq!(l.length(), 5.0);
        let pts = l.points();
        assert_eq!(pts.len(), 6);
        assert_eq!(pts.last().unwrap().1, (3, 4));
    }

    #[test]
    fn a_zero_length_line_has_one_point() {
        assert_eq!(line((2, 2), (2, 2)).points().len(), 1);
    }

    #[test]
    fn numbers_continue_after_the_largest_on_the_same_image() {
        let mut a = line((0, 0), (1, 1));
        a.number = 3;
        let mut other = line((0, 0), (1, 1));
        other.path = PathBuf::from("b.png");
        other.number = 9;
        assert_eq!(next_number(&[a, other], std::path::Path::new("a.png")), 4);
        assert_eq!(next_number(&[], std::path::Path::new("a.png")), 1);
    }

    #[test]
    fn shift_snaps_to_horizontal_vertical_and_diagonal() {
        assert_eq!(snap((10, 10), (20, 12), 100, 100), (20, 10));
        assert_eq!(snap((10, 10), (11, 25), 100, 100), (10, 25));
        assert_eq!(snap((10, 10), (19, 21), 100, 100), (20, 20));
        assert_eq!(snap((10, 10), (2, 0), 100, 100), (1, 1));
        assert_eq!(snap((1, 1), (0, 30), 100, 100), (1, 30));
        assert_eq!(snap((90, 50), (120, 52), 100, 100), (99, 50));
    }

    #[test]
    fn sixteen_bit_values_are_the_stored_values() {
        let img = VwImage::new(
            3,
            1,
            ColorModel::Gray,
            PixelData::U16(vec![0, 40000, 65535]),
        )
        .unwrap();
        let m = Metadata::default();
        let v = Values::of(&m);
        let p = profile(&img, &m, &line((0, 0), (2, 0)), &v);
        assert_eq!(p.pixel, vec![vec![0.0, 40000.0, 65535.0]]);
        assert_eq!(p.measured, None);
        assert!(!v.two_kinds());
    }

    #[test]
    fn color_images_give_red_green_and_blue_lines() {
        let img =
            VwImage::new(2, 1, ColorModel::Rgb, PixelData::U8(vec![1, 2, 3, 4, 5, 6])).unwrap();
        let m = Metadata::default();
        let v = Values::of(&m);
        let p = profile(&img, &m, &line((0, 0), (1, 0)), &v);
        assert_eq!(
            p.pixel,
            vec![vec![1.0, 4.0], vec![2.0, 5.0], vec![3.0, 6.0]]
        );
        let c = chart(&[p], ColorModel::Rgb, &v, &LABELS, Theme::Lab);
        let names: Vec<&str> = c.panels[0].series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Line 1 R", "Line 1 G", "Line 1 B"]);
    }

    #[test]
    fn ct_gives_stored_pixel_values_and_hu() {
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
        assert!(v.two_kinds());
        let p = profile(&img, &m, &line((0, 0), (1, 0)), &v);
        assert_eq!(p.pixel, vec![vec![24.0, 1064.0]]);
        assert_eq!(p.measured, Some(vec![-1000.0, 40.0]));
        let c = chart(
            std::slice::from_ref(&p),
            ColorModel::Gray,
            &v,
            &LABELS,
            Theme::Lab,
        );
        assert_eq!(c.panels.len(), 2);
        assert_eq!(c.panels[1].y_label, "Measured value (HU)");
        let t = table(std::slice::from_ref(&p), ColorModel::Gray, &v, &LABELS);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[1][4], "Measured value (HU)");
        let csv = rows(&[p], ColorModel::Gray, &v, ',');
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "line,index,distance_px,x,y,pixel_value,measured_value,measured_unit"
        );
        assert_eq!(lines[1], "1,0,0.000,0,0,24,-1000,HU");
    }

    #[test]
    fn fits_bscale_and_bzero_give_two_kinds() {
        let m = meta(
            vw_fits::GROUP,
            &[
                ("BITPIX", "16"),
                ("BSCALE", "0.5"),
                ("BZERO", "100"),
                ("BUNIT", "'Jy/beam'"),
            ],
        );
        let v = Values::of(&m);
        assert!(v.two_kinds());
        assert_eq!(v.pixel(150.0), 100.0);
        assert_eq!(v.measured(150.0), 150.0);
        assert_eq!(v.unit, "Jy/beam");
    }

    #[test]
    fn unsigned_sixteen_bit_fits_is_one_kind() {
        let m = meta(
            vw_fits::GROUP,
            &[("BITPIX", "16"), ("BSCALE", "1"), ("BZERO", "32768")],
        );
        assert!(!Values::of(&m).two_kinds());
    }

    #[test]
    fn pet_measures_in_suv() {
        let m = meta(
            "DICOM",
            &[
                ("Modality", "PT"),
                ("Units", "BQML"),
                (vw_dicom::SUV_FACTOR, "0.0002"),
            ],
        );
        let v = Values::of(&m);
        assert!(v.two_kinds());
        assert_eq!(v.unit, "SUVbw");
        assert!((v.measured(10_000.0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn missing_pixels_leave_gaps_and_are_left_out_of_the_statistics() {
        assert_eq!(stats(&[1.0, f64::NAN, 3.0]), Some([1.0, 3.0, 2.0, 1.0]));
        assert_eq!(stats(&[f64::NAN]), None);
    }

    #[test]
    fn tab_separated_rows_do_not_quote() {
        let img = VwImage::new(1, 1, ColorModel::Gray, PixelData::U8(vec![7])).unwrap();
        let m = Metadata::default();
        let v = Values::of(&m);
        let p = profile(&img, &m, &line((0, 0), (0, 0)), &v);
        assert_eq!(
            rows(&[p], ColorModel::Gray, &v, '\t'),
            "line\tindex\tdistance_px\tx\ty\tpixel_value\n1\t0\t0.000\t0\t0\t7\n"
        );
    }
}
