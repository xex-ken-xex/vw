//! dicom-pixeldata が RGB にしてくれないカラーの形を、vw の側で RGB にする。
//!
//! - PALETTE COLOR: 画素は番号で、Red / Green / Blue の Palette Color LUT で色を引く
//!   （PS3.3 C.7.6.3.1.5〜6、Segmented は C.7.9.2）
//! - 非圧縮の YBR_FULL_422: 横に並んだ 2 画素が Cb・Cr を共有し、Y1 Y2 Cb Cr の 4 サンプルで並ぶ
//!   （PS3.3 C.7.6.3.1.2）。dicom-pixeldata は 3 サンプル / 画素として数えてしまい読めない
//! - 16bit の RGB・YBR_FULL: U16 の RGB で持つ

use dicom_core::Tag;
use dicom_core::value::PrimitiveValue;
use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;
use vw_core::{ColorModel, DecodeError, PixelData, VwImage};

/// 1 色分の Palette Color LUT。
#[derive(Debug, Clone, PartialEq)]
struct Lut {
    info: LutInfo,
    values: Vec<u16>,
}

/// LUT の記述子と、Segmented かどうか。画素の変換の図に出す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LutInfo {
    /// 項目数。記述子の 0 は 65536 を表す。
    pub entries: u32,
    /// 表の先頭が受け持つ画素の値。これより小さい値は先頭の色になる。
    pub first: i64,
    /// 1 項目のビット数（8 か 16）。
    pub bits: u16,
    pub segmented: bool,
}

/// PALETTE COLOR の画像の、3 色分の LUT。
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    red: Lut,
    green: Lut,
    blue: Lut,
}

impl Palette {
    pub fn read(obj: &InMemDicomObject) -> Result<Self, DecodeError> {
        Ok(Self {
            red: Lut::read(
                obj,
                tags::RED_PALETTE_COLOR_LOOKUP_TABLE_DESCRIPTOR,
                tags::RED_PALETTE_COLOR_LOOKUP_TABLE_DATA,
                tags::SEGMENTED_RED_PALETTE_COLOR_LOOKUP_TABLE_DATA,
            )?,
            green: Lut::read(
                obj,
                tags::GREEN_PALETTE_COLOR_LOOKUP_TABLE_DESCRIPTOR,
                tags::GREEN_PALETTE_COLOR_LOOKUP_TABLE_DATA,
                tags::SEGMENTED_GREEN_PALETTE_COLOR_LOOKUP_TABLE_DATA,
            )?,
            blue: Lut::read(
                obj,
                tags::BLUE_PALETTE_COLOR_LOOKUP_TABLE_DESCRIPTOR,
                tags::BLUE_PALETTE_COLOR_LOOKUP_TABLE_DATA,
                tags::SEGMENTED_BLUE_PALETTE_COLOR_LOOKUP_TABLE_DATA,
            )?,
        })
    }

    /// Red・Green・Blue の順。
    pub fn info(&self) -> [LutInfo; 3] {
        [self.red.info, self.green.info, self.blue.info]
    }

    /// どれかの色が 16bit の表なら、U16 の RGB で持つ。
    pub fn is_deep(&self) -> bool {
        self.info().iter().any(|l| l.bits > 8)
    }

    /// 画素の値（番号）を RGB の画像にする。
    pub fn apply(&self, columns: u32, rows: u32, values: &[i64]) -> Result<VwImage, DecodeError> {
        let luts = [&self.red, &self.green, &self.blue];
        let data = if self.is_deep() {
            // 8bit と 16bit の表が混ざっていたら、8bit の方を 16bit の幅に広げて揃える。
            let widen = |l: &Lut, v: u16| if l.info.bits > 8 { v } else { v * 257 };
            PixelData::U16(
                values
                    .iter()
                    .flat_map(|&v| luts.map(|l| widen(l, l.lookup(v))))
                    .collect(),
            )
        } else {
            PixelData::U8(
                values
                    .iter()
                    .flat_map(|&v| luts.map(|l| l.lookup(v) as u8))
                    .collect(),
            )
        };
        VwImage::new(columns, rows, ColorModel::Rgb, data).map_err(DecodeError::Invalid)
    }
}

