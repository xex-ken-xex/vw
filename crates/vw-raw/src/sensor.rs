//! 現像する前のセンサーの値。白飛び・黒つぶれと露出の余裕を、カメラが作った JPEG や
//! vw の現像の明るさ合わせに左右されずに確かめるため。

use rawler::decoders::RawDecodeParams;
use rawler::rawimage::{RawImage, RawImageData, RawPhotometricInterpretation};
use rawler::rawsource::RawSource;
use vw_core::DecodeError;

use crate::{decoder_for, invalid};

/// 値の色。ベイヤーの色の並び（CFA）の色か、画素ごとの成分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorColor {
    Red,
    Green,
    Blue,
    /// 色のないセンサー、または R・G・B 以外の色（CMY など）。
    Other,
}

/// ヒストグラムの横軸の左端（EV、白レベルが 0）。これより暗い値と黒レベル以下の値は左端の区間に入れる。
/// 14 EV にするのは、今のカメラのセンサーの幅（ダイナミックレンジ）がおおよそ 12〜15 EV のため。
pub const DARKEST_EV: f64 = -14.0;

/// 1 色分のセンサーの値の数え。値は黒レベルを 0、白レベルを 1 とした割合にし、
/// 白レベルから何 EV 下か（[`DARKEST_EV`]〜0）で区間に分ける。
/// 割合のまま等分しないのは、光の量に比例する値では暗い所がほとんど左端の数区間に詰まり、
/// 露出の余裕や暗部の様子が読めないため。
#[derive(Debug, Clone, PartialEq)]
pub struct SensorChannel {
    pub color: SensorColor,
    pub counts: Vec<u32>,
    /// 白レベル以上で、光の量が分からなくなった値の数。
    pub clipped: u64,
    /// 黒レベル以下の値の数。
    pub at_black: u64,
    pub total: u64,
    /// いちばん明るい値（白レベルに対する割合）。
    pub max: f64,
}

