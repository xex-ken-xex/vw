//! LZF（h5py が使う HDF5 の圧縮、フィルタ番号 32000）をほどく。
//!
//! 読み込みのクレートには LZF がないので、差し込み口（[`hdf5_reader::FilterRegistry`]）に足す。
//! 外のクレートを足さないのは、ほどく側は 30 行ほどで済み、依存を増やすほどではないため。

use hdf5_reader::FilterRegistry;
use hdf5_reader::error::Error;
use hdf5_reader::messages::filter_pipeline::FilterDescription;

/// HDF5 に登録された LZF のフィルタ番号。
pub const FILTER_LZF: u16 = 32000;

/// 組み込みのフィルタに LZF を足した差し込み口。
pub fn registry() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    registry.register(
        FILTER_LZF,
        Box::new(
            |desc: &FilterDescription, data: &[u8], _element_size: usize| {
                // h5py は 3 番目の値にほどいた後のチャンクの大きさを書く。ない古いファイルもある。
                let expected = desc.client_data.get(2).map_or(0, |&n| n as usize);
                decompress(data, expected, vw_core::limits::max_image_bytes())
                    .map_err(Error::DecompressionError)
            },
        ),
    );
    registry
}

/// LZF の 1 かたまりをほどく。`expected` は見込みの大きさ（0 なら分からない）、`limit` は上限。
pub fn decompress(input: &[u8], expected: usize, limit: usize) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity(expected.min(limit));
    let mut i = 0;
    let truncated = || "LZF: data ends in the middle of a block".to_owned();
    while i < input.len() {
        let ctrl = usize::from(input[i]);
        i += 1;
        if ctrl < 32 {
            let len = ctrl + 1;
            let lit = input.get(i..i + len).ok_or_else(truncated)?;
            if out.len() + len > limit {
                return Err("LZF: output exceeds the limit".into());
            }
            out.extend_from_slice(lit);
            i += len;
            continue;
        }
        let mut len = ctrl >> 5;
        if len == 7 {
            len += usize::from(*input.get(i).ok_or_else(truncated)?);
            i += 1;
        }
        let low = usize::from(*input.get(i).ok_or_else(truncated)?);
        i += 1;
        let back = ((ctrl & 0x1f) << 8) + low + 1;
        let len = len + 2;
        let start = out
            .len()
            .checked_sub(back)
            .ok_or_else(|| "LZF: back reference before the start".to_owned())?;
        if out.len() + len > limit {
            return Err("LZF: output exceeds the limit".into());
        }
        // 参照が書いている途中の範囲に重なることがあるので、1 バイトずつ写す。
        for k in 0..len {
            let b = out[start + k];
            out.push(b);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_runs_are_copied() {
        assert_eq!(decompress(&[2, b'a', b'b', b'c'], 3, 100).unwrap(), b"abc");
    }

    #[test]
    fn back_references_may_overlap_the_bytes_being_written() {
        // "a" のあと、1 つ前から 8 バイト（len 6 + 2）写す → "a" が 9 個。
        assert_eq!(
            decompress(&[0, b'a', 6 << 5, 0], 9, 100).unwrap(),
            b"aaaaaaaaa"
        );
    }

    #[test]
    fn long_back_references_read_an_extra_length_byte() {
        // len 7 + 3 + 2 = 12 バイト、2 つ前から。
        let out = decompress(&[1, b'x', b'y', 7 << 5, 3, 1], 14, 100).unwrap();
        assert_eq!(out, b"xyxyxyxyxyxyxy");
    }

    #[test]
    fn references_before_the_start_are_rejected() {
        assert!(decompress(&[0, b'a', 1 << 5, 5], 0, 100).is_err());
    }

    #[test]
    fn output_over_the_limit_is_rejected() {
        assert!(decompress(&[0, b'a', 7 << 5, 255, 0], 0, 16).is_err());
    }

    #[test]
    fn truncated_input_is_rejected() {
        assert!(decompress(&[5, b'a'], 0, 100).is_err());
    }
}
