//! CF 規約の `grid_mapping` の図法で、x・y（m）から緯度・経度へ直す式。
//!
//! 式は Snyder "Map Projections: A Working Manual"（USGS Professional Paper 1395, 1987）の楕円体の版。
//! 図法のライブラリ（C の PROJ）を足さずに、気象・海洋のデータでよく使う図法だけを持つ。
//! PROJ は全部の図法と測地系の変換を持つが、C のライブラリとデータのファイルを配る必要があり、
//! 十字の位置を出すだけの用途には重すぎるため。

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

/// 回転楕円体。球なら `e = 0`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ellipsoid {
    /// 長半径（m）。
    pub a: f64,
    /// 離心率。
    pub e: f64,
}

impl Ellipsoid {
    pub const WGS84: Ellipsoid = Ellipsoid::from_flattening(6_378_137.0, 298.257_223_563);

    pub const fn sphere(radius: f64) -> Self {
        Self { a: radius, e: 0.0 }
    }

    /// 長半径と扁平率の逆数から。`inverse_flattening` が 0 なら球。
    pub const fn from_flattening(a: f64, inverse_flattening: f64) -> Self {
        if inverse_flattening == 0.0 {
            return Self::sphere(a);
        }
        let f = 1.0 / inverse_flattening;
        Self {
            a,
            e: const_sqrt(2.0 * f - f * f),
        }
    }

    /// 長半径と短半径から。
    pub fn from_axes(a: f64, b: f64) -> Self {
        Self {
            a,
            e: (1.0 - (b * b) / (a * a)).max(0.0).sqrt(),
        }
    }

    fn e2(&self) -> f64 {
        self.e * self.e
    }

    /// Snyder (14-15) の m。
    pub(crate) fn m(&self, phi: f64) -> f64 {
        phi.cos() / (1.0 - self.e2() * phi.sin().powi(2)).sqrt()
    }

    /// Snyder (15-9) の t。
    fn t(&self, phi: f64) -> f64 {
        let s = self.e * phi.sin();
        (FRAC_PI_4 - phi / 2.0).tan() / ((1.0 - s) / (1.0 + s)).powf(self.e / 2.0)
    }

    /// t から緯度（Snyder (7-9) を繰り返す）。
    fn phi_from_t(&self, t: f64) -> f64 {
        let mut phi = FRAC_PI_2 - 2.0 * t.atan();
        for _ in 0..15 {
            let s = self.e * phi.sin();
            let next = FRAC_PI_2 - 2.0 * (t * ((1.0 - s) / (1.0 + s)).powf(self.e / 2.0)).atan();
            if (next - phi).abs() < 1e-12 {
                return next;
            }
            phi = next;
        }
        phi
    }

    /// Snyder (3-12) の q。
    fn q(&self, phi: f64) -> f64 {
        let s = phi.sin();
        if self.e == 0.0 {
            return 2.0 * s;
        }
        let es = self.e * s;
        (1.0 - self.e2())
            * (s / (1.0 - es * es) - (1.0 / (2.0 * self.e)) * ((1.0 - es) / (1.0 + es)).ln())
    }

    /// 赤道から緯度 `phi` までの子午線の長さ（Snyder (3-21)）。
    fn meridian(&self, phi: f64) -> f64 {
        let (e2, e4, e6) = (self.e2(), self.e2().powi(2), self.e2().powi(3));
        self.a
            * ((1.0 - e2 / 4.0 - 3.0 * e4 / 64.0 - 5.0 * e6 / 256.0) * phi
                - (3.0 * e2 / 8.0 + 3.0 * e4 / 32.0 + 45.0 * e6 / 1024.0) * (2.0 * phi).sin()
                + (15.0 * e4 / 256.0 + 45.0 * e6 / 1024.0) * (4.0 * phi).sin()
                - (35.0 * e6 / 3072.0) * (6.0 * phi).sin())
    }
}

const fn const_sqrt(v: f64) -> f64 {
    // const fn で使える平方根（ニュートン法）。WGS84 の定数を作るためだけ。
    let mut x = v;
    let mut i = 0;
    while i < 60 {
        x = 0.5 * (x + v / x);
        i += 1;
    }
    x
}