impl SensorChannel {
    /// 白レベルまでの余裕（EV）。白飛びしていれば 0。
    pub fn headroom_ev(&self) -> f64 {
        if self.max <= 0.0 {
            f64::INFINITY
        } else {
            (1.0 / self.max).log2().max(0.0)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SensorHistogram {
    pub channels: Vec<SensorChannel>,
}

/// RAW ファイルの、写る範囲（クロップ）のセンサーの値を色ごとに数える。
pub fn sensor_histogram(data: &[u8], bins: usize) -> Result<SensorHistogram, DecodeError> {
    let source = RawSource::new_from_slice(data);
    let decoder = decoder_for(&source)?;
    let raw = decoder
        .raw_image(&source, &RawDecodeParams::default(), false)
        .map_err(invalid)?;
    let values: Vec<f32> = match &raw.data {
        RawImageData::Integer(v) => v.iter().map(|&x| f32::from(x)).collect(),
        RawImageData::Float(v) => v.clone(),
    };
    let area = raw
        .crop_area
        .or(raw.active_area)
        .map_or((0, 0, raw.width, raw.height), |r| {
            (r.p.x, r.p.y, r.d.w, r.d.h)
        });
    let sensor = Sensor {
        width: raw.width,
        height: raw.height,
        cpp: raw.cpp.max(1),
        values: &values,
        black: Black {
            levels: raw.blacklevel.as_vec(),
            width: raw.blacklevel.width.max(1),
            height: raw.blacklevel.height.max(1),
            cpp: raw.blacklevel.cpp.max(1),
        },
        white: raw.whitelevel.0.clone(),
        colors: Colors::of(&raw),
        area,
    };
    Ok(sensor.count(bins.max(1)))
}

/// 数えるのに要る、センサーの値と黒・白のレベル。rawler の `RawImage` から必要な分だけ抜き出す。
struct Sensor<'a> {
    width: usize,
    height: usize,
    cpp: usize,
    values: &'a [f32],
    black: Black,
    white: Vec<u32>,
    colors: Colors,
    /// 写る範囲（x, y, 幅, 高さ）。
    area: (usize, usize, usize, usize),
}

/// 黒レベル。`width`×`height` の升の繰り返しで、升ごとに `cpp` 個。
struct Black {
    levels: Vec<f32>,
    width: usize,
    height: usize,
    cpp: usize,
}

impl Black {
    fn at(&self, row: usize, col: usize, comp: usize) -> f64 {
        let i = ((row % self.height) * self.width + col % self.width) * self.cpp
            + comp.min(self.cpp - 1);
        self.levels
            .get(i)
            .or(self.levels.first())
            .map_or(0.0, |&b| f64::from(b))
    }
}

impl Sensor<'_> {
    fn count(&self, bins: usize) -> SensorHistogram {
        let mut channels: Vec<SensorChannel> = self
            .colors
            .distinct()
            .into_iter()
            .map(|color| SensorChannel {
                color,
                counts: vec![0; bins],
                clipped: 0,
                at_black: 0,
                total: 0,
                max: 0.0,
            })
            .collect();
        let slots: Vec<usize> = self
            .colors
            .all()
            .iter()
            .map(|&c| {
                channels
                    .iter()
                    .position(|ch| ch.color == c)
                    .expect("every color has a channel")
            })
            .collect();
        let (x0, y0, w, h) = self.area;
        for row in y0..(y0 + h).min(self.height) {
            for col in x0..(x0 + w).min(self.width) {
                for comp in 0..self.cpp {
                    let v = f64::from(self.values[(row * self.width + col) * self.cpp + comp]);
                    let black = self.black.at(row, col, comp);
                    let white = self
                        .white
                        .get(comp)
                        .or(self.white.first())
                        .map_or(f64::from(u16::MAX), |&w| f64::from(w));
                    let ch = &mut channels[slots[self.colors.index(row, col, comp)]];
                    ch.total += 1;
                    if v >= white {
                        ch.clipped += 1;
                    }
                    if v <= black {
                        ch.at_black += 1;
                    }
                    let x = ((v - black) / (white - black).max(1.0)).clamp(0.0, 1.0);
                    ch.max = ch.max.max(x);
                    ch.counts[bin_of(x, bins)] += 1;
                }
            }
        }
        SensorHistogram { channels }
    }
}

/// 白レベルに対する割合 `x` が入る区間。
fn bin_of(x: f64, bins: usize) -> usize {
    if x <= 0.0 {
        return 0;
    }
    let t = (x.log2() - DARKEST_EV) / -DARKEST_EV;
    ((t * bins as f64).max(0.0) as usize).min(bins - 1)
}

/// 画素の位置と成分から色を引く表。
enum Colors {
    /// ベイヤーなどの色の並び。`pattern[row % h][col % w]`。
    Cfa { pattern: Vec<Vec<SensorColor>> },
    /// 画素ごとに成分を持つ（LinearRaw など）。
    Components(Vec<SensorColor>),
}

impl Colors {
    fn of(raw: &RawImage) -> Self {
        match &raw.photometric {
            RawPhotometricInterpretation::Cfa(config) if raw.cpp == 1 && config.cfa.width > 0 => {
                let cfa = &config.cfa;
                Colors::Cfa {
                    pattern: (0..cfa.height)
                        .map(|r| {
                            (0..cfa.width)
                                .map(|c| cfa_color(cfa.color_at(r, c)))
                                .collect()
                        })
                        .collect(),
                }
            }
            _ if raw.cpp >= 3 => Colors::Components(
                (0..raw.cpp)
                    .map(|c| match c {
                        0 => SensorColor::Red,
                        1 => SensorColor::Green,
                        2 => SensorColor::Blue,
                        _ => SensorColor::Other,
                    })
                    .collect(),
            ),
            _ => Colors::Components(vec![SensorColor::Other; raw.cpp.max(1)]),
        }
    }

    /// 表に出てくる色。出てくる順ではなく R・G・B・その他の順。
    fn distinct(&self) -> Vec<SensorColor> {
        let all = self.all();
        [
            SensorColor::Red,
            SensorColor::Green,
            SensorColor::Blue,
            SensorColor::Other,
        ]
        .into_iter()
        .filter(|c| all.contains(c))
        .collect()
    }

    /// 表のすべての升の色（[`Colors::index`] の番号の順）。
    fn all(&self) -> Vec<SensorColor> {
        match self {
            Colors::Cfa { pattern } => pattern.iter().flatten().copied().collect(),
            Colors::Components(c) => c.clone(),
        }
    }

