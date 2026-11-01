//! rawler に渡す前に、TIFF の IFD の項目がファイルの中に収まっているか確かめる。
//!
//! rawler は項目の個数（count）をそのまま信じて `vec![0; count]` を作ってから読むので、数バイト
//! 壊れた NEF・ARW で数 GB〜TB を取りに行き、取れずにプロセスごと終わる（確保の失敗は捕まえられない）。
//! rawler を直すのが筋だが、同梱して差し替える（vendor/winit のように）ほどではないので、
//! vw の側で先に歩いて断る。
//!
//! 読む範囲は rawler と同じ、ルートの IFD の連なり、SubIFDs・Exif・GPS、メーカーノート。
//! 「次の IFD」をたどるのはルートの連なりだけで、ほかは IFD を 1 つだけ読む（rawler もそうする）。
//! DNG はメーカーノートを読まない（rawler の DNG のデコーダは読まない）。
//! ファイルの外を指す項目でも小さなものは断らない。rawler はその項目を読めないと警告して飛ばすので、
//! 小さな壊れを許すその振る舞いを変えないため。

use std::collections::HashSet;

use vw_core::DecodeError;

/// これより大きな確保になる項目だけ断る。
const LARGE: u64 = 16 << 20;
/// 歩く IFD の数の上限。壊れた「次の IFD」の輪で止まらなくならないように。
const MAX_IFDS: usize = 256;

const SUB_IFDS: u16 = 0x014A;
const EXIF_IFD: u16 = 0x8769;
const GPS_IFD: u16 = 0x8825;
const MAKER_NOTE: u16 = 0x927C;
const DNG_VERSION: u16 = 0xC612;

/// メーカーノートの見出しと、その後ろの IFD（または II / MM）までのバイト数。
/// OLYMPUS は OLYMP より先に照らす。
const MAKER_NOTE_HEADERS: [(&[u8], u64); 6] = [
    (b"OLYMPUS", 12),
    (b"OLYMP", 8),
    (b"EPSON", 8),
    (b"FUJIFILM", 12),
    (b"SONY DSC ", 12),
    (b"AOC\0", 4),
];

/// 型番号ごとの 1 要素のバイト数（rawler と同じ表。不明な型は 1）。
const TYPE_SIZE: [u64; 13] = [1, 1, 1, 2, 4, 8, 1, 1, 2, 4, 8, 4, 8];

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

struct Walker<'a> {
    data: &'a [u8],
    seen: HashSet<u64>,
    /// IFD0 に DNGVersion がある。メーカーノートを歩かない。
    dng: bool,
}

/// `data` が TIFF なら IFD を歩き、巨大な確保になる項目があれば `Err`。TIFF でなければ何もしない。
pub fn check(data: &[u8]) -> Result<(), DecodeError> {
    let endian = match data.get(..4) {
        Some(b"II*\0") => Endian::Little,
        Some(b"MM\0*") => Endian::Big,
        _ => return Ok(()),
    };
    let mut walker = Walker {
        data,
        seen: HashSet::new(),
        dng: false,
    };
    let first = walker.u32(4, endian).ok_or_else(truncated)?;
    walker.dng = walker.has_tag(u64::from(first), DNG_VERSION, endian);
    walker.chain(0, u64::from(first), endian)
}

fn truncated() -> DecodeError {
    DecodeError::Invalid("TIFF header is truncated".into())
}

