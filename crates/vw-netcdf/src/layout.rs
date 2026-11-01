//! ファイルの中の、画像にできる変数と、その縦・横・面の次元。

use netcdf_reader::cf::{CfAxisType, identify_axis};
use netcdf_reader::{NcFile, NcGroup, NcType, NcVariable};

/// 画像にできる 1 つの変数。
#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    /// ルートからの道筋（`sea_surface/analysed_sst`）。ルートの変数は名前だけ。
    pub path: String,
    /// 次元の名前と大きさ。ファイルの並びのまま。
    pub dims: Vec<(String, u64)>,
    pub dtype: NcType,
    /// 縦（行）と横（列）にする次元の添字。
    pub rows: usize,
    pub cols: usize,
}

impl Variable {
    pub fn width(&self) -> u64 {
        self.dims[self.cols].1
    }

    pub fn height(&self) -> u64 {
        self.dims[self.rows].1
    }

    /// 縦と横の他の次元の添字。外側から。
    pub fn plane_dims(&self) -> Vec<usize> {
        (0..self.dims.len())
            .filter(|&d| d != self.rows && d != self.cols)
            .collect()
    }

    /// 面の数（縦と横の他の次元の大きさの積）。
    pub fn planes(&self) -> usize {
        self.plane_dims()
            .iter()
            .map(|&d| self.dims[d].1 as usize)
            .product()
    }

    /// `plane` 番目の面での、縦と横の他の次元の添字。C の順（最後の次元がいちばん速く変わる）。
    pub fn plane_index(&self, plane: usize) -> Vec<(usize, u64)> {
        let dims = self.plane_dims();
        let mut rest = plane as u64;
        let mut index = vec![(0, 0); dims.len()];
        for (slot, &d) in index.iter_mut().zip(&dims).rev() {
            let size = self.dims[d].1.max(1);
            *slot = (d, rest % size);
            rest /= size;
        }
        index
    }

    /// 横の次元が縦の次元より前に並ぶ（`(lon, lat)` の順の古いファイル）。
    pub fn transposed(&self) -> bool {
        self.cols < self.rows
    }

    /// 面の大きさ（画素の数）。
    pub fn plane_pixels(&self) -> u64 {
        self.width() * self.height()
    }
}

/// 画像にできる変数。ファイルの並び（グループは親の変数の後）のまま。
pub fn variables(file: &NcFile) -> Vec<Variable> {
    let mut out = Vec::new();
    if let Ok(root) = file.root_group() {
        collect(root, &[], "", &mut out);
    }
    out
}

/// 最初に見せる変数。面の画素がいちばん多い変数（同じなら先のもの）。
pub fn default_variable(variables: &[Variable]) -> Option<&Variable> {
    variables.iter().rev().max_by_key(|v| v.plane_pixels())
}

fn collect<'a>(group: &'a NcGroup, parents: &[&'a NcGroup], prefix: &str, out: &mut Vec<Variable>) {
    let mut scope: Vec<&NcGroup> = parents.to_vec();
    scope.push(group);
    let bounds: Vec<String> = group
        .variables
        .iter()
        .filter_map(|v| v.attribute("bounds")?.value.as_string())
        .collect();
    for var in &group.variables {
        if !numeric(&var.dtype)
            || var.ndim() < 2
            || var.is_coordinate_variable()
            || bounds.iter().any(|b| b == &var.name)
            || var.dimensions.iter().any(|d| d.size == 0)
        {
            continue;
        }
        let (rows, cols) = axes(var, &scope);
        out.push(Variable {
            path: format!("{prefix}{}", var.name),
            dims: var
                .dimensions
                .iter()
                .map(|d| (d.name.clone(), d.size))
                .collect(),
            dtype: var.dtype.clone(),
            rows,
            cols,
        });
    }
    for child in &group.groups {
        collect(child, &scope, &format!("{prefix}{}/", child.name), out);
    }
}

fn numeric(t: &NcType) -> bool {
    matches!(
        t,
        NcType::Byte
            | NcType::Short
            | NcType::Int
            | NcType::Float
            | NcType::Double
            | NcType::UByte
            | NcType::UShort
            | NcType::UInt
            | NcType::Int64
            | NcType::UInt64
    )
}

/// 縦と横の次元。座標の変数の `axis`・`standard_name`・`units` で Y と X が両方分かればそれ、
/// 分からなければ最後の 2 つ。
fn axes(var: &NcVariable, scope: &[&NcGroup]) -> (usize, usize) {
    let n = var.ndim();
    let kind = |d: usize| {
        coordinate(&var.dimensions[d].name, scope).map_or(CfAxisType::Unknown, identify_axis)
    };
    let y = (0..n).rev().find(|&d| kind(d) == CfAxisType::Y);
    let x = (0..n).rev().find(|&d| kind(d) == CfAxisType::X);
    match (y, x) {
        (Some(y), Some(x)) if y != x => (y, x),
        _ => (n - 2, n - 1),
    }
}

/// 次元 `name` の座標の変数。内側のグループから外へ探す（NetCDF-4 の次元は親のグループのものも見える）。
pub fn coordinate<'a>(name: &str, scope: &[&'a NcGroup]) -> Option<&'a NcVariable> {
    scope.iter().rev().find_map(|g| {
        g.variables
            .iter()
            .find(|v| v.is_coordinate_variable_for(name))
    })
}

/// 変数 `path` のあるグループと、ルートからそこまでのグループ。
pub fn scope<'a>(file: &'a NcFile, path: &str) -> Vec<&'a NcGroup> {
    let Ok(root) = file.root_group() else {
        return Vec::new();
    };
    let mut scope = vec![root];
    let parts: Vec<&str> = path.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        let Some(child) = scope
            .last()
            .and_then(|g| g.groups.iter().find(|c| c.name == *part))
        else {
            break;
        };
        scope.push(child);
    }
    scope
}
