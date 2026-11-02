//! 1 / 2 / 4bit のパレットの BMP（無圧縮）を読む。
//!
//! zune-bmp はこれらの行を読むとき、行の末尾の 4 バイト境界までの詰め物を飛ばさない。
//! そのため、1 行のバイト数が 4 の倍数でない幅では 2 行目から読み始めがずれ、画像が斜めに流れる。

use crate::MAX_SIDE;
use crate::header::Header;
use vw_core::DecodeError;

/// 上の行から並べた RGB の画素。
pub fn decode(data: &[u8], header: &Header) -> Result<Vec<u8>, DecodeError> {
    let bits = usize::from(header.bits);
    let (width, height) = (header.width, header.height);
    if width <= 0
        || height == 0
        || width as usize > MAX_SIDE
        || height.unsigned_abs() as usize > MAX_SIDE
    {
        return Err(DecodeError::Invalid(format!("BMP size {width} x {height}")));
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let palette = palette(data, header, 1 << bits)?;
    let stride = (w * bits).div_ceil(32) * 4;
    let rows = data
        .get(header.offset..)
        .filter(|rows| rows.len() >= stride * (h - 1) + (w * bits).div_ceil(8))
        .ok_or_else(|| DecodeError::Invalid("BMP pixel data is cut short".into()))?;
    let per_byte = 8 / bits;
    let mask = (1u8 << bits) - 1;
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        // 下の行から並んでいるのがふつうで、高さが負なら上の行から。
        let row = if header.top_down() { y } else { h - 1 - y };
        let line = &rows[row * stride..];
        for x in 0..w {
            let byte = line[x / per_byte];
            let shift = 8 - bits * (x % per_byte + 1);
            let index = usize::from((byte >> shift) & mask);
            // パレットの外を指す画素は、ほかのソフトと同じく黒にする。
            rgb.extend_from_slice(palette.get(index).unwrap_or(&[0, 0, 0]));
        }
    }
    Ok(rgb)
}

/// パレット。OS/2 1.x のヘッダでは 1 色 3 バイト、ほかは 4 バイト（BGR と予備）。
fn palette(data: &[u8], header: &Header, max: usize) -> Result<Vec<[u8; 3]>, DecodeError> {
    let colors = match header.colors_used {
        0 => max,
        n => (n as usize).min(max),
    };
    let entry = if header.info_size == 12 { 3 } else { 4 };
    let at = 14 + header.info_size as usize;
    (0..colors)
        .map(|i| {
            let p = at + i * entry;
            data.get(p..p + 3)
                .map(|b| [b[2], b[1], b[0]])
                .ok_or_else(|| DecodeError::Invalid("BMP palette is cut short".into()))
        })
        .collect()
}
