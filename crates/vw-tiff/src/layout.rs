//! tiff クレートが返す並びを、vw の画素の並びに直す処理と、IFD の書き換え。

use vw_core::{DecodeError, PixelData};

/// 1 サンプル `bits`（1/2/4）ビットで詰めた行（行末は 1 バイトに切り上げ）を、1 サンプル 1 バイトに広げる。
pub fn unpack(packed: &[u8], width: usize, height: usize, bits: u8) -> Vec<u8> {
    let bits = bits as usize;
    let row_bytes = (width * bits).div_ceil(8);
    let mask = (1u8 << bits) - 1;
    let mut out = Vec::with_capacity(width * height);
    for row in packed.chunks(row_bytes).take(height) {
        for x in 0..width {
            let bit = x * bits;
            let byte = row.get(bit / 8).copied().unwrap_or(0);
            let shift = 8 - bits - bit % 8;
            out.push((byte >> shift) & mask);
        }
    }
    out
}

/// CMYK（`channels` が 5 なら CMYK + アルファ）を RGB（RGBA）にする。
///
/// インクの量を引くだけの単純な変換。ICC プロファイルで色を合わせる変換は、色の管理を
/// 持たない今のビューワでは他の形式の表示と揃わないので採らない。
pub fn cmyk_to_rgb(data: PixelData, channels: usize) -> Result<PixelData, DecodeError> {
    fn convert<T: Copy>(v: &[T], channels: usize, ink: impl Fn(T, T) -> T) -> Vec<T> {
        let mut out = Vec::with_capacity(v.len() / channels * (channels - 1));
        for px in v.chunks_exact(channels) {
            let k = px[3];
            out.extend([ink(px[0], k), ink(px[1], k), ink(px[2], k)]);
            if channels == 5 {
                out.push(px[4]);
            }
        }
        out
    }
    Ok(match data {
        PixelData::U8(v) => PixelData::U8(convert(&v, channels, |c, k| {
            ((255 - c as u32) * (255 - k as u32) / 255) as u8
        })),
        PixelData::U16(v) => PixelData::U16(convert(&v, channels, |c, k| {
            ((65535 - c as u32) as u64 * (65535 - k as u32) as u64 / 65535) as u16
        })),
        PixelData::F32(_) => return Err(DecodeError::Unsupported("TIFF CMYK depth".into())),
    })
}

/// YCbCr（8bit）を RGB にする。JPEG（JFIF）と同じ、全範囲を使う式で変換する。
///
/// ReferenceBlackWhite のタグ（値の範囲）は見ない。YCbCr の TIFF はほぼすべてが JPEG 圧縮で、
/// JPEG の中身は全範囲で書かれるため。
pub fn ycbcr_to_rgb(data: PixelData) -> Result<PixelData, DecodeError> {
    let PixelData::U8(v) = data else {
        return Err(DecodeError::Unsupported("TIFF YCbCr depth".into()));
    };
    let mut out = Vec::with_capacity(v.len());
    for px in v.as_chunks::<3>().0 {
        let (y, cb, cr) = (px[0] as f32, px[1] as f32 - 128.0, px[2] as f32 - 128.0);
        out.extend(
            [
                y + 1.402 * cr,
                y - 0.344_136 * cb - 0.714_136 * cr,
                y + 1.772 * cb,
            ]
            .map(|c| c.round().clamp(0.0, 255.0) as u8),
        );
    }
    Ok(PixelData::U8(out))
}

/// パレットの番号をカラーマップ（R の並び、G の並び、B の並びの順、各 16bit）で 8bit の RGB にする。
/// 範囲外の番号があれば `None`。
///
/// 16bit の色を 8bit に落とすのは、カラーマップは 8bit の色を 257 倍して書くのがふつうで、
/// 16bit で持ってもメモリが倍になるだけのため。
pub fn apply_color_map(indices: &[usize], map: &[u16]) -> Option<Vec<u8>> {
    let n = map.len() / 3;
    let mut out = Vec::with_capacity(indices.len() * 3);
    for &i in indices {
        if i >= n {
            return None;
        }
        out.extend([map[i] >> 8, map[n + i] >> 8, map[2 * n + i] >> 8].map(|c| c as u8));
    }
    Some(out)
}

/// `data` の写しの、`ifd` 番目の IFD の PhotometricInterpretation をグレー（BlackIsZero）にしたもの。
/// IFD が壊れていて書き換えられなければ `None`。
pub fn with_gray_photometric(data: &[u8], ifd: usize) -> Option<Vec<u8>> {
    let header = Header::read(data)?;
    let mut offset = header.first_ifd(data)?;
    for _ in 0..ifd {
        offset = header.next_ifd(data, offset)?;
    }
    let count = header.entry_count(data, offset)?;
    for i in 0..count {
        let entry = offset + header.count_size() + i * header.entry_size();
        if header.u16(data, entry)? == PHOTOMETRIC {
            let value = entry + header.value_offset();
            let mut out = data.to_vec();
            let gray = if header.little {
                1u16.to_le_bytes()
            } else {
                1u16.to_be_bytes()
            };
            out.get_mut(value..value + 2)?.copy_from_slice(&gray);
            return Some(out);
        }
    }
    None
}

const PHOTOMETRIC: u16 = 262;

/// TIFF（BigTIFF を含む）の、IFD をたどるのに要る情報。
struct Header {
    little: bool,
    big: bool,
}

impl Header {
    fn read(data: &[u8]) -> Option<Self> {
        let little = match data.get(0..2)? {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let mut h = Header { little, big: false };
        h.big = match h.u16(data, 2)? {
            42 => false,
            43 => true,
            _ => return None,
        };
        Some(h)
    }

    fn first_ifd(&self, data: &[u8]) -> Option<usize> {
        if self.big {
            self.u64(data, 8)
        } else {
            self.u32(data, 4)
        }
    }

    fn entry_count(&self, data: &[u8], ifd: usize) -> Option<usize> {
        if self.big {
            self.u64(data, ifd)
        } else {
            self.u16(data, ifd).map(usize::from)
        }
    }

    fn next_ifd(&self, data: &[u8], ifd: usize) -> Option<usize> {
        let after = ifd + self.count_size() + self.entry_count(data, ifd)? * self.entry_size();
        let next = if self.big {
            self.u64(data, after)?
        } else {
            self.u32(data, after)?
        };
        (next != 0).then_some(next)
    }

    fn count_size(&self) -> usize {
        if self.big { 8 } else { 2 }
    }

    fn entry_size(&self) -> usize {
        if self.big { 20 } else { 12 }
    }

    /// エントリの先頭から値（4 バイトか 8 バイトに収まる値はそこに直接入る）までの距離。
    fn value_offset(&self) -> usize {
        if self.big { 12 } else { 8 }
    }

    fn bytes<const N: usize>(data: &[u8], at: usize) -> Option<[u8; N]> {
        data.get(at..at.checked_add(N)?)?.try_into().ok()
    }

    fn u16(&self, data: &[u8], at: usize) -> Option<u16> {
        let b = Self::bytes(data, at)?;
        Some(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32(&self, data: &[u8], at: usize) -> Option<usize> {
        let b = Self::bytes(data, at)?;
        let v = if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        };
        usize::try_from(v).ok()
    }

    fn u64(&self, data: &[u8], at: usize) -> Option<usize> {
        let b = Self::bytes(data, at)?;
        let v = if self.little {
            u64::from_le_bytes(b)
        } else {
            u64::from_be_bytes(b)
        };
        usize::try_from(v).ok()
    }
}