impl Lut {
    fn read(
        obj: &InMemDicomObject,
        descriptor: Tag,
        data: Tag,
        segmented: Tag,
    ) -> Result<Self, DecodeError> {
        let missing = |what: &str| DecodeError::Invalid(format!("palette color LUT: no {what}"));
        let d: Vec<i64> = obj
            .get(descriptor)
            .and_then(|e| e.to_multi_int().ok())
            .filter(|d: &Vec<i64>| d.len() >= 3)
            .ok_or_else(|| missing(&format!("descriptor {descriptor}")))?;
        // 項目数は US か SS で書かれる。SS で 32768 以上を書くと負になるので、16bit として読み直す。
        let entries = match d[0] as u16 {
            0 => 65_536,
            n => u32::from(n),
        };
        let bits = d[2] as u16;
        if bits != 8 && bits != 16 {
            return Err(DecodeError::Unsupported(format!(
                "palette color LUT with {bits}-bit entries"
            )));
        }
        let (words, is_segmented) = match (obj.get(data), obj.get(segmented)) {
            (Some(e), _) => (words(e.value().primitive()), false),
            (None, Some(e)) => (words(e.value().primitive()), true),
            (None, None) => return Err(missing(&format!("data {data}"))),
        };
        let mut values = if is_segmented {
            expand_segments(&words, entries as usize)?
        } else if bits == 8 {
            eight_bit_entries(&words, entries as usize)
        } else {
            words
        };
        values.truncate(entries as usize);
        if values.is_empty() {
            return Err(DecodeError::Invalid(format!(
                "palette color LUT {data} is empty"
            )));
        }
        Ok(Self {
            info: LutInfo {
                entries,
                first: d[1],
                bits,
                segmented: is_segmented,
            },
            values,
        })
    }

    /// 表の外の値は、規格どおり端の項目の色にする。
    fn lookup(&self, value: i64) -> u16 {
        let i = (value - self.info.first).clamp(0, self.values.len() as i64 - 1);
        self.values[i as usize]
    }
}

/// OW の値を 16bit の語の並びにする。読み込み時の VR によって、U16 でなく U8 のまま
/// 持っていることがある（暗黙 VR で辞書が OB/OW を決められないときなど）。
fn words(value: Option<&PrimitiveValue>) -> Vec<u16> {
    match value {
        Some(PrimitiveValue::U16(v)) => v.to_vec(),
        Some(other) => other
            .to_bytes()
            .chunks(2)
            .map(|b| u16::from_le_bytes([b[0], b.get(1).copied().unwrap_or(0)]))
            .collect(),
        None => Vec::new(),
    }
}

/// 8bit の表。規格は 2 項目を 1 語に詰める（下位バイトが先の項目）が、1 語に 1 項目を
/// 入れて書くファイルも多いので、語の数が項目数と同じならそちらとみなす。
/// 1 語に 1 項目のファイルでは、値を上位バイトに置くもの（16bit の表の上位だけを使う作り）も
/// あるので、下位バイトに収まらない値があれば上位バイトを使う。
fn eight_bit_entries(words: &[u16], entries: usize) -> Vec<u16> {
    if words.len() >= entries {
        let shift = if words.iter().any(|&w| w > 0xFF) {
            8
        } else {
            0
        };
        words.iter().map(|&w| (w >> shift) & 0xFF).collect()
    } else {
        words
            .iter()
            .flat_map(|w| w.to_le_bytes().map(u16::from))
            .collect()
    }
}

/// Segmented Palette Color LUT（PS3.3 C.7.9.2）を、項目の並びに広げる。
fn expand_segments(words: &[u16], entries: usize) -> Result<Vec<u16>, DecodeError> {
    let mut out = Vec::with_capacity(entries);
    expand_segments_into(words, 0, None, entries, 0, &mut out)?;
    Ok(out)
}

/// 間接の段は前の段を指すので、壊れたファイルで指し合って終わらないように深さを限る。
const MAX_INDIRECTION: usize = 8;

