//! PNG に書く（貼り合わせた画像の保存など）。値はそのまま、8bit は 8bit、16bit は 16bit で書く。

use png::{BitDepth, ColorType};
use vw_core::{ColorModel, PixelData, VwImage};

/// `img` を PNG にする。`description` は tEXt の Description に入れる。
/// 実数の画像は PNG に入らないので断る（丸めると値が変わってしまうため、TIFF か FITS で書く）。
pub fn encode(img: &VwImage, description: &str) -> Result<Vec<u8>, String> {
    let color = match img.color {
        ColorModel::Gray => ColorType::Grayscale,
        ColorModel::GrayAlpha => ColorType::GrayscaleAlpha,
        ColorModel::Rgb => ColorType::Rgb,
        ColorModel::Rgba => ColorType::Rgba,
    };
    let (depth, bytes): (BitDepth, Vec<u8>) = match &img.data {
        PixelData::U8(v) => (BitDepth::Eight, v.clone()),
        PixelData::U16(v) => (
            BitDepth::Sixteen,
            v.iter().flat_map(|x| x.to_be_bytes()).collect(),
        ),
        PixelData::F32(_) => {
            return Err("PNG cannot hold real numbers: save as TIFF or FITS".to_owned());
        }
    };
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.width, img.height);
    enc.set_color(color);
    enc.set_depth(depth);
    enc.add_text_chunk("Description".to_owned(), description.to_owned())
        .map_err(|e| e.to_string())?;
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&bytes).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}
