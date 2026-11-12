//! FITS の表（`BINTABLE`、ASCII の `TABLE`）。
//!
//! - 列の見出しは `TTYPEn`、単位は `TUNITn`、型は `TFORMn`
//! - 値には `TSCALn`・`TZEROn` をかけ、`TNULLn` のセルは空にする
//! - 配列のセルは頭の数個だけを出す（`[1024] 1.5, 2.25, 3, …`）。可変長の配列（`P`・`Q`）も読む
//! - 行は描くときに読む（[`vw_core::table::Table::cell`]）
//!
//! 波長（周波数）と流量の列を持つ表（1 次元のスペクトルによくある形）は、[`spectrum`] で
//! スペクトルとしても読める。
//!
//! タイル圧縮の画像の BINTABLE を読む [`crate::tile`] の読み方は使わない。あちらは圧縮の決まった列
//! （`COMPRESSED_DATA` など）を速く引くためのもので、ASCII の表も、文字・論理・ビット・複素数の列も
//! 持たないため。

use std::sync::Arc;

use vw_core::DecodeError;
use vw_core::spectrum::{Axis, AxisKind, Spectrum};
use vw_core::table::{Column, Table};

use crate::header::Hdu;

/// 配列のセルで出す値の数。
const SHOWN_ELEMENTS: usize = 4;

/// 表の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Binary,
    Ascii,
}

impl Kind {
    fn of(hdu: &Hdu) -> Option<Self> {
        match hdu.xtension()? {
            // タイル圧縮の画像も BINTABLE に入っているが、それは画像として見せる。
            "BINTABLE" if !hdu.logical("ZIMAGE") => Some(Kind::Binary),
            "TABLE" => Some(Kind::Ascii),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Binary => "BINTABLE",
            Kind::Ascii => "TABLE",
        }
    }
}

/// `hdu` が表なら、その種類。
pub fn kind(hdu: &Hdu) -> Option<Kind> {
    Kind::of(hdu)
}

/// 列 1 つの、行の中での位置と読み方。
#[derive(Debug, Clone, PartialEq)]
struct Field {
    offset: usize,
    form: Form,
    scale: f64,
    zero: f64,
    null: Option<Null>,
}