/// 対応している図法。角度はラジアン、長さは m。
#[derive(Debug, Clone, PartialEq)]
pub enum Projection {
    LambertConformalConic {
        ellipsoid: Ellipsoid,
        parallels: (f64, f64),
        origin_lat: f64,
        central_lon: f64,
        false_easting: f64,
        false_northing: f64,
    },
    PolarStereographic {
        ellipsoid: Ellipsoid,
        /// 北極が中心なら true。
        north: bool,
        /// 中心の経度（`straight_vertical_longitude_from_pole`）。
        central_lon: f64,
        scale: PolarScale,
        false_easting: f64,
        false_northing: f64,
    },
    Mercator {
        ellipsoid: Ellipsoid,
        central_lon: f64,
        /// 赤道での縮尺（標準緯線から求めたものも含む）。
        k0: f64,
        false_easting: f64,
        false_northing: f64,
    },
    TransverseMercator {
        ellipsoid: Ellipsoid,
        origin_lat: f64,
        central_lon: f64,
        k0: f64,
        false_easting: f64,
        false_northing: f64,
    },
    AlbersEqualArea {
        ellipsoid: Ellipsoid,
        parallels: (f64, f64),
        origin_lat: f64,
        central_lon: f64,
        false_easting: f64,
        false_northing: f64,
    },
}

/// 極の平射図法の縮尺の決め方。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PolarScale {
    /// 縮尺が 1 になる緯線（ラジアン、絶対値）。
    StandardParallel(f64),
    /// 極での縮尺。
    ScaleAtPole(f64),
}

impl Projection {
    /// x・y（m）の `(緯度, 経度)`（度）。
    pub fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (phi, lambda) = match *self {
            Projection::LambertConformalConic {
                ellipsoid: el,
                parallels: (p1, p2),
                origin_lat,
                central_lon,
                false_easting,
                false_northing,
            } => {
                let (m1, m2) = (el.m(p1), el.m(p2));
                let (t1, t2) = (el.t(p1), el.t(p2));
                let n = if (p1 - p2).abs() < 1e-10 {
                    p1.sin()
                } else {
                    (m1.ln() - m2.ln()) / (t1.ln() - t2.ln())
                };
                let f = m1 / (n * t1.powf(n));
                let rho0 = el.a * f * el.t(origin_lat).powf(n);
                let (x, y) = (x - false_easting, y - false_northing);
                let sign = n.signum();
                let rho = sign * (x * x + (rho0 - y).powi(2)).sqrt();
                let theta = (sign * x).atan2(sign * (rho0 - y));
                let t = (rho / (el.a * f)).powf(1.0 / n);
                (el.phi_from_t(t), theta / n + central_lon)
            }
            Projection::PolarStereographic {
                ellipsoid: el,
                north,
                central_lon,
                scale,
                false_easting,
                false_northing,
            } => {
                let (x, y) = (x - false_easting, y - false_northing);
                let rho = x.hypot(y);
                let t = match scale {
                    PolarScale::StandardParallel(pc) if (pc - FRAC_PI_2).abs() > 1e-10 => {
                        rho * el.t(pc) / (el.a * el.m(pc))
                    }
                    PolarScale::StandardParallel(_) => rho * polar_factor(el.e) / (2.0 * el.a),
                    PolarScale::ScaleAtPole(k0) => rho * polar_factor(el.e) / (2.0 * el.a * k0),
                };
                let phi = el.phi_from_t(t);
                if north {
                    (phi, central_lon + x.atan2(-y))
                } else {
                    (-phi, central_lon + x.atan2(y))
                }
            }
            Projection::Mercator {
                ellipsoid: el,
                central_lon,
                k0,
                false_easting,
                false_northing,
            } => {
                let t = (-(y - false_northing) / (el.a * k0)).exp();
                (
                    el.phi_from_t(t),
                    (x - false_easting) / (el.a * k0) + central_lon,
                )
            }
            Projection::TransverseMercator {
                ellipsoid: el,
                origin_lat,
                central_lon,
                k0,
                false_easting,
                false_northing,
            } => transverse_mercator(
                el,
                origin_lat,
                central_lon,
                k0,
                x - false_easting,
                y - false_northing,
            ),
            Projection::AlbersEqualArea {
                ellipsoid: el,
                parallels: (p1, p2),
                origin_lat,
                central_lon,
                false_easting,
                false_northing,
            } => albers(
                el,
                (p1, p2),
                origin_lat,
                central_lon,
                x - false_easting,
                y - false_northing,
            )?,
        };
        let (lat, lon) = (phi.to_degrees(), lambda.to_degrees());
        (lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 + 1e-9)
            .then(|| (lat.clamp(-90.0, 90.0), vw_core::geo::wrap_longitude(lon)))
    }
}

/// Snyder (21-33) の √((1+e)^(1+e) (1−e)^(1−e))。
fn polar_factor(e: f64) -> f64 {
    ((1.0 + e).powf(1.0 + e) * (1.0 - e).powf(1.0 - e)).sqrt()
}

