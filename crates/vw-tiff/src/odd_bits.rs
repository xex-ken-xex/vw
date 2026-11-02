//! 1 サンプルが 8・16・32・64bit でない画像（計測用のカメラの 10・12・14bit、24bit など）。
//!
//! tiff クレートはこのビット数を断るので、圧縮をほどくところだけをクレートに任せる。
//! ファイルの写しの IFD を「1 行のバイト数を幅に持つ 8bit の画像」に書き換えて読み、
//! 返ってきた生のバイトを、上位ビットから順に切り出す（TIFF の詰め方）。
//! 圧縮をほどく処理（LZW・Deflate・PackBits）を自前で持たずに済むため。

use std::io::Cursor;

use tiff::decoder::Decoder;
use tiff::tags::Tag;
use vw_core::{ColorModel, DecodeError, PixelData, VwImage};

use crate::{invalid, layout, open};

/// PhotometricInterpretation。
const WHITE_IS_ZERO: u16 = 0;
const BLACK_IS_ZERO: u16 = 1;
const RGB: u16 = 2;

/// SampleFormat の符号付き整数。
const SIGNED: u16 = 2;
/// SampleFormat の浮動小数点。
const FLOAT: u16 = 3;

/// 今のページのサンプルが、ここで読むビット数（9〜31bit。24bit を含む）ならそのビット数。
pub fn bits(decoder: &mut Decoder<Cursor<&[u8]>>) -> Option<u8> {
    let bits = decoder
        .find_tag_unsigned_vec::<u16>(Tag::BitsPerSample)
        .ok()
        .flatten()?;
    let first = *bits.first()?;
    let odd = first > 8 && first < 32 && first != 16;
    (odd && bits.iter().all(|&b| b == first)).then_some(first as u8)
}

/// 画像の組み立て（IFD から読んだもの）。
struct Shape {
    width: usize,
    height: usize,
    bits: u32,
    /// 1 画素のサンプル数（色 + アルファ）。
    samples: usize,
    /// 面を分けて持つ（PlanarConfiguration = 2）。
    planar: bool,
    signed: bool,
    photometric: u16,
    /// タイルなら `(幅, 高さ)`。ストリップなら `None`。
    tile: Option<(usize, usize)>,
    rows_per_strip: usize,
}

