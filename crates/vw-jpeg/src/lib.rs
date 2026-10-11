//! JPEG デコーダプラグイン。
//!
//! - グレーはグレーのまま、それ以外（YCbCr / CMYK / YCCK）は 8bit の RGB にする
//! - EXIF の向き（Orientation）に従って画素を並べ替える
//! - EXIF のタグをメタデータにする（メーカー独自のノートとサムネイルの IFD は除く）

use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// JPEG の先頭（SOI マーカーと、続くマーカーの始まり）。
const SIGNATURE: &[u8] = b"\xFF\xD8\xFF";
/// JPEG の幅と高さの上限。zune-jpeg の既定の上限（16384）では、パノラマ写真が開けないため。
const MAX_SIDE: usize = u16::MAX as usize;

pub struct JpegPlugin;

impl DecoderPlugin for JpegPlugin {
    fn name(&self) -> &str {
        "JPEG"
    }

    fn extensions(&self) -> &[&str] {
        &["jpg", "jpeg", "jpe", "jfif"]
    }

    fn probe(&self, header: &[u8]) -> bool {
        header.starts_with(SIGNATURE)
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        // 先に印を確かめるのは、zune-jpeg のエラー（"Illegal start bytes:7468" など）では
        // JPEG でないファイルだと分かりにくいため。
        if !self.probe(data) {
            return Err(DecodeError::Invalid("not a JPEG file".into()));
        }
        let mut decoder = headers(data)?;
        let gray = decoder.input_colorspace() == Some(ColorSpace::Luma);
        let out = if gray {
            ColorSpace::Luma
        } else {
            ColorSpace::RGB
        };
        decoder.set_options(decoder.options().jpeg_set_out_colorspace(out));
        let pixels = decoder.decode().map_err(invalid)?;
        let color = match decoder.output_colorspace() {
            Some(ColorSpace::Luma) => ColorModel::Gray,
            Some(ColorSpace::RGB) => ColorModel::Rgb,
            other => {
                return Err(DecodeError::Unsupported(format!(
                    "JPEG output colour space {other:?}"
                )))
            }
        };
        let info = decoder
            .info()
            .ok_or_else(|| DecodeError::Invalid("no JPEG header".into()))?;
        let mut img = VwImage::new(
            info.width.into(),
            info.height.into(),
            color,
            PixelData::U8(pixels),
        )
        .map_err(DecodeError::Invalid)?;
        let (metadata, orientation) = metadata(&decoder);
        img.metadata = metadata;
        img.apply_orientation(orientation);
        Ok(img)
    }

    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        Ok(metadata(&headers(data)?).0)
    }
}

/// ヘッダまで読んだデコーダ。
fn headers(data: &[u8]) -> Result<JpegDecoder<ZCursor<&[u8]>>, DecodeError> {
    let options = DecoderOptions::default()
        .set_max_width(MAX_SIDE)
        .set_max_height(MAX_SIDE);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);
    decoder.decode_headers().map_err(invalid)?;
    Ok(decoder)
}

fn invalid(e: zune_jpeg::errors::DecodeErrors) -> DecodeError {
    DecodeError::Invalid(e.to_string())
}

