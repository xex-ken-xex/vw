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
    if let Some(gps) = gps(&exif) {
        gps.push_to(m);
    }
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .unwrap_or(1)
}

/// 撮影地点。緯度・経度は度（北緯・東経が正）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gps {
    pub latitude: f64,
    pub longitude: f64,
    /// 海抜（メートル、海面下は負）。
    pub altitude: Option<f64>,
}

/// メタデータ欄で撮影地点を出すグループ。度と分と秒で書かれた EXIF の値を、地図や検索で
/// そのまま使える 10 進の度にして並べる。
pub const LOCATION: &str = "Location";

impl Gps {
    pub fn push_to(&self, m: &mut Metadata) {
        m.push(LOCATION, "Latitude", format!("{:.6}", self.latitude));
        m.push(LOCATION, "Longitude", format!("{:.6}", self.longitude));
        if let Some(a) = self.altitude {
            m.push(LOCATION, "Altitude", format!("{a:.1} m"));
        }
    }

    /// [`Gps::push_to`] で書いた値から読み戻す。
    pub fn from_metadata(m: &Metadata) -> Option<Self> {
        let number = |key| m.get(LOCATION, key)?.parse::<f64>().ok();
        Self::new(number("Latitude")?, number("Longitude")?, None)
    }

    /// 範囲の外（壊れた EXIF）なら `None`。0, 0 も `None` にする。GPS が測れなかったときに
    /// 0 を書く機器があり、地図でギニア湾に点が集まってしまうため。
    pub fn new(latitude: f64, longitude: f64, altitude: Option<f64>) -> Option<Self> {
        let valid = latitude.abs() <= 90.0
            && longitude.abs() <= 180.0
            && latitude.is_finite()
            && longitude.is_finite()
            && (latitude, longitude) != (0.0, 0.0);
        valid.then_some(Self {
            latitude,
            longitude,
            altitude: altitude.filter(|a| a.is_finite()),
        })
    }
}

/// EXIF の GPS のタグから撮影地点を読む。
pub fn gps(exif: &exif::Exif) -> Option<Gps> {
    use exif::{In, Tag, Value};
    let field = |tag| exif.get_field(tag, In::PRIMARY);
    let degrees = |tag, reference, negative: u8| {
        let Value::Rational(parts) = &field(tag)?.value else {
            return None;
        };
        let d = parts
            .iter()
            .zip([1.0, 60.0, 3600.0])
            .map(|(r, unit)| r.to_f64() / unit)
            .sum::<f64>();
        let sign = match &field(reference)?.value {
            Value::Ascii(v) if v.first()?.first() == Some(&negative) => -1.0,
            _ => 1.0,
        };
        Some(d * sign)
    };
    let altitude = match field(Tag::GPSAltitude).map(|f| &f.value) {
        Some(Value::Rational(r)) if !r.is_empty() => {
            let below = field(Tag::GPSAltitudeRef).and_then(|f| f.value.get_uint(0)) == Some(1);
            Some(if below { -r[0].to_f64() } else { r[0].to_f64() })
        }
        _ => None,
    };
    Gps::new(
        degrees(Tag::GPSLatitude, Tag::GPSLatitudeRef, b'S')?,
        degrees(Tag::GPSLongitude, Tag::GPSLongitudeRef, b'W')?,
        altitude,
    )
}

