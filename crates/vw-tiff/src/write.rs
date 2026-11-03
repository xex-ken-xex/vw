//! TIFF に書く（貼り合わせた画像の保存など）。値はそのまま、8bit・16bit の整数と 32bit の実数で書く。

use std::io::Cursor;

use tiff::encoder::colortype::{self, ColorType};
use tiff::encoder::{Compression, TiffEncoder, TiffValue};
use tiff::tags::Tag;
use vw_core::{ColorModel, PixelData, VwImage};

/// `img` を Deflate で圧縮した TIFF にする。`description` は ImageDescription に入れる。
/// 灰色 + アルファは、TIFF の書き手に型がないので RGBA にする。
pub fn encode(img: &VwImage, description: &str) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    let mut enc = TiffEncoder::new(&mut out)
        .map_err(|e| e.to_string())?
        .with_compression(Compression::Deflate(Default::default()));
    let (w, h) = (img.width, img.height);
    let gray_alpha = img.color == ColorModel::GrayAlpha;
    let model = if gray_alpha {
        ColorModel::Rgba
    } else {
        img.color
    };
    macro_rules! write {
        ($data:expr, $gray:ty, $rgb:ty, $rgba:ty) => {{
            let data = if gray_alpha {
                spread_gray($data)
            } else {
                $data.to_vec()
            };
            match model {
                ColorModel::Gray => image::<$gray, _>(&mut enc, w, h, description, &data),
                ColorModel::Rgb => image::<$rgb, _>(&mut enc, w, h, description, &data),
                _ => image::<$rgba, _>(&mut enc, w, h, description, &data),
            }
        }};
    }
    match &img.data {
        PixelData::U8(v) => write!(v, colortype::Gray8, colortype::RGB8, colortype::RGBA8),
        PixelData::U16(v) => write!(v, colortype::Gray16, colortype::RGB16, colortype::RGBA16),
        PixelData::F32(v) => write!(
            v,
            colortype::Gray32Float,
            colortype::RGB32Float,
            colortype::RGBA32Float
        ),
    }?;
    Ok(out.into_inner())
}

fn spread_gray<T: Copy>(data: &[T]) -> Vec<T> {
    data.as_chunks::<2>()
        .0
        .iter()
        .flat_map(|&[v, a]| [v, v, v, a])
        .collect()
}

fn image<C: ColorType, W: std::io::Write + std::io::Seek>(
    enc: &mut TiffEncoder<W>,
    width: u32,
    height: u32,
    description: &str,
    data: &[C::Inner],
) -> Result<(), String>
where
    [C::Inner]: TiffValue,
{
    let mut image = enc
        .new_image::<C>(width, height)
        .map_err(|e| e.to_string())?;
    if !description.is_empty() {
        image
            .encoder()
            .write_tag(Tag::ImageDescription, description)
            .map_err(|e| e.to_string())?;
    }
    image.write_data(data).map_err(|e| e.to_string())
}
