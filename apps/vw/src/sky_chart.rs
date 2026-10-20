//! 星図の座標の計算と、背景の星・星座のデータ。
//!
//! 投影は、見ている所を中心にした平射図法（ステレオ投影）1 つにしている。引いたときは
//! 半球より広く見渡せ、寄ったときはほぼ接平面（写真の TAN 投影）と同じ形になるので、
//! 全天用と拡大用の 2 つの投影を切り替えなくても済むため。

use eframe::egui::{pos2, Pos2, Rect, Vec2};

/// 赤経・赤緯（度）。
pub type Radec = [f64; 2];

/// どこを、どの倍率で見ているか。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// 画面の中心の赤経・赤緯。
    pub center: Radec,
    /// 投影面の長さ 1（中心から 90 度の所が 2）あたりの画素数。
    pub scale: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],
            scale: 200.0,
        }
    }
}

/// 中心から見てこれより遠い点（裏側）は描かない。平射図法では裏側の極が無限遠に飛ぶため。
const FARTHEST_COS: f64 = -0.6;
/// 寄れる限り（画面の短い辺に入る角度、度）。
const NARROWEST_DEGREES: f64 = 0.05;
/// 引ける限り。画面の短い辺に、中心から片側この角度が入る。
const WIDEST_HALF_DEGREES: f64 = 110.0;

impl View {
    /// 赤経・赤緯を画面の点にする。中心の裏側は `None`。赤経は左が東（星図の決まり）。
    pub fn to_screen(self, rect: Rect, sky: Radec) -> Option<Pos2> {
        let [x, y] = project(self.center, sky)?;
        let c = rect.center();
        Some(pos2(
            c.x - (x * self.scale) as f32,
            c.y - (y * self.scale) as f32,
        ))
    }

    pub fn to_sky(self, rect: Rect, screen: Pos2) -> Radec {
        let d = screen - rect.center();
        unproject(
            self.center,
            [-f64::from(d.x) / self.scale, -f64::from(d.y) / self.scale],
        )
    }

    /// 画面の短い辺に入る角度（度）。
    pub fn field_degrees(&self, rect: Rect) -> f64 {
        let half = f64::from(rect.width().min(rect.height())) / 2.0 / self.scale;
        4.0 * (half / 2.0).atan().to_degrees()
    }

    /// 画面を `delta` 画素だけドラッグした分、空を動かす。
    pub fn pan(&mut self, rect: Rect, delta: Vec2) {
        self.center = self.to_sky(rect, rect.center() - delta);
    }

    /// `at`（画面の点）の下の空を動かさずに、倍率を `factor` 倍にする。
    pub fn zoom_at(&mut self, rect: Rect, at: Pos2, factor: f64) {
        let target = self.to_sky(rect, at);
        self.scale = (self.scale * factor).clamp(min_scale(rect), max_scale(rect));
        // 倍率を変えると点がずれるので、ずれた分だけ戻す。平射図法では 1 回で戻りきらないので
        // 数回繰り返す。
        for _ in 0..3 {
            let Some(now) = self.to_screen(rect, target) else {
                break;
            };
            self.pan(rect, at - now);
        }
    }

    /// `points` がすべて入るように合わせる。1 か所だけなら、写真 1 枚の範囲が見える倍率にする。
    pub fn fit(&mut self, rect: Rect, points: &[Radec]) {
        let Some(center) = mean_direction(points) else {
            return;
        };
        self.center = center;
        let farthest = points
            .iter()
            .map(|p| angle(center, *p))
            .fold(0.0, f64::max)
            .max(2.0);
        let margin = 40.0;
        let half = f64::from(rect.width().min(rect.height()) / 2.0 - margin).max(10.0);
        let rho = 2.0 * (farthest.to_radians() / 2.0).tan();
        self.scale = (half / rho).clamp(min_scale(rect), max_scale(rect));
    }

    /// 画面の大きさが変わっても、倍率を限りの中に収める。
    pub fn settle(&mut self, rect: Rect) {
        self.scale = self.scale.clamp(min_scale(rect), max_scale(rect));
    }
}

fn min_scale(rect: Rect) -> f64 {
    let half = f64::from(rect.width().min(rect.height())) / 2.0;
    half / (2.0 * (WIDEST_HALF_DEGREES.to_radians() / 2.0).tan())
}

fn max_scale(rect: Rect) -> f64 {
    let half = f64::from(rect.width().min(rect.height())) / 2.0;
    half / (2.0 * (NARROWEST_DEGREES.to_radians() / 4.0).tan())
}

fn unit(sky: Radec) -> [f64; 3] {
    let (ra, dec) = (sky[0].to_radians(), sky[1].to_radians());
    [dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()]
}

/// 2 点の間の角度（度）。
pub fn angle(a: Radec, b: Radec) -> f64 {
    let (u, v) = (unit(a), unit(b));
    let dot = u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    dot.clamp(-1.0, 1.0).acos().to_degrees()
}

