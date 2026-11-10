//! SGI の LogLuv・LogL（明るさを対数で持つ HDR の TIFF）。tiff クレートはこの圧縮を読まない。
//!
//! 明るさの幅を残すため、線形の実数（RGB かグレー）で返す。表示は実数の画像と同じく値の幅に合わせる。
//! 色の値を 1 バイトに詰めた LogLuv24（圧縮 34677）は、色の符号の表（libtiff の uvcode.h）が要るので読まない。
//! LogLuv32 が書き手の既定で、24 を書くものはほぼないため。

use std::borrow::Cow;
use std::io::Cursor;

use tiff::decoder::Decoder;
use tiff::tags::Tag;
use vw_core::{ColorModel, DecodeError, PixelData, VwImage};

use crate::decompress::Shape;
use crate::layout;

const LOG_L: u64 = 32844;
const LOG_LUV: u64 = 32845;
const SGILOG: u32 = 34676;
const SGILOG24: u32 = 34677;
const BLACK_IS_ZERO: u16 = 1;

/// tiff クレートは LogL・LogLuv の PhotometricInterpretation を見るとファイルを開くところで断るので、
/// その IFD をグレーと書き換えた写しにする（ページを数えることやタグを読むことはクレートに任せるため）。
/// どのページかは圧縮の方式（SGILog はこの色の形にしか使わない）で見分ける。
pub fn hidden_from_the_crate(data: Cow<'_, [u8]>) -> Cow<'_, [u8]> {
    let mut data = data;
    for index in 0..10_000 {
        let Some(found) = layout::ifd(&data, index) else {
            break;
        };
        if !matches!(found.value(262), Some(LOG_L | LOG_LUV)) {
            continue;
        }
        if let Some(copy) = layout::with_photometric(&data, index, BLACK_IS_ZERO) {
            data = Cow::Owned(copy);
        }
    }
    data
}

/// 今のページが SGILog の圧縮か。
pub fn is_sgilog(decoder: &mut Decoder<Cursor<&[u8]>>) -> bool {
    matches!(
        decoder
            .find_tag_unsigned::<u32>(Tag::Compression)
            .ok()
            .flatten(),
        Some(SGILOG | SGILOG24)
    )
}

/// `ifd` 番目のページ（SGILog の圧縮）の画素。サンプルが 3 つなら LogLuv、1 つなら LogL。
pub fn pixels(data: &[u8], ifd: usize) -> Result<VwImage, DecodeError> {
    let found = layout::ifd(data, ifd).ok_or_else(|| DecodeError::Invalid("broken IFD".into()))?;
    if found.value(259) == Some(u64::from(SGILOG24)) {
        return Err(DecodeError::Unsupported("TIFF SGILog24 compression".into()));
    }
    let color = found.value(277) == Some(3);
    if found.value(284) == Some(2) {
        return Err(DecodeError::Unsupported(
            "TIFF LogLuv in separate planes".into(),
        ));
    }
    let shape = Shape::read(&found)?;
    let channels = if color { 3 } else { 1 };
    vw_core::limits::check_image(shape.width as u64, shape.height as u64, channels * 4)?;
    let mut out = vec![0f32; shape.width * shape.height * channels as usize];
    let planes = if color { 4 } else { 2 };
    let mut row = Vec::new();
    for i in 0..shape.offsets.len() {
        let mut input = shape.input(data, i)?;
        let (x0, y0) = shape.origin(i);
        let (w, rows) = shape.chunk_size(i);
        for r in 0..rows {
            input = unpack_row(input, w, planes, &mut row)?;
            let y = y0 + r;
            if y >= shape.height {
                continue;
            }
            for (x, &v) in row.iter().enumerate().take(shape.width.saturating_sub(x0)) {
                let at = (y * shape.width + x0 + x) * channels as usize;
                if color {
                    out[at..at + 3].copy_from_slice(&rgb_of_luv32(v));
                } else {
                    out[at] = y_of_log_l16(v as u16);
                }
            }
        }
    }
    let model = if color {
        ColorModel::Rgb
    } else {
        ColorModel::Gray
    };
    VwImage::new(
        shape.width as u32,
        shape.height as u32,
        model,
        PixelData::F32(out),
    )
    .map_err(DecodeError::Invalid)
}

/// 1 行（`width` 画素）をほどき、残りの入力を返す。値は上位バイトの面から順に、面ごとに連長で符号化してある
/// （128 以上の 1 バイトは「次の 1 バイトを（値 − 126）回」、それより小さい値は「続く n バイトをそのまま」）。
fn unpack_row<'a>(
    mut input: &'a [u8],
    width: usize,
    planes: u32,
    row: &mut Vec<u32>,
) -> Result<&'a [u8], DecodeError> {
    let short = || DecodeError::Invalid("TIFF LogLuv row is shorter than its width".into());
    row.clear();
    row.resize(width, 0);
    for plane in (0..planes).rev() {
        let shift = plane * 8;
        let mut i = 0;
        while i < width {
            let (&head, rest) = input.split_first().ok_or_else(short)?;
            input = rest;
            if head >= 128 {
                let (&value, rest) = input.split_first().ok_or_else(short)?;
                input = rest;
                let n = usize::from(head) - 126;
                for v in row.iter_mut().skip(i).take(n) {
                    *v |= u32::from(value) << shift;
                }
                i += n;
            } else {
                let n = usize::from(head);
                let literal = input.get(..n).ok_or_else(short)?;
                input = &input[n..];
                for (v, &b) in row.iter_mut().skip(i).zip(literal) {
                    *v |= u32::from(b) << shift;
                }
                i += n;
            }
        }
    }
    Ok(input)
}

