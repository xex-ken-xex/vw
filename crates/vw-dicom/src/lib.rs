//! DICOM デコーダプラグイン。
//!
//! - 非圧縮・RLE・JPEG（ベースライン / ロスレス）・JPEG-LS・JPEG 2000 を読む。JPEG-LS は
//!   CharLS（C++）、JPEG 2000 は OpenJPEG（C）を静的にリンクする。純 Rust の候補は、
//!   pure_jpegls がカラーを読めず、openjp2 がデコードのたびに未定義動作（null の Box の解放）
//!   を起こすので採らない
//! - モノクロは Rescale Slope / Intercept を当てた値（CT なら HU）で持ち、ファイルの
//!   ウィンドウ幅 / レベルを表示の階調として渡す。MONOCHROME1 は値を反転して持つ
//! - 1 ファイルに複数フレームが入ったもの（シネなど）はフレームごとに読む
//! - 患者情報は「Patient」のメタデータにだけ出す

use std::io::Cursor;

use dicom_core::Tag;
use dicom_dictionary_std::tags;
use dicom_object::file::ReadPreamble;
use dicom_object::{DefaultDicomObject, OpenFileOptions};
use dicom_pixeldata::{
    DecodedPixelData, PhotometricInterpretation, PixelDecoder, PixelRepresentation,
    PlanarConfiguration, VoiLutFunction,
};
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

pub mod charset;
pub mod net;
pub mod pipeline;
pub mod synth;

/// 128 バイトのプリアンブルの後ろに置かれる印。
const MAGIC: &[u8] = b"DICM";
const MAGIC_AT: usize = 128;

pub struct DicomPlugin;

impl DecoderPlugin for DicomPlugin {
    fn name(&self) -> &str {
        "DICOM"
    }

    fn extensions(&self) -> &[&str] {
        &["dcm", "dicom", "dic"]
    }

    fn probe(&self, header: &[u8]) -> bool {
        header.get(MAGIC_AT..MAGIC_AT + MAGIC.len()) == Some(MAGIC)
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        self.decode_frame(data, 0)
    }

    fn frame_count(&self, data: &[u8]) -> Result<usize, DecodeError> {
        let obj = open(data, true)?;
        Ok(frames_in(&obj))
    }

    fn decode_frame(&self, data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
        let obj = open(data, false)?;
        let frames = frames_in(&obj);
        if frame >= frames {
            return Err(DecodeError::Invalid(format!(
                "no frame {frame} (the file has {frames})"
            )));
        }
        let px = obj
            .decode_pixel_data_frame(frame as u32)
            .map_err(|e| DecodeError::Unsupported(e.to_string()))?;
        let mut img = to_image(&px, frame)?;
        img.metadata = metadata(&obj);
        Ok(img)
    }

    /// 画素データの手前で読むのをやめるので、大きなファイルでも速い。
    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        Ok(metadata(&open(data, true)?))
    }

    /// 画素データなどの大きなバイナリを除いた全タグ。グループは `DICOM`、キーは `0010,0020` の形。
    fn read_all_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        let obj = open(data, true)?;
        let mut m = Metadata::default();
        let meta = obj.meta();
        for (tag, value) in [
            (
                tags::MEDIA_STORAGE_SOP_CLASS_UID,
                &meta.media_storage_sop_class_uid,
            ),
            (tags::TRANSFER_SYNTAX_UID, &meta.transfer_syntax),
        ] {
            push_tag(&mut m, tag, value);
        }
        all_tags(&obj, &mut m);
        Ok(m)
    }
}

/// 検索カタログのキー（`0010,0020`）からタグの名前（`PatientID`）を引く。画面でキーの候補に添える。
pub fn tag_name(key: &str) -> Option<&'static str> {
    use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry};
    let (group, element) = key.split_once(',')?;
    let tag = Tag(
        u16::from_str_radix(group, 16).ok()?,
        u16::from_str_radix(element, 16).ok()?,
    );
    dicom_dictionary_std::StandardDataDictionary
        .by_tag(tag)
        .map(|e| e.alias())
}

