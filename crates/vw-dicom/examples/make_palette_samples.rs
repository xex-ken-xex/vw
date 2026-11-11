//! 十字で番号を出す PALETTE COLOR と、灰色の一部の値だけに色を当てる Supplemental Palette の
//! DICOM を作る。`cargo run -p vw-dicom --example make_palette_samples -- <出力フォルダ>`
//!
//! - `index-16bit.dcm`: PALETTE COLOR、画素は 16bit の番号 1000〜1255（左から右へ）、表の先頭は 1000。
//!   十字には「#1000 → 色」の形で 1000 台の番号が出る
//! - `supplemental-8bit.dcm`: 8bit の灰色。左から右へ 0〜255 の明るさで、200〜255 だけに
//!   赤から黄の色を当てる（右の 1/5 ほどが色になる）
//! - `supplemental-ct.dcm`: 16bit の CT（Rescale Intercept -1024）。灰色の円の中に、格納値 3000〜3255 の
//!   リング（16bit の表で青から赤）がある。WW/WL を動かすと灰色だけが変わり、リングの色は変わらない
//! - `supplemental-3frames.dcm`: 8bit の灰色の 3 フレーム。色の帯がフレームごとに右へ動く
//! - `supplemental-broken.dcm`: Red の表が空で、Green・Blue の表がない。灰色のまま読める
//! - `enhanced-palette.dcm`: Enhanced Palette Color LUT の印だけを持つ灰色の CT。灰色で出て、
//!   ログに「not supported」の行が 1 回だけ出る

use std::path::PathBuf;

use dicom_dictionary_std::tags;
use vw_dicom::synth::{SC_IMAGE, Synth};

const SIDE: usize = 256;

/// 0..1 を 青 → 緑 → 赤 にする。
fn heat(t: f32) -> [f32; 3] {
    [
        (2.0 * t - 1.0).clamp(0.0, 1.0),
        1.0 - (2.0 * t - 1.0).abs(),
        (1.0 - 2.0 * t).clamp(0.0, 1.0),
    ]
}

/// `n` 項目の色の表を、8bit なら 2 項目を 1 語に詰めて、16bit ならそのまま、色ごとに返す。
fn table(n: usize, bits: u16, color: impl Fn(f32) -> [f32; 3]) -> [Vec<u16>; 3] {
    let colors: Vec<[f32; 3]> = (0..n).map(|i| color(i as f32 / (n - 1) as f32)).collect();
    [0, 1, 2].map(|c| {
        let values: Vec<f32> = colors.iter().map(|v| v[c]).collect();
        if bits == 8 {
            values
                .chunks(2)
                .map(|p| {
                    let b = |v: f32| (v * 255.0).round() as u8;
                    u16::from_le_bytes([b(p[0]), b(*p.get(1).unwrap_or(&p[0]))])
                })
                .collect()
        } else {
            values
                .iter()
                .map(|v| (v * 65535.0).round() as u16)
                .collect()
        }
    })
}

fn with_table(mut s: Synth, n: usize, first: u16, bits: u16, t: &[Vec<u16>; 3]) -> Synth {
    for (c, words) in t.iter().enumerate() {
        s = s.lut(c, n as u32, first, bits, words, false);
    }
    s
}

fn ramp8() -> Vec<u8> {
    (0..SIDE * SIDE).map(|i| (i % SIDE) as u8).collect()
}

fn main() -> std::io::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&dir)?;
    let side = SIDE as u16;
    let mut files: Vec<(&str, Synth)> = Vec::new();

    let index: Vec<u16> = (0..SIDE * SIDE).map(|i| 1000 + (i % SIDE) as u16).collect();
    let rainbow = table(256, 16, heat);
    files.push((
        "index-16bit",
        with_table(
            Synth::new(SC_IMAGE, side, side)
                .pixels_u16(&index, 16)
                .photometric("PALETTE COLOR"),
            256,
            1000,
            16,
            &rainbow,
        ),
    ));

    let fire = table(56, 8, |t| [1.0, t, 0.0]);
    files.push((
        "supplemental-8bit",
        with_table(
            Synth::new(SC_IMAGE, side, side).frames_u8(&[&ramp8()]),
            56,
            200,
            8,
            &fire,
        ),
    ));

    let c = SIDE as f32 / 2.0;
    let ct: Vec<u16> = (0..SIDE * SIDE)
        .map(|i| {
            let (x, y) = ((i % SIDE) as f32, (i / SIDE) as f32);
            let r = ((x - c).powi(2) + (y - c).powi(2)).sqrt();
            if (70.0..90.0).contains(&r) {
                // リングの角度で 3000..=3255 を回す。
                let a = (y - c).atan2(x - c) / std::f32::consts::TAU + 0.5;
                3000 + (a * 255.0).round() as u16
            } else if r < 110.0 {
                // 円の中は -24..=1000 HU（格納値 1000..=2024）の上下のグラデーション。
                1000 + (y / SIDE as f32 * 1024.0) as u16
            } else {
                24
            }
        })
        .collect();
    files.push((
        "supplemental-ct",
        with_table(
            Synth::new(SC_IMAGE, side, side)
                .pixels_u16(&ct, 12)
                .rescale(1.0, -1024.0)
                .window(500.0, 1000.0),
            256,
            3000,
            16,
            &rainbow,
        ),
    ));

    let frames: Vec<Vec<u8>> = (0..3)
        .map(|k| {
            (0..SIDE * SIDE)
                .map(|i| {
                    let x = i % SIDE;
                    if (k * 80..k * 80 + 40).contains(&x) {
                        250
                    } else {
                        (x / 2) as u8
                    }
                })
                .collect()
        })
        .collect();
    let frame_refs: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
    files.push((
        "supplemental-3frames",
        with_table(
            Synth::new(SC_IMAGE, side, side).frames_u8(&frame_refs),
            6,
            250,
            8,
            &table(6, 8, |_| [0.0, 1.0, 1.0]),
        ),
    ));

    files.push((
        "supplemental-broken",
        Synth::new(SC_IMAGE, side, side)
            .frames_u8(&[&ramp8()])
            .lut(0, 4, 0, 8, &[], false),
    ));

    files.push((
        "enhanced-palette",
        Synth::ct(side, side).empty_sequence(tags::ENHANCED_PALETTE_COLOR_LOOKUP_TABLE_SEQUENCE),
    ));

    for (name, synth) in &files {
        let path = dir.join(format!("{name}.dcm"));
        synth.write(&path)?;
        println!("{}", path.display());
    }
    Ok(())
}
