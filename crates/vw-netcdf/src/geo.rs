//! 変数の画素ごとの緯度・経度。座標の変数（1 本の軸、画素ごとの表）か、`grid_mapping` の図法から作る。

use std::f64::consts::FRAC_PI_2;

use netcdf_reader::{NcFile, NcGroup, NcVariable};
use vw_core::geo::{Cells, GeoGrid};

use crate::layout::{self, Variable};
use crate::projection::{Ellipsoid, PolarScale, Projection, unrotate};
use crate::values::number;

/// 座標の変数の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Latitude,
    Longitude,
    /// 回転極の格子の緯度・経度（度）。
    GridLatitude,
    GridLongitude,
    /// 図法の x・y。値は m に直す倍率を持つ。
    ProjectionX,
    ProjectionY,
    Other,
}

fn text(var: &NcVariable, name: &str) -> Option<String> {
    var.attribute(name)?.value.as_string()
}

fn role(var: &NcVariable) -> Role {
    let standard = text(var, "standard_name").unwrap_or_default();
    let units = text(var, "units").unwrap_or_default().to_ascii_lowercase();
    match standard.as_str() {
        "latitude" => return Role::Latitude,
        "longitude" => return Role::Longitude,
        "grid_latitude" => return Role::GridLatitude,
        "grid_longitude" => return Role::GridLongitude,
        "projection_x_coordinate" => return Role::ProjectionX,
        "projection_y_coordinate" => return Role::ProjectionY,
        _ => {}
    }
    match units.as_str() {
        "degrees_north" | "degree_north" | "degree_n" | "degrees_n" | "degreen" | "degreesn" => {
            Role::Latitude
        }
        "degrees_east" | "degree_east" | "degree_e" | "degrees_e" | "degreee" | "degreese" => {
            Role::Longitude
        }
        _ => Role::Other,
    }
}

/// x・y の値を m にする倍率。
fn meters(var: &NcVariable) -> f64 {
    match text(var, "units").unwrap_or_default().trim() {
        "km" | "kilometer" | "kilometers" | "kilometre" | "kilometres" => 1000.0,
        _ => 1.0,
    }
}

/// `var` を読むための、ルートからの道筋。
fn path_of(scope: &[&NcGroup], var: &NcVariable) -> String {
    let depth = scope
        .iter()
        .rposition(|g| g.variables.iter().any(|v| std::ptr::eq(v, var)))
        .unwrap_or(0);
    let groups: Vec<&str> = scope[1..=depth].iter().map(|g| g.name.as_str()).collect();
    if groups.is_empty() {
        var.name.clone()
    } else {
        format!("{}/{}", groups.join("/"), var.name)
    }
}

fn find<'a>(scope: &[&'a NcGroup], name: &str) -> Option<&'a NcVariable> {
    scope
        .iter()
        .rev()
        .find_map(|g| g.variables.iter().find(|v| v.name == name))
}

fn read(file: &NcFile, scope: &[&NcGroup], var: &NcVariable) -> Option<Vec<f64>> {
    let values = file.read_variable_as_f64(&path_of(scope, var)).ok()?;
    Some(values.iter().copied().collect())
}

/// 経度の軸が 0〜360° のとき、−180〜180° に回すために列を回す数（180° 以上の最初の列）。
/// 回さなくてよい（180° を超える経度がない、軸がない、昇順でない）なら `None`。
pub fn longitude_roll(file: &NcFile, layout: &Variable) -> Option<usize> {
    let scope = layout::scope(file, &layout.path);
    let col = layout::coordinate(&layout.dims[layout.cols].0, &scope)?;
    if role(col) != Role::Longitude {
        return None;
    }
    let lon = read(file, &scope, col)?;
    if !lon.windows(2).all(|w| w[0] < w[1]) {
        return None;
    }
    let first = lon.iter().position(|&v| v >= 180.0)?;
    (first > 0 && lon.iter().all(|&v| v < 360.0 + 1e-9)).then_some(first)
}

