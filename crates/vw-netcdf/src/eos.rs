//! HDF-EOS の構造の文字（`StructMetadata.0`）。軌道のデータ（swath）の「緯度・経度が何画素おきか」と、
//! 格子のデータ（grid）の図法・角の座標を読む。HDF-EOS2（HDF4）と HDF-EOS5（HDF5）で書き方は同じ。
//!
//! 文字は ODL（`GROUP=…` / `OBJECT=…` / `名前=値` / `END_GROUP=…` の入れ子）。

use std::f64::consts::FRAC_PI_2;

use crate::projection::{Ellipsoid, PolarScale, Projection};

/// ODL の 1 段（GROUP か OBJECT）。
#[derive(Debug, Default)]
struct Node {
    values: Vec<(String, String)>,
    children: Vec<(String, Node)>,
}

impl Node {
    fn get(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn text(&self, key: &str) -> Option<String> {
        self.get(key).map(unquote)
    }

    fn number(&self, key: &str) -> Option<f64> {
        self.get(key)?.trim().parse().ok()
    }

    fn list(&self, key: &str) -> Option<Vec<String>> {
        let v = self.get(key)?.trim();
        let inner = v.strip_prefix('(')?.strip_suffix(')')?;
        Some(inner.split(',').map(unquote).collect())
    }

    fn numbers(&self, key: &str) -> Option<Vec<f64>> {
        self.list(key)?
            .iter()
            .map(|s| s.trim().parse().ok())
            .collect()
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.children
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, c)| c)
    }

    fn all(&self) -> impl Iterator<Item = &Node> {
        self.children.iter().map(|(_, c)| c)
    }
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_owned()
}

/// ODL を木にする。崩れた行は飛ばす（壊れた文字でも、読めたところまでは使う）。
fn parse_odl(text: &str) -> Node {
    let mut stack: Vec<(String, Node)> = vec![(String::new(), Node::default())];
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, mut value) = (key.trim(), value.trim().to_owned());
        // `(…)` の値は、次の行にまたがることがある。
        if value.starts_with('(') {
            while !value.ends_with(')') {
                let Some(next) = lines.next() else { break };
                value.push_str(next);
            }
        }
        match key {
            "GROUP" | "OBJECT" => stack.push((value, Node::default())),
            "END_GROUP" | "END_OBJECT" => {
                if stack.len() > 1 {
                    let (name, node) = stack.pop().expect("more than the root");
                    stack
                        .last_mut()
                        .expect("root")
                        .1
                        .children
                        .push((name, node));
                }
            }
            _ => stack
                .last_mut()
                .expect("root")
                .1
                .values
                .push((key.to_owned(), value)),
        }
    }
    while stack.len() > 1 {
        let (name, node) = stack.pop().expect("more than the root");
        stack
            .last_mut()
            .expect("root")
            .1
            .children
            .push((name, node));
    }
    stack.pop().expect("root").1
}

/// データの次元と緯度・経度の次元の対応。データの `i` 番目は、緯度・経度の `(i − offset) / increment` 番目。
#[derive(Debug, Clone, PartialEq)]
pub struct DimensionMap {
    pub geo: String,
    pub data: String,
    pub offset: f64,
    pub increment: f64,
}