/// `words[start..]` の段を、`count` 段（`None` なら最後まで）広げる。
fn expand_segments_into(
    words: &[u16],
    start: usize,
    count: Option<usize>,
    entries: usize,
    depth: usize,
    out: &mut Vec<u16>,
) -> Result<(), DecodeError> {
    let broken = |why: &str| DecodeError::Invalid(format!("segmented palette color LUT: {why}"));
    if depth > MAX_INDIRECTION {
        return Err(broken("indirect segments nest too deep"));
    }
    let mut at = start;
    let mut done = 0;
    while at + 1 < words.len() && count.is_none_or(|c| done < c) && out.len() < entries {
        let (opcode, length) = (words[at], words[at + 1] as usize);
        at += 2;
        match opcode {
            0 => {
                let end = (at + length).min(words.len());
                out.extend_from_slice(&words[at..end]);
                at = end;
            }
            1 => {
                let y0 = *out.last().ok_or_else(|| broken("linear segment first"))?;
                let y1 = *words.get(at).ok_or_else(|| broken("linear segment cut"))?;
                at += 1;
                let step = (f64::from(y1) - f64::from(y0)) / length.max(1) as f64;
                out.extend((1..=length).map(|i| (f64::from(y0) + step * i as f64).round() as u16));
            }
            2 => {
                let (lo, hi) = match words.get(at..at + 2) {
                    Some(&[lo, hi]) => (lo, hi),
                    _ => return Err(broken("indirect segment cut")),
                };
                at += 2;
                // オフセットは、間接の段の中でも常にデータ全体の先頭からのバイト数。
                let from = (usize::from(lo) | usize::from(hi) << 16) / 2;
                if from >= words.len() {
                    return Err(broken("indirect segment points outside"));
                }
                expand_segments_into(words, from, Some(length), entries, depth + 1, out)?;
            }
            n => return Err(broken(&format!("unknown segment type {n}"))),
        }
        done += 1;
    }
    out.truncate(entries);
    Ok(())
}

/// 非圧縮の YBR_FULL_422 の 1 フレーム（Y1 Y2 Cb Cr の並び）を、1 画素 3 サンプルの
/// YBR_FULL の並びに広げる。`sample` は 1 サンプルのバイト数。
pub fn expand_422(
    raw: &[u8],
    columns: usize,
    rows: usize,
    sample: usize,
) -> Result<Vec<u8>, DecodeError> {
    let pairs = columns.div_ceil(2);
    let row_bytes = pairs * 4 * sample;
    if raw.len() < row_bytes * rows {
        return Err(DecodeError::Invalid(format!(
            "YBR_FULL_422 frame has {} bytes, expected {}",
            raw.len(),
            row_bytes * rows
        )));
    }
    let mut out = Vec::with_capacity(columns * rows * 3 * sample);
    for row in raw[..row_bytes * rows].chunks_exact(row_bytes) {
        for (pair, group) in row.chunks_exact(4 * sample).enumerate() {
            let [y1, y2, cb, cr] = [0, 1, 2, 3].map(|i| &group[i * sample..(i + 1) * sample]);
            out.extend([y1, cb, cr].concat());
            // 列数が奇数なら、最後の組の 2 つ目の Y は画像の外。
            if pair * 2 + 1 < columns {
                out.extend([y2, cb, cr].concat());
            }
        }
    }
    Ok(out)
}

/// 1 画素 3 サンプルが並んだ（Planar Configuration 0 の）カラーの画素を [`VwImage`] にする。
/// 8bit は U8、16bit は Bits Stored を有効ビットにした U16 の RGB。
pub fn interleaved_image(
    columns: u32,
    rows: u32,
    raw: &[u8],
    allocated: u16,
    stored: u16,
    ybr: bool,
) -> Result<VwImage, DecodeError> {
    let samples = columns as usize * rows as usize * 3;
    let (data, significant_bits) = match allocated {
        8 => {
            let mut v = raw
                .get(..samples)
                .ok_or_else(|| DecodeError::Invalid("pixel data is too short".into()))?
                .to_vec();
            if ybr {
                ybr8_to_rgb(&mut v);
            }
            (PixelData::U8(v), 8)
        }
        16 => {
            let stored = stored.clamp(1, 16);
            let mask = (1u32 << stored) - 1;
            let mut v: Vec<u16> = raw
                .get(..samples * 2)
                .ok_or_else(|| DecodeError::Invalid("pixel data is too short".into()))?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&b| (u32::from(u16::from_le_bytes(b)) & mask) as u16)
                .collect();
            if ybr {
                ybr16_to_rgb(&mut v, stored);
            }
            (PixelData::U16(v), u32::from(stored))
        }
        n => return Err(DecodeError::Unsupported(format!("{n}-bit color"))),
    };
    let mut img =
        VwImage::new(columns, rows, ColorModel::Rgb, data).map_err(DecodeError::Invalid)?;
    img.significant_bits = significant_bits;
    Ok(img)
}

/// Planar Configuration 1（R の面、G の面、B の面）を、画素ごとの並びに直す。
pub fn interleave_planes(raw: &[u8], pixels: usize, sample: usize) -> Vec<u8> {
    let mut out = vec![0; pixels * 3 * sample];
    for (i, dst) in out.chunks_exact_mut(sample).enumerate() {
        let src = ((i % 3) * pixels + i / 3) * sample;
        if let Some(s) = raw.get(src..src + sample) {
            dst.copy_from_slice(s);
        }
    }
    out
}

