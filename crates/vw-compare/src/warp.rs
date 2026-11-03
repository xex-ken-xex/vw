//! 貼り合わせで、元の画像の画素から貼り合わせの座標への写し方。
//!
//! - アフィン: ずらす・回す・大きさを変える（分けて撮った平らなもの、天体のモザイク）
//! - 射影: 遠近のゆがみ（その場で回して撮った狭いパノラマ、斜めから撮った平らなもの）
//! - 円筒: その場で回して撮った広いパノラマ（射影では端が無限に伸びるため）

use crate::affine::Affine;
use crate::linalg::{Mat3, apply3, invert3, mul3, transpose3};

/// 3 × 3 の射影変換。`(x, y, 1)` に掛け、3 つめで割る。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography(pub Mat3);

impl Homography {
    pub const IDENTITY: Homography = Homography(crate::linalg::IDENTITY3);

    pub fn from_affine(t: &Affine) -> Self {
        Homography([[t.a, t.b, t.c], [t.d, t.e, t.f], [0.0, 0.0, 1.0]])
    }

    /// 写した点。無限遠より向こう（3 つめが 0 以下）なら `None`。
    pub fn apply(&self, [x, y]: [f64; 2]) -> Option<[f64; 2]> {
        let [u, v, w] = apply3(&self.0, [x, y, 1.0]);
        (w > 1e-12 && w.is_finite()).then(|| [u / w, v / w])
    }

    pub fn inverse(&self) -> Option<Homography> {
        invert3(&self.0).map(|m| Homography(m).normalized())
    }

    /// `self` のあとに `then` をかける。
    pub fn then(&self, then: &Homography) -> Homography {
        Homography(mul3(&then.0, &self.0)).normalized()
    }

    /// 右下を 1 にそろえる（同じ写し方の定数倍を同じ値で表すため）。右下が 0 に近いときは
    /// 大きさだけそろえる。
    pub fn normalized(self) -> Homography {
        let m = self.0;
        let k = if m[2][2].abs() > 1e-12 {
            1.0 / m[2][2]
        } else {
            let n: f64 = m.iter().flatten().map(|v| v * v).sum::<f64>().sqrt();
            if n > 0.0 { 1.0 / n } else { 1.0 }
        };
        Homography(m.map(|row| row.map(|v| v * k)))
    }

    /// 下の行が (0, 0, 1) ならアフィン。
    pub fn as_affine(&self) -> Option<Affine> {
        let m = self.normalized().0;
        (m[2][0].abs() < 1e-15 && m[2][1].abs() < 1e-15).then_some(Affine {
            a: m[0][0],
            b: m[0][1],
            c: m[0][2],
            d: m[1][0],
            e: m[1][1],
            f: m[1][2],
        })
    }
}

/// 円筒への写し方。元の画素 → カメラの向きの平面（`to_camera`）→ 世界の向き（`rotation` の転置）
/// → 円筒の角度と高さ（ラジアン、高さは半径 1 の円筒で）→ 貼り合わせの座標（`to_mosaic`）。
/// 世界の座標は x が右、y が下、z が前（画像と同じ向き）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cylinder {
    /// 元の画素から、焦点距離 1 の平面の座標（主点が原点）。
    pub to_camera: Affine,
    /// 世界の向きからカメラの向きへの回転。
    pub rotation: Mat3,
    /// 円筒の（角度, 高さ）から、貼り合わせの座標へ。
    pub to_mosaic: Affine,
}

impl Cylinder {
    pub fn apply(&self, p: [f64; 2]) -> Option<[f64; 2]> {
        let [x, y] = self.to_camera.apply(p);
        let d = apply3(&transpose3(&self.rotation), [x, y, 1.0]);
        let r = d[0].hypot(d[2]);
        (r > 1e-12).then(|| self.to_mosaic.apply([d[0].atan2(d[2]), d[1] / r]))
    }
}

/// 円筒から元の画素へ戻す写し方（逆の行列を先に作っておく）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CylinderBack {
    from_mosaic: Affine,
    rotation: Mat3,
    from_camera: Affine,
}

impl CylinderBack {
    pub fn apply(&self, q: [f64; 2]) -> Option<[f64; 2]> {
        let [theta, h] = self.from_mosaic.apply(q);
        // 1 周より外は同じ向きの繰り返しなので描かない（2 度描かないため）。
        if theta.abs() > std::f64::consts::PI {
            return None;
        }
        let (s, c) = theta.sin_cos();
        let [x, y, z] = apply3(&self.rotation, [s, h, c]);
        (z > 1e-9).then(|| self.from_camera.apply([x / z, y / z]))
    }
}