/// 点の向きの平均。全部が打ち消し合う（反対側にある）ときは最初の点。
fn mean_direction(points: &[Radec]) -> Option<Radec> {
    let first = *points.first()?;
    let sum = points
        .iter()
        .map(|p| unit(*p))
        .fold([0.0; 3], |s, u| [s[0] + u[0], s[1] + u[1], s[2] + u[2]]);
    let len = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
    if len < 1e-9 {
        return Some(first);
    }
    Some([
        sum[1].atan2(sum[0]).to_degrees().rem_euclid(360.0),
        (sum[2] / len).asin().to_degrees(),
    ])
}

/// 平射図法。`center` を原点に、東を +x、北を +y にした投影面の座標。
pub fn project(center: Radec, sky: Radec) -> Option<[f64; 2]> {
    let (a0, d0) = (center[0].to_radians(), center[1].to_radians());
    let (a, d) = (sky[0].to_radians(), sky[1].to_radians());
    let da = a - a0;
    let cos_c = d0.sin() * d.sin() + d0.cos() * d.cos() * da.cos();
    if cos_c < FARTHEST_COS {
        return None;
    }
    let k = 2.0 / (1.0 + cos_c);
    Some([
        k * d.cos() * da.sin(),
        k * (d0.cos() * d.sin() - d0.sin() * d.cos() * da.cos()),
    ])
}

pub fn unproject(center: Radec, plane: [f64; 2]) -> Radec {
    let [x, y] = plane;
    let rho = x.hypot(y);
    if rho < 1e-12 {
        return center;
    }
    let (a0, d0) = (center[0].to_radians(), center[1].to_radians());
    let c = 2.0 * (rho / 2.0).atan();
    let dec = (c.cos() * d0.sin() + y * c.sin() * d0.cos() / rho).asin();
    let ra = a0 + (x * c.sin()).atan2(rho * d0.cos() * c.cos() - y * d0.sin() * c.sin());
    [ra.to_degrees().rem_euclid(360.0), dec.to_degrees()]
}

/// 星 1 つ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Star {
    pub at: Radec,
    pub magnitude: f32,
    /// 色指数 B-V。分からなければ白。
    pub bv: Option<f32>,
}

/// 星座の名前と、名前を置く所。
#[derive(Debug, Clone, PartialEq)]
pub struct Constellation {
    pub at: Radec,
    pub latin: String,
    pub japanese: String,
}

/// 星図の背景。明るい星（6 等級まで）、星座の線と境界、星座の名前。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Background {
    /// 明るい順。
    pub stars: Vec<Star>,
    pub lines: Vec<Vec<Radec>>,
    pub borders: Vec<Vec<Radec>>,
    pub names: Vec<Constellation>,
}

impl Background {
    /// vw に同梱した背景（`tools/skydata/make_skydata.py` で d3-celestial のデータから作ったもの）。
    pub fn bundled() -> Self {
        Self::parse(include_str!("../assets/sky/sky.tsv"))
    }

    /// 読めない行は飛ばす（同梱のデータなので、壊れていたらテストで分かる）。
    pub fn parse(text: &str) -> Self {
        let mut b = Self::default();
        let polyline = |s: &str| -> Option<Vec<Radec>> {
            s.split(' ')
                .map(|p| {
                    let (ra, dec) = p.split_once(',')?;
                    Some([ra.parse().ok()?, dec.parse().ok()?])
                })
                .collect()
        };
        for line in text.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            match cols.as_slice() {
                ["S", ra, dec, mag, bv] => {
                    if let (Ok(ra), Ok(dec), Ok(magnitude)) = (ra.parse(), dec.parse(), mag.parse())
                    {
                        b.stars.push(Star {
                            at: [ra, dec],
                            magnitude,
                            bv: bv.parse().ok(),
                        });
                    }
                }
                ["L", _, points] => b.lines.extend(polyline(points)),
                ["B", points] => b.borders.extend(polyline(points)),
                ["N", _, ra, dec, latin, japanese] => {
                    if let (Ok(ra), Ok(dec)) = (ra.parse(), dec.parse()) {
                        b.names.push(Constellation {
                            at: [ra, dec],
                            latin: (*latin).to_owned(),
                            japanese: (*japanese).to_owned(),
                        });
                    }
                }
                _ => {}
            }
        }
        b
    }
}

/// 色指数 B-V から、星の色（青白〜橙）。
pub fn star_color(bv: Option<f32>) -> [u8; 3] {
    let Some(bv) = bv else {
        return [255, 255, 255];
    };
    let t = ((bv + 0.3) / 2.0).clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| (a + (b - a) * t).round() as u8;
    [lerp(170.0, 255.0), lerp(200.0, 190.0), lerp(255.0, 120.0)]
}

/// 等級から、星の点の半径（画素）。明るいほど大きい。
pub fn star_radius(magnitude: f32) -> f32 {
    (3.2 - 0.45 * magnitude).max(0.6)
}

