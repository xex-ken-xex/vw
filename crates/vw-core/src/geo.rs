//! 画素ごとの地球の上の位置（緯度・経度）。気象・海洋の格子のデータの十字や計測線、地図に使う。

/// 地球の半径（km）。距離は球として大円で測る。楕円体との差は 0.5% 以内で、
/// 計測線のグラフの横軸には十分なため、楕円体の測地線の計算は採らない。
pub const EARTH_RADIUS_KM: f64 = 6371.0;

/// 画像の画素と緯度・経度の対応。画素の位置は表示の向き（上下を返した後）で数える。
#[derive(Debug, Clone, PartialEq)]
pub struct GeoGrid {
    pub width: u32,
    pub height: u32,
    pub cells: Cells,
}

/// 緯度・経度の持ち方。
#[derive(Debug, Clone, PartialEq)]
pub enum Cells {
    /// 列ごとの経度と、行ごとの緯度（緯度・経度の格子）。
    Axes {
        longitudes: Vec<f64>,
        latitudes: Vec<f64>,
    },
    /// 画素ごとの緯度と経度（曲がった格子や、図法から直した格子）。並びは行ごと。
    Table {
        latitudes: Vec<f32>,
        longitudes: Vec<f32>,
    },
}

impl GeoGrid {
    /// 画素の中心 `(x, y)`（0 始まり、小数も可）の `(緯度, 経度)`。画像の外や値がなければ `None`。
    /// 経度は −180〜180° に直す。
    pub fn at(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (w, h) = (f64::from(self.width), f64::from(self.height));
        if !(x > -0.5 && y > -0.5 && x < w - 0.5 && y < h - 0.5) {
            return None;
        }
        let (lat, lon) = match &self.cells {
            Cells::Axes {
                longitudes,
                latitudes,
            } => (
                interpolate(latitudes, y)?,
                interpolate_longitude(longitudes, x)?,
            ),
            Cells::Table {
                latitudes,
                longitudes,
            } => self.bilinear(latitudes, longitudes, x, y)?,
        };
        (lat.is_finite() && lon.is_finite()).then(|| (lat, wrap_longitude(lon)))
    }

    fn bilinear(&self, lat: &[f32], lon: &[f32], x: f64, y: f64) -> Option<(f64, f64)> {
        let w = self.width as usize;
        let x = x.clamp(0.0, f64::from(self.width - 1));
        let y = y.clamp(0.0, f64::from(self.height - 1));
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let x1 = (x0 + 1).min(w - 1);
        let y1 = (y0 + 1).min(self.height as usize - 1);
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let corner = |cx: usize, cy: usize| {
            let i = cy * w + cx;
            Some((f64::from(*lat.get(i)?), f64::from(*lon.get(i)?)))
        };
        let (a, b, c, d) = (
            corner(x0, y0)?,
            corner(x1, y0)?,
            corner(x0, y1)?,
            corner(x1, y1)?,
        );
        let mix = |p: f64, q: f64, r: f64, s: f64| {
            (p * (1.0 - fx) + q * fx) * (1.0 - fy) + (r * (1.0 - fx) + s * fx) * fy
        };
        // 日付変更線をまたぐ 4 点は、経度を a に近い側へそろえてから混ぜる。
        let near = |v: f64| v + 360.0 * ((a.1 - v) / 360.0).round();
        Some((
            mix(a.0, b.0, c.0, d.0),
            mix(a.1, near(b.1), near(c.1), near(d.1)),
        ))
    }

    /// 画像の外周を、各辺 `per_side` 点ずつたどった `(緯度, 経度)`。地図に範囲を描くのに使う。
    pub fn outline(&self, per_side: usize) -> Vec<(f64, f64)> {
        let (w, h) = (f64::from(self.width - 1), f64::from(self.height - 1));
        let n = per_side.max(1);
        let step = |i: usize| i as f64 / n as f64;
        let edge = (0..n)
            .map(|i| (w * step(i), 0.0))
            .chain((0..n).map(|i| (w, h * step(i))))
            .chain((0..n).map(|i| (w * (1.0 - step(i)), h)))
            .chain((0..n).map(|i| (0.0, h * (1.0 - step(i)))));
        edge.filter_map(|(x, y)| self.at(x, y)).collect()
    }
}