/// LogL16 の値を、明るさ Y（cd/m² の尺度のまま）にする。上位 1 ビットは符号。
fn y_of_log_l16(p: u16) -> f32 {
    let le = p & 0x7fff;
    if le == 0 {
        return 0.0;
    }
    let y = (std::f64::consts::LN_2 / 256.0 * (f64::from(le) + 0.5) - std::f64::consts::LN_2 * 64.0)
        .exp() as f32;
    if p & 0x8000 != 0 { -y } else { y }
}

/// LogLuv32（上位 16 ビットが LogL16、続く 8 ビットずつが u′・v′）を、線形の RGB にする。
/// XYZ から RGB への行列は libtiff の LogLuv の読み込みと同じもの（白は等エネルギー白色）。
fn rgb_of_luv32(p: u32) -> [f32; 3] {
    let l = y_of_log_l16((p >> 16) as u16);
    if l <= 0.0 {
        return [0.0; 3];
    }
    const UV_SCALE: f32 = 410.0;
    let u = (((p >> 8) & 0xff) as f32 + 0.5) / UV_SCALE;
    let v = ((p & 0xff) as f32 + 0.5) / UV_SCALE;
    let s = 1.0 / (6.0 * u - 16.0 * v + 12.0);
    let (x, y) = (9.0 * u * s, 4.0 * v * s);
    let (cx, cy, cz) = (x / y * l, l, (1.0 - x - y) / y * l);
    [
        2.690 * cx - 1.276 * cy - 0.414 * cz,
        -1.022 * cx + 1.978 * cy + 0.044 * cz,
        0.061 * cx - 0.224 * cy + 1.163 * cz,
    ]
    .map(|c| c.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_l16_doubles_the_luminance_every_256_steps() {
        let a = y_of_log_l16(64 * 256);
        let b = y_of_log_l16(65 * 256);
        assert!((a - 1.0).abs() < 0.01, "{a}");
        assert!((b / a - 2.0).abs() < 1e-4, "{b}");
    }

    #[test]
    fn rows_mix_runs_and_literals_per_byte_plane() {
        // 上位の面: 4 画素とも 0x12（連長 130 = 4 回）。下位の面: そのまま 4 バイト。
        let input = [130, 0x12, 4, 1, 2, 3, 4, 0xEE];
        let mut row = Vec::new();
        let rest = unpack_row(&input, 4, 2, &mut row).unwrap();
        assert_eq!(row, [0x1201, 0x1202, 0x1203, 0x1204]);
        assert_eq!(rest, [0xEE]);
    }
}