impl DimensionMap {
    /// データの `i` 番目に当たる、緯度・経度の（小数の）位置。
    pub fn geo_index(&self, i: f64) -> f64 {
        if self.increment > 0.0 {
            (i - self.offset) / self.increment
        } else {
            // 負の増分は、緯度・経度のほうが細かいことを表す。
            self.offset + i * -self.increment
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub dims: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Swath {
    pub name: String,
    pub maps: Vec<DimensionMap>,
    pub geo_fields: Vec<Field>,
    pub data_fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub name: String,
    pub columns: usize,
    pub rows: usize,
    pub upper_left: (f64, f64),
    pub lower_right: (f64, f64),
    /// `GCTP_` の後ろ（`SNSOID` など）。HDF-EOS5 の `HE5_` は外す。
    pub projection: String,
    pub params: Vec<f64>,
    pub sphere: i32,
    pub zone: i32,
    /// `(0, 0)` の画素が下の端か（`HDFE_GD_LL`・`HDFE_GD_LR`）。
    pub origin_bottom: bool,
    /// `(0, 0)` の画素が右の端か。
    pub origin_right: bool,
    /// 座標が画素の角を指すか（`HDFE_CORNER`）。既定は中心。
    pub corner: bool,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Structure {
    pub swaths: Vec<Swath>,
    pub grids: Vec<Grid>,
}

fn strip_he5(s: &str) -> &str {
    s.strip_prefix("HE5_").unwrap_or(s)
}

fn field(node: &Node, key: &str) -> Option<Field> {
    Some(Field {
        name: node.text(key)?,
        dims: node.list("DimList").unwrap_or_default(),
    })
}

pub fn parse(text: &str) -> Structure {
    let root = parse_odl(text);
    let mut out = Structure::default();
    if let Some(swaths) = root.child("SwathStructure") {
        for s in swaths.all() {
            let Some(name) = s.text("SwathName") else {
                continue;
            };
            let maps = s
                .child("DimensionMap")
                .into_iter()
                .flat_map(Node::all)
                .filter_map(|m| {
                    Some(DimensionMap {
                        geo: m.text("GeoDimension")?,
                        data: m.text("DataDimension")?,
                        offset: m.number("Offset")?,
                        increment: m.number("Increment")?,
                    })
                })
                .filter(|m| m.increment != 0.0)
                .collect();
            let fields = |group: &str, key: &str| -> Vec<Field> {
                s.child(group)
                    .into_iter()
                    .flat_map(Node::all)
                    .filter_map(|f| field(f, key))
                    .collect()
            };
            out.swaths.push(Swath {
                name,
                maps,
                geo_fields: fields("GeoField", "GeoFieldName"),
                data_fields: fields("DataField", "DataFieldName"),
            });
        }
    }
    if let Some(grids) = root.child("GridStructure") {
        for g in grids.all() {
            let read = || -> Option<Grid> {
                let ul = g.numbers("UpperLeftPointMtrs")?;
                let lr = g.numbers("LowerRightMtrs")?;
                let origin = g.text("GridOrigin").unwrap_or_default();
                let origin = strip_he5(&origin).trim_start_matches("HDFE_GD_").to_owned();
                Some(Grid {
                    name: g.text("GridName")?,
                    columns: g.number("XDim")? as usize,
                    rows: g.number("YDim")? as usize,
                    upper_left: (*ul.first()?, *ul.get(1)?),
                    lower_right: (*lr.first()?, *lr.get(1)?),
                    projection: strip_he5(&g.text("Projection")?)
                        .trim_start_matches("GCTP_")
                        .to_owned(),
                    params: g.numbers("ProjParams").unwrap_or_default(),
                    sphere: g.number("SphereCode").unwrap_or(0.0) as i32,
                    zone: g.number("ZoneCode").unwrap_or(0.0) as i32,
                    origin_bottom: origin.starts_with('L'),
                    origin_right: origin.ends_with('R'),
                    corner: g
                        .text("PixelRegistration")
                        .is_some_and(|r| r.ends_with("CORNER")),
                })
            };
            if let Some(grid) = read() {
                out.grids.push(grid);
            }
        }
    }
    out
}

/// GCTP の角度（DDDMMMSSS.SS の詰めた度・分・秒）を度に。
pub fn packed_dms(v: f64) -> f64 {
    let a = v.abs();
    let deg = (a / 1e6).trunc();
    let min = ((a - deg * 1e6) / 1e3).trunc();
    let sec = a - deg * 1e6 - min * 1e3;
    (deg + min / 60.0 + sec / 3600.0).copysign(v)
}

impl Grid {
    fn param(&self, i: usize) -> f64 {
        self.params.get(i).copied().unwrap_or(0.0)
    }

    fn angle(&self, i: usize) -> f64 {
        packed_dms(self.param(i)).to_radians()
    }

    /// 地球の形。`ProjParams` の 1・2 番目に書いてあればそれ、なければ `SphereCode` の楕円体。
    /// UTM の 1・2 番目は帯を決める経度・緯度なので、`SphereCode` だけを見る。
    fn ellipsoid(&self) -> Ellipsoid {
        let (a, b) = (self.param(0), self.param(1));
        if a > 0.0 && self.projection != "UTM" {
            return if b <= 0.0 {
                Ellipsoid::sphere(a)
            } else if b < 1.0 {
                // 1 より小さいときは離心率の 2 乗。
                Ellipsoid { a, e: b.sqrt() }
            } else {
                Ellipsoid::from_axes(a, b)
            };
        }
        match self.sphere {
            8 => Ellipsoid::from_flattening(6_378_137.0, 298.257_222_101),
            12 => Ellipsoid::WGS84,
            19 => Ellipsoid::sphere(6_370_997.0),
            // 0 は Clarke 1866（GCTP の既定）。ほかの古い楕円体も、十字の位置には差が小さいので代える。
            _ => Ellipsoid::from_axes(6_378_206.4, 6_356_583.8),
        }
    }

    /// 図法。緯度経度の格子（`GEO`）と、対応していない図法は `None`。
    pub fn projection(&self) -> Option<Projection> {
        let el = self.ellipsoid();
        let (fe, fn_) = (self.param(6), self.param(7));
        Some(match self.projection.as_str() {
            "SNSOID" => Projection::Sinusoidal {
                radius: el.a,
                central_lon: self.angle(4),
                false_easting: fe,
                false_northing: fn_,
            },
            "LAMAZ" => Projection::LambertAzimuthalEqualArea {
                radius: el.a,
                center: (self.angle(5), self.angle(4)),
                false_easting: fe,
                false_northing: fn_,
            },
            "CEA" => Projection::CylindricalEqualArea {
                ellipsoid: el,
                central_lon: self.angle(4),
                true_scale_lat: self.angle(5),
                false_easting: fe,
                false_northing: fn_,
            },
            "PS" => {
                let true_scale = self.angle(5);
                Projection::PolarStereographic {
                    ellipsoid: el,
                    north: true_scale >= 0.0,
                    central_lon: self.angle(4),
                    scale: PolarScale::StandardParallel(if true_scale == 0.0 {
                        FRAC_PI_2
                    } else {
                        true_scale.abs()
                    }),
                    false_easting: fe,
                    false_northing: fn_,
                }
            }
            "UTM" => {
                let zone = if self.zone != 0 {
                    self.zone
                } else {
                    // 帯の番号がなければ、ProjParams の経度・緯度から決める。
                    let lon = packed_dms(self.param(0));
                    let lat = packed_dms(self.param(1));
                    let z = (((lon + 180.0) / 6.0).floor() as i32 + 1).clamp(1, 60);
                    if lat < 0.0 { -z } else { z }
                };
                Projection::TransverseMercator {
                    ellipsoid: el,
                    origin_lat: 0.0,
                    central_lon: (6.0 * f64::from(zone.abs()) - 183.0).to_radians(),
                    k0: 0.9996,
                    false_easting: 500_000.0,
                    false_northing: if zone < 0 { 10_000_000.0 } else { 0.0 },
                }
            }
            "TM" => Projection::TransverseMercator {
                ellipsoid: el,
                origin_lat: self.angle(5),
                central_lon: self.angle(4),
                k0: if self.param(2) > 0.0 {
                    self.param(2)
                } else {
                    1.0
                },
                false_easting: fe,
                false_northing: fn_,
            },
            "LAMCC" => Projection::LambertConformalConic {
                ellipsoid: el,
                parallels: (self.angle(2), self.angle(3)),
                origin_lat: self.angle(5),
                central_lon: self.angle(4),
                false_easting: fe,
                false_northing: fn_,
            },
            "ALBERS" => Projection::AlbersEqualArea {
                ellipsoid: el,
                parallels: (self.angle(2), self.angle(3)),
                origin_lat: self.angle(5),
                central_lon: self.angle(4),
                false_easting: fe,
                false_northing: fn_,
            },
            "MERCAT" => Projection::Mercator {
                ellipsoid: el,
                central_lon: self.angle(4),
                k0: el.m(self.angle(5)),
                false_easting: fe,
                false_northing: fn_,
            },
            _ => return None,
        })
    }

    /// 列・行ごとの、画素の x と y（図法の m か、`GEO` なら度）。並びはファイルの順。
    pub fn pixel_coordinates(&self) -> (Vec<f64>, Vec<f64>) {
        let geo = self.projection == "GEO";
        let at = |v: f64| if geo { packed_dms(v) } else { v };
        let (x0, y0) = (at(self.upper_left.0), at(self.upper_left.1));
        let (x1, y1) = (at(self.lower_right.0), at(self.lower_right.1));
        let shift = if self.corner { 0.0 } else { 0.5 };
        // `(0, 0)` の画素の側の端から数える。
        let axis = |near: f64, far: f64, n: usize| -> Vec<f64> {
            let step = (far - near) / n as f64;
            (0..n).map(|i| near + (i as f64 + shift) * step).collect()
        };
        let (left, right) = if self.origin_right {
            (x1, x0)
        } else {
            (x0, x1)
        };
        let (top, bottom) = if self.origin_bottom {
            (y1, y0)
        } else {
            (y0, y1)
        };
        (
            axis(left, right, self.columns),
            axis(top, bottom, self.rows),
        )
    }
}

/// 粗い緯度か経度の表（`gw` × `gh`、行ごと）を、データの画素（`w` × `h`）へ両線形に延ばす。
/// 端の外は、端の 2 点の傾きのまま延ばす。経度は日付変更線をまたいでも 360° 跳ばないよう、
/// 隣の点を左上の点の ±180° の中に寄せてから混ぜる。欠損（NaN）の混じる点は欠損。
pub fn interpolate(
    coarse: &[f64],
    (gw, gh): (usize, usize),
    (w, h): (usize, usize),
    (rows, cols): (&DimensionMap, &DimensionMap),
    longitude: bool,
) -> Vec<f32> {
    let place = |g: f64, n: usize| -> (usize, f64) {
        if n < 2 {
            return (0, 0.0);
        }
        let i0 = (g.floor().max(0.0) as usize).min(n - 2);
        (i0, g - i0 as f64)
    };
    let cols: Vec<(usize, f64)> = (0..w)
        .map(|i| place(cols.geo_index(i as f64), gw))
        .collect();
    let mut out = Vec::with_capacity(w * h);
    for j in 0..h {
        let (r0, tr) = place(rows.geo_index(j as f64), gh);
        let r1 = (r0 + 1).min(gh - 1);
        for &(c0, tc) in &cols {
            let c1 = (c0 + 1).min(gw - 1);
            let a = coarse[r0 * gw + c0];
            let mut corners = [
                a,
                coarse[r0 * gw + c1],
                coarse[r1 * gw + c0],
                coarse[r1 * gw + c1],
            ];
            if longitude {
                for v in &mut corners[1..] {
                    *v -= 360.0 * ((*v - a) / 360.0).round();
                }
            }
            let weights = [
                (1.0 - tr) * (1.0 - tc),
                (1.0 - tr) * tc,
                tr * (1.0 - tc),
                tr * tc,
            ];
            // 重みが 0 の点は混ぜない（端の画素が、隣の欠損に引きずられないように）。
            let v: f64 = corners
                .iter()
                .zip(weights)
                .filter(|(_, w)| *w != 0.0)
                .map(|(v, w)| v * w)
                .sum();
            // 端の外へ延ばした緯度は、極を越えることがある。
            let v = if longitude {
                vw_core::geo::wrap_longitude(v)
            } else {
                v.clamp(-90.0, 90.0)
            };
            out.push(v as f32);
        }
    }
    out
}

#[cfg(test)]
mod tests;
