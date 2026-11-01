//! メタデータのペインと検索カタログに出す、属性・次元・変数の一覧と、面の札。

use netcdf_reader::cf::{decode_time_exact, time_ref_from_variable};
use netcdf_reader::{
    NcAttrValue, NcAttribute, NcFile, NcFormat, NcGroup, NcSliceInfo, NcSliceInfoElem,
};
use vw_core::Metadata;

use crate::layout::{self, Variable};
use crate::{GLOBAL_GROUP, GROUP, VARIABLE_GROUP, VARIABLES_GROUP};

/// 値の長さの上限（バイト）。データセットの決まりと同じく、超えたら長さだけ書く。
const VALUE_LIMIT: usize = 2048;

pub fn format_name(f: NcFormat) -> &'static str {
    match f {
        NcFormat::Classic => "NetCDF-3 classic",
        NcFormat::Offset64 => "NetCDF-3 64-bit offset",
        NcFormat::Cdf5 => "NetCDF-3 64-bit data",
        NcFormat::Nc4 => "NetCDF-4",
        NcFormat::Nc4Classic => "NetCDF-4 classic model",
    }
}

/// 属性の値を 1 行の文字列にする。
pub fn attr_text(v: &NcAttrValue) -> String {
    fn join<T: ToString>(v: &[T]) -> String {
        v.iter().map(T::to_string).collect::<Vec<_>>().join(", ")
    }
    let text = match v {
        NcAttrValue::Chars(s) => s.trim_end_matches('\0').to_owned(),
        NcAttrValue::Strings(v) => v.join(", "),
        NcAttrValue::Bytes(v) => join(v),
        NcAttrValue::UBytes(v) => join(v),
        NcAttrValue::Shorts(v) => join(v),
        NcAttrValue::UShorts(v) => join(v),
        NcAttrValue::Ints(v) => join(v),
        NcAttrValue::UInts(v) => join(v),
        NcAttrValue::Int64s(v) => join(v),
        NcAttrValue::UInt64s(v) => join(v),
        NcAttrValue::Floats(v) => join(v),
        NcAttrValue::Doubles(v) => join(v),
        #[allow(unreachable_patterns)]
        _ => "(not shown)".to_owned(),
    };
    if text.len() > VALUE_LIMIT {
        format!("(over 2KB: {} bytes)", text.len())
    } else {
        text
    }
}

fn push_attrs(m: &mut Metadata, group: &str, attrs: &[NcAttribute]) {
    for a in attrs {
        m.push(group, &a.name, attr_text(&a.value));
    }
}

/// `chosen` を見せているときのメタデータ（面の札は [`plane_label`]）。
pub fn metadata(file: &NcFile, variables: &[Variable], chosen: &Variable) -> Metadata {
    let mut m = Metadata::default();
    m.push(GROUP, crate::FORMAT, format_name(file.format()));
    m.push(GROUP, crate::VARIABLE, &chosen.path);
    m.push(GROUP, crate::DIMENSIONS, dims_text(&chosen.dims));
    if let Ok(var) = file.variable(&chosen.path) {
        m.push(GROUP, crate::TYPE, type_name(&var.dtype));
        if let Some(units) = var.attribute("units").and_then(|a| a.value.as_string()) {
            m.push(GROUP, crate::UNITS, units.trim());
        }
        if let Some(p) = crate::values::Packing::of(var) {
            m.push(GROUP, crate::SCALE_FACTOR, p.scale);
            m.push(GROUP, crate::ADD_OFFSET, p.offset);
        }
        push_attrs(&mut m, VARIABLE_GROUP, &var.attributes);
    }
    for v in variables {
        let units = file
            .variable(&v.path)
            .ok()
            .and_then(|var| var.attribute("units")?.value.as_string())
            .unwrap_or_default();
        let text = format!("{} {}", dims_text(&v.dims), type_name(&v.dtype));
        let text = if units.is_empty() {
            text
        } else {
            format!("{text} [{}]", units.trim())
        };
        m.push(VARIABLES_GROUP, &v.path, text);
    }
    if let Ok(attrs) = file.global_attributes() {
        push_attrs(&mut m, GLOBAL_GROUP, attrs);
    }
    m
}