/// YCbCr（PS3.3 C.7.6.3.1.2 の YBR_FULL。JPEG と同じ式）を RGB に直す。
fn ybr8_to_rgb(px: &mut [u8]) {
    for p in px.as_chunks_mut::<3>().0 {
        let [r, g, b] = ybr_to_rgb([p[0], p[1], p[2]].map(f32::from), 128.0);
        *p = [r, g, b].map(|v| v.round().clamp(0.0, 255.0) as u8);
    }
}

/// 16bit の YBR_FULL。Cb・Cr の中心は Bits Stored の半分。
fn ybr16_to_rgb(px: &mut [u16], stored: u16) {
    let max = ((1u32 << stored) - 1) as f32;
    let center = (1u32 << (stored - 1)) as f32;
    for p in px.as_chunks_mut::<3>().0 {
        let [r, g, b] = ybr_to_rgb([p[0], p[1], p[2]].map(f32::from), center);
        *p = [r, g, b].map(|v| v.round().clamp(0.0, max) as u16);
    }
}

fn ybr_to_rgb([y, cb, cr]: [f32; 3], center: f32) -> [f32; 3] {
    let (cb, cr) = (cb - center, cr - center);
    [
        y + 1.402 * cr,
        y - 0.344136 * cb - 0.714136 * cr,
        y + 1.772 * cb,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lut(first: i64, bits: u16, values: Vec<u16>) -> Lut {
        Lut {
            info: LutInfo {
                entries: values.len() as u32,
                first,
                bits,
                segmented: false,
            },
            values,
        }
    }

    #[test]
    fn values_outside_the_table_take_the_color_of_its_ends() {
        let l = lut(10, 8, vec![1, 2, 3]);
        assert_eq!([0, 10, 11, 12, 99].map(|v| l.lookup(v)), [1, 1, 2, 3, 3]);
    }

    #[test]
    fn eight_bit_entries_packed_two_per_word_are_unpacked_low_byte_first() {
        assert_eq!(eight_bit_entries(&[0x0201, 0x0403], 4), [1, 2, 3, 4]);
    }

    #[test]
    fn eight_bit_entries_written_one_per_word_are_read_from_whichever_byte_holds_them() {
        assert_eq!(eight_bit_entries(&[1, 2, 255], 3), [1, 2, 255]);
        assert_eq!(eight_bit_entries(&[0x0100, 0x0200, 0xFF00], 3), [1, 2, 255]);
    }

    #[test]
    fn segmented_lut_expands_discrete_linear_and_indirect_segments() {
        let words = [
            0, 2, 10, 20, // 離散: 10, 20
            1, 4, 60, // 直線: 20 から 60 へ 4 項目
            2, 2, 0, 0, // 間接: 先頭（バイト 0）から 2 段をもう一度
        ];
        let out = expand_segments(&words, 100).unwrap();
        assert_eq!(out, [10, 20, 30, 40, 50, 60, 10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn segmented_lut_stops_at_the_declared_number_of_entries() {
        let out = expand_segments(&[0, 4, 1, 2, 3, 4], 3).unwrap();
        assert_eq!(out, [1, 2, 3]);
    }

    #[test]
    fn segmented_lut_that_points_at_itself_forever_is_an_error() {
        assert!(expand_segments(&[2, 1, 0, 0], 1000).is_err());
    }

    #[test]
    fn ybr_422_pairs_share_their_chroma() {
        // 3 列 x 1 行: 組 (Y 10, 20 / Cb 1 / Cr 2) と、2 つ目の Y が画像の外の組 (30, 0 / 3 / 4)。
        let raw = [10, 20, 1, 2, 30, 0, 3, 4];
        let out = expand_422(&raw, 3, 1, 1).unwrap();
        assert_eq!(out, [10, 1, 2, 20, 1, 2, 30, 3, 4]);
    }

    #[test]
    fn sixteen_bit_ybr_is_centered_on_half_of_bits_stored() {
        let gray = [3000u16, 2048, 2048];
        let raw: Vec<u8> = gray.iter().flat_map(|v| v.to_le_bytes()).collect();
        let img = interleaved_image(1, 1, &raw, 16, 12, true).unwrap();
        assert_eq!(img.significant_bits, 12);
        assert!(matches!(&img.data, PixelData::U16(v) if v == &[3000, 3000, 3000]));
    }
}
