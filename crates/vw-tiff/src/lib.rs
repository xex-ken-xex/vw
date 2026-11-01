//! TIFF デコーダプラグイン。
//!
//! - 複数ページの TIFF は、ページごとに 1 枚として返す（[`DecoderPlugin::frame_count`]）。
//!   縮小版のページ（NewSubfileType で縮小版と印の付いたもの）はページに数えない
//! - 16bit のサンプルは 16bit のまま、浮動小数点と符号付き整数は [`PixelData::F32`] にする
//! - 1/2/4bit のグレーは 8bit に広げ、パレット（カラーマップ）は RGB に展開する
//! - CMYK と YCbCr は RGB に変換する
//! - 向き（Orientation）のタグに従って画素を並べ替える
//! - TIFF のタグ（カメラ名、作成ソフトなど）を EXIF と同じ形でメタデータにする

mod layout;

use std::io::Cursor;

use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

pub struct TiffPlugin;

/// 数えるページの上限。壊れたファイルで IFD の連なりが輪になっていても止まるように。
const MAX_PAGES: usize = 10_000;

/// EXIF の欄に出さないタグ。
///
/// - 画像の組み立て（大きさ・圧縮・色の種類など）は TIFF の欄に読みやすい形で出すので、重ねて出さない
/// - 画素の置き場所や色の表などは、ファイルの中の長い数の並びで、表示しても読めない
///   （XMP や Photoshop の情報も長いバイナリや XML になる）
const HIDDEN_TAGS: &[u16] = &[
    254,   // NewSubfileType
    256,   // ImageWidth
    257,   // ImageLength
    258,   // BitsPerSample
    259,   // Compression
    262,   // PhotometricInterpretation
    273,   // StripOffsets
    277,   // SamplesPerPixel
    278,   // RowsPerStrip
    279,   // StripByteCounts
    284,   // PlanarConfiguration
    317,   // Predictor
    320,   // ColorMap
    322,   // TileWidth
    323,   // TileLength
    324,   // TileOffsets
    325,   // TileByteCounts
    330,   // SubIFDs
    338,   // ExtraSamples
    339,   // SampleFormat
    347,   // JPEGTables
    530,   // YCbCrSubSampling
    700,   // XMP
    33723, // IPTC
    34377, // Photoshop
    34675, // ICC プロファイル
    37724, // ImageSourceData（Photoshop のレイヤー）
    50341, // PrintIM
];

impl DecoderPlugin for TiffPlugin {
    fn name(&self) -> &str {
        "TIFF"
    }

    fn extensions(&self) -> &[&str] {
        &["tif", "tiff"]
    }

    /// 中身では受け持たない。Nikon の NEF や Sony の ARW、DNG などのカメラ RAW も先頭が
    /// TIFF と同じなので、中身で受け持つとカメラ RAW を TIFF として開いてしまうため。
    /// TIFF は拡張子（.tif / .tiff）で選ばれる。
    fn probe(&self, _header: &[u8]) -> bool {
        false
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        self.decode_frame(data, 0)
    }

    fn frame_count(&self, data: &[u8]) -> Result<usize, DecodeError> {
        Ok(pages(data)?.len())
    }

    fn decode_frame(&self, data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
        let pages = pages(data)?;
        let ifd = *pages
            .get(frame)
            .ok_or_else(|| DecodeError::Invalid(format!("no page {frame}")))?;
        let mut decoder = open(data)?;
        decoder.seek_to_image(ifd).map_err(invalid)?;
        let metadata = metadata(&mut decoder, data, frame, pages.len());
        let orientation = decoder
            .find_tag_unsigned::<u32>(Tag::Orientation)
            .ok()
            .flatten()
            .unwrap_or(1);
        let photometric = decoder
            .find_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
            .map_err(invalid)?;
        check_size(&mut decoder)?;
        let mut img = if photometric == Some(PALETTE) {
            palette(data, ifd, &mut decoder)?
        } else {
            pixels(&mut decoder)?
        };
        img.metadata = metadata;
        img.gamut = icc_profile(&mut decoder).and_then(|icc| vw_core::icc::gamut(&icc));
        img.apply_orientation(orientation);
        Ok(img)
    }

