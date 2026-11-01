//! 地図の座標の計算。Web メルカトル（OpenStreetMap や多くの地図のタイルと同じ図法）で、
//! 地球全体を 1 辺 1 の正方形（左上 (0, 0)、右下 (1, 1)）として扱う。
//!
//! 東西はどこまでも続けて見られるよう、中心の x は 0〜1 に収めず、描くときに地図を横に
//! 繰り返す。南北は地図の端で止める。

use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};

/// タイル 1 枚の画素数。
pub const TILE: f64 = 256.0;
/// タイルの最大の拡大段階。OpenStreetMap の標準のタイルは 19 まで。
pub const MAX_ZOOM: f64 = 19.0;
/// メルカトル図法で描ける緯度の限り。これより極に近い所は無限に伸びるので、地図にない。
const MAX_LATITUDE: f64 = 85.051_128_78;

/// 緯度・経度（度）を地図の座標にする。
pub fn project(latitude: f64, longitude: f64) -> [f64; 2] {
    let lat = latitude.clamp(-MAX_LATITUDE, MAX_LATITUDE).to_radians();
    let x = (longitude + 180.0) / 360.0;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0;
    [x, y]
}

/// 地図の座標を緯度・経度（度）にする。x は何周していてもよい。
pub fn unproject(world: [f64; 2]) -> (f64, f64) {
    let lon = world[0].rem_euclid(1.0) * 360.0 - 180.0;
    let n = std::f64::consts::PI * (1.0 - 2.0 * world[1]);
    (n.sinh().atan().to_degrees(), lon)
}

/// 地図のどこを、どの倍率で見ているか。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// 画面の中心にある地図の座標。
    pub center: [f64; 2],
    /// 拡大の段階（0 で世界がタイル 1 枚、1 増えるごとに 2 倍）。小数も使う。
    pub zoom: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            center: [0.5, 0.5],
            zoom: 2.0,
        }
    }
}

impl View {
    /// 地図の座標 1 あたりの画素数。
    pub fn scale(&self) -> f64 {
        TILE * self.zoom.exp2()
    }

    pub fn to_screen(self, rect: Rect, world: [f64; 2]) -> Pos2 {
        let s = self.scale();
        let c = rect.center();
        pos2(
            c.x + ((world[0] - self.center[0]) * s) as f32,
            c.y + ((world[1] - self.center[1]) * s) as f32,
        )
    }

    pub fn to_world(self, rect: Rect, screen: Pos2) -> [f64; 2] {
        let s = self.scale();
        let d = screen - rect.center();
        [
            self.center[0] + f64::from(d.x) / s,
            self.center[1] + f64::from(d.y) / s,
        ]
    }

    /// 画面を `delta` 画素だけドラッグした分、地図を動かす。
    pub fn pan(&mut self, rect: Rect, delta: Vec2) {
        let s = self.scale();
        self.center[0] -= f64::from(delta.x) / s;
        self.center[1] -= f64::from(delta.y) / s;
        self.settle(rect);
    }

    /// `at`（画面の点）の下にある地図の場所を動かさずに、拡大の段階を `dz` 変える。
    pub fn zoom_at(&mut self, rect: Rect, at: Pos2, dz: f64) {
        let world = self.to_world(rect, at);
        self.zoom = (self.zoom + dz).clamp(min_zoom(rect), MAX_ZOOM);
        let s = self.scale();
        let d = at - rect.center();
        self.center = [world[0] - f64::from(d.x) / s, world[1] - f64::from(d.y) / s];
        self.settle(rect);
    }

    /// `points` がすべて入るように合わせる。1 か所だけなら、町の様子が分かる段階にする。
    pub fn fit(&mut self, rect: Rect, points: &[[f64; 2]]) {
        let Some(bounds) = Bounds::of(points) else {
            return;
        };
        let margin = 48.0;
        let w = f64::from((rect.width() - 2.0 * margin).max(1.0));
        let h = f64::from((rect.height() - 2.0 * margin).max(1.0));
        let zoom_for = |extent: f64, px: f64| {
            if extent <= 0.0 {
                f64::INFINITY
            } else {
                (px / (extent * TILE)).log2()
            }
        };
        let zoom = zoom_for(bounds.width, w).min(zoom_for(bounds.height, h));
        self.zoom = zoom.min(15.0).clamp(min_zoom(rect), MAX_ZOOM);
        self.center = bounds.center;
        self.settle(rect);
    }

    /// 南北は地図の外を見せない。東西は何周しても同じ所なので、中心を 0〜1 に戻す
    /// （何周もドラッグしても値が大きくなり続けないため）。
    fn settle(&mut self, rect: Rect) {
        self.zoom = self.zoom.clamp(min_zoom(rect), MAX_ZOOM);
        let half = f64::from(rect.height()) / 2.0 / self.scale();
        self.center[1] = if half >= 0.5 {
            0.5
        } else {
            self.center[1].clamp(half, 1.0 - half)
        };
        self.center[0] = self.center[0].rem_euclid(1.0);
    }

