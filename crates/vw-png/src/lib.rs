//! PNG decoder plugin. Keeps 16-bit samples as 16-bit; expands palettes and
//! low bit depths to 8-bit.

use std::io::Cursor;

use png::{BitDepth, ColorType, Transformations};
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

pub struct PngPlugin;

impl DecoderPlugin for PngPlugin {
    fn name(&self) -> &str {
        "PNG"
    }

    fn extensions(&self) -> &[&str] {
        &["png"]
    }

    fn probe(&self, header: &[u8]) -> bool {
        header.starts_with(SIGNATURE)
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        let mut decoder = png::Decoder::new(Cursor::new(data));
        decoder.set_transformations(Transformations::EXPAND);
        let mut reader = decoder.read_info().map_err(invalid)?;
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| DecodeError::Invalid("image too large".into()))?;
        let mut buf = vec![0u8; size];
        let frame = reader.next_frame(&mut buf).map_err(invalid)?;
        buf.truncate(frame.buffer_size());
        // Text chunks may follow the image data.
        let _ = reader.finish();

        let color = match frame.color_type {
            ColorType::Grayscale => ColorModel::Gray,
            ColorType::GrayscaleAlpha => ColorModel::GrayAlpha,
            ColorType::Rgb => ColorModel::Rgb,
            ColorType::Rgba => ColorModel::Rgba,
            ColorType::Indexed => {
                return Err(DecodeError::Invalid("palette was not expanded".into()))
            }
        };
        let pixels = match frame.bit_depth {
            BitDepth::Sixteen => PixelData::U16(
                buf.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&b| u16::from_be_bytes(b))
                    .collect(),
            ),
            _ => PixelData::U8(buf),
        };
        let mut img =
            VwImage::new(frame.width, frame.height, color, pixels).map_err(DecodeError::Invalid)?;
        if let (PixelData::U16(_), Some(sbit)) = (&img.data, &reader.info().sbit) {
            if let Some(&bits) = sbit.iter().max() {
                img.significant_bits = (bits as u32).clamp(1, 16);
            }
        }
        img.metadata = metadata(reader.info());
        Ok(img)
    }

    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        let reader = png::Decoder::new(Cursor::new(data))
            .read_info()
            .map_err(invalid)?;
        Ok(metadata(reader.info()))
    }
}

fn invalid(e: png::DecodingError) -> DecodeError {
    DecodeError::Invalid(e.to_string())
}

fn metadata(info: &png::Info) -> Metadata {
    let mut m = Metadata::default();
    m.push("PNG", "Size", format!("{} x {}", info.width, info.height));
    m.push("PNG", "Color type", format!("{:?}", info.color_type));
    m.push("PNG", "Bit depth", info.bit_depth as u8);
    m.push("PNG", "Interlaced", info.interlaced);
    if let Some(d) = &info.pixel_dims {
        m.push(
            "PNG",
            "Pixel density",
            format!("{} x {} per {:?}", d.xppu, d.yppu, d.unit),
        );
    }
    if let Some(g) = info.source_gamma {
        m.push("PNG", "Gamma", g.into_value());
    }
    if let Some(s) = &info.srgb {
        m.push("PNG", "sRGB intent", format!("{s:?}"));
    }
    if info.icc_profile.is_some() {
        m.push("PNG", "ICC profile", "present");
    }
    if let Some(a) = &info.animation_control {
        m.push("PNG", "Animation frames", a.num_frames);
    }
    for t in &info.uncompressed_latin1_text {
        m.push("Text", &t.keyword, &t.text);
    }
    for t in &info.compressed_latin1_text {
        if let Ok(text) = t.get_text() {
            m.push("Text", &t.keyword, text);
        }
    }
    for t in &info.utf8_text {
        if let Ok(text) = t.get_text() {
            m.push("Text", &t.keyword, text);
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(w: u32, h: u32, color: ColorType, depth: BitDepth, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(depth);
        enc.add_text_chunk("Title".into(), "vw test".into())
            .unwrap();
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(data).unwrap();
        writer.finish().unwrap();
        out
    }

    #[test]
    fn decodes_rgb8_with_text() {
        let png = encode(2, 1, ColorType::Rgb, BitDepth::Eight, &[1, 2, 3, 4, 5, 6]);
        assert!(PngPlugin.probe(&png));
        let img = PngPlugin.decode(&png).unwrap();
        assert_eq!((img.width, img.height, img.color), (2, 1, ColorModel::Rgb));
        assert!(matches!(&img.data, PixelData::U8(v) if v == &[1, 2, 3, 4, 5, 6]));
        assert!(img
            .metadata
            .entries
            .iter()
            .any(|(_, k, v)| k == "Title" && v == "vw test"));
    }

    #[test]
    fn keeps_16_bit_gray() {
        let png = encode(1, 1, ColorType::Grayscale, BitDepth::Sixteen, &[0x12, 0x34]);
        let img = PngPlugin.decode(&png).unwrap();
        assert!(matches!(&img.data, PixelData::U16(v) if v == &[0x1234]));
    }

    #[test]
    fn rejects_non_png() {
        assert!(!PngPlugin.probe(b"BM......"));
        assert!(PngPlugin.decode(b"not a png").is_err());
    }
}
