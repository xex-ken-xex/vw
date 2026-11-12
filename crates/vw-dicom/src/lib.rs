//! DICOM デコーダプラグイン。
//!
//! - 非圧縮・RLE・JPEG（ベースライン / ロスレス）・JPEG-LS・JPEG 2000 を読む。JPEG-LS は
//!   CharLS（C++）、JPEG 2000 は OpenJPEG（C）を静的にリンクする。純 Rust の候補は、
//!   pure_jpegls がカラーを読めず、openjp2 がデコードのたびに未定義動作（null の Box の解放）
//!   を起こすので採らない
//! - モノクロは Rescale Slope / Intercept を当てた値（CT なら HU）で持ち、ファイルの
//!   ウィンドウ幅 / レベルを表示の階調として渡す。MONOCHROME1 は値を反転して持つ
//! - カラーは RGB にして持つ。PALETTE COLOR・非圧縮の YBR_FULL_422・16bit のカラーは
//!   dicom-pixeldata が RGB にしないので [`color`] で直す
//! - 1 ファイルに複数フレームが入ったもの（シネなど）はフレームごとに読む
//! - 患者情報は「Patient」のメタデータにだけ出す

use std::io::Cursor;

use dicom_core::Tag;
use dicom_dictionary_std::tags;
use dicom_object::file::ReadPreamble;
use dicom_object::{DefaultDicomObject, OpenFileOptions};
use dicom_pixeldata::{
    DecodedPixelData, PhotometricInterpretation, PixelDecoder, PixelRepresentation,
    PlanarConfiguration, VoiLutFunction,
};
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

use charset::Charset;

pub mod charset;
mod check;
pub mod color;
pub mod frame_reader;
mod lenient;
pub mod net;
pub mod pipeline;
pub mod structure;
pub mod suv;
pub mod synth;

/// 128 バイトのプリアンブルの後ろに置かれる印。
const MAGIC: &[u8] = b"DICM";
const MAGIC_AT: usize = 128;

pub struct DicomPlugin;

impl DecoderPlugin for DicomPlugin {
    fn name(&self) -> &str {
        "DICOM"
    }

    fn extensions(&self) -> &[&str] {
        &["dcm", "dicom", "dic"]
    }

    fn probe(&self, header: &[u8]) -> bool {
        header.get(MAGIC_AT..MAGIC_AT + MAGIC.len()) == Some(MAGIC)
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        self.decode_frame(data, 0)
    }

    fn frame_count(&self, data: &[u8]) -> Result<usize, DecodeError> {
        let obj = open(data, true)?;
        Ok(frames_in(&obj))
    }

    fn decode_frame(&self, data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
        let obj = open(data, false)?;
        let frames = frames_in(&obj);
        if frame >= frames {
            return Err(DecodeError::Invalid(format!(
                "no frame {frame} (the file has {frames})"
            )));
        }
        check_pixel_size(&obj)?;
        let colors = Colors::of(&obj)?;
        let mut img = decode_one(&obj, &colors, frame)?;
        img.metadata = metadata(&obj);
        Ok(img)
    }

    /// 解析は 1 回だけにして、フレームを順に取り出す。
    fn decode_frames(
        &self,
        _path: &std::path::Path,
        data: &[u8],
        order: &[usize],
        each: &mut dyn FnMut(usize, Result<VwImage, DecodeError>) -> bool,
    ) {
        let opened = open(data, false).and_then(|obj| {
            check_pixel_size(&obj)?;
            let colors = Colors::of(&obj)?;
            Ok((obj, colors))
        });
        let (obj, colors) = match opened {
            Ok(obj) => obj,
            Err(e) => {
                if let Some(&first) = order.first() {
                    each(first, Err(e));
                }
                return;
            }
        };
        let frames = frames_in(&obj);
        let meta = metadata(&obj);
        for &frame in order {
            let image = if frame >= frames {
                Err(DecodeError::Invalid(format!(
                    "no frame {frame} (the file has {frames})"
                )))
            } else {
                decode_one(&obj, &colors, frame).map(|mut img| {
                    img.metadata = meta.clone();
                    img
                })
            };
            if !each(frame, image) {
                return;
            }
        }
    }

    fn open_frames(
        &self,
        path: &std::path::Path,
    ) -> Option<std::sync::Arc<dyn vw_core::FrameSource>> {
        let src = frame_reader::FileAt::open(path).ok()?;
        let frames = frame_reader::FileFrames::open(Box::new(src)).ok()??;
        Some(std::sync::Arc::new(frames))
    }

    /// 画素データの手前で読むのをやめるので、大きなファイルでも速い。
    ///
    /// メタ情報が規格に外れて開けないファイルも、タグは [`lenient`] で読む。画像は読めないままにする。
    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        Ok(match open_for_tags(data)? {
            ForTags::Strict(obj) => metadata(&obj),
            ForTags::Lenient(l) => {
                let mut m = metadata_with(&l.obj, &l.meta_rows());
                l.push_problems(&mut m);
                m
            }
        })
    }

    /// 画素データなどの大きなバイナリを除いた全タグ。グループは `DICOM`、キーは `0010,0020` の形。
    fn read_all_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        let opened = open_for_tags(data)?;
        let obj = opened.obj();
        let mut m = Metadata::default();
        let meta = obj.meta();
        for (tag, value) in [
            (
                tags::MEDIA_STORAGE_SOP_CLASS_UID,
                &meta.media_storage_sop_class_uid,
            ),
            (tags::TRANSFER_SYNTAX_UID, &meta.transfer_syntax),
        ] {
            push_tag(&mut m, tag, value);
        }
        let charset = charset_of(obj, Charset::default());
        all_tags(obj, &charset, &mut m);
        check::push_problems(obj, &charset, 0, &mut m);
        if let ForTags::Lenient(l) = &opened {
            l.push_problems(&mut m);
        }
        Ok(m)
    }
}

/// 検索カタログのキー（`0010,0020`）からタグの名前（`PatientID`）を引く。画面でキーの候補に添える。
pub fn tag_name(key: &str) -> Option<&'static str> {
    use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry};
    let (group, element) = key.split_once(',')?;
    let tag = Tag(
        u16::from_str_radix(group, 16).ok()?,
        u16::from_str_radix(element, 16).ok()?,
    );
    dicom_dictionary_std::StandardDataDictionary
        .by_tag(tag)
        .map(|e| e.alias())
}

/// シーケンスの中も同じように辿る。入れ子の位置は持たない（検索では値が見つかれば足りるため）。
fn all_tags(obj: &dicom_object::InMemDicomObject, charset: &Charset, m: &mut Metadata) {
    use dicom_core::VR;
    use dicom_core::value::Value;
    for e in obj.iter() {
        match e.value() {
            Value::Sequence(seq) => {
                for item in seq.items() {
                    all_tags(item, &charset_of(item, *charset), m);
                }
            }
            Value::PixelSequence(_) => {}
            Value::Primitive(p) => {
                let binary = matches!(
                    e.vr(),
                    VR::OB | VR::OD | VR::OF | VR::OL | VR::OV | VR::OW | VR::UN
                );
                if !binary {
                    push_tag(m, e.header().tag, &charset::value_text(p, charset));
                }
            }
        }
    }
}

fn push_tag(m: &mut Metadata, tag: Tag, value: &str) {
    let value = value.trim_matches([' ', '\0']);
    if !value.is_empty() {
        m.push(
            "DICOM",
            format!("{:04X},{:04X}", tag.group(), tag.element()),
            value,
        );
    }
}

