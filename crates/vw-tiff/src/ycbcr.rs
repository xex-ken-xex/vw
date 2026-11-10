//! 色差を間引いた YCbCr（YCbCrSubSampling が 1, 1 でないもの）で、JPEG 以外の圧縮のもの。
//!
//! 間引いた YCbCr は、横 h × 縦 v 画素の塊ごとに「Y を h × v 個、Cb、Cr」の順で並ぶ。tiff クレートは
//! この並びを読まないので、写しの IFD を「1 行が塊 1 段分のバイトの 8bit グレー」に書き換え、
//! ほどいたバイトを受け取ってここで並べ直す（圧縮をほどく処理を自前で持たずに済むため）。
//! JPEG で圧縮したものは JPEG の中で間引きが戻るので、ここは通らない。

use std::io::Cursor;

use tiff::decoder::Decoder;
use tiff::tags::Tag;
use vw_core::{ColorModel, DecodeError, PixelData, VwImage};

use crate::{invalid, layout, open};

const YCBCR_SUBSAMPLING: u16 = 530;

/// 今のページが、ここで読む間引き方なら `(h, v)`。
pub fn subsampling(decoder: &mut Decoder<Cursor<&[u8]>>) -> Option<(usize, usize)> {
    let tag = |d: &mut Decoder<Cursor<&[u8]>>, t| d.find_tag_unsigned::<u32>(t).ok().flatten();
    if tag(decoder, Tag::PhotometricInterpretation) != Some(6)
        || matches!(tag(decoder, Tag::Compression), Some(6 | 7))
    {
        return None;
    }
    let s = decoder
        .find_tag_unsigned_vec::<u16>(Tag::Unknown(YCBCR_SUBSAMPLING))
        .ok()
        .flatten()?;
    let (h, v) = (usize::from(*s.first()?), usize::from(*s.get(1)?));
    let valid = |n| matches!(n, 1 | 2 | 4);
    (valid(h) && valid(v) && (h, v) != (1, 1)).then_some((h, v))
}

pub fn pixels(
    data: &[u8],
    ifd: usize,
    decoder: &mut Decoder<Cursor<&[u8]>>,
    (h, v): (usize, usize),
) -> Result<VwImage, DecodeError> {
    let tag = |d: &mut Decoder<Cursor<&[u8]>>, t| d.find_tag_unsigned::<u32>(t).ok().flatten();
    let (width, height) = decoder.dimensions().map_err(invalid)?;
    let (width, height) = (width as usize, height as usize);
    let bits = decoder
        .find_tag_unsigned_vec::<u16>(Tag::BitsPerSample)
        .ok()
        .flatten()
        .unwrap_or_default();
    if bits.iter().any(|&b| b != 8) || tag(decoder, Tag::PlanarConfiguration) == Some(2) {
        return Err(DecodeError::Unsupported(
            "TIFF subsampled YCbCr other than 8-bit interleaved".into(),
        ));
    }
    let tile = match (tag(decoder, Tag::TileWidth), tag(decoder, Tag::TileLength)) {
        (Some(w), Some(l)) if w > 0 && l > 0 => Some((w as usize, l as usize)),
        _ => None,
    };
    let rows_per_strip = tag(decoder, Tag::RowsPerStrip)
        .map_or(height, |r| r as usize)
        .min(height)
        .max(1);
    let (chunk_w, chunk_h) = tile.unwrap_or((width, rows_per_strip));
    if chunk_h % v != 0 && chunk_h != height {
        return Err(DecodeError::Invalid(
            "TIFF YCbCr strip height is not a multiple of the subsampling".into(),
        ));
    }
    let unit = h * v + 2;
    // 1 つのストリップかタイルの、塊 1 段分のバイト数と段の数。
    let block_row = chunk_w.div_ceil(h) * unit;
    let block_rows = chunk_h.div_ceil(v);
    let across = tile.map_or(1, |_| width.div_ceil(chunk_w));
    let down = height.div_ceil(chunk_h);
    let patched = layout::with_byte_samples(
        data,
        ifd,
        &layout::ByteSamples {
            image_width: across * block_row,
            tile_width: tile.map(|_| block_row),
            keep_samples: false,
            rows: Some((
                if tile.is_some() {
                    down * block_rows
                } else {
                    height.div_ceil(v)
                },
                block_rows,
            )),
        },
    )
    .ok_or_else(|| DecodeError::Invalid("broken IFD".into()))?;
    let mut bytes = open(&patched)?;
    bytes.seek_to_image(ifd).map_err(invalid)?;
    let mut ycbcr = vec![0u8; width * height * 3];
    for index in 0..across * down {
        let mut chunk = bytes.read_chunk(index as u32).map_err(invalid)?;
        let chunk = chunk.as_buffer(0);
        let chunk = chunk.as_bytes();
        let (x0, y0) = (index % across * chunk_w, index / across * chunk_h);
        let rows = if tile.is_some() {
            chunk_h
        } else {
            chunk_h.min(height - y0)
        };
        for by in 0..rows.div_ceil(v) {
            for bx in 0..chunk_w.div_ceil(h) {
                let at = by * block_row + bx * unit;
                let block = chunk.get(at..at + unit).ok_or_else(|| {
                    DecodeError::Invalid("TIFF chunk is shorter than its size".into())
                })?;
                let (cb, cr) = (block[h * v], block[h * v + 1]);
                for j in 0..v {
                    for i in 0..h {
                        let (x, y) = (x0 + bx * h + i, y0 + by * v + j);
                        if x < width && y < height {
                            let to = (y * width + x) * 3;
                            ycbcr[to..to + 3].copy_from_slice(&[block[j * h + i], cb, cr]);
                        }
                    }
                }
            }
        }
    }
    let rgb = layout::ycbcr_to_rgb(PixelData::U8(ycbcr))?;
    VwImage::new(width as u32, height as u32, ColorModel::Rgb, rgb).map_err(DecodeError::Invalid)
}
