//! EXIF（JPEG の APP1 や WebP の EXIF チャンクに入る、TIFF 形式のタグの集まり）をメタデータにする。
//!
//! 形式ごとのプラグインで同じ表示にするため、ここにまとめる。

use crate::image::Metadata;

/// EXIF の `raw`（TIFF の形、`Exif\0\0` を除いたもの）のタグを `m` の "EXIF" に足し、
/// 向き（Orientation、1〜8）を返す（なければ 1）。読めない EXIF は無視する。
/// 画像が表示できることを優先し、EXIF の不備で開けなくはしないため。
pub fn push_exif(m: &mut Metadata, raw: Vec<u8>) -> u32 {
    push_exif_except(m, raw, &[])
}

/// [`push_exif`] の、タグ番号が `skip` のものを除く版。TIFF ファイルそのものを EXIF として
/// 読むとき、画素の置き場所（StripOffsets など）の長い数の並びを出さないために使う。
pub fn push_exif_except(m: &mut Metadata, raw: Vec<u8>, skip: &[u16]) -> u32 {
    let Ok(exif) = exif::Reader::new().read_raw(raw) else {
        return 1;
    };
    for f in exif.fields() {
        // メーカーノートは中身がメーカー独自の長いバイナリで、表示しても読めないため除く。
        // サムネイルの IFD は本体と同じタグ（幅や向き）を持ち、紛らわしいため除く。
        if f.ifd_num == exif::In::THUMBNAIL
            || f.tag == exif::Tag::MakerNote
            || skip.contains(&f.tag.number())
        {
            continue;
        }
        m.push("EXIF", f.tag.to_string(), value(f, &exif));
    }
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .unwrap_or(1)
}

/// EXIF の値の表示。文字列は kamadak-exif の表示だと `"NIKON"` のように引用符が付くので、
/// 中身だけを出す。
fn value(f: &exif::Field, exif: &exif::Exif) -> String {
    match &f.value {
        exif::Value::Ascii(parts) => parts
            .iter()
            .map(|p| String::from_utf8_lossy(p).trim_end().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        _ => f.display_value().with_unit(exif).to_string(),
    }
}

/// テスト用の EXIF（TIFF の形、リトルエンディアン）。他のクレートのテストからも使う。
#[doc(hidden)]
pub mod sample {
    /// 向き（Orientation）のタグだけを持つ EXIF。
    pub fn orientation(orientation: u16) -> Vec<u8> {
        let mut t = b"II*\0".to_vec();
        t.extend(8u32.to_le_bytes()); // 0 番目の IFD の位置
        t.extend(1u16.to_le_bytes()); // タグの数
        t.extend(0x0112u16.to_le_bytes()); // Orientation
        t.extend(3u16.to_le_bytes()); // SHORT
        t.extend(1u32.to_le_bytes()); // 個数
        t.extend(orientation.to_le_bytes());
        t.extend([0, 0]);
        t.extend(0u32.to_le_bytes()); // 次の IFD はない
        t
    }

    /// メーカー名（Make、3 文字）のタグだけを持つ EXIF。
    pub fn make(make: &[u8; 3]) -> Vec<u8> {
        let mut t = b"II*\0".to_vec();
        t.extend(8u32.to_le_bytes());
        t.extend(1u16.to_le_bytes());
        t.extend(0x010Fu16.to_le_bytes()); // Make
        t.extend(2u16.to_le_bytes()); // ASCII
        t.extend(4u32.to_le_bytes()); // 終端の NUL を含む長さ
        t.extend(make);
        t.push(0);
        t.extend(0u32.to_le_bytes());
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_shown_without_quotes() {
        let mut m = Metadata::default();
        push_exif(&mut m, sample::make(b"Cam"));
        assert_eq!(m.get("EXIF", "Make"), Some("Cam"));
    }

    #[test]
    fn returns_the_orientation() {
        let mut m = Metadata::default();
        assert_eq!(push_exif(&mut m, sample::orientation(6)), 6);
        assert_eq!(
            m.get("EXIF", "Orientation"),
            Some("row 0 at right and column 0 at top")
        );
    }

    #[test]
    fn broken_exif_is_ignored_and_means_upright() {
        let mut m = Metadata::default();
        assert_eq!(push_exif(&mut m, b"II*\0garbage".to_vec()), 1);
        assert!(m.entries.is_empty());
    }
}
