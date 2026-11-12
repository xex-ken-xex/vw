//! 分光キューブのモーメント図。スペクトルの軸に沿って、画素ごとに値をまとめた 1 枚の画像にする。
//!
//! - 0 次: 値 × チャンネルの幅（速度）の和。線の強さ（単位は `BUNIT`・km/s）
//! - 1 次: 値で重みを付けた速度の平均。視線速度（km/s）
//! - 2 次: 値で重みを付けた速度の広がり（標準偏差、km/s）
//!
//! 作った図は FITS のバイト列で返す。元の空の座標（WCS）とビームを引き継ぐので、読み込めば
//! ほかの FITS と同じように星図や十字の空の座標が使え、そのまま保存もできる。

use vw_core::spectrum::{Axis, AxisKind};
use vw_core::{ColorModel, DecodeError, PixelData, VwImage};

use crate::header::{self, Hdu};

/// 次数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Order {
    Zero,
    One,
    Two,
}

impl Order {
    pub const ALL: [Order; 3] = [Order::Zero, Order::One, Order::Two];

    pub fn number(self) -> u8 {
        match self {
            Order::Zero => 0,
            Order::One => 1,
            Order::Two => 2,
        }
    }

    pub fn from_number(n: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.number() == n)
    }
}

/// 作り方。
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub order: Order,
    /// 使うチャンネル（0 から、両端を含む）。`None` は全部。
    pub channels: Option<(usize, usize)>,
    /// しきい値（雑音の何倍か）。中央値 + これ × 雑音 より大きい値だけを足す。0 以下は全部の値を足す。
    pub clip: f64,
    /// ほかの軸（偏光など）の位置を決める面。キューブの面の番号（0 から）。
    pub frame: usize,
    /// 見る HDU（`None` はデータのある最初の画像）。
    pub hdu: Option<usize>,
}

/// キューブのスペクトルの軸。モーメント図の窓で、チャンネルの範囲を選ぶのに使う。
#[derive(Debug, Clone, PartialEq)]
pub struct Channels {
    pub count: usize,
    pub axis: Axis,
}

/// ファイルのキューブのスペクトルの軸。データは読まず、ヘッダだけを読む。キューブでなければ `None`。
pub fn channels_of_file(path: &std::path::Path, hdu: Option<usize>) -> Option<Channels> {
    let mut hdus = crate::sky::headers_of_file_all(path).ok()?;
    // `-TAB` の軸の値は表の HDU のデータにあるので、そのときだけファイルを全部読む。
    let lookup = hdus.iter().any(|h| {
        h.cards.iter().any(|c| {
            c.key.starts_with("CTYPE") && c.value.trim().to_ascii_uppercase().ends_with("-TAB")
        })
    });
    if lookup {
        let data = std::fs::read(path).ok()?;
        hdus = header::hdus(&crate::gunzip(&data).ok()?).ok()?;
    }
    let image = crate::find_image(&hdus, hdu).ok()?;
    let found = crate::cube::channels(image.hdu, &image.extra_axes)?;
    Some(Channels {
        count: found.count,
        axis: spectral_axis(image.hdu, found.index + 3, found.count),
    })
}

