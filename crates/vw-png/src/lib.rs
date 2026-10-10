//! PNG デコーダプラグイン。
//!
//! - 16bit のサンプルは 16bit のまま [`PixelData::U16`] にする
//! - パレット、1/2/4bit、tRNS（透過色）は 8bit のグレー / RGB / RGBA に展開する
//! - テキストチャンク（tEXt / zTXt / iTXt）と主要な補助チャンクをメタデータにする
//!
//! APNG は最初のフレームだけを表示する。

use std::io::Cursor;

use png::{BitDepth, ColorType, Transformations};
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

/// PNG ファイルの先頭 8 バイト。
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
        // EXPAND だけを指定し STRIP_16 は付けない。8bit に落とすと DICOM 由来などの
        // 16bit PNG で階調が失われ、後からウィンドウ調整できなくなるため。
        decoder.set_transformations(Transformations::EXPAND);
        let mut reader = decoder.read_info().map_err(invalid)?;
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| DecodeError::Invalid("image too large".into()))?;
        let mut buf = vec![0u8; size];
        let frame = reader.next_frame(&mut buf).map_err(invalid)?;
        buf.truncate(frame.buffer_size());
        // IDAT の後ろにあるテキストチャンクも読むために最後まで進める。
        // 末尾が壊れていても画素は読めているので、ここでの失敗は無視して表示を優先する。
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
        // sBIT があれば有効ビット数として使う（12bit 医用画像を 16bit PNG に入れた場合など）。
        // チャンネルごとに値が違うこともあるが、表示の上限は 1 つなので最大値を採る。
        if let (PixelData::U16(_), Some(sbit)) = (&img.data, &reader.info().sbit) {
            if let Some(&bits) = sbit.iter().max() {
                img.significant_bits = (bits as u32).clamp(1, 16);
            }
        }
        img.metadata = metadata(reader.info());
        Ok(img)
    }

    /// IDAT の前までしか読まないので速いが、IDAT の後ろにあるテキストチャンクは含まれない。
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

