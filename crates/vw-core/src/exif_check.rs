//! EXIF（TIFF の形）の IFD をたどり、壊れている所をタグごとに挙げる。
//!
//! kamadak-exif も壊れた所を飛ばして読めるが、返すエラーは「Truncated field value」のような
//! 文だけで、どのタグかが分からない。ログにタグを出すため、IFD の並びだけを自分でたどる。
//! 値の中身は読まない（表示は kamadak-exif のまま）。

use crate::image::Problem;

/// 壊れたファイルで延々とたどらないための、IFD の数の上限。複数ページの TIFF も 1 ページ 1 つなので、
/// ふつうのファイルはこれに届かない。
const MAX_IFDS: usize = 256;

#[derive(Clone, Copy)]
enum Kind {
    /// 0 番目から続く IFD（TIFF なら 1 ページに 1 つ）。番号を持つ。
    Main(usize),
    Exif,
    Gps,
    Interop,
}

impl Kind {
    fn name(self) -> String {
        match self {
            Kind::Main(n) => format!("IFD{n}"),
            Kind::Exif => "Exif IFD".into(),
            Kind::Gps => "GPS IFD".into(),
            Kind::Interop => "Interop IFD".into(),
        }
    }

    fn context(self) -> exif::Context {
        match self {
            Kind::Main(_) => exif::Context::Tiff,
            Kind::Exif => exif::Context::Exif,
            Kind::Gps => exif::Context::Gps,
            Kind::Interop => exif::Context::Interop,
        }
    }
}

/// 1 つの値の大きさ（バイト）。知らない型は `None`。
fn unit_len(typ: u16) -> Option<u64> {
    match typ {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 | 13 => Some(4),
        5 | 10 | 12 => Some(8),
        _ => None,
    }
}

struct Walk<'a> {
    data: &'a [u8],
    group: &'a str,
    little: bool,
    seen: Vec<usize>,
    problems: Vec<Problem>,
}

impl Walk<'_> {
    fn u16_at(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.data.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    fn problem(&mut self, place: String, error: String) {
        self.problems.push(Problem {
            group: self.group.to_owned(),
            place,
            error,
        });
    }

    /// `at` の IFD を読み、次の IFD の位置を返す（0 なら終わり）。
    fn ifd(&mut self, at: usize, kind: Kind) -> Option<usize> {
        let len = self.data.len();
        if self.seen.contains(&at) {
            self.problem(
                kind.name(),
                format!("IFD at offset {at} was already read (the IFDs point in a loop)"),
            );
            return None;
        }
        if self.seen.len() >= MAX_IFDS {
            return None;
        }
        self.seen.push(at);
        let Some(count) = self.u16_at(at) else {
            self.problem(
                kind.name(),
                format!("IFD offset {at} is past the end of the data ({len} bytes)"),
            );
            return None;
        };
        let entries = at + 2;
        for i in 0..usize::from(count) {
            let entry = entries + i * 12;
            if entry + 12 > len {
                self.problem(
                    kind.name(),
                    format!(
                        "IFD at offset {at} says {count} entries, but the data ends inside entry {i} ({len} bytes)"
                    ),
                );
                return None;
            }
            self.entry(entry, kind);
        }
        let next_at = entries + usize::from(count) * 12;
        match self.u32_at(next_at) {
            Some(next) => Some(next as usize),
            None => {
                self.problem(
                    kind.name(),
                    format!(
                        "the offset of the next IFD (at {next_at}) is past the end of the data ({len} bytes)"
                    ),
                );
                None
            }
        }
    }

    fn entry(&mut self, at: usize, kind: Kind) {
        let (Some(tag), Some(typ), Some(count)) =
            (self.u16_at(at), self.u16_at(at + 2), self.u32_at(at + 4))
        else {
            return;
        };
        let name = exif::Tag(kind.context(), tag).to_string();
        // kamadak-exif は知らないタグを `Tag(Tiff, 50000)` と書く。番号はもう出すので名前は空にする。
        let name = if name.starts_with("Tag(") { "" } else { &name };
        let place = format!("{} (0x{tag:04X}) {name}", kind.name())
            .trim_end()
            .to_owned();
        let Some(unit) = unit_len(typ) else {
            self.problem(
                place,
                format!("unknown value type {typ} (entry at offset {at})"),
            );
            return;
        };
        let size = unit * u64::from(count);
        let len = self.data.len() as u64;
        if size > 4 {
            let Some(offset) = self.u32_at(at + 8) else {
                return;
            };
            if u64::from(offset) + size > len {
                self.problem(
                    place,
                    format!(
                        "value of {count} x {unit} bytes at offset {offset} runs past the end of the data ({len} bytes; entry at offset {at})"
                    ),
                );
                return;
            }
        }
        let child = match (kind, tag) {
            (Kind::Main(_), 0x8769) => Kind::Exif,
            (Kind::Main(_), 0x8825) => Kind::Gps,
            (Kind::Exif, 0xA005) => Kind::Interop,
            _ => return,
        };
        let pointer = match (typ, count) {
            (4 | 13, 1) => self.u32_at(at + 8).map(|p| p as usize),
            (3, 1) => self.u16_at(at + 8).map(usize::from),
            _ => None,
        };
        let Some(pointer) = pointer else {
            self.problem(
                place,
                format!(
                    "pointer to the {} must be one LONG, but is {count} values of type {typ}",
                    child.name()
                ),
            );
            return;
        };
        self.ifd(pointer, child);
    }
}