/// `data`（ファイルの中身、gzip のままでよい）のキューブから、`options` のモーメント図を作り、
/// FITS のバイト列で返す。
pub fn make(data: &[u8], options: &Options) -> Result<Vec<u8>, DecodeError> {
    let data = crate::gunzip(data)?;
    let hdus = header::hdus(&data)?;
    let image = crate::find_image(&hdus, options.hdu)?;
    let channels = crate::cube::channels(image.hdu, &image.extra_axes)
        .ok_or_else(|| DecodeError::Invalid("FITS image has no spectral axis".into()))?;
    let (from, to) = options.channels.unwrap_or((0, channels.count - 1));
    if from > to || to >= channels.count {
        return Err(DecodeError::Invalid(format!(
            "channels {}-{} are outside 1-{}",
            from + 1,
            to + 1,
            channels.count
        )));
    }
    let frame = options.frame.min(image.planes - 1);
    let here = (frame / channels.stride) % channels.count;
    let first = frame - here * channels.stride;
    let plane_of = |c: usize| first + c * channels.stride;
    let read = |c: usize| -> Result<Vec<f32>, DecodeError> {
        let stored = if image.compressed {
            crate::compressed_plane(&data, &image, plane_of(c))?
        } else {
            crate::plain_plane(&data, &image, plane_of(c))?
        };
        Ok(match crate::to_pixels(stored, image.hdu) {
            PixelData::U8(v) => v.into_iter().map(f32::from).collect(),
            PixelData::U16(v) => v.into_iter().map(f32::from).collect(),
            PixelData::F32(v) => v,
        })
    };
    let axis = spectral_axis(image.hdu, channels.index + 3, channels.count);
    let (speeds, speed_unit) = speeds(&axis);
    let widths = widths(&speeds);
    let threshold = if options.clip > 0.0 {
        let (median, sigma) = noise(&read, from, to)?;
        median + options.clip * sigma
    } else {
        f64::NEG_INFINITY
    };
    let pixels = image.plane_pixels();
    let mut sum = vec![0f64; pixels];
    let mut weighted = vec![0f64; pixels];
    let mut squared = vec![0f64; pixels];
    let mut area = vec![0f64; pixels];
    let mut seen = vec![false; pixels];
    for c in from..=to {
        let plane = read(c)?;
        let (u, du) = (speeds[c], widths[c]);
        for (i, &v) in plane.iter().enumerate() {
            if !v.is_finite() {
                continue;
            }
            seen[i] = true;
            let v = f64::from(v);
            if v <= threshold {
                continue;
            }
            sum[i] += v;
            weighted[i] += v * u;
            squared[i] += v * u * u;
            area[i] += v * du;
        }
    }
    let values: Vec<f32> = (0..pixels)
        .map(|i| {
            if !seen[i] {
                return f32::NAN;
            }
            let mean = weighted[i] / sum[i];
            let v = match options.order {
                Order::Zero => area[i],
                _ if sum[i] <= 0.0 => f64::NAN,
                Order::One => mean,
                Order::Two => (squared[i] / sum[i] - mean * mean).max(0.0).sqrt(),
            };
            v as f32
        })
        .collect();
    let img = VwImage::new(
        image.width as u32,
        image.height as u32,
        ColorModel::Gray,
        PixelData::F32(values),
    )
    .map_err(DecodeError::Invalid)?;
    let lookup = |key: &str| {
        image
            .hdu
            .get(key)
            .or_else(|| hdus[0].get(key))
            .map(|v| v.trim().to_owned())
    };
    let mut cards: Vec<(String, String)> = KEPT
        .iter()
        .filter_map(|k| Some(((*k).to_owned(), lookup(k)?)))
        .chain(
            image
                .hdu
                .cards
                .iter()
                .filter(|c| c.key.starts_with("PV1_") || c.key.starts_with("PV2_"))
                .map(|c| (c.key.clone(), c.value.trim().to_owned())),
        )
        .collect();
    let value_unit = lookup("BUNIT").unwrap_or_default();
    let unit = match options.order {
        Order::Zero if value_unit.is_empty() => speed_unit.clone(),
        Order::Zero => format!("{value_unit}.{speed_unit}"),
        _ => speed_unit.clone(),
    };
    if !unit.is_empty() {
        cards.push(("BUNIT".into(), unit));
    }
    cards.push(("BTYPE".into(), format!("Moment {}", options.order.number())));
    let clip = if options.clip > 0.0 {
        format!("values above the median + {} x noise", options.clip)
    } else {
        "all values".to_owned()
    };
    let history = vec![
        format!(
            "vw: moment {} of channels {}-{} of {}, {clip}",
            options.order.number(),
            from + 1,
            to + 1,
            channels.count
        ),
        format!(
            "vw: velocities in {speed_unit} from axis {}",
            channels.index + 3
        ),
    ];
    crate::write::encode_rows(&img, &cards, &history, lookup("ROWORDER").as_deref())
        .map_err(DecodeError::Invalid)
}