/// PNG のヘッダと補助チャンクを、表示用のメタデータに変換する。
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

    /// テスト用の PNG を作る。`extra` は IHDR の直後に書き込む追加チャンク。
    fn encode_with(
        w: u32,
        h: u32,
        color: ColorType,
        depth: BitDepth,
        data: &[u8],
        setup: impl FnOnce(&mut png::Encoder<&mut Vec<u8>>),
        extra: &[(png::chunk::ChunkType, &[u8])],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(depth);
        setup(&mut enc);
        let mut writer = enc.write_header().unwrap();
        for (ty, body) in extra {
            writer.write_chunk(*ty, body).unwrap();
        }
        writer.write_image_data(data).unwrap();
        writer.finish().unwrap();
        out
    }

    fn encode(w: u32, h: u32, color: ColorType, depth: BitDepth, data: &[u8]) -> Vec<u8> {
        encode_with(w, h, color, depth, data, |_| {}, &[])
    }

    #[test]
    fn probe_accepts_png_signature_only() {
        assert!(PngPlugin.probe(SIGNATURE));
        assert!(!PngPlugin.probe(b"BM......"));
        assert!(!PngPlugin.probe(b"\x89PN"));
    }

    #[test]
    fn decodes_rgb8_with_text() {
        let png = encode_with(
            2,
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            &[1, 2, 3, 4, 5, 6],
            |e| e.add_text_chunk("Title".into(), "vw test".into()).unwrap(),
            &[],
        );
        assert!(PngPlugin.probe(&png));
        let img = PngPlugin.decode(&png).unwrap();
        assert_eq!((img.width, img.height, img.color), (2, 1, ColorModel::Rgb));
        assert!(matches!(&img.data, PixelData::U8(v) if v == &[1, 2, 3, 4, 5, 6]));
        assert_eq!(img.metadata.get("Text", "Title"), Some("vw test"));
    }

    #[test]
    fn decodes_compressed_and_utf8_text() {
        let png = encode_with(
            1,
            1,
            ColorType::Grayscale,
            BitDepth::Eight,
            &[0],
            |e| {
                e.add_ztxt_chunk("Comment".into(), "compressed latin-1".into())
                    .unwrap();
                e.add_itxt_chunk("Description".into(), "日本語の説明".into())
                    .unwrap();
            },
            &[],
        );
        let img = PngPlugin.decode(&png).unwrap();
        assert_eq!(
            img.metadata.get("Text", "Comment"),
            Some("compressed latin-1")
        );
        assert_eq!(
            img.metadata.get("Text", "Description"),
            Some("日本語の説明")
        );
    }

    #[test]
    fn keeps_16_bit_gray() {
        let png = encode(1, 1, ColorType::Grayscale, BitDepth::Sixteen, &[0x12, 0x34]);
        let img = PngPlugin.decode(&png).unwrap();
        assert!(matches!(&img.data, PixelData::U16(v) if v == &[0x1234]));
        assert_eq!(img.significant_bits, 16);
    }

    #[test]
    fn keeps_16_bit_rgba() {
        let data = [0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0xff, 0xff];
        let png = encode(1, 1, ColorType::Rgba, BitDepth::Sixteen, &data);
        let img = PngPlugin.decode(&png).unwrap();
        assert_eq!(img.color, ColorModel::Rgba);
        assert!(matches!(&img.data, PixelData::U16(v) if v == &[1, 2, 3, 0xffff]));
    }

    #[test]
    fn sbit_sets_significant_bits_for_16_bit() {
        let png = encode_with(
            1,
            1,
            ColorType::Grayscale,
            BitDepth::Sixteen,
            &[0x0f, 0xff],
            |_| {},
            &[(png::chunk::sBIT, &[12])],
        );
        let img = PngPlugin.decode(&png).unwrap();
        assert_eq!(img.significant_bits, 12);
    }

    #[test]
    fn decodes_gray_alpha() {
        let png = encode(1, 1, ColorType::GrayscaleAlpha, BitDepth::Eight, &[100, 50]);
        let img = PngPlugin.decode(&png).unwrap();
        assert_eq!(img.color, ColorModel::GrayAlpha);
        assert!(matches!(&img.data, PixelData::U8(v) if v == &[100, 50]));
    }

    #[test]
    fn expands_palette_to_rgb() {
        let png = encode_with(
            2,
            1,
            ColorType::Indexed,
            BitDepth::Eight,
            &[1, 0],
            |e| e.set_palette(vec![10, 20, 30, 40, 50, 60]),
            &[],
        );
        let img = PngPlugin.decode(&png).unwrap();
        assert_eq!(img.color, ColorModel::Rgb);
        assert!(matches!(&img.data, PixelData::U8(v) if v == &[40, 50, 60, 10, 20, 30]));
    }

    #[test]
    fn expands_palette_with_transparency_to_rgba() {
        let png = encode_with(
            1,
            1,
            ColorType::Indexed,
            BitDepth::Eight,
            &[0],
            |e| {
                e.set_palette(vec![10, 20, 30]);
                e.set_trns(vec![0]);
            },
            &[],
        );
        let img = PngPlugin.decode(&png).unwrap();
        assert_eq!(img.color, ColorModel::Rgba);
        assert!(matches!(&img.data, PixelData::U8(v) if v == &[10, 20, 30, 0]));
    }

    #[test]
    fn expands_1_bit_gray_to_8_bit() {
        // 8 画素 = 1 バイト。1 が白、0 が黒。
        let png = encode(8, 1, ColorType::Grayscale, BitDepth::One, &[0b1010_0000]);
        let img = PngPlugin.decode(&png).unwrap();
        assert!(matches!(&img.data, PixelData::U8(v) if v == &[255, 0, 255, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn metadata_reports_size_and_bit_depth() {
        let png = encode(3, 2, ColorType::Rgb, BitDepth::Eight, &[0; 18]);
        let m = PngPlugin.read_metadata(&png).unwrap();
        assert_eq!(m.get("PNG", "Size"), Some("3 x 2"));
        assert_eq!(m.get("PNG", "Bit depth"), Some("8"));
        assert_eq!(m.get("PNG", "Interlaced"), Some("false"));
    }

    #[test]
    fn rejects_non_png() {
        assert!(PngPlugin.decode(b"not a png").is_err());
    }

    #[test]
    fn rejects_truncated_png() {
        let png = encode(16, 16, ColorType::Rgb, BitDepth::Eight, &[7; 16 * 16 * 3]);
        let truncated = &png[..png.len() / 2];
        assert!(matches!(
            PngPlugin.decode(truncated),
            Err(DecodeError::Invalid(_))
        ));
    }
}