/// 検索カタログに入れる分。全体の属性と、変数ごとの名前・単位・`standard_name`。
pub fn catalog(file: &NcFile, variables: &[Variable]) -> Metadata {
    let mut m = Metadata::default();
    m.push(GROUP, crate::FORMAT, format_name(file.format()));
    if let Ok(attrs) = file.global_attributes() {
        push_attrs(&mut m, GLOBAL_GROUP, attrs);
    }
    for v in variables {
        let Ok(var) = file.variable(&v.path) else {
            continue;
        };
        let text = |name: &str| {
            var.attribute(name)
                .and_then(|a| a.value.as_string())
                .unwrap_or_default()
        };
        m.push(
            VARIABLES_GROUP,
            &v.path,
            format!("{} {}", text("units"), text("standard_name"))
                .trim()
                .to_owned(),
        );
    }
    m
}

pub fn dims_text(dims: &[(String, u64)]) -> String {
    dims.iter()
        .map(|(n, s)| format!("{n}={s}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn type_name(t: &netcdf_reader::NcType) -> &'static str {
    use netcdf_reader::NcType as T;
    match t {
        T::Byte => "byte",
        T::UByte => "ubyte",
        T::Short => "short",
        T::UShort => "ushort",
        T::Int => "int",
        T::UInt => "uint",
        T::Int64 => "int64",
        T::UInt64 => "uint64",
        T::Float => "float",
        T::Double => "double",
        T::Char => "char",
        T::String => "string",
        _ => "other",
    }
}

/// 面の札。縦と横の他の次元ごとに「名前 = 値」をカンマでつなぐ。時刻は日時にする。
/// 座標の変数がなければ添字（0 始まり）。
pub fn plane_label(file: &NcFile, layout: &Variable, plane: usize) -> Option<String> {
    let index = layout.plane_index(plane);
    if index.is_empty() {
        return None;
    }
    let scope = layout::scope(file, &layout.path);
    let parts: Vec<String> = index
        .iter()
        .map(|&(d, i)| {
            let name = &layout.dims[d].0;
            let value = coordinate_text(file, &scope, name, i).unwrap_or_else(|| i.to_string());
            format!("{name} = {value}")
        })
        .collect();
    Some(parts.join(", "))
}

fn coordinate_text(file: &NcFile, scope: &[&NcGroup], dim: &str, i: u64) -> Option<String> {
    let (depth, var) = scope.iter().enumerate().rev().find_map(|(depth, g)| {
        g.variables
            .iter()
            .find(|v| v.is_coordinate_variable_for(dim))
            .map(|v| (depth, v))
    })?;
    let path = scoped_path(&scope[..=depth], &var.name);
    let slice = NcSliceInfo {
        selections: vec![NcSliceInfoElem::Index(i)],
    };
    let value = file
        .read_variable_slice_as_f64(&path, &slice)
        .ok()?
        .iter()
        .next()
        .copied()?;
    if let Ok(Some(time)) = time_ref_from_variable(var)
        && let Ok(t) = decode_time_exact(value, &time)
    {
        let date = format!("{:04}-{:02}-{:02}", t.year, t.month, t.day);
        return Some(if (t.hour, t.minute, t.second) == (0, 0, 0) {
            date
        } else if t.second == 0 {
            format!("{date} {:02}:{:02}", t.hour, t.minute)
        } else {
            format!("{date} {:02}:{:02}:{:02}", t.hour, t.minute, t.second)
        });
    }
    let units = var
        .attribute("units")
        .and_then(|a| a.value.as_string())
        .unwrap_or_default();
    let number = if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    };
    Some(if units.trim().is_empty() {
        number
    } else {
        format!("{number} {}", units.trim())
    })
}

/// `scope` の最も内側のグループにある `name` の道筋。
fn scoped_path(scope: &[&NcGroup], name: &str) -> String {
    let groups: Vec<&str> = scope.iter().skip(1).map(|g| g.name.as_str()).collect();
    if groups.is_empty() {
        name.to_owned()
    } else {
        format!("{}/{name}", groups.join("/"))
    }
}