#[derive(Debug, Clone, PartialEq)]
enum Null {
    /// BINTABLE の整数の列の、欠損を表す値（格納したままの値）。
    Int(i64),
    /// ASCII の表の、欠損を表す文字。
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Form {
    /// BINTABLE。`code` は型の文字（`L X B I J K A E D C M`）、`repeat` は数。
    Binary { code: u8, repeat: usize },
    /// BINTABLE の可変長の配列。`code` は中身の型、`long` は `Q`（64bit の記述子）か。
    Array { code: u8, long: bool },
    /// ASCII の表。`code` は `A I F E D`、`width` は文字数。
    Ascii { code: u8, width: usize },
}

/// 型の文字 1 つのバイト数（`X` はビットなので 0）。
fn element_size(code: u8) -> Option<usize> {
    Some(match code {
        b'L' | b'B' | b'A' => 1,
        b'X' => 0,
        b'I' => 2,
        b'J' | b'E' => 4,
        b'K' | b'D' | b'C' => 8,
        b'M' => 16,
        _ => return None,
    })
}

fn invalid(what: impl std::fmt::Display) -> DecodeError {
    DecodeError::Invalid(format!("FITS table: {what}"))
}

/// 読んだ表。データ部（表とヒープ）の写しを持つ。
#[derive(Debug)]
pub struct FitsTable {
    kind: Kind,
    columns: Vec<Column>,
    fields: Vec<Field>,
    rows: usize,
    row_len: usize,
    heap: usize,
    data: Arc<[u8]>,
}

/// `data`（ファイル全体）の `hdu` の表を読む。表でなければ `None`。
pub fn read(data: &[u8], hdu: &Hdu) -> Result<Option<FitsTable>, DecodeError> {
    let Some(kind) = Kind::of(hdu) else {
        return Ok(None);
    };
    let row_len = usize::try_from(hdu.int("NAXIS1").unwrap_or(0)).map_err(invalid)?;
    let rows = usize::try_from(hdu.int("NAXIS2").unwrap_or(0)).map_err(invalid)?;
    let count = hdu.int("TFIELDS").unwrap_or(0);
    if !(0..=999).contains(&count) {
        return Err(invalid("broken TFIELDS"));
    }
    let mut columns = Vec::new();
    let mut fields = Vec::new();
    let mut offset = 0usize;
    for i in 1..=count {
        let text = |k: &str| {
            hdu.get(&format!("{k}{i}"))
                .map(|v| v.trim().to_owned())
                .unwrap_or_default()
        };
        let tform = text("TFORM").to_ascii_uppercase();
        let (form, width) = match kind {
            Kind::Binary => binary_form(&tform)?,
            Kind::Ascii => ascii_form(&tform)?,
        };
        let start = match kind {
            Kind::Binary => offset,
            Kind::Ascii => usize::try_from(hdu.int(&format!("TBCOL{i}")).unwrap_or(1) - 1)
                .map_err(|_| invalid("broken TBCOL"))?,
        };
        let end = start
            .checked_add(width)
            .ok_or_else(|| invalid("broken TFORM"))?;
        if end > row_len {
            return Err(invalid(format!("column {i} runs past the end of the row")));
        }
        offset = end;
        let null = match kind {
            Kind::Binary => hdu.int(&format!("TNULL{i}")).map(Null::Int),
            Kind::Ascii => hdu
                .get(&format!("TNULL{i}"))
                .map(|s| Null::Text(s.trim().to_owned())),
        };
        let name = match text("TTYPE") {
            n if n.is_empty() => format!("col{i}"),
            n => n,
        };
        columns.push(Column {
            name,
            unit: text("TUNIT"),
            format: tform,
        });
        fields.push(Field {
            offset: start,
            form,
            scale: hdu.float(&format!("TSCAL{i}")).unwrap_or(1.0),
            zero: hdu.float(&format!("TZERO{i}")).unwrap_or(0.0),
            null,
        });
    }
    let main = row_len
        .checked_mul(rows)
        .ok_or_else(|| invalid("too large"))?;
    let heap = match hdu.int("THEAP") {
        Some(h) => usize::try_from(h).map_err(|_| invalid("broken THEAP"))?,
        None => main,
    };
    let bytes = data
        .get(hdu.data_start..hdu.data_start + hdu.data_len)
        .ok_or_else(|| invalid("truncated data"))?;
    if bytes.len() < main {
        return Err(invalid("truncated data"));
    }
    Ok(Some(FitsTable {
        kind,
        columns,
        fields,
        rows,
        row_len,
        heap,
        data: bytes.into(),
    }))
}

/// BINTABLE の `TFORM`（`1E`、`16A`、`1PD(2048)`、`QJ`）の読み方と、行の中のバイト数。
fn binary_form(tform: &str) -> Result<(Form, usize), DecodeError> {
    let digits = tform.bytes().take_while(u8::is_ascii_digit).count();
    let repeat: usize = match digits {
        0 => 1,
        _ => tform[..digits]
            .parse()
            .map_err(|_| invalid("broken TFORM"))?,
    };
    let rest = &tform.as_bytes()[digits..];
    let code = *rest.first().ok_or_else(|| invalid("broken TFORM"))?;
    if matches!(code, b'P' | b'Q') && repeat > 0 {
        let inner = *rest.get(1).ok_or_else(|| invalid("broken TFORM"))?;
        element_size(inner).ok_or_else(|| invalid(format!("unknown TFORM {tform}")))?;
        let size = if code == b'Q' { 16 } else { 8 };
        let form = Form::Array {
            code: inner,
            long: code == b'Q',
        };
        return Ok((form, size));
    }
    // 数が 0 の列（`0P`・`0J`）は行の中に場所を取らない。
    let size = match code {
        b'P' | b'Q' => 0,
        _ => element_size(code).ok_or_else(|| invalid(format!("unknown TFORM {tform}")))?,
    };
    let width = if code == b'X' {
        repeat.div_ceil(8)
    } else {
        repeat
            .checked_mul(size)
            .ok_or_else(|| invalid("broken TFORM"))?
    };
    Ok((Form::Binary { code, repeat }, width))
}

/// ASCII の表の `TFORM`（`A8`、`I6`、`F10.4`、`E15.7`）の読み方と、文字数。
fn ascii_form(tform: &str) -> Result<(Form, usize), DecodeError> {
    let code = *tform
        .as_bytes()
        .first()
        .ok_or_else(|| invalid("broken TFORM"))?;
    if !matches!(code, b'A' | b'I' | b'F' | b'E' | b'D') {
        return Err(invalid(format!("unknown TFORM {tform}")));
    }
    let width: usize = tform[1..]
        .split('.')
        .next()
        .and_then(|w| w.parse().ok())
        .ok_or_else(|| invalid("broken TFORM"))?;
    Ok((Form::Ascii { code, width }, width))
}

/// 格納したままの値 1 つ。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Raw {
    Logical(Option<bool>),
    Int(i64),
    Float(f64),
    Complex(f64, f64),
}