/// シーケンスの中も同じように辿る。入れ子の位置は持たない（検索では値が見つかれば足りるため）。
fn all_tags(obj: &dicom_object::InMemDicomObject, m: &mut Metadata) {
    use dicom_core::value::Value;
    use dicom_core::VR;
    for e in obj.iter() {
        match e.value() {
            Value::Sequence(seq) => {
                for item in seq.items() {
                    all_tags(item, m);
                }
            }
            Value::PixelSequence(_) => {}
            Value::Primitive(p) => {
                let binary = matches!(
                    e.vr(),
                    VR::OB | VR::OD | VR::OF | VR::OL | VR::OV | VR::OW | VR::UN
                );
                if !binary {
                    push_tag(m, e.header().tag, &charset::value_text(p));
                }
            }
        }
    }
}

fn push_tag(m: &mut Metadata, tag: Tag, value: &str) {
    let value = value.trim_matches([' ', '\0']);
    if !value.is_empty() {
        m.push(
            "DICOM",
            format!("{:04X},{:04X}", tag.group(), tag.element()),
            value,
        );
    }
}

/// DICOM ファイルを開く。`header_only` なら画素データの手前で読むのをやめる。
fn open(data: &[u8], header_only: bool) -> Result<DefaultDicomObject, DecodeError> {
    let mut options = OpenFileOptions::new().read_preamble(ReadPreamble::Always);
    if header_only {
        options = options.read_until(tags::PIXEL_DATA);
    }
    options
        .from_reader(Cursor::new(data))
        .map_err(|e| DecodeError::Invalid(e.to_string()))
}

fn frames_in(obj: &DefaultDicomObject) -> usize {
    int(obj, tags::NUMBER_OF_FRAMES).map_or(1, |n| n.max(1) as usize)
}

fn text(obj: &DefaultDicomObject, tag: Tag) -> Option<String> {
    let s = charset::value_text(obj.get(tag)?.value().primitive()?);
    let s = s.trim_matches([' ', '\0']);
    (!s.is_empty()).then(|| s.to_owned())
}

fn int(obj: &DefaultDicomObject, tag: Tag) -> Option<i64> {
    obj.get(tag)?.to_int::<i64>().ok()
}

/// 1 フレーム分の画素を [`VwImage`] にする。
fn to_image(px: &DecodedPixelData, frame: usize) -> Result<VwImage, DecodeError> {
    let (w, h) = (px.columns(), px.rows());
    let pixels = w as usize * h as usize;
    let raw = px.data();
    match px.samples_per_pixel() {
        1 => gray_image(px, frame, raw, pixels),
        3 => color_image(px, raw, pixels),
        n => Err(DecodeError::Unsupported(format!("{n} samples per pixel"))),
    }
}

/// モノクロの画素。値を変えずに済む（傾き 1・切片 0・符号なし・MONOCHROME2）なら U8 / U16 の
/// まま持ち、それ以外は Rescale を当てた F32 にする。U16 に詰め直して持つ方式は、CT の負の値
/// （空気は -1000 HU）が表せないので採らない。
fn gray_image(
    px: &DecodedPixelData,
    frame: usize,
    raw: &[u8],
    pixels: usize,
) -> Result<VwImage, DecodeError> {
    let stored = u32::from(px.bits_stored()).clamp(1, 32);
    let signed = px.pixel_representation() == PixelRepresentation::Signed;
    let invert = *px.photometric_interpretation() == PhotometricInterpretation::Monochrome1;
    let (slope, intercept) = px
        .rescale()
        .ok()
        .and_then(|r| r.get(frame).or(r.first()))
        .map_or((1.0, 0.0), |r| (r.slope, r.intercept));
    let function = px
        .voi_lut_function()
        .ok()
        .flatten()
        .and_then(|f| f.get(frame).or(f.first()).copied())
        .unwrap_or_default();
    let window = px
        .window()
        .ok()
        .flatten()
        .and_then(|w| w.get(frame).or(w.first()))
        .filter(|w| w.width > 0.0)
        .map(|w| linear_window(w.center, w.width, function));
    let values = stored_values(raw, px.bits_allocated(), stored, signed, pixels)?;
    // 近い値を同一視する必要はない。ちょうど恒等のときだけ格納値をそのまま使う近道で、
    // 外れても遅い道が同じ結果を出す。
    let plain = slope == 1.0 && intercept == 0.0 && !signed && !invert;
    let (data, significant_bits) = match (plain, px.bits_allocated()) {
        (true, 8) => (PixelData::U8(values.iter().map(|&v| v as u8).collect()), 8),
        (true, 16) => (
            PixelData::U16(values.iter().map(|&v| v as u16).collect()),
            stored.min(16),
        ),
        _ => {
            let sign = if invert { -1.0 } else { 1.0 };
            let f = values
                .iter()
                .map(|&v| (sign * (v as f64 * slope + intercept)) as f32)
                .collect();
            (PixelData::F32(f), 32)
        }
    };
    let mut img = VwImage::new(px.columns(), px.rows(), ColorModel::Gray, data)
        .map_err(DecodeError::Invalid)?;
    img.significant_bits = significant_bits;
    // MONOCHROME1 は値を反転して持つので、ウィンドウも反転する。
    img.window = window.map(|(lo, hi)| if invert { (-hi, -lo) } else { (lo, hi) });
    Ok(img)
}

