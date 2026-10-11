//! カメラ RAW（Nikon NEF・Sony ARW・富士フイルム RAF など）のデコーダプラグイン。
//!
//! RAW に埋め込まれたプレビュー JPEG を表示する。センサーのデータを現像するのは、
//! プレビューがないときだけ。
//!
//! 毎回現像しないのは、4500 万画素級の RAW の現像には 1 枚で数秒かかり、フォルダを
//! 開いたときのサムネイルやスライドショーに間に合わないため。プレビュー JPEG は
//! カメラが自分で現像したもので、多くの機種ではセンサーと同じ大きさで入っている。

use image::DynamicImage;
use rawler::decoders::{Decoder, RawDecodeParams, RawMetadata};
use rawler::formats::tiff::{Rational, SRational};
use rawler::imgop::develop::RawDevelop;
use rawler::rawsource::RawSource;
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

/// 富士フイルムの RAF の先頭。
const RAF_SIGNATURE: &[u8] = b"FUJIFILMCCD-RAW";

pub struct RawPlugin;

impl DecoderPlugin for RawPlugin {
    fn name(&self) -> &str {
        "RAW"
    }

    fn extensions(&self) -> &[&str] {
        &[
            "nef", "nrw", // Nikon
            "arw", "sr2", "srf", // Sony
            "raf", // 富士フイルム
            "cr2", "cr3", // Canon
            "dng", // Adobe（スマホや Leica、Pentax など）
        ]
    }

    /// RAF だけ中身で判定する。NEF・ARW・CR2・DNG は先頭が TIFF と同じで、中身では
    /// TIFF の画像と区別できないため拡張子に任せる。TIFF まで受け持つと、RAW でない
    /// TIFF を開いたときに分かりにくいエラーになる。
    fn probe(&self, header: &[u8]) -> bool {
        header.starts_with(RAF_SIGNATURE)
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        let source = RawSource::new_from_slice(data);
        let decoder = decoder_for(&source)?;
        let params = RawDecodeParams::default();
        let raw_metadata = decoder.raw_metadata(&source, &params).ok();
        let (mut img, shown) = match decoder.preview_image(&source, &params) {
            Ok(Some(preview)) => (from_dynamic(preview)?, Shown::Preview),
            _ => (
                develop(decoder.as_ref(), &source, &params)?,
                Shown::Developed,
            ),
        };
        let orientation = raw_metadata
            .as_ref()
            .and_then(|m| m.exif.orientation)
            .unwrap_or(1);
        img.metadata = metadata(raw_metadata.as_ref(), Some(shown));
        img.apply_orientation(orientation.into());
        Ok(img)
    }

    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        let source = RawSource::new_from_slice(data);
        let decoder = decoder_for(&source)?;
        let raw_metadata = decoder
            .raw_metadata(&source, &RawDecodeParams::default())
            .map_err(invalid)?;
        Ok(metadata(Some(&raw_metadata), None))
    }
}

/// 表示した画像がどちらか。
#[derive(Clone, Copy)]
enum Shown {
    /// カメラが埋め込んだプレビュー JPEG。
    Preview,
    /// センサーのデータを vw で現像したもの。
    Developed,
}

/// センサーのデータを現像する。プレビューがあっても現像したいとき用。
pub fn develop_bytes(data: &[u8]) -> Result<VwImage, DecodeError> {
    let source = RawSource::new_from_slice(data);
    let decoder = decoder_for(&source)?;
    let params = RawDecodeParams::default();
    let raw_metadata = decoder.raw_metadata(&source, &params).ok();
    let mut img = develop(decoder.as_ref(), &source, &params)?;
    let orientation = raw_metadata
        .as_ref()
        .and_then(|m| m.exif.orientation)
        .unwrap_or(1);
    img.metadata = metadata(raw_metadata.as_ref(), Some(Shown::Developed));
    img.apply_orientation(orientation.into());
    Ok(img)
}

/// センサーのデータを現像し、16bit の RGB にする。
fn develop(
    decoder: &dyn Decoder,
    source: &RawSource,
    params: &RawDecodeParams,
) -> Result<VwImage, DecodeError> {
    let raw = decoder.raw_image(source, params, false).map_err(invalid)?;
    let developed = RawDevelop::default()
        .develop_intermediate(&raw)
        .map_err(invalid)?
        .to_dynamic_image()
        .ok_or_else(|| DecodeError::Invalid("developed image has no pixels".into()))?;
    from_dynamic(developed)
}

/// `image` の画像を vw の形にする。8bit と 16bit はそのまま、それ以外は 8bit か 16bit に寄せる。
fn from_dynamic(img: DynamicImage) -> Result<VwImage, DecodeError> {
    let (w, h) = (img.width(), img.height());
    let (color, data) = match img {
        DynamicImage::ImageLuma8(b) => (ColorModel::Gray, PixelData::U8(b.into_raw())),
        DynamicImage::ImageRgb8(b) => (ColorModel::Rgb, PixelData::U8(b.into_raw())),
        DynamicImage::ImageRgba8(b) => (ColorModel::Rgba, PixelData::U8(b.into_raw())),
        DynamicImage::ImageLuma16(b) => (ColorModel::Gray, PixelData::U16(b.into_raw())),
        DynamicImage::ImageRgb16(b) => (ColorModel::Rgb, PixelData::U16(b.into_raw())),
        DynamicImage::ImageRgba16(b) => (ColorModel::Rgba, PixelData::U16(b.into_raw())),
        other => (ColorModel::Rgb, PixelData::U8(other.into_rgb8().into_raw())),
    };
    VwImage::new(w, h, color, data).map_err(DecodeError::Invalid)
}

