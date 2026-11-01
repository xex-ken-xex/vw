//! 格納値から計測の値へ（CF の `_Unsigned`、欠損、`scale_factor`・`add_offset`）。

use netcdf_reader::unpack::SignParams;
use netcdf_reader::{NcAttrValue, NcFile, NcSliceInfo, NcSliceInfoElem, NcType, NcVariable};
use vw_core::DecodeError;

use crate::layout::Variable;

/// `scale_factor`・`add_offset`。どちらもなければ `None`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Packing {
    pub scale: f64,
    pub offset: f64,
}

impl Packing {
    pub fn of(var: &NcVariable) -> Option<Self> {
        let scale = number(var, "scale_factor");
        let offset = number(var, "add_offset");
        (scale.is_some() || offset.is_some()).then(|| Self {
            scale: scale.unwrap_or(1.0),
            offset: offset.unwrap_or(0.0),
        })
    }

    pub fn apply(self, stored: f64) -> f64 {
        stored * self.scale + self.offset
    }
}

/// 欠損にする値。
#[derive(Debug, Clone, Default, PartialEq)]
struct Mask {
    fill: Option<f64>,
    missing: Vec<f64>,
    /// 格納値で比べる範囲と、計測の値で比べる範囲。
    stored_range: (Option<f64>, Option<f64>),
    measured_range: (Option<f64>, Option<f64>),
}

impl Mask {
    fn of(var: &NcVariable, sign: Option<SignParams>) -> Self {
        let stored = |v: f64| {
            let v = if var.dtype == NcType::Float {
                f64::from(v as f32)
            } else {
                v
            };
            sign.map_or(v, |s| s.reinterpret(v))
        };
        let fill = number(var, "_FillValue")
            .or_else(|| default_fill(&var.dtype))
            .map(stored);
        let missing = var
            .attribute("missing_value")
            .and_then(|a| a.value.as_f64_vec())
            .unwrap_or_default()
            .into_iter()
            .map(stored)
            .collect();
        let range = match var.attribute("valid_range").and_then(range_of) {
            Some((lo, hi)) => Some((Some(lo), Some(hi), "valid_range")),
            None => {
                let lo = number(var, "valid_min");
                let hi = number(var, "valid_max");
                (lo.is_some() || hi.is_some()).then_some((
                    lo,
                    hi,
                    if lo.is_some() {
                        "valid_min"
                    } else {
                        "valid_max"
                    },
                ))
            }
        };
        let mut mask = Self {
            fill,
            missing,
            ..Self::default()
        };
        if let Some((lo, hi, name)) = range {
            // CF: 範囲が scale_factor と同じ型（変数の型と違う）なら、計測の値の範囲。
            let packed_type = var.attribute("scale_factor").map(|a| kind(&a.value));
            let range_type = var.attribute(name).map(|a| kind(&a.value));
            let measured = packed_type.is_some()
                && packed_type == range_type
                && range_type != Some(stored_kind(&var.dtype));
            if measured {
                mask.measured_range = (lo, hi);
            } else {
                mask.stored_range = (lo.map(stored), hi.map(stored));
            }
        }
        mask
    }

    fn stored_masks(&self, v: f64) -> bool {
        v.is_nan()
            || self.fill == Some(v)
            || self.missing.contains(&v)
            || outside(self.stored_range, v)
    }
}

fn outside((lo, hi): (Option<f64>, Option<f64>), v: f64) -> bool {
    lo.is_some_and(|lo| v < lo) || hi.is_some_and(|hi| v > hi)
}

