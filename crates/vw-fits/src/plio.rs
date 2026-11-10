//! PLIO_1 の tile の展開（IRAF のピクセルリスト。マスクのような、同じ値が続く整数の画像向け）。
//!
//! 手順は CFITSIO の pl_l2pi（pliocomp.c、IRAF の PLIO を移したもの）と同じにしてある。
//! 告知は tile.rs の頭に載せたもの（CFITSIO）と同じ。

use vw_core::DecodeError;

fn invalid(what: &str) -> DecodeError {
    DecodeError::Invalid(format!("FITS PLIO tile: {what}"))
}

/// 1 つの tile（`pixels` 個）。`words` は列の中身の 16bit の並び（上位バイトが先）。
pub fn decode(words: &[u8], pixels: usize) -> Result<Vec<i64>, DecodeError> {
    let ll: Vec<i64> = words
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| i64::from(i16::from_be_bytes(*w)))
        .collect();
    // 以下の番号は元の手順と同じく 1 から数える。
    let at = |i: usize| {
        ll.get(i - 1)
            .copied()
            .ok_or_else(|| invalid("header is short"))
    };
    let (len, first) = if at(3)? > 0 {
        (at(3)? as usize, 4)
    } else {
        (
            ((at(5)? << 15) + at(4)?) as usize,
            usize::try_from(at(2)? + 1).map_err(|_| invalid("broken header"))?,
        )
    };
    let mut out = vec![0i64; pixels];
    if len == 0 || pixels == 0 {
        return Ok(out);
    }
    let len = len.min(ll.len());
    let (xs, xe) = (1i64, pixels as i64);
    let mut op = 0usize;
    let mut x1: i64 = 1;
    let mut pv: i64 = 1;
    let mut ip = first;
    while ip <= len {
        let word = at(ip)?;
        let (opcode, data) = (word / 4096, word & 4095);
        match opcode {
            // 0: 0 が data 個、4: 今の値が data 個、5: 0 が data − 1 個のあとに今の値が 1 個。
            0 | 4 | 5 => {
                let x2 = x1 + data - 1;
                let (i1, i2) = (x1.max(xs), x2.min(xe));
                let np = i2 - i1 + 1;
                if np > 0 {
                    let top = op + np as usize;
                    let fill = if opcode == 4 { pv } else { 0 };
                    out.get_mut(op..top)
                        .ok_or_else(|| invalid("runs past the tile"))?
                        .fill(fill);
                    if opcode == 5 && i2 == x2 {
                        out[top - 1] = pv;
                    }
                    op = top;
                }
                x1 = x2 + 1;
            }
            // 1: 値を 2 語で決める（続く語が上位）。
            1 => {
                pv = (at(ip + 1)? << 12) + data;
                ip += 1;
            }
            2 => pv += data,
            3 => pv -= data,
            // 6・7: 値を増やす・減らしてから 1 画素。
            6 | 7 => {
                pv += if opcode == 6 { data } else { -data };
                if (xs..=xe).contains(&x1) {
                    *out.get_mut(op)
                        .ok_or_else(|| invalid("runs past the tile"))? = pv;
                    op += 1;
                }
                x1 += 1;
            }
            _ => return Err(invalid("unknown instruction")),
        }
        ip += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(v: &[i16]) -> Vec<u8> {
        v.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    #[test]
    fn runs_of_zeros_and_values_fill_the_tile() {
        // 古い形の頭（3 語目が長さ）: 0 を 2 個、値を 5 に上げて 3 個、残りは 0。
        let ll = words(&[0, 0, 6, 2, 2 * 4096 + 4, 4 * 4096 + 3]);
        assert_eq!(decode(&ll, 7).unwrap(), [0, 0, 5, 5, 5, 0, 0]);
    }

    #[test]
    fn single_pixels_step_the_value() {
        // 6: 1 増やして 1 画素（2）、7: 2 減らして 1 画素（0）、5: 0 を 1 個のあと今の値 1 個。
        let ll = words(&[0, 0, 6, 6 * 4096 + 1, 7 * 4096 + 2, 5 * 4096 + 2]);
        assert_eq!(decode(&ll, 4).unwrap(), [2, 0, 0, 0]);
    }
}