/// DICOM ファイルを開く。`header_only` なら画素データの手前で読むのをやめる。
fn open(data: &[u8], header_only: bool) -> Result<DefaultDicomObject, DecodeError> {
    structure::check(data, data.len() as u64, header_only)?;
    let mut options = OpenFileOptions::new().read_preamble(ReadPreamble::Always);
    if header_only {
        options = options.read_until(tags::PIXEL_DATA);
    }
    options
        .from_reader(Cursor::new(data))
        .map_err(|e| DecodeError::Invalid(error_chain(&e)))
}

/// タグを読むために開いたファイル。
enum ForTags {
    Strict(DefaultDicomObject),
    Lenient(lenient::Opened),
}

impl ForTags {
    fn obj(&self) -> &DefaultDicomObject {
        match self {
            ForTags::Strict(obj) => obj,
            ForTags::Lenient(l) => &l.obj,
        }
    }
}

/// 画素データの手前まで開く。dicom-rs が開けなければ [`lenient`] で開き直す。どちらでも
/// 開けなければ、dicom-rs のエラーを返す（何が外れているかを一番よく言うのはそちらのため）。
fn open_for_tags(data: &[u8]) -> Result<ForTags, DecodeError> {
    match open(data, true) {
        Ok(obj) => Ok(ForTags::Strict(obj)),
        Err(strict) => lenient::open(data, true)
            .map(ForTags::Lenient)
            .map_err(|_| strict),
    }
}

/// エラーと、その原因をたどった全部を「: 」でつなぐ。dicom-rs の一番外のエラーは
/// 「Could not read data set token」のように、どのタグで何が起きたかを持たず、それは原因の方にあるため。
/// タグの番号には名前を添える。
pub(crate) fn error_chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let part = s.to_string();
        if !text.ends_with(&part) {
            text = format!("{text}: {part}");
        }
        source = s.source();
    }
    with_tag_names(&text)
}

