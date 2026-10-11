//! 4bit の RLE（BI_RLE4）を読む。
//!
//! zune-bmp にも RLE4 の読み込みはあるが、最後の行の「行の終わり」のすぐ後に「画像の終わり」が
//! 来るファイル（Windows のペイントなど多くのソフトが書く形）を、画像の外へ出たとしてエラーにする。
//! RLE8 にはこの問題がないので、RLE4 だけをここで読む。

use crate::header::Header;
use crate::MAX_SIDE;
use vw_core::DecodeError;

/// 上の行から並べた RGB の画素。
pub fn decode(data: &[u8], header: &Header) -> Result<Vec<u8>, DecodeError> {
    // RLE は下の行から並べると決まっていて、上下逆（高さが負）は仕様にない。
    let (width, height) = (header.width, header.height);
    if width <= 0 || height <= 0 || width as usize > MAX_SIDE || height as usize > MAX_SIDE {
        return Err(DecodeError::Invalid(format!(
            "RLE4 BMP size {width} x {height}"
        )));
    }
    let (w, h) = (width as usize, height as usize);
    let colors = match header.colors_used {
        0 => 16,
        n => (n as usize).min(16),
    };
    let palette_at = 14 + header.info_size as usize;
    let palette: Vec<[u8; 3]> = (0..colors)
        .map(|i| {
            let p = palette_at + i * 4;
            data.get(p..p + 3)
                .map(|b| [b[2], b[1], b[0]])
                .ok_or_else(|| DecodeError::Invalid("BMP palette is cut short".into()))
        })
        .collect::<Result<_, _>>()?;

    let mut stream = data
        .get(header.offset..)
        .ok_or_else(|| DecodeError::Invalid("BMP pixel data is missing".into()))?
        .iter()
        .copied();
    let mut next = || {
        stream
            .next()
            .ok_or_else(|| DecodeError::Invalid("RLE4 data is cut short".into()))
    };
    let mut indices = vec![0u8; w * h];
    let (mut x, mut y) = (0usize, 0usize);
    let mut put = |x: usize, y: usize, v: u8| {
        if x < w && y < h {
            indices[(h - 1 - y) * w + x] = v;
        }
    };
    while y < h {
        let count = next()?;
        if count > 0 {
            let pair = next()?;
            for i in 0..count {
                put(x, y, if i % 2 == 0 { pair >> 4 } else { pair & 0x0f });
                x += 1;
            }
            continue;
        }
        match next()? {
            0 => {
                x = 0;
                y += 1;
            }
            1 => break,
            2 => {
                x += usize::from(next()?);
                y += usize::from(next()?);
            }
            literal => {
                let bytes = usize::from(literal).div_ceil(2);
                for i in 0..bytes {
                    let pair = next()?;
                    put(x, y, pair >> 4);
                    if 2 * i + 1 < usize::from(literal) {
                        put(x + 1, y, pair & 0x0f);
                    }
                    x += 2;
                }
                x -= bytes * 2 - usize::from(literal);
                // 生の画素の並びは 2 バイト境界までゼロで詰めてある。
                if bytes % 2 == 1 {
                    next()?;
                }
            }
        }
    }
    Ok(indices
        .iter()
        .flat_map(|&i| palette.get(usize::from(i)).copied().unwrap_or([0; 3]))
        .collect())
}
