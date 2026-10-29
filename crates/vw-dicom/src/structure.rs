//! 要素のヘッダだけを辿って、長さと入れ子の深さが正しいかを確かめる。
//!
//! dicom-rs は、要素の長さの欄に書かれた分のメモリを値を読む前に確保し、シーケンスの入れ子を
//! 再帰で読む。長さの欄が壊れたファイル（最大 4GB）や入れ子の深すぎるファイルを渡すと、
//! メモリの確保やスタックが尽きてプロセスごと止まる。panic ではないので `catch_unwind` でも
//! 捕まえられない。そこで、渡す前にここで止める。dicom-rs の読み手に上限を渡せる口はないので、
//! 読み手を包むのではなく前に置く。

use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry};
use dicom_core::{Tag, VR};
use vw_core::DecodeError;

/// シーケンスの入れ子の上限。規格の上限はないが、実際のファイルは 10 段もない。
pub const MAX_DEPTH: usize = 64;

/// 値がファイルの終わりをこれ以上越える要素を、壊れた長さとする。終わりちょうどで
/// 止めないのは、dicom-rs が終わりを少し越える最後の値を、ある分だけ読んで画像を出せるため
/// （長さを 1 だけ書き損じたファイルが今は開ける）。越えた分だけ余計に確保されるが、
/// この大きさなら読み込みのスレッドが並んでも足りる。
const OVERRUN_LIMIT: u64 = 64 << 20;

const PREAMBLE: usize = 128 + 4;
const ITEM: Tag = Tag(0xFFFE, 0xE000);
const ITEM_END: Tag = Tag(0xFFFE, 0xE00D);
const SEQUENCE_END: Tag = Tag(0xFFFE, 0xE0DD);
const UNDEFINED: u32 = 0xFFFF_FFFF;
const TRANSFER_SYNTAX: Tag = Tag(0x0002, 0x0010);
const PIXEL_DATA: Tag = Tag(0x7FE0, 0x0010);

/// `data` はファイルの頭から `data.len()` バイト。`total` はファイル全体の長さで、
/// 要素の長さはこれを越えてはいけない。頭だけを渡したときは、その終わりで辿るのをやめる。
/// `header_only` なら、一番外の画素データの手前でやめる（値は読まないので長さも問わない）。
pub fn check(data: &[u8], total: u64, header_only: bool) -> Result<(), DecodeError> {
    let Some(body) = data.get(PREAMBLE..) else {
        return Ok(());
    };
    let mut w = Walker {
        data: body,
        base: PREAMBLE as u64,
        total,
        pos: 0,
        syntax: Syntax::EXPLICIT_LE,
    };
    let syntax = match w.meta()? {
        Walk::More(s) => s,
        Walk::End => return Ok(()),
    };
    // 圧縮（deflate）された本体は展開しないと辿れない。まれな形式なので、メタ情報だけを確かめる。
    if syntax.deflated {
        return Ok(());
    }
    w.syntax = syntax;
    w.elements(None, 0, header_only).map(drop)
}

#[derive(Clone, Copy)]
struct Syntax {
    explicit: bool,
    big_endian: bool,
    deflated: bool,
}

impl Syntax {
    const EXPLICIT_LE: Syntax = Syntax {
        explicit: true,
        big_endian: false,
        deflated: false,
    };

    fn of(uid: &str) -> Syntax {
        match uid.trim_end_matches(['\0', ' ']) {
            "1.2.840.10008.1.2" => Syntax {
                explicit: false,
                ..Self::EXPLICIT_LE
            },
            "1.2.840.10008.1.2.2" => Syntax {
                big_endian: true,
                ..Self::EXPLICIT_LE
            },
            "1.2.840.10008.1.2.1.99" | "1.2.840.10008.1.2.4.95" => Syntax {
                deflated: true,
                ..Self::EXPLICIT_LE
            },
            _ => Self::EXPLICIT_LE,
        }
    }
}

/// 辿り続けるか、`data` の終わりに着いたか。
enum Walk<T> {
    More(T),
    End,
}

struct Header {
    tag: Tag,
    vr: Option<VR>,
    length: u32,
}

struct Walker<'a> {
    data: &'a [u8],
    /// `data` の頭がファイルの何バイト目か。
    base: u64,
    total: u64,
    pos: usize,
    syntax: Syntax,
}