/// ウィンドウを、vw-core が使う「下端から上端へ直線で 0..=255」の範囲にする。
///
/// LINEAR は規格（PS3.3 C.11.2.1.2.1）の式どおり中心を 0.5 ずらし、幅から 1 を引く。
/// 単純に中心 ± 幅 / 2 にすると、ピクセルパイプラインの図（規格の式で描く）と最大 2 段ずれ、
/// 図をファイルの値のまま開いても表示が変わってしまうため。
/// SIGMOID は直線では表せないので、中心 ± 幅 / 2 の直線で近似する。
fn linear_window(center: f64, width: f64, function: VoiLutFunction) -> (f32, f32) {
    let (lo, hi) = match function {
        VoiLutFunction::Linear => {
            let half = (width - 1.0).max(0.0) / 2.0;
            (center - 0.5 - half, center - 0.5 + half)
        }
        VoiLutFunction::LinearExact | VoiLutFunction::Sigmoid => {
            (center - width / 2.0, center + width / 2.0)
        }
    };
    (lo as f32, hi as f32)
}

/// 格納されている値。Bits Stored より上のビット（オーバーレイなどに使われる）は捨て、
/// 符号付きなら Bits Stored の最上位ビットで符号を広げる。
fn stored_values(
    raw: &[u8],
    allocated: u16,
    stored: u32,
    signed: bool,
    pixels: usize,
) -> Result<Vec<i64>, DecodeError> {
    let bytes = match allocated {
        8 => 1,
        16 => 2,
        32 => 4,
        n => return Err(DecodeError::Unsupported(format!("{n} bits allocated"))),
    };
    if raw.len() < pixels * bytes {
        return Err(DecodeError::Invalid(format!(
            "pixel data has {} bytes, expected {}",
            raw.len(),
            pixels * bytes
        )));
    }
    let mask = if stored >= 64 {
        u64::MAX
    } else {
        (1u64 << stored) - 1
    };
    let sign_bit = 1u64 << (stored - 1);
    Ok(raw[..pixels * bytes]
        .chunks_exact(bytes)
        .map(|b| {
            let mut v = [0u8; 8];
            v[..bytes].copy_from_slice(b);
            let u = u64::from_le_bytes(v) & mask;
            if signed && u & sign_bit != 0 {
                u as i64 - (mask as i64 + 1)
            } else {
                u as i64
            }
        })
        .collect())
}

/// カラーの画素（8bit の RGB / YBR_FULL）。
fn color_image(px: &DecodedPixelData, raw: &[u8], pixels: usize) -> Result<VwImage, DecodeError> {
    if px.bits_allocated() != 8 {
        return Err(DecodeError::Unsupported(format!(
            "{}-bit color",
            px.bits_allocated()
        )));
    }
    if raw.len() < pixels * 3 {
        return Err(DecodeError::Invalid("pixel data is too short".into()));
    }
    let mut rgb = match px.planar_configuration() {
        PlanarConfiguration::Standard => raw[..pixels * 3].to_vec(),
        PlanarConfiguration::PixelFirst => (0..pixels * 3)
            .map(|i| raw[(i % 3) * pixels + i / 3])
            .collect(),
    };
    match px.photometric_interpretation() {
        PhotometricInterpretation::Rgb => {}
        PhotometricInterpretation::YbrFull => ybr_to_rgb(&mut rgb),
        other => {
            return Err(DecodeError::Unsupported(format!(
                "photometric interpretation {other}"
            )))
        }
    }
    VwImage::new(px.columns(), px.rows(), ColorModel::Rgb, PixelData::U8(rgb))
        .map_err(DecodeError::Invalid)
}

