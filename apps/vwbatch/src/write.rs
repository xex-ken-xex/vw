//! 書き出す形式。

use std::io::Write;
use std::path::Path;

use crate::convert::Picture;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// PNG、1 色 8bit
    Png,
    /// PNG、1 色 16bit
    Png16,
    /// TIFF、1 色 16bit（Deflate で圧縮）
    Tiff,
    /// JPEG（品質は --quality）
    Jpeg,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png | Format::Png16 => "png",
            Format::Tiff => "tif",
            Format::Jpeg => "jpg",
        }
    }
}

pub fn encode(picture: &Picture, format: Format, quality: u8) -> Result<Vec<u8>, String> {
    match format {
        Format::Png => png(picture, png::BitDepth::Eight),
        Format::Png16 => png(picture, png::BitDepth::Sixteen),
        Format::Tiff => tiff(picture),
        Format::Jpeg => jpeg(picture, quality),
    }
}

/// 同じ名前のファイルがあれば書かない（`Ok(false)`）。確かめてから作るまでの間に
/// ほかから作られても上書きしないよう、作るときに「新しく作る」を指定する。
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    file.write_all(bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

fn level8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

fn level16(v: f32) -> u16 {
    (v * 65535.0).round().clamp(0.0, 65535.0) as u16
}

fn png(p: &Picture, depth: png::BitDepth) -> Result<Vec<u8>, String> {
    let color = match p.channels {
        1 => png::ColorType::Grayscale,
        2 => png::ColorType::GrayscaleAlpha,
        3 => png::ColorType::Rgb,
        _ => png::ColorType::Rgba,
    };
    let bytes: Vec<u8> = match depth {
        png::BitDepth::Sixteen => p
            .data
            .iter()
            .flat_map(|&v| level16(v).to_be_bytes())
            .collect(),
        _ => p.data.iter().map(|&v| level8(v)).collect(),
    };
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, p.width as u32, p.height as u32);
    enc.set_color(color);
    enc.set_depth(depth);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&bytes).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

fn tiff(p: &Picture) -> Result<Vec<u8>, String> {
    use tiff::encoder::{Compression, TiffEncoder, colortype};
    // TIFF の書き手には灰色 + アルファの型がないので、そのときは RGBA にする。
    let rgba;
    let p = match p.channels {
        2 => {
            rgba = Picture {
                channels: 4,
                data: p
                    .data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .flat_map(|&[v, a]| [v, v, v, a])
                    .collect(),
                ..*p
            };
            &rgba
        }
        _ => p,
    };
    let data: Vec<u16> = p.data.iter().map(|&v| level16(v)).collect();
    let mut out = std::io::Cursor::new(Vec::new());
    let mut enc = TiffEncoder::new(&mut out)
        .map_err(|e| e.to_string())?
        .with_compression(Compression::Deflate(Default::default()));
    let (w, h) = (p.width as u32, p.height as u32);
    match p.channels {
        1 => enc.write_image::<colortype::Gray16>(w, h, &data),
        3 => enc.write_image::<colortype::RGB16>(w, h, &data),
        _ => enc.write_image::<colortype::RGBA16>(w, h, &data),
    }
    .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

fn jpeg(p: &Picture, quality: u8) -> Result<Vec<u8>, String> {
    use jpeg_encoder::{ColorType, Encoder};
    let p = p.without_alpha();
    let (Ok(w), Ok(h)) = (u16::try_from(p.width), u16::try_from(p.height)) else {
        return Err(format!(
            "JPEG cannot hold {}x{} pixels (65535 at most)",
            p.width, p.height
        ));
    };
    let bytes: Vec<u8> = p.data.iter().map(|&v| level8(v)).collect();
    let color = match p.channels {
        1 => ColorType::Luma,
        _ => ColorType::Rgb,
    };
    let mut out = Vec::new();
    Encoder::new(&mut out, quality)
        .encode(&bytes, w, h, color)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_core::DecoderPlugin;

    fn gradient(channels: usize) -> Picture {
        let (width, height) = (4, 2);
        let data = (0..width * height * channels)
            .map(|i| i as f32 / (width * height * channels - 1) as f32)
            .collect();
        Picture {
            width,
            height,
            channels,
            data,
        }
    }

    #[test]
    fn png16_keeps_sixteen_bit_levels() {
        let p = gradient(1);
        let bytes = encode(&p, Format::Png16, 90).unwrap();
        let image = vw_png::PngPlugin.decode(&bytes).unwrap();
        let vw_core::PixelData::U16(v) = image.data else {
            panic!("not 16 bit");
        };
        assert_eq!(v[1], level16(p.data[1]));
        assert_eq!(v[7], 65535);
    }

    #[test]
    fn every_format_reads_back_with_the_same_size_and_color() {
        for (channels, format, color) in [
            (1, Format::Png, vw_core::ColorModel::Gray),
            (4, Format::Png, vw_core::ColorModel::Rgba),
            (2, Format::Tiff, vw_core::ColorModel::Rgba),
            (3, Format::Tiff, vw_core::ColorModel::Rgb),
            (4, Format::Jpeg, vw_core::ColorModel::Rgb),
            (1, Format::Jpeg, vw_core::ColorModel::Gray),
        ] {
            let bytes = encode(&gradient(channels), format, 95).unwrap();
            let image = match format {
                Format::Tiff => vw_tiff::TiffPlugin.decode(&bytes),
                Format::Jpeg => vw_jpeg::JpegPlugin.decode(&bytes),
                _ => vw_png::PngPlugin.decode(&bytes),
            }
            .unwrap();
            assert_eq!(
                (image.width, image.height, image.color),
                (4, 2, color),
                "{channels} {format:?}"
            );
        }
    }

    #[test]
    fn an_existing_file_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("vwbatch-write-{}", std::process::id()));
        let path = dir.join("sub").join("a.png");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(write_new(&path, b"first"), Ok(true));
        assert_eq!(write_new(&path, b"second"), Ok(false));
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