impl FitsTable {
    pub fn kind(&self) -> Kind {
        self.kind
    }

    fn row_bytes(&self, row: usize) -> Option<&[u8]> {
        let at = row.checked_mul(self.row_len)?;
        self.data.get(at..at + self.row_len)
    }

    /// 可変長の配列の、要素の数とヒープの上のバイト列。`cell` は行の中の記述子。
    fn array(&self, cell: &[u8], code: u8, long: bool) -> Option<(usize, &[u8])> {
        let (count, offset) = if long {
            (
                u64::from_be_bytes(cell.get(..8)?.try_into().ok()?) as usize,
                u64::from_be_bytes(cell.get(8..16)?.try_into().ok()?) as usize,
            )
        } else {
            (
                u32::from_be_bytes(cell.get(..4)?.try_into().ok()?) as usize,
                u32::from_be_bytes(cell.get(4..8)?.try_into().ok()?) as usize,
            )
        };
        let len = match code {
            b'X' => count.div_ceil(8),
            _ => count.checked_mul(element_size(code)?)?,
        };
        let start = self.heap.checked_add(offset)?;
        Some((count, self.data.get(start..start.checked_add(len)?)?))
    }

    /// セルの値（型の文字、要素の数、バイト列）。ASCII の表では使わない。
    fn binary_cell(&self, row: usize, column: usize) -> Option<(u8, usize, &[u8])> {
        let field = self.fields.get(column)?;
        let bytes = self.row_bytes(row)?;
        match field.form {
            Form::Binary { code, repeat } => {
                let width = match code {
                    b'X' => repeat.div_ceil(8),
                    _ => repeat * element_size(code)?,
                };
                Some((code, repeat, bytes.get(field.offset..field.offset + width)?))
            }
            Form::Array { code, long } => {
                let size = if long { 16 } else { 8 };
                let cell = bytes.get(field.offset..field.offset + size)?;
                let (count, body) = self.array(cell, code, long)?;
                Some((code, count, body))
            }
            Form::Ascii { .. } => None,
        }
    }

    /// 要素 `i` の、格納したままの値。
    fn raw(code: u8, bytes: &[u8], i: usize) -> Option<Raw> {
        let take = |n: usize| bytes.get(i * n..(i + 1) * n);
        Some(match code {
            b'L' => Raw::Logical(match *take(1)?.first()? {
                b'T' => Some(true),
                b'F' => Some(false),
                _ => None,
            }),
            b'B' => Raw::Int(i64::from(take(1)?[0])),
            b'I' => Raw::Int(i64::from(i16::from_be_bytes(take(2)?.try_into().ok()?))),
            b'J' => Raw::Int(i64::from(i32::from_be_bytes(take(4)?.try_into().ok()?))),
            b'K' => Raw::Int(i64::from_be_bytes(take(8)?.try_into().ok()?)),
            b'E' => Raw::Float(f64::from(f32::from_be_bytes(take(4)?.try_into().ok()?))),
            b'D' => Raw::Float(f64::from_be_bytes(take(8)?.try_into().ok()?)),
            b'C' => {
                let b = take(8)?;
                Raw::Complex(
                    f64::from(f32::from_be_bytes(b[..4].try_into().ok()?)),
                    f64::from(f32::from_be_bytes(b[4..].try_into().ok()?)),
                )
            }
            b'M' => {
                let b = take(16)?;
                Raw::Complex(
                    f64::from_be_bytes(b[..8].try_into().ok()?),
                    f64::from_be_bytes(b[8..].try_into().ok()?),
                )
            }
            _ => return None,
        })
    }

    /// 値 1 つを文字にする。`TSCAL`・`TZERO` をかけ、`TNULL` は空にする。
    fn element_text(field: &Field, raw: Raw) -> String {
        match raw {
            Raw::Logical(Some(true)) => "T".into(),
            Raw::Logical(Some(false)) => "F".into(),
            Raw::Logical(None) => String::new(),
            Raw::Int(v) if field.null == Some(Null::Int(v)) => String::new(),
            Raw::Int(v) if field.scale == 1.0 && field.zero.fract() == 0.0 => {
                // 符号なしの整数（`TZERO = 32768` など）も、丸めずに整数で出す。
                (i128::from(v) + field.zero as i128).to_string()
            }
            Raw::Int(v) => float_text(v as f64 * field.scale + field.zero),
            Raw::Float(v) => float_text(v * field.scale + field.zero),
            Raw::Complex(re, im) => format!(
                "({}, {})",
                float_text(re * field.scale + field.zero),
                float_text(im * field.scale)
            ),
        }
    }