/// YCbCr（JPEG と同じ式）を RGB に直す。
fn ybr_to_rgb(px: &mut [u8]) {
    for p in px.as_chunks_mut::<3>().0 {
        let (y, cb, cr) = (p[0] as f32, p[1] as f32 - 128.0, p[2] as f32 - 128.0);
        p[0] = (y + 1.402 * cr).round().clamp(0.0, 255.0) as u8;
        p[1] = (y - 0.344136 * cb - 0.714136 * cr)
            .round()
            .clamp(0.0, 255.0) as u8;
        p[2] = (y + 1.772 * cb).round().clamp(0.0, 255.0) as u8;
    }
}

/// 表示するメタデータ。検査やシリーズの並べ替えに使う UID もここから読む。
fn metadata(obj: &DefaultDicomObject) -> Metadata {
    let mut m = Metadata::default();
    let fields: &[(&str, &str, Tag)] = &[
        ("Patient", "Name", tags::PATIENT_NAME),
        ("Patient", "ID", tags::PATIENT_ID),
        ("Patient", "Birth date", tags::PATIENT_BIRTH_DATE),
        ("Patient", "Sex", tags::PATIENT_SEX),
        ("DICOM", "Modality", tags::MODALITY),
        ("DICOM", "Study date", tags::STUDY_DATE),
        ("DICOM", "Study time", tags::STUDY_TIME),
        ("DICOM", "Study", tags::STUDY_DESCRIPTION),
        ("DICOM", "Series number", tags::SERIES_NUMBER),
        ("DICOM", "Series", tags::SERIES_DESCRIPTION),
        ("DICOM", "Instance number", tags::INSTANCE_NUMBER),
        ("DICOM", "Rows", tags::ROWS),
        ("DICOM", "Columns", tags::COLUMNS),
        ("DICOM", "Number of frames", tags::NUMBER_OF_FRAMES),
        ("DICOM", "Frame time (ms)", tags::FRAME_TIME),
        ("DICOM", "Cine rate", tags::CINE_RATE),
        (
            "DICOM",
            "Recommended frame rate",
            tags::RECOMMENDED_DISPLAY_FRAME_RATE,
        ),
        ("DICOM", "Photometric", tags::PHOTOMETRIC_INTERPRETATION),
        ("DICOM", "Bits allocated", tags::BITS_ALLOCATED),
        ("DICOM", "Bits stored", tags::BITS_STORED),
        ("DICOM", "Pixel representation", tags::PIXEL_REPRESENTATION),
        ("DICOM", "Rescale slope", tags::RESCALE_SLOPE),
        ("DICOM", "Rescale intercept", tags::RESCALE_INTERCEPT),
        ("DICOM", "Window center", tags::WINDOW_CENTER),
        ("DICOM", "Window width", tags::WINDOW_WIDTH),
        ("DICOM", "Slice thickness", tags::SLICE_THICKNESS),
        ("DICOM", "Slice location", tags::SLICE_LOCATION),
        ("DICOM", "Pixel spacing", tags::PIXEL_SPACING),
        ("DICOM", "Manufacturer", tags::MANUFACTURER),
        ("DICOM", "Model", tags::MANUFACTURER_MODEL_NAME),
        ("DICOM", "Institution", tags::INSTITUTION_NAME),
        ("DICOM", "Study UID", tags::STUDY_INSTANCE_UID),
        ("DICOM", "Series UID", tags::SERIES_INSTANCE_UID),
        ("DICOM", "SOP instance UID", tags::SOP_INSTANCE_UID),
    ];
    for &(group, key, tag) in fields {
        if let Some(v) = text(obj, tag) {
            m.push(group, key, v);
        }
    }
    let ts = obj.meta().transfer_syntax().trim_end_matches('\0');
    let ts_name = dicom_transfer_syntax_name(ts);
    m.push("DICOM", "Transfer syntax", ts_name.unwrap_or(ts));
    tag_listing(obj, &mut m);
    m
}

/// メタデータ欄で全タグを並べるグループ。
pub const TAG_LISTING: &str = "DICOM tags";

/// 長い値はここまでで切る。検索カタログの値の上限と揃える。
const LISTED_VALUE_MAX: usize = 2048;

