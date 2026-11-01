//! BMP デコーダプラグイン。
//!
//! - 1 / 4 / 8bit のパレット、RLE4 / RLE8、16 / 24 / 32bit、上下逆（トップダウン）の BMP を読む
//! - 8bit のグレーと、パレットが灰色だけの画像はグレーにする
//! - 32bit でアルファがすべて 0（またはすべて 255）の画像は不透明の RGB にする

mod header;
mod rgb16;
mod rle4;

use header::{BI_ALPHABITFIELDS, BI_BITFIELDS, BI_RGB, BI_RLE4, Header};
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};
use zune_bmp::BmpDecoder;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

/// BMP の幅と高さの上限。zune-bmp の既定の上限（16384）では、大きなスキャン画像が開けないため。
const MAX_SIDE: usize = 65_535;

/// zune-bmp が読み込みに必要とする最小のバイト数（ファイルヘッダ 14 + 情報ヘッダ 40 + ビットフィールド 12）。
const MIN_DECODABLE_LEN: usize = 14 + 40 + 12;

pub struct BmpPlugin;

impl DecoderPlugin for BmpPlugin {
    fn name(&self) -> &str {
        "BMP"
    }

    fn extensions(&self) -> &[&str] {
        &["bmp", "dib"]
    }

    fn probe(&self, header: &[u8]) -> bool {
        zune_bmp::probe_bmp(header)
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        if !self.probe(data) {
            return Err(DecodeError::Invalid("not a BMP file".into()));
        }
        let header = Header::parse(data)?;
        // zune-bmp は RGBA の 8bit で返すので、1 画素は最大 4 バイト。
        vw_core::limits::check_image(
            header.width.unsigned_abs() as u64,
            header.height.unsigned_abs() as u64,
            4,
        )?;
        let own = match (header.bits, header.compression) {
            (4, BI_RLE4) => Some(rle4::decode(data, &header)?),
            (16, BI_RGB | BI_BITFIELDS | BI_ALPHABITFIELDS) => Some(rgb16::decode(data, &header)?),
            _ => None,
        };
        let (width, height, color, pixels) = match own {
            Some(rgb) => {
                let (color, pixels) = rgb_or_gray(rgb);
                let size = (header.width.unsigned_abs(), header.height.unsigned_abs());
                (size.0 as usize, size.1 as usize, color, pixels)
            }
            None => decode_with_zune(data)?,
        };
        let mut img = VwImage::new(width as u32, height as u32, color, PixelData::U8(pixels))
            .map_err(DecodeError::Invalid)?;
        img.metadata = metadata(&header);
        Ok(img)
    }

    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        if !self.probe(data) {
            return Err(DecodeError::Invalid("not a BMP file".into()));
        }
        Ok(metadata(&Header::parse(data)?))
    }
}

fn decode_with_zune(data: &[u8]) -> Result<(usize, usize, ColorModel, Vec<u8>), DecodeError> {
    let options = DecoderOptions::default()
        .set_max_width(MAX_SIDE)
        .set_max_height(MAX_SIDE);
    // zune-bmp は BITMAPINFOHEADER の後ろに必ず 12 バイトのビットフィールドがあるものとして読むため、
    // 1x1 のような小さいファイルは途中で終わったと見なされる。読み込みを直すより、末尾に 0 を
    // 足して渡すほうが変更が小さい（画素は先頭のオフセットから読むので、足した分は画素にならない）。
    let padded;
    let data = if data.len() < MIN_DECODABLE_LEN {
        padded = [
            data,
            &[0; MIN_DECODABLE_LEN][..MIN_DECODABLE_LEN - data.len()],
        ]
        .concat();
        &padded[..]
    } else {
        data
    };
    let mut decoder = BmpDecoder::new_with_options(ZCursor::new(data), options);
    let pixels = decoder.decode().map_err(invalid)?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| DecodeError::Invalid("no BMP header".into()))?;
    let (color, pixels) = match decoder.colorspace() {
        Some(ColorSpace::Luma) => (ColorModel::Gray, pixels),
        Some(ColorSpace::RGB) => rgb_or_gray(pixels),
        Some(ColorSpace::RGBA) => drop_unused_alpha(pixels),
        other => {
            return Err(DecodeError::Unsupported(format!(
                "BMP colour space {other:?}"
            )));
        }
    };
    Ok((width, height, color, pixels))
}

