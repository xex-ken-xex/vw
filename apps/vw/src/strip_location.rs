//! 書き出す画像のファイルから、撮影地点（EXIF の GPS、XMP の地点と地名、FITS の観測地）を消す。
//!
//! 消し方は、値のあった所を 0 や空白で塗りつぶし、ファイルの長さと並びを変えない。
//! 並べ直して詰めないのは、TIFF や TIFF の形の RAW では画素の置き場所がファイルの頭からの
//! 位置で書かれていて、詰めると位置を全部書き直すことになり、メーカー独自の部分まで読み解かないと
//! 壊してしまうため。

use std::ops::Range;

/// 消したもの。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Removed {
    pub gps: bool,
    pub xmp: usize,
    pub fits_cards: usize,
}

impl Removed {
    pub fn any(&self) -> bool {
        self.gps || self.xmp > 0 || self.fits_cards > 0
    }
}

/// `bytes` から撮影地点を消す。読み方の分からない形式なら `Err`（消せたか確かめられないので、
/// 黙って写さないよう呼ぶ側で知らせる）。地点がなかったときも `Ok` で、`Removed::any` が偽。
pub fn strip(bytes: &mut [u8]) -> Result<Removed, String> {
    let mut removed = Removed::default();
    if bytes.starts_with(&[0xFF, 0xD8]) {
        strip_jpeg(bytes, &mut removed);
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        strip_png(bytes, &mut removed);
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        strip_webp(bytes, &mut removed);
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        // TIFF と、TIFF の形の RAW（NEF・ARW・DNG など）。
        removed.gps |= strip_tiff_gps(bytes);
        removed.xmp += blank_xmp(bytes);
    } else if bytes.starts_with(b"SIMPLE  =") {
        removed.fits_cards += strip_fits(bytes);
    } else if bytes.starts_with(b"BM")
        || bytes.starts_with(b"GIF8")
        || bytes.get(128..132) == Some(b"DICM")
    {
        // 撮影地点を入れる場所のない形式。
    } else {
        return Err("location cannot be removed from this file format".into());
    }
    Ok(removed)
}

fn strip_jpeg(bytes: &mut [u8], removed: &mut Removed) {
    let mut i = 2;
    while i + 4 <= bytes.len() && bytes[i] == 0xFF {
        let marker = bytes[i + 1];
        // SOS のあとは画素のデータで、セグメントは続かない。
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        let body = i + 4..(i + 2 + len).min(bytes.len());
        if marker == 0xE1 {
            let seg = &mut bytes[body.clone()];
            if seg.starts_with(b"Exif\0\0") {
                removed.gps |= strip_tiff_gps(&mut seg[6..]);
            } else {
                removed.xmp += blank_xmp(seg);
            }
        }
        i = body.end;
    }
}

fn strip_png(bytes: &mut [u8], removed: &mut Removed) {
    let mut i = 8;
    while i + 12 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        let end = i + 8 + len;
        if end + 4 > bytes.len() {
            break;
        }
        let kind: [u8; 4] = bytes[i + 4..i + 8].try_into().unwrap();
        let changed = match &kind {
            b"eXIf" => strip_tiff_gps(&mut bytes[i + 8..end]),
            b"iTXt" | b"tEXt" => {
                let n = blank_xmp(&mut bytes[i + 8..end]);
                removed.xmp += n;
                n > 0
            }
            _ => false,
        };
        if kind == *b"eXIf" {
            removed.gps |= changed;
        }
        if changed {
            let crc = crc32(&bytes[i + 4..end]);
            bytes[end..end + 4].copy_from_slice(&crc.to_be_bytes());
        }
        i = end + 4;
    }
}

fn strip_webp(bytes: &mut [u8], removed: &mut Removed) {
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        let body = i + 8..(i + 8 + len).min(bytes.len());
        match &bytes[i..i + 4] {
            b"EXIF" => {
                let data = &mut bytes[body.clone()];
                // 頭に `Exif\0\0` を付けて書くソフトもある。
                let tiff = if data.starts_with(b"Exif\0\0") {
                    &mut data[6..]
                } else {
                    data
                };
                removed.gps |= strip_tiff_gps(tiff);
            }
            b"XMP " => removed.xmp += blank_xmp(&mut bytes[body.clone()]),
            _ => {}
        }
        i = body.end + (len & 1);
    }
}

/// TIFF の形のデータ（EXIF）の GPS の IFD を 0 で塗りつぶし、IFD0 からの指し先を外す。
fn strip_tiff_gps(tiff: &mut [u8]) -> bool {
    let Some(t) = Tiff::new(tiff) else {
        return false;
    };
    let Some(ifd0) = t.u32(4).map(|o| o as usize) else {
        return false;
    };
    let Some(count) = t.u16(ifd0).map(usize::from) else {
        return false;
    };
    let entry = |k: usize| ifd0 + 2 + 12 * k;
    let Some(k) = (0..count).find(|&k| t.u16(entry(k)) == Some(GPS_IFD)) else {
        return false;
    };
    let gps = t.u32(entry(k) + 8).map(|o| o as usize);
    let little = t.little;
    // 指し先が頭の 8 バイト（TIFF の見出し）や外を指す壊れた EXIF では、IFD とみなして塗ると見出しを消してしまう。
    if let Some(gps) = gps.filter(|&g| g >= 8 && g < tiff.len()) {
        for range in ifd_ranges(&Tiff { data: tiff, little }, gps) {
            tiff[range].fill(0);
        }
    }
    // 指し先の項目を詰めて数を 1 つ減らす。項目は番号の順に並べる決まりなので、
    // 0 で塗るだけにせず抜く（番号 0 の項目が残ると、読むソフトによっては壊れたとみなす）。
    let end = entry(count) + 4;
    if end > tiff.len() {
        return false;
    }
    tiff.copy_within(entry(k + 1)..end, entry(k));
    tiff[end - 12..end].fill(0);
    let n = (count - 1) as u16;
    let n = if little {
        n.to_le_bytes()
    } else {
        n.to_be_bytes()
    };
    tiff[ifd0..ifd0 + 2].copy_from_slice(&n);
    true
}