impl Shape {
    fn read(decoder: &mut Decoder<Cursor<&[u8]>>, bits: u8) -> Result<Self, DecodeError> {
        let tag = |d: &mut Decoder<Cursor<&[u8]>>, t| d.find_tag_unsigned::<u32>(t).ok().flatten();
        let (width, height) = decoder.dimensions().map_err(invalid)?;
        let samples = tag(decoder, Tag::SamplesPerPixel).unwrap_or(1) as usize;
        let planar = tag(decoder, Tag::PlanarConfiguration) == Some(2) && samples > 1;
        let photometric = tag(decoder, Tag::PhotometricInterpretation)
            .ok_or_else(|| DecodeError::Invalid("TIFF without PhotometricInterpretation".into()))?
            as u16;
        let format = tag(decoder, Tag::SampleFormat).unwrap_or(1) as u16;
        let unsupported = |what: &str| {
            Err(DecodeError::Unsupported(format!(
                "TIFF with {bits}-bit samples {what}"
            )))
        };
        if format == FLOAT {
            return unsupported("in floating point");
        }
        // 差分（Predictor）は、libtiff もこのビット数では書かない。
        if tag(decoder, Tag::Predictor).is_some_and(|p| p != 1) {
            return unsupported("and a predictor");
        }
        if tag(decoder, Tag::Unknown(266)).is_some_and(|f| f != 1) {
            return unsupported("filled from the lowest bit");
        }
        // JPEG の圧縮は 8bit の入れ物に書き換えると中身ごと別物として読まれるため。
        if matches!(tag(decoder, Tag::Compression), Some(6 | 7)) {
            return unsupported("in JPEG");
        }
        let tile = match (tag(decoder, Tag::TileWidth), tag(decoder, Tag::TileLength)) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some((w as usize, h as usize)),
            (None, None) => None,
            _ => return Err(DecodeError::Invalid("TIFF tile size is incomplete".into())),
        };
        let rows_per_strip = tag(decoder, Tag::RowsPerStrip)
            .map_or(height, |r| r.min(height))
            .max(1) as usize;
        Ok(Self {
            width: width as usize,
            height: height as usize,
            bits: u32::from(bits),
            samples,
            planar,
            signed: format == SIGNED,
            photometric,
            tile,
            rows_per_strip,
        })
    }

    /// 1 つのストリップかタイルの 1 行に入るサンプルの数。
    fn chunk_samples(&self) -> usize {
        let across = self.tile.map_or(self.width, |(w, _)| w);
        across * if self.planar { 1 } else { self.samples }
    }

    /// 1 つのストリップかタイルの 1 行のバイト数（行末は 1 バイトに切り上げ）。
    fn chunk_row_bytes(&self) -> usize {
        (self.chunk_samples() * self.bits as usize).div_ceil(8)
    }

    fn model(&self) -> Result<ColorModel, DecodeError> {
        Ok(match (self.photometric, self.samples) {
            (WHITE_IS_ZERO | BLACK_IS_ZERO, 1) => ColorModel::Gray,
            (WHITE_IS_ZERO | BLACK_IS_ZERO, 2) => ColorModel::GrayAlpha,
            (RGB, 3) => ColorModel::Rgb,
            (RGB, 4) => ColorModel::Rgba,
            (p, n) => {
                return Err(DecodeError::Unsupported(format!(
                    "TIFF with {}-bit samples, photometric {p} and {n} samples",
                    self.bits
                )));
            }
        })
    }
}

/// `ifd` 番目のページ（`decoder` はそのページを指す）の画素。16bit 以下は値を変えずに 16bit の入れ物に入れ、
/// 意味のあるビット数をファイルのビット数にする。それより多いもの（24bit など）と符号付きは実数にする。
pub fn pixels(
    data: &[u8],
    ifd: usize,
    decoder: &mut Decoder<Cursor<&[u8]>>,
    bits: u8,
) -> Result<VwImage, DecodeError> {
    let shape = Shape::read(decoder, bits)?;
    let model = shape.model()?;
    let values = samples(data, ifd, &shape)?;
    let data = if shape.signed {
        let shift = 32 - shape.bits;
        PixelData::F32(
            values
                .into_iter()
                .map(|v| (((v << shift) as i32) >> shift) as f32)
                .collect(),
        )
    } else {
        let max = (1u32 << shape.bits) - 1;
        let invert = shape.photometric == WHITE_IS_ZERO;
        let value = |v: u32| if invert { max - v.min(max) } else { v };
        if shape.bits <= 16 {
            // アルファは計測の値ではなく、表示は 16bit の全範囲を不透明とみなすので、16bit に引き伸ばす。
            // 色の値のように「意味のあるビット数」で読ませる口がアルファにはないため。
            let alpha = matches!(model, ColorModel::GrayAlpha | ColorModel::Rgba)
                .then_some(shape.samples - 1);
            PixelData::U16(
                values
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| match alpha {
                        Some(a) if i % shape.samples == a => {
                            (u64::from(v.min(max)) * 65535 / u64::from(max)) as u16
                        }
                        _ => value(v) as u16,
                    })
                    .collect(),
            )
        } else {
            PixelData::F32(values.into_iter().map(|v| value(v) as f32).collect())
        }
    };
    let keeps_bits = matches!(data, PixelData::U16(_));
    let mut img = VwImage::new(shape.width as u32, shape.height as u32, model, data)
        .map_err(DecodeError::Invalid)?;
    if keeps_bits {
        img.significant_bits = shape.bits;
    }
    Ok(img)
}