/// EXIF の `raw`（TIFF の形）の、壊れている所。壊れていなければ空。
pub fn problems(raw: &[u8], group: &str) -> Vec<Problem> {
    let mut walk = Walk {
        data: raw,
        group,
        little: raw.starts_with(b"II"),
        seen: Vec::new(),
        problems: Vec::new(),
    };
    let header_ok =
        (raw.starts_with(b"II") || raw.starts_with(b"MM")) && walk.u16_at(2) == Some(42);
    let Some(first) = walk.u32_at(4).filter(|_| header_ok) else {
        let head: Vec<String> = raw.iter().take(8).map(|b| format!("{b:02X}")).collect();
        walk.problem(
            String::new(),
            format!(
                "not a TIFF header (first bytes {}; {} bytes in all)",
                head.join(" "),
                raw.len()
            ),
        );
        return walk.problems;
    };
    let mut at = first as usize;
    let mut n = 0;
    while at != 0 {
        match walk.ifd(at, Kind::Main(n)) {
            Some(next) => at = next,
            None => break,
        }
        n += 1;
    }
    walk.problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif::sample;

    fn errors(raw: &[u8]) -> Vec<String> {
        problems(raw, "EXIF")
            .iter()
            .map(|p| p.to_string())
            .collect()
    }

    #[test]
    fn a_sound_exif_has_no_problems() {
        assert!(errors(&sample::make(b"Cam")).is_empty());
        assert!(errors(&sample::gps(35.0, 139.0, Some(10.0))).is_empty());
    }

    #[test]
    fn a_value_past_the_end_names_its_tag_and_offset() {
        let mut raw = sample::gps(35.0, 139.0, None);
        raw.truncate(raw.len() - 4);
        let found = errors(&raw);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0]
                .starts_with("EXIF GPS IFD (0x0004) GPSLongitude: value of 3 x 8 bytes at offset"),
            "{found:?}"
        );
    }

    #[test]
    fn an_unknown_type_names_its_tag() {
        let mut raw = sample::make(b"Cam");
        raw[12] = 99; // Make の型
        assert_eq!(
            errors(&raw),
            ["EXIF IFD0 (0x010F) Make: unknown value type 99 (entry at offset 10)"]
        );
    }

    #[test]
    fn an_entry_count_beyond_the_data_is_reported() {
        let mut raw = sample::make(b"Cam");
        raw[8] = 5; // 実際は 1 つ
        let found = errors(&raw);
        assert_eq!(
            found,
            [
                "EXIF IFD0: IFD at offset 8 says 5 entries, but the data ends inside entry 1 (26 bytes)"
            ]
        );
    }

    #[test]
    fn a_loop_of_ifds_is_reported_once() {
        let mut raw = sample::make(b"Cam");
        let next = raw.len() - 4;
        raw[next..].copy_from_slice(&8u32.to_le_bytes()); // 次の IFD が自分を指す
        assert_eq!(
            errors(&raw),
            ["EXIF IFD1: IFD at offset 8 was already read (the IFDs point in a loop)"]
        );
    }

    #[test]
    fn a_broken_header_shows_its_first_bytes() {
        assert_eq!(
            errors(b"XX*\0\x08\0\0\0"),
            ["EXIF: not a TIFF header (first bytes 58 58 2A 00 08 00 00 00; 8 bytes in all)"]
        );
    }
}
