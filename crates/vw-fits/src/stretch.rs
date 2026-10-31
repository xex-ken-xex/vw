//! 最初に表示する黒と白（[`vw_core::VwImage::window`]）を画素の分布から決める。
//!
//! 天体の画像は明るい星が数個あるだけで背景はごく暗いので、最小値〜最大値で伸ばすと
//! ほとんど真っ黒になる。

use vw_core::PixelData;

/// 黒と白の決め方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stretch {
    MinMax,
    /// 値の小さいほうから `low`％ の値を黒、`high`％ の値を白にする。
    #[default]
    Percentile,
    /// IRAF の zscale（astropy の `ZScaleInterval` と同じ手順）。背景の付近の分布から決める。
    ZScale,
    /// 0 をはさんで上下同じ幅。値の絶対値の `high`％ の値を白、その負を黒にする。
    /// 電波干渉計の画像の負の値や、速度の地図を、0 を真ん中にして見るため。
    Symmetric,
}

impl Stretch {
    /// 英語の名前（設定の画面と同じ文言）。
    pub fn title(self) -> &'static str {
        match self {
            Stretch::MinMax => "Minimum to maximum",
            Stretch::Percentile => "Percentiles",
            Stretch::ZScale => "zscale",
            Stretch::Symmetric => "Symmetric around zero",
        }
    }
}

/// 分布を見るのに使う値の数の上限。全画素を並べ替えると大きな画像で遅いので、間引いて使う。
const PERCENTILE_SAMPLES: usize = 1_000_000;
/// zscale が見る値の数（astropy の既定）。
const ZSCALE_SAMPLES: usize = 1000;

/// 欠損（NaN）を除いた値から黒と白を決める。値がなければ `None`。
pub fn window(data: &PixelData, stretch: Stretch, low: f32, high: f32) -> Option<(f32, f32)> {
    let (lo, hi) = match stretch {
        Stretch::MinMax => min_max(data)?,
        Stretch::Percentile => match data {
            PixelData::U8(v) => {
                histogram_percentile(v.iter().map(|&s| s as usize), 256, low, high)?
            }
            PixelData::U16(v) => {
                histogram_percentile(v.iter().map(|&s| s as usize), 65536, low, high)?
            }
            PixelData::F32(_) => {
                let sorted = sorted_sample(data, PERCENTILE_SAMPLES);
                (at_percent(&sorted, low)?, at_percent(&sorted, high)?)
            }
        },
        Stretch::ZScale => zscale(&sorted_sample(data, ZSCALE_SAMPLES))?,
        Stretch::Symmetric => {
            let mut magnitudes = sorted_sample(data, PERCENTILE_SAMPLES);
            magnitudes.iter_mut().for_each(|v| *v = v.abs());
            magnitudes.sort_by(f32::total_cmp);
            let a = at_percent(&magnitudes, high)?;
            (-a, a)
        }
    };
    // 黒と白が同じだと階調が出ない（一面同じ値の画像）。最小〜最大に戻し、それでも同じなら幅 1 にする。
    if hi > lo {
        return Some((lo, hi));
    }
    let (min, max) = min_max(data)?;
    Some(if max > min {
        (min, max)
    } else {
        (min, min + 1.0)
    })
}

fn values(data: &PixelData) -> Box<dyn Iterator<Item = f32> + '_> {
    match data {
        PixelData::U8(v) => Box::new(v.iter().map(|&s| s as f32)),
        PixelData::U16(v) => Box::new(v.iter().map(|&s| s as f32)),
        PixelData::F32(v) => Box::new(v.iter().copied().filter(|s| s.is_finite())),
    }
}

fn min_max(data: &PixelData) -> Option<(f32, f32)> {
    values(data).fold(None, |acc, v| match acc {
        None => Some((v, v)),
        Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
    })
}

/// 整数の画像は値ごとの数を数えて、全画素の正確なパーセンタイルを出す。
fn histogram_percentile(
    samples: impl Iterator<Item = usize>,
    bins: usize,
    low: f32,
    high: f32,
) -> Option<(f32, f32)> {
    let mut counts = vec![0u64; bins];
    let mut total = 0u64;
    for s in samples {
        counts[s] += 1;
        total += 1;
    }
    if total == 0 {
        return None;
    }
    let find = |percent: f32| {
        let rank = ((percent.clamp(0.0, 100.0) as f64 / 100.0) * (total - 1) as f64).round() as u64;
        let mut seen = 0;
        for (value, &n) in counts.iter().enumerate() {
            seen += n;
            if seen > rank {
                return value as f32;
            }
        }
        (bins - 1) as f32
    };
    Some((find(low), find(high)))
}

