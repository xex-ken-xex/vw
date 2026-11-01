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

const IMAGE_WIDTH: u16 = 256;
const IMAGE_LENGTH: u16 = 257;
const BITS_PER_SAMPLE: u16 = 258;
const COMPRESSION: u16 = 259;
const STRIP_OFFSETS: u16 = 273;
const SAMPLES_PER_PIXEL: u16 = 277;
const ROWS_PER_STRIP: u16 = 278;
const STRIP_BYTE_COUNTS: u16 = 279;
const PLANAR_CONFIGURATION: u16 = 284;

/// たどる IFD の上限（lib.rs の MAX_PAGES と同じ意味。輪になっていても止まる）。
const MAX_IFDS: usize = 10_000;

/// StripOffsets はあるのに StripByteCounts がない IFD に、無圧縮として計った大きさを補った写し。
/// 直す IFD がなければ `None`。
///
/// TIFF の仕様では StripByteCounts は必須だが、産業用 CT などの装置が書く無圧縮の TIFF には
/// 省いたものがあり、ほかのビューワは幅 × 高さ × ビット数から補って読む。tiff クレートは
/// 読めないと断るので、ここで IFD を写しの末尾に書き直して渡す。元の IFD を書き換えないのは、
/// エントリを 1 つ足す隙間が IFD の中にないため。圧縮されたものや平面ごとに分かれたものは、
/// 大きさを計算で決められないので補わない。
pub fn with_strip_byte_counts(data: &[u8]) -> Option<Vec<u8>> {
    let header = Header::read(data)?;
    let mut out: Option<Vec<u8>> = None;
    // 今の IFD の位置が書いてある場所（最初はファイル先頭のポインタ）。写しを作ったら写しの中の位置。
    let mut link = if header.big { 8 } else { 4 };
    let mut offset = header.first_ifd(data)?;
    for _ in 0..MAX_IFDS {
        let count = header.entry_count(data, offset)?;
        let after = offset + header.count_size() + count * header.entry_size();
        match header.strip_byte_counts(data, offset, count) {
            Some(counts) => {
                let mut copy = out.take().unwrap_or_else(|| data.to_vec());
                let next_link = header.append_ifd(&mut copy, data, offset, count, &counts, link)?;
                link = next_link;
                out = Some(copy);
            }
            None => link = after,
        }
        match header.next_ifd(data, offset) {
            Some(next) => offset = next,
            None => break,
        }
    }
    out
}

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

    /// `ifd` のエントリのうち `tag` の値（SHORT / LONG / LONG8 のどれでも）。なければ `None`。
    fn tag_values(&self, data: &[u8], ifd: usize, count: usize, tag: u16) -> Option<Vec<u64>> {
        (0..count)
            .map(|i| ifd + self.count_size() + i * self.entry_size())
            .find(|&entry| self.u16(data, entry) == Some(tag))
            .and_then(|entry| self.entry_values(data, entry))
    }

    fn entry_values(&self, data: &[u8], entry: usize) -> Option<Vec<u64>> {
        let kind = self.u16(data, entry + 2)?;
        let n = if self.big {
            self.u64(data, entry + 4)?
        } else {
            self.u32(data, entry + 4)?
        };
        let size = match kind {
            3 => 2,
            4 => 4,
            16 => 8,
            _ => return None,
        };
        let inline = if self.big { 8 } else { 4 };
        let at = if n * size <= inline {
            entry + self.value_offset()
        } else if self.big {
            self.u64(data, entry + self.value_offset())?
        } else {
            self.u32(data, entry + self.value_offset())?
        };
        (0..n)
            .map(|i| {
                let p = at + i * size;
                Some(match size {
                    2 => self.u16(data, p)? as u64,
                    4 => self.u32(data, p)? as u64,
                    _ => self.u64(data, p)? as u64,
                })
            })
            .collect()
    }

    /// 無圧縮でストリップごとの大きさが省かれた IFD なら、各ストリップのバイト数。
    /// 補う必要がない、または計算で決められない IFD なら `None`。
    fn strip_byte_counts(&self, data: &[u8], ifd: usize, count: usize) -> Option<Vec<u64>> {
        let tag = |t| self.tag_values(data, ifd, count, t);
        let offsets = tag(STRIP_OFFSETS)?;
        if tag(STRIP_BYTE_COUNTS).is_some()
            || tag(COMPRESSION).is_some_and(|v| v != [1])
            || tag(PLANAR_CONFIGURATION).is_some_and(|v| v != [1])
        {
            return None;
        }
        let width = *tag(IMAGE_WIDTH)?.first()?;
        let height = *tag(IMAGE_LENGTH)?.first()?;
        let bits: u64 = tag(BITS_PER_SAMPLE).map_or(1, |b| b.iter().sum());
        let samples = tag(SAMPLES_PER_PIXEL).map_or(1, |s| s[0]);
        // BitsPerSample が 1 つだけ書かれていてサンプルが複数のときは、全サンプル同じビット数。
        let bits = if tag(BITS_PER_SAMPLE).is_some_and(|b| b.len() == 1) {
            bits * samples
        } else {
            bits
        };
        let rows_per_strip = tag(ROWS_PER_STRIP).map_or(height, |r| r[0]).min(height);
        if width == 0 || height == 0 || rows_per_strip == 0 {
            return None;
        }
        let row_bytes = (width * bits).div_ceil(8);
        let strips = height.div_ceil(rows_per_strip);
        if strips as usize != offsets.len() {
            return None;
        }
        let counts: Vec<u64> = (0..strips)
            .map(|i| (height - i * rows_per_strip).min(rows_per_strip) * row_bytes)
            .collect();
        let fits = offsets.iter().zip(&counts).all(|(o, c)| {
            o.checked_add(*c)
                .is_some_and(|end| end <= data.len() as u64)
        });
        fits.then_some(counts)
    }

    /// `ifd`（`data` の中）に StripByteCounts を足した IFD を `copy` の末尾に書き、`link` の
    /// ポインタをそこへ向ける。次の IFD へのポインタの位置を返す。
    fn append_ifd(
        &self,
        copy: &mut Vec<u8>,
        data: &[u8],
        ifd: usize,
        count: usize,
        counts: &[u64],
        link: usize,
    ) -> Option<usize> {
        if copy.len() % 2 == 1 {
            copy.push(0);
        }
        let new_ifd = copy.len();
        self.write_ptr(copy, link, new_ifd)?;
        let entries: Vec<usize> = (0..count)
            .map(|i| ifd + self.count_size() + i * self.entry_size())
            .collect();
        let inserted_before = entries
            .iter()
            .position(|&e| self.u16(data, e).is_some_and(|t| t > STRIP_BYTE_COUNTS))
            .unwrap_or(count);
        // エントリ数
        if self.big {
            copy.extend(self.bytes_of_u64((count + 1) as u64));
        } else {
            copy.extend(self.bytes_of_u16((count + 1) as u16));
        }
        let inline = if self.big { 8 } else { 4 };
        let values_at = new_ifd
            + self.count_size()
            + (count + 1) * self.entry_size()
            + if self.big { 8 } else { 4 };
        let mut new_entry = Vec::new();
        new_entry.extend(self.bytes_of_u16(STRIP_BYTE_COUNTS));
        let value_bytes: Vec<u8> = if self.big {
            new_entry.extend(self.bytes_of_u16(16));
            new_entry.extend(self.bytes_of_u64(counts.len() as u64));
            counts.iter().flat_map(|c| self.bytes_of_u64(*c)).collect()
        } else {
            new_entry.extend(self.bytes_of_u16(4));
            new_entry.extend(self.bytes_of_u32(counts.len() as u32));
            counts
                .iter()
                .map(|c| u32::try_from(*c).ok())
                .collect::<Option<Vec<u32>>>()?
                .iter()
                .flat_map(|c| self.bytes_of_u32(*c))
                .collect()
        };
        let mut tail = Vec::new();
        if value_bytes.len() <= inline {
            let mut v = value_bytes;
            v.resize(inline, 0);
            new_entry.extend(v);
        } else {
            if self.big {
                new_entry.extend(self.bytes_of_u64(values_at as u64));
            } else {
                new_entry.extend(self.bytes_of_u32(u32::try_from(values_at).ok()?));
            }
            tail = value_bytes;
        }
        for (i, &e) in entries.iter().enumerate() {
            if i == inserted_before {
                copy.extend(&new_entry);
            }
            copy.extend(data.get(e..e + self.entry_size())?);
        }
        if inserted_before == count {
            copy.extend(&new_entry);
        }
        let next_link = copy.len();
        let after = ifd + self.count_size() + count * self.entry_size();
        let ptr_size = if self.big { 8 } else { 4 };
        copy.extend(data.get(after..after + ptr_size)?);
        copy.extend(tail);
        Some(next_link)
    }

    fn write_ptr(&self, copy: &mut [u8], at: usize, value: usize) -> Option<()> {
        if self.big {
            copy.get_mut(at..at + 8)?
                .copy_from_slice(&self.bytes_of_u64(value as u64));
        } else {
            copy.get_mut(at..at + 4)?
                .copy_from_slice(&self.bytes_of_u32(u32::try_from(value).ok()?));
        }
        Some(())
    }

    fn bytes_of_u16(&self, v: u16) -> [u8; 2] {
        if self.little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    }

    fn bytes_of_u32(&self, v: u32) -> [u8; 4] {
        if self.little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    }

    fn bytes_of_u64(&self, v: u64) -> [u8; 8] {
        if self.little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
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