    /// `row` 行の `column` 列の値を、`TSCAL`・`TZERO` をかけた実数で全部。欠損は NaN、
    /// 数でない列は `None`。
    pub fn numbers(&self, row: usize, column: usize) -> Option<Vec<f64>> {
        let field = self.fields.get(column)?;
        if let Form::Ascii { code, .. } = field.form {
            if code == b'A' {
                return None;
            }
            let text = self.ascii_text(row, column)?;
            if let Some(Null::Text(n)) = &field.null
                && *n == text
            {
                return Some(vec![f64::NAN]);
            }
            let v: f64 = text.replace(['D', 'd'], "E").parse().unwrap_or(f64::NAN);
            return Some(vec![v * field.scale + field.zero]);
        }
        let (code, count, bytes) = self.binary_cell(row, column)?;
        if matches!(code, b'A' | b'L' | b'X' | b'C' | b'M') {
            return None;
        }
        Some(
            (0..count)
                .map(|i| match Self::raw(code, bytes, i) {
                    Some(Raw::Int(v)) if field.null == Some(Null::Int(v)) => f64::NAN,
                    Some(Raw::Int(v)) => v as f64 * field.scale + field.zero,
                    Some(Raw::Float(v)) => v * field.scale + field.zero,
                    _ => f64::NAN,
                })
                .collect(),
        )
    }

    /// 列が数の列か（文字・論理・ビット・複素数でない）。
    fn is_numeric(&self, column: usize) -> bool {
        match self.fields.get(column).map(|f| f.form) {
            Some(Form::Binary { code, .. } | Form::Array { code, .. }) => {
                matches!(code, b'B' | b'I' | b'J' | b'K' | b'E' | b'D')
            }
            Some(Form::Ascii { code, .. }) => code != b'A',
            None => false,
        }
    }

    /// 列のセルが 1 つの値か（配列でない）。
    fn is_scalar(&self, column: usize) -> bool {
        match self.fields.get(column).map(|f| f.form) {
            Some(Form::Binary { repeat, .. }) => repeat == 1,
            Some(Form::Array { .. }) => false,
            Some(Form::Ascii { .. }) => true,
            None => false,
        }
    }

    fn ascii_text(&self, row: usize, column: usize) -> Option<String> {
        let field = self.fields.get(column)?;
        let Form::Ascii { width, .. } = field.form else {
            return None;
        };
        let bytes = self
            .row_bytes(row)?
            .get(field.offset..field.offset + width)?;
        Some(String::from_utf8_lossy(bytes).trim().to_owned())
    }
}

impl Table for FitsTable {
    fn columns(&self) -> &[Column] {
        &self.columns
    }

    fn rows(&self) -> usize {
        self.rows
    }

    fn cell(&self, row: usize, column: usize) -> String {
        let Some(field) = self.fields.get(column) else {
            return String::new();
        };
        if let Form::Ascii { code, .. } = field.form {
            let Some(text) = self.ascii_text(row, column) else {
                return String::new();
            };
            if matches!(&field.null, Some(Null::Text(n)) if *n == text) {
                return String::new();
            }
            if code == b'A' || (field.scale == 1.0 && field.zero == 0.0) {
                return text;
            }
            return match text.replace(['D', 'd'], "E").parse::<f64>() {
                Ok(v) => float_text(v * field.scale + field.zero),
                Err(_) => text,
            };
        }
        let Some((code, count, bytes)) = self.binary_cell(row, column) else {
            return String::new();
        };
        match code {
            b'A' => {
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                String::from_utf8_lossy(&bytes[..end]).trim_end().to_owned()
            }
            b'X' => (0..count)
                .map(|i| {
                    if bytes[i / 8] & (0x80 >> (i % 8)) != 0 {
                        '1'
                    } else {
                        '0'
                    }
                })
                .collect(),
            _ => {
                let text = |i: usize| {
                    Self::raw(code, bytes, i)
                        .map(|r| Self::element_text(field, r))
                        .unwrap_or_default()
                };
                match (count, field.form) {
                    (1, Form::Binary { .. }) => text(0),
                    _ => {
                        let shown: Vec<String> = (0..count.min(SHOWN_ELEMENTS)).map(text).collect();
                        let more = if count > SHOWN_ELEMENTS { ", …" } else { "" };
                        format!("[{count}] {}{more}", shown.join(", "))
                    }
                }
            }
        }
    }

