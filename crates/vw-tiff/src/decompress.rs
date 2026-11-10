//! tiff クレートがほどけない圧縮（FAX の Group 3 と Huffman、LZMA、ZSTD）を自前でほどき、
//! 無圧縮の写しに書き換える。色の扱い・面の分け方・差分（Predictor）の戻しは、写しを今の読み込みに渡して任せる。

use std::io::Read;

use vw_core::DecodeError;

use crate::fax;
use crate::layout::{self, Ifd};

const COMPRESSION: u16 = 259;
const LZMA: u64 = 34925;
const ZSTD: u64 = 50000;

/// `ifd` 番目のページが自前でほどく圧縮なら、ほどいた写し。そうでなければ `None`。
pub fn uncompressed(data: &[u8], ifd: usize) -> Result<Option<Vec<u8>>, DecodeError> {
    let found = layout::ifd(data, ifd).ok_or_else(|| DecodeError::Invalid("broken IFD".into()))?;
    let compression = found.value(COMPRESSION).unwrap_or(1);
    let fax = fax::Kind::of(compression, found.value(T4_OPTIONS).unwrap_or(0));
    if fax.is_none() && compression != LZMA && compression != ZSTD {
        return Ok(None);
    }
    let shape = Shape::read(&found)?;
    vw_core::limits::check_image(
        shape.width as u64,
        shape.height as u64,
        (shape.samples * shape.bits).div_ceil(8).max(1) as u64,
    )?;
    if fax.is_some() && (shape.bits != 1 || shape.samples != 1) {
        return Err(DecodeError::Unsupported(
            "CCITT fax compression with more than 1 bit per pixel".into(),
        ));
    }
    let lsb_first = found.value(FILL_ORDER) == Some(2);
    let mut chunks = Vec::with_capacity(shape.offsets.len());
    for i in 0..shape.offsets.len() {
        let input = shape.input(data, i)?;
        let (width, rows) = shape.chunk_size(i);
        let expected = rows * shape.row_bytes(width);
        let chunk = match fax {
            Some(kind) => fax::decode(input, width, rows, kind, lsb_first)?,
            None if compression == LZMA => {
                read_up_to(lzma_rust2::XzReader::new(input, true), expected, "LZMA")?
            }
            None => read_up_to(
                ruzstd::decoding::StreamingDecoder::new(input)
                    .map_err(|e| DecodeError::Invalid(format!("broken ZSTD data: {e}")))?,
                expected,
                "ZSTD",
            )?,
        };
        chunks.push(chunk);
    }
    layout::with_uncompressed_chunks(data, ifd, &chunks)
        .map(Some)
        .ok_or_else(|| DecodeError::Invalid("broken IFD".into()))
}

const FILL_ORDER: u16 = 266;
const T4_OPTIONS: u16 = 292;

/// 伸長したバイトを、多くても `limit` バイトまで読む。壊れたファイルで、宣言した大きさを超えて
/// メモリを使い続けないように。
fn read_up_to(reader: impl Read, limit: usize, name: &str) -> Result<Vec<u8>, DecodeError> {
    let mut out = Vec::with_capacity(limit);
    reader
        .take(limit as u64)
        .read_to_end(&mut out)
        .map_err(|e| DecodeError::Invalid(format!("broken {name} data: {e}")))?;
    Ok(out)
}

/// ストリップかタイルの組み立て。
pub struct Shape {
    pub width: usize,
    pub height: usize,
    /// 1 つのストリップかタイルに入る、1 画素のサンプル数（面を分けるなら 1）。
    pub samples: usize,
    pub bits: usize,
    tile: Option<(usize, usize)>,
    rows_per_strip: usize,
    pub offsets: Vec<u64>,
    counts: Vec<u64>,
}

impl Shape {
    pub fn read(ifd: &Ifd) -> Result<Self, DecodeError> {
        let invalid = |what: &str| DecodeError::Invalid(format!("TIFF without {what}"));
        let width = ifd.value(256).ok_or_else(|| invalid("ImageWidth"))? as usize;
        let height = ifd.value(257).ok_or_else(|| invalid("ImageLength"))? as usize;
        let samples = ifd.value(277).unwrap_or(1) as usize;
        let planar = ifd.value(284) == Some(2);
        let bits = ifd.value(258).unwrap_or(1) as usize;
        let tile = match (ifd.value(322), ifd.value(323)) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some((w as usize, h as usize)),
            _ => None,
        };
        let (offsets, counts) = if tile.is_some() {
            (ifd.values(324), ifd.values(325))
        } else {
            (ifd.values(273), ifd.values(279))
        };
        let offsets = offsets.ok_or_else(|| invalid("chunk offsets"))?;
        let counts = counts.ok_or_else(|| invalid("chunk byte counts"))?;
        if offsets.len() != counts.len() {
            return Err(DecodeError::Invalid(
                "TIFF chunk offsets and byte counts differ in number".into(),
            ));
        }
        let rows_per_strip = ifd
            .value(278)
            .map_or(height, |r| r as usize)
            .clamp(1, height.max(1));
        Ok(Self {
            width,
            height,
            samples: if planar { 1 } else { samples },
            bits,
            tile,
            rows_per_strip,
            offsets,
            counts,
        })
    }

    /// `index` 番目のストリップかタイルの、ファイルの中のバイト。
    pub fn input<'a>(&self, data: &'a [u8], index: usize) -> Result<&'a [u8], DecodeError> {
        let offset = usize::try_from(self.offsets[index]).ok();
        let count = usize::try_from(self.counts[index]).ok();
        offset
            .zip(count)
            .and_then(|(o, c)| data.get(o..o.checked_add(c)?))
            .ok_or_else(|| DecodeError::Invalid("TIFF chunk runs past the end of the file".into()))
    }

    /// `index` 番目のストリップかタイルの左上の位置（面を分けた画像では面の中の位置）。
    pub fn origin(&self, index: usize) -> (usize, usize) {
        match self.tile {
            Some((w, h)) => {
                let across = self.width.div_ceil(w).max(1);
                let down = self.height.div_ceil(h).max(1);
                let local = index % (across * down);
                (local % across * w, local / across * h)
            }
            None => {
                let strips = self.height.div_ceil(self.rows_per_strip).max(1);
                (0, index % strips * self.rows_per_strip)
            }
        }
    }

    /// `index` 番目のストリップかタイルの、幅と行の数。タイルは端でも全部の大きさで入っている。
    pub fn chunk_size(&self, index: usize) -> (usize, usize) {
        match self.tile {
            Some(size) => size,
            None => {
                let strips = self.height.div_ceil(self.rows_per_strip).max(1);
                let first = index % strips * self.rows_per_strip;
                (
                    self.width,
                    self.rows_per_strip.min(self.height.saturating_sub(first)),
                )
            }
        }
    }

    fn row_bytes(&self, width: usize) -> usize {
        (width * self.samples * self.bits).div_ceil(8)
    }
}