    /// 画素を読まずに、1 ページ目のタグだけを読む。
    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        let pages = pages(data)?;
        let mut decoder = open(data)?;
        decoder.seek_to_image(pages[0]).map_err(invalid)?;
        Ok(metadata(&mut decoder, data, 0, pages.len()))
    }
}

/// PhotometricInterpretation の、パレット（カラーマップの番号）を表す値。
const PALETTE: u16 = 3;

fn open(data: &[u8]) -> Result<Decoder<Cursor<&[u8]>>, DecodeError> {
    // 既定の上限（256 MiB）は、スキャンした図面や顕微鏡の画像のような大きな TIFF で
    // 足りなくなる。画像の大きさの上限（設定で変えられる）をそのまま渡す。
    let mut limits = Limits::unlimited();
    limits.decoding_buffer_size = vw_core::limits::max_image_bytes();
    limits.intermediate_buffer_size = vw_core::limits::max_image_bytes();
    Decoder::new(Cursor::new(data))
        .map(|d| d.with_limits(limits))
        .map_err(invalid)
}

fn invalid(e: tiff::TiffError) -> DecodeError {
    match e {
        tiff::TiffError::UnsupportedError(e) => DecodeError::Unsupported(e.to_string()),
        e => DecodeError::Invalid(e.to_string()),
    }
}

/// ページとして見せる IFD の番号。縮小版（NewSubfileType の 1 ビット目）は飛ばす。
/// 縮小版はピラミッド形式の TIFF などで本体の後ろに並び、同じ絵が何枚も続いて見えるため。
fn pages(data: &[u8]) -> Result<Vec<usize>, DecodeError> {
    let mut decoder = open(data)?;
    let mut pages = Vec::new();
    for ifd in 0..MAX_PAGES {
        let subfile = decoder
            .find_tag_unsigned::<u32>(Tag::NewSubfileType)
            .ok()
            .flatten()
            .unwrap_or(0);
        if subfile & 1 == 0 {
            pages.push(ifd);
        }
        // 後ろのページが壊れていても、読めたページは見せる。
        if !decoder.more_images() || decoder.next_image().is_err() {
            break;
        }
    }
    if pages.is_empty() {
        return Err(DecodeError::Invalid("no full-size image".into()));
    }
    Ok(pages)
}

/// 画素を読む前に、幅・高さ・色の種類から要るメモリを確かめる。tiff クレートの上限でも止まるが、
/// こちらは幅と高さを添えて断れる。
fn check_size(decoder: &mut Decoder<Cursor<&[u8]>>) -> Result<(), DecodeError> {
    let (width, height) = decoder.dimensions().map_err(invalid)?;
    // パレットのページは tiff クレートが色の種類を返さない。その画素は後でグレーとして読み直し、
    // そのときクレートの上限で止まるので、ここでは確かめない。
    let Ok(color) = decoder.colortype() else {
        return Ok(());
    };
    let channels = match color {
        ColorType::Gray(_) => 1,
        ColorType::GrayA(_) => 2,
        ColorType::RGB(_) | ColorType::YCbCr(_) => 3,
        ColorType::RGBA(_) | ColorType::CMYK(_) => 4,
        ColorType::CMYKA(_) => 5,
        _ => 4,
    };
    let bytes = u64::from(color.bit_depth()).div_ceil(8).max(1);
    vw_core::limits::check_image(u64::from(width), u64::from(height), channels * bytes).map(drop)
}

