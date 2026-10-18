//! ISO 2022 のエスケープで文字集合を切り替えた文字列（日本語の名前、Latin とキリルの混在など）を読む。
//!
//! 装置は Specific Character Set を `ISO 2022 IR 6\ISO 2022 IR 87` や
//! `ISO 2022 IR 100\ISO 2022 IR 144` のように複数書き、値の中でエスケープを使って G0（英数字・
//! 半角カナ・漢字）と G1（0xA0 以上に置く 1 バイトの文字集合）を切り替える。dicom-rs 0.10 は
//! 最初の値しか使わないので、切り替えた先の文字が最初の文字集合の文字として文字列に残る。
//!
//! dicom-rs を手元に取り込んで直す（vendor）のではなく、読んだ後の文字列を読み直す。
//! 最初の文字集合が 1 バイトで 1 文字のもの（ASCII・半角カナ・Latin・キリルなど）なら、
//! 読んだ文字列から元のバイト列に損失なく戻せるため。戻したバイト列を、G0 と G1 を別に持つ
//! 状態で読み直す。encoding の `ISO_2022_JP` に任せないのは、G1 の割り当て（`ESC - A` など）を
//! 知らず、pydicom が漢字の後を `ESC - A` で戻すファイルや、キリルなどへの切り替えを読めないため。

use std::borrow::Cow;

use dicom_core::value::PrimitiveValue;
use encoding::all::{
    EUC_JP, ISO_8859_2, ISO_8859_3, ISO_8859_4, ISO_8859_5, ISO_8859_6, ISO_8859_7, ISO_8859_8,
    WINDOWS_874,
};
use encoding::{DecoderTrap, EncoderTrap, Encoding, EncodingRef};

const ESC: u8 = 0x1b;
const REPLACEMENT: char = '\u{FFFD}';

/// G1 に割り当てられる 1 バイトの文字集合。`ESC - <終端>` で割り当て、0xA0〜0xFF に置く。
/// IR 148（トルコ語）は、dicom-rs が最初の値として読めないので入れていない。
struct Set {
    ir: &'static str,
    designator: u8,
    /// `None` は ISO 8859-1（0xA0〜0xFF がそのまま U+00A0〜U+00FF）。
    encoding: Option<EncodingRef>,
}

const SETS: &[Set] = &[
    Set {
        ir: "IR 100",
        designator: b'A',
        encoding: None,
    },
    Set {
        ir: "IR 101",
        designator: b'B',
        encoding: Some(ISO_8859_2),
    },
    Set {
        ir: "IR 109",
        designator: b'C',
        encoding: Some(ISO_8859_3),
    },
    Set {
        ir: "IR 110",
        designator: b'D',
        encoding: Some(ISO_8859_4),
    },
    Set {
        ir: "IR 144",
        designator: b'L',
        encoding: Some(ISO_8859_5),
    },
    Set {
        ir: "IR 127",
        designator: b'G',
        encoding: Some(ISO_8859_6),
    },
    Set {
        ir: "IR 126",
        designator: b'F',
        encoding: Some(ISO_8859_7),
    },
    Set {
        ir: "IR 138",
        designator: b'H',
        encoding: Some(ISO_8859_8),
    },
    Set {
        ir: "IR 166",
        designator: b'T',
        encoding: Some(WINDOWS_874),
    },
];

/// G1 に置いている文字集合。
#[derive(Debug, Clone, Copy, PartialEq)]
enum G1 {
    /// 半角カナ（JIS X 0201、`ISO 2022 IR 13`）。`ESC ) I`
    Katakana,
    /// [`SETS`] の何番目か。
    Set(usize),
}