/// 中身に合う rawler のデコーダ。
fn decoder_for(source: &RawSource) -> Result<Box<dyn Decoder>, DecodeError> {
    // rawler のエラー（"No decoder found, model '', make: ''"）では、RAW でないファイルか
    // 対応していない機種かが分かりにくいため言い換える。
    rawler::get_decoder(source).map_err(|_| {
        DecodeError::Unsupported("not a camera RAW file, or the camera is not supported".into())
    })
}

fn invalid(e: rawler::RawlerError) -> DecodeError {
    DecodeError::Invalid(e.to_string())
}

/// メタデータ欄に出す、撮影情報と表示した画像の種類。
fn metadata(raw: Option<&RawMetadata>, shown: Option<Shown>) -> Metadata {
    let mut m = Metadata::default();
    if let Some(shown) = shown {
        let text = match shown {
            Shown::Preview => "Embedded preview (JPEG made by the camera)",
            Shown::Developed => "Developed from sensor data",
        };
        m.push("RAW", "Shown", text);
    }
    let Some(raw) = raw else {
        return m;
    };
    m.push("RAW", "Make", &raw.make);
    m.push("RAW", "Model", &raw.model);
    let e = &raw.exif;
    let lens = raw
        .lens
        .as_ref()
        .map(|l| {
            format!("{} {}", l.lens_make, l.lens_model)
                .trim()
                .to_string()
        })
        .or_else(|| e.lens_model.clone());
    if let Some(lens) = lens.filter(|l| !l.is_empty()) {
        m.push("RAW", "Lens", lens);
    }
    let mut push = |key: &str, value: Option<String>| {
        if let Some(v) = value {
            m.push("RAW", key, v);
        }
    };
    push("Date taken", e.date_time_original.clone());
    push("Exposure time", e.exposure_time.map(exposure_time));
    push("F-number", e.fnumber.map(|f| format!("f/{}", decimal(f))));
    let iso = e.iso_speed.or(e.iso_speed_ratings.map(u32::from));
    push("ISO", iso.map(|i| i.to_string()));
    push("Exposure bias", e.exposure_bias.map(exposure_bias));
    push(
        "Focal length",
        e.focal_length.map(|f| format!("{} mm", decimal(f))),
    );
    push("Orientation", e.orientation.map(|o| o.to_string()));
    m
}

/// 1 秒未満は `1/250 s`、それ以上は `2.5 s` のように出す。
fn exposure_time(t: Rational) -> String {
    if t.n == 0 || t.d == 0 {
        return "0 s".into();
    }
    let seconds = f64::from(t.n) / f64::from(t.d);
    if seconds < 1.0 {
        format!("1/{} s", (1.0 / seconds).round())
    } else {
        format!("{} s", trim(seconds))
    }
}

fn exposure_bias(b: SRational) -> String {
    if b.d == 0 {
        return "0 EV".into();
    }
    let ev = f64::from(b.n) / f64::from(b.d);
    format!("{}{} EV", if ev > 0.0 { "+" } else { "" }, trim(ev))
}

/// `28/10` を `2.8` のように出す。
fn decimal(r: Rational) -> String {
    if r.d == 0 {
        return "0".into();
    }
    trim(f64::from(r.n) / f64::from(r.d))
}

/// 小数第 1 位まで。`.0` は付けない。
fn trim(x: f64) -> String {
    let s = format!("{x:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_accepts_fujifilm_raf_only() {
        assert!(RawPlugin.probe(b"FUJIFILMCCD-RAW 0201FF383501"));
        // NEF や ARW の先頭は TIFF と同じなので、中身では受け持たない。
        assert!(!RawPlugin.probe(b"MM\0*\0\0\0\x08"));
        assert!(!RawPlugin.probe(b"II*\0\x08\0\0\0"));
    }

    #[test]
    fn exposure_time_is_shown_as_a_fraction_below_one_second() {
        assert_eq!(exposure_time(Rational { n: 1, d: 250 }), "1/250 s");
        assert_eq!(exposure_time(Rational { n: 10, d: 2500 }), "1/250 s");
        assert_eq!(exposure_time(Rational { n: 5, d: 2 }), "2.5 s");
        assert_eq!(exposure_time(Rational { n: 2, d: 1 }), "2 s");
    }

    #[test]
    fn f_number_and_focal_length_drop_needless_decimals() {
        assert_eq!(decimal(Rational { n: 28, d: 10 }), "2.8");
        assert_eq!(decimal(Rational { n: 50, d: 1 }), "50");
    }

    #[test]
    fn exposure_bias_has_a_sign() {
        assert_eq!(exposure_bias(SRational { n: -2, d: 3 }), "-0.7 EV");
        assert_eq!(exposure_bias(SRational { n: 1, d: 1 }), "+1 EV");
        assert_eq!(exposure_bias(SRational { n: 0, d: 1 }), "0 EV");
    }

    #[test]
    fn file_that_is_not_raw_is_an_error_not_a_crash() {
        let err = RawPlugin.decode(b"this is not an image\n").unwrap_err();
        assert_eq!(
            err.to_string(),
            "unsupported: not a camera RAW file, or the camera is not supported"
        );
        assert!(RawPlugin.decode(b"MM\0*\0\0\0\x08broken").is_err());
    }
}
