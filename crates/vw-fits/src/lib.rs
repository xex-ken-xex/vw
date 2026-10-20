//! FITS（天体の画像）のデコーダプラグイン。
//!
//! - 最初の画像だけを見せる。Primary HDU にデータがなければ（`.fz` や JWST など）、データのある
//!   最初の画像の拡張を使う。その画像の 3 次元目以降（キューブの面、`NAXIS3=3` の 3 面）は、
//!   1 枚ずつのフレームにする（[`DecoderPlugin::frame_count`]）
//! - `BSCALE`・`BZERO` をかけた値にする。符号なし 16bit（`BZERO=32768`）は 16bit のまま持つ
//! - `BLANK` の画素と NaN は欠損として NaN にする（表示は黒）
//! - `ROWORDER` があれば行の並びをそれに従わせ、なければ設定で上下を返す
//! - `BAYERPAT` のモザイクは、設定に従って RGB にする
//! - 最初の黒と白を画素の分布から決める（[`stretch`]）
//! - tile 圧縮（RICE、GZIP）の `.fz` と、ファイル全体を gzip した `.fits.gz` も読む

mod bayer;
mod header;
mod sky;
pub mod stretch;
mod tile;

use std::borrow::Cow;
use std::io::Read;
use std::sync::{Arc, RwLock};

pub use vw_core::Curve;
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

pub use header::is_fits;
use header::Hdu;
pub use sky::{header_of_file, sky, ImageHeader, Sky};
pub use stretch::Stretch;

/// 表示の決め方。設定画面で変える。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub stretch: Stretch,
    /// [`Stretch::Percentile`] の黒と白の位置（％）。
    pub low_percent: f32,
    pub high_percent: f32,
    /// 黒と白の間の明るさの付け方。淡い星雲を見るときは直線以外にする。
    pub curve: Curve,
    /// `ROWORDER` のないファイルの上下を返す（規格どおり、最初の行を下にする）。
    pub flip_rows: bool,
    /// `BAYERPAT` のモザイクを RGB にする。
    pub demosaic: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            stretch: Stretch::Percentile,
            low_percent: 0.5,
            high_percent: 99.5,
            curve: Curve::Linear,
            flip_rows: false,
            demosaic: true,
        }
    }
}

/// プラグインとアプリで共有する [`Options`]。読み込みスレッドごとに複製した `Registry` でも、
/// 設定を変えたらすぐ同じ値を使うため。
#[derive(Debug, Clone, Default)]
pub struct SharedOptions(Arc<RwLock<Options>>);

impl SharedOptions {
    pub fn get(&self) -> Options {
        *self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set(&self, options: Options) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = options;
    }
}

#[derive(Default)]
pub struct FitsPlugin {
    options: SharedOptions,
}

impl FitsPlugin {
    pub fn new(options: SharedOptions) -> Self {
        Self { options }
    }
}

/// メタデータの見出し。
pub const GROUP: &str = "FITS";

/// 1 枚の画素の上限。巨大な `NAXIS` を書いた壊れたファイルで、メモリを取りに行かないように。
/// 2^31 画素（16bit なら 4 GiB）を超える画像は、天体の観測データでもまずない。
const MAX_PIXELS: usize = 1 << 31;
/// `.fits.gz` を展開する大きさの上限。
const MAX_GUNZIP_BYTES: u64 = 16 << 30;

impl DecoderPlugin for FitsPlugin {
    fn name(&self) -> &str {
        "FITS"
    }

    fn extensions(&self) -> &[&str] {
        &["fits", "fit", "fts", "fz", "fits.gz", "fit.gz", "fts.gz"]
    }

    fn probe(&self, header: &[u8]) -> bool {
        header::is_fits(header)
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        self.decode_frame(data, 0)
    }

    fn frame_count(&self, data: &[u8]) -> Result<usize, DecodeError> {
        let data = gunzip(data)?;
        let hdus = header::hdus(&data)?;
        Ok(find_image(&hdus)?.planes)
    }

    fn decode_frame(&self, data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
        decode_with(&gunzip(data)?, frame, self.options.get())
    }

    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        let data = gunzip(data)?;
        let hdus = header::hdus(&data)?;
        let image = find_image(&hdus)?;
        Ok(metadata(&hdus, &image, 0))
    }
}

/// gzip で固めたファイルを展開する。そうでなければそのまま。
fn gunzip(data: &[u8]) -> Result<Cow<'_, [u8]>, DecodeError> {
    if !data.starts_with(&[0x1f, 0x8b]) {
        return Ok(Cow::Borrowed(data));
    }
    let mut out = Vec::new();
    flate2::read::MultiGzDecoder::new(data)
        .take(MAX_GUNZIP_BYTES)
        .read_to_end(&mut out)
        .map_err(|e| DecodeError::Invalid(format!("gzip: {e}")))?;
    Ok(Cow::Owned(out))
}

/// 表示する画像の HDU と、その形。
struct Image<'a> {
    index: usize,
    hdu: &'a Hdu,
    /// tile 圧縮（BINTABLE に入った画像）か。
    compressed: bool,
    bitpix: i64,
    width: usize,
    height: usize,
    planes: usize,
}