    /// 画面に入るタイルと、それを描く場所。タイルの段階は今の倍率に一番近い整数にし、
    /// 間の倍率ではタイルを拡大・縮小して描く。
    pub fn tiles(&self, rect: Rect) -> Vec<(TileId, Rect)> {
        let level = self.zoom.round().clamp(0.0, MAX_ZOOM) as u8;
        let n = 1u32 << level;
        let tile_px = self.scale() / f64::from(n);
        let top_left = self.to_world(rect, rect.min);
        let bottom_right = self.to_world(rect, rect.max);
        let tx0 = (top_left[0] * f64::from(n)).floor() as i64;
        let tx1 = (bottom_right[0] * f64::from(n)).ceil() as i64;
        let ty0 = (top_left[1] * f64::from(n)).floor().max(0.0) as i64;
        let ty1 = (bottom_right[1] * f64::from(n)).ceil().min(f64::from(n)) as i64;
        let mut out = Vec::new();
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let world = [tx as f64 / f64::from(n), ty as f64 / f64::from(n)];
                let min = self.to_screen(rect, world);
                let size = vec2(tile_px as f32, tile_px as f32);
                let id = TileId {
                    z: level,
                    x: tx.rem_euclid(i64::from(n)) as u32,
                    y: ty as u32,
                };
                out.push((id, Rect::from_min_size(min, size)));
            }
        }
        out
    }

    /// 地図を横に繰り返したとき、`world` が画面（`rect` を `margin` 広げた範囲）に入る位置すべて。
    pub fn copies(&self, rect: Rect, world: [f64; 2], margin: f32) -> Vec<Pos2> {
        let area = rect.expand(margin);
        let base = self.to_screen(rect, [world[0].rem_euclid(1.0), world[1]]);
        let width = self.scale() as f32;
        let first = ((area.min.x - base.x) / width).floor() as i32;
        let last = ((area.max.x - base.x) / width).ceil() as i32;
        (first..=last)
            .map(|k| pos2(base.x + k as f32 * width, base.y))
            .filter(|p| area.contains(*p))
            .collect()
    }
}

/// 世界の縦の全体が画面の高さより低くならない段階。南北の外に何もない帯が出ないようにする。
fn min_zoom(rect: Rect) -> f64 {
    (f64::from(rect.height().max(1.0)) / TILE).log2().max(0.0)
}

/// タイルの番号（拡大の段階・列・行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    /// 1 段下の（広い範囲を描いた）タイルと、その中でこのタイルにあたる範囲（0〜1 の割合）。
    pub fn parent(self) -> Option<(TileId, Rect)> {
        let z = self.z.checked_sub(1)?;
        let parent = TileId {
            z,
            x: self.x / 2,
            y: self.y / 2,
        };
        let min = pos2((self.x % 2) as f32 * 0.5, (self.y % 2) as f32 * 0.5);
        Some((parent, Rect::from_min_size(min, vec2(0.5, 0.5))))
    }
}

/// 点の範囲。東西は、日付変更線をまたいだ方が狭くなるなら、またいだ範囲にする。
struct Bounds {
    center: [f64; 2],
    width: f64,
    height: f64,
}

impl Bounds {
    fn of(points: &[[f64; 2]]) -> Option<Self> {
        let first = points.first()?;
        let (mut y0, mut y1) = (first[1], first[1]);
        let mut xs: Vec<f64> = Vec::with_capacity(points.len());
        for p in points {
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
            xs.push(p[0].rem_euclid(1.0));
        }
        xs.sort_by(f64::total_cmp);
        // 点の間の一番広い隙間を、範囲の外にする。
        let mut gap = (xs[0] + 1.0 - xs[xs.len() - 1], xs.len() - 1);
        for i in 1..xs.len() {
            let g = xs[i] - xs[i - 1];
            if g > gap.0 {
                gap = (g, i - 1);
            }
        }
        let start = xs[(gap.1 + 1) % xs.len()];
        let width = 1.0 - gap.0;
        Some(Self {
            center: [start + width / 2.0, (y0 + y1) / 2.0],
            width,
            height: y1 - y0,
        })
    }
}

/// 画面の上で近い点を 1 つにまとめたもの。
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    /// 含む点の平均の位置（地図の座標）。
    pub world: [f64; 2],
    /// 含む点の番号。
    pub members: Vec<usize>,
}