/// 赤経・赤緯の目盛りの間隔（度）。画面に 4〜10 本ほど出るように選ぶ。
pub fn grid_step(field_degrees: f64) -> f64 {
    const STEPS: [f64; 12] = [
        30.0,
        15.0,
        10.0,
        5.0,
        2.0,
        1.0,
        0.5,
        0.25,
        1.0 / 6.0,
        0.1,
        0.05,
        1.0 / 60.0,
    ];
    STEPS
        .into_iter()
        .rev()
        .find(|s| field_degrees / s <= 8.0)
        .unwrap_or(30.0)
}

/// 赤経・赤緯の文字（`5h35m17s  -5°23′28″`）。
pub fn radec_text(sky: Radec) -> String {
    let hours = sky[0].rem_euclid(360.0) / 15.0;
    let (h, m, s) = sexagesimal(hours);
    let sign = if sky[1] < 0.0 { '-' } else { '+' };
    let (d, am, asec) = sexagesimal(sky[1].abs());
    format!("{h}h{m:02}m{s:02}s  {sign}{d}°{am:02}′{asec:02}″")
}

fn sexagesimal(v: f64) -> (u32, u32, u32) {
    let total = (v * 3600.0).round() as u64;
    (
        (total / 3600) as u32,
        (total / 60 % 60) as u32,
        (total % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::vec2;

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))
    }

    fn close(a: Radec, b: Radec) -> bool {
        angle(a, b) < 1e-6
    }

    #[test]
    fn centre_of_the_view_is_the_centre_of_the_screen() {
        let view = View {
            center: [83.8, -5.4],
            scale: 1000.0,
        };
        let at = view.to_screen(rect(), [83.8, -5.4]).unwrap();
        assert!((at - rect().center()).length() < 1e-3);
    }

    #[test]
    fn east_is_left_and_north_is_up() {
        let view = View {
            center: [100.0, 20.0],
            scale: 1000.0,
        };
        let c = rect().center();
        let east = view.to_screen(rect(), [101.0, 20.0]).unwrap();
        let north = view.to_screen(rect(), [100.0, 21.0]).unwrap();
        assert!(east.x < c.x && (east.y - c.y).abs() < 1.0);
        assert!(north.y < c.y && (north.x - c.x).abs() < 1e-3);
    }

    #[test]
    fn screen_and_sky_round_trip() {
        let view = View {
            center: [300.0, 70.0],
            scale: 350.0,
        };
        for p in [[0.0, 89.0], [280.0, 40.0], [330.0, 60.0]] {
            let at = view.to_screen(rect(), p).unwrap();
            // 画面の座標は f32 なので、画素の 1/1000 ほどの誤差は残る。
            assert!(angle(view.to_sky(rect(), at), p) < 1e-3, "{p:?}");
        }
    }

    #[test]
    fn far_side_of_the_sky_is_not_drawn() {
        let view = View::default();
        assert_eq!(view.to_screen(rect(), [180.0, 0.0]), None);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut view = View {
            center: [10.0, 30.0],
            scale: 2000.0,
        };
        let at = pos2(650.0, 120.0);
        let before = view.to_sky(rect(), at);
        view.zoom_at(rect(), at, 4.0);
        assert!(angle(view.to_sky(rect(), at), before) < 1e-4);
        assert_eq!(view.scale, 8000.0);
    }

    #[test]
    fn fit_shows_every_point() {
        let mut view = View::default();
        let points = [[80.0, -5.0], [90.0, 5.0], [85.0, 10.0]];
        view.fit(rect(), &points);
        for p in points {
            assert!(rect().contains(view.to_screen(rect(), p).unwrap()), "{p:?}");
        }
        assert!(view.field_degrees(rect()) < 40.0);
    }

    #[test]
    fn fit_across_ra_zero_centres_between_the_points() {
        let mut view = View::default();
        view.fit(rect(), &[[359.0, 0.0], [1.0, 0.0]]);
        assert!(close(view.center, [0.0, 0.0]));
    }

    #[test]
    fn bundled_background_has_stars_lines_and_all_88_constellations() {
        let b = Background::bundled();
        assert!(b.stars.len() > 5000);
        assert!(b.stars.windows(2).all(|w| w[0].magnitude <= w[1].magnitude));
        // いちばん明るいのはシリウス。
        let sirius = b.stars[0];
        assert!(angle(sirius.at, [101.2872, -16.7161]) < 0.01);
        assert!(b.lines.len() > 100 && b.borders.len() > 200);
        assert!(b.names.len() >= 88);
        let orion = b.names.iter().find(|n| n.latin == "Orion").unwrap();
        assert_eq!(orion.japanese, "オリオン座");
    }

    #[test]
    fn grid_lines_fit_the_field() {
        assert_eq!(grid_step(180.0), 30.0);
        assert_eq!(grid_step(40.0), 5.0);
        assert_eq!(grid_step(3.0), 0.5);
    }

    #[test]
    fn radec_is_written_in_hours_and_degrees() {
        assert_eq!(radec_text([83.822_083, -5.391_111]), "5h35m17s  -5°23′28″");
        assert_eq!(radec_text([0.0, 41.5]), "0h00m00s  +41°30′00″");
    }
}