impl Image<'_> {
    fn plane_pixels(&self) -> usize {
        self.width * self.height
    }
}

/// データのある最初の画像。
fn find_image(hdus: &[Hdu]) -> Result<Image<'_>, DecodeError> {
    for (index, hdu) in hdus.iter().enumerate() {
        let (compressed, prefix) = match hdu.xtension() {
            None | Some("IMAGE") => (false, ""),
            Some("BINTABLE") if hdu.logical("ZIMAGE") => (true, "Z"),
            _ => continue,
        };
        let Some(axes) = hdu.axes(prefix) else {
            continue;
        };
        if axes.len() < 2 || axes.contains(&0) {
            continue;
        }
        let Some(bitpix) = hdu.int(&format!("{prefix}BITPIX")) else {
            continue;
        };
        let too_large = || {
            DecodeError::Unsupported(format!("FITS image {} x {} is too large", axes[0], axes[1]))
        };
        let planes = axes[2..]
            .iter()
            .try_fold(1usize, |n, &a| n.checked_mul(a))
            .ok_or_else(too_large)?;
        if axes[0].checked_mul(axes[1]).is_none_or(|n| n > MAX_PIXELS) {
            return Err(too_large());
        }
        return Ok(Image {
            index,
            hdu,
            compressed,
            bitpix,
            width: axes[0],
            height: axes[1],
            planes,
        });
    }
    Err(DecodeError::Unsupported("FITS file has no image".into()))
}

fn decode_with(data: &[u8], frame: usize, options: Options) -> Result<VwImage, DecodeError> {
    let hdus = header::hdus(data)?;
    let image = find_image(&hdus)?;
    if frame >= image.planes {
        return Err(DecodeError::Invalid(format!("no frame {frame}")));
    }
    let stored = if image.compressed {
        compressed_plane(data, &image, frame)?
    } else {
        plain_plane(data, &image, frame)?
    };
    let (w, h) = (image.width, image.height);
    let mut pixels = to_pixels(stored, image.hdu);
    let mut color = ColorModel::Gray;
    if options.demosaic && image.planes == 1 {
        if let Some(pattern) = bayer_pattern(image.hdu) {
            pixels = demosaic(pixels, w, h, pattern);
            color = ColorModel::Rgb;
        }
    }
    let mut img = VwImage::new(w as u32, h as u32, color, pixels).map_err(DecodeError::Invalid)?;
    if bottom_up(image.hdu, options.flip_rows) {
        // 左右はそのまま、上下だけ返す（EXIF の向きの 4）。
        img.apply_orientation(4);
    }
    img.window = stretch::window(
        &img.data,
        options.stretch,
        options.low_percent,
        options.high_percent,
    );
    img.curve = options.curve;
    img.metadata = metadata(&hdus, &image, frame);
    Ok(img)
}

/// 最初の行が下か。`ROWORDER` が書いてあればそれに従い、なければ設定に従う。
fn bottom_up(hdu: &Hdu, flip_rows: bool) -> bool {
    match hdu.get("ROWORDER").map(|v| v.trim().to_ascii_uppercase()) {
        Some(v) if v == "TOP-DOWN" => false,
        Some(v) if v == "BOTTOM-UP" => true,
        _ => flip_rows,
    }
}

/// `BAYERPAT` と、それをずらす `XBAYROFF`・`YBAYROFF`。並びはファイルの最初の画素から数える。
/// 上下を返す前に色を付けるので、返したときの並びの読み替え（RGGB → GBRG）は要らない。
fn bayer_pattern(hdu: &Hdu) -> Option<bayer::Pattern> {
    let name = hdu.get("BAYERPAT")?;
    bayer::Pattern::new(
        name,
        hdu.int("XBAYROFF").unwrap_or(0),
        hdu.int("YBAYROFF").unwrap_or(0),
    )
}

fn demosaic(pixels: PixelData, w: usize, h: usize, pattern: bayer::Pattern) -> PixelData {
    let values: Vec<f32> = match &pixels {
        PixelData::U8(v) => v.iter().map(|&s| s as f32).collect(),
        PixelData::U16(v) => v.iter().map(|&s| s as f32).collect(),
        PixelData::F32(v) => v.clone(),
    };
    let rgb = bayer::demosaic(&values, w, h, pattern);
    match pixels {
        PixelData::U8(_) => PixelData::U8(rgb.iter().map(|&s| s.round() as u8).collect()),
        PixelData::U16(_) => PixelData::U16(rgb.iter().map(|&s| s.round() as u16).collect()),
        PixelData::F32(_) => PixelData::F32(rgb),
    }
}