impl G1 {
    fn decode(self, byte: u8) -> char {
        match self {
            G1::Katakana if (0xA1..=0xDF).contains(&byte) => {
                char::from_u32(0xFF61 + u32::from(byte - 0xA1)).unwrap_or(REPLACEMENT)
            }
            G1::Katakana => REPLACEMENT,
            G1::Set(n) => match SETS[n].encoding {
                None => char::from(byte),
                Some(e) => e
                    .decode(&[byte], DecoderTrap::Strict)
                    .ok()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(REPLACEMENT),
            },
        }
    }

    /// dicom-rs がこの文字集合で読んだ 1 文字を、元の 1 バイトに戻す。
    fn encode(self, c: char) -> Option<u8> {
        match self {
            G1::Katakana => {
                let offset = u32::from(c).checked_sub(0xFF61)?;
                (offset <= 0xDF - 0xA1).then(|| 0xA1 + offset as u8)
            }
            G1::Set(n) => match SETS[n].encoding {
                None => u8::try_from(u32::from(c)).ok().filter(|b| *b >= 0xA0),
                Some(e) => match e.encode(c.encode_utf8(&mut [0; 4]), EncoderTrap::Strict) {
                    Ok(b) if b.len() == 1 && b[0] >= 0xA0 => Some(b[0]),
                    _ => None,
                },
            },
        }
    }
}

/// ファイルの Specific Character Set のうち、読み直すのに要る、最初の値が決める初めの状態。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Charset {
    initial_g1: Option<G1>,
}

impl Charset {
    /// Specific Character Set の値から。`ISO 2022 IR 100` と `ISO_IR 100` のどちらの綴りもある。
    /// 最初の値が知らない文字集合（UTF-8 や韓国語など）なら、ASCII だけを戻して読み直す。
    pub fn from_values<S: AsRef<str>>(values: &[S]) -> Self {
        let first = values.first().map_or("", |v| v.as_ref().trim());
        let ir = first
            .strip_prefix("ISO 2022 ")
            .or_else(|| first.strip_prefix("ISO_"))
            .unwrap_or(first);
        let initial_g1 = match ir {
            "IR 13" => Some(G1::Katakana),
            other => SETS.iter().position(|s| s.ir == other).map(G1::Set),
        };
        Self { initial_g1 }
    }

    /// dicom-rs が最初の文字集合で読んだ文字列を、元のバイトに戻す。戻せない文字（最初の文字集合が
    /// 複数バイトのときなど）は、そのまま文字として残す。
    fn raw(&self, decoded: &str) -> Vec<Piece> {
        decoded
            .chars()
            .map(|c| {
                if c.is_ascii() {
                    Piece::Byte(c as u8)
                } else {
                    self.initial_g1
                        .and_then(|g1| g1.encode(c))
                        .map_or(Piece::Char(c), Piece::Byte)
                }
            })
            .collect()
    }