/// 画素ごとの緯度・経度。画素の並びは表示の向き（`flipped` なら上下を返した後、
/// `roll` があれば列を回した後）。座標が分からなければ `None`。
pub fn grid(
    file: &NcFile,
    layout: &Variable,
    flipped: bool,
    roll: Option<usize>,
) -> Option<GeoGrid> {
    let scope = layout::scope(file, &layout.path);
    let var = file.variable(&layout.path).ok()?;
    let (w, h) = (layout.width() as usize, layout.height() as usize);
    let row_dim = &layout.dims[layout.rows].0;
    let col_dim = &layout.dims[layout.cols].0;
    let rows = layout::coordinate(row_dim, &scope);
    let cols = layout::coordinate(col_dim, &scope);
    let mut cells = axes(file, &scope, rows, cols, w, h)
        .or_else(|| table(file, &scope, var, row_dim, col_dim, w, h))
        .or_else(|| projected(file, &scope, var, rows?, cols?, w, h))?;
    if flipped {
        flip(&mut cells, w, h);
    }
    if let (Some(k), Cells::Axes { longitudes, .. }) = (roll, &mut cells) {
        longitudes.rotate_left(k);
        for lon in longitudes.iter_mut().filter(|l| **l >= 180.0) {
            *lon -= 360.0;
        }
    }
    Some(GeoGrid {
        width: w as u32,
        height: h as u32,
        cells,
    })
}

fn axes(
    file: &NcFile,
    scope: &[&NcGroup],
    rows: Option<&NcVariable>,
    cols: Option<&NcVariable>,
    w: usize,
    h: usize,
) -> Option<Cells> {
    let (rows, cols) = (rows?, cols?);
    if (role(rows), role(cols)) != (Role::Latitude, Role::Longitude) {
        return None;
    }
    let latitudes = read(file, scope, rows)?;
    let longitudes = read(file, scope, cols)?;
    (latitudes.len() == h && longitudes.len() == w).then_some(Cells::Axes {
        longitudes,
        latitudes,
    })
}

/// `coordinates` 属性に書かれた、画素ごとの緯度・経度の表（曲がった格子）。
fn table(
    file: &NcFile,
    scope: &[&NcGroup],
    var: &NcVariable,
    row_dim: &str,
    col_dim: &str,
    w: usize,
    h: usize,
) -> Option<Cells> {
    let names = text(var, "coordinates")?;
    let mut lat = None;
    let mut lon = None;
    for name in names.split_whitespace() {
        let Some(c) = find(scope, name) else {
            continue;
        };
        let dims: Vec<&str> = c.dimensions.iter().map(|d| d.name.as_str()).collect();
        let order = match dims.as_slice() {
            [a, b] if *a == row_dim && *b == col_dim => false,
            [a, b] if *a == col_dim && *b == row_dim => true,
            _ => continue,
        };
        match role(c) {
            Role::Latitude => lat = Some((c, order)),
            Role::Longitude => lon = Some((c, order)),
            _ => {}
        }
    }
    let ((lat, lat_t), (lon, lon_t)) = (lat?, lon?);
    vw_core::limits::check_image(w as u64, h as u64, 8).ok()?;
    let as_rows = |values: Vec<f64>, transposed: bool| -> Option<Vec<f32>> {
        if values.len() != w * h {
            return None;
        }
        Some(if transposed {
            (0..w * h)
                .map(|i| values[(i % w) * h + i / w] as f32)
                .collect()
        } else {
            values.into_iter().map(|v| v as f32).collect()
        })
    };
    Some(Cells::Table {
        latitudes: as_rows(read(file, scope, lat)?, lat_t)?,
        longitudes: as_rows(read(file, scope, lon)?, lon_t)?,
    })
}

/// x・y（座標の変数の値）から `(緯度, 経度)`。
type ToLatLon = dyn Fn(f64, f64) -> Option<(f64, f64)>;