/// 欠損を除いた値を、最大 `n` 個に等間隔で間引いて並べたもの。
fn sorted_sample(data: &PixelData, n: usize) -> Vec<f32> {
    let total = data.len();
    let step = total.div_ceil(n.max(1)).max(1);
    let mut sample: Vec<f32> = match data {
        PixelData::U8(v) => v.iter().step_by(step).map(|&s| s as f32).collect(),
        PixelData::U16(v) => v.iter().step_by(step).map(|&s| s as f32).collect(),
        PixelData::F32(v) => v
            .iter()
            .step_by(step)
            .copied()
            .filter(|s| s.is_finite())
            .collect(),
    };
    sample.sort_by(f32::total_cmp);
    sample
}

fn at_percent(sorted: &[f32], percent: f32) -> Option<f32> {
    let last = sorted.len().checked_sub(1)?;
    let rank = (percent.clamp(0.0, 100.0) / 100.0 * last as f32).round() as usize;
    sorted.get(rank.min(last)).copied()
}

/// astropy の `ZScaleInterval`（contrast 0.25、krej 2.5、max_reject 0.5、min_npixels 5、
/// max_iterations 5）を並べた値に当てる。
fn zscale(sorted: &[f32]) -> Option<(f32, f32)> {
    const CONTRAST: f64 = 0.25;
    const KREJ: f64 = 2.5;
    const MAX_REJECT: f64 = 0.5;
    const MIN_NPIXELS: usize = 5;
    const MAX_ITERATIONS: usize = 5;

    let npix = sorted.len();
    let (&first, &last) = (sorted.first()?, sorted.last()?);
    let (mut vmin, mut vmax) = (first as f64, last as f64);
    let minpix = MIN_NPIXELS.max((npix as f64 * MAX_REJECT) as usize);
    let ngrow = 1usize.max((npix as f64 * 0.01) as usize);
    let y: Vec<f64> = sorted.iter().map(|&v| v as f64).collect();
    let mut bad = vec![false; npix];
    let mut ngood = npix;
    let mut last_ngood = npix + 1;
    let mut fit = (0.0, 0.0);
    for _ in 0..MAX_ITERATIONS {
        if ngood >= last_ngood || ngood < minpix {
            break;
        }
        fit = line_fit(&y, &bad)?;
        let flat: Vec<f64> = y
            .iter()
            .enumerate()
            .map(|(x, v)| v - (fit.0 * x as f64 + fit.1))
            .collect();
        let good: Vec<f64> = flat
            .iter()
            .zip(&bad)
            .filter(|(_, b)| !**b)
            .map(|(f, _)| *f)
            .collect();
        let mean = good.iter().sum::<f64>() / good.len() as f64;
        let std = (good.iter().map(|f| (f - mean).powi(2)).sum::<f64>() / good.len() as f64).sqrt();
        let threshold = KREJ * std;
        let rejected: Vec<bool> = flat
            .iter()
            .zip(&bad)
            .map(|(f, &b)| b || *f < -threshold || *f > threshold)
            .collect();
        bad = grow(&rejected, ngrow);
        last_ngood = ngood;
        ngood = bad.iter().filter(|b| !**b).count();
    }
    if ngood >= minpix {
        let slope = fit.0 / CONTRAST;
        let center = (npix - 1) / 2;
        let median = if npix % 2 == 1 {
            y[npix / 2]
        } else {
            f64::midpoint(y[npix / 2 - 1], y[npix / 2])
        };
        vmin = vmin.max(median - (center as f64 - 1.0) * slope);
        vmax = vmax.min(median + (npix - center) as f64 * slope);
    }
    Some((vmin as f32, vmax as f32))
}

/// 外れていない点に当てた直線 `(傾き, 切片)`。x は並びの位置。
fn line_fit(y: &[f64], bad: &[bool]) -> Option<(f64, f64)> {
    let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (x, (&v, &b)) in y.iter().zip(bad).enumerate() {
        if b {
            continue;
        }
        let x = x as f64;
        n += 1.0;
        sx += x;
        sy += v;
        sxx += x * x;
        sxy += x * v;
    }
    let d = n * sxx - sx * sx;
    if n < 2.0 || d == 0.0 {
        return None;
    }
    let slope = (n * sxy - sx * sy) / d;
    Some((slope, (sy - slope * sx) / n))
}

/// 外れた点の前後 `ngrow` 個も外れにする（astropy の `convolve(..., mode='same')` と同じ幅）。
fn grow(bad: &[bool], ngrow: usize) -> Vec<bool> {
    let before = (ngrow - 1) / 2;
    let after = ngrow - 1 - before;
    let mut out = vec![false; bad.len()];
    for (i, _) in bad.iter().enumerate().filter(|(_, b)| **b) {
        let from = i.saturating_sub(after);
        let to = (i + before).min(bad.len() - 1);
        out[from..=to].iter_mut().for_each(|o| *o = true);
    }
    out
}