/// JPEG のヘッダと EXIF を表示用のメタデータにし、EXIF の向きと一緒に返す（なければ 1）。
fn metadata(decoder: &JpegDecoder<ZCursor<&[u8]>>) -> (Metadata, u32) {
    let mut m = Metadata::default();
    let Some(info) = decoder.info() else {
        return (m, 1);
    };
    m.push("JPEG", "Size", format!("{} x {}", info.width, info.height));
    if let Some(cs) = decoder.input_colorspace() {
        m.push("JPEG", "Color space", format!("{cs:?}"));
    }
    m.push("JPEG", "Components", info.components);
    m.push("JPEG", "Coding", format!("{:?}", info.sof));
    if info.x_density > 0 && info.y_density > 0 {
        let unit = match info.pixel_density {
            1 => "dpi",
            2 => "dpcm",
            _ => "(aspect ratio)",
        };
        m.push(
            "JPEG",
            "Density",
            format!("{} x {} {unit}", info.x_density, info.y_density),
        );
    }
    if decoder.icc_profile().is_some() {
        m.push("JPEG", "ICC profile", "present");
    }
    let orientation = decoder
        .exif()
        .map_or(1, |raw| vw_core::exif::push_exif(&mut m, raw.clone()));
    (m, orientation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jpeg_encoder::{ColorType, Encoder};
    use vw_core::exif::sample;

    /// テスト用の JPEG。`exif` があれば APP1 に入れる。
    fn encode(w: u16, h: u16, color: ColorType, data: &[u8], exif: Option<Vec<u8>>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = Encoder::new(&mut out, 100);
        if let Some(tiff) = exif {
            let mut app1 = b"Exif\0\0".to_vec();
            app1.extend(tiff);
            enc.add_app_segment(1, app1).unwrap();
        }
        enc.encode(data, w, h, color).unwrap();
        out
    }

    #[test]
    fn probe_accepts_jpeg_start_of_image_only() {
        assert!(JpegPlugin.probe(b"\xFF\xD8\xFF\xE0"));
        assert!(!JpegPlugin.probe(b"\x89PNG\r\n\x1a\n"));
        assert!(!JpegPlugin.probe(b"\xFF\xD8"));
    }

    #[test]
    fn decodes_color_jpeg_as_rgb8() {
        let jpg = encode(8, 8, ColorType::Rgb, &[200u8, 30, 30].repeat(64), None);
        let img = JpegPlugin.decode(&jpg).unwrap();
        assert_eq!((img.width, img.height, img.color), (8, 8, ColorModel::Rgb));
        let PixelData::U8(v) = &img.data else {
            panic!("8bit expected")
        };
        // 非可逆なので値はぴったりにはならない。赤が強いことだけ確かめる。
        assert!(v[0] > 150 && v[1] < 80 && v[2] < 80, "{:?}", &v[..3]);
        assert_eq!(img.metadata.get("JPEG", "Size"), Some("8 x 8"));
    }

    #[test]
    fn decodes_gray_jpeg_as_gray() {
        let jpg = encode(4, 2, ColorType::Luma, &[128u8; 8], None);
        let img = JpegPlugin.decode(&jpg).unwrap();
        assert_eq!((img.width, img.height, img.color), (4, 2, ColorModel::Gray));
    }

    #[test]
    fn decodes_cmyk_jpeg_as_rgb8() {
        let jpg = encode(4, 4, ColorType::Cmyk, &[0u8, 255, 255, 0].repeat(16), None);
        let img = JpegPlugin.decode(&jpg).unwrap();
        assert_eq!(img.color, ColorModel::Rgb);
    }

    #[test]
    fn exif_orientation_turns_the_image_upright() {
        let jpg = encode(
            16,
            8,
            ColorType::Luma,
            &[0u8; 128],
            Some(sample::orientation(6)),
        );
        let img = JpegPlugin.decode(&jpg).unwrap();
        assert_eq!((img.width, img.height), (8, 16));
        assert_eq!(
            img.metadata.get("EXIF", "Orientation"),
            Some("row 0 at right and column 0 at top")
        );
    }

    #[test]
    fn metadata_can_be_read_without_decoding_pixels() {
        let jpg = encode(
            16,
            8,
            ColorType::Luma,
            &[0u8; 128],
            Some(sample::orientation(3)),
        );
        let m = JpegPlugin.read_metadata(&jpg).unwrap();
        assert_eq!(m.get("JPEG", "Size"), Some("16 x 8"));
        assert!(m.get("EXIF", "Orientation").is_some());
    }

    #[test]
    fn broken_exif_does_not_stop_the_image_from_opening() {
        let jpg = encode(
            4,
            4,
            ColorType::Luma,
            &[0u8; 16],
            Some(b"II*\0garbage".to_vec()),
        );
        assert!(JpegPlugin.decode(&jpg).is_ok());
    }

    #[test]
    fn file_that_is_not_jpeg_says_so() {
        let err = JpegPlugin.decode(b"this is not an image\n").unwrap_err();
        assert_eq!(err.to_string(), "invalid data: not a JPEG file");
    }

    #[test]
    fn truncated_file_is_an_error() {
        let jpg = encode(8, 8, ColorType::Luma, &[0u8; 64], None);
        assert!(JpegPlugin.decode(&jpg[..20]).is_err());
    }
}