/// 文の中の `(gggg,eeee)` の後ろに、辞書にあるタグの名前を足す。
fn with_tag_names(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        out.push_str(&rest[..open]);
        let candidate = rest.get(open + 1..open + 10);
        let number = candidate.filter(|c| {
            c.as_bytes()[4] == b','
                && rest.as_bytes().get(open + 10) == Some(&b')')
                && c.bytes()
                    .enumerate()
                    .all(|(i, b)| i == 4 || b.is_ascii_hexdigit())
        });
        match number {
            Some(number) => {
                out.push_str(&rest[open..open + 11]);
                if let Some(name) = tag_name(&number.to_ascii_uppercase()) {
                    out.push(' ');
                    out.push_str(name);
                }
                rest = &rest[open + 11..];
            }
            None => {
                out.push('(');
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 画素を展開する前に、Rows・Columns・Samples per Pixel・Bits Allocated から要るメモリを確かめる。
fn check_pixel_size(obj: &DefaultDicomObject) -> Result<(), DecodeError> {
    let (rows, columns) = (
        int(obj, tags::ROWS).unwrap_or(0).max(0) as u64,
        int(obj, tags::COLUMNS).unwrap_or(0).max(0) as u64,
    );
    let samples = int(obj, tags::SAMPLES_PER_PIXEL).unwrap_or(1).max(1) as u64;
    let bytes = (int(obj, tags::BITS_ALLOCATED).unwrap_or(16).max(1) as u64).div_ceil(8);
    vw_core::limits::check_image(columns, rows, samples * bytes).map(drop)
}

fn frames_in(obj: &DefaultDicomObject) -> usize {
    int(obj, tags::NUMBER_OF_FRAMES).map_or(1, |n| n.max(1) as usize)
}

/// 項目の Specific Character Set。項目に書いていなければ、外側の `inherited` を使う。
fn charset_of(obj: &dicom_object::InMemDicomObject, inherited: Charset) -> Charset {
    obj.get(tags::SPECIFIC_CHARACTER_SET)
        .and_then(|e| e.to_multi_str().ok())
        .map_or(inherited, |values| Charset::from_values(&values))
}

fn text(obj: &DefaultDicomObject, tag: Tag) -> Option<String> {
    let charset = charset_of(obj, Charset::default());
    let s = charset::value_text(obj.get(tag)?.value().primitive()?, &charset);
    let s = s.trim_matches([' ', '\0']);
    (!s.is_empty()).then(|| s.to_owned())
}

fn int(obj: &DefaultDicomObject, tag: Tag) -> Option<i64> {
    obj.get(tag)?.to_int::<i64>().ok()
}

/// フレームによらず、ファイルで 1 度だけ読めばよいカラーの情報。
enum Colors {
    /// dicom-pixeldata が展開した画素をそのまま使う。
    Decoded,
    Palette(color::Palette),
    /// MONOCHROME2 の灰色の画像に、範囲の値だけ色を当てる表（Supplemental Palette）。
    Supplemental(color::Palette),
    /// 非圧縮の YBR_FULL_422。dicom-pixeldata を通さず、画素データを自前で広げる。
    NativeYbr422,
}

impl Colors {
    fn of(obj: &DefaultDicomObject) -> Result<Self, DecodeError> {
        match text(obj, tags::PHOTOMETRIC_INTERPRETATION).as_deref() {
            Some("PALETTE COLOR") => Ok(Self::Palette(color::Palette::read(obj)?)),
            Some("YBR_FULL_422")
                if !net::is_native(obj.meta().transfer_syntax().trim_end_matches('\0')) =>
            {
                Ok(Self::Decoded)
            }
            Some("YBR_FULL_422") => Ok(Self::NativeYbr422),
            // 表が壊れていても灰色の画像としては読めるので、失敗にせず灰色だけで出す。
            Some("MONOCHROME2")
                if obj
                    .get(tags::RED_PALETTE_COLOR_LOOKUP_TABLE_DESCRIPTOR)
                    .is_some() =>
            {
                Ok(color::Palette::read(obj).map_or(Self::Decoded, Self::Supplemental))
            }
            _ => Ok(Self::Decoded),
        }
    }
}

fn decode_one(
    obj: &DefaultDicomObject,
    colors: &Colors,
    frame: usize,
) -> Result<VwImage, DecodeError> {
    if let Colors::NativeYbr422 = colors {
        return native_ybr_422(obj, frame);
    }
    let px = obj
        .decode_pixel_data_frame(frame as u32)
        .map_err(|e| DecodeError::Unsupported(error_chain(&e)))?;
    to_image(&px, frame, colors)
}

/// 1 フレーム分の画素を [`VwImage`] にする。
fn to_image(px: &DecodedPixelData, frame: usize, colors: &Colors) -> Result<VwImage, DecodeError> {
    let (w, h) = (px.columns(), px.rows());
    let pixels = w as usize * h as usize;
    let raw = px.data();
    match (px.samples_per_pixel(), colors) {
        (1, Colors::Palette(palette)) => {
            let stored = u32::from(px.bits_stored()).clamp(1, 32);
            let signed = px.pixel_representation() == PixelRepresentation::Signed;
            let values = stored_values(raw, px.bits_allocated(), stored, signed, pixels)?;
            palette.apply(w, h, &values)
        }
        (1, Colors::Supplemental(palette)) => gray_image(px, frame, raw, pixels, Some(palette)),
        (1, _) => gray_image(px, frame, raw, pixels, None),
        (3, _) => color_image(px, raw, pixels),
        (n, _) => Err(DecodeError::Unsupported(format!("{n} samples per pixel"))),
    }
}

/// 非圧縮の YBR_FULL_422 の `frame` 番目のフレーム。
fn native_ybr_422(obj: &DefaultDicomObject, frame: usize) -> Result<VwImage, DecodeError> {
    let u = |tag| int(obj, tag).unwrap_or(0).max(0) as usize;
    let (columns, rows, allocated) = (u(tags::COLUMNS), u(tags::ROWS), u(tags::BITS_ALLOCATED));
    let stored = int(obj, tags::BITS_STORED).map_or(allocated as u16, |v| v as u16);
    let sample = allocated.div_ceil(8).max(1);
    let data = obj
        .get(tags::PIXEL_DATA)
        .and_then(|e| e.value().primitive())
        .ok_or_else(|| DecodeError::Invalid("no pixel data".into()))?
        .to_bytes();
    let size = columns.div_ceil(2) * 4 * sample * rows;
    let raw = data
        .get(frame * size..(frame + 1) * size)
        .ok_or_else(|| DecodeError::Invalid(format!("pixel data has no frame {frame}")))?;
    let full = color::expand_422(raw, columns, rows, sample)?;
    color::interleaved_image(
        columns as u32,
        rows as u32,
        &full,
        allocated as u16,
        stored,
        true,
    )
}

/// モノクロの画素。値を変えずに済む（傾き 1・切片 0・符号なし・MONOCHROME2）なら U8 / U16 の
/// まま持ち、それ以外は Rescale を当てた F32 にする。U16 に詰め直して持つ方式は、CT の負の値
/// （空気は -1000 HU）が表せないので採らない。
fn gray_image(
    px: &DecodedPixelData,
    frame: usize,
    raw: &[u8],
    pixels: usize,
    supplemental: Option<&color::Palette>,
) -> Result<VwImage, DecodeError> {
    let stored = u32::from(px.bits_stored()).clamp(1, 32);
    let signed = px.pixel_representation() == PixelRepresentation::Signed;
    let invert = *px.photometric_interpretation() == PhotometricInterpretation::Monochrome1;
    let (slope, intercept) = px
        .rescale()
        .ok()
        .and_then(|r| r.get(frame).or(r.first()))
        .map_or((1.0, 0.0), |r| (r.slope, r.intercept));
    let function = px
        .voi_lut_function()
        .ok()
        .flatten()
        .and_then(|f| f.get(frame).or(f.first()).copied())
        .unwrap_or_default();
    let window = px
        .window()
        .ok()
        .flatten()
        .and_then(|w| w.get(frame).or(w.first()))
        .filter(|w| w.width > 0.0)
        .map(|w| linear_window(w.center, w.width, function));
    let values = stored_values(raw, px.bits_allocated(), stored, signed, pixels)?;
    // 近い値を同一視する必要はない。ちょうど恒等のときだけ格納値をそのまま使う近道で、
    // 外れても遅い道が同じ結果を出す。
    let plain = slope == 1.0 && intercept == 0.0 && !signed && !invert;
    let (data, significant_bits) = match (plain, px.bits_allocated()) {
        (true, 8) => (PixelData::U8(values.iter().map(|&v| v as u8).collect()), 8),
        (true, 16) => (
            PixelData::U16(values.iter().map(|&v| v as u16).collect()),
            stored.min(16),
        ),
        _ => {
            let sign = if invert { -1.0 } else { 1.0 };
            let f = values
                .iter()
                .map(|&v| (sign * (v as f64 * slope + intercept)) as f32)
                .collect();
            (PixelData::F32(f), 32)
        }
    };
    let mut img = VwImage::new(px.columns(), px.rows(), ColorModel::Gray, data)
        .map_err(DecodeError::Invalid)?;
    img.significant_bits = significant_bits;
    // MONOCHROME1 は値を反転して持つので、ウィンドウも反転する。
    img.window = window.map(|(lo, hi)| if invert { (-hi, -lo) } else { (lo, hi) });
    let sign = if invert { -1.0 } else { 1.0 };
    img.value_colors =
        supplemental.map(|p| std::sync::Arc::new(p.value_colors(slope, intercept, sign)));
    Ok(img)
}

/// ウィンドウを、vw-core が使う「下端から上端へ直線で 0..=255」の範囲にする。
///
/// LINEAR は規格（PS3.3 C.11.2.1.2.1）の式どおり中心を 0.5 ずらし、幅から 1 を引く。
/// 単純に中心 ± 幅 / 2 にすると、ピクセルパイプラインの図（規格の式で描く）と最大 2 段ずれ、
/// 図をファイルの値のまま開いても表示が変わってしまうため。
/// SIGMOID は直線では表せないので、中心 ± 幅 / 2 の直線で近似する。
fn linear_window(center: f64, width: f64, function: VoiLutFunction) -> (f32, f32) {
    let (lo, hi) = match function {
        VoiLutFunction::Linear => {
            let half = (width - 1.0).max(0.0) / 2.0;
            (center - 0.5 - half, center - 0.5 + half)
        }
        VoiLutFunction::LinearExact | VoiLutFunction::Sigmoid => {
            (center - width / 2.0, center + width / 2.0)
        }
    };
    (lo as f32, hi as f32)
}

/// 格納されている値。Bits Stored より上のビット（オーバーレイなどに使われる）は捨て、
/// 符号付きなら Bits Stored の最上位ビットで符号を広げる。
fn stored_values(
    raw: &[u8],
    allocated: u16,
    stored: u32,
    signed: bool,
    pixels: usize,
) -> Result<Vec<i64>, DecodeError> {
    let bytes = match allocated {
        8 => 1,
        16 => 2,
        32 => 4,
        n => return Err(DecodeError::Unsupported(format!("{n} bits allocated"))),
    };
    if raw.len() < pixels * bytes {
        return Err(DecodeError::Invalid(format!(
            "pixel data has {} bytes, expected {}",
            raw.len(),
            pixels * bytes
        )));
    }
    let mask = if stored >= 64 {
        u64::MAX
    } else {
        (1u64 << stored) - 1
    };
    let sign_bit = 1u64 << (stored - 1);
    Ok(raw[..pixels * bytes]
        .chunks_exact(bytes)
        .map(|b| {
            let mut v = [0u8; 8];
            v[..bytes].copy_from_slice(b);
            let u = u64::from_le_bytes(v) & mask;
            if signed && u & sign_bit != 0 {
                u as i64 - (mask as i64 + 1)
            } else {
                u as i64
            }
        })
        .collect())
}

/// カラーの画素（8bit / 16bit の RGB・YBR_FULL）。
fn color_image(px: &DecodedPixelData, raw: &[u8], pixels: usize) -> Result<VwImage, DecodeError> {
    let ybr = match px.photometric_interpretation() {
        PhotometricInterpretation::Rgb => false,
        PhotometricInterpretation::YbrFull => true,
        other => {
            return Err(DecodeError::Unsupported(format!(
                "photometric interpretation {other}"
            )));
        }
    };
    let sample = usize::from(px.bits_allocated()).div_ceil(8);
    let interleaved;
    let raw = match px.planar_configuration() {
        PlanarConfiguration::Standard => raw,
        PlanarConfiguration::PixelFirst => {
            interleaved = color::interleave_planes(raw, pixels, sample);
            &interleaved
        }
    };
    color::interleaved_image(
        px.columns(),
        px.rows(),
        raw,
        px.bits_allocated(),
        px.bits_stored(),
        ybr,
    )
}

/// 表示するメタデータ。検査やシリーズの並べ替えに使う UID もここから読む。
fn metadata(obj: &DefaultDicomObject) -> Metadata {
    metadata_with(obj, &file_meta_rows(obj))
}

/// `meta_rows` は「DICOM tags」の頭に出すメタ情報の要素。
fn metadata_with(obj: &DefaultDicomObject, meta_rows: &[(Tag, String)]) -> Metadata {
    let mut m = Metadata::default();
    let fields: &[(&str, &str, Tag)] = &[
        ("Patient", "Name", tags::PATIENT_NAME),
        ("Patient", "ID", tags::PATIENT_ID),
        ("Patient", "Birth date", tags::PATIENT_BIRTH_DATE),
        ("Patient", "Sex", tags::PATIENT_SEX),
        ("Patient", "Weight (kg)", tags::PATIENT_WEIGHT),
        ("DICOM", "Modality", tags::MODALITY),
        ("DICOM", "Study date", tags::STUDY_DATE),
        ("DICOM", "Study time", tags::STUDY_TIME),
        ("DICOM", "Study", tags::STUDY_DESCRIPTION),
        ("DICOM", "Series number", tags::SERIES_NUMBER),
        ("DICOM", "Series", tags::SERIES_DESCRIPTION),
        ("DICOM", "Instance number", tags::INSTANCE_NUMBER),
        ("DICOM", "Rows", tags::ROWS),
        ("DICOM", "Columns", tags::COLUMNS),
        ("DICOM", "Number of frames", tags::NUMBER_OF_FRAMES),
        ("DICOM", "Frame time (ms)", tags::FRAME_TIME),
        ("DICOM", "Cine rate", tags::CINE_RATE),
        (
            "DICOM",
            "Recommended frame rate",
            tags::RECOMMENDED_DISPLAY_FRAME_RATE,
        ),
        ("DICOM", "Photometric", tags::PHOTOMETRIC_INTERPRETATION),
        ("DICOM", "Bits allocated", tags::BITS_ALLOCATED),
        ("DICOM", "Bits stored", tags::BITS_STORED),
        ("DICOM", "Pixel representation", tags::PIXEL_REPRESENTATION),
        ("DICOM", "Rescale slope", tags::RESCALE_SLOPE),
        ("DICOM", "Rescale intercept", tags::RESCALE_INTERCEPT),
        ("DICOM", "Units", tags::UNITS),
        ("DICOM", "Decay correction", tags::DECAY_CORRECTION),
        ("DICOM", "Window center", tags::WINDOW_CENTER),
        ("DICOM", "Window width", tags::WINDOW_WIDTH),
        ("DICOM", "Slice thickness", tags::SLICE_THICKNESS),
        ("DICOM", "Slice location", tags::SLICE_LOCATION),
        ("DICOM", "Pixel spacing", tags::PIXEL_SPACING),
        ("DICOM", "Manufacturer", tags::MANUFACTURER),
        ("DICOM", "Model", tags::MANUFACTURER_MODEL_NAME),
        ("DICOM", "Institution", tags::INSTITUTION_NAME),
        ("DICOM", "Study UID", tags::STUDY_INSTANCE_UID),
        ("DICOM", "Series UID", tags::SERIES_INSTANCE_UID),
        ("DICOM", "SOP instance UID", tags::SOP_INSTANCE_UID),
    ];
    for &(group, key, tag) in fields {
        if let Some(v) = text(obj, tag) {
            m.push(group, key, v);
        }
    }
    if m.get("DICOM", "Modality") == Some("PT") && m.get("DICOM", "Units") == Some("BQML") {
        match suv::factor(&suv::read(obj)) {
            Ok(f) => m.push("DICOM", SUV_FACTOR, f.to_string()),
            Err(missing) => m.push("DICOM", SUV_NEEDS, missing.join(", ")),
        }
    }
    if obj
        .get(tags::ENHANCED_PALETTE_COLOR_LOOKUP_TABLE_SEQUENCE)
        .is_some()
    {
        m.push("DICOM", NOT_APPLIED, ENHANCED_PALETTE);
    }
    let ts = obj.meta().transfer_syntax().trim_end_matches('\0');
    let ts_name = dicom_transfer_syntax_name(ts);
    m.push("DICOM", "Transfer syntax", ts_name.unwrap_or(ts));
    tag_listing(obj, meta_rows, &mut m);
    check::push_problems(obj, &charset_of(obj, Charset::default()), 0, &mut m);
    m
}

/// PET の Bq/ml に掛けると SUVbw になる値（[`suv::factor`]）のメタデータのキー。
pub const SUV_FACTOR: &str = "SUVbw factor";
/// SUVbw を出せないときに、足りない項目を並べるキー。
pub const SUV_NEEDS: &str = "SUVbw needs";

/// ファイルにあるが vw が当てていない表示の指定を書くキー。読み込んだときにログにも出す。
pub const NOT_APPLIED: &str = "Not applied";
/// Enhanced Palette Color LUT（PS3.3 C.7.6.23）は、ken の判断（2026-11-11）で作らず、灰色で出す。
pub const ENHANCED_PALETTE: &str = "Enhanced Palette Color LUT (shown in gray)";

/// メタデータ欄で全タグを並べるグループ。
pub const TAG_LISTING: &str = "DICOM tags";

/// 長い値はここまでで切る。検索カタログの値の上限と揃える。
const LISTED_VALUE_MAX: usize = 2048;

/// ファイルの全タグを、ファイルの順に `(0010,0010) PatientName` の形のキーで並べる。
/// シーケンスの項目の中のタグは、深さの分だけキーの頭に `>` を付ける（dcmdump と同じ書き方）。
///
/// 主なタグを名前で出す上のグループとは別に持つのは、そちらを分かりやすい名前のまま残したいのと、
/// シリーズの並べ替えなどがそちらのキーを読むため。
fn tag_listing(obj: &DefaultDicomObject, meta_rows: &[(Tag, String)], m: &mut Metadata) {
    for (tag, value) in meta_rows {
        m.push(TAG_LISTING, listed_key(*tag, 0), listed_value(value));
    }
    list_elements(obj, &charset_of(obj, Charset::default()), 0, m);
}

/// dicom-rs が読んだメタ情報のうち、欄に出す要素。
fn file_meta_rows(obj: &DefaultDicomObject) -> Vec<(Tag, String)> {
    let meta = obj.meta();
    let mut rows = vec![
        (
            tags::MEDIA_STORAGE_SOP_CLASS_UID,
            meta.media_storage_sop_class_uid.clone(),
        ),
        (
            tags::MEDIA_STORAGE_SOP_INSTANCE_UID,
            meta.media_storage_sop_instance_uid.clone(),
        ),
        (tags::TRANSFER_SYNTAX_UID, meta.transfer_syntax.clone()),
        (
            tags::IMPLEMENTATION_CLASS_UID,
            meta.implementation_class_uid.clone(),
        ),
    ];
    if let Some(name) = &meta.implementation_version_name {
        rows.push((tags::IMPLEMENTATION_VERSION_NAME, name.clone()));
    }
    rows
}

fn list_elements(
    obj: &dicom_object::InMemDicomObject,
    charset: &Charset,
    depth: usize,
    m: &mut Metadata,
) {
    use dicom_core::VR;
    use dicom_core::value::Value;
    for e in obj.iter() {
        let tag = e.header().tag;
        let key = listed_key(tag, depth);
        match e.value() {
            Value::Sequence(seq) => {
                let n = seq.items().len();
                let unit = if n == 1 { "item" } else { "items" };
                m.push(TAG_LISTING, key, format!("{n} {unit}"));
                for item in seq.items() {
                    list_elements(item, &charset_of(item, *charset), depth + 1, m);
                }
            }
            Value::PixelSequence(seq) => {
                m.push(
                    TAG_LISTING,
                    key,
                    format!("({} fragments)", seq.fragments().len()),
                );
            }
            Value::Primitive(p) => {
                let binary = matches!(
                    e.vr(),
                    VR::OB | VR::OD | VR::OF | VR::OL | VR::OV | VR::OW | VR::UN
                );
                let value = if binary {
                    format!("({:?}, {} bytes)", e.vr(), p.calculate_byte_len())
                } else {
                    listed_value(&charset::value_text(p, charset))
                };
                m.push(TAG_LISTING, key, value);
            }
        }
    }
}

fn listed_key(tag: Tag, depth: usize) -> String {
    let number = format!("{:04X},{:04X}", tag.group(), tag.element());
    let name = tag_name(&number).unwrap_or("");
    format!("{}({number}) {name}", ">".repeat(depth))
        .trim_end()
        .to_owned()
}

fn listed_value(value: &str) -> String {
    let value = value.trim_matches([' ', '\0']);
    if value.len() <= LISTED_VALUE_MAX {
        return value.to_owned();
    }
    let mut end = LISTED_VALUE_MAX;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} bytes)", &value[..end], value.len())
}

/// よく見る転送構文の名前。
fn dicom_transfer_syntax_name(uid: &str) -> Option<&'static str> {
    Some(match uid {
        "1.2.840.10008.1.2" => "Implicit VR Little Endian",
        "1.2.840.10008.1.2.1" => "Explicit VR Little Endian",
        "1.2.840.10008.1.2.2" => "Explicit VR Big Endian",
        "1.2.840.10008.1.2.5" => "RLE Lossless",
        "1.2.840.10008.1.2.4.50" => "JPEG Baseline",
        "1.2.840.10008.1.2.4.51" => "JPEG Extended",
        "1.2.840.10008.1.2.4.57" => "JPEG Lossless",
        "1.2.840.10008.1.2.4.70" => "JPEG Lossless SV1",
        "1.2.840.10008.1.2.4.80" => "JPEG-LS Lossless",
        "1.2.840.10008.1.2.4.81" => "JPEG-LS Near-lossless",
        "1.2.840.10008.1.2.4.90" => "JPEG 2000 Lossless",
        "1.2.840.10008.1.2.4.91" => "JPEG 2000",
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use dicom_core::VR;
    use synth::{
        CT_IMAGE, JPEG_2000_LOSSLESS, JPEG_LS_LOSSLESS, JPEG_LS_NEAR_LOSSLESS, Synth, XA_IMAGE,
    };

    #[test]
    fn a_value_running_past_the_end_names_the_tag_it_belongs_to() {
        let mut data = Synth::ct(2, 2).patient("Doe^Jane").bytes();
        let at = data.windows(8).position(|w| w == b"Doe^Jane").unwrap();
        // 患者名の長さの欄（値の直前の 2 バイト）を、ファイルの残りより長くする。
        data[at - 2..at].copy_from_slice(&0xFFF0u16.to_le_bytes());
        let err = DicomPlugin.read_metadata(&data).unwrap_err().to_string();
        assert!(err.contains("(0010,0010) PatientName"), "{err}");
    }

    #[test]
    fn tag_numbers_in_error_text_get_their_names() {
        assert_eq!(
            with_tag_names("bad (0028,0030) and (abcd,ef01) (not a tag)"),
            "bad (0028,0030) PixelSpacing and (abcd,ef01) (not a tag)"
        );
    }

    #[test]
    fn huge_rows_and_columns_are_refused_before_decoding_pixels() {
        // 65535 x 65535 の 16bit（8 GiB）を名乗る、画素が 1 つしかないファイル。
        let data = Synth::new(CT_IMAGE, 65_535, 65_535)
            .pixels_i16(&[0])
            .bytes();
        let err = DicomPlugin.decode(&data).unwrap_err().to_string();
        assert!(err.contains("65535 x 65535"), "{err}");
        assert!(err.contains("image size limit"), "{err}");
    }

    /// testdata/*.j2k に入れた値。testdata/make_j2k.py で作った。
    pub(crate) const J2K_GRAY16: [u16; 8] = [0, 1000, 2000, 3000, 4095, 300, 40, 7];
    const J2K_RGB8: [u8; 24] = [
        255, 0, 0, 0, 255, 0, 0, 0, 255, 10, 20, 30, 200, 100, 50, 0, 0, 0, 255, 255, 255, 128, 64,
        32,
    ];

    pub(crate) fn jpeg_ls(
        width: u32,
        height: u32,
        bits: i32,
        components: i32,
        near: i32,
        src: &[u8],
    ) -> Vec<u8> {
        let frame = charls::FrameInfo {
            width,
            height,
            bits_per_sample: bits,
            component_count: components,
        };
        charls::CharLS::default().encode(frame, near, src).unwrap()
    }

    fn u16_bytes(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn gray_f32(img: &VwImage) -> &[f32] {
        match &img.data {
            PixelData::F32(v) => v,
            other => panic!("expected F32, got {} bits", other.bits_per_sample()),
        }
    }

    #[test]
    fn recognises_the_dicm_magic_after_the_preamble() {
        let bytes = Synth::ct(2, 2).bytes();
        assert!(DicomPlugin.probe(&bytes));
        assert!(!DicomPlugin.probe(b"\x89PNG\r\n\x1a\n"));
        assert!(!DicomPlugin.probe(&bytes[..100]));
    }

    #[test]
    fn ct_values_become_hounsfield_units_with_the_files_window() {
        let synth = Synth::ct(2, 1).pixels_i16(&[0, 1000]).rescale(1.0, -1024.0);
        let img = DicomPlugin
            .decode(&synth.window(40.0, 400.0).bytes())
            .unwrap();
        assert_eq!(gray_f32(&img), [-1024.0, -24.0]);
        // LINEAR の規格の式: 中心 - 0.5 ± (幅 - 1) / 2。
        assert_eq!(img.window, Some((-160.0, 239.0)));
    }

    #[test]
    fn linear_exact_window_spans_center_plus_minus_half_width() {
        let synth = Synth::ct(2, 1)
            .pixels_i16(&[0, 1000])
            .rescale(1.0, -1024.0)
            .window(40.0, 400.0)
            .text(tags::VOILUT_FUNCTION, dicom_core::VR::CS, "LINEAR_EXACT");
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert_eq!(img.window, Some((-160.0, 240.0)));
    }

    #[test]
    fn plain_16bit_images_stay_u16_with_bits_stored_as_depth() {
        let synth = Synth::new(CT_IMAGE, 2, 1).pixels_u16(&[0x0fff, 0xf123], 12);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        // Bits Stored より上のビットは捨てる。
        assert!(matches!(&img.data, PixelData::U16(v) if v == &[0x0fff, 0x0123]));
        assert_eq!(img.significant_bits, 12);
        assert_eq!(img.window, None);
    }

    #[test]
    fn signed_values_are_sign_extended_from_bits_stored() {
        // 12bit の符号付きで 0xfff は -1。
        let synth = Synth::new(CT_IMAGE, 1, 1)
            .pixels_u16(&[0x0fff], 12)
            .signed();
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert_eq!(gray_f32(&img), [-1.0]);
    }

    #[test]
    fn monochrome1_is_inverted_together_with_its_window() {
        let synth = Synth::new(CT_IMAGE, 2, 1)
            .pixels_u16(&[0, 100], 16)
            .monochrome1()
            .window(50.0, 100.0);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert_eq!(gray_f32(&img), [0.0, -100.0]);
        assert_eq!(img.window, Some((-99.0, 0.0)));
    }

    #[test]
    fn each_frame_of_a_multiframe_file_is_read_separately() {
        let synth = Synth::new(XA_IMAGE, 1, 1).frames_u8(&[&[10], &[20], &[30]]);
        let bytes = synth.bytes();
        assert_eq!(DicomPlugin.frame_count(&bytes).unwrap(), 3);
        let third = DicomPlugin.decode_frame(&bytes, 2).unwrap();
        assert!(matches!(&third.data, PixelData::U8(v) if v == &[30]));
        assert!(DicomPlugin.decode_frame(&bytes, 3).is_err());
    }

    #[test]
    fn decode_frames_hands_out_frames_in_the_asked_order_and_stops_when_told() {
        let synth = Synth::new(XA_IMAGE, 1, 1).frames_u8(&[&[10], &[20], &[30]]);
        let bytes = synth.bytes();
        let mut got = Vec::new();
        DicomPlugin.decode_frames(
            std::path::Path::new("x.dcm"),
            &bytes,
            &[2, 0, 1],
            &mut |i, img| {
                let img = img.unwrap();
                let PixelData::U8(v) = &img.data else {
                    panic!("8-bit frames");
                };
                got.push((i, v[0]));
                got.len() < 2
            },
        );
        assert_eq!(got, [(2, 30), (0, 10)]);
    }

    #[test]
    fn decode_frames_reports_a_frame_the_file_does_not_have() {
        let bytes = Synth::new(XA_IMAGE, 1, 1).frames_u8(&[&[10]]).bytes();
        let mut errors = 0;
        DicomPlugin.decode_frames(
            std::path::Path::new("x.dcm"),
            &bytes,
            &[0, 5],
            &mut |_, img| {
                errors += usize::from(img.is_err());
                true
            },
        );
        assert_eq!(errors, 1);
    }

    #[test]
    fn rgb_images_are_read_in_either_planar_configuration() {
        let interleaved = Synth::rgb(2, 1, &[1, 2, 3, 4, 5, 6], false);
        let planar = Synth::rgb(2, 1, &[1, 4, 2, 5, 3, 6], true);
        for synth in [interleaved, planar] {
            let img = DicomPlugin.decode(&synth.bytes()).unwrap();
            assert!(matches!(&img.data, PixelData::U8(v) if v == &[1, 2, 3, 4, 5, 6]));
        }
    }

    fn rgb_u8(img: &VwImage) -> &[u8] {
        assert_eq!(img.color, ColorModel::Rgb);
        match &img.data {
            PixelData::U8(v) => v,
            other => panic!("expected U8, got {} bits", other.bits_per_sample()),
        }
    }

    fn rgb_u16(img: &VwImage) -> &[u16] {
        assert_eq!(img.color, ColorModel::Rgb);
        match &img.data {
            PixelData::U16(v) => v,
            other => panic!("expected U16, got {} bits", other.bits_per_sample()),
        }
    }

    /// 番号 0〜3 を 黒・赤・緑・青 に引く 8bit の表（2 項目を 1 語に詰める）。
    fn four_color_palette(indices: &[u8]) -> Synth {
        let pack = |a: u8, b: u8| u16::from_le_bytes([a, b]);
        Synth::palette(indices.len() as u16, 1, indices)
            .lut(0, 4, 0, 8, &[pack(0, 255), pack(0, 0)], false)
            .lut(1, 4, 0, 8, &[pack(0, 0), pack(255, 0)], false)
            .lut(2, 4, 0, 8, &[pack(0, 0), pack(0, 255)], false)
    }

    #[test]
    fn palette_color_indices_are_looked_up_in_the_color_tables() {
        let img = DicomPlugin
            .decode(&four_color_palette(&[0, 1, 2, 3]).bytes())
            .unwrap();
        assert_eq!(rgb_u8(&img), [0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn palette_color_with_16_bit_tables_stays_16_bit() {
        let ramp = [0, 1000, 65535];
        let bytes = Synth::palette(3, 1, &[10, 11, 12])
            .lut(0, 3, 10, 16, &ramp, false)
            .lut(1, 3, 10, 16, &[0, 0, 0], false)
            .lut(2, 3, 10, 16, &[65535, 65535, 65535], false)
            .bytes();
        let img = DicomPlugin.decode(&bytes).unwrap();
        assert_eq!(
            rgb_u16(&img),
            [0, 0, 65535, 1000, 0, 65535, 65535, 0, 65535]
        );
    }

    #[test]
    fn segmented_palette_color_tables_are_expanded() {
        // 256 項目: 0 から 255 へ直線（離散 1 項目 + 直線 255 項目）。緑と青は 0 のまま。
        let up = [0, 1, 0, 1, 255, 255 * 256];
        let flat = [0, 1, 0, 1, 255, 0];
        let bytes = Synth::palette(3, 1, &[0, 128, 255])
            .lut(0, 256, 0, 16, &up, true)
            .lut(1, 256, 0, 16, &flat, true)
            .lut(2, 256, 0, 16, &flat, true)
            .bytes();
        let img = DicomPlugin.decode(&bytes).unwrap();
        let red: Vec<u16> = rgb_u16(&img).iter().step_by(3).copied().collect();
        assert_eq!(red, [0, 128 * 256, 255 * 256]);
    }

    #[test]
    fn palette_color_keeps_the_index_of_each_pixel() {
        let img = DicomPlugin
            .decode(&four_color_palette(&[3, 0, 2]).bytes())
            .unwrap();
        let indices = img.indices.expect("palette images keep their indices");
        assert_eq!(
            (0..3).map(|i| indices.get(i)).collect::<Vec<_>>(),
            [Some(3), Some(0), Some(2)]
        );
    }

    /// 格納値 100〜101 を赤・緑にする Supplemental Palette の付いた、16bit の灰色の画像。
    fn supplemental(values: &[u16]) -> Synth {
        let pack = |a: u8, b: u8| u16::from_le_bytes([a, b]);
        Synth::new(synth::SC_IMAGE, values.len() as u16, 1)
            .pixels_u16(values, 12)
            .lut(0, 2, 100, 8, &[pack(255, 0)], false)
            .lut(1, 2, 100, 8, &[pack(0, 255)], false)
            .lut(2, 2, 100, 8, &[pack(0, 0)], false)
    }

    #[test]
    fn supplemental_palette_keeps_the_image_gray_and_colors_only_its_range() {
        let img = DicomPlugin
            .decode(&supplemental(&[99, 100, 101, 102]).bytes())
            .unwrap();
        assert_eq!(img.color, ColorModel::Gray);
        assert!(matches!(&img.data, PixelData::U16(v) if v == &[99, 100, 101, 102]));
        let colors = img
            .value_colors
            .expect("the palette is kept for the display");
        assert_eq!(colors.range(), (100, 101));
        assert_eq!(colors.color_of(99.0), None);
        assert_eq!(colors.color_of(100.0), Some([65535, 0, 0]));
        assert_eq!(colors.color_of(101.0), Some([0, 65535, 0]));
    }

    #[test]
    fn supplemental_palette_follows_the_rescale_of_the_gray_values() {
        let bytes = supplemental(&[100]).rescale(1.0, -1024.0).bytes();
        let img = DicomPlugin.decode(&bytes).unwrap();
        assert!(matches!(&img.data, PixelData::F32(v) if v == &[-924.0]));
        assert_eq!(
            img.value_colors.unwrap().color_of(-924.0),
            Some([65535, 0, 0])
        );
    }

    #[test]
    fn enhanced_palette_is_noted_as_not_applied() {
        let bytes = Synth::ct(1, 1)
            .empty_sequence(tags::ENHANCED_PALETTE_COLOR_LOOKUP_TABLE_SEQUENCE)
            .bytes();
        let img = DicomPlugin.decode(&bytes).unwrap();
        assert_eq!(img.color, ColorModel::Gray);
        assert_eq!(
            img.metadata.get("DICOM", NOT_APPLIED),
            Some(ENHANCED_PALETTE)
        );
    }

    #[test]
    fn palette_color_without_its_tables_is_an_error_not_a_gray_image() {
        let bytes = Synth::palette(1, 1, &[0]).bytes();
        let err = DicomPlugin.decode(&bytes).unwrap_err().to_string();
        assert!(err.contains("palette"), "{err}");
    }

    #[test]
    fn uncompressed_ybr_full_422_is_widened_to_one_rgb_per_pixel() {
        // 4 x 2: 1 行目は灰色の 2 組、2 行目は赤（Y 76, Cb 85, Cr 255）の 2 組。
        let data = [
            100, 100, 128, 128, 200, 200, 128, 128, //
            76, 76, 85, 255, 76, 76, 85, 255,
        ];
        let frame = DicomPlugin
            .decode(&Synth::ybr_full_422(4, 2, &data).bytes())
            .unwrap();
        let px = rgb_u8(&frame);
        assert_eq!(
            &px[..12],
            [100, 100, 100, 100, 100, 100, 200, 200, 200, 200, 200, 200]
        );
        for red in px[12..].chunks(3) {
            assert!(red[0] > 250 && red[1] < 5 && red[2] < 5, "{red:?}");
        }
    }

    #[test]
    fn each_frame_of_an_uncompressed_ybr_full_422_file_is_read_separately() {
        let frames = [[10, 10, 128, 128], [90, 90, 128, 128]];
        let bytes = Synth::ybr_full_422(2, 1, &frames.concat())
            .text(tags::NUMBER_OF_FRAMES, VR::IS, "2")
            .bytes();
        for (i, y) in [10, 90].into_iter().enumerate() {
            let img = DicomPlugin.decode_frame(&bytes, i).unwrap();
            assert_eq!(rgb_u8(&img), [y; 6]);
        }
    }

    #[test]
    fn sixteen_bit_rgb_stays_16_bit_with_bits_stored_as_depth() {
        let values = [4095, 0, 2000, 1, 2, 3];
        let img = DicomPlugin
            .decode(&Synth::rgb16(2, 1, &values, 12).bytes())
            .unwrap();
        assert_eq!(rgb_u16(&img), values);
        assert_eq!(img.significant_bits, 12);
    }

    #[test]
    fn sixteen_bit_ybr_full_is_converted_to_16_bit_rgb() {
        let bytes = Synth::rgb16(1, 1, &[40000, 32768, 32768], 16)
            .photometric("YBR_FULL")
            .bytes();
        let img = DicomPlugin.decode(&bytes).unwrap();
        assert_eq!(rgb_u16(&img), [40000, 40000, 40000]);
    }

    #[test]
    fn patient_details_go_to_their_own_metadata_group() {
        let bytes = Synth::ct(1, 1).patient("Doe^Jane").bytes();
        let m = DicomPlugin.read_metadata(&bytes).unwrap();
        assert_eq!(m.get("Patient", "Name"), Some("Doe^Jane"));
        assert_eq!(m.get("DICOM", "Modality"), Some("CT"));
        assert_eq!(
            m.get("DICOM", "Transfer syntax"),
            Some("Explicit VR Little Endian")
        );
    }

    fn pet(weight: Option<&str>) -> Synth {
        let mut s = Synth::new(synth::PET_IMAGE, 1, 1)
            .pixels_u16(&[100], 16)
            .rescale(124.0, 0.0)
            .text(tags::UNITS, dicom_core::VR::CS, "BQML")
            .text(tags::DECAY_CORRECTION, dicom_core::VR::CS, "START")
            .text(tags::SERIES_DATE, dicom_core::VR::DA, "20261018")
            .text(tags::SERIES_TIME, dicom_core::VR::TM, "100000")
            .radiopharmaceutical(Some(370e6), Some(6586.2), Some("090000"));
        if let Some(w) = weight {
            s = s.text(tags::PATIENT_WEIGHT, dicom_core::VR::DS, w);
        }
        s
    }

    #[test]
    fn pet_in_bq_per_ml_carries_the_suv_factor() {
        let m = DicomPlugin.read_metadata(&pet(Some("70")).bytes()).unwrap();
        assert_eq!(m.get("DICOM", "Units"), Some("BQML"));
        assert_eq!(m.get("Patient", "Weight (kg)"), Some("70"));
        let decayed = 370e6 * 0.5f64.powf(3600.0 / 6586.2);
        let factor: f64 = m.get("DICOM", SUV_FACTOR).unwrap().parse().unwrap();
        assert!((factor - 70_000.0 / decayed).abs() < 1e-12);
        assert_eq!(m.get("DICOM", SUV_NEEDS), None);
    }

    #[test]
    fn pet_without_a_weight_names_what_suv_needs() {
        let m = DicomPlugin.read_metadata(&pet(None).bytes()).unwrap();
        assert_eq!(m.get("DICOM", SUV_FACTOR), None);
        assert_eq!(m.get("DICOM", SUV_NEEDS), Some("Patient weight"));
    }

    #[test]
    fn catalog_metadata_has_every_tag_by_number_except_pixel_data() {
        let bytes = Synth::ct(2, 2).patient("Doe^Jane").bytes();
        let m = DicomPlugin.read_all_metadata(&bytes).unwrap();
        assert_eq!(m.get("DICOM", "0010,0010"), Some("Doe^Jane"));
        assert_eq!(m.get("DICOM", "0008,0060"), Some("CT"));
        assert_eq!(m.get("DICOM", "0002,0010"), Some("1.2.840.10008.1.2.1"));
        assert_eq!(m.get("DICOM", "7FE0,0010"), None);
        assert!(m.entries.iter().all(|(g, _, _)| g == "DICOM"));
    }

    #[test]
    fn every_tag_is_listed_by_number_and_name_with_sequence_items_nested() {
        let bytes = Synth::ct(2, 2)
            .patient("Doe^Jane")
            .frames_i16(&[&[0; 4], &[0; 4]])
            .per_frame_values(&[(1.0, -1024.0, 40.0, 400.0), (1.0, -824.0, 40.0, 400.0)])
            .bytes();
        let m = DicomPlugin.read_metadata(&bytes).unwrap();
        let listed = |key: &str| m.get(TAG_LISTING, key);
        assert_eq!(listed("(0010,0010) PatientName"), Some("Doe^Jane"));
        assert_eq!(
            listed("(0002,0010) TransferSyntaxUID"),
            Some("1.2.840.10008.1.2.1")
        );
        assert_eq!(
            listed("(5200,9230) PerFrameFunctionalGroupsSequence"),
            Some("2 items")
        );
        assert_eq!(
            listed(">(0028,9145) PixelValueTransformationSequence"),
            Some("1 item")
        );
        assert_eq!(listed(">>(0028,1052) RescaleIntercept"), Some("-1024"));
        assert_eq!(m.get("Patient", "Name"), Some("Doe^Jane"));
    }

    #[test]
    fn listed_values_longer_than_the_cap_are_cut_with_their_length() {
        let long = "A".repeat(LISTED_VALUE_MAX + 10);
        let shown = listed_value(&long);
        assert!(shown.starts_with(&"A".repeat(LISTED_VALUE_MAX)));
        assert!(shown.ends_with(&format!("… ({} bytes)", LISTED_VALUE_MAX + 10)));
    }

    #[test]
    fn japanese_names_written_with_iso_2022_escapes_are_read_everywhere() {
        let name = "Yamada^Taro=\x1b$B;3ED\x1b(B^\x1b$BB@O:\x1b(B";
        let study = "\x1b$B6;It\x1b(B CT";
        let bytes = Synth::ct(1, 1)
            .text(
                tags::SPECIFIC_CHARACTER_SET,
                VR::CS,
                "ISO 2022 IR 6\\ISO 2022 IR 87",
            )
            .patient(name)
            .text(tags::STUDY_DESCRIPTION, VR::LO, study)
            .bytes();
        let shown = DicomPlugin.read_metadata(&bytes).unwrap();
        assert_eq!(shown.get("Patient", "Name"), Some("Yamada^Taro=山田^太郎"));
        assert_eq!(shown.get("DICOM", "Study"), Some("胸部 CT"));
        assert_eq!(
            shown.get(TAG_LISTING, "(0010,0010) PatientName"),
            Some("Yamada^Taro=山田^太郎")
        );
        let catalog = DicomPlugin.read_all_metadata(&bytes).unwrap();
        assert_eq!(
            catalog.get("DICOM", "0010,0010"),
            Some("Yamada^Taro=山田^太郎")
        );
    }

    #[test]
    fn latin_text_with_kanji_written_by_pydicom_is_read_everywhere() {
        // pydicom 3 は漢字の後を ESC ( B ではなく ESC - A で戻す。`Synth` は ASCII しか書けないので、
        // 同じ長さの印で書いてからバイト列を置き換える。
        let written: &[u8] = b"\x1b-AKopf-CT \x1b$BF,It\x1b-A M\xfcller";
        let mark = "#".repeat(written.len());
        let mut bytes = Synth::ct(1, 1)
            .text(
                tags::SPECIFIC_CHARACTER_SET,
                VR::CS,
                "ISO 2022 IR 100\\ISO 2022 IR 87",
            )
            .text(tags::STUDY_DESCRIPTION, VR::LO, &mark)
            .bytes();
        let at = bytes
            .windows(written.len())
            .position(|w| w == mark.as_bytes())
            .unwrap();
        bytes[at..at + written.len()].copy_from_slice(written);
        let shown = DicomPlugin.read_metadata(&bytes).unwrap();
        assert_eq!(shown.get("DICOM", "Study"), Some("Kopf-CT 頭部 Müller"));
        assert_eq!(
            shown.get(TAG_LISTING, "(0008,1030) StudyDescription"),
            Some("Kopf-CT 頭部 Müller")
        );
    }

    #[test]
    fn tag_numbers_are_named_from_the_dictionary() {
        assert_eq!(tag_name("0010,0020"), Some("PatientID"));
        assert_eq!(tag_name("7FE0,0010"), Some("PixelData"));
        assert_eq!(tag_name("not a tag"), None);
    }

    #[test]
    fn lossless_jpeg_ls_gives_back_the_exact_values() {
        let values = [0, 1, 4095, 2048, 100, 7, 3000, 12];
        let jls = jpeg_ls(4, 2, 12, 1, 0, &u16_bytes(&values));
        let synth = Synth::new(CT_IMAGE, 4, 2)
            .pixels_u16(&values, 12)
            .encapsulated(JPEG_LS_LOSSLESS, vec![jls]);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert!(matches!(&img.data, PixelData::U16(v) if v == &values));
    }

    #[test]
    fn near_lossless_jpeg_ls_stays_within_the_allowed_error() {
        let values: Vec<u16> = (0..64).map(|i| (i * 37 % 251) as u16).collect();
        let jls = jpeg_ls(
            8,
            8,
            8,
            1,
            3,
            &values.iter().map(|&v| v as u8).collect::<Vec<_>>(),
        );
        let synth = Synth::new(XA_IMAGE, 8, 8)
            .frames_u8(&[&[0; 64]])
            .encapsulated(JPEG_LS_NEAR_LOSSLESS, vec![jls]);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        let PixelData::U8(got) = &img.data else {
            panic!("expected U8")
        };
        for (g, v) in got.iter().zip(&values) {
            assert!((*g as i32 - *v as i32).abs() <= 3, "{g} vs {v}");
        }
    }

    #[test]
    fn jpeg_ls_color_and_multiframe_are_read_per_frame() {
        let first = [1, 2, 3, 4, 5, 6];
        let second = [60, 50, 40, 30, 20, 10];
        let frames = [first, second].map(|f| jpeg_ls(2, 1, 8, 3, 0, &f)).to_vec();
        let synth = Synth::rgb(2, 1, &[0; 6], false).encapsulated(JPEG_LS_LOSSLESS, frames);
        let bytes = synth.bytes();
        assert_eq!(DicomPlugin.frame_count(&bytes).unwrap(), 2);
        let img = DicomPlugin.decode_frame(&bytes, 1).unwrap();
        assert_eq!(img.color, ColorModel::Rgb);
        assert!(matches!(&img.data, PixelData::U8(v) if v == &second));
    }

    #[test]
    fn lossless_jpeg_2000_gives_back_the_exact_values() {
        let j2k = include_bytes!("../testdata/gray16.j2k").to_vec();
        let synth = Synth::new(CT_IMAGE, 4, 2)
            .pixels_u16(&J2K_GRAY16, 16)
            .encapsulated(JPEG_2000_LOSSLESS, vec![j2k]);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert!(matches!(&img.data, PixelData::U16(v) if v == &J2K_GRAY16));
    }

    #[test]
    fn jpeg_2000_color_written_as_ybr_rct_is_shown_as_rgb() {
        let j2k = include_bytes!("../testdata/rgb8.j2k").to_vec();
        let synth = Synth::rgb(4, 2, &J2K_RGB8, false)
            .photometric("YBR_RCT")
            .encapsulated(JPEG_2000_LOSSLESS, vec![j2k]);
        let img = DicomPlugin.decode(&synth.bytes()).unwrap();
        assert!(matches!(&img.data, PixelData::U8(v) if v == &J2K_RGB8));
    }

    #[test]
    fn broken_file_is_an_error_not_a_panic() {
        let mut bytes = Synth::ct(2, 2).bytes();
        bytes.truncate(200);
        assert!(DicomPlugin.decode(&bytes).is_err());
    }
}