/// 元の画素から貼り合わせの座標への写し方。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Warp {
    Affine(Affine),
    Projective(Homography),
    Cylinder(Cylinder),
}

/// [`Warp`] の逆（貼り合わせの座標から元の画素へ）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Back {
    Affine(Affine),
    Projective(Homography),
    Cylinder(CylinderBack),
}

impl Back {
    pub fn apply(&self, q: [f64; 2]) -> Option<[f64; 2]> {
        match self {
            Back::Affine(t) => Some(t.apply(q)),
            Back::Projective(h) => h.apply(q),
            Back::Cylinder(c) => c.apply(q),
        }
    }
}

impl From<Affine> for Warp {
    fn from(t: Affine) -> Self {
        Warp::Affine(t)
    }
}

impl Warp {
    pub fn apply(&self, p: [f64; 2]) -> Option<[f64; 2]> {
        match self {
            Warp::Affine(t) => Some(t.apply(p)),
            Warp::Projective(h) => h.apply(p),
            Warp::Cylinder(c) => c.apply(p),
        }
    }

    pub fn back(&self) -> Option<Back> {
        Some(match self {
            Warp::Affine(t) => Back::Affine(t.inverse()?),
            Warp::Projective(h) => Back::Projective(h.inverse()?),
            Warp::Cylinder(c) => Back::Cylinder(CylinderBack {
                from_mosaic: c.to_mosaic.inverse()?,
                rotation: c.rotation,
                from_camera: c.to_camera.inverse()?,
            }),
        })
    }

    /// 先に `first`（元の画素の座標を変える）をかけてから `self`。
    pub fn after(&self, first: &Affine) -> Warp {
        match self {
            Warp::Affine(t) => Warp::Affine(first.then(t)),
            Warp::Projective(h) => Warp::Projective(Homography::from_affine(first).then(h)),
            Warp::Cylinder(c) => Warp::Cylinder(Cylinder {
                to_camera: first.then(&c.to_camera),
                ..*c
            }),
        }
    }

    /// `self` のあとに `then`（貼り合わせの座標を変える）。
    pub fn then(&self, then: &Affine) -> Warp {
        match self {
            Warp::Affine(t) => Warp::Affine(t.then(then)),
            Warp::Projective(h) => Warp::Projective(h.then(&Homography::from_affine(then))),
            Warp::Cylinder(c) => Warp::Cylinder(Cylinder {
                to_mosaic: c.to_mosaic.then(then),
                ..*c
            }),
        }
    }

    /// アフィンで表せるなら、その写し方。
    pub fn as_affine(&self) -> Option<Affine> {
        match self {
            Warp::Affine(t) => Some(*t),
            Warp::Projective(h) => h.as_affine(),
            Warp::Cylinder(_) => None,
        }
    }

    /// 大きさ `size` の画像の縁を写した点（左上から時計回り）。アフィンなら 4 隅だけ、
    /// ほかは縁が曲がって写ることがあるので辺ごとに `steps` 個。写せない点は除く。
    pub fn outline(&self, size: [usize; 2], steps: usize) -> Vec<[f64; 2]> {
        let c = corners(size);
        let n = if matches!(self, Warp::Affine(_)) {
            1
        } else {
            steps.max(1)
        };
        (0..4)
            .flat_map(|k| {
                let (p, q) = (c[k], c[(k + 1) % 4]);
                (0..n).map(move |i| {
                    let t = i as f64 / n as f64;
                    [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]
                })
            })
            .filter_map(|p| self.apply(p))
            .collect()
    }
}

/// 画素の外側の縁の 4 隅（画素の中心を整数にとるので、左上は (−0.5, −0.5)）。
pub fn corners([w, h]: [usize; 2]) -> [[f64; 2]; 4] {
    let (r, b) = (w as f64 - 0.5, h as f64 - 0.5);
    [[-0.5, -0.5], [r, -0.5], [r, b], [-0.5, b]]
}