/// 引き継ぐカード。空の座標（軸 1 と 2）、ビーム、観測の名前。
const KEPT: &[&str] = &[
    "CTYPE1", "CTYPE2", "CRVAL1", "CRVAL2", "CRPIX1", "CRPIX2", "CDELT1", "CDELT2", "CUNIT1",
    "CUNIT2", "CROTA1", "CROTA2", "CD1_1", "CD1_2", "CD2_1", "CD2_2", "PC1_1", "PC1_2", "PC2_1",
    "PC2_2", "LONPOLE", "LATPOLE", "RADESYS", "RADECSYS", "EQUINOX", "EPOCH", "DATE-OBS",
    "MJD-OBS", "OBJECT", "TELESCOP", "INSTRUME", "BMAJ", "BMIN", "BPA", "RESTFRQ", "RESTFREQ",
    "RESTWAV", "SPECSYS",
];

/// 軸 `number` のスペクトルの軸。読めない軸は線形の軸にする。
fn spectral_axis(hdu: &Hdu, number: usize, count: usize) -> Axis {
    match crate::spectral::read(hdu, number) {
        Some(spectral) => spectral.axis(count),
        None => crate::cube::linear_axis(hdu, number, count),
    }
}

/// チャンネルごとの速度（km/s）。静止周波数などがなく速度にできない軸は、軸の値と単位のまま。
fn speeds(axis: &Axis) -> (Vec<f64>, String) {
    let values = axis.values.iter();
    if axis.kind == AxisKind::Velocity {
        return (values.map(|v| v / 1000.0).collect(), "km/s".into());
    }
    let velocities: Option<Vec<f64>> = values.clone().map(|&v| axis.velocity(v)).collect();
    if let Some(v) = velocities {
        return (v.into_iter().map(|v| v / 1000.0).collect(), "km/s".into());
    }
    let unit = match &axis.kind {
        AxisKind::Frequency => "Hz".to_owned(),
        AxisKind::Wavelength => "m".to_owned(),
        AxisKind::Other { unit, .. } => unit.clone(),
        AxisKind::Velocity => unreachable!("handled above"),
    };
    (values.copied().collect(), unit)
}

/// チャンネルごとの幅（隣との間の半分ずつ、向きは問わない）。チャンネルが 1 つなら 1。
fn widths(speeds: &[f64]) -> Vec<f64> {
    let n = speeds.len();
    if n < 2 {
        return vec![1.0; n];
    }
    (0..n)
        .map(|c| {
            let a = speeds[c.saturating_sub(1)];
            let b = speeds[(c + 1).min(n - 1)];
            let span = (c + 1).min(n - 1) - c.saturating_sub(1);
            ((b - a) / span as f64).abs()
        })
        .collect()
}

/// 選んだチャンネルの値の、中央値と雑音（中央値からの外れの中央値 × 1.4826）。間引いて数える。
fn noise(
    read: &dyn Fn(usize) -> Result<Vec<f32>, DecodeError>,
    from: usize,
    to: usize,
) -> Result<(f64, f64), DecodeError> {
    const PLANES: usize = 16;
    const SAMPLES: usize = 1_000_000;
    let n = to - from + 1;
    let count = n.min(PLANES);
    let mut values: Vec<f32> = Vec::new();
    for k in 0..count {
        let plane = read(from + k * n / count)?;
        let step = (plane.len() * count).div_ceil(SAMPLES).max(1);
        values.extend(plane.iter().step_by(step).filter(|v| v.is_finite()));
    }
    if values.is_empty() {
        return Ok((0.0, 0.0));
    }
    values.sort_by(f32::total_cmp);
    let median = f64::from(values[values.len() / 2]);
    let mut d: Vec<f64> = values
        .iter()
        .map(|&v| (f64::from(v) - median).abs())
        .collect();
    d.sort_by(f64::total_cmp);
    Ok((median, 1.4826 * d[d.len() / 2]))
}