/// 変数の 1 面。
pub struct Plane {
    /// 計測の値。欠損は NaN。行は縦の次元の順（上下を返す前）。
    pub values: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

/// `var` の `plane` 番目の面を読み、計測の値にする。
pub fn read_plane(
    file: &NcFile,
    layout: &Variable,
    var: &NcVariable,
    plane: usize,
) -> Result<Plane, DecodeError> {
    let (w, h) = (layout.width() as usize, layout.height() as usize);
    vw_core::limits::check_image(w as u64, h as u64, 4)?;
    let mut selections = vec![
        NcSliceInfoElem::Slice {
            start: 0,
            end: u64::MAX,
            step: 1,
        };
        layout.dims.len()
    ];
    for (d, i) in layout.plane_index(plane) {
        selections[d] = NcSliceInfoElem::Index(i);
    }
    let raw = file
        .read_variable_slice_as_f64(&layout.path, &NcSliceInfo { selections })
        .map_err(|e| DecodeError::Invalid(format!("NetCDF {}: {e}", layout.path)))?;
    let flat: Vec<f64> = raw.iter().copied().collect();
    if flat.len() != w * h {
        return Err(DecodeError::Invalid(format!(
            "NetCDF {}: read {} values for a {w}x{h} plane",
            layout.path,
            flat.len()
        )));
    }
    let sign = SignParams::from_variable(var);
    let mask = Mask::of(var, sign);
    let packing = Packing::of(var);
    let measure = |stored: f64| {
        let stored = sign.map_or(stored, |s| s.reinterpret(stored));
        if mask.stored_masks(stored) {
            return f32::NAN;
        }
        let value = packing.map_or(stored, |p| p.apply(stored));
        if outside(mask.measured_range, value) {
            f32::NAN
        } else {
            value as f32
        }
    };
    let values = if layout.transposed() {
        // ファイルは (横, 縦) の順。画像の行に並べ替える。
        let mut out = vec![0.0; w * h];
        for x in 0..w {
            for y in 0..h {
                out[y * w + x] = measure(flat[x * h + y]);
            }
        }
        out
    } else {
        flat.into_iter().map(measure).collect()
    };
    Ok(Plane {
        values,
        width: w,
        height: h,
    })
}

/// 属性の数値（最初の 1 つ）。
pub fn number(var: &NcVariable, name: &str) -> Option<f64> {
    var.attribute(name)?.value.as_f64()
}

fn range_of(a: &netcdf_reader::NcAttribute) -> Option<(f64, f64)> {
    let v = a.value.as_f64_vec()?;
    (v.len() >= 2).then(|| (v[0], v[1]))
}

/// netCDF-C の既定の欠損の値。`_FillValue` がなくても、書かれなかった値はこれになる。
/// byte は既定の値を欠損にしない（netCDF の Python の版と同じ。byte は全部の値を使うことが多いため）。
fn default_fill(t: &NcType) -> Option<f64> {
    Some(match t {
        NcType::Short => -32767.0,
        NcType::Int => -2147483647.0,
        NcType::Float => f64::from(9.969_21e36_f32),
        NcType::Double => 9.969_209_968_386_869e36,
        NcType::UShort => 65535.0,
        NcType::UInt => 4294967295.0,
        NcType::Int64 => -9223372036854775806.0,
        NcType::UInt64 => 18446744073709551614.0,
        _ => return None,
    })
}

/// 型の比べ方だけに使う、値の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    Text,
}

fn kind(v: &NcAttrValue) -> Kind {
    match v {
        NcAttrValue::Bytes(_) => Kind::I8,
        NcAttrValue::UBytes(_) => Kind::U8,
        NcAttrValue::Shorts(_) => Kind::I16,
        NcAttrValue::UShorts(_) => Kind::U16,
        NcAttrValue::Ints(_) => Kind::I32,
        NcAttrValue::UInts(_) => Kind::U32,
        NcAttrValue::Int64s(_) => Kind::I64,
        NcAttrValue::UInt64s(_) => Kind::U64,
        NcAttrValue::Floats(_) => Kind::F32,
        NcAttrValue::Doubles(_) => Kind::F64,
        _ => Kind::Text,
    }
}

fn stored_kind(t: &NcType) -> Kind {
    match t {
        NcType::Byte => Kind::I8,
        NcType::UByte => Kind::U8,
        NcType::Short => Kind::I16,
        NcType::UShort => Kind::U16,
        NcType::Int => Kind::I32,
        NcType::UInt => Kind::U32,
        NcType::Int64 => Kind::I64,
        NcType::UInt64 => Kind::U64,
        NcType::Float => Kind::F32,
        NcType::Double => Kind::F64,
        _ => Kind::Text,
    }
}
