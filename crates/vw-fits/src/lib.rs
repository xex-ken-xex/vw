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
//! - 場所の分かるライトには、近くやライブラリのダーク・フラット・バイアスを当てる（[`calibration`]）

mod bayer;
pub mod calibration;
mod header;
pub mod radio;
mod sky;
pub mod stretch;
mod tile;

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

pub use vw_core::Curve;
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

pub use calibration::Calibration;
pub use header::is_fits;
use header::Hdu;
pub use sky::{header_of_file, pixel_map, sky, ImageHeader, Sky};
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
    /// キューブの面（チャンネル）全部に、同じ黒と白を使う。面ごとに決めると、隣の面と明るさを
    /// 比べられず、輝線のない面では雑音が強調されるため。
    pub same_window_for_planes: bool,
    /// 欠損（`BLANK`・NaN）を描く色（[`vw_core::VwImage::missing`]）。`None` は黒。
    pub missing: Option<[u8; 3]>,
}

/// 欠損の色の既定。色の表のどの色とも見分けやすい、少し青みのある灰色。
pub const MISSING_COLOR: [u8; 3] = [112, 120, 136];

impl Default for Options {
    fn default() -> Self {
        Self {
            stretch: Stretch::Percentile,
            low_percent: 0.5,
            high_percent: 99.5,
            curve: Curve::Linear,
            flip_rows: false,
            demosaic: true,
            same_window_for_planes: true,
            missing: Some(MISSING_COLOR),
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
    calibration: Calibration,
    cube_windows: CubeWindows,
}

impl FitsPlugin {
    pub fn new(options: SharedOptions, calibration: Calibration) -> Self {
        Self {
            options,
            calibration,
            cube_windows: CubeWindows::default(),
        }
    }
}

/// キューブ全体から決めた黒と白。面を送るたびに全体を読み直すと重いので、ファイルごとに覚える。
/// 鍵はヘッダの中身とファイルの長さと決め方（場所はデコーダに届かないことがあるため）。
#[derive(Default, Clone)]
struct CubeWindows(Arc<Mutex<HashMap<u64, (f32, f32)>>>);

impl CubeWindows {
    /// 覚えておく数。超えたら全部忘れる（キューブを次々に開いても増え続けないように）。
    const KEEP: usize = 64;

    fn get_or(&self, key: u64, make: impl FnOnce() -> Option<(f32, f32)>) -> Option<(f32, f32)> {
        if let Some(w) = self.0.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Some(*w);
        }
        let window = make()?;
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= Self::KEEP {
            map.clear();
        }
        map.insert(key, window);
        Some(window)
    }
}

/// メタデータの見出し。
pub const GROUP: &str = "FITS";
/// 行の並びを返して表示したとき、メタデータに [`ROWS_FLIPPED`] を書く鍵。
/// 画面の行と FITS の y 座標の向きが逆になるので、WCS で位置を合わせるときに使う。
pub const ROWS: &str = "Rows";
pub const ROWS_FLIPPED: &str = "Turned upside down";
/// 表示の黒と白の決め方（[`Stretch::title`]）をメタデータに書く鍵。
pub const STRETCH: &str = "Black and white";
/// [`Stretch::Percentile`] のときに、黒と白にした割合を書く鍵。
pub const BLACK_PERCENTILE: &str = "Black percentile";
pub const WHITE_PERCENTILE: &str = "White percentile";
/// キューブの面が軸のどこか（周波数・速度・偏光）を書く鍵（[`radio`]）。
pub const PLANE: &str = "Plane";
/// キューブの全部の面に同じ黒と白を使ったことを書く鍵と値。
pub const PLANES_WINDOW: &str = "Black and white of planes";
pub const PLANES_WINDOW_SAME: &str = "Same for all planes";

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
        decode_with(
            &gunzip(data)?,
            frame,
            self.options.get(),
            None,
            Some(&self.cube_windows),
        )
    }

    fn decode_frame_at(
        &self,
        path: &Path,
        data: &[u8],
        frame: usize,
    ) -> Result<VwImage, DecodeError> {
        decode_with(
            &gunzip(data)?,
            frame,
            self.options.get(),
            Some((&self.calibration, path)),
            Some(&self.cube_windows),
        )
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
    // 展開した大きさは展開してみるまで分からないので、画像の上限まで読んで、それでも続くなら断る。
    let limit = vw_core::limits::max_image_bytes() as u64;
    let mut out = Vec::new();
    flate2::read::MultiGzDecoder::new(data)
        .take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|e| DecodeError::Invalid(format!("gzip: {e}")))?;
    vw_core::limits::check_bytes(out.len() as u64, "gzip-compressed FITS file")?;
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
    /// 3 次元目以降の軸の長さ。
    extra_axes: Vec<usize>,
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
        let planes = axes[2..]
            .iter()
            .try_fold(1usize, |n, &a| n.checked_mul(a))
            .ok_or_else(|| {
                DecodeError::Unsupported(format!(
                    "FITS image {} x {} is too large",
                    axes[0], axes[1]
                ))
            })?;
        // tile 圧縮の整数は i64 に展開するので、1 画素の最大は 8 バイト。
        let bytes_per_pixel = if compressed {
            8
        } else {
            (bitpix.unsigned_abs() / 8).max(1)
        };
        vw_core::limits::check_image(axes[0] as u64, axes[1] as u64, bytes_per_pixel)?;
        return Ok(Image {
            index,
            hdu,
            compressed,
            bitpix,
            width: axes[0],
            height: axes[1],
            planes,
            extra_axes: axes[2..].to_vec(),
        });
    }
    Err(DecodeError::Unsupported("FITS file has no image".into()))
}