/// すべてのサンプルを、画素ごとに色を並べた順（RGBRGB…）で。
fn samples(data: &[u8], ifd: usize, shape: &Shape) -> Result<Vec<u32>, DecodeError> {
    let row_bytes = shape.chunk_row_bytes();
    let tiles_across = shape.tile.map(|(w, _)| shape.width.div_ceil(w));
    let patched = layout::with_byte_samples(
        data,
        ifd,
        &layout::ByteSamples {
            image_width: tiles_across.map_or(row_bytes, |n| n * row_bytes),
            tile_width: shape.tile.map(|_| row_bytes),
            keep_samples: shape.planar,
        },
    )
    .ok_or_else(|| DecodeError::Invalid("broken IFD".into()))?;
    let mut decoder = open(&patched)?;
    decoder.seek_to_image(ifd).map_err(invalid)?;
    let (chunk_w, chunk_h) = shape.tile.unwrap_or((shape.width, shape.rows_per_strip));
    let across = tiles_across.unwrap_or(1);
    let per_plane = across * shape.height.div_ceil(chunk_h);
    let planes = if shape.planar { shape.samples } else { 1 };
    let per_pixel = shape.samples;
    let mut out = vec![0u32; shape.width * shape.height * per_pixel];
    let mut row_values = Vec::with_capacity(shape.chunk_samples());
    for plane in 0..planes {
        for local in 0..per_plane {
            let index = u32::try_from(plane * per_plane + local)
                .map_err(|_| DecodeError::Invalid("too many TIFF chunks".into()))?;
            let mut chunk = decoder.read_chunk(index).map_err(invalid)?;
            let bytes = chunk.as_buffer(0);
            let bytes = bytes.as_bytes();
            let x0 = local % across * chunk_w;
            let y0 = local / across * chunk_h;
            let w = chunk_w.min(shape.width - x0);
            let h = chunk_h.min(shape.height - y0);
            for row in 0..h {
                let line = bytes
                    .get(row * row_bytes..(row + 1) * row_bytes)
                    .ok_or_else(|| {
                        DecodeError::Invalid("TIFF chunk is shorter than its size".into())
                    })?;
                unpack_row(line, shape.chunk_samples(), shape.bits, &mut row_values);
                let in_chunk = if shape.planar { 1 } else { per_pixel };
                for col in 0..w {
                    for k in 0..in_chunk {
                        let to = ((y0 + row) * shape.width + x0 + col) * per_pixel
                            + if shape.planar { plane } else { k };
                        out[to] = row_values[col * in_chunk + k];
                    }
                }
            }
        }
    }
    Ok(out)
}

/// 上位ビットから順に `bits` ビットずつ詰めた行から、`count` 個の値を `out` に取り出す。
pub fn unpack_row(line: &[u8], count: usize, bits: u32, out: &mut Vec<u32>) {
    out.clear();
    let mask = (1u64 << bits) - 1;
    let mut acc: u64 = 0;
    let mut have = 0u32;
    let mut bytes = line.iter();
    for _ in 0..count {
        while have < bits {
            acc = (acc << 8) | u64::from(bytes.next().copied().unwrap_or(0));
            have += 8;
        }
        have -= bits;
        out.push(((acc >> have) & mask) as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_bit_values_are_taken_from_the_highest_bit_first() {
        let mut out = Vec::new();
        unpack_row(&[0xAB, 0xCD, 0xEF], 2, 12, &mut out);
        assert_eq!(out, [0xABC, 0xDEF]);
    }

    #[test]
    fn values_running_across_bytes_are_joined() {
        // 10bit: 1111111111 0000000001 → 11111111 11000000 00010000
        let mut out = Vec::new();
        unpack_row(&[0xFF, 0xC0, 0x10], 2, 10, &mut out);
        assert_eq!(out, [0x3FF, 0x001]);
    }

    #[test]
    fn twenty_four_bit_values_are_kept_whole() {
        let mut out = Vec::new();
        unpack_row(&[0x12, 0x34, 0x56, 0xFE, 0xDC, 0xBA], 2, 24, &mut out);
        assert_eq!(out, [0x123456, 0xFEDCBA]);
    }
}