    fn summary(&self) -> String {
        format!(
            "{} {}",
            self.kind.name(),
            vw_core::table::size_text(self.columns.len(), self.rows)
        )
    }
}

/// 実数を短く出す。整数なら小数点を付けず、とても大きい・小さい数は指数で。
fn float_text(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    let a = v.abs();
    let exponent = a != 0.0 && !(1e-4..1e15).contains(&a);
    // f32 の値は f32 の短い表し方で出す（0.1f32 を 0.10000000149011612 にしない）。
    let single = v as f32;
    match (exponent, f64::from(single) == v) {
        (true, true) => format!("{single:e}"),
        (true, false) => format!("{v:e}"),
        (false, true) => format!("{single}"),
        (false, false) => format!("{v}"),
    }
}

/// 横軸にする列の名前の頭と、その量。
fn axis_column(name: &str) -> Option<AxisKind> {
    let n = name.to_ascii_uppercase();
    if n.starts_with("WAVE") || n.starts_with("LAMBDA") || n == "LAM" || n.starts_with("LOGLAM") {
        Some(AxisKind::Wavelength)
    } else if n.starts_with("FREQ") {
        Some(AxisKind::Frequency)
    } else if n.starts_with("VELO") || n == "VRAD" || n == "VOPT" {
        Some(AxisKind::Velocity)
    } else {
        None
    }
}

/// 縦軸にする列の名前か。
fn is_flux_column(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    ["FLUX", "FLAM", "FNU", "INTENS", "SPEC"]
        .iter()
        .any(|p| n.starts_with(p))
        && !["ERR", "IVAR", "SIG", "VAR"].iter().any(|e| n.contains(e))
}

/// 横軸の単位を基本の単位（波長は m、周波数は Hz、速度は m/s）にする倍率。
/// 単位がない波長は Å とみなす（可視の分光の表は Å で書くものがほとんどのため）。
fn axis_factor(kind: &AxisKind, unit: &str) -> f64 {
    let u = unit.trim();
    match kind {
        AxisKind::Wavelength => match u {
            "m" => 1.0,
            "cm" => 1e-2,
            "mm" => 1e-3,
            "um" | "micron" | "microns" | "µm" => 1e-6,
            "nm" => 1e-9,
            _ => 1e-10,
        },
        AxisKind::Frequency => match u {
            "kHz" => 1e3,
            "MHz" => 1e6,
            "GHz" => 1e9,
            "THz" => 1e12,
            _ => 1.0,
        },
        AxisKind::Velocity => match u.to_ascii_lowercase().as_str() {
            "km/s" | "km s-1" | "km.s-1" | "km s^-1" => 1e3,
            "cm/s" => 1e-2,
            _ => 1.0,
        },
        AxisKind::Other { .. } => 1.0,
    }
}

/// 波長（周波数・速度）の列と流量の列があれば、スペクトルにする。セルが配列なら最初の行の配列を、
/// 1 つの値なら全部の行を並べる。`hdu` の `RESTFRQ`・`RESTWAV` は速度を出すのに使う。
pub fn spectrum(table: &FitsTable, hdu: &Hdu) -> Option<Spectrum> {
    let names = || table.columns.iter().enumerate();
    let (x, kind) = names().find_map(|(i, c)| Some((i, axis_column(&c.name)?)))?;
    let y = names()
        .find(|(i, c)| *i != x && is_flux_column(&c.name))
        .map(|(i, _)| i)?;
    if !table.is_numeric(x) || !table.is_numeric(y) || table.rows == 0 {
        return None;
    }
    let (xs, ys) = if table.is_scalar(x) && table.is_scalar(y) {
        let column = |c: usize| -> Vec<f64> {
            (0..table.rows)
                .map(|r| {
                    table
                        .numbers(r, c)
                        .and_then(|v| v.first().copied())
                        .unwrap_or(f64::NAN)
                })
                .collect()
        };
        (column(x), column(y))
    } else {
        (table.numbers(0, x)?, table.numbers(0, y)?)
    };
    if xs.len() != ys.len() || xs.len() < 2 {
        return None;
    }
    let x_name = &table.columns[x].name;
    let log = x_name.to_ascii_uppercase().starts_with("LOGLAM");
    let factor = axis_factor(&kind, &table.columns[x].unit);
    let values: Arc<[f64]> = xs
        .iter()
        .map(|&v| if log { 10f64.powf(v) } else { v } * factor)
        .collect();
    let positive = |k: &str| hdu.float(k).filter(|v| v.is_finite() && *v > 0.0);
    Some(Spectrum {
        values: ys.iter().map(|&v| v as f32).collect(),
        axis: Axis {
            kind,
            values,
            rest_frequency: positive("RESTFRQ").or_else(|| positive("RESTFREQ")),
            rest_wavelength: positive("RESTWAV").or_else(|| positive("RESTWAVE")),
            frame: hdu
                .get("SPECSYS")
                .map(|s| s.trim().to_uppercase())
                .unwrap_or_default(),
        },
        value_unit: table.columns[y].unit.clone(),
    })
}