    fn index(&self, row: usize, col: usize, comp: usize) -> usize {
        match self {
            Colors::Cfa { pattern } => {
                let (h, w) = (pattern.len(), pattern[0].len());
                (row % h) * w + col % w
            }
            Colors::Components(_) => comp,
        }
    }
}

/// rawler の色の番号（`CFAColor`）。
fn cfa_color(n: usize) -> SensorColor {
    match n {
        0 => SensorColor::Red,
        1 | 7 => SensorColor::Green,
        2 => SensorColor::Blue,
        _ => SensorColor::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2 の RGGB で、黒 100、白 1100 の小さなセンサー。
    fn count_bayer(values: &[f32]) -> SensorHistogram {
        use SensorColor::{Blue, Green, Red};
        Sensor {
            width: 2,
            height: 2,
            cpp: 1,
            values,
            black: Black {
                levels: vec![100.0],
                width: 1,
                height: 1,
                cpp: 1,
            },
            white: vec![1100],
            colors: Colors::Cfa {
                pattern: vec![vec![Red, Green], vec![Green, Blue]],
            },
            area: (0, 0, 2, 2),
        }
        .count(4)
    }

    fn channel(h: &SensorHistogram, c: SensorColor) -> &SensorChannel {
        h.channels.iter().find(|ch| ch.color == c).unwrap()
    }

    #[test]
    fn counts_each_bayer_color_separately() {
        let h = count_bayer(&[1100.0, 600.0, 600.0, 100.0]);
        assert_eq!(h.channels.len(), 3);
        assert_eq!(channel(&h, SensorColor::Green).total, 2);
        assert_eq!(channel(&h, SensorColor::Red).total, 1);
    }

    #[test]
    fn values_at_the_white_level_are_clipped() {
        let h = count_bayer(&[1100.0, 600.0, 600.0, 100.0]);
        let red = channel(&h, SensorColor::Red);
        assert_eq!(red.clipped, 1);
        assert_eq!(red.counts[3], 1, "white is the rightmost bin");
        assert_eq!(red.headroom_ev(), 0.0);
    }

    #[test]
    fn values_at_the_black_level_are_counted_as_black() {
        let h = count_bayer(&[1100.0, 600.0, 600.0, 100.0]);
        let blue = channel(&h, SensorColor::Blue);
        assert_eq!(blue.at_black, 1);
        assert_eq!(blue.counts[0], 1);
    }

    #[test]
    fn headroom_is_the_stops_left_below_white() {
        // 黒 100 から白 1100 のちょうど半分（600）。白まで 1 EV。
        let h = count_bayer(&[100.0, 600.0, 600.0, 100.0]);
        let green = channel(&h, SensorColor::Green);
        assert!((green.headroom_ev() - 1.0).abs() < 1e-9);
        // -14〜0 EV を 4 区間（3.5 EV ずつ）に分けると、-1 EV は右端の区間。
        assert_eq!(green.counts[3], 2);
    }

    #[test]
    fn bins_are_spaced_by_stops_below_white() {
        // 14 区間なら 1 区間が 1 EV。白の半分（-1 EV）は右から 2 番目、1/4（-2 EV）は 3 番目。
        assert_eq!(bin_of(1.0, 14), 13);
        assert_eq!(bin_of(0.5, 14), 13);
        assert_eq!(bin_of(0.49, 14), 12);
        assert_eq!(bin_of(0.25, 14), 12);
        assert_eq!(bin_of(0.0, 14), 0);
        assert_eq!(bin_of(1e-9, 14), 0);
    }

    #[test]
    fn only_the_cropped_area_is_counted() {
        use SensorColor::{Blue, Green, Red};
        let values = [1100.0, 600.0, 600.0, 100.0];
        let h = Sensor {
            width: 2,
            height: 2,
            cpp: 1,
            values: &values,
            black: Black {
                levels: vec![100.0],
                width: 1,
                height: 1,
                cpp: 1,
            },
            white: vec![1100],
            colors: Colors::Cfa {
                pattern: vec![vec![Red, Green], vec![Green, Blue]],
            },
            area: (1, 1, 1, 1),
        }
        .count(4);
        assert_eq!(h.channels.iter().map(|c| c.total).sum::<u64>(), 1);
        assert_eq!(channel(&h, SensorColor::Blue).total, 1);
    }
}
