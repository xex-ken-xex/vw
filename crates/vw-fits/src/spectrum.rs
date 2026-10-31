//! 軸が 1 本だけ（ほかの軸の長さが 1）のデータを、スペクトルとして読む。

use std::sync::Arc;

use vw_core::spectrum::{Axis, AxisKind, Spectrum};
use vw_core::{ColorModel, PixelData, VwImage};

use crate::header::Hdu;

/// 高さ 1 画素の灰色の画像なら、その値と 1 本目の軸をスペクトルにする。
///
/// 軸の値は `CRVAL1 + (i - CRPIX1) × CDELT1` の線形で出す。`FREQ-W2F` のような非線形の
/// 書き方も線形として読むのは、チャンネルの幅に比べて曲がりがずっと小さく、グラフの目盛りと
/// 見分けがつかないため（正確な変換は規格の式が多く、表示のためだけに持つほどではない）。
pub fn of(hdu: &Hdu, image: &VwImage) -> Option<Spectrum> {
    if image.height != 1 || image.width < 2 || image.color != ColorModel::Gray {
        return None;
    }
    let values: Arc<[f32]> = match &image.data {
        PixelData::U8(v) => v.iter().map(|&x| f32::from(x)).collect(),
        PixelData::U16(v) => v.iter().map(|&x| f32::from(x)).collect(),
        PixelData::F32(v) => v.as_slice().into(),
    };
    let ctype = hdu.get("CTYPE1").map(|c| c.trim().to_uppercase());
    let unit = hdu
        .get("CUNIT1")
        .map(|u| u.trim().to_owned())
        .unwrap_or_default();
    let (kind, factor) = match ctype.as_deref().map(|c| c.split('-').next().unwrap_or(c)) {
        Some("FREQ") => (AxisKind::Frequency, frequency_factor(&unit)),
        Some("VRAD" | "VOPT" | "VELO" | "FELO" | "VELOCITY") => {
            (AxisKind::Velocity, velocity_factor(&unit))
        }
        Some("WAVE" | "AWAV") => (AxisKind::Wavelength, wavelength_factor(&unit)),
        Some(name) => (
            AxisKind::Other {
                name: name.to_owned(),
                unit,
            },
            1.0,
        ),
        None => (
            AxisKind::Other {
                name: String::new(),
                unit,
            },
            1.0,
        ),
    };
    let axis = Axis {
        kind,
        reference: hdu.float("CRVAL1").unwrap_or(1.0) * factor,
        reference_pixel: hdu.float("CRPIX1").unwrap_or(1.0),
        step: hdu
            .float("CDELT1")
            .or_else(|| hdu.float("CD1_1"))
            .unwrap_or(1.0)
            * factor,
        rest_frequency: hdu
            .float("RESTFRQ")
            .or_else(|| hdu.float("RESTFREQ"))
            .filter(|f| *f > 0.0),
    };
    Some(Spectrum {
        values,
        axis,
        value_unit: hdu
            .get("BUNIT")
            .map(|u| u.trim().to_owned())
            .unwrap_or_default(),
    })
}

/// メタデータに書く、点の数と横軸の種類。
pub fn summary(s: &Spectrum) -> String {
    let kind = match &s.axis.kind {
        AxisKind::Frequency => "frequency".to_owned(),
        AxisKind::Velocity => "velocity".to_owned(),
        AxisKind::Wavelength => "wavelength".to_owned(),
        AxisKind::Other { name, .. } if name.is_empty() => "channel".to_owned(),
        AxisKind::Other { name, .. } => name.clone(),
    };
    format!("{} points, {kind}", s.values.len())
}

/// 規格の既定は基本の単位（Hz・m/s・m）なので、書いていない単位や読めない単位は 1 倍にする。
fn frequency_factor(unit: &str) -> f64 {
    match unit {
        "kHz" => 1e3,
        "MHz" => 1e6,
        "GHz" => 1e9,
        _ => 1.0,
    }
}

fn velocity_factor(unit: &str) -> f64 {
    match unit.to_lowercase().as_str() {
        "km/s" | "km s-1" | "km.s-1" => 1e3,
        _ => 1.0,
    }
}

fn wavelength_factor(unit: &str) -> f64 {
    match unit {
        "cm" => 1e-2,
        "mm" => 1e-3,
        "um" => 1e-6,
        "nm" => 1e-9,
        "Angstrom" | "angstrom" | "A" => 1e-10,
        _ => 1.0,
    }
}
