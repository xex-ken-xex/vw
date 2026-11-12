//! キューブの 1 点の、スペクトルの軸に沿った値（十字の点のスペクトル）。
//!
//! 面を 1 枚ずつ読むと、1 点のために全部の面を戻すことになる。圧縮していないファイルは
//! その点のバイトだけを拾い、タイル圧縮のファイルは戻した面を覚えておいて、十字を動かしても
//! 戻し直さない。

use std::collections::HashMap;
use std::sync::Arc;

use vw_core::spectrum::{Axis, AxisKind, Spectrum};
use vw_core::{DecodeError, PixelData};

use crate::header::{self, Hdu};

/// 戻した面を覚えておく上限（バイト）。超えたら忘れて読み直す。
const KEEP_BYTES: usize = 1 << 30;

/// 1 つのファイルのキューブ。ファイルの中身を持ち、点ごとのスペクトルを出す。
pub struct Cube {
    data: Vec<u8>,
    chosen: Option<usize>,
    flip_rows: bool,
    planes: HashMap<usize, Arc<Vec<f32>>>,
}

/// スペクトルを出す軸（3 本目以降のどれか）。
pub(crate) struct Channels {
    /// 3 本目の軸を 0 とした番号。
    pub(crate) index: usize,
    pub(crate) count: usize,
    /// 面の番号で、この軸の 1 つ分の幅。
    pub(crate) stride: usize,
}

impl Cube {
    /// `data` はファイルの中身（gzip のままでよい）。`chosen` は選んだ HDU、`flip_rows` は
    /// 設定の「下の行から並べる」。
    pub fn new(data: &[u8], chosen: Option<usize>, flip_rows: bool) -> Result<Self, DecodeError> {
        let data = crate::gunzip(data)?.into_owned();
        Ok(Self {
            data,
            chosen,
            flip_rows,
            planes: HashMap::new(),
        })
    }

    /// 表示中の面 `frame` の、画面の画素 `(x, y)` のスペクトル。キューブでなければ `None`。
    /// ほかの軸（偏光など）は、`frame` の位置のまま動かさない。
    pub fn spectrum(&mut self, frame: usize, x: u32, y: u32) -> Result<Option<Point>, DecodeError> {
        let hdus = header::hdus(&self.data)?;
        let image = crate::find_image(&hdus, self.chosen)?;
        let Some(channels) = channels(image.hdu, &image.extra_axes) else {
            return Ok(None);
        };
        let (x, y) = (x as usize, y as usize);
        if x >= image.width || y >= image.height || frame >= image.planes {
            return Ok(None);
        }
        let y = if crate::bottom_up(image.hdu, self.flip_rows) {
            image.height - 1 - y
        } else {
            y
        };
        let here = (frame / channels.stride) % channels.count;
        let first = frame - here * channels.stride;
        let mut values = Vec::with_capacity(channels.count);
        for c in 0..channels.count {
            let plane = first + c * channels.stride;
            let at = y * image.width + x;
            let v = if image.compressed {
                self.compressed_value(&image, plane, at)?
            } else {
                plain_value(&self.data, &image, plane, at)?
            };
            values.push(v);
        }
        let hdu = image.hdu;
        let axis_number = channels.index + 3;
        let axis = match crate::spectral::read(hdu, axis_number) {
            Some(spectral) => spectral.axis(channels.count),
            None => linear_axis(hdu, axis_number, channels.count),
        };
        Ok(Some(Point {
            spectrum: Spectrum {
                values: values.into(),
                axis,
                value_unit: hdu
                    .get("BUNIT")
                    .or_else(|| hdus[0].get("BUNIT"))
                    .map(|u| u.trim().to_owned())
                    .unwrap_or_default(),
            },
            channel: here,
            first_frame: first,
            stride: channels.stride,
        }))
    }

    fn compressed_value(
        &mut self,
        image: &crate::Image,
        plane: usize,
        at: usize,
    ) -> Result<f32, DecodeError> {
        if let Some(p) = self.planes.get(&plane) {
            return Ok(p.get(at).copied().unwrap_or(f32::NAN));
        }
        let stored = crate::compressed_plane(&self.data, image, plane)?;
        let values: Vec<f32> = match crate::to_pixels(stored, image.hdu) {
            PixelData::U8(v) => v.into_iter().map(f32::from).collect(),
            PixelData::U16(v) => v.into_iter().map(f32::from).collect(),
            PixelData::F32(v) => v,
        };
        let v = values.get(at).copied().unwrap_or(f32::NAN);
        let held: usize = self.planes.values().map(|p| p.len() * 4).sum();
        if held + values.len() * 4 > KEEP_BYTES {
            self.planes.clear();
        }
        self.planes.insert(plane, Arc::new(values));
        Ok(v)
    }
}