/// `grid_mapping` の図法で、x・y の軸から画素ごとの緯度・経度を出す。
fn projected(
    file: &NcFile,
    scope: &[&NcGroup],
    var: &NcVariable,
    rows: &NcVariable,
    cols: &NcVariable,
    w: usize,
    h: usize,
) -> Option<Cells> {
    // `grid_mapping` は名前だけか、「名前: 座標 …」の長い形。最初の名前を使う。
    let name = text(var, "grid_mapping")?;
    let name = name.split_whitespace().next()?.trim_end_matches(':');
    let mapping = find(scope, name)?;
    let kind = text(mapping, "grid_mapping_name")?;
    let ys = read(file, scope, rows)?;
    let xs = read(file, scope, cols)?;
    if ys.len() != h || xs.len() != w {
        return None;
    }
    vw_core::limits::check_image(w as u64, h as u64, 8).ok()?;
    let point: Box<ToLatLon> = if kind == "rotated_latitude_longitude" {
        if (role(rows), role(cols)) != (Role::GridLatitude, Role::GridLongitude)
            && !(units_are_degrees(rows) && units_are_degrees(cols))
        {
            return None;
        }
        let pole = (
            number(mapping, "grid_north_pole_latitude")?,
            number(mapping, "grid_north_pole_longitude")?,
        );
        let in_grid = number(mapping, "north_pole_grid_longitude").unwrap_or(0.0);
        Box::new(move |x, y| Some(unrotate(y, x, pole, in_grid)))
    } else {
        let projection = projection(mapping, &kind)?;
        let (sx, sy) = (meters(cols), meters(rows));
        Box::new(move |x, y| projection.inverse(x * sx, y * sy))
    };
    let mut latitudes = Vec::with_capacity(w * h);
    let mut longitudes = Vec::with_capacity(w * h);
    for &y in &ys {
        for &x in &xs {
            let (lat, lon) = point(x, y).unwrap_or((f64::NAN, f64::NAN));
            latitudes.push(lat as f32);
            longitudes.push(lon as f32);
        }
    }
    Some(Cells::Table {
        latitudes,
        longitudes,
    })
}

fn units_are_degrees(var: &NcVariable) -> bool {
    text(var, "units").is_some_and(|u| u.trim().starts_with("degree"))
}

/// `grid_mapping` の変数の属性から図法を作る。対応していない図法は `None`。
fn projection(m: &NcVariable, kind: &str) -> Option<Projection> {
    let deg = |name: &str| number(m, name).map(f64::to_radians);
    let all = |name: &str| {
        m.attribute(name)?
            .value
            .as_f64_vec()
            .map(|v| v.into_iter().map(f64::to_radians).collect::<Vec<_>>())
    };
    let ellipsoid = ellipsoid(m);
    let fe = number(m, "false_easting").unwrap_or(0.0);
    let fn_ = number(m, "false_northing").unwrap_or(0.0);
    let parallels = || {
        let p = all("standard_parallel")?;
        Some((*p.first()?, *p.get(1).unwrap_or(&p[0])))
    };
    Some(match kind {
        "lambert_conformal_conic" => Projection::LambertConformalConic {
            ellipsoid,
            parallels: parallels()?,
            origin_lat: deg("latitude_of_projection_origin")?,
            central_lon: deg("longitude_of_central_meridian")?,
            false_easting: fe,
            false_northing: fn_,
        },
        "polar_stereographic" => {
            let origin = number(m, "latitude_of_projection_origin")?;
            let scale = match (
                number(m, "standard_parallel"),
                number(m, "scale_factor_at_projection_origin"),
            ) {
                (Some(p), _) => PolarScale::StandardParallel(p.abs().to_radians()),
                (None, Some(k)) => PolarScale::ScaleAtPole(k),
                (None, None) => PolarScale::StandardParallel(FRAC_PI_2),
            };
            Projection::PolarStereographic {
                ellipsoid,
                north: origin > 0.0,
                central_lon: deg("straight_vertical_longitude_from_pole")
                    .or_else(|| deg("longitude_of_projection_origin"))?,
                scale,
                false_easting: fe,
                false_northing: fn_,
            }
        }
        "mercator" => {
            let k0 = match (
                deg("standard_parallel"),
                number(m, "scale_factor_at_projection_origin"),
            ) {
                (Some(p), _) => ellipsoid.m(p),
                (None, Some(k)) => k,
                (None, None) => 1.0,
            };
            Projection::Mercator {
                ellipsoid,
                central_lon: deg("longitude_of_projection_origin")?,
                k0,
                false_easting: fe,
                false_northing: fn_,
            }
        }
        "transverse_mercator" => Projection::TransverseMercator {
            ellipsoid,
            origin_lat: deg("latitude_of_projection_origin").unwrap_or(0.0),
            central_lon: deg("longitude_of_central_meridian")?,
            k0: number(m, "scale_factor_at_central_meridian").unwrap_or(1.0),
            false_easting: fe,
            false_northing: fn_,
        },
        "albers_conical_equal_area" => Projection::AlbersEqualArea {
            ellipsoid,
            parallels: parallels()?,
            origin_lat: deg("latitude_of_projection_origin")?,
            central_lon: deg("longitude_of_central_meridian")?,
            false_easting: fe,
            false_northing: fn_,
        },
        _ => return None,
    })
}

