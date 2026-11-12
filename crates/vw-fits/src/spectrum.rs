//! 軸が 1 本だけ（ほかの軸の長さが 1）のデータを、スペクトルとして読む。

use std::sync::Arc;

use vw_core::spectrum::{Axis, AxisKind, Spectrum};
use vw_core::{ColorModel, PixelData, VwImage};

use crate::header::Hdu;

/// 高さ 1 画素の灰色の画像なら、その値と 1 本目の軸をスペクトルにする。
///
/// 軸の値は [`crate::spectral`] の式で出す。スペクトルの軸として読めない `CTYPE1` は、
/// `CRVAL1 + (i - CRPIX1) × CDELT1` の線形で、`CTYPE1` と単位をそのまま見出しにする。
pub fn of(hdu: &Hdu, image: &VwImage) -> Option<Spectrum> {
    if image.height != 1 || image.width < 2 || image.color != ColorModel::Gray {
        return None;
    }
    let values: Arc<[f32]> = match &image.data {
        PixelData::U8(v) => v.iter().map(|&x| f32::from(x)).collect(),
        PixelData::U16(v) => v.iter().map(|&x| f32::from(x)).collect(),
        PixelData::F32(v) => v.as_slice().into(),
    };
    let axis = match crate::spectral::read(hdu, 1) {
        Some(spectral) => spectral.axis(values.len()),
        None => {
            let name = hdu
                .get("CTYPE1")
                .map(|c| c.trim().to_uppercase())
                .map(|c| c.split('-').next().unwrap_or(&c).to_owned())
                .unwrap_or_default();
            let unit = hdu
                .get("CUNIT1")
                .map(|u| u.trim().to_owned())
                .unwrap_or_default();
            Axis::linear(
                AxisKind::Other { name, unit },
                hdu.float("CRVAL1").unwrap_or(1.0),
                hdu.float("CRPIX1").unwrap_or(1.0),
                hdu.float("CDELT1")
                    .or_else(|| hdu.float("CD1_1"))
                    .unwrap_or(1.0),
                values.len(),
            )
        }
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