/// 2 点 `(緯度, 経度)` の間の大円の距離（km）。
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (p1, p2) = (a.0.to_radians(), b.0.to_radians());
    let dp = p2 - p1;
    let dl = (b.1 - a.1).to_radians();
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.sqrt().min(1.0).asin()
}

/// 経度を −180〜180° にする。
pub fn wrap_longitude(lon: f64) -> f64 {
    let v = (lon + 180.0).rem_euclid(360.0) - 180.0;
    if v == -180.0 && lon > 0.0 { 180.0 } else { v }
}

/// `35.68°N 139.77°E` の形。
pub fn format_lat_lon(lat: f64, lon: f64) -> String {
    let ns = if lat < 0.0 { 'S' } else { 'N' };
    let ew = if lon < 0.0 { 'W' } else { 'E' };
    format!("{:.2}°{ns} {:.2}°{ew}", lat.abs(), lon.abs())
}

fn interpolate(values: &[f64], at: f64) -> Option<f64> {
    let last = values.len().checked_sub(1)?;
    let at = at.clamp(0.0, last as f64);
    let i = (at.floor() as usize).min(last);
    let j = (i + 1).min(last);
    let f = at - i as f64;
    Some(values[i] * (1.0 - f) + values[j] * f)
}

/// 経度の軸の補間。359.75° と 0° の間のように 360° をまたぐ所でも、近い側で混ぜる。
fn interpolate_longitude(values: &[f64], at: f64) -> Option<f64> {
    let last = values.len().checked_sub(1)?;
    let at = at.clamp(0.0, last as f64);
    let i = (at.floor() as usize).min(last);
    let j = (i + 1).min(last);
    let f = at - i as f64;
    let b = values[j] + 360.0 * ((values[i] - values[j]) / 360.0).round();
    Some(values[i] * (1.0 - f) + b * f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axes() -> GeoGrid {
        GeoGrid {
            width: 4,
            height: 3,
            cells: Cells::Axes {
                longitudes: vec![0.0, 90.0, 180.0, 270.0],
                latitudes: vec![60.0, 30.0, 0.0],
            },
        }
    }

    #[test]
    fn axes_give_the_coordinates_of_pixel_centers() {
        assert_eq!(axes().at(1.0, 1.0), Some((30.0, 90.0)));
    }

    #[test]
    fn longitudes_above_180_are_shown_west() {
        assert_eq!(axes().at(3.0, 0.0), Some((60.0, -90.0)));
    }

    #[test]
    fn positions_between_pixels_are_interpolated() {
        let (lat, lon) = axes().at(0.5, 0.5).unwrap();
        assert!((lat - 45.0).abs() < 1e-9 && (lon - 45.0).abs() < 1e-9);
    }

    #[test]
    fn outside_the_image_has_no_coordinates() {
        assert_eq!(axes().at(-1.0, 0.0), None);
        assert_eq!(axes().at(0.0, 3.0), None);
    }

    #[test]
    fn table_cells_are_read_per_pixel() {
        let g = GeoGrid {
            width: 2,
            height: 1,
            cells: Cells::Table {
                latitudes: vec![10.0, 11.0],
                longitudes: vec![179.0, -179.0],
            },
        };
        assert_eq!(g.at(1.0, 0.0), Some((11.0, -179.0)));
        // 日付変更線をまたぐ 2 点の真ん中は 180°（0° ではない）。
        let (_, lon) = g.at(0.5, 0.0).unwrap();
        assert!((lon.abs() - 180.0).abs() < 1e-6, "{lon}");
    }

    #[test]
    fn one_degree_of_latitude_is_about_111_km() {
        let d = distance_km((0.0, 0.0), (1.0, 0.0));
        assert!((d - 111.19).abs() < 0.01, "{d}");
    }

    #[test]
    fn tokyo_to_osaka_is_about_400_km() {
        let d = distance_km((35.68, 139.77), (34.69, 135.50));
        assert!((390.0..410.0).contains(&d), "{d}");
    }

    #[test]
    fn coordinates_are_written_with_hemispheres() {
        assert_eq!(format_lat_lon(35.681, 139.767), "35.68°N 139.77°E");
        assert_eq!(format_lat_lon(-33.87, -70.65), "33.87°S 70.65°W");
    }

    #[test]
    fn outline_goes_around_the_edge() {
        let o = axes().outline(2);
        assert_eq!(o.len(), 8);
        assert_eq!(o[0], (60.0, 0.0));
    }
}
