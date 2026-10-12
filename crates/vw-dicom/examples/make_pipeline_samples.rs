//! 画素の変換の図を確かめる DICOM を作る。
//! `cargo run -p vw-dicom --example make_pipeline_samples -- <出力フォルダ>`
//!
//! - `ct-window.dcm`: CT（HU、ウィンドウ 40 / 400）
//! - `ct-no-window.dcm`: ウィンドウのない CT
//! - `ct-sigmoid.dcm`: VOI LUT Function が SIGMOID で、ウィンドウが 2 組ある CT
//! - `cr-monochrome1.dcm`: MONOCHROME1（値が大きいほど黒い）の 12bit 画像
//! - `enhanced-ct.dcm`: フレームごとに Rescale とウィンドウが違う 4 フレームの CT
//! - `lut-sequence.dcm`: Modality LUT Sequence と VOI LUT Sequence を持つ CT
//! - `pet.dcm`: Units が BQML の PET
//! - `rgb.dcm` / `ybr-full.dcm`: カラー

use std::path::PathBuf;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use vw_dicom::synth::Synth;

const SIDE: u16 = 256;

/// 左から右へ値が増える帯と、中央の円。`lo`..`hi` は格納値。
fn gradient(lo: i32, hi: i32, disc: i32) -> Vec<i32> {
    let n = SIDE as usize;
    let c = SIDE as f32 / 2.0;
    (0..n * n)
        .map(|i| {
            let (x, y) = ((i % n) as f32, (i / n) as f32);
            if ((x - c).powi(2) + (y - c).powi(2)).sqrt() < 40.0 {
                disc
            } else {
                lo + ((hi - lo) as f32 * x / (n - 1) as f32) as i32
            }
        })
        .collect()
}

fn i16s(v: &[i32]) -> Vec<i16> {
    v.iter().map(|&x| x as i16).collect()
}

/// ツリーでファイルごとに別のシリーズに見えるよう、UID と名前を分ける。
fn named(s: Synth, number: i32, name: &str) -> Synth {
    let uid = format!("1.2.826.0.1.3680043.9.77{number:02}");
    s.patient("Sample^Pipeline")
        .series(&format!("{uid}.1"), number, name)
        .instance(&format!("{uid}.2"), 1)
}

fn ct(number: i32, name: &str) -> Synth {
    // 格納値 0..2048 は -1024..1024 HU。円は 40 HU。
    let s = Synth::ct(SIDE, SIDE)
        .pixels_i16(&i16s(&gradient(0, 2048, 1064)))
        .signed()
        .rescale(1.0, -1024.0);
    named(s, number, name)
}

fn main() -> std::io::Result<()> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "pipeline-samples".into()),
    );
    std::fs::create_dir_all(&out)?;

    ct(1, "window")
        .window(40.0, 400.0)
        .write(&out.join("ct-window.dcm"))?;
    ct(2, "no window").write(&out.join("ct-no-window.dcm"))?;
    ct(3, "sigmoid")
        .text(tags::WINDOW_CENTER, VR::DS, "40\\-600")
        .text(tags::WINDOW_WIDTH, VR::DS, "400\\1500")
        .text(
            tags::WINDOW_CENTER_WIDTH_EXPLANATION,
            VR::LO,
            "SOFT TISSUE\\LUNG",
        )
        .text(tags::VOILUT_FUNCTION, VR::CS, "SIGMOID")
        .write(&out.join("ct-sigmoid.dcm"))?;
    ct(4, "lut sequence")
        .window(40.0, 400.0)
        .empty_sequence(tags::MODALITY_LUT_SEQUENCE)
        .empty_sequence(tags::VOILUT_SEQUENCE)
        .write(&out.join("lut-sequence.dcm"))?;

    let cr: Vec<u16> = gradient(0, 4095, 1000).iter().map(|&v| v as u16).collect();
    let cr = Synth::new(vw_dicom::synth::SC_IMAGE, SIDE, SIDE).pixels_u16(&cr, 12);
    named(cr, 5, "monochrome1")
        .monochrome1()
        .window(2048.0, 4096.0)
        .text(tags::MODALITY, VR::CS, "CR")
        .write(&out.join("cr-monochrome1.dcm"))?;

    // フレームごとに切片とウィンドウを変える（同じ格納値でも明るさが変わる）。
    let frame = i16s(&gradient(0, 2048, 1064));
    let frames: Vec<&[i16]> = vec![&frame; 4];
    named(Synth::ct(SIDE, SIDE), 6, "enhanced")
        .frames_i16(&frames)
        .signed()
        .per_frame_values(&[
            (1.0, -1024.0, 40.0, 400.0),
            (1.0, -1024.0, -600.0, 1500.0),
            (1.0, -824.0, 40.0, 400.0),
            (2.0, -2048.0, 40.0, 400.0),
        ])
        .write(&out.join("enhanced-ct.dcm"))?;

    let pet: Vec<u16> = gradient(0, 30000, 20000)
        .iter()
        .map(|&v| v as u16)
        .collect();
    let pet = Synth::new(vw_dicom::synth::SC_IMAGE, SIDE, SIDE).pixels_u16(&pet, 16);
    named(pet, 7, "pet")
        .rescale(0.5, 0.0)
        .window(5000.0, 10000.0)
        .text(tags::MODALITY, VR::CS, "PT")
        .text(tags::UNITS, VR::CS, "BQML")
        .write(&out.join("pet.dcm"))?;

    let n = SIDE as usize;
    let rgb: Vec<u8> = (0..n * n)
        .flat_map(|i| {
            let (x, y) = (i % n, i / n);
            [x as u8, y as u8, 128]
        })
        .collect();
    named(Synth::rgb(SIDE, SIDE, &rgb, false), 8, "rgb").write(&out.join("rgb.dcm"))?;
    let ybr: Vec<u8> = (0..n * n)
        .flat_map(|i| {
            let (x, y) = (i % n, i / n);
            [128, x as u8, y as u8]
        })
        .collect();
    named(Synth::rgb(SIDE, SIDE, &ybr, false), 9, "ybr")
        .photometric("YBR_FULL")
        .write(&out.join("ybr-full.dcm"))?;
    println!("wrote samples to {}", out.display());
    Ok(())
}