/// ファイルの全タグを、ファイルの順に `(0010,0010) PatientName` の形のキーで並べる。
/// シーケンスの項目の中のタグは、深さの分だけキーの頭に `>` を付ける（dcmdump と同じ書き方）。
///
/// 主なタグを名前で出す上のグループとは別に持つのは、そちらを分かりやすい名前のまま残したいのと、
/// シリーズの並べ替えなどがそちらのキーを読むため。
fn tag_listing(obj: &DefaultDicomObject, m: &mut Metadata) {
    let meta = obj.meta();
    let file_meta = [
        (
            tags::MEDIA_STORAGE_SOP_CLASS_UID,
            meta.media_storage_sop_class_uid.as_str(),
        ),
        (
            tags::MEDIA_STORAGE_SOP_INSTANCE_UID,
            meta.media_storage_sop_instance_uid.as_str(),
        ),
        (tags::TRANSFER_SYNTAX_UID, meta.transfer_syntax.as_str()),
        (
            tags::IMPLEMENTATION_CLASS_UID,
            meta.implementation_class_uid.as_str(),
        ),
    ];
    for (tag, value) in file_meta {
        m.push(TAG_LISTING, listed_key(tag, 0), listed_value(value));
    }
    if let Some(name) = &meta.implementation_version_name {
        m.push(
            TAG_LISTING,
            listed_key(tags::IMPLEMENTATION_VERSION_NAME, 0),
            listed_value(name),
        );
    }
    list_elements(obj, 0, m);
}

fn list_elements(obj: &dicom_object::InMemDicomObject, depth: usize, m: &mut Metadata) {
    use dicom_core::value::Value;
    use dicom_core::VR;
    for e in obj.iter() {
        let tag = e.header().tag;
        let key = listed_key(tag, depth);
        match e.value() {
            Value::Sequence(seq) => {
                let n = seq.items().len();
                let unit = if n == 1 { "item" } else { "items" };
                m.push(TAG_LISTING, key, format!("{n} {unit}"));
                for item in seq.items() {
                    list_elements(item, depth + 1, m);
                }
            }
            Value::PixelSequence(seq) => {
                m.push(
                    TAG_LISTING,
                    key,
                    format!("({} fragments)", seq.fragments().len()),
                );
            }
            Value::Primitive(p) => {
                let binary = matches!(
                    e.vr(),
                    VR::OB | VR::OD | VR::OF | VR::OL | VR::OV | VR::OW | VR::UN
                );
                let value = if binary {
                    format!("({:?}, {} bytes)", e.vr(), p.calculate_byte_len())
                } else {
                    listed_value(&charset::value_text(p))
                };
                m.push(TAG_LISTING, key, value);
            }
        }
    }
}

fn listed_key(tag: Tag, depth: usize) -> String {
    let number = format!("{:04X},{:04X}", tag.group(), tag.element());
    let name = tag_name(&number).unwrap_or("");
    format!("{}({number}) {name}", ">".repeat(depth))
        .trim_end()
        .to_owned()
}

fn listed_value(value: &str) -> String {
    let value = value.trim_matches([' ', '\0']);
    if value.len() <= LISTED_VALUE_MAX {
        return value.to_owned();
    }
    let mut end = LISTED_VALUE_MAX;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} bytes)", &value[..end], value.len())
}