/// `-TAB` の軸を持つ HDU に、その軸の表（[`crate::header::Lookup`]）を付ける。表が見つからない軸は
/// 付けない（その軸はスペクトルの軸として読まない）。
///
/// 表は `PSi_0`（`EXTNAME`、既定 `WCS-TAB`）・`PVi_1`（`EXTVER`）・`PVi_2`（`EXTLEVEL`）で探し、
/// 座標の列 `PSi_1` と番号の列 `PSi_2` の 1 行目を読む。座標の列が多次元（`M` 本の軸をまとめた表）
/// なら、`PVi_3` 本目の値だけを取り出す。
pub(crate) fn resolve_lookups(data: &[u8], hdus: &mut [Hdu]) {
    let mut found: Vec<(usize, usize, crate::header::Lookup)> = Vec::new();
    for (at, hdu) in hdus.iter().enumerate() {
        let Some(n) = hdu.int("WCSAXES").or_else(|| hdu.int("NAXIS")) else {
            continue;
        };
        for axis in 1..=usize::try_from(n).unwrap_or(0).min(999) {
            let tab = hdu
                .get(&format!("CTYPE{axis}"))
                .is_some_and(|c| c.trim().to_ascii_uppercase().ends_with("-TAB"));
            if tab && let Some(lookup) = lookup_of(data, hdus, hdu, axis) {
                found.push((at, axis, lookup));
            }
        }
    }
    for (at, axis, lookup) in found {
        hdus[at].lookups.push((axis, lookup));
    }
}

fn lookup_of(data: &[u8], hdus: &[Hdu], hdu: &Hdu, axis: usize) -> Option<crate::header::Lookup> {
    let ps = |m: u32| {
        hdu.get(&format!("PS{axis}_{m}"))
            .map(|v| v.trim().to_owned())
    };
    let pv = |m: u32, default: i64| hdu.int(&format!("PV{axis}_{m}")).unwrap_or(default);
    let name = ps(0).unwrap_or_else(|| "WCS-TAB".into());
    let (version, level) = (pv(1, 1), pv(2, 1));
    let table_hdu = hdus.iter().find(|h| {
        kind(h) == Some(Kind::Binary)
            && h.get("EXTNAME")
                .is_some_and(|n| n.trim().eq_ignore_ascii_case(&name))
            && h.int("EXTVER").unwrap_or(1) == version
            && h.int("EXTLEVEL").unwrap_or(1) == level
    })?;
    let table = read(data, table_hdu).ok()??;
    let column = |name: &str| {
        table
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    };
    let coords_column = column(&ps(1)?)?;
    let coords = table.numbers(0, coords_column)?;
    let index = match ps(2) {
        Some(n) => Some(table.numbers(0, column(&n)?)?),
        None => None,
    };
    // 座標の列の形は `TDIM` の (M, K1, …)。1 次元の軸だけを読むので、M 個おきに m 本目を取る。
    let dims: Vec<usize> = table_hdu
        .get(&format!("TDIM{}", coords_column + 1))
        .map(|d| {
            d.trim_matches(|c: char| c == '(' || c == ')' || c.is_whitespace())
                .split(',')
                .filter_map(|n| n.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let stride = if dims.len() >= 2 { dims[0].max(1) } else { 1 };
    let m = usize::try_from(pv(3, 1)).ok()?.max(1);
    if m > stride {
        return None;
    }
    let coords: Vec<f64> = coords.iter().skip(m - 1).step_by(stride).copied().collect();
    if coords.is_empty() || index.as_ref().is_some_and(|i| i.len() != coords.len()) {
        return None;
    }
    Some(crate::header::Lookup { index, coords })
}