/// 1 点のスペクトルと、表示中の面がその何番目か。
#[derive(Debug, Clone)]
pub struct Point {
    pub spectrum: Spectrum,
    /// 表示中の面のチャンネル（0 から）。
    pub channel: usize,
    first_frame: usize,
    stride: usize,
}

impl Point {
    /// チャンネル `channel` を出す面の番号（ほかの軸は表示中の面のまま）。
    pub fn frame_of(&self, channel: usize) -> usize {
        self.first_frame + channel * self.stride
    }
}

/// スペクトルを出す軸。スペクトルの軸として読めるものがあればそれ、なければ長さ 2 以上の最初の軸。
pub(crate) fn channels(hdu: &Hdu, extra_axes: &[usize]) -> Option<Channels> {
    let mut stride = 1;
    let mut found: Vec<(bool, Channels)> = Vec::new();
    for (index, &n) in extra_axes.iter().enumerate() {
        if n > 1 {
            let spectral = crate::spectral::read(hdu, index + 3).is_some();
            found.push((
                spectral,
                Channels {
                    index,
                    count: n,
                    stride,
                },
            ));
        }
        stride *= n.max(1);
    }
    let first_spectral = found.iter().position(|(s, _)| *s).unwrap_or(0);
    (!found.is_empty()).then(|| found.swap_remove(first_spectral).1)
}

/// スペクトルの軸として読めない軸は、`CRVAL + (i − CRPIX) × CDELT` で、名前と単位をそのまま見出しにする。
pub(crate) fn linear_axis(hdu: &Hdu, axis: usize, n: usize) -> Axis {
    let key = |k: &str| format!("{k}{axis}");
    let name = hdu
        .get(&key("CTYPE"))
        .map(|c| c.trim().to_uppercase())
        .map(|c| c.split('-').next().unwrap_or(&c).to_owned())
        .unwrap_or_default();
    let unit = hdu
        .get(&key("CUNIT"))
        .map(|u| u.trim().to_owned())
        .unwrap_or_default();
    Axis::linear(
        AxisKind::Other { name, unit },
        hdu.float(&key("CRVAL")).unwrap_or(1.0),
        hdu.float(&key("CRPIX")).unwrap_or(1.0),
        hdu.float(&key("CDELT"))
            .or_else(|| hdu.float(&format!("CD{axis}_{axis}")))
            .unwrap_or(1.0),
        n,
    )
}

/// 圧縮していない画像の、面 `plane` の `at` 番目の画素（`BSCALE`・`BZERO`・`BLANK` を当てた値）。
fn plain_value(
    data: &[u8],
    image: &crate::Image,
    plane: usize,
    at: usize,
) -> Result<f32, DecodeError> {
    let bytes = (image.bitpix.unsigned_abs() / 8) as usize;
    let offset = plane
        .checked_mul(image.plane_pixels())
        .and_then(|p| p.checked_add(at))
        .and_then(|p| p.checked_mul(bytes))
        .and_then(|p| p.checked_add(image.hdu.data_start))
        .ok_or_else(|| DecodeError::Invalid("FITS file is truncated".into()))?;
    let raw = data
        .get(offset..offset + bytes)
        .ok_or_else(|| DecodeError::Invalid("FITS file is truncated".into()))?;
    let hdu = image.hdu;
    let scale = hdu.float("BSCALE").unwrap_or(1.0);
    let zero = hdu.float("BZERO").unwrap_or(0.0);
    let integer = |v: i64| {
        if Some(v) == hdu.int("BLANK") {
            f32::NAN
        } else {
            (zero + scale * v as f64) as f32
        }
    };
    let real = |v: f64| (zero + scale * v) as f32;
    Ok(match image.bitpix {
        8 => integer(i64::from(raw[0])),
        16 => integer(i64::from(i16::from_be_bytes([raw[0], raw[1]]))),
        32 => integer(i64::from(i32::from_be_bytes(
            raw.try_into().expect("4 bytes"),
        ))),
        64 => integer(i64::from_be_bytes(raw.try_into().expect("8 bytes"))),
        -32 => real(f64::from(f32::from_be_bytes(
            raw.try_into().expect("4 bytes"),
        ))),
        -64 => real(f64::from_be_bytes(raw.try_into().expect("8 bytes"))),
        other => return Err(DecodeError::Invalid(format!("FITS BITPIX {other}"))),
    })
}
