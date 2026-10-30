//! 天体写真の非線形の伸縮の曲線。一般化双曲線ストレッチ（GHS）と、MTF による自動ストレッチ。
//!
//! どちらも 0〜1 の値にかかる。伸縮ノードの後ろ（黒と白で 0〜1 にした線形の値）に置く。
//!
//! GHS は Cranfield と Shelley が公開した式の説明（ghsastro.co.uk の GeneralizedHyperbolicStretch の
//! 説明の頁）だけから書いた。GPL-3.0 で配られている実装は、ライセンスを混ぜないため見ていない。

/// 曲線を表に引いて当てるときの、表の大きさ。画素ごとに累乗を計算するより速く、
/// 16bit の階調より細かいので、間を線形に補うと差は見えない。
const TABLE: usize = 65536;

/// GHS の値。`stretch` は伸ばす強さ ln(D + 1)、`local` は局所の強さ b、
/// `symmetry` は傾きがいちばん大きい所（SP）、`shadows`・`highlights` はそこから外を直線にする所（LP・HP）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ghs {
    pub stretch: f32,
    pub local: f32,
    pub symmetry: f32,
    pub shadows: f32,
    pub highlights: f32,
}

impl Ghs {
    /// 0〜1 の `x` を写す。範囲の外は端に寄せる。
    pub fn map(&self, x: f32) -> f32 {
        let d = f64::from(self.stretch.max(0.0)).exp() - 1.0;
        if d < 1e-9 {
            return x.clamp(0.0, 1.0);
        }
        let sp = f64::from(self.symmetry.clamp(0.0, 1.0));
        let lp = f64::from(self.shadows.clamp(0.0, 1.0)).min(sp);
        let hp = f64::from(self.highlights.clamp(0.0, 1.0)).max(sp);
        let base = Base {
            d,
            b: f64::from(self.local),
        };
        // 説明の T1〜T4: SP より上は T(x − SP)、下は −T(SP − x)、LP より下と HP より上は接線。
        let t = |x: f64| -> f64 {
            if x < lp {
                base.slope(sp - lp) * (x - lp) - base.value(sp - lp)
            } else if x < sp {
                -base.value(sp - x)
            } else if x < hp {
                base.value(x - sp)
            } else {
                base.slope(hp - sp) * (x - hp) + base.value(hp - sp)
            }
        };
        let (low, high) = (t(0.0), t(1.0));
        let x = f64::from(x.clamp(0.0, 1.0));
        ((t(x) - low) / (high - low)) as f32
    }

    /// 画像の全チャンネルに当てる。
    pub fn apply(&self, data: &mut [f32]) {
        let table: Vec<f32> = (0..TABLE)
            .map(|i| self.map(i as f32 / (TABLE - 1) as f32))
            .collect();
        for v in data {
            *v = lookup(&table, *v);
        }
    }
}

/// GHS の基の変換 T(x) と、その傾き T'(x)。`b` で形の族が変わる。
struct Base {
    d: f64,
    b: f64,
}

impl Base {
    /// b がこれより 0 や −1 に近ければ、その値の式を使う（一般の式は割り算で桁が落ちる）。
    const NEAR: f64 = 1e-6;

    fn value(&self, x: f64) -> f64 {
        let Base { d, b } = *self;
        if (b + 1.0).abs() < Self::NEAR {
            (1.0 + d * x).ln()
        } else if b < -Self::NEAR {
            (1.0 - (1.0 - b * d * x).powf((b + 1.0) / b)) / (d * (b + 1.0))
        } else if b.abs() <= Self::NEAR {
            1.0 - (-d * x).exp()
        } else {
            1.0 - (1.0 + b * d * x).powf(-1.0 / b)
        }
    }

    fn slope(&self, x: f64) -> f64 {
        let Base { d, b } = *self;
        if (b + 1.0).abs() < Self::NEAR {
            d / (1.0 + d * x)
        } else if b < -Self::NEAR {
            (1.0 - b * d * x).powf(1.0 / b)
        } else if b.abs() <= Self::NEAR {
            d * (-d * x).exp()
        } else {
            d * (1.0 + b * d * x).powf(-(1.0 + b) / b)
        }
    }
}

/// 中間調の変換（midtones transfer function）。`x = m` を 0.5 に、0 と 1 はそのままにする有理式。
pub fn mtf(m: f32, x: f32) -> f32 {
    let (m, x) = (f64::from(m), f64::from(x.clamp(0.0, 1.0)));
    if x <= 0.0 || x >= 1.0 {
        return x as f32;
    }
    ((m - 1.0) * x / ((2.0 * m - 1.0) * x - m)) as f32
}

/// `mtf(m, x) = y` となる中間調 `m`。
pub fn midtones_for(x: f32, y: f32) -> f32 {
    let (x, y) = (f64::from(x), f64::from(y));
    let den = x - 2.0 * x * y + y;
    if den.abs() < 1e-12 {
        return 0.5;
    }
    ((x * (1.0 - y)) / den).clamp(1e-6, 1.0 - 1e-6) as f32
}