/// パレット以外のページの画素。
fn pixels(decoder: &mut Decoder<Cursor<&[u8]>>) -> Result<VwImage, DecodeError> {
    let (width, height) = decoder.dimensions().map_err(invalid)?;
    let color = decoder.colortype().map_err(invalid)?;
    let result = decoder.read_image().map_err(invalid)?;
    let (model, channels) = match color {
        ColorType::Gray(_) => (ColorModel::Gray, 1),
        ColorType::GrayA(_) => (ColorModel::GrayAlpha, 2),
        ColorType::RGB(_) | ColorType::YCbCr(_) => (ColorModel::Rgb, 3),
        ColorType::RGBA(_) => (ColorModel::Rgba, 4),
        ColorType::CMYK(_) => (ColorModel::Rgb, 4),
        ColorType::CMYKA(_) => (ColorModel::Rgba, 5),
        other => {
            return Err(DecodeError::Unsupported(format!("TIFF color {other:?}")));
        }
    };
    let bits = color.bit_depth();
    let mut data = match result {
        DecodingResult::U8(v) if bits < 8 => {
            if channels != 1 {
                return Err(DecodeError::Unsupported(format!("TIFF color {color:?}")));
            }
            let max = (1u16 << bits) - 1;
            let samples = layout::unpack(&v, width as usize, height as usize, bits);
            PixelData::U8(
                samples
                    .into_iter()
                    .map(|s| (s as u16 * 255 / max) as u8)
                    .collect(),
            )
        }
        DecodingResult::U8(v) => PixelData::U8(v),
        DecodingResult::U16(v) => PixelData::U16(v),
        other => PixelData::F32(to_f32(other)),
    };
    match color {
        ColorType::CMYK(_) | ColorType::CMYKA(_) => data = layout::cmyk_to_rgb(data, channels)?,
        // tiff クレートは JPEG で圧縮したものも YCbCr のまま返す。
        ColorType::YCbCr(_) => data = layout::ycbcr_to_rgb(data)?,
        _ => {}
    }
    VwImage::new(width, height, model, data).map_err(DecodeError::Invalid)
}