impl Walker<'_> {
    fn u16(&self, at: u64, endian: Endian) -> Option<u16> {
        let b = self.data.get(at as usize..(at as usize).checked_add(2)?)?;
        Some(match endian {
            Endian::Little => u16::from_le_bytes([b[0], b[1]]),
            Endian::Big => u16::from_be_bytes([b[0], b[1]]),
        })
    }

    fn u32(&self, at: u64, endian: Endian) -> Option<u32> {
        let b = self.data.get(at as usize..(at as usize).checked_add(4)?)?;
        Some(match endian {
            Endian::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Endian::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        })
    }

    /// `at` の IFD に `tag` の項目があるか。
    fn has_tag(&self, at: u64, tag: u16, endian: Endian) -> bool {
        let Some(count) = self.u16(at, endian) else {
            return false;
        };
        (0..u64::from(count)).any(|i| self.u16(at + 2 + i * 12, endian) == Some(tag))
    }

    /// `base + offset` から始まる IFD だけ。「次の IFD」はたどらない。Canon のメーカーノートのように、
    /// ルート以外の IFD の「次の IFD」の欄には 0 でない値（画像のデータの中）を書くものがあり、
    /// rawler もたどらないため。
    fn single(&mut self, base: u64, offset: u64, endian: Endian) -> Result<(), DecodeError> {
        if offset == 0 || self.seen.len() >= MAX_IFDS || !self.seen.insert(base + offset) {
            return Ok(());
        }
        self.ifd(base, offset, endian).map(drop)
    }

    /// ルートの、`base + offset` から始まる IFD と、その「次の IFD」の連なり。
    fn chain(&mut self, base: u64, offset: u64, endian: Endian) -> Result<(), DecodeError> {
        let mut next = offset;
        while next != 0 {
            if self.seen.len() >= MAX_IFDS || !self.seen.insert(base + next) {
                return Ok(());
            }
            next = self.ifd(base, next, endian)?;
        }
        Ok(())
    }

    /// 1 つの IFD の項目を確かめ、次の IFD の位置を返す（なければ 0）。
    fn ifd(&mut self, base: u64, offset: u64, endian: Endian) -> Result<u64, DecodeError> {
        let at = base + offset;
        let Some(count) = self.u16(at, endian) else {
            return Ok(0);
        };
        let entries = at + 2;
        for i in 0..u64::from(count) {
            let entry = entries + i * 12;
            let (Some(tag), Some(kind), Some(n)) = (
                self.u16(entry, endian),
                self.u16(entry + 2, endian),
                self.u32(entry + 4, endian),
            ) else {
                return Ok(0);
            };
            let size = u64::from(n) * TYPE_SIZE.get(usize::from(kind)).copied().unwrap_or(1);
            let value_at = if size <= 4 {
                entry + 8
            } else {
                let Some(o) = self.u32(entry + 8, endian) else {
                    return Ok(0);
                };
                let value_at = base + u64::from(o);
                if size > LARGE && value_at + size > self.data.len() as u64 {
                    return Err(DecodeError::Invalid(format!(
                        "TIFF tag 0x{tag:04X} claims {size} bytes beyond the end of the file"
                    )));
                }
                value_at
            };
            match tag {
                SUB_IFDS | EXIF_IFD | GPS_IFD if kind == 4 || kind == 13 => {
                    for k in 0..u64::from(n).min(16) {
                        if let Some(sub) = self.u32(value_at + k * 4, endian) {
                            self.single(base, u64::from(sub), endian)?;
                        }
                    }
                }
                MAKER_NOTE if kind == 7 && !self.dng => {
                    self.maker_note(base, value_at, size, endian)?;
                }
                _ => {}
            }
        }
        Ok(self
            .u32(entries + u64::from(count) * 12, endian)
            .map_or(0, u64::from))
    }

    /// メーカーノートの中の IFD。見出しの表は rawler 0.8.0 の `IFD::parse_makernote` と同じにする。
    /// 表にない英字の見出し（SIGMA・LEICA など）は歩かない。そうした見出しを IFD として読むと、
    /// 文字を項目数や個数と取り違えて「巨大な項目」を作り、壊れていないファイルを断ってしまう。
    /// rawler は DNG などのメーカーノートを読まないので、歩かなくても守る範囲は減らない。
    fn maker_note(
        &mut self,
        base: u64,
        at: u64,
        size: u64,
        endian: Endian,
    ) -> Result<(), DecodeError> {
        let Some(head) = self.data.get(at as usize..(at as usize).saturating_add(16)) else {
            return Ok(());
        };
        let endian_at = |off: u64| match self.data.get((at + off) as usize..(at + off) as usize + 2)
        {
            Some(b"II") => Some(Endian::Little),
            Some(b"MM") => Some(Endian::Big),
            _ => None,
        };
        if head.starts_with(b"Nikon\0\x02") {
            // 10 バイトの印の後ろに TIFF の見出しがあり、位置はそこから数える。
            let endian = endian_at(10).unwrap_or(Endian::Big);
            return self.single(at + 10, 8, endian);
        }
        if head.starts_with(b"PENTAX") {
            // IFD は 10 バイト目から。値の位置はメーカーノートの先頭から数える。
            let endian = endian_at(8).unwrap_or(Endian::Big);
            return self.single(at, 10, endian);
        }
        let skip = MAKER_NOTE_HEADERS
            .iter()
            .find(|(mark, _)| head.starts_with(mark))
            .map(|&(_, skip)| skip);
        let mut off = match skip {
            Some(skip) => skip,
            None if endian_at(0).is_none() && head[..3].iter().all(u8::is_ascii_alphabetic) => {
                return Ok(());
            }
            None => 0,
        };
        let mut endian = endian;
        if let Some(e) = endian_at(off) {
            off += 2;
            endian = e;
        }
        if off < size {
            self.single(base, at + off - base, endian)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 項目 `entries`（タグ, 型, 個数, 値または位置）を 1 つの IFD に持つリトルエンディアンの TIFF。
    fn tiff(entries: &[(u16, u16, u32, u32)]) -> Vec<u8> {
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        out.extend((entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in entries {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            out.extend(value.to_le_bytes());
        }
        out.extend(0u32.to_le_bytes());
        out
    }

    #[test]
    fn plain_tiff_with_small_tags_passes() {
        let data = tiff(&[(256, 4, 1, 100), (257, 4, 1, 50)]);
        assert!(check(&data).is_ok());
    }

    #[test]
    fn tag_claiming_gigabytes_beyond_the_file_is_refused() {
        // 1,700,000,000 個の LONG（6.8 GB）を名乗る項目。
        let data = tiff(&[(0x9003, 4, 1_700_000_000, 8)]);
        let err = check(&data).unwrap_err().to_string();
        assert!(err.contains("0x9003"), "{err}");
    }

    #[test]
    fn small_tag_beyond_the_file_is_left_for_rawler_to_skip() {
        let data = tiff(&[(0x9003, 4, 100, 100_000)]);
        assert!(check(&data).is_ok());
    }

    #[test]
    fn huge_tag_inside_a_sub_ifd_is_found() {
        let mut data = tiff(&[(EXIF_IFD, 4, 1, 0)]);
        let exif_at = data.len() as u32;
        data[18..22].copy_from_slice(&exif_at.to_le_bytes());
        data.extend(1u16.to_le_bytes());
        data.extend(0x9003u16.to_le_bytes());
        data.extend(4u16.to_le_bytes());
        data.extend(1_700_000_000u32.to_le_bytes());
        data.extend(8u32.to_le_bytes());
        data.extend(0u32.to_le_bytes());
        assert!(check(&data).is_err());
    }

    #[test]
    fn ifd_loop_terminates() {
        let mut data = tiff(&[(256, 4, 1, 1)]);
        let len = data.len();
        data[len - 4..].copy_from_slice(&8u32.to_le_bytes());
        assert!(check(&data).is_ok());
    }

    #[test]
    fn non_tiff_data_is_not_checked() {
        assert!(check(b"FUJIFILMCCD-RAW 0201").is_ok());
    }

    /// IFD0 にメーカーノート（型 7）だけを持ち、その中身を `note` にした TIFF。
    fn with_maker_note(note: &[u8]) -> Vec<u8> {
        let mut data = tiff(&[(MAKER_NOTE, 7, note.len() as u32, 0)]);
        let note_at = data.len() as u32;
        data[18..22].copy_from_slice(&note_at.to_le_bytes());
        data.extend(note);
        data
    }

    /// 1 項目で 6.8 GB を名乗る IFD（位置はファイルの外）。
    fn huge_ifd() -> Vec<u8> {
        let mut ifd = 1u16.to_le_bytes().to_vec();
        ifd.extend(0x9003u16.to_le_bytes());
        ifd.extend(4u16.to_le_bytes());
        ifd.extend(1_700_000_000u32.to_le_bytes());
        ifd.extend(8u32.to_le_bytes());
        ifd.extend(0u32.to_le_bytes());
        ifd
    }

    #[test]
    fn sigma_maker_note_is_not_read_as_an_ifd() {
        // SIGMA fp の DNG と同じ先頭。IFD として読むと 0x4D47 が 67,174,400 バイトを名乗る。
        let mut note = b"SIGMA\0\0\0\x01\x04".to_vec();
        note.extend(huge_ifd());
        note.resize(64, 0);
        assert!(check(&with_maker_note(&note)).is_ok());
    }

    #[test]
    fn ifd_after_pentax_aoc_header_is_walked() {
        let mut note = b"AOC\0II".to_vec();
        note.extend(huge_ifd());
        let err = check(&with_maker_note(&note)).unwrap_err().to_string();
        assert!(err.contains("0x9003"), "{err}");
    }

    #[test]
    fn ifd_after_sony_header_is_walked() {
        let mut note = b"SONY DSC \0\0\0".to_vec();
        note.extend(huge_ifd());
        assert!(check(&with_maker_note(&note)).is_err());
    }

    #[test]
    fn maker_note_without_header_is_walked_as_an_ifd() {
        assert!(check(&with_maker_note(&huge_ifd())).is_err());
    }

    /// 小さな項目 1 つと、「次の IFD」の欄に `next` を持つ IFD。
    fn small_ifd(next: u32) -> Vec<u8> {
        let mut ifd = 1u16.to_le_bytes().to_vec();
        ifd.extend(256u16.to_le_bytes());
        ifd.extend(4u16.to_le_bytes());
        ifd.extend(1u32.to_le_bytes());
        ifd.extend(1u32.to_le_bytes());
        ifd.extend(next.to_le_bytes());
        ifd
    }

    #[test]
    fn next_ifd_of_a_maker_note_is_not_followed() {
        // Canon の CR2 のように、メーカーノートの「次の IFD」の欄が画像のデータの中を指す。
        let note_at = tiff(&[(MAKER_NOTE, 7, 0, 0)]).len() as u32;
        let garbage_at = note_at + small_ifd(0).len() as u32;
        let mut note = small_ifd(garbage_at);
        note.extend(huge_ifd());
        assert!(check(&with_maker_note(&note)).is_ok());
    }

    #[test]
    fn next_ifd_of_an_exif_ifd_is_not_followed() {
        let mut data = tiff(&[(EXIF_IFD, 4, 1, 0)]);
        let exif_at = data.len() as u32;
        data[18..22].copy_from_slice(&exif_at.to_le_bytes());
        let garbage_at = exif_at + small_ifd(0).len() as u32;
        data.extend(small_ifd(garbage_at));
        data.extend(huge_ifd());
        assert!(check(&data).is_ok());
    }

    #[test]
    fn huge_tag_in_the_second_root_ifd_is_refused() {
        let mut data = tiff(&[(256, 4, 1, 1)]);
        let len = data.len();
        data[len - 4..].copy_from_slice(&(len as u32).to_le_bytes());
        data.extend(huge_ifd());
        assert!(check(&data).is_err());
    }

    #[test]
    fn maker_note_of_a_dng_is_not_walked() {
        let mut data = tiff(&[(MAKER_NOTE, 7, 0, 0), (DNG_VERSION, 1, 4, 0x0104)]);
        let note = huge_ifd();
        let note_at = data.len() as u32;
        data[14..18].copy_from_slice(&(note.len() as u32).to_le_bytes());
        data[18..22].copy_from_slice(&note_at.to_le_bytes());
        data.extend(note);
        assert!(check(&data).is_ok());
    }
}