fn invalid(e: zune_bmp::BmpDecoderErrors) -> DecodeError {
    DecodeError::Invalid(format!("{e:?}"))
}

/// R = G = B の画素だけならグレーにする。パレットの BMP は zune-bmp が RGB に広げて返すため、
/// グレーのパレット（スキャナの画像に多い）もここでグレーに戻す。
fn rgb_or_gray(pixels: Vec<u8>) -> (ColorModel, Vec<u8>) {
    let gray = pixels
        .as_chunks::<3>()
        .0
        .iter()
        .all(|[r, g, b]| r == g && g == b);
    if gray {
        (
            ColorModel::Gray,
            pixels.as_chunks::<3>().0.iter().map(|p| p[0]).collect(),
        )
    } else {
        (ColorModel::Rgb, pixels)
    }
}

/// アルファが使われていなければ RGB にする。32bit の BMP の多くは 4 バイト目を使わずに 0 で
/// 埋めていて、そのままアルファとして扱うと画像全体が透明になるため。
fn drop_unused_alpha(pixels: Vec<u8>) -> (ColorModel, Vec<u8>) {
    let first = pixels.get(3).copied().unwrap_or(255);
    let uniform =
        (first == 0 || first == 255) && pixels.as_chunks::<4>().0.iter().all(|p| p[3] == first);
    if uniform {
        let rgb = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        (ColorModel::Rgb, rgb)
    } else {
        (ColorModel::Rgba, pixels)
    }
}