/// 今の倍率で、`cell` 画素四方の升に入る点をまとめる。
pub fn cluster(points: &[[f64; 2]], zoom: f64, cell: f64) -> Vec<Cluster> {
    let s = TILE * zoom.exp2() / cell;
    let mut cells: std::collections::BTreeMap<(i64, i64), Vec<usize>> = Default::default();
    for (i, p) in points.iter().enumerate() {
        let key = (
            (p[0].rem_euclid(1.0) * s).floor() as i64,
            (p[1] * s).floor() as i64,
        );
        cells.entry(key).or_default().push(i);
    }
    cells
        .into_values()
        .map(|members| {
            let n = members.len() as f64;
            let (x, y) = members.iter().fold((0.0, 0.0), |(x, y), &i| {
                (x + points[i][0].rem_euclid(1.0), y + points[i][1])
            });
            Cluster {
                world: [x / n, y / n],
                members,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn projection_round_trips_tokyo() {
        let w = project(35.681236, 139.767125);
        let (lat, lon) = unproject(w);
        assert!(close(lat, 35.681236) && close(lon, 139.767125));
    }

    #[test]
    fn greenwich_on_the_equator_is_the_middle_of_the_map() {
        let w = project(0.0, 0.0);
        assert!(close(w[0], 0.5) && close(w[1], 0.5));
    }

    #[test]
    fn east_west_never_ends() {
        let mut v = View::default();
        v.pan(screen(), vec2(-10_000.0, 0.0));
        assert!((0.0..1.0).contains(&v.center[0]));
        let (_, lon) = unproject(v.center);
        assert!((-180.0..180.0).contains(&lon));
    }

    #[test]
    fn north_south_stops_at_the_edge_of_the_map() {
        let mut v = View {
            center: [0.5, 0.5],
            zoom: 4.0,
        };
        v.pan(screen(), vec2(0.0, 100_000.0));
        let top = v.to_world(screen(), screen().min);
        assert!(close(top[1], 0.0));
    }

    #[test]
    fn zooming_keeps_the_place_under_the_cursor() {
        let mut v = View {
            center: [0.3, 0.4],
            zoom: 5.0,
        };
        let at = pos2(100.0, 450.0);
        let before = v.to_world(screen(), at);
        v.zoom_at(screen(), at, 1.5);
        let after = v.to_world(screen(), at);
        assert!(close(before[0], after[0]) && close(before[1], after[1]));
    }

    #[test]
    fn zoom_out_stops_when_the_world_fills_the_height() {
        let mut v = View::default();
        v.zoom_at(screen(), screen().center(), -10.0);
        assert!(v.scale() >= 600.0 - 1e-6);
    }

    #[test]
    fn tiles_cover_the_screen_and_wrap_around_the_date_line() {
        let v = View {
            center: [0.0, 0.5],
            zoom: 3.0,
        };
        let tiles = v.tiles(screen());
        let covered = tiles.iter().fold(Rect::NOTHING, |r, (_, t)| r.union(*t));
        assert!(covered.contains_rect(screen()));
        assert!(tiles.iter().all(|(id, _)| id.x < 8 && id.z == 3));
        assert!(tiles.iter().any(|(id, _)| id.x == 7) && tiles.iter().any(|(id, _)| id.x == 0));
    }

    #[test]
    fn a_point_shows_once_per_world_copy_on_screen() {
        let v = View {
            center: [0.5, 0.5],
            zoom: 1.2,
        };
        // 世界の幅は約 588 画素なので、幅 800 の画面には同じ点が 1〜2 回出る。
        let n = v.copies(screen(), [0.1, 0.5], 0.0).len();
        assert!((1..=2).contains(&n));
        let far = View {
            center: [0.5, 0.5],
            zoom: 10.0,
        };
        assert!(far.copies(screen(), [0.1, 0.5], 0.0).is_empty());
    }

    #[test]
    fn fit_spans_the_points_across_the_date_line_the_short_way() {
        let mut v = View::default();
        let points = [project(35.0, 179.0), project(36.0, -179.0)];
        v.fit(screen(), &points);
        let (_, lon) = unproject(v.center);
        assert!(lon.abs() > 179.0, "center {lon}");
    }

    #[test]
    fn fit_to_one_place_shows_the_town() {
        let mut v = View::default();
        v.fit(screen(), &[project(43.06, 141.35)]);
        assert!(close(v.zoom, 15.0));
    }

    #[test]
    fn nearby_points_are_grouped_and_split_when_zoomed_in() {
        let points = [
            project(35.68, 139.76),
            project(35.69, 139.77),
            project(34.69, 135.50),
        ];
        let far = cluster(&points, 4.0, 48.0);
        assert_eq!(far.len(), 2);
        let near = cluster(&points, 16.0, 48.0);
        assert_eq!(near.len(), 3);
    }

    #[test]
    fn parent_tile_covers_a_quarter() {
        let (p, uv) = TileId { z: 3, x: 5, y: 2 }.parent().unwrap();
        assert_eq!(p, TileId { z: 2, x: 2, y: 1 });
        assert_eq!(uv.min, pos2(0.5, 0.0));
    }
}
