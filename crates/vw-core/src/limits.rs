//! 1 枚の画像をデコードするときに使ってよいメモリの上限。
//!
//! 見出しに巨大な幅・高さを書いたファイル（細工したものでも、数バイト壊れただけのものでも）で
//! デコーダがその大きさのメモリを一度に取りに行くと、取れずにプロセスごと終わる。Rust の
//! 確保の失敗は捕まえられないので、取りに行く前に見出しの大きさで断る。
//!
//! 上限はプロセスで 1 つのグローバルに置く。プラグインに渡す案は、[`DecoderPlugin`] がバイト列
//! だけを受け取る形で、後で C ABI の動的プラグインにするときもその形を保ちたいため採らない。
//!
//! [`DecoderPlugin`]: crate::DecoderPlugin

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::DecodeError;

/// 既定の上限（4 GiB）。16bit RGB なら 26,000 x 26,000 まで。
pub const DEFAULT_MAX_IMAGE_BYTES: usize = 4 << 30;

static MAX_IMAGE_BYTES: AtomicUsize = AtomicUsize::new(DEFAULT_MAX_IMAGE_BYTES);

/// 今の上限（バイト）。
pub fn max_image_bytes() -> usize {
    MAX_IMAGE_BYTES.load(Ordering::Relaxed)
}

/// 上限を変える。設定が変わったときに呼ぶ。
pub fn set_max_image_bytes(bytes: usize) {
    MAX_IMAGE_BYTES.store(bytes.max(1), Ordering::Relaxed);
}

/// 幅 `width` × 高さ `height`、1 画素 `bytes_per_pixel` バイトの画像が上限に収まるか。
/// 収まれば画素のバイト数を返す。
pub fn check_image(width: u64, height: u64, bytes_per_pixel: u64) -> Result<usize, DecodeError> {
    let bytes = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(bytes_per_pixel));
    match bytes {
        Some(b) if b <= max_image_bytes() as u64 => Ok(b as usize),
        _ => Err(too_large(format!("image {width} x {height}"), bytes)),
    }
}

/// `bytes` バイトのバッファが上限に収まるか。見出しから幅と高さが分からない形式で使う。
pub fn check_bytes(bytes: u64, what: &str) -> Result<usize, DecodeError> {
    if bytes <= max_image_bytes() as u64 {
        Ok(bytes as usize)
    } else {
        Err(too_large(what.to_owned(), Some(bytes)))
    }
}

fn too_large(what: String, bytes: Option<u64>) -> DecodeError {
    let needs = match bytes {
        Some(b) => format!("needs {}", gib(b)),
        None => "is impossibly large".to_owned(),
    };
    DecodeError::Unsupported(format!(
        "{what} {needs}, more than the image size limit of {}",
        gib(max_image_bytes() as u64)
    ))
}

fn gib(bytes: u64) -> String {
    let g = bytes as f64 / (1u64 << 30) as f64;
    if g >= 10.0 {
        format!("{g:.0} GiB")
    } else {
        format!("{g:.1} GiB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_within_the_limit_passes_with_its_byte_count() {
        assert_eq!(check_image(100, 50, 3).unwrap(), 15_000);
    }

    #[test]
    fn image_over_the_limit_is_refused_with_both_sizes() {
        let e = check_image(200_000, 200_000, 8).unwrap_err().to_string();
        assert!(e.contains("200000 x 200000"), "{e}");
        assert!(e.contains("298 GiB"), "{e}");
        assert!(e.contains("limit of 4.0 GiB"), "{e}");
    }

    #[test]
    fn overflowing_dimensions_are_refused_instead_of_wrapping() {
        let e = check_image(u64::MAX, 2, 1).unwrap_err().to_string();
        assert!(e.contains("impossibly large"), "{e}");
    }

    #[test]
    fn byte_count_is_checked_against_the_same_limit() {
        assert!(check_bytes(1, "buffer").is_ok());
        assert!(check_bytes(5 << 30, "buffer").is_err());
    }
}