/// 自動ストレッチの値。背景（中央値）を `target` の明るさにし、中央値から
/// MAD（正規分布の σ に直した値）の `shadows` 倍だけ暗い所を黒にする。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoStretch {
    pub target: f32,
    pub shadows: f32,
    /// RGB のチャンネルを同じ曲線で伸ばす（色の釣り合いを変えない）。外すとチャンネルごとに背景をそろえる。
    pub linked: bool,
}

/// 自動ストレッチで決めた、黒の値と中間調。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fitted {
    pub black: f32,
    pub midtones: f32,
}

/// 中央値の絶対偏差を、正規分布の標準偏差に直す係数。
const MAD_TO_SIGMA: f32 = 1.4826;
/// 分布を見るのに使う値の数の上限。全画素を並べ替えると大きな画像で遅いので、間引いて使う。
const SAMPLES: usize = 1 << 20;

impl AutoStretch {
    /// 当てる。決めた値をチャンネルごと（つないだときは 1 つ）に返す。
    pub fn apply(&self, data: &mut [f32], channels: usize) -> Vec<Fitted> {
        let channels = channels.max(1);
        let per_channel: Vec<Option<Fitted>> = (0..channels)
            .map(|c| self.fit(sample(data, channels, c)))
            .collect();
        if self.linked {
            // チャンネルごとに決めた値の平均を全チャンネルに使う。全チャンネルの値をまとめて
            // 1 つの分布にしないのは、色かぶりがあると分布が広がって雑音が大きく見え、黒が下がりすぎるため。
            let found: Vec<Fitted> = per_channel.iter().flatten().copied().collect();
            if found.is_empty() {
                return Vec::new();
            }
            let n = found.len() as f32;
            let mean = Fitted {
                black: found.iter().map(|f| f.black).sum::<f32>() / n,
                midtones: found.iter().map(|f| f.midtones).sum::<f32>() / n,
            };
            for v in data.iter_mut() {
                *v = stretch(*v, mean);
            }
            return vec![mean];
        }
        for (i, v) in data.iter_mut().enumerate() {
            if let Some(f) = per_channel[i % channels] {
                *v = stretch(*v, f);
            }
        }
        per_channel.into_iter().flatten().collect()
    }

    fn fit(&self, mut values: Vec<f32>) -> Option<Fitted> {
        let median = median_of(&mut values)?;
        let mut deviations: Vec<f32> = values.iter().map(|v| (v - median).abs()).collect();
        let sigma = median_of(&mut deviations)? * MAD_TO_SIGMA;
        let black = (median - self.shadows.max(0.0) * sigma).clamp(0.0, 1.0);
        let background = ((median - black) / (1.0 - black).max(f32::EPSILON)).clamp(0.0, 1.0);
        let midtones = if background > 0.0 {
            midtones_for(background, self.target.clamp(0.001, 0.999))
        } else {
            0.5
        };
        Some(Fitted { black, midtones })
    }
}

/// [`AutoStretch::apply`] が決めた値（`fitted`）で、別の画像を同じように伸ばす。
pub fn replay(data: &mut [f32], channels: usize, linked: bool, fitted: &[Fitted]) {
    let channels = channels.max(1);
    for (i, v) in data.iter_mut().enumerate() {
        let f = if linked {
            fitted.first()
        } else {
            fitted.get(i % channels)
        };
        if let Some(&f) = f {
            *v = stretch(*v, f);
        }
    }
}

/// チャンネル `c` の、欠けていない値を間引いて集める。
fn sample(data: &[f32], channels: usize, c: usize) -> Vec<f32> {
    let pixels = data.len() / channels;
    let step = pixels.div_ceil(SAMPLES).max(1);
    (0..pixels)
        .step_by(step)
        .map(|p| data[p * channels + c])
        .filter(|v| v.is_finite())
        .collect()
}

/// 黒で切ってから中間調の変換をかける。NaN はそのまま。
fn stretch(v: f32, f: Fitted) -> f32 {
    if v.is_nan() {
        return v;
    }
    mtf(
        f.midtones,
        (v - f.black) / (1.0 - f.black).max(f32::EPSILON),
    )
}

fn median_of(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mid = values.len() / 2;
    let (_, m, _) = values.select_nth_unstable_by(mid, f32::total_cmp);
    Some(*m)
}