/// ファイルに格納された値（`BSCALE`・`BZERO` をかける前）。
enum Stored {
    U8(Vec<u8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    I64(Vec<i64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

/// 圧縮されていない画像の `plane` 面目。ビッグエンディアンをまとめて変換する。
fn plain_plane(data: &[u8], image: &Image, plane: usize) -> Result<Stored, DecodeError> {
    let truncated = || DecodeError::Invalid("FITS file is truncated".into());
    let bytes = (image.bitpix.unsigned_abs() / 8) as usize;
    let len = image.plane_pixels() * bytes;
    let start = plane
        .checked_mul(len)
        .and_then(|o| o.checked_add(image.hdu.data_start))
        .ok_or_else(truncated)?;
    let raw = data
        .get(start..start.checked_add(len).ok_or_else(truncated)?)
        .ok_or_else(truncated)?;
    fn be<const N: usize, T>(raw: &[u8], f: fn([u8; N]) -> T) -> Vec<T> {
        raw.as_chunks::<N>().0.iter().map(|c| f(*c)).collect()
    }
    Ok(match image.bitpix {
        8 => Stored::U8(raw.to_vec()),
        16 => Stored::I16(be(raw, i16::from_be_bytes)),
        32 => Stored::I32(be(raw, i32::from_be_bytes)),
        64 => Stored::I64(be(raw, i64::from_be_bytes)),
        -32 => Stored::F32(be(raw, f32::from_be_bytes)),
        -64 => Stored::F64(be(raw, f64::from_be_bytes)),
        other => return Err(DecodeError::Invalid(format!("FITS BITPIX {other}"))),
    })
}

/// tile 圧縮の画像の `plane` 面目。
fn compressed_plane(data: &[u8], image: &Image, plane: usize) -> Result<Stored, DecodeError> {
    if ![8, 16, 32, -32, -64].contains(&image.bitpix) {
        return Err(DecodeError::Unsupported(format!(
            "FITS compressed BITPIX {}",
            image.bitpix
        )));
    }
    let axes = image.hdu.axes("Z").unwrap_or_default();
    Ok(
        match tile::plane(data, image.hdu, &axes, image.bitpix, plane)? {
            tile::Plane::Floats(v) => Stored::F32(v),
            tile::Plane::Integers(v) => match image.bitpix {
                8 => Stored::U8(v.into_iter().map(|x| x as u8).collect()),
                16 => Stored::I16(v.into_iter().map(|x| x as i16).collect()),
                _ => Stored::I32(v.into_iter().map(|x| x as i32).collect()),
            },
        },
    )
}

/// 格納値に `BSCALE`・`BZERO` をかけ、`BLANK` を欠損（NaN）にする。
///
/// 符号付き 16bit（`BZERO` なし）は、32768 足して U16 で持つと表示の階調や画素の値、ROI の
/// 統計がすべて足した分ずれてしまい、それぞれで引き直すことになるので、実数にする。
fn to_pixels(stored: Stored, hdu: &Hdu) -> PixelData {
    let scale = hdu.float("BSCALE").unwrap_or(1.0);
    let zero = hdu.float("BZERO").unwrap_or(0.0);
    let blank = hdu.int("BLANK");
    let plain = scale == 1.0 && blank.is_none();
    let real = |v: Vec<f64>| -> PixelData {
        PixelData::F32(v.into_iter().map(|x| (zero + scale * x) as f32).collect())
    };
    let integers = |v: Vec<i64>| -> PixelData {
        PixelData::F32(
            v.into_iter()
                .map(|x| {
                    if Some(x) == blank {
                        f32::NAN
                    } else {
                        (zero + scale * x as f64) as f32
                    }
                })
                .collect(),
        )
    };
    match stored {
        Stored::U8(v) if plain && zero == 0.0 => PixelData::U8(v),
        Stored::I16(v) if plain && zero == 32768.0 => {
            PixelData::U16(v.into_iter().map(|x| (x as u16) ^ 0x8000).collect())
        }
        Stored::U8(v) => integers(v.into_iter().map(i64::from).collect()),
        Stored::I16(v) => integers(v.into_iter().map(i64::from).collect()),
        Stored::I32(v) => integers(v.into_iter().map(i64::from).collect()),
        Stored::I64(v) => integers(v),
        // 実数の画像の欠損は NaN で書かれている（BLANK は整数の画像だけのもの）。
        Stored::F32(v) if scale == 1.0 && zero == 0.0 => PixelData::F32(v),
        Stored::F32(v) => real(v.into_iter().map(f64::from).collect()),
        Stored::F64(v) => real(v),
    }
}

/// 全カードを出現順に出す。Primary と表示する画像が別の HDU なら、画像の HDU を
/// 「FITS HDU n」の見出しで後ろに足す（n は Primary を 0 とした番号）。
fn metadata(hdus: &[Hdu], image: &Image, frame: usize) -> Metadata {
    let mut m = Metadata::default();
    m.push(GROUP, "Size", format!("{} x {}", image.width, image.height));
    if image.planes > 1 {
        m.push(GROUP, "Frame", format!("{} / {}", frame + 1, image.planes));
    }
    push_cards(&mut m, GROUP, &hdus[0]);
    if image.index > 0 {
        push_cards(&mut m, &format!("{GROUP} HDU {}", image.index), image.hdu);
    }
    m
}

fn push_cards(m: &mut Metadata, group: &str, hdu: &Hdu) {
    for card in &hdu.cards {
        if card.key.is_empty() && card.value.trim().is_empty() {
            continue;
        }
        m.push(group, card.key.as_str(), card.value.as_str());
    }
}

#[cfg(test)]
mod tests;
