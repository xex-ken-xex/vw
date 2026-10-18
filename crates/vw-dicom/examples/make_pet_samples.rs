//! PET の Bq/ml と SUVbw の出し方を確かめる DICOM を作る。
//! `cargo run -p vw-dicom --example make_pet_samples -- <出力フォルダ>`
//!
//! どれも 128x128 で、背景が 5,000 Bq/ml、中央の円が 20,000 Bq/ml（GML は 1.5 / 6.0 g/ml、
//! CNTS は 500 / 2,000）。体重 70 kg、F-18（半減期 6586.2 秒）370 MBq を 9:00 に投与し、
//! シリーズは 10:00、撮像は 10:05。名前などはすべて架空のもの。
//!
//! - `1-suv.dcm`: SUV を出せる（Decay Correction は START）
//! - `2-no-weight.dcm`: 体重が無い。Bq/ml だけになる
//! - `3-no-injection.dcm`: 投与の時刻が無い。Bq/ml だけになる
//! - `4-admin.dcm`: Decay Correction が ADMIN（投与の時刻に補正済み）
//! - `5-gml.dcm`: Units が GML（装置が SUV にしたもの）
//! - `6-cnts.dcm`: Units が CNTS
//!
//! 期待する値は、作るときに標準出力に出す。

use std::path::PathBuf;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use vw_dicom::synth::{self, Synth};

const SIZE: u16 = 128;
const HALF_LIFE: f64 = 6586.2;
const DOSE: f64 = 370e6;

/// 背景と中央の円の値を、`slope` で割った 16bit の画素にする。
fn pixels(background: f64, hot: f64, slope: f64) -> Vec<u16> {
    let c = f64::from(SIZE) / 2.0;
    (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
        .map(|(x, y)| {
            let r = ((f64::from(x) - c).powi(2) + (f64::from(y) - c).powi(2)).sqrt();
            let v = if r < 20.0 { hot } else { background };
            (v / slope).round() as u16
        })
        .collect()
}

struct Sample {
    file: &'static str,
    units: &'static str,
    decay: &'static str,
    weight: Option<&'static str>,
    injection: Option<&'static str>,
    values: (f64, f64),
    slope: f64,
}

fn build(number: i32, s: &Sample) -> Synth {
    let uid = format!("1.2.826.0.1.3680043.9.79{number:02}");
    let (background, hot) = s.values;
    let mut synth = Synth::new(synth::PET_IMAGE, SIZE, SIZE)
        .pixels_u16(&pixels(background, hot, s.slope), 16)
        .rescale(s.slope, 0.0)
        .window(hot / 2.0, hot)
        .patient("Kakuu^Pet")
        .study(&format!("{uid}.1"), "PET sample", "20261018")
        .series(&format!("{uid}.2"), number, s.file)
        .instance(&format!("{uid}.3"), 1)
        .text(tags::UNITS, VR::CS, s.units)
        .text(tags::DECAY_CORRECTION, VR::CS, s.decay)
        .text(tags::SERIES_DATE, VR::DA, "20261018")
        .text(tags::SERIES_TIME, VR::TM, "100000")
        .text(tags::ACQUISITION_DATE, VR::DA, "20261018")
        .text(tags::ACQUISITION_TIME, VR::TM, "100500")
        .radiopharmaceutical(Some(DOSE), Some(HALF_LIFE), s.injection);
    if let Some(w) = s.weight {
        synth = synth.text(tags::PATIENT_WEIGHT, VR::DS, w);
    }
    synth
}

fn main() -> std::io::Result<()> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "pet-samples".into()),
    );
    std::fs::create_dir_all(&out)?;
    let bq = (5_000.0, 20_000.0);
    let samples = [
        Sample {
            file: "1-suv.dcm",
            units: "BQML",
            decay: "START",
            weight: Some("70"),
            injection: Some("090000"),
            values: bq,
            slope: 1.0,
        },
        Sample {
            file: "2-no-weight.dcm",
            units: "BQML",
            decay: "START",
            weight: None,
            injection: Some("090000"),
            values: bq,
            slope: 1.0,
        },
        Sample {
            file: "3-no-injection.dcm",
            units: "BQML",
            decay: "START",
            weight: Some("70"),
            injection: None,
            values: bq,
            slope: 1.0,
        },
        Sample {
            file: "4-admin.dcm",
            units: "BQML",
            decay: "ADMIN",
            weight: Some("70"),
            injection: Some("090000"),
            values: bq,
            slope: 1.0,
        },
        Sample {
            file: "5-gml.dcm",
            units: "GML",
            decay: "START",
            weight: Some("70"),
            injection: Some("090000"),
            values: (1.5, 6.0),
            slope: 0.001,
        },
        Sample {
            file: "6-cnts.dcm",
            units: "CNTS",
            decay: "START",
            weight: Some("70"),
            injection: Some("090000"),
            values: (500.0, 2_000.0),
            slope: 1.0,
        },
    ];
    for (i, s) in samples.iter().enumerate() {
        build(i as i32 + 1, s).write(&out.join(s.file))?;
    }
    let start = DOSE * 0.5f64.powf(3600.0 / HALF_LIFE);
    let factor = |dose: f64| 70_000.0 / dose;
    println!(
        "1-suv: background {:.2} SUVbw, circle {:.2} SUVbw",
        5_000.0 * factor(start),
        20_000.0 * factor(start)
    );
    println!(
        "4-admin: background {:.2} SUVbw, circle {:.2} SUVbw",
        5_000.0 * factor(DOSE),
        20_000.0 * factor(DOSE)
    );
    println!("wrote {} files to {}", samples.len(), out.display());
    Ok(())
}