/// `grid_mapping` の変数の地球の形。書かれていなければ WGS84。
/// CF 規約は既定を決めていないが、今の観測やモデルの多くが使う楕円体なので、それを仮定する。
fn ellipsoid(m: &NcVariable) -> Ellipsoid {
    if let Some(r) = number(m, "earth_radius") {
        return Ellipsoid::sphere(r);
    }
    match (
        number(m, "semi_major_axis"),
        number(m, "inverse_flattening"),
        number(m, "semi_minor_axis"),
    ) {
        (Some(a), Some(f), _) => Ellipsoid::from_flattening(a, f),
        (Some(a), None, Some(b)) => Ellipsoid::from_axes(a, b),
        (Some(a), None, None) => Ellipsoid::sphere(a),
        _ => Ellipsoid::WGS84,
    }
}

fn flip(cells: &mut Cells, w: usize, h: usize) {
    match cells {
        Cells::Axes { latitudes, .. } => latitudes.reverse(),
        Cells::Table {
            latitudes,
            longitudes,
        } => {
            for v in [latitudes, longitudes] {
                let rows: Vec<Vec<f32>> = v.chunks(w).rev().map(<[f32]>::to_vec).collect();
                *v = rows.concat();
                debug_assert_eq!(v.len(), w * h);
            }
        }
    }
}

/// 偏差（平年との差）の変数か。`standard_name` に `anomaly` が入るか、`valid_range` が 0 をはさんで対称。
/// 0 を真ん中にした色で見ると、正と負がひと目で分かるため。
pub fn is_anomaly(var: &NcVariable) -> bool {
    let named = anomaly_named(
        &var.name,
        text(var, "standard_name").as_deref(),
        text(var, "long_name").as_deref(),
    );
    let symmetric = var
        .attribute("valid_range")
        .and_then(|a| a.value.as_f64_vec())
        .is_some_and(|v| v.len() >= 2 && v[1] > 0.0 && (v[0] + v[1]).abs() <= v[1] * 1e-6);
    named || symmetric
}

/// 名前・`standard_name`・`long_name` が偏差を表すか。変数の名前は `sst_anom` のように略すことが多い。
fn anomaly_named(name: &str, standard_name: Option<&str>, long_name: Option<&str>) -> bool {
    let has = |s: &str, word: &str| s.to_lowercase().contains(word);
    let word_starts_anom = name
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w.starts_with("anom"));
    word_starts_anom
        || standard_name.is_some_and(|s| has(s, "anomaly"))
        || long_name.is_some_and(|s| has(s, "anomal") || s.contains("偏差"))
}

#[cfg(test)]
mod tests {
    use super::anomaly_named;

    #[test]
    fn anomalies_are_found_by_name_standard_name_or_long_name() {
        assert!(anomaly_named("sst_anom", None, None));
        assert!(!anomaly_named("manometer", None, None));
        assert!(anomaly_named("t", Some("air_temperature_anomaly"), None));
        assert!(anomaly_named("t", None, Some("Air Temperature Anomalies")));
        assert!(anomaly_named("t", None, Some("気温の平年偏差")));
        assert!(!anomaly_named(
            "t2m",
            Some("air_temperature"),
            Some("2 m temperature")
        ));
    }
}
