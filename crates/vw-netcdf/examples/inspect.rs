//! `cargo run --release -p vw-netcdf --example inspect -- <file.nc> [variable]`
//! 画像にできる変数と、選んだ変数の面ごとの欠損の数・最小・最大・平均・読む時間を出す。

use std::time::Instant;

use vw_core::{DecoderPlugin, PixelData};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: inspect <file.nc> [variable]");
    let data = std::fs::read(&path).expect("read");
    let variables = vw_netcdf::variables(&data).expect("open");
    for v in &variables {
        println!("variable {} {:?} planes={}", v.path, v.dims, v.planes());
    }
    let choices = vw_netcdf::Choices::default();
    if let Some(name) = args.next() {
        choices.set(&data, &name);
    }
    let plugin = vw_netcdf::NetcdfPlugin::new(Default::default(), choices);
    let frames = plugin.frame_count(&data).expect("frames");
    let (mut nan, mut sum, mut n) = (0usize, 0f64, 0usize);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let start = Instant::now();
    for f in 0..frames {
        let img = plugin.decode_frame(&data, f).expect("decode");
        if f == 0 {
            for (g, k, v) in &img.metadata.entries {
                println!("  [{g}] {k}: {v}");
            }
        }
        let PixelData::F32(values) = &img.data else {
            unreachable!()
        };
        for &v in values {
            if v.is_nan() {
                nan += 1;
            } else {
                let v = f64::from(v);
                sum += v;
                n += 1;
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    println!(
        "frames={frames} nan={nan} min={lo:.4} max={hi:.4} mean={:.4} in {:?}",
        sum / n as f64,
        start.elapsed()
    );
}