    /// エスケープを含む値を読み直す。含まなければそのまま返す。
    pub fn decode<'a>(&self, value: &'a str) -> Cow<'a, str> {
        if !value.contains(char::from(ESC)) {
            return Cow::Borrowed(value);
        }
        Cow::Owned(self.read(&self.raw(value)))
    }

    fn read(&self, pieces: &[Piece]) -> String {
        let mut out = String::with_capacity(pieces.len());
        let mut g0 = G0::Ascii;
        let mut g1 = self.initial_g1;
        let byte = |i: usize| match pieces.get(i) {
            Some(Piece::Byte(b)) => Some(*b),
            _ => None,
        };
        let mut i = 0;
        while i < pieces.len() {
            let b = match pieces[i] {
                Piece::Char(c) => {
                    out.push(c);
                    i += 1;
                    continue;
                }
                Piece::Byte(b) => b,
            };
            if b == ESC {
                let tail = [byte(i + 1), byte(i + 2), byte(i + 3)];
                let (len, switch) = escape(tail);
                match switch {
                    Some(Switch::G0(set)) => g0 = set,
                    Some(Switch::G1(set)) => {
                        g1 = Some(set);
                        // pydicom は漢字の後を ESC ( B ではなく ESC - A で戻す。G0 は切り替えて
                        // いないが、続く英字を漢字の 2 バイトとして読まないよう、漢字を終えたものとする。
                        if matches!(g0, G0::Kanji | G0::Supplementary) {
                            g0 = G0::Ascii;
                        }
                    }
                    None => out.push(REPLACEMENT),
                }
                i += len;
            } else if b >= 0xA0 {
                out.push(g1.map_or(REPLACEMENT, |g1| g1.decode(b)));
                i += 1;
            } else if b >= 0x80 {
                out.push(REPLACEMENT);
                i += 1;
            } else if matches!(b, b'\n' | b'\r' | 0x0C) {
                // 改行と改ページで初めの状態に戻る（規格の決まり）。
                g0 = G0::Ascii;
                g1 = self.initial_g1;
                out.push(char::from(b));
                i += 1;
            } else if b > 0x20 && b < 0x7F && matches!(g0, G0::Kanji | G0::Supplementary) {
                out.push(double_byte(g0, b, byte(i + 1)));
                i += 2;
            } else if (0x21..=0x5F).contains(&b) && g0 == G0::Kana {
                out.push(char::from_u32(0xFF61 + u32::from(b - 0x21)).unwrap_or(REPLACEMENT));
                i += 1;
            } else {
                out.push(char::from(b));
                i += 1;
            }
        }
        out
    }
}

/// 読み直す前の 1 つ分。元のバイトに戻せなかった文字は、そのまま出す。
#[derive(Debug, Clone, Copy)]
enum Piece {
    Byte(u8),
    Char(char),
}

/// G0 に置いている文字集合。
#[derive(Debug, Clone, Copy, PartialEq)]
enum G0 {
    Ascii,
    /// 半角カナを 7 ビットで（`ESC ( I`）。
    Kana,
    /// JIS X 0208（`ESC $ B`、`ISO 2022 IR 87`）。
    Kanji,
    /// JIS X 0212 の補助漢字（`ESC $ ( D`、`ISO 2022 IR 159`）。
    Supplementary,
}

enum Switch {
    G0(G0),
    G1(G1),
}

/// ESC に続くバイトから、エスケープの長さ（ESC を含む）と切り替え先を読む。知らないエスケープは
/// ESC だけを読み飛ばし、続くバイトはふつうの文字として出す（何が書いてあったか見えるように）。
fn escape(tail: [Option<u8>; 3]) -> (usize, Option<Switch>) {
    match tail {
        [Some(b'('), Some(b'B' | b'J'), _] => (3, Some(Switch::G0(G0::Ascii))),
        [Some(b'('), Some(b'I'), _] => (3, Some(Switch::G0(G0::Kana))),
        [Some(b'$'), Some(b'B' | b'@'), _] => (3, Some(Switch::G0(G0::Kanji))),
        [Some(b'$'), Some(b'('), Some(b'D')] => (4, Some(Switch::G0(G0::Supplementary))),
        [Some(b')'), Some(b'I'), _] => (3, Some(Switch::G1(G1::Katakana))),
        [Some(b'-'), Some(f), _] => match SETS.iter().position(|s| s.designator == f) {
            Some(n) => (3, Some(Switch::G1(G1::Set(n)))),
            None => (1, None),
        },
        _ => (1, None),
    }
}

/// 漢字の 2 バイトを読む。JIS X 0208 は EUC-JP の 2 バイトから 0x80 を引いたもの、JIS X 0212 は
/// EUC-JP で 0x8F に続く 2 バイトから 0x80 を引いたもの。
fn double_byte(g0: G0, first: u8, second: Option<u8>) -> char {
    let Some(second) = second.filter(|b| (0x21..0x7F).contains(b)) else {
        return REPLACEMENT;
    };
    let pair = [first | 0x80, second | 0x80];
    let euc: Vec<u8> = match g0 {
        G0::Supplementary => [&[0x8F], &pair[..]].concat(),
        _ => pair.to_vec(),
    };
    match EUC_JP.decode(&euc, DecoderTrap::Strict) {
        Ok(s) if s.chars().count() == 1 => s.chars().next().unwrap_or(REPLACEMENT),
        _ => REPLACEMENT,
    }
}