/// `a` の画素から `b` の画素へ、置き方 `pa`・`pb` を通した写し方を、`a` の中で `b` にも写る所で
/// いちばん近いアフィンにしたもの（重なりの絵を比べるのに使う。重なりの中では射影でも円筒でも
/// アフィンに近い）。重なりが狭すぎれば `None`。
pub fn local_affine(pa: &Warp, sa: [usize; 2], pb: &Warp, sb: [usize; 2]) -> Option<Affine> {
    let back = pb.back()?;
    const STEPS: usize = 12;
    let mut pairs = Vec::new();
    for j in 0..=STEPS {
        for i in 0..=STEPS {
            let p = [
                i as f64 / STEPS as f64 * (sa[0] - 1) as f64,
                j as f64 / STEPS as f64 * (sa[1] - 1) as f64,
            ];
            let Some(q) = pa.apply(p).and_then(|m| back.apply(m)) else {
                continue;
            };
            if q[0] >= -0.5
                && q[1] >= -0.5
                && q[0] <= sb[0] as f64 - 0.5
                && q[1] <= sb[1] as f64 - 0.5
            {
                pairs.push([p, q]);
            }
        }
    }
    if pairs.len() < 3 {
        return None;
    }
    crate::estimate::affine(&pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::rotation_of;

    #[test]
    fn a_homography_and_its_inverse_return_the_point() {
        let h = Homography([[1.1, 0.05, 30.0], [-0.02, 0.95, -12.0], [1e-4, -2e-4, 1.0]]);
        let p = [120.0, 80.0];
        let q = h.apply(p).unwrap();
        let back = h.inverse().unwrap().apply(q).unwrap();
        assert!((back[0] - p[0]).abs() < 1e-9 && (back[1] - p[1]).abs() < 1e-9);
    }

    #[test]
    fn an_affine_homography_is_recognised() {
        let t = Affine::similarity(1.2, 0.3, [0.0, 0.0], [5.0, 6.0]);
        assert_eq!(Homography::from_affine(&t).as_affine(), Some(t));
        let h = Homography([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1e-4, 0.0, 1.0]]);
        assert!(h.as_affine().is_none());
    }

    fn cylinder(yaw: f64) -> Cylinder {
        let f = 500.0;
        Cylinder {
            to_camera: Affine {
                a: 1.0 / f,
                c: -320.0 / f,
                e: 1.0 / f,
                f: -240.0 / f,
                ..Affine::IDENTITY
            },
            rotation: rotation_of([0.0, yaw, 0.0]),
            to_mosaic: Affine {
                a: f,
                e: f,
                ..Affine::IDENTITY
            },
        }
    }

    #[test]
    fn a_point_on_the_cylinder_goes_back_to_its_pixel() {
        let w = Warp::Cylinder(cylinder(0.4));
        let p = [100.0, 400.0];
        let q = w.apply(p).unwrap();
        let back = w.back().unwrap().apply(q).unwrap();
        assert!((back[0] - p[0]).abs() < 1e-9 && (back[1] - p[1]).abs() < 1e-9);
    }

    #[test]
    fn turning_the_camera_moves_the_picture_along_the_cylinder() {
        // カメラを右へ回すと、同じ画素の写る角度は回した分だけ増える。
        let centre = [320.0, 240.0];
        let a = Warp::Cylinder(cylinder(0.0)).apply(centre).unwrap();
        let b = Warp::Cylinder(cylinder(-0.5)).apply(centre).unwrap();
        assert!((b[0] - a[0] - 0.5 * 500.0).abs() < 1e-6, "{a:?} {b:?}");
        assert!((b[1] - a[1]).abs() < 1e-9);
    }

    #[test]
    fn composing_before_and_after_matches_doing_both() {
        let w = Warp::Projective(Homography([
            [1.0, 0.1, 3.0],
            [0.0, 1.0, 4.0],
            [1e-4, 0.0, 1.0],
        ]));
        let first = Affine::similarity(2.0, 0.0, [0.0, 0.0], [1.0, 1.0]);
        let then = Affine::similarity(0.5, 0.2, [0.0, 0.0], [-3.0, 2.0]);
        let p = [10.0, 20.0];
        let want = then.apply(w.apply(first.apply(p)).unwrap());
        let got = w.after(&first).then(&then).apply(p).unwrap();
        assert!((got[0] - want[0]).abs() < 1e-9 && (got[1] - want[1]).abs() < 1e-9);
        let c = Warp::Cylinder(cylinder(0.2));
        let want = then.apply(c.apply(first.apply(p)).unwrap());
        let got = c.after(&first).then(&then).apply(p).unwrap();
        assert!((got[0] - want[0]).abs() < 1e-9 && (got[1] - want[1]).abs() < 1e-9);
    }
}