/// 8/16bit 以外の数は実数にする。表示は最小値〜最大値で階調を合わせるので、
/// 32bit 整数や符号付きの画像（計測データなど）もそのまま見られる。
fn to_f32(result: DecodingResult) -> Vec<f32> {
    match result {
        DecodingResult::U8(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::U16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::U32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::F16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::F32(v) => v,
        DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I8(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::I16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::I32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I64(v) => v.into_iter().map(|x| x as f32).collect(),
    }
}

/// パレットのページを RGB にする。
///
/// tiff クレートはパレットのページを読めないので、ファイルの写しの PhotometricInterpretation を
/// グレーに書き換えて番号をグレーの値として読み、カラーマップで色に直す。展開や圧縮の処理を
/// 自前で持たずに済むため。
fn palette(
    data: &[u8],
    ifd: usize,
    decoder: &mut Decoder<Cursor<&[u8]>>,
) -> Result<VwImage, DecodeError> {
    let map = decoder.get_tag_u16_vec(Tag::ColorMap).map_err(invalid)?;
    let patched = layout::with_gray_photometric(data, ifd)
        .ok_or_else(|| DecodeError::Invalid("broken IFD".into()))?;
    let mut gray = open(&patched)?;
    gray.seek_to_image(ifd).map_err(invalid)?;
    let (width, height) = gray.dimensions().map_err(invalid)?;
    let bits = gray.colortype().map_err(invalid)?.bit_depth();
    let indices: Vec<usize> = match gray.read_image().map_err(invalid)? {
        DecodingResult::U8(v) if bits < 8 => {
            layout::unpack(&v, width as usize, height as usize, bits)
                .into_iter()
                .map(usize::from)
                .collect()
        }
        DecodingResult::U8(v) => v.into_iter().map(usize::from).collect(),
        DecodingResult::U16(v) => v.into_iter().map(usize::from).collect(),
        _ => return Err(DecodeError::Unsupported("TIFF palette depth".into())),
    };
    let rgb = layout::apply_color_map(&indices, &map)
        .ok_or_else(|| DecodeError::Invalid("broken color map".into()))?;
    VwImage::new(width, height, ColorModel::Rgb, PixelData::U8(rgb)).map_err(DecodeError::Invalid)
}

/// ページ `page`（全 `pages` ページ）のメタデータ。
fn metadata(
    decoder: &mut Decoder<Cursor<&[u8]>>,
    data: &[u8],
    page: usize,
    pages: usize,
) -> Metadata {
    let mut m = Metadata::default();
    let unsigned =
        |d: &mut Decoder<Cursor<&[u8]>>, tag| d.find_tag_unsigned::<u32>(tag).ok().flatten();
    if let Ok((w, h)) = decoder.dimensions() {
        m.push("TIFF", "Size", format!("{w} x {h}"));
    }
    if pages > 1 {
        m.push("TIFF", "Page", format!("{} / {pages}", page + 1));
    }
    if let Some(c) = unsigned(decoder, Tag::Compression) {
        m.push("TIFF", "Compression", compression(c));
    }
    if let Some(p) = unsigned(decoder, Tag::PhotometricInterpretation) {
        m.push("TIFF", "Photometric", photometric(p));
    }
    if let Ok(Some(bits)) = decoder.find_tag_unsigned_vec::<u16>(Tag::BitsPerSample) {
        let bits: Vec<String> = bits.iter().map(u16::to_string).collect();
        m.push("TIFF", "Bits per sample", bits.join(", "));
    }
    if let Ok(Some(formats)) = decoder.find_tag_unsigned_vec::<u16>(Tag::SampleFormat)
        && let Some(&f) = formats.first()
    {
        m.push("TIFF", "Sample format", sample_format(f));
    }
    if let (Some(w), Some(h)) = (
        unsigned(decoder, Tag::TileWidth),
        unsigned(decoder, Tag::TileLength),
    ) {
        m.push("TIFF", "Tiles", format!("{w} x {h}"));
    }
    // 1 ページ目の IFD を EXIF として読む。2 ページ目以降のタグは読めないが、カメラや作成ソフトの
    // 情報はふつう 1 ページ目にしかないため、全ページに同じものを出す。
    vw_core::exif::push_exif_except(&mut m, data.to_vec(), HIDDEN_TAGS);
    if let Some(xmp) = bytes_tag(decoder, Tag::Unknown(XMP)) {
        vw_core::xmp::push_xmp(&mut m, &xmp);
    }
    if let Some(icc) = icc_profile(decoder) {
        m.push("TIFF", "ICC profile", "present");
        vw_core::icc::push_profile(&mut m, &icc);
    }
    m
}

const XMP: u16 = 700;

fn icc_profile(decoder: &mut Decoder<Cursor<&[u8]>>) -> Option<Vec<u8>> {
    bytes_tag(decoder, Tag::IccProfile)
}

fn bytes_tag(decoder: &mut Decoder<Cursor<&[u8]>>, tag: Tag) -> Option<Vec<u8>> {
    decoder
        .find_tag(tag)
        .ok()
        .flatten()
        .and_then(|v| v.into_u8_vec().ok())
}

fn compression(c: u32) -> String {
    match c {
        1 => "None".into(),
        2 => "CCITT RLE".into(),
        3 => "CCITT Group 3".into(),
        4 => "CCITT Group 4".into(),
        5 => "LZW".into(),
        6 | 7 => "JPEG".into(),
        8 | 32946 => "Deflate".into(),
        32773 => "PackBits".into(),
        34925 => "LZMA".into(),
        50000 => "Zstandard".into(),
        50001 => "WebP".into(),
        other => other.to_string(),
    }
}

fn photometric(p: u32) -> String {
    match p {
        0 => "White is zero".into(),
        1 => "Black is zero".into(),
        2 => "RGB".into(),
        3 => "Palette".into(),
        4 => "Transparency mask".into(),
        5 => "CMYK".into(),
        6 => "YCbCr".into(),
        8 => "CIE L*a*b*".into(),
        other => other.to_string(),
    }
}

fn sample_format(f: u16) -> String {
    match f {
        1 => "Unsigned integer".into(),
        2 => "Signed integer".into(),
        3 => "Floating point".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
