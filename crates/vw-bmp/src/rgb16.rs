//! 16bit の BMP（既定の 5-5-5 と、ビットフィールドで指定する 5-6-5 など）を読む。
//!
//! zune-bmp の 16bit の読み込みは、行の詰め物の計算に幅ではなく高さを使っていて、
//! 正方形でない画像の行がずれる。

use crate::header::{Header, BI_RGB};
use crate::MAX_SIDE;
use vw_core::DecodeError;

/// 上の行から並べた RGB の画素。16bit の BMP のアルファ（1-5-5-5 など）は使うソフトがほぼないので読まない。
pub fn decode(data: &[u8], header: &Header) -> Result<Vec<u8>, DecodeError> {
    let (w, h) = (
        header.width.unsigned_abs() as usize,
        header.height.unsigned_abs() as usize,
    );
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(DecodeError::Invalid(format!(
            "BMP size {} x {}",
            header.width, header.height
        )));
    }
    let masks = if header.compression == BI_RGB {
        [0x7c00, 0x03e0, 0x001f]
    } else {
        header.masks(data)?
    };
    let channels = masks.map(Channel::new);
    let stride = (w * 2).div_ceil(4) * 4;
    let rows = data
        .get(header.offset..)
        .filter(|d| d.len() >= stride * h)
        .ok_or_else(|| DecodeError::Invalid("BMP pixel data is cut short".into()))?;
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let row = if header.top_down() { y } else { h - 1 - y };
        let row = &rows[row * stride..row * stride + w * 2];
        for p in row.chunks_exact(2) {
            let v = u32::from(u16::from_le_bytes([p[0], p[1]]));
            rgb.extend(channels.iter().map(|c| c.expand(v)));
        }
    }
    Ok(rgb)
}

/// ビットマスクで切り出す 1 色。
struct Channel {
    mask: u32,
    shift: u32,
    max: u32,
}

impl Channel {
    fn new(mask: u32) -> Self {
        let shift = mask.trailing_zeros().min(31);
        Self {
            mask,
            shift,
            max: mask >> shift,
        }
    }

    /// 0〜255 に引き伸ばす。
    fn expand(&self, v: u32) -> u8 {
        if self.max == 0 {
            return 0;
        }
        (((v & self.mask) >> self.shift) * 255 / self.max) as u8
    }
}
