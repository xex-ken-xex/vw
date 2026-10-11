//! RAW ファイルをプラグインで読み、表示に使う画像の大きさ・かかった時間・メタデータを出す。
//! 実機の RAW で動作を確かめるための道具。
//!
//! `cargo run --release -p vw-raw --example inspect -- [--develop] <file>...`

use std::time::Instant;
use vw_core::DecoderPlugin;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let develop = args.iter().any(|a| a == "--develop");
    for path in args.iter().filter(|a| *a != "--develop") {
        println!("== {path}");
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                println!("read error: {e}");
                continue;
            }
        };
        let t = Instant::now();
        let result = if develop {
            vw_raw::develop_bytes(&data)
        } else {
            vw_raw::RawPlugin.decode(&data)
        };
        match result {
            Ok(img) => {
                println!(
                    "{}x{} {:?} {}bit in {:.0} ms",
                    img.width,
                    img.height,
                    img.color,
                    img.data.bits_per_sample(),
                    t.elapsed().as_secs_f64() * 1000.0
                );
                for (g, k, v) in &img.metadata.entries {
                    println!("  [{g}] {k}: {v}");
                }
            }
            Err(e) => println!("error: {e}"),
        }
    }
}