const GPS_IFD: u16 = 0x8825;

struct Tiff<'a> {
    data: &'a [u8],
    little: bool,
}

impl<'a> Tiff<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        let little = match data.get(..4)? {
            b"II*\0" => true,
            b"MM\0*" => false,
            _ => return None,
        };
        Some(Self { data, little })
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.data.get(at..at + 2)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(at..at + 4)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }
}

/// IFD の項目の並びと、項目に入りきらずに外に置いた値の場所。
fn ifd_ranges(t: &Tiff, at: usize) -> Vec<Range<usize>> {
    let Some(count) = t.u16(at).map(usize::from) else {
        return Vec::new();
    };
    let table = at..at + 2 + 12 * count + 4;
    if table.end > t.data.len() {
        return Vec::new();
    }
    let mut out = vec![table];
    for k in 0..count {
        let e = at + 2 + 12 * k;
        let (Some(kind), Some(n)) = (t.u16(e + 2), t.u32(e + 4)) else {
            break;
        };
        let size = match kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => continue,
        } * n as usize;
        if size > 4 {
            if let Some(o) = t.u32(e + 8).map(|o| o as usize) {
                if o + size <= t.data.len() {
                    out.push(o..o + size);
                }
            }
        }
    }
    out
}

/// XMP の中の、地点と地名の項目。
const XMP_PLACES: [&str; 7] = [
    "exif:GPS",
    "photoshop:City",
    "photoshop:State",
    "photoshop:Country",
    "Iptc4xmpCore:Location",
    "Iptc4xmpCore:CountryCode",
    "Iptc4xmpExt:LocationCreated",
];

/// `<x:xmpmeta` から `</x:xmpmeta>` までの中で、[`XMP_PLACES`] で始まる名前の属性と要素を空白にする。
/// 消した数を返す。
fn blank_xmp(bytes: &mut [u8]) -> usize {
    let mut blanked = 0;
    let mut from = 0;
    while let Some(start) = find(&bytes[from..], b"<x:xmpmeta").map(|i| from + i) {
        let end = find(&bytes[start..], b"</x:xmpmeta>").map_or(bytes.len(), |i| start + i);
        for range in xmp_places(&bytes[start..end]) {
            bytes[start + range.start..start + range.end].fill(b' ');
            blanked += 1;
        }
        from = end;
    }
    blanked
}

/// XMP の文字の中で、地点の属性（`exif:GPSLatitude="..."`）と要素（`<photoshop:City>...</photoshop:City>`）の場所。
fn xmp_places(xmp: &[u8]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < xmp.len() {
        let Some(name) = XMP_PLACES
            .iter()
            .find(|p| xmp[i..].starts_with(p.as_bytes()))
            .map(|_| name_at(xmp, i))
        else {
            i += 1;
            continue;
        };
        let opened = i > 0 && xmp[i - 1] == b'<';
        let after = i + name.len();
        if opened {
            // 要素。同じ名前の閉じタグまで（入れ子の rdf:Seq なども含めて）。
            let close = [b"</".as_slice(), name, b">"].concat();
            let end = match find(&xmp[after..], &close) {
                Some(j) => after + j + close.len(),
                None => match find(&xmp[after..], b"/>") {
                    Some(j) => after + j + 2,
                    None => break,
                },
            };
            out.push(i - 1..end);
            i = end;
        } else if let Some(end) = attribute_end(xmp, after) {
            out.push(i..end);
            i = end;
        } else {
            i = after;
        }
    }
    out
}

fn name_at(xmp: &[u8], at: usize) -> &[u8] {
    let len = xmp[at..]
        .iter()
        .position(|&b| !(b.is_ascii_alphanumeric() || b == b':' || b == b'_' || b == b'-'))
        .unwrap_or(xmp.len() - at);
    &xmp[at..at + len]
}

/// `="..."` または `='...'` の終わり。属性でなければ `None`。
fn attribute_end(xmp: &[u8], at: usize) -> Option<usize> {
    let mut j = at;
    while xmp.get(j)?.is_ascii_whitespace() {
        j += 1;
    }
    if *xmp.get(j)? != b'=' {
        return None;
    }
    j += 1;
    while xmp.get(j)?.is_ascii_whitespace() {
        j += 1;
    }
    let quote = *xmp.get(j)?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let close = xmp[j + 1..].iter().position(|&b| b == quote)?;
    Some(j + 1 + close + 1)
}

/// FITS の観測地のカードを、空白だけのカード（何も表さない）にする。ヘッダーの終わり（END）まで見る。
fn strip_fits(bytes: &mut [u8]) -> usize {
    const SITE: [&str; 9] = [
        "SITELAT", "SITELONG", "SITEELEV", "OBSGEO-B", "OBSGEO-L", "OBSGEO-H", "OBSGEO-X",
        "OBSGEO-Y", "OBSGEO-Z",
    ];
    const MORE: [&str; 4] = ["LAT-OBS", "LONG-OBS", "ALT-OBS", "OBSERVAT"];
    let mut n = 0;
    for card in bytes.chunks_mut(80) {
        if card.len() < 80 || card.starts_with(b"END     ") {
            break;
        }
        let key = std::str::from_utf8(&card[..8]).unwrap_or("").trim_end();
        if SITE.contains(&key) || MORE.contains(&key) {
            card.fill(b' ');
            n += 1;
        }
    }
    n
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests;