impl Walker<'_> {
    /// メタ情報グループ（0002）を辿り、本体の転送構文を返す。メタ情報は常に明示 VR の
    /// リトルエンディアン。
    fn meta(&mut self) -> Result<Walk<Syntax>, DecodeError> {
        let mut syntax = Syntax::EXPLICIT_LE;
        loop {
            let Some(group) = self.peek_group() else {
                return Ok(Walk::End);
            };
            if group != 0x0002 {
                return Ok(Walk::More(syntax));
            }
            let Walk::More(h) = self.header()? else {
                return Ok(Walk::End);
            };
            if h.length == UNDEFINED {
                return Err(self.broken(&h, "has no length in the file meta information"));
            }
            // dicom-rs はメタ情報の値を、VR に関わらず書かれた長さの分だけ読む。
            let Walk::More(value) = self.value(&h, h.length)? else {
                return Ok(Walk::End);
            };
            if h.tag == TRANSFER_SYNTAX {
                syntax = Syntax::of(&String::from_utf8_lossy(value));
            }
        }
    }

    /// 要素を並びの終わりまで辿る。`end` は定義長の項目の終わりの位置（未定義長なら `None`
    /// で、区切りのタグまで）。`depth` はシーケンスの入れ子の深さ。
    fn elements(
        &mut self,
        end: Option<usize>,
        depth: usize,
        header_only: bool,
    ) -> Result<Walk<()>, DecodeError> {
        while end.is_none_or(|e| self.pos < e) {
            let Walk::More(h) = self.header()? else {
                return Ok(Walk::End);
            };
            if h.tag == ITEM_END || h.tag == SEQUENCE_END {
                if depth == 0 {
                    return Err(self.broken(&h, "is a delimiter outside a sequence"));
                }
                if end.is_none() {
                    return Ok(Walk::More(()));
                }
                continue;
            }
            if header_only && depth == 0 && h.tag == PIXEL_DATA {
                return Ok(Walk::End);
            }
            let sequence = h.vr == Some(VR::SQ) || h.length == UNDEFINED;
            if !sequence {
                if let Walk::End = self.value(&h, read_length(&h))? {
                    return Ok(Walk::End);
                }
                continue;
            }
            if depth + 1 > MAX_DEPTH {
                return Err(DecodeError::Invalid(format!(
                    "sequences are nested deeper than {MAX_DEPTH}"
                )));
            }
            // 画素データの断片（未定義長の OB）は項目の中身が画素なので、中を辿らない。
            let fragments = h.vr != Some(VR::SQ) && h.tag == PIXEL_DATA;
            let seq_end = self.end_of(&h);
            if let Walk::End = self.items(seq_end, depth + 1, fragments)? {
                return Ok(Walk::End);
            }
        }
        Ok(Walk::More(()))
    }

    /// シーケンスの項目を辿る。
    fn items(
        &mut self,
        end: Option<usize>,
        depth: usize,
        fragments: bool,
    ) -> Result<Walk<()>, DecodeError> {
        while end.is_none_or(|e| self.pos < e) {
            let Walk::More(h) = self.item_header()? else {
                return Ok(Walk::End);
            };
            match h.tag {
                // 定義長のシーケンスに区切りがあっても、dicom-rs はそこで終わりにして読み進める。
                SEQUENCE_END => return Ok(Walk::More(())),
                ITEM if fragments => {
                    if let Walk::End = self.value(&h, read_length(&h))? {
                        return Ok(Walk::End);
                    }
                }
                ITEM => {
                    let item_end = self.end_of(&h);
                    if let Walk::End = self.elements(item_end, depth, false)? {
                        return Ok(Walk::End);
                    }
                }
                _ => return Err(self.broken(&h, "is not an item of a sequence")),
            }
        }
        Ok(Walk::More(()))
    }

    /// 定義長のシーケンスや項目の終わりの位置。dicom-rs は中身を確保せずに項目を読むので、
    /// 終わりを越えていても、データの終わりまで辿れば足りる。
    fn end_of(&self, h: &Header) -> Option<usize> {
        (h.length != UNDEFINED).then(|| self.pos.saturating_add(h.length as usize))
    }

    fn fits(&self, h: &Header) -> Result<(), DecodeError> {
        let end = self.base + self.pos as u64 + u64::from(h.length);
        if end > self.total.saturating_add(OVERRUN_LIMIT) {
            Err(self.broken(h, "is longer than the rest of the file"))
        } else {
            Ok(())
        }
    }

    /// 値を読み飛ばす。値が `data` の終わりを越えるなら `End`。
    fn value(&mut self, h: &Header, length: u32) -> Result<Walk<&[u8]>, DecodeError> {
        self.fits(h)?;
        let start = self.pos;
        let Some(end) = start.checked_add(length as usize) else {
            return Ok(Walk::End);
        };
        let Some(value) = self.data.get(start..end) else {
            return Ok(Walk::End);
        };
        self.pos = end;
        Ok(Walk::More(value))
    }

    fn broken(&self, h: &Header, why: &str) -> DecodeError {
        DecodeError::Invalid(format!(
            "broken element ({:04X},{:04X}) at byte {}: {why}",
            h.tag.group(),
            h.tag.element(),
            self.base + self.pos as u64
        ))
    }

    fn peek_group(&self) -> Option<u16> {
        let b = self.data.get(self.pos..self.pos + 2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u16_at(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.data.get(at..at + 2)?.try_into().ok()?;
        Some(if self.syntax.big_endian {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(at..at + 4)?.try_into().ok()?;
        Some(if self.syntax.big_endian {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }

    fn tag_at(&self, at: usize) -> Option<Tag> {
        Some(Tag(self.u16_at(at)?, self.u16_at(at + 2)?))
    }

    /// 項目と区切りのタグは、明示 VR でも VR を持たない。
    fn item_header(&mut self) -> Result<Walk<Header>, DecodeError> {
        let p = self.pos;
        let (Some(tag), Some(length)) = (self.tag_at(p), self.u32_at(p + 4)) else {
            return Ok(Walk::End);
        };
        self.pos = p + 8;
        Ok(Walk::More(Header {
            tag,
            vr: None,
            length,
        }))
    }

    fn header(&mut self) -> Result<Walk<Header>, DecodeError> {
        let p = self.pos;
        let Some(tag) = self.tag_at(p) else {
            return Ok(Walk::End);
        };
        if tag.group() == 0xFFFE {
            return self.item_header();
        }
        if !self.syntax.explicit {
            let Some(length) = self.u32_at(p + 4) else {
                return Ok(Walk::End);
            };
            self.pos = p + 8;
            let vr = dicom_dictionary_std::StandardDataDictionary
                .by_tag(tag)
                .map(|e| e.vr().relaxed());
            return Ok(Walk::More(Header { tag, vr, length }));
        }
        let Some(code) = self.data.get(p + 4..p + 6) else {
            return Ok(Walk::End);
        };
        let vr = VR::from_binary([code[0], code[1]]);
        let long = vr.is_none_or(|vr| {
            matches!(
                vr,
                VR::OB
                    | VR::OD
                    | VR::OF
                    | VR::OL
                    | VR::OV
                    | VR::OW
                    | VR::SQ
                    | VR::SV
                    | VR::UC
                    | VR::UN
                    | VR::UR
                    | VR::UT
                    | VR::UV
            )
        });
        let length = if long {
            self.pos = p + 12;
            self.u32_at(p + 8)
        } else {
            self.pos = p + 8;
            self.u16_at(p + 6).map(u32::from)
        };
        match length {
            Some(length) => Ok(Walk::More(Header { tag, vr, length })),
            None => Ok(Walk::End),
        }
    }
}

/// dicom-rs が値として読み進める長さ。数の VR は 1 つの大きさで割り切れる分だけを読むので、
/// 長さが奇数に書き損じられていても、次の要素は正しい位置から読める。
fn read_length(h: &Header) -> u32 {
    let size = match h.vr {
        Some(VR::US | VR::SS | VR::OW) => 2,
        Some(VR::UL | VR::SL | VR::FL | VR::AT | VR::OF | VR::OL) => 4,
        Some(VR::FD | VR::OD | VR::SV | VR::UV | VR::OV) => 8,
        _ => 1,
    };
    h.length - h.length % size
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Synth;

    fn short(tag: (u16, u16), vr: &[u8; 2], length: u16, value: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(tag.0.to_le_bytes());
        b.extend(tag.1.to_le_bytes());
        b.extend(vr);
        b.extend(length.to_le_bytes());
        b.extend(value);
        b
    }

    fn long(tag: (u16, u16), vr: &[u8; 2], length: u32, value: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(tag.0.to_le_bytes());
        b.extend(tag.1.to_le_bytes());
        b.extend(vr);
        b.extend([0, 0]);
        b.extend(length.to_le_bytes());
        b.extend(value);
        b
    }

    fn item(tag: (u16, u16), length: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(tag.0.to_le_bytes());
        b.extend(tag.1.to_le_bytes());
        b.extend(length.to_le_bytes());
        b
    }

    /// プリアンブルとメタ情報（明示 VR のリトルエンディアン）の後ろに `body` を置いたファイル。
    fn file(body: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b.extend(b"DICM");
        b.extend(short((0x0002, 0x0010), b"UI", 20, b"1.2.840.10008.1.2.1\0"));
        b.extend(body);
        b
    }

    fn checked(data: &[u8]) -> Result<(), DecodeError> {
        check(data, data.len() as u64, false)
    }

    /// `depth` 段の未定義長のシーケンス。
    fn nested(depth: usize) -> Vec<u8> {
        let mut b = Vec::new();
        for _ in 0..depth {
            b.extend(long((0x0040, 0x0260), b"SQ", UNDEFINED, &[]));
            b.extend(item((0xFFFE, 0xE000), UNDEFINED));
        }
        for _ in 0..depth {
            b.extend(item((0xFFFE, 0xE00D), 0));
            b.extend(item((0xFFFE, 0xE0DD), 0));
        }
        b
    }

    #[test]
    fn a_whole_file_passes() {
        let data = Synth::ct(4, 4).pixels_i16(&[0; 16]).bytes();
        assert!(checked(&data).is_ok());
        assert!(check(&data, data.len() as u64, true).is_ok());
    }

    #[test]
    fn a_length_far_past_the_end_of_the_file_is_broken() {
        let data = file(&long((0x0008, 0x1030), b"UT", 0xFFFF_FFF0, b"text"));
        let e = checked(&data).unwrap_err().to_string();
        assert!(e.contains("(0008,1030)"), "{e}");
    }

    #[test]
    fn a_last_value_a_little_too_long_is_left_to_the_reader() {
        let data = file(&long((0x7FE0, 0x0010), b"OW", 33, &[0; 32]));
        assert!(checked(&data).is_ok());
    }

    #[test]
    fn an_odd_length_of_a_number_does_not_shift_the_next_element() {
        let mut body = short((0x0028, 0x0002), b"US", 3, &[1, 0]);
        body.extend(short((0x0028, 0x0010), b"US", 2, &[4, 0]));
        assert!(checked(&file(&body)).is_ok());
    }

    #[test]
    fn sequences_nested_too_deep_are_refused() {
        assert!(checked(&file(&nested(MAX_DEPTH))).is_ok());
        let e = checked(&file(&nested(MAX_DEPTH + 1))).unwrap_err();
        assert!(e.to_string().contains("nested deeper"), "{e}");
    }

    #[test]
    fn a_delimiter_outside_a_sequence_is_broken() {
        let data = file(&item((0xFFFE, 0xE00D), 0));
        assert!(checked(&data).is_err());
    }

    #[test]
    fn a_broken_length_in_the_file_meta_information_is_broken() {
        let mut data = vec![0u8; 128];
        data.extend(b"DICM");
        data.extend(long((0x0002, 0x0001), b"OB", 0x5000_0000, &[0, 1]));
        assert!(checked(&data).is_err());
    }

    #[test]
    fn the_head_of_a_longer_file_is_checked_against_the_whole_length() {
        let fits = file(&long((0x0008, 0x1030), b"UT", 4000, b"text"));
        assert!(check(&fits, fits.len() as u64 + 4000, true).is_ok());
        let broken = file(&long((0x0008, 0x1030), b"UT", 0x8000_0000, b"text"));
        assert!(check(&broken, broken.len() as u64 + 4000, true).is_err());
    }
}