/// よく見る転送構文の名前。
fn dicom_transfer_syntax_name(uid: &str) -> Option<&'static str> {
    Some(match uid {
        "1.2.840.10008.1.2" => "Implicit VR Little Endian",
        "1.2.840.10008.1.2.1" => "Explicit VR Little Endian",
        "1.2.840.10008.1.2.2" => "Explicit VR Big Endian",
        "1.2.840.10008.1.2.5" => "RLE Lossless",
        "1.2.840.10008.1.2.4.50" => "JPEG Baseline",
        "1.2.840.10008.1.2.4.51" => "JPEG Extended",
        "1.2.840.10008.1.2.4.57" => "JPEG Lossless",
        "1.2.840.10008.1.2.4.70" => "JPEG Lossless SV1",
        "1.2.840.10008.1.2.4.80" => "JPEG-LS Lossless",
        "1.2.840.10008.1.2.4.81" => "JPEG-LS Near-lossless",
        "1.2.840.10008.1.2.4.90" => "JPEG 2000 Lossless",
        "1.2.840.10008.1.2.4.91" => "JPEG 2000",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::VR;
    use synth::{
        Synth, CT_IMAGE, JPEG_2000_LOSSLESS, JPEG_LS_LOSSLESS, JPEG_LS_NEAR_LOSSLESS, XA_IMAGE,
    };

    /// testdata/*.j2k に入れた値。testdata/make_j2k.py で作った。
    const J2K_GRAY16: [u16; 8] = [0, 1000, 2000, 3000, 4095, 300, 40, 7];
    const J2K_RGB8: [u8; 24] = [
        255, 0, 0, 0, 255, 0, 0, 0, 255, 10, 20, 30, 200, 100, 50, 0, 0, 0, 255, 255, 255, 128, 64,
        32,
    ];

    fn jpeg_ls(
        width: u32,
        height: u32,
        bits: i32,
        components: i32,
        near: i32,
        src: &[u8],
    ) -> Vec<u8> {
        let frame = charls::FrameInfo {
            width,
            height,
            bits_per_sample: bits,
            component_count: components,
        };
        charls::CharLS::default().encode(frame, near, src).unwrap()
    }

    fn u16_bytes(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn gray_f32(img: &VwImage) -> &[f32] {
        match &img.data {
            PixelData::F32(v) => v,
            other => panic!("expected F32, got {} bits", other.bits_per_sample()),
        }
    }

    #[test]
    fn recognises_the_dicm_magic_after_the_preamble() {
        let bytes = Synth::ct(2, 2).bytes();
        assert!(DicomPlugin.probe(&bytes));
        assert!(!DicomPlugin.probe(b"\x89PNG\r\n\x1a\n"));
        assert!(!DicomPlugin.probe(&bytes[..100]));
    }

    #[test]
    fn ct_values_become_hounsfield_units_with_the_files_window() {
        let synth = Synth::ct(2, 1).pixels_i16(&[0, 1000]).rescale(1.0, -1024.0);
        let img = DicomPlugin
            .decode(&synth.window(40.0, 400.0).bytes())
            .unwrap();
        assert_eq!(gray_f32(&img), [-1024.0, -24.0]);
        // LINEAR の規格の式: 中心 - 0.5 ± (幅 - 1) / 2。
        assert_eq!(img.window, Some((-160.0, 239.0)));
    }

    #[test]
    fn linear_exact_window_spans_center_plus_minus_half_width() {
        let synth = Synth::ct(2, 1)
            .pixels_i16(&[0, 1000])
            .rescale(1.0, -1024.0)
            .window(40.0, 400.0)
            .text(tags::VOILUT_FUNCTION, dicom_core::VR::CS, "LINEAR_EXACT");
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert_eq!(img.window, Some((-160.0, 240.0)));
    }

    #[test]
    fn plain_16bit_images_stay_u16_with_bits_stored_as_depth() {
        let synth = Synth::new(CT_IMAGE, 2, 1).pixels_u16(&[0x0fff, 0xf123], 12);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        // Bits Stored より上のビットは捨てる。
        assert!(matches!(&img.data, PixelData::U16(v) if v == &[0x0fff, 0x0123]));
        assert_eq!(img.significant_bits, 12);
        assert_eq!(img.window, None);
    }

    #[test]
    fn signed_values_are_sign_extended_from_bits_stored() {
        // 12bit の符号付きで 0xfff は -1。
        let synth = Synth::new(CT_IMAGE, 1, 1)
            .pixels_u16(&[0x0fff], 12)
            .signed();
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert_eq!(gray_f32(&img), [-1.0]);
    }

    #[test]
    fn monochrome1_is_inverted_together_with_its_window() {
        let synth = Synth::new(CT_IMAGE, 2, 1)
            .pixels_u16(&[0, 100], 16)
            .monochrome1()
            .window(50.0, 100.0);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert_eq!(gray_f32(&img), [0.0, -100.0]);
        assert_eq!(img.window, Some((-99.0, 0.0)));
    }

    #[test]
    fn each_frame_of_a_multiframe_file_is_read_separately() {
        let synth = Synth::new(XA_IMAGE, 1, 1).frames_u8(&[&[10], &[20], &[30]]);
        let bytes = synth.bytes();
        assert_eq!(DicomPlugin.frame_count(&bytes).unwrap(), 3);
        let third = DicomPlugin.decode_frame(&bytes, 2).unwrap();
        assert!(matches!(&third.data, PixelData::U8(v) if v == &[30]));
        assert!(DicomPlugin.decode_frame(&bytes, 3).is_err());
    }

    #[test]
    fn rgb_images_are_read_in_either_planar_configuration() {
        let interleaved = Synth::rgb(2, 1, &[1, 2, 3, 4, 5, 6], false);
        let planar = Synth::rgb(2, 1, &[1, 4, 2, 5, 3, 6], true);
        for synth in [interleaved, planar] {
            let img = DicomPlugin.decode(&synth.bytes()).unwrap();
            assert!(matches!(&img.data, PixelData::U8(v) if v == &[1, 2, 3, 4, 5, 6]));
        }
    }

    #[test]
    fn patient_details_go_to_their_own_metadata_group() {
        let bytes = Synth::ct(1, 1).patient("Doe^Jane").bytes();
        let m = DicomPlugin.read_metadata(&bytes).unwrap();
        assert_eq!(m.get("Patient", "Name"), Some("Doe^Jane"));
        assert_eq!(m.get("DICOM", "Modality"), Some("CT"));
        assert_eq!(
            m.get("DICOM", "Transfer syntax"),
            Some("Explicit VR Little Endian")
        );
    }

    #[test]
    fn catalog_metadata_has_every_tag_by_number_except_pixel_data() {
        let bytes = Synth::ct(2, 2).patient("Doe^Jane").bytes();
        let m = DicomPlugin.read_all_metadata(&bytes).unwrap();
        assert_eq!(m.get("DICOM", "0010,0010"), Some("Doe^Jane"));
        assert_eq!(m.get("DICOM", "0008,0060"), Some("CT"));
        assert_eq!(m.get("DICOM", "0002,0010"), Some("1.2.840.10008.1.2.1"));
        assert_eq!(m.get("DICOM", "7FE0,0010"), None);
        assert!(m.entries.iter().all(|(g, _, _)| g == "DICOM"));
    }

    #[test]
    fn every_tag_is_listed_by_number_and_name_with_sequence_items_nested() {
        let bytes = Synth::ct(2, 2)
            .patient("Doe^Jane")
            .frames_i16(&[&[0; 4], &[0; 4]])
            .per_frame_values(&[(1.0, -1024.0, 40.0, 400.0), (1.0, -824.0, 40.0, 400.0)])
            .bytes();
        let m = DicomPlugin.read_metadata(&bytes).unwrap();
        let listed = |key: &str| m.get(TAG_LISTING, key);
        assert_eq!(listed("(0010,0010) PatientName"), Some("Doe^Jane"));
        assert_eq!(
            listed("(0002,0010) TransferSyntaxUID"),
            Some("1.2.840.10008.1.2.1")
        );
        assert_eq!(
            listed("(5200,9230) PerFrameFunctionalGroupsSequence"),
            Some("2 items")
        );
        assert_eq!(
            listed(">(0028,9145) PixelValueTransformationSequence"),
            Some("1 item")
        );
        assert_eq!(listed(">>(0028,1052) RescaleIntercept"), Some("-1024"));
        assert_eq!(m.get("Patient", "Name"), Some("Doe^Jane"));
    }

    #[test]
    fn listed_values_longer_than_the_cap_are_cut_with_their_length() {
        let long = "A".repeat(LISTED_VALUE_MAX + 10);
        let shown = listed_value(&long);
        assert!(shown.starts_with(&"A".repeat(LISTED_VALUE_MAX)));
        assert!(shown.ends_with(&format!("… ({} bytes)", LISTED_VALUE_MAX + 10)));
    }

    #[test]
    fn japanese_names_written_with_iso_2022_escapes_are_read_everywhere() {
        let name = "Yamada^Taro=\x1b$B;3ED\x1b(B^\x1b$BB@O:\x1b(B";
        let study = "\x1b$B6;It\x1b(B CT";
        let bytes = Synth::ct(1, 1)
            .text(
                tags::SPECIFIC_CHARACTER_SET,
                VR::CS,
                "ISO 2022 IR 6\\ISO 2022 IR 87",
            )
            .patient(name)
            .text(tags::STUDY_DESCRIPTION, VR::LO, study)
            .bytes();
        let shown = DicomPlugin.read_metadata(&bytes).unwrap();
        assert_eq!(shown.get("Patient", "Name"), Some("Yamada^Taro=山田^太郎"));
        assert_eq!(shown.get("DICOM", "Study"), Some("胸部 CT"));
        assert_eq!(
            shown.get(TAG_LISTING, "(0010,0010) PatientName"),
            Some("Yamada^Taro=山田^太郎")
        );
        let catalog = DicomPlugin.read_all_metadata(&bytes).unwrap();
        assert_eq!(
            catalog.get("DICOM", "0010,0010"),
            Some("Yamada^Taro=山田^太郎")
        );
    }

    #[test]
    fn tag_numbers_are_named_from_the_dictionary() {
        assert_eq!(tag_name("0010,0020"), Some("PatientID"));
        assert_eq!(tag_name("7FE0,0010"), Some("PixelData"));
        assert_eq!(tag_name("not a tag"), None);
    }

    #[test]
    fn lossless_jpeg_ls_gives_back_the_exact_values() {
        let values = [0, 1, 4095, 2048, 100, 7, 3000, 12];
        let jls = jpeg_ls(4, 2, 12, 1, 0, &u16_bytes(&values));
        let synth = Synth::new(CT_IMAGE, 4, 2)
            .pixels_u16(&values, 12)
            .encapsulated(JPEG_LS_LOSSLESS, vec![jls]);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert!(matches!(&img.data, PixelData::U16(v) if v == &values));
    }

    #[test]
    fn near_lossless_jpeg_ls_stays_within_the_allowed_error() {
        let values: Vec<u16> = (0..64).map(|i| (i * 37 % 251) as u16).collect();
        let jls = jpeg_ls(
            8,
            8,
            8,
            1,
            3,
            &values.iter().map(|&v| v as u8).collect::<Vec<_>>(),
        );
        let synth = Synth::new(XA_IMAGE, 8, 8)
            .frames_u8(&[&[0; 64]])
            .encapsulated(JPEG_LS_NEAR_LOSSLESS, vec![jls]);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        let PixelData::U8(got) = &img.data else {
            panic!("expected U8")
        };
        for (g, v) in got.iter().zip(&values) {
            assert!((*g as i32 - *v as i32).abs() <= 3, "{g} vs {v}");
        }
    }

    #[test]
    fn jpeg_ls_color_and_multiframe_are_read_per_frame() {
        let first = [1, 2, 3, 4, 5, 6];
        let second = [60, 50, 40, 30, 20, 10];
        let frames = [first, second].map(|f| jpeg_ls(2, 1, 8, 3, 0, &f)).to_vec();
        let synth = Synth::rgb(2, 1, &[0; 6], false).encapsulated(JPEG_LS_LOSSLESS, frames);
        let bytes = synth.bytes();
        assert_eq!(DicomPlugin.frame_count(&bytes).unwrap(), 2);
        let img = DicomPlugin.decode_frame(&bytes, 1).unwrap();
        assert_eq!(img.color, ColorModel::Rgb);
        assert!(matches!(&img.data, PixelData::U8(v) if v == &second));
    }

    #[test]
    fn lossless_jpeg_2000_gives_back_the_exact_values() {
        let j2k = include_bytes!("../testdata/gray16.j2k").to_vec();
        let synth = Synth::new(CT_IMAGE, 4, 2)
            .pixels_u16(&J2K_GRAY16, 16)
            .encapsulated(JPEG_2000_LOSSLESS, vec![j2k]);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert!(matches!(&img.data, PixelData::U16(v) if v == &J2K_GRAY16));
    }

    #[test]
    fn jpeg_2000_color_written_as_ybr_rct_is_shown_as_rgb() {
        let j2k = include_bytes!("../testdata/rgb8.j2k").to_vec();
        let synth = Synth::rgb(4, 2, &J2K_RGB8, false)
            .photometric("YBR_RCT")
            .encapsulated(JPEG_2000_LOSSLESS, vec![j2k]);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert!(matches!(&img.data, PixelData::U8(v) if v == &J2K_RGB8));
    }

    #[test]
    fn broken_file_is_an_error_not_a_panic() {
        let mut bytes = Synth::ct(2, 2).bytes();
        bytes.truncate(200);
        assert!(DicomPlugin.decode(&bytes).is_err());
    }
}
