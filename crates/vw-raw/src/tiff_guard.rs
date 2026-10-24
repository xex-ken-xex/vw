//! rawler に渡す前に、TIFF の IFD の項目がファイルの中に収まっているか確かめる。
//!
//! rawler は項目の個数（count）をそのまま信じて `vec![0; count]` を作ってから読むので、数バイト
//! 壊れた NEF・ARW で数 GB〜TB を取りに行き、取れずにプロセスごと終わる（確保の失敗は捕まえられない）。
//! rawler を直すのが筋だが、同梱して差し替える（vendor/winit のように）ほどではないので、
//! vw の側で先に歩いて断る。
//!
//! 読む範囲は rawler と同じ、ルートの IFD の連なり、SubIFDs・Exif・GPS、メーカーノート。
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
    };
    let first = walker.u32(4, endian).ok_or_else(truncated)?;
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

    /// `base + offset` から始まる IFD と、その「次の IFD」の連なり。
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
                            self.chain(base, u64::from(sub), endian)?;
                        }
                    }
                }
                MAKER_NOTE if kind == 7 => self.maker_note(base, value_at, size, endian)?,
                _ => {}
            }
        }
        Ok(self
            .u32(entries + u64::from(count) * 12, endian)
            .map_or(0, u64::from))
    }

    /// メーカーノートの中の IFD。rawler が読むメーカーのうち、vw が扱う Nikon と Sony のほか、
    /// 先頭に II / MM を置く一般の形。
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
        if head.starts_with(b"Nikon\0\x02") {
            // 10 バイトの印の後ろに TIFF の見出しがあり、位置はそこから数える。
            let inner = at + 10;
            let endian = if self.data.get(inner as usize..inner as usize + 2) == Some(b"II") {
                Endian::Little
            } else {
                Endian::Big
            };
            return self.chain(inner, 8, endian);
        }
        let mut off = 0u64;
        if head.starts_with(b"SONY DSC ") {
            off += 12;
        }
        let mut endian = endian;
        match self.data.get((at + off) as usize..(at + off) as usize + 2) {
            Some(b"II") => {
                off += 2;
                endian = Endian::Little;
            }
            Some(b"MM") => {
                off += 2;
                endian = Endian::Big;
            }
            _ => {}
        }
        if off < size {
            self.chain(base, at + off - base, endian)?;
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
}
