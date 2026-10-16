//! ISO 2022 のエスケープで文字集合を切り替えた文字列（日本語の名前など）を読む。
//!
//! 日本の装置は Specific Character Set を `ISO 2022 IR 6\ISO 2022 IR 87` のように複数書き、
//! 値の中で `ESC $ B`（JIS X 0208）と `ESC ( B`（ASCII）を切り替える。dicom-rs 0.10 は
//! 最初の値しか使わないので、漢字の部分が `ESC $ B ;3ED` のような記号のまま文字列に残る。
//!
//! dicom-rs を手元に取り込んで直す（vendor）のではなく、読んだ後の文字列を読み直す。
//! エスケープも JIS X 0208 のバイトもすべて 7 ビットなので、最初の文字集合で読んだ後も
//! 文字列にそのまま残っており、元のバイト列に戻せるため。

use std::borrow::Cow;

use dicom_core::value::PrimitiveValue;
use encoding::all::ISO_2022_JP;
use encoding::{DecoderTrap, Encoding};

const ESC: char = '\x1b';

/// G1 に半角カナ（JIS X 0201）を割り当てるエスケープ。G1 の文字は 8 ビットで、
/// 最初の文字集合（`ISO 2022 IR 13`）の読み方で読めているので、印だけを捨てる。
const DESIGNATE_KANA_G1: &str = "\x1b)I";

/// エスケープを含む値を読み直す。含まなければそのまま返す。
pub fn decode_escapes(value: &str) -> Cow<'_, str> {
    if !value.contains(ESC) {
        return Cow::Borrowed(value);
    }
    let value = value.replace(DESIGNATE_KANA_G1, "");
    let mut out = String::with_capacity(value.len());
    let mut seven_bit = Vec::new();
    for c in value.chars() {
        if c.is_ascii() {
            seven_bit.push(c as u8);
        } else {
            decode_into(&mut seven_bit, &mut out);
            out.push(c);
        }
    }
    decode_into(&mut seven_bit, &mut out);
    Cow::Owned(out)
}

fn decode_into(bytes: &mut Vec<u8>, out: &mut String) {
    if !bytes.is_empty() {
        // Replace なので失敗しない（読めないバイトは置換文字になる）。
        let _ = ISO_2022_JP.decode_to(bytes, DecoderTrap::Replace, out);
        bytes.clear();
    }
}

/// 値を文字列にする。複数の値は `\` でつなぐ。
///
/// 値ごとに読み直すのは、`\` のバイト（0x5C）が JIS X 0208 の 2 バイト文字の一部にも
/// なり得るため。つないだ後では区切りと区別できない。
pub fn value_text(value: &PrimitiveValue) -> String {
    match value {
        PrimitiveValue::Strs(parts) => parts
            .iter()
            .map(|p| decode_escapes(p))
            .collect::<Vec<_>>()
            .join("\\"),
        PrimitiveValue::Str(s) => decode_escapes(s).into_owned(),
        other => other.to_str().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 「山田^太郎」を JIS X 0208 で書いたもの（ESC $ B で切り替え、ESC ( B で戻す）。
    const YAMADA_TARO: &str = "\x1b$B;3ED\x1b(B^\x1b$BB@O:\x1b(B";

    #[test]
    fn kanji_switched_in_with_escapes_is_read_as_japanese() {
        assert_eq!(decode_escapes(YAMADA_TARO), "山田^太郎");
    }

    #[test]
    fn person_name_groups_keep_their_separators() {
        let pn = format!("Yamada^Taro={YAMADA_TARO}=\x1b$B$d$^$@\x1b(B^\x1b$B$?$m$&\x1b(B");
        assert_eq!(decode_escapes(&pn), "Yamada^Taro=山田^太郎=やまだ^たろう");
    }

    #[test]
    fn half_width_kana_already_read_from_g1_stays_as_is() {
        assert_eq!(decode_escapes("\x1b)Iﾔﾏﾀﾞ^ﾀﾛｳ"), "ﾔﾏﾀﾞ^ﾀﾛｳ");
        assert_eq!(decode_escapes("ﾔﾏﾀﾞ^ﾀﾛｳ"), "ﾔﾏﾀﾞ^ﾀﾛｳ");
    }

    #[test]
    fn text_without_escapes_is_returned_without_copying() {
        assert!(matches!(
            decode_escapes("Doe^Jane"),
            Cow::Borrowed("Doe^Jane")
        ));
    }

    #[test]
    fn each_of_several_values_is_read_on_its_own() {
        // 「旬」は JIS X 0208 で 0x3D 0x5C。2 バイト目が `\` と同じなので、つないでから読むと壊れる。
        let value = PrimitiveValue::Strs(
            vec!["\x1b$B=\\\x1b(B".to_owned(), "\x1b$B;3\x1b(B".to_owned()].into(),
        );
        assert_eq!(value_text(&value), "旬\\山");
    }
}