/// 値を文字列にする。複数の値は `\` でつなぐ。
///
/// 値ごとに読み直すのは、`\` のバイト（0x5C）が JIS X 0208 の 2 バイト文字の一部にも
/// なり得るため。つないだ後では区切りと区別できない。
pub fn value_text(value: &PrimitiveValue, charset: &Charset) -> String {
    match value {
        PrimitiveValue::Strs(parts) => parts
            .iter()
            .map(|p| charset.decode(p))
            .collect::<Vec<_>>()
            .join("\\"),
        PrimitiveValue::Str(s) => charset.decode(s).into_owned(),
        other => other.to_str().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 「山田^太郎」を JIS X 0208 で書いたもの（ESC $ B で切り替え、ESC ( B で戻す）。
    const YAMADA_TARO: &str = "\x1b$B;3ED\x1b(B^\x1b$BB@O:\x1b(B";

    fn charset(values: &[&str]) -> Charset {
        Charset::from_values(values)
    }

    /// dicom-rs が Latin-1 で読んだときの文字列（各バイトが U+0000〜U+00FF の文字）。
    fn latin1(bytes: &[u8]) -> String {
        bytes.iter().map(|b| char::from(*b)).collect()
    }

    /// dicom-rs が `encoding` の 1 バイトの文字集合で読んだときの文字列。
    fn read_as(e: EncodingRef, bytes: &[u8]) -> String {
        e.decode(bytes, DecoderTrap::Strict).unwrap()
    }

    #[test]
    fn kanji_switched_in_with_escapes_is_read_as_japanese() {
        let jis = charset(&["ISO 2022 IR 6", "ISO 2022 IR 87"]);
        assert_eq!(jis.decode(YAMADA_TARO), "山田^太郎");
        assert_eq!(
            charset(&["", "ISO 2022 IR 87"]).decode(YAMADA_TARO),
            "山田^太郎"
        );
    }

    #[test]
    fn person_name_groups_keep_their_separators() {
        let pn = format!("Yamada^Taro={YAMADA_TARO}=\x1b$B$d$^$@\x1b(B^\x1b$B$?$m$&\x1b(B");
        assert_eq!(
            charset(&["ISO 2022 IR 6", "ISO 2022 IR 87"]).decode(&pn),
            "Yamada^Taro=山田^太郎=やまだ^たろう"
        );
    }

    #[test]
    fn jahis_names_returning_with_esc_paren_j_are_read() {
        let jahis = charset(&["ISO 2022 IR 13", "ISO 2022 IR 87"]);
        assert_eq!(
            jahis.decode("\x1b$B;3ED\x1b(J^\x1b$BB@O:\x1b(J"),
            "山田^太郎"
        );
    }

    #[test]
    fn half_width_kana_already_read_from_g1_stays_as_is() {
        let jahis = charset(&["ISO 2022 IR 13", "ISO 2022 IR 87"]);
        assert_eq!(jahis.decode("\x1b)Iﾔﾏﾀﾞ^ﾀﾛｳ"), "ﾔﾏﾀﾞ^ﾀﾛｳ");
        assert_eq!(jahis.decode("ﾔﾏﾀﾞ^ﾀﾛｳ"), "ﾔﾏﾀﾞ^ﾀﾛｳ");
        assert_eq!(
            jahis.decode(&format!("ﾔﾏﾀﾞ^ﾀﾛｳ={YAMADA_TARO}")),
            "ﾔﾏﾀﾞ^ﾀﾛｳ=山田^太郎"
        );
    }

    #[test]
    fn text_without_escapes_is_returned_without_copying() {
        assert!(matches!(
            charset(&["ISO 2022 IR 100"]).decode("Doe^Jane"),
            Cow::Borrowed("Doe^Jane")
        ));
    }

    #[test]
    fn latin_with_kanji_is_read_whether_returned_by_g0_or_by_g1() {
        let set = charset(&["ISO 2022 IR 100", "ISO 2022 IR 87"]);
        let standard = latin1(b"Kopf-CT \x1b$BF,It\x1b(B M\xfcller");
        assert_eq!(set.decode(&standard), "Kopf-CT 頭部 Müller");
        // pydicom 3 が書く形。漢字の後を ESC - A で戻す。
        let pydicom = latin1(b"\x1b-AKopf-CT \x1b$BF,It\x1b-A M\xfcller");
        assert_eq!(set.decode(&pydicom), "Kopf-CT 頭部 Müller");
        let spelled = charset(&["ISO_IR 100", "ISO 2022 IR 87"]);
        assert_eq!(spelled.decode(&pydicom), "Kopf-CT 頭部 Müller");
    }

    #[test]
    fn cyrillic_and_greek_first_values_are_read_with_kanji() {
        let cyrillic = charset(&["ISO 2022 IR 144", "ISO 2022 IR 87"]);
        let read = read_as(
            ISO_8859_5,
            b"\x1b-L\xb8\xd2\xd0\xdd\xde\xd2 \x1b$B;3ED\x1b-L",
        );
        assert_eq!(cyrillic.decode(&read), "Иванов 山田");
        let greek = charset(&["ISO 2022 IR 126", "ISO 2022 IR 87"]);
        let read = read_as(
            ISO_8859_7,
            b"\x1b-F\xd0\xe1\xf0\xe1\xe4\xfc\xf0\xef\xf5\xeb\xef\xf2 \x1b$B;3ED\x1b-F",
        );
        assert_eq!(greek.decode(&read), "Παπαδόπουλος 山田");
    }

    #[test]
    fn g1_switched_mid_value_reads_the_bytes_in_the_new_set() {
        let set = charset(&["ISO 2022 IR 100", "ISO 2022 IR 144"]);
        let read = latin1(b"\x1b-AM\xfcller \x1b-L\xb8\xd2\xd0\xdd\xde\xd2 ");
        assert_eq!(set.decode(&read), "Müller Иванов ");
    }

    #[test]
    fn a_new_line_returns_to_the_first_character_sets() {
        let set = charset(&["ISO 2022 IR 100", "ISO 2022 IR 144"]);
        let read = latin1(b"\x1b-L\xb8\r\n\xfc");
        assert_eq!(set.decode(&read), "И\r\nü");
    }

    #[test]
    fn supplementary_kanji_is_read() {
        // 「丂」は JIS X 0212 で 0x30 0x21。
        let set = charset(&["ISO 2022 IR 6", "ISO 2022 IR 87", "ISO 2022 IR 159"]);
        assert_eq!(set.decode("\x1b$(D0!\x1b(B"), "丂");
    }

    #[test]
    fn unknown_escapes_and_broken_kanji_become_replacement_characters() {
        let set = charset(&["ISO 2022 IR 6", "ISO 2022 IR 87"]);
        assert_eq!(set.decode("a\x1b%Gb"), "a\u{FFFD}%Gb");
        assert_eq!(set.decode("\x1b$B;"), "\u{FFFD}");
    }

    #[test]
    fn each_of_several_values_is_read_on_its_own() {
        // 「旬」は JIS X 0208 で 0x3D 0x5C。2 バイト目が `\` と同じなので、つないでから読むと壊れる。
        let value = PrimitiveValue::Strs(
            vec!["\x1b$B=\\\x1b(B".to_owned(), "\x1b$B;3\x1b(B".to_owned()].into(),
        );
        let set = charset(&["ISO 2022 IR 6", "ISO 2022 IR 87"]);
        assert_eq!(value_text(&value, &set), "旬\\山");
    }
}
