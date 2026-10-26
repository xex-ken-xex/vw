//! テストで使う画像。

use crate::affine::Affine;
use crate::plane::{Plane, Rgb};

/// 決まった乱数で散らした、ぼけた点と線の模様（0〜255）。
pub fn texture(width: usize, height: usize) -> Plane {
    let mut seed = 0x2545_f491_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        f64::from(seed) / f64::from(u32::MAX)
    };
    let blobs: Vec<(f64, f64, f64, f64)> = (0..80)
        .map(|_| {
            (
                next() * 400.0 - 50.0,
                next() * 400.0 - 50.0,
                3.0 + next() * 18.0,
                next() * 2.0 - 1.0,
            )
        })
        .collect();
    let data = (0..width * height)
        .map(|i| {
            let (x, y) = ((i % width) as f64, (i / width) as f64);
            let mut v = 128.0 + 30.0 * ((x * 0.07).sin() * (y * 0.05 + x * 0.01).cos());
            for &(bx, by, r, amp) in &blobs {
                let d2 = (x - bx).powi(2) + (y - by).powi(2);
                v += amp * 90.0 * (-d2 / (2.0 * r * r)).exp();
            }
            v.clamp(0.0, 255.0) as f32
        })
        .collect();
    Plane::new(width, height, data)
}

/// `t` で写した先が `src` になる画像（`out(t(x)) = src(x)`）。写らない所は 128。
pub fn transformed(src: &Plane, width: usize, height: usize, t: &Affine) -> Plane {
    let inv = t.inverse().unwrap();
    let data = (0..width * height)
        .map(|i| {
            let p = inv.apply([(i % width) as f64, (i / width) as f64]);
            src.sample(p[0], p[1]).unwrap_or(128.0)
        })
        .collect();
    Plane::new(width, height, data)
}

/// 灰色の模様を 3 色に分けた画像（緑を少し明るく、青を少し暗く）。
pub fn color_texture(width: usize, height: usize) -> Rgb {
    let p = texture(width, height);
    let g = Plane::new(
        width,
        height,
        p.data.iter().map(|v| (v * 1.1).min(255.0)).collect(),
    );
    let b = Plane::new(width, height, p.data.iter().map(|v| v * 0.8).collect());
    Rgb::from_channels([&p, &g, &b])
}