/// ヘッダを表示用のメタデータにする。
fn metadata(header: &Header) -> Metadata {
    let mut m = Metadata::default();
    let version = match header.info_size {
        12 => "OS/2 1.x (BITMAPCOREHEADER)",
        16 | 64 => "OS/2 2.x",
        40 => "Windows 3.x (BITMAPINFOHEADER)",
        52 | 56 => "Windows (BITMAPV2/V3INFOHEADER)",
        108 => "Windows 95 (BITMAPV4HEADER)",
        124 => "Windows 98 (BITMAPV5HEADER)",
        _ => "unknown",
    };
    m.push("BMP", "Header", version);
    m.push(
        "BMP",
        "Size",
        format!("{} x {}", header.width, header.height.unsigned_abs()),
    );
    let order = if header.top_down() {
        "top-down"
    } else {
        "bottom-up"
    };
    m.push("BMP", "Row order", order);
    m.push("BMP", "Bits per pixel", header.bits);
    if header.info_size >= 40 {
        let name = match header.compression {
            0 => "none (RGB)".to_string(),
            1 => "RLE8".to_string(),
            2 => "RLE4".to_string(),
            3 => "bit fields".to_string(),
            4 => "JPEG".to_string(),
            5 => "PNG".to_string(),
            6 => "alpha bit fields".to_string(),
            other => format!("unknown ({other})"),
        };
        m.push("BMP", "Compression", name);
    }
    if let Some((x, y)) = header.resolution {
        let dpi = |v: u32| (f64::from(v) * 0.0254).round();
        m.push("BMP", "Resolution", format!("{} x {} dpi", dpi(x), dpi(y)));
    }
    if header.colors_used > 0 {
        m.push("BMP", "Palette colors", header.colors_used);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用の BITMAPINFOHEADER の BMP。`rows` は下の行から順に、行末の詰め物なしで渡す。
    fn bmp(width: i32, height: i32, bits: u16, palette: &[[u8; 3]], rows: &[Vec<u8>]) -> Vec<u8> {
        let stride = rows[0].len().div_ceil(4) * 4;
        let offset = 14 + 40 + palette.len() * 4;
        let size = offset + stride * rows.len();
        let mut out = Vec::new();
        out.extend(b"BM");
        out.extend((size as u32).to_le_bytes());
        out.extend([0u8; 4]);
        out.extend((offset as u32).to_le_bytes());
        out.extend(40u32.to_le_bytes());
        out.extend(width.to_le_bytes());
        out.extend(height.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(bits.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend(((stride * rows.len()) as u32).to_le_bytes());
        out.extend(3780u32.to_le_bytes());
        out.extend(3780u32.to_le_bytes());
        out.extend((palette.len() as u32).to_le_bytes());
        out.extend(0u32.to_le_bytes());
        for [r, g, b] in palette {
            out.extend([*b, *g, *r, 0]);
        }
        for row in rows {
            out.extend(row);
            out.resize(out.len() + stride - row.len(), 0);
        }
        out
    }

    fn u8_pixels(img: &VwImage) -> &[u8] {
        match &img.data {
            PixelData::U8(v) => v,
            _ => panic!("8bit expected"),
        }
    }

    #[test]
    fn huge_dimensions_in_the_header_are_refused_before_allocating() {
        let file = bmp(60_000, 60_000, 32, &[], &[vec![0; 4]]);
        let err = BmpPlugin.decode(&file).unwrap_err().to_string();
        assert!(err.contains("60000 x 60000"), "{err}");
        assert!(err.contains("image size limit"), "{err}");
    }

    #[test]
    fn probe_accepts_bmp_with_a_known_header_size_only() {
        let file = bmp(1, 1, 24, &[], &[vec![0, 0, 0]]);
        assert!(BmpPlugin.probe(&file));
        assert!(!BmpPlugin.probe(b"BMxxxxxxxxxxxxxxxxxx"));
        assert!(!BmpPlugin.probe(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn decodes_24bit_bmp_as_rgb_with_the_bottom_row_last() {
        // 下の行が赤、上の行が青。ファイルは下の行から並ぶ（BGR）。
        let file = bmp(1, 2, 24, &[], &[vec![0, 0, 255], vec![255, 0, 0]]);
        let img = BmpPlugin.decode(&file).unwrap();
        assert_eq!((img.width, img.height, img.color), (1, 2, ColorModel::Rgb));
        assert_eq!(u8_pixels(&img), [0, 0, 255, 255, 0, 0]);
    }

    #[test]
    fn top_down_bmp_keeps_the_first_row_on_top() {
        let file = bmp(1, -2, 24, &[], &[vec![0, 0, 255], vec![255, 0, 0]]);
        let img = BmpPlugin.decode(&file).unwrap();
        assert_eq!(u8_pixels(&img), [255, 0, 0, 0, 0, 255]);
        assert_eq!(img.metadata.get("BMP", "Row order"), Some("top-down"));
    }

    #[test]
    fn color_palette_bmp_is_expanded_to_rgb() {
        let palette = [[255, 0, 0], [0, 255, 0]];
        let file = bmp(2, 1, 8, &palette, &[vec![0, 1]]);
        let img = BmpPlugin.decode(&file).unwrap();
        assert_eq!(img.color, ColorModel::Rgb);
        assert_eq!(u8_pixels(&img), [255, 0, 0, 0, 255, 0]);
    }

    #[test]
    fn gray_palette_bmp_is_decoded_as_gray() {
        let palette: Vec<[u8; 3]> = (0..=255).map(|v| [v, v, v]).collect();
        let file = bmp(3, 1, 8, &palette, &[vec![0, 128, 255]]);
        let img = BmpPlugin.decode(&file).unwrap();
        assert_eq!(img.color, ColorModel::Gray);
        assert_eq!(u8_pixels(&img), [0, 128, 255]);
    }

    #[test]
    fn one_bit_bmp_uses_its_two_palette_colors() {
        let file = bmp(2, 1, 1, &[[0, 0, 0], [255, 255, 255]], &[vec![0b0100_0000]]);
        let img = BmpPlugin.decode(&file).unwrap();
        assert_eq!(u8_pixels(&img), [0, 255]);
    }

    #[test]
    fn thirty_two_bit_bmp_with_zero_alpha_is_opaque() {
        let file = bmp(1, 1, 32, &[], &[vec![0, 0, 255, 0]]);
        let img = BmpPlugin.decode(&file).unwrap();
        assert_eq!(img.color, ColorModel::Rgb);
        assert_eq!(u8_pixels(&img), [255, 0, 0]);
    }

    #[test]
    fn rle4_ending_with_a_line_end_before_the_image_end_is_decoded() {
        // 3x2、パレットは 0 = 黒 / 1 = 白。下の行は 3 画素の連続（白黒白）、上の行は生の 3 画素（黒白黒）。
        let mut file = bmp(3, 2, 4, &[[0, 0, 0], [255, 255, 255]], &[vec![0]]);
        file.truncate(14 + 40 + 8);
        file[30] = 2;
        file.extend([3, 0x10, 0, 0, 0, 3, 0x01, 0x00, 0, 0, 0, 1]);
        let img = BmpPlugin.decode(&file).unwrap();
        assert_eq!(img.color, ColorModel::Gray);
        assert_eq!(u8_pixels(&img), [0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn sixteen_bit_bmp_defaults_to_5_5_5_and_keeps_rows_of_a_wide_image_aligned() {
        // 3x2: 1 行 6 バイト + 詰め物 2 バイト。下の行が赤・緑・青、上の行が白・黒・白。
        let px = |v: u16| v.to_le_bytes();
        let bottom = [px(0x7c00), px(0x03e0), px(0x001f)].concat();
        let top = [px(0x7fff), px(0), px(0x7fff)].concat();
        let file = bmp(3, 2, 16, &[], &[bottom, top]);
        let img = BmpPlugin.decode(&file).unwrap();
        assert_eq!(img.color, ColorModel::Rgb);
        #[rustfmt::skip]
        assert_eq!(u8_pixels(&img), [
            255, 255, 255, 0, 0, 0, 255, 255, 255,
            255, 0, 0, 0, 255, 0, 0, 0, 255,
        ]);
    }

    #[test]
    fn sixteen_bit_bmp_reads_5_6_5_from_its_bit_fields() {
        let mut file = bmp(1, 1, 16, &[], &[0x07e0u16.to_le_bytes().to_vec()]);
        // ビットフィールドは BITMAPINFOHEADER の直後の 12 バイト。
        let masks: Vec<u8> = [0xf800u32, 0x07e0, 0x001f]
            .iter()
            .flat_map(|m| m.to_le_bytes())
            .collect();
        file.splice(54..54, masks);
        file[10] += 12;
        file[30] = 3;
        let img = BmpPlugin.decode(&file).unwrap();
        assert_eq!(u8_pixels(&img), [0, 255, 0]);
    }

    #[test]
    fn header_fields_become_metadata() {
        let file = bmp(1, 1, 24, &[], &[vec![0, 0, 0]]);
        let m = BmpPlugin.read_metadata(&file).unwrap();
        assert_eq!(m.get("BMP", "Bits per pixel"), Some("24"));
        assert_eq!(m.get("BMP", "Compression"), Some("none (RGB)"));
        assert_eq!(m.get("BMP", "Resolution"), Some("96 x 96 dpi"));
    }

    #[test]
    fn truncated_file_is_an_error_not_a_panic() {
        let file = bmp(4, 4, 24, &[], &vec![vec![0; 12]; 4]);
        assert!(BmpPlugin.decode(&file[..60]).is_err());
    }
}
