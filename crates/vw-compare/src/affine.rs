//! 基準の画像の座標から、比べる画像の座標への写し方（アフィン変換）。
//! 座標は画素の中心を整数にとる（左上の画素の中心が (0, 0)）。

/// `比べる = [a b; d e] · 基準 + [c; f]`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

/// 写し方を、人が読める値に分けたもの。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parts {
    /// 基準の中心が写る所の、比べる画像の中心からのずれ（比べる画像の画素）。
    pub shift: [f64; 2],
    /// 回転（度、時計回りが正。画面の y は下向き）。
    pub rotation: f64,
    /// 倍率（比べる画像の画素 / 基準の画素）。
    pub scale: f64,
    /// 縦横の倍率の比（1 で縦横同じ）。
    pub aspect: f64,
    /// せん断（度）。
    pub shear: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 0.0,
        e: 1.0,
        f: 0.0,
    };

    /// 回転 `angle`（ラジアン、時計回り）と倍率 `scale` を、点 `about`（基準の座標）の周りにかけ、
    /// そこを比べる画像の `to` に写す。
    pub fn similarity(scale: f64, angle: f64, about: [f64; 2], to: [f64; 2]) -> Affine {
        let (s, c) = angle.sin_cos();
        let (a, b, d, e) = (scale * c, -scale * s, scale * s, scale * c);
        Affine {
            a,
            b,
            c: to[0] - a * about[0] - b * about[1],
            d,
            e,
            f: to[1] - d * about[0] - e * about[1],
        }
    }

    pub fn apply(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        [
            self.a * x + self.b * y + self.c,
            self.d * x + self.e * y + self.f,
        ]
    }

    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.e - self.b * self.d;
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let (a, b, d, e) = (self.e / det, -self.b / det, -self.d / det, self.a / det);
        Some(Affine {
            a,
            b,
            c: -(a * self.c + b * self.f),
            d,
            e,
            f: -(d * self.c + e * self.f),
        })
    }

    /// `self` のあとに `then` をかける（`then(self(x))`）。
    pub fn then(&self, then: &Affine) -> Affine {
        Affine {
            a: then.a * self.a + then.b * self.d,
            b: then.a * self.b + then.b * self.e,
            c: then.a * self.c + then.b * self.f + then.c,
            d: then.d * self.a + then.e * self.d,
            e: then.d * self.b + then.e * self.e,
            f: then.d * self.c + then.e * self.f + then.f,
        }
    }

    /// 基準を倍率 `from`、比べる画像を倍率 `to` で縮めた座標どうしでの写し方。
    /// 画素の中心を整数にとるので、倍率 `k` の座標は `x' = k (x + 0.5) − 0.5`。
    pub fn between_scales(&self, from: f64, to: f64) -> Affine {
        let unscale = Affine {
            a: 1.0 / from,
            b: 0.0,
            c: (0.5 / from) - 0.5,
            d: 0.0,
            e: 1.0 / from,
            f: (0.5 / from) - 0.5,
        };
        let rescale = Affine {
            a: to,
            b: 0.0,
            c: 0.5 * to - 0.5,
            d: 0.0,
            e: to,
            f: 0.5 * to - 0.5,
        };
        unscale.then(self).then(&rescale)
    }

    /// 基準の大きさ `reference` と比べる画像の大きさ `moving` の、中心どうしを基点に分けた値。
    pub fn parts(&self, reference: [f64; 2], moving: [f64; 2]) -> Parts {
        let centre = |s: [f64; 2]| [(s[0] - 1.0) / 2.0, (s[1] - 1.0) / 2.0];
        let to = self.apply(centre(reference));
        let centre = centre(moving);
        // [a b; d e] = R(θ) · [sx m; 0 sy]（QR 分解）。
        let sx = self.a.hypot(self.d);
        let rotation = self.d.atan2(self.a);
        let m = (self.a * self.b + self.d * self.e) / sx;
        let sy = (self.a * self.e - self.b * self.d) / sx;
        Parts {
            shift: [to[0] - centre[0], to[1] - centre[1]],
            rotation: rotation.to_degrees(),
            scale: (sx * sy).abs().sqrt(),
            aspect: sy / sx,
            shear: (m / sy).atan().to_degrees(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn a_similarity_maps_its_centre_and_inverts() {
        let t = Affine::similarity(2.0, 0.3, [10.0, 20.0], [5.0, 7.0]);
        assert!(close(t.apply([10.0, 20.0]), [5.0, 7.0]));
        let inv = t.inverse().unwrap();
        assert!(close(inv.apply(t.apply([3.0, -4.0])), [3.0, -4.0]));
        assert!(close(t.then(&inv).apply([8.0, 1.0]), [8.0, 1.0]));
    }

    #[test]
    fn parts_recover_the_rotation_scale_and_shift() {
        let t = Affine::similarity(1.5, 10f64.to_radians(), [50.0, 25.0], [87.0, 33.0]);
        let p = t.parts([101.0, 51.0], [151.0, 75.0]);
        assert!((p.rotation - 10.0).abs() < 1e-9);
        assert!((p.scale - 1.5).abs() < 1e-9);
        assert!((p.aspect - 1.0).abs() < 1e-9);
        assert!(p.shear.abs() < 1e-9);
        assert!(close(p.shift, [12.0, -4.0]));
    }

    #[test]
    fn changing_scales_keeps_pixel_centres_consistent() {
        // 2 倍の大きさの画像で、半画素ずらした写し方。
        let full = Affine {
            c: 0.5,
            ..Affine::IDENTITY
        };
        let half = full.between_scales(0.5, 0.5);
        // 縮めた画素 0 の中心は元の 0.5。それが 1.0 に写り、縮めた座標では 0.25。
        assert!(close(half.apply([0.0, 0.0]), [0.25, 0.0]));
    }
}
