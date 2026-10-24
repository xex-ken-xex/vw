//! カメラ RAW（Nikon NEF・Sony ARW・富士フイルム RAF など）のデコーダプラグイン。
//!
//! RAW に埋め込まれたプレビュー JPEG を表示する。センサーのデータを現像するのは、
//! プレビューがないときと、[`DecoderPlugin::develop`] を頼まれたとき（拡大したときや操作で）。
//!
//! 毎回現像しないのは、4500 万画素級の RAW の現像には 1 枚で数秒かかり、フォルダを
//! 開いたときのサムネイルやスライドショーに間に合わないため。プレビュー JPEG は
//! カメラが自分で現像したもので、多くの機種ではセンサーと同じ大きさで入っている。

use image::{DynamicImage, GenericImageView};
use rawler::decoders::{Decoder, RawDecodeParams, RawMetadata};
use rawler::formats::tiff::{Rational, SRational};
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::imgop::srgb::{srgb_apply_gamma, srgb_invert_gamma};
use rawler::rawsource::RawSource;
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

mod tiff_guard;

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
        let orientation = orientation(raw_metadata.as_ref());
        match decoder.preview_image(&source, &params) {
            Ok(Some(preview)) => {
                let mut img = from_dynamic(preview)?;
                img.metadata = metadata(raw_metadata.as_ref(), Some(Shown::Preview));
                img.apply_orientation(orientation.into());
                img.develop_size = sensor_size(decoder.as_ref(), &source, &params, orientation);
                Ok(img)
            }
            _ => developed(
                decoder.as_ref(),
                &source,
                &params,
                raw_metadata.as_ref(),
                None,
            ),
        }
    }

    fn develop(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        let source = RawSource::new_from_slice(data);
        let decoder = decoder_for(&source)?;
        let params = RawDecodeParams::default();
        let raw_metadata = decoder.raw_metadata(&source, &params).ok();
        let preview = decoder.preview_image(&source, &params).ok().flatten();
        developed(
            decoder.as_ref(),
            &source,
            &params,
            raw_metadata.as_ref(),
            preview.as_ref(),
        )
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

/// センサーのデータを現像し、メタデータと向きを整えた画像。
/// `preview` があれば、明るさをそれに合わせる（[`brightness_gain`]）。
fn developed(
    decoder: &dyn Decoder,
    source: &RawSource,
    params: &RawDecodeParams,
    raw_metadata: Option<&RawMetadata>,
    preview: Option<&DynamicImage>,
) -> Result<VwImage, DecodeError> {
    let (mut img, steps) = develop(decoder, source, params, preview)?;
    img.metadata = metadata(raw_metadata, Some(Shown::Developed));
    steps.push_to(&mut img.metadata);
    img.apply_orientation(orientation(raw_metadata).into());
    Ok(img)
}

/// 現像で使った値。色導出の図に段として出す。
struct DevelopSteps {
    black: Vec<f32>,
    white: Vec<u32>,
    white_balance: [f32; 4],
    gain: f32,
}

impl DevelopSteps {
    fn push_to(&self, m: &mut Metadata) {
        let list = |v: &[String]| v.join(", ");
        m.push(
            "RAW",
            "Black level",
            list(
                &self
                    .black
                    .iter()
                    .map(|b| trim(f64::from(*b)))
                    .collect::<Vec<_>>(),
            ),
        );
        m.push(
            "RAW",
            "White level",
            list(&self.white.iter().map(u32::to_string).collect::<Vec<_>>()),
        );
        let wb: Vec<String> = self
            .white_balance
            .iter()
            .filter(|c| c.is_finite() && **c != 0.0)
            .map(|c| format!("{c:.3}"))
            .collect();
        if !wb.is_empty() {
            m.push("RAW", "White balance", list(&wb));
        }
        m.push("RAW", "Brightness gain", format!("{:.2}", self.gain));
    }
}

/// EXIF の向き（1〜8）。分からなければ 1。
fn orientation(raw_metadata: Option<&RawMetadata>) -> u16 {
    raw_metadata.and_then(|m| m.exif.orientation).unwrap_or(1)
}

/// 現像したときのおおよその大きさ（向きは補正後）。センサーのデータは展開せずに、
/// 大きさだけ読む。
///
/// 現像の結果と 1 画素単位では一致しないことがある（富士フイルムの一部の機種は現像で
/// 45 度回す）。使い道が「プレビューより精細か」の判断とメモリの見積もりなので、
/// 正確な大きさのために現像はしない。
fn sensor_size(
    decoder: &dyn Decoder,
    source: &RawSource,
    params: &RawDecodeParams,
    orientation: u16,
) -> Option<(u32, u32)> {
    let raw = decoder.raw_image(source, params, true).ok()?;
    let (w, h) = raw
        .crop_area
        .or(raw.active_area)
        .map_or((raw.width, raw.height), |r| (r.d.w, r.d.h));
    let (w, h) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    // 5〜8 は 90 度回す向き。
    Some(if (5..=8).contains(&orientation) {
        (h, w)
    } else {
        (w, h)
    })
}

/// センサーのデータを現像し、16bit の RGB にする。
fn develop(
    decoder: &dyn Decoder,
    source: &RawSource,
    params: &RawDecodeParams,
    preview: Option<&DynamicImage>,
) -> Result<(VwImage, DevelopSteps), DecodeError> {
    let raw = decoder.raw_image(source, params, false).map_err(invalid)?;
    let mut used = DevelopSteps {
        black: raw.blacklevel.as_vec(),
        white: raw.whitelevel.0.clone(),
        white_balance: raw.wb_coeffs,
        gain: 1.0,
    };
    // sRGB のガンマは、明るさを合わせてから自分で掛ける。明るさは光の量に比例する
    // ガンマ前の値で掛け合わせないと、色の比が崩れるため。
    let mut steps = RawDevelop::default();
    steps.steps.retain(|s| *s != ProcessingStep::SRgb);
    let mut linear = steps.develop_intermediate(&raw).map_err(invalid)?;
    let gain = preview.map_or(1.0, |p| {
        brightness_gain(median_luminance_of_preview(p), median_luminance(&linear))
    });
    used.gain = gain;
    let encode = |v: f32| srgb_apply_gamma((v * gain).clamp(0.0, 1.0));
    match &mut linear {
        Intermediate::Monochrome(p) => p.for_each(encode),
        Intermediate::ThreeColor(p) => p.for_each(|v| v.map(encode)),
        Intermediate::FourColor(p) => p.for_each(|v| v.map(encode)),
    }
    let developed = linear
        .to_dynamic_image()
        .ok_or_else(|| DecodeError::Invalid("developed image has no pixels".into()))?;
    Ok((from_dynamic(developed)?, used))
}

/// 中央値を測るのに使う画素の数の目安。全画素を使わないのは、数千万画素を並べ替えると
/// 現像の時間が目に見えて延びるため。間引いても中央値はほとんど変わらない。
const LUMINANCE_SAMPLES: usize = 100_000;

/// 現像した画像に掛ける明るさの倍率。カメラのプレビューの明るさ（中間の明るさ）に合わせる。
///
/// 合わせるのは、rawler の現像はトーンカーブを掛けないのでカメラの JPEG より暗くなり、
/// 拡大したときにプレビューから差し替わると急に暗くなって見えるため。さらに古い機種では
/// rawler の白レベルが合わず、何倍も暗くなることがある（Sony A300 など）。
/// 平均ではなく中央値で合わせるのは、空や照明のような一部の明るい所に引きずられないため。
/// 倍率に上限と下限を付けるのは、真っ暗なプレビューなどで極端な倍率になるのを防ぐため。
fn brightness_gain(preview_median: f32, developed_median: f32) -> f32 {
    const RANGE: std::ops::RangeInclusive<f32> = 1.0 / 16.0..=16.0;
    if preview_median <= 0.0 || developed_median <= 0.0 {
        return 1.0;
    }
    (preview_median / developed_median).clamp(*RANGE.start(), *RANGE.end())
}

/// 光の量に比例する（ガンマ前の）輝度。
fn luminance([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn median(mut values: Vec<f32>) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mid = values.len() / 2;
    *values.select_nth_unstable_by(mid, f32::total_cmp).1
}

/// 現像した（ガンマ前の）画像の輝度の中央値。
fn median_luminance(linear: &Intermediate) -> f32 {
    let samples: Vec<f32> = match linear {
        Intermediate::Monochrome(p) => {
            let step = (p.data.len() / LUMINANCE_SAMPLES).max(1);
            p.data.iter().step_by(step).copied().collect()
        }
        Intermediate::ThreeColor(p) => {
            let step = (p.data.len() / LUMINANCE_SAMPLES).max(1);
            p.data.iter().step_by(step).map(|&v| luminance(v)).collect()
        }
        Intermediate::FourColor(p) => {
            let step = (p.data.len() / LUMINANCE_SAMPLES).max(1);
            let rgb = |[r, g, b, _]: [f32; 4]| luminance([r, g, b]);
            p.data.iter().step_by(step).map(|&v| rgb(v)).collect()
        }
    };
    median(samples)
}

/// プレビュー（sRGB）の、ガンマを外した輝度の中央値。
fn median_luminance_of_preview(preview: &DynamicImage) -> f32 {
    let (w, h) = preview.dimensions();
    let pixels = w as usize * h as usize;
    let step = (pixels / LUMINANCE_SAMPLES).max(1);
    let samples = (0..pixels)
        .step_by(step)
        .map(|i| {
            let [r, g, b, _] = preview.get_pixel(i as u32 % w, i as u32 / w).0;
            luminance([r, g, b].map(|c| srgb_invert_gamma(f32::from(c) / 255.0)))
        })
        .collect();
    median(samples)
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
    tiff_guard::check(source.subview_until_eof(0).map_err(DecodeError::Io)?)?;
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
    fn developed_image_is_brightened_to_the_preview() {
        assert_eq!(brightness_gain(0.2, 0.05), 4.0);
        assert_eq!(brightness_gain(0.1, 0.2), 0.5);
    }

    #[test]
    fn brightness_gain_is_bounded() {
        assert_eq!(brightness_gain(0.9, 0.0001), 16.0);
        assert_eq!(brightness_gain(0.0, 0.2), 1.0);
        assert_eq!(brightness_gain(0.2, 0.0), 1.0);
    }

    #[test]
    fn median_luminance_of_preview_undoes_the_srgb_gamma() {
        // sRGB の 188 は、光の量ではおよそ半分。
        let gray = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([188; 3])));
        let m = median_luminance_of_preview(&gray);
        assert!((m - 0.5).abs() < 0.01, "{m}");
    }

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