/// 0〜1 の表を線形に補って引く。NaN はそのまま。
fn lookup(table: &[f32], v: f32) -> f32 {
    if v.is_nan() {
        return v;
    }
    let at = v.clamp(0.0, 1.0) * (table.len() - 1) as f32;
    let i = (at as usize).min(table.len() - 2);
    let f = at - i as f32;
    table[i] + (table[i + 1] - table[i]) * f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ghs(stretch: f32, local: f32, symmetry: f32) -> Ghs {
        Ghs {
            stretch,
            local,
            symmetry,
            shadows: 0.0,
            highlights: 1.0,
        }
    }

    #[test]
    fn ghs_keeps_black_and_white_and_rises_monotonically_for_every_family() {
        for local in [-3.0, -1.0, -0.5, 0.0, 1.0, 4.0] {
            let g = ghs(3.0, local, 0.1);
            assert!(g.map(0.0).abs() < 1e-6, "b = {local}");
            assert!((g.map(1.0) - 1.0).abs() < 1e-6, "b = {local}");
            let mut last = 0.0;
            for i in 1..=100 {
                let y = g.map(i as f32 / 100.0);
                assert!(y >= last, "b = {local} at {i}");
                last = y;
            }
        }
    }

    #[test]
    fn ghs_with_no_stretch_is_the_identity() {
        let g = ghs(0.0, 2.0, 0.3);
        for x in [0.0, 0.1, 0.5, 0.9, 1.0] {
            assert!((g.map(x) - x).abs() < 1e-6);
        }
    }

    #[test]
    fn ghs_brightens_the_faint_values_most_steeply_at_the_symmetry_point() {
        let g = ghs(5.0, 2.0, 0.05);
        assert!(g.map(0.05) > 0.05);
        let slope = |x: f32| (g.map(x + 0.001) - g.map(x - 0.001)) / 0.002;
        assert!(slope(0.05) > slope(0.02));
        assert!(slope(0.05) > slope(0.2));
    }

    #[test]
    fn ghs_matches_the_published_base_curves_with_symmetry_at_zero() {
        // SP = 0、LP = 0、HP = 1 なら、T(x) / T(1) そのもの。
        let d = 3.0f64.exp() - 1.0;
        let log = ghs(3.0, -1.0, 0.0).map(0.2) as f64;
        assert!((log - (1.0 + d * 0.2).ln() / (1.0 + d).ln()).abs() < 1e-5);
        let exp = ghs(3.0, 0.0, 0.0).map(0.2) as f64;
        assert!((exp - (1.0 - (-d * 0.2).exp()) / (1.0 - (-d).exp())).abs() < 1e-5);
        let harmonic = ghs(3.0, 1.0, 0.0).map(0.2) as f64;
        let h = |x: f64| 1.0 - 1.0 / (1.0 + d * x);
        assert!((harmonic - h(0.2) / h(1.0)).abs() < 1e-5);
    }

    #[test]
    fn ghs_is_a_straight_line_below_the_shadow_protection() {
        let g = Ghs {
            shadows: 0.1,
            ..ghs(4.0, 1.0, 0.2)
        };
        let (a, b, c) = (g.map(0.0), g.map(0.05), g.map(0.1));
        assert!(((b - a) - (c - b)).abs() < 1e-4);
    }

    #[test]
    fn mtf_maps_the_midtones_balance_to_one_half_and_keeps_the_ends() {
        assert!((mtf(0.2, 0.2) - 0.5).abs() < 1e-6);
        assert_eq!(mtf(0.2, 0.0), 0.0);
        assert_eq!(mtf(0.2, 1.0), 1.0);
        assert!((mtf(0.5, 0.3) - 0.3).abs() < 1e-6);
        let m = midtones_for(0.01, 0.25);
        assert!((mtf(m, 0.01) - 0.25).abs() < 1e-5);
    }

    #[test]
    fn auto_stretch_puts_the_background_at_the_target_brightness() {
        let mut state = 7u64;
        let mut data: Vec<f32> = (0..20000)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                0.02 + ((state >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.004
            })
            .collect();
        data[0] = 1.0;
        let auto = AutoStretch {
            target: 0.25,
            shadows: 2.8,
            linked: true,
        };
        let fitted = auto.apply(&mut data, 1);
        assert_eq!(fitted.len(), 1);
        assert!(fitted[0].black > 0.015 && fitted[0].black < 0.02);
        let mut sorted = data[1..].to_vec();
        let background = median_of(&mut sorted).unwrap();
        assert!((background - 0.25).abs() < 0.01, "{background}");
        assert!((data[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn linked_auto_stretch_uses_one_curve_and_keeps_the_colour_order() {
        let mut data: Vec<f32> = (0..3000)
            .flat_map(|i| {
                let n = (i % 7) as f32 * 0.0005;
                [0.03 + n, 0.025 + n, 0.02 + n]
            })
            .collect();
        let auto = AutoStretch {
            target: 0.25,
            shadows: 2.8,
            linked: true,
        };
        assert_eq!(auto.apply(&mut data, 3).len(), 1);
        let px = &data[3 * 6..3 * 7];
        assert!(px[0] > px[1] && px[1] > px[2], "{px:?}");
    }

    #[test]
    fn unlinked_auto_stretch_evens_out_a_colour_cast_in_the_background() {
        let mut data: Vec<f32> = (0..3000)
            .flat_map(|i| {
                let n = (i % 7) as f32 * 0.0005;
                [0.05 + n, 0.02 + n, 0.01 + n]
            })
            .collect();
        let auto = AutoStretch {
            target: 0.25,
            shadows: 2.8,
            linked: false,
        };
        assert_eq!(auto.apply(&mut data, 3).len(), 3);
        let px = &data[..3];
        assert!(
            (px[0] - px[1]).abs() < 0.05 && (px[1] - px[2]).abs() < 0.05,
            "{px:?}"
        );
    }
}