fn transverse_mercator(
    el: Ellipsoid,
    phi0: f64,
    lambda0: f64,
    k0: f64,
    x: f64,
    y: f64,
) -> (f64, f64) {
    let e2 = el.e2();
    let ep2 = e2 / (1.0 - e2);
    let m = el.meridian(phi0) + y / k0;
    let mu = m / (el.a * (1.0 - e2 / 4.0 - 3.0 * e2 * e2 / 64.0 - 5.0 * e2.powi(3) / 256.0));
    let r = (1.0 - e2).sqrt();
    let e1 = (1.0 - r) / (1.0 + r);
    let phi1 = mu
        + (3.0 * e1 / 2.0 - 27.0 * e1.powi(3) / 32.0) * (2.0 * mu).sin()
        + (21.0 * e1 * e1 / 16.0 - 55.0 * e1.powi(4) / 32.0) * (4.0 * mu).sin()
        + (151.0 * e1.powi(3) / 96.0) * (6.0 * mu).sin()
        + (1097.0 * e1.powi(4) / 512.0) * (8.0 * mu).sin();
    let (s, c) = phi1.sin_cos();
    let c1 = ep2 * c * c;
    let t1 = phi1.tan().powi(2);
    let w = 1.0 - e2 * s * s;
    let n1 = el.a / w.sqrt();
    let r1 = el.a * (1.0 - e2) / w.powf(1.5);
    let d = x / (n1 * k0);
    let phi = phi1
        - (n1 * phi1.tan() / r1)
            * (d * d / 2.0
                - (5.0 + 3.0 * t1 + 10.0 * c1 - 4.0 * c1 * c1 - 9.0 * ep2) * d.powi(4) / 24.0
                + (61.0 + 90.0 * t1 + 298.0 * c1 + 45.0 * t1 * t1 - 252.0 * ep2 - 3.0 * c1 * c1)
                    * d.powi(6)
                    / 720.0);
    let lambda = lambda0
        + (d - (1.0 + 2.0 * t1 + c1) * d.powi(3) / 6.0
            + (5.0 - 2.0 * c1 + 28.0 * t1 - 3.0 * c1 * c1 + 8.0 * ep2 + 24.0 * t1 * t1)
                * d.powi(5)
                / 120.0)
            / c;
    (phi, lambda)
}

fn albers(
    el: Ellipsoid,
    (p1, p2): (f64, f64),
    phi0: f64,
    lambda0: f64,
    x: f64,
    y: f64,
) -> Option<(f64, f64)> {
    let (m1, m2) = (el.m(p1), el.m(p2));
    let (q0, q1, q2) = (el.q(phi0), el.q(p1), el.q(p2));
    let n = if (p1 - p2).abs() < 1e-10 {
        p1.sin()
    } else {
        (m1 * m1 - m2 * m2) / (q2 - q1)
    };
    let c = m1 * m1 + n * q1;
    let rho0 = el.a * (c - n * q0).sqrt() / n;
    let sign = n.signum();
    let rho = sign * (x * x + (rho0 - y).powi(2)).sqrt();
    let theta = (sign * x).atan2(sign * (rho0 - y));
    let q = (c - (rho * n / el.a).powi(2)) / n;
    let phi = if el.e == 0.0 {
        (q / 2.0).clamp(-1.0, 1.0).asin()
    } else {
        let e2 = el.e2();
        let mut phi = (q / 2.0).clamp(-1.0, 1.0).asin();
        for _ in 0..25 {
            let s = phi.sin();
            let es = el.e * s;
            let w = 1.0 - es * es;
            let next = phi
                + w * w / (2.0 * phi.cos())
                    * (q / (1.0 - e2) - s / w
                        + (1.0 / (2.0 * el.e)) * ((1.0 - es) / (1.0 + es)).ln());
            if !next.is_finite() {
                return None;
            }
            if (next - phi).abs() < 1e-12 {
                phi = next;
                break;
            }
            phi = next;
        }
        phi
    };
    Some((phi, lambda0 + theta / n))
}

/// 回転極の格子の `(格子の緯度, 格子の経度)`（度）から、地理の `(緯度, 経度)`（度）。
/// `pole` は格子の北極の地理の `(緯度, 経度)`、`pole_lon_in_grid` は `north_pole_grid_longitude`。
pub fn unrotate(rlat: f64, rlon: f64, pole: (f64, f64), pole_lon_in_grid: f64) -> (f64, f64) {
    let (pr, pl) = (pole.0.to_radians(), pole.1.to_radians());
    let phi = rlat.to_radians();
    let lam = (rlon - pole_lon_in_grid).to_radians();
    let (sp, cp) = pr.sin_cos();
    let lat = (sp * phi.sin() + cp * phi.cos() * lam.cos())
        .clamp(-1.0, 1.0)
        .asin();
    let lon = pl
        + (phi.cos() * lam.sin()).atan2(sp * phi.cos() * lam.cos() - cp * phi.sin())
        + std::f64::consts::PI;
    (
        lat.to_degrees(),
        vw_core::geo::wrap_longitude(lon.to_degrees()),
    )
}

#[cfg(test)]
mod tests;
