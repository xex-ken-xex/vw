//! 「位置合わせして比べる」を試す画像を作る。
//!
//! ```sh
//! cargo run --release -p vw-compare --example make_samples -- <出力フォルダ>
//! ```
//!
//! 基準の 1 枚と、ずらす・回す・縮める・明るさを変える・ぼかす・ノイズを足す・一部を描き替える・
//! 周辺を暗くする、を加えた画像、関係のない画像を PNG で書く。どれだけ変えたかはファイル名に入れる。

use std::path::{Path, PathBuf};

use vw_compare::{Affine, Rgb};

const W: usize = 1600;
const H: usize = 1200;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("compare-samples"));
    std::fs::create_dir_all(&dir).expect("create the output folder");
    let base = scene(W, H, 1);
    write(&dir, "01-reference.png", &base);

    let centre = [(W as f64 - 1.0) / 2.0, (H as f64 - 1.0) / 2.0];
    // 基準の画素 → 比べる画像の画素。比べる画像はこの逆で描く。
    let shifted = Affine::similarity(
        1.0,
        2f64.to_radians(),
        centre,
        [centre[0] + 24.0, centre[1] - 15.0],
    );
    write(
        &dir,
        "02-shift-24-15-rotate-2deg.png",
        &warped(&base, &shifted, W, H),
    );

    let (sw, sh) = (W * 3 / 4, H * 3 / 4);
    let small = Affine::similarity(
        0.75,
        (-5f64).to_radians(),
        centre,
        [(sw as f64 - 1.0) / 2.0, (sh as f64 - 1.0) / 2.0],
    );
    write(
        &dir,
        "03-smaller-75pct-rotate-minus-5deg.png",
        &warped(&base, &small, sw, sh),
    );

    write(
        &dir,
        "04-brighter-gamma-0.8.png",
        &map(&base, |v| 255.0 * (v / 255.0).powf(0.8)),
    );
    write(&dir, "05-blurred.png", &blurred(&base, 3));
    write(&dir, "06-noisy.png", &noisy(&base, 8.0));
    write(&dir, "07-changed-patch.png", &patched(&base));
    write(&dir, "08-vignetting.png", &vignetted(&base, 0.45));
    write(&dir, "09-unrelated.png", &scene(W, H, 7));
    println!("wrote samples to {}", dir.display());
}

/// 位置合わせの手がかりになる模様（縞、円、四角、細かい格子）。`seed` で配置が変わる。
fn scene(w: usize, h: usize, seed: u64) -> Rgb {
    let mut rng = Lcg(seed * 7919 + 1);
    let shapes: Vec<(f64, f64, f64, [f32; 3], bool)> = (0..40)
        .map(|_| {
            let c = [
                rng.next() as f32 * 255.0,
                rng.next() as f32 * 255.0,
                rng.next() as f32 * 255.0,
            ];
            (
                rng.next() * w as f64,
                rng.next() * h as f64,
                20.0 + rng.next() * 120.0,
                c,
                rng.next() < 0.5,
            )
        })
        .collect();
    let mut data = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f64, y as f64);
            let mut p = [
                (60.0 + 80.0 * fx / w as f64) as f32,
                (70.0 + 60.0 * fy / h as f64) as f32,
                (90.0 + 40.0 * ((fx + fy) * 0.01).sin()) as f32,
            ];
            for &(cx, cy, r, c, square) in &shapes {
                let inside = if square {
                    (fx - cx).abs() < r && (fy - cy).abs() < r * 0.6
                } else {
                    (fx - cx).hypot(fy - cy) < r
                };
                if inside {
                    p = c;
                }
            }
            if (x / 8 + y / 8) % 2 == 0 && x % 200 < 64 && y % 160 < 48 {
                p = p.map(|v| v * 0.6);
            }
            data.push(p);
        }
    }
    Rgb {
        width: w,
        height: h,
        data,
    }
}

/// `t`（`base` の画素 → 作る画像の画素）で写した画像。外は灰色。
fn warped(base: &Rgb, t: &Affine, w: usize, h: usize) -> Rgb {
    let back = t.inverse().expect("an invertible transform");
    let data = (0..w * h)
        .map(|i| {
            let [x, y] = back.apply([(i % w) as f64, (i / w) as f64]);
            base.sample(x, y).unwrap_or([128.0; 3])
        })
        .collect();
    Rgb {
        width: w,
        height: h,
        data,
    }
}

fn map(img: &Rgb, f: impl Fn(f32) -> f32) -> Rgb {
    Rgb {
        data: img.data.iter().map(|p| p.map(&f)).collect(),
        ..img.clone()
    }
}

/// 半径 `r` の箱ぼかし。
fn blurred(img: &Rgb, r: usize) -> Rgb {
    let (w, h) = (img.width, img.height);
    let mut data = img.data.clone();
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0.0f32; 3];
            let mut n = 0.0;
            for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                    let p = img.data[yy * w + xx];
                    for c in 0..3 {
                        sum[c] += p[c];
                    }
                    n += 1.0;
                }
            }
            data[y * w + x] = sum.map(|v| v / n);
        }
    }
    Rgb {
        data,
        ..img.clone()
    }
}

fn noisy(img: &Rgb, sigma: f64) -> Rgb {
    let mut rng = Lcg(42);
    Rgb {
        data: img
            .data
            .iter()
            .map(|p| p.map(|v| v + (rng.gauss() * sigma) as f32))
            .collect(),
        ..img.clone()
    }
}

/// 右上の一部を別の色の四角で描き替える。
fn patched(img: &Rgb) -> Rgb {
    let mut out = img.clone();
    for y in 200..360 {
        for x in 1100..1300 {
            out.data[y * out.width + x] = [240.0, 30.0, 200.0];
        }
    }
    out
}

/// 周辺を `strength` の割合まで暗くする（cos⁴ に近い落ち方）。
fn vignetted(img: &Rgb, strength: f64) -> Rgb {
    let (w, h) = (img.width as f64, img.height as f64);
    let half = (w / 2.0).hypot(h / 2.0);
    let data = img
        .data
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (x, y) = ((i % img.width) as f64, (i / img.width) as f64);
            let r = (x - w / 2.0).hypot(y - h / 2.0) / half;
            let k = (1.0 - strength * r * r) as f32;
            p.map(|v| v * k)
        })
        .collect();
    Rgb {
        data,
        ..img.clone()
    }
}

fn write(dir: &Path, name: &str, img: &Rgb) {
    let bytes = img.to_rgba8();
    let file = std::fs::File::create(dir.join(name)).expect("create a PNG");
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(file),
        img.width as u32,
        img.height as u32,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(&bytes))
        .expect("write a PNG");
}

/// 再現できる乱数（線形合同法）。
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.next().max(1e-12), self.next());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}
