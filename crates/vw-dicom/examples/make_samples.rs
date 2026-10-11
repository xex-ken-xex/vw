//! 画面確認用の DICOM を作る。`cargo run -p vw-dicom --example make_samples -- <出力フォルダ>`
//!
//! - `ct/`: 2 つの検査に分かれた CT の 3 シリーズ（円柱のファントム、HU、ウィンドウ付き）
//! - `cine.dcm`: 30 フレームのシネ（動く円、15 fps）
//! - `IM0001`: 拡張子のない CT 1 枚
//! - `jpeg-ls.dcm` / `jpeg-ls-color.dcm`: JPEG-LS（ロスレス）で圧縮した CT とカラー画像

use std::path::PathBuf;

use vw_dicom::synth::{Synth, CT_IMAGE, JPEG_LS_LOSSLESS, XA_IMAGE};

const SIDE: u16 = 256;

/// 中心からの距離で値を決める円柱のファントム。`z` で中の円の大きさを変える。
fn phantom(z: usize, slices: usize) -> Vec<i16> {
    let c = SIDE as f32 / 2.0;
    let inner = 20.0 + 40.0 * (z as f32 / slices as f32 * std::f32::consts::PI).sin();
    (0..SIDE as usize * SIDE as usize)
        .map(|i| {
            let (x, y) = (
                (i % SIDE as usize) as f32 - c,
                (i / SIDE as usize) as f32 - c,
            );
            let r = (x * x + y * y).sqrt();
            let hu: i32 = if r < inner {
                300
            } else if r < 100.0 {
                40 + ((x * 0.3).sin() * 20.0) as i32
            } else if r < 110.0 {
                -100
            } else {
                -1000
            };
            (hu + 1024) as i16
        })
        .collect()
}

fn jpeg_ls(bits: i32, components: i32, src: &[u8]) -> Vec<u8> {
    let frame = charls::FrameInfo {
        width: SIDE.into(),
        height: SIDE.into(),
        bits_per_sample: bits,
        component_count: components,
    };
    charls::CharLS::default()
        .encode(frame, 0, src)
        .expect("CharLS encodes a valid frame")
}

fn main() -> std::io::Result<()> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "dicom-samples".into()),
    );
    let ct = out.join("ct");
    std::fs::create_dir_all(&ct)?;
    let series = [
        (
            "1.2.826.0.1.3680043.9.100",
            "Head",
            "20261001",
            "1.2.826.0.1.3680043.9.101",
            1,
            "Axial 5mm",
            12,
        ),
        (
            "1.2.826.0.1.3680043.9.100",
            "Head",
            "20261001",
            "1.2.826.0.1.3680043.9.102",
            2,
            "Axial 1mm",
            24,
        ),
        (
            "1.2.826.0.1.3680043.9.200",
            "Chest",
            "20261005",
            "1.2.826.0.1.3680043.9.201",
            1,
            "Lung",
            16,
        ),
    ];
    let mut file_no = 0;
    for (study, study_desc, date, uid, number, desc, slices) in series {
        for z in 0..slices {
            file_no += 1;
            // 撮影順と逆の名前にして、Instance Number で並べ直していることを確かめられるようにする。
            let name = format!("IMG{:04}.dcm", 1000 - file_no);
            Synth::new(CT_IMAGE, SIDE, SIDE)
                .pixels_i16(&phantom(z, slices))
                .signed()
                .rescale(1.0, -1024.0)
                .window(40.0, 400.0)
                .patient("Sample^Phantom")
                .study(study, study_desc, date)
                .series(uid, number, desc)
                .instance(&format!("{uid}.{}", z + 1), z as i32 + 1)
                .write(&ct.join(name))?;
        }
    }
    let frames: Vec<Vec<u8>> = (0..30)
        .map(|f| {
            let a = f as f32 / 30.0 * std::f32::consts::TAU;
            let (cx, cy) = (128.0 + 60.0 * a.cos(), 128.0 + 60.0 * a.sin());
            (0..SIDE as usize * SIDE as usize)
                .map(|i| {
                    let (x, y) = ((i % 256) as f32 - cx, (i / 256) as f32 - cy);
                    if x * x + y * y < 400.0 {
                        230
                    } else {
                        40
                    }
                })
                .collect()
        })
        .collect();
    let refs: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
    Synth::new(XA_IMAGE, SIDE, SIDE)
        .frames_u8(&refs)
        .frame_time(66.7)
        .patient("Sample^Cine")
        .study("1.2.826.0.1.3680043.10.1.3", "Angio", "20261003")
        .series("1.2.826.0.1.3680043.10.1.3.1", 1, "Run 1")
        .write(&out.join("cine.dcm"))?;
    Synth::new(CT_IMAGE, SIDE, SIDE)
        .pixels_i16(&phantom(5, 10))
        .signed()
        .rescale(1.0, -1024.0)
        .window(40.0, 400.0)
        .study("1.2.826.0.1.3680043.10.1.4", "Scout", "20261004")
        .series("1.2.826.0.1.3680043.10.1.4.1", 1, "Localizer")
        .write(&out.join("IM0001"))?;
    let stored: Vec<u16> = phantom(3, 10).iter().map(|&v| v as u16).collect();
    let bytes: Vec<u8> = stored.iter().flat_map(|v| v.to_le_bytes()).collect();
    Synth::new(CT_IMAGE, SIDE, SIDE)
        .pixels_u16(&stored, 12)
        .rescale(1.0, -1024.0)
        .window(40.0, 400.0)
        .study("1.2.826.0.1.3680043.10.1.5", "Compressed", "20261011")
        .series("1.2.826.0.1.3680043.10.1.5.1", 1, "JPEG-LS")
        .encapsulated(JPEG_LS_LOSSLESS, vec![jpeg_ls(12, 1, &bytes)])
        .write(&out.join("jpeg-ls.dcm"))?;
    let rgb: Vec<u8> = (0..SIDE as usize * SIDE as usize)
        .flat_map(|i| [(i % 256) as u8, (i / 256) as u8, 128])
        .collect();
    Synth::rgb(SIDE, SIDE, &rgb, false)
        .study("1.2.826.0.1.3680043.10.1.5", "Compressed", "20261011")
        .series("1.2.826.0.1.3680043.10.1.5.2", 2, "JPEG-LS color")
        .encapsulated(JPEG_LS_LOSSLESS, vec![jpeg_ls(8, 3, &rgb)])
        .write(&out.join("jpeg-ls-color.dcm"))?;
    println!("wrote samples to {}", out.display());
    Ok(())
}
