//! DICOM デコーダプラグイン。
//!
//! - 非圧縮・RLE・JPEG（ベースライン / ロスレス）を読む。JPEG 2000 / JPEG-LS は C ライブラリ
//!   （OpenJPEG / CharLS）が要り、Windows のビルドが重くなるので今は読まない
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
    PlanarConfiguration,
};
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

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
        &["dcm", "dicom", "dic", "ima"]
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
    let s = obj.get(tag)?.to_str().ok()?;
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
    let window = px
        .window()
        .ok()
        .flatten()
        .and_then(|w| w.get(frame).or(w.first()))
        .filter(|w| w.width > 0.0)
        .map(|w| {
            let lo = w.center - w.width / 2.0;
            (lo as f32, (lo + w.width) as f32)
        });
    let values = stored_values(raw, px.bits_allocated(), stored, signed, pixels)?;
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
    m
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
    use synth::{Synth, CT_IMAGE, XA_IMAGE};

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
        assert_eq!(img.window, Some((-100.0, 0.0)));
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
    fn broken_file_is_an_error_not_a_panic() {
        let mut bytes = Synth::ct(2, 2).bytes();
        bytes.truncate(200);
        assert!(DicomPlugin.decode(&bytes).is_err());
    }
}
