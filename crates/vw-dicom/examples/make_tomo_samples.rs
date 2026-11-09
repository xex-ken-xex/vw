//! 大きなマルチフレーム（トモシンセシス）の読み方と、シリーズの中の並べ方を確かめる DICOM を作る。
//! `cargo run --release -p vw-dicom --example make_tomo_samples -- <出力フォルダ> [大きなファイルの MB]`
//!
//! - `series/`: 1 つのシリーズに、1 枚だけの 2D が 2 つ（Instance Number 1・2）と、
//!   20 フレームのトモシンセシスが 2 つ（右 CC = 3、左 CC = 4）。各フレームには番号の数だけ
//!   白い四角が並ぶ
//! - `big/tomo-big.dcm`: 大きさを MB で指定したときだけ作る、50 フレームの 1 ファイル。
//!   作るときにファイルの大きさの 2 倍ほどのメモリを使う

use std::path::{Path, PathBuf};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use vw_dicom::synth::Synth;

const BT_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.13.1.3";
const MG_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.1.2";
const STUDY: &str = "1.2.826.0.1.3680043.9.7575.1";
const SERIES: &str = "1.2.826.0.1.3680043.9.7575.1.1";

/// 斜めの濃淡に、フレームの番号 + 1 個の白い四角を左上から並べた 12bit の画素。
fn frame(width: usize, height: usize, number: usize) -> Vec<u16> {
    let side = (width / 16).max(4);
    let mut v: Vec<u16> = (0..width * height)
        .map(|i| (((i % width) + (i / width)) * 2048 / (width + height)) as u16 + 512)
        .collect();
    for k in 0..=number {
        let (x0, y0) = ((k % 8) * side * 2 + side / 2, (k / 8) * side * 2 + side / 2);
        for y in y0..(y0 + side).min(height) {
            for x in x0..(x0 + side).min(width) {
                v[y * width + x] = 4095;
            }
        }
    }
    v
}

fn tomo(width: u16, height: u16, frames: usize, laterality: &str, instance: i32) -> Synth {
    let (w, h) = (width as usize, height as usize);
    let values: Vec<u16> = (0..frames).flat_map(|n| frame(w, h, n)).collect();
    Synth::new(BT_IMAGE, width, height)
        .pixels_u16(&values, 12)
        .text(tags::NUMBER_OF_FRAMES, VR::IS, &frames.to_string())
        .text(tags::MODALITY, VR::CS, "MG")
        .text(tags::IMAGE_LATERALITY, VR::CS, laterality)
        .study(STUDY, "Tomosynthesis sample", "20261109")
        .series(SERIES, 1, "Tomo + 2D")
        .instance(&format!("{SERIES}.{instance}"), instance)
        .window(2048.0, 4096.0)
}

fn two_d(width: u16, height: u16, laterality: &str, instance: i32) -> Synth {
    Synth::new(MG_IMAGE, width, height)
        .pixels_u16(&frame(width as usize, height as usize, 0), 12)
        .text(tags::MODALITY, VR::CS, "MG")
        .text(tags::IMAGE_LATERALITY, VR::CS, laterality)
        .study(STUDY, "Tomosynthesis sample", "20261109")
        .series(SERIES, 1, "Tomo + 2D")
        .instance(&format!("{SERIES}.{instance}"), instance)
        .window(2048.0, 4096.0)
}

fn write(synth: &Synth, path: &Path) {
    synth.write(path).expect("write the sample");
    println!("{}", path.display());
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().expect("output folder"));
    let big_mb: usize = args.next().map_or(0, |a| a.parse().expect("size in MB"));

    let series = out.join("series");
    std::fs::create_dir_all(&series).expect("create the folder");
    write(&two_d(512, 640, "R", 1), &series.join("2d-r.dcm"));
    write(&two_d(512, 640, "L", 2), &series.join("2d-l.dcm"));
    write(&tomo(512, 640, 20, "R", 3), &series.join("tomo-r.dcm"));
    write(&tomo(512, 640, 20, "L", 4), &series.join("tomo-l.dcm"));

    if big_mb > 0 {
        let big = out.join("big");
        std::fs::create_dir_all(&big).expect("create the folder");
        const FRAMES: usize = 50;
        let width = 2048usize;
        let height = (big_mb << 20) / FRAMES / 2 / width;
        let synth = tomo(width as u16, height.clamp(16, 65535) as u16, FRAMES, "R", 9);
        write(&synth, &big.join("tomo-big.dcm"));
    }
}