/// ファイルの EXIF から撮影地点を読む。画像は読まない（地図に点を出すために多くのファイルを
/// 見るので、画素を読む時間をかけないため）。撮影日時（DateTimeOriginal）も返す。
pub fn gps_of_file(path: &std::path::Path) -> Option<(Gps, Option<String>)> {
    let file = std::fs::File::open(path).ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()?;
    let taken = exif
        .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        .map(|f| value(f, &exif));
    Some((gps(&exif)?, taken))
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

    /// GPS の緯度・経度・高度だけを持つ EXIF。南緯・西経は負の値で渡す。
    pub fn gps(latitude: f64, longitude: f64, altitude: Option<f64>) -> Vec<u8> {
        // 度・分・秒の 3 つの分数（秒は 1/1000 まで）。
        let dms = |v: f64| {
            let v = v.abs();
            let d = v.trunc();
            let m = ((v - d) * 60.0).trunc();
            let s = ((v - d) * 60.0 - m) * 60.0;
            [
                (d as u32, 1),
                (m as u32, 1),
                ((s * 1000.0).round() as u32, 1000),
            ]
        };
        let mut gps: Vec<(u16, u16, u32, Vec<u8>)> = vec![
            (1, 2, 2, vec![if latitude < 0.0 { b'S' } else { b'N' }, 0]),
            (2, 5, 3, rationals(&dms(latitude))),
            (3, 2, 2, vec![if longitude < 0.0 { b'W' } else { b'E' }, 0]),
            (4, 5, 3, rationals(&dms(longitude))),
        ];
        if let Some(a) = altitude {
            gps.push((5, 1, 1, vec![u8::from(a < 0.0)]));
            gps.push((6, 5, 1, rationals(&[((a.abs() * 10.0).round() as u32, 10)])));
        }
        // 0 番目の IFD は GPS の IFD の位置（GPSInfo、0x8825）だけを持つ。
        let ifd0: u32 = 8;
        let gps_ifd = ifd0 + 2 + 12 + 4;
        let mut t = b"II*\0".to_vec();
        t.extend(ifd0.to_le_bytes());
        t.extend(1u16.to_le_bytes());
        t.extend(0x8825u16.to_le_bytes());
        t.extend(4u16.to_le_bytes()); // LONG
        t.extend(1u32.to_le_bytes());
        t.extend(gps_ifd.to_le_bytes());
        t.extend(0u32.to_le_bytes());
        // 4 バイトに収まらない値は、IFD の後ろに順に置く。
        let mut data_at = gps_ifd + 2 + 12 * gps.len() as u32 + 4;
        let mut data: Vec<u8> = Vec::new();
        t.extend((gps.len() as u16).to_le_bytes());
        for (tag, kind, count, bytes) in &gps {
            t.extend(tag.to_le_bytes());
            t.extend(kind.to_le_bytes());
            t.extend(count.to_le_bytes());
            if bytes.len() <= 4 {
                let mut inline = bytes.clone();
                inline.resize(4, 0);
                t.extend(inline);
            } else {
                t.extend(data_at.to_le_bytes());
                data_at += bytes.len() as u32;
                data.extend(bytes);
            }
        }
        t.extend(0u32.to_le_bytes());
        t.extend(data);
        t
    }

    fn rationals(parts: &[(u32, u32)]) -> Vec<u8> {
        parts
            .iter()
            .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
            .collect()
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
    fn gps_is_listed_as_decimal_degrees() {
        let mut m = Metadata::default();
        push_exif(&mut m, sample::gps(35.681236, 139.767125, Some(3.5)));
        assert_eq!(m.get(LOCATION, "Latitude"), Some("35.681236"));
        assert_eq!(m.get(LOCATION, "Longitude"), Some("139.767125"));
        assert_eq!(m.get(LOCATION, "Altitude"), Some("3.5 m"));
    }

    #[test]
    fn south_west_and_below_sea_level_are_negative() {
        let raw = sample::gps(-33.8688, -70.6693, Some(-12.0));
        let exif = exif::Reader::new().read_raw(raw).unwrap();
        let g = gps(&exif).unwrap();
        assert!((g.latitude + 33.8688).abs() < 1e-6);
        assert!((g.longitude + 70.6693).abs() < 1e-6);
        assert_eq!(g.altitude, Some(-12.0));
    }

    #[test]
    fn zero_zero_is_treated_as_no_position() {
        let mut m = Metadata::default();
        push_exif(&mut m, sample::gps(0.0, 0.0, None));
        assert_eq!(m.get(LOCATION, "Latitude"), None);
    }

    #[test]
    fn position_is_read_back_from_the_metadata() {
        let mut m = Metadata::default();
        push_exif(&mut m, sample::gps(43.0621, 141.3544, None));
        let g = Gps::from_metadata(&m).unwrap();
        assert!((g.latitude - 43.0621).abs() < 1e-6);
    }

    #[test]
    fn broken_exif_is_ignored_and_means_upright() {
        let mut m = Metadata::default();
        assert_eq!(push_exif(&mut m, b"II*\0garbage".to_vec()), 1);
        assert!(m.entries.is_empty());
    }
}