fn decode_with(
    data: &[u8],
    frame: usize,
    options: Options,
    calibration: Option<(&Calibration, &Path)>,
    cube_windows: Option<&CubeWindows>,
) -> Result<VwImage, DecodeError> {
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
    let pattern = bayer_pattern(image.hdu);
    let mut calibrated = Metadata::default();
    // キューブの面ごとに合うマスターはないので、1 面の画像だけに当てる。
    if let Some((calibration, path)) = calibration.filter(|_| image.planes == 1) {
        let get = |key: &str| {
            image
                .hdu
                .get(key)
                .or_else(|| hdus[0].get(key))
                .map(str::to_owned)
        };
        pixels = calibration.calibrate(path, &get, pattern.is_some(), pixels, &mut calibrated);
    }
    let mut color = ColorModel::Gray;
    if options.demosaic && image.planes == 1 {
        if let Some(pattern) = pattern {
            pixels = demosaic(pixels, w, h, pattern);
            color = ColorModel::Rgb;
        }
    }
    let mut img = VwImage::new(w as u32, h as u32, color, pixels).map_err(DecodeError::Invalid)?;
    let flipped = bottom_up(image.hdu, options.flip_rows);
    if flipped {
        // 左右はそのまま、上下だけ返す（EXIF の向きの 4）。
        img.apply_orientation(4);
    }
    let cube = image.planes > 1 && options.same_window_for_planes;
    img.window = match cube_windows.filter(|_| cube) {
        Some(cache) => cache.get_or(cube_key(data, &image, &options), || {
            cube_window(data, &image, &options)
        }),
        None => None,
    }
    .or_else(|| {
        stretch::window(
            &img.data,
            options.stretch,
            options.low_percent,
            options.high_percent,
        )
    });
    img.curve = options.curve;
    img.missing = options.missing;
    img.metadata = metadata(&hdus, &image, frame);
    if flipped {
        img.metadata.push(GROUP, ROWS, ROWS_FLIPPED);
    }
    if cube {
        img.metadata.push(GROUP, PLANES_WINDOW, PLANES_WINDOW_SAME);
    }
    img.metadata.push(GROUP, STRETCH, options.stretch.title());
    if options.stretch == Stretch::Percentile {
        for (key, percent) in [
            (BLACK_PERCENTILE, options.low_percent),
            (WHITE_PERCENTILE, options.high_percent),
        ] {
            img.metadata.push(GROUP, key, format!("{percent} %"));
        }
    }
    img.metadata.entries.extend(calibrated.entries);
    Ok(img)
}

/// 1 面目の値を、`BSCALE`・`BZERO` をかけたモザイクのまま実数で返す（校正のマスター用）。
pub(crate) fn mosaic_values(data: &[u8]) -> Result<Vec<f32>, DecodeError> {
    let data = gunzip(data)?;
    let hdus = header::hdus(&data)?;
    let image = find_image(&hdus)?;
    let stored = if image.compressed {
        compressed_plane(&data, &image, 0)?
    } else {
        plain_plane(&data, &image, 0)?
    };
    Ok(match to_pixels(stored, image.hdu) {
        PixelData::U8(v) => v.into_iter().map(f32::from).collect(),
        PixelData::U16(v) => v.into_iter().map(f32::from).collect(),
        PixelData::F32(v) => v,
    })
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

/// 黒と白を決めるのに見る面の数の上限。全部の面を読むと大きなキューブで重いので、等間隔に選ぶ。
const WINDOW_PLANES: usize = 16;

/// キューブの面を [`WINDOW_PLANES`] 枚まで等間隔に読み、まとめた値から黒と白を決める。
fn cube_window(data: &[u8], image: &Image, options: &Options) -> Option<(f32, f32)> {
    let count = image.planes.min(WINDOW_PLANES);
    // 1 面あたりの値の数。合わせて 100 万程度にする（stretch が間引く数と同じくらい）。
    let per_plane = (1_000_000 / count).max(1);
    let mut values = Vec::new();
    for i in 0..count {
        let plane = i * image.planes / count;
        let stored = if image.compressed {
            compressed_plane(data, image, plane)
        } else {
            plain_plane(data, image, plane)
        }
        .ok()?;
        let pixels = to_pixels(stored, image.hdu);
        let step = pixels.len().div_ceil(per_plane).max(1);
        match &pixels {
            PixelData::U8(v) => values.extend(v.iter().step_by(step).map(|&x| f32::from(x))),
            PixelData::U16(v) => values.extend(v.iter().step_by(step).map(|&x| f32::from(x))),
            PixelData::F32(v) => values.extend(v.iter().step_by(step).copied()),
        }
    }
    stretch::window(
        &PixelData::F32(values),
        options.stretch,
        options.low_percent,
        options.high_percent,
    )
}

/// [`CubeWindows`] の鍵。
fn cube_key(data: &[u8], image: &Image, options: &Options) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.len().hash(&mut h);
    for card in &image.hdu.cards {
        card.key.hash(&mut h);
        card.value.hash(&mut h);
    }
    format!("{:?}", options.stretch).hash(&mut h);
    options.low_percent.to_bits().hash(&mut h);
    options.high_percent.to_bits().hash(&mut h);
    h.finish()
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
        if let Some(text) = radio::plane_text(image.hdu, &image.extra_axes, frame) {
            m.push(GROUP, PLANE, text);
        }
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
