//! 1 次元のスペクトル。画像ではなく、軸の値ごとの値の並びとして描くためのもの。

use std::sync::Arc;

/// 横軸の種類。値は基本の単位（周波数は Hz、速度は m/s、波長は m）で持つ。
#[derive(Debug, Clone, PartialEq)]
pub enum AxisKind {
    Frequency,
    Velocity,
    Wavelength,
    /// 読めない種類。`CTYPE` と単位をそのまま見出しにする。
    Other {
        name: String,
        unit: String,
    },
}

/// 横軸。点ごとの値を持つ（周波数で書いた軸を波長で見るときなど、軸は線形とは限らないため）。
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub kind: AxisKind,
    /// 点ごとの横軸の値（基本の単位）。値の並びと同じ数。
    pub values: Arc<[f64]>,
    /// 静止周波数（Hz）。分かれば、周波数の軸から視線速度を出せる。
    pub rest_frequency: Option<f64>,
    /// 静止波長（m）。分かれば、波長の軸から視線速度を出せる。
    pub rest_wavelength: Option<f64>,
    /// 速度の基準（FITS の `SPECSYS`、`LSRK`・`BARYCENT` など）。分からなければ空。
    pub frame: String,
}

impl Axis {
    /// `reference_pixel`（1 始まり）で `reference`、1 点ごとに `step` ずつ変わる `n` 点の軸。
    pub fn linear(
        kind: AxisKind,
        reference: f64,
        reference_pixel: f64,
        step: f64,
        n: usize,
    ) -> Self {
        Self {
            kind,
            values: (0..n)
                .map(|i| reference + (i as f64 + 1.0 - reference_pixel) * step)
                .collect(),
            rest_frequency: None,
            rest_wavelength: None,
            frame: String::new(),
        }
    }

    /// `index` 番目（0 から）の点の横軸の値。
    pub fn value(&self, index: usize) -> f64 {
        self.values[index]
    }

    /// 横軸の値が `x` にいちばん近い点の番号。点がなければ 0。
    pub fn nearest(&self, x: f64) -> usize {
        self.values
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
            .map_or(0, |(i, _)| i)
    }

    /// 横軸の値 `at` に当たる視線速度（m/s）。周波数の軸は電波の定義、波長の軸は光学の定義
    /// （どちらもその分野の慣習）。静止周波数か静止波長のどちらかがあれば、もう一方に直して使う。
    pub fn velocity(&self, at: f64) -> Option<f64> {
        let rest_hz = self
            .rest_frequency
            .or(self.rest_wavelength.map(|w| LIGHT_M_S / w))
            .filter(|f| f.is_finite() && *f > 0.0);
        let rest_m = self
            .rest_wavelength
            .or(self.rest_frequency.map(|f| LIGHT_M_S / f))
            .filter(|w| w.is_finite() && *w > 0.0);
        match self.kind {
            AxisKind::Frequency => rest_hz.map(|f0| LIGHT_M_S * (1.0 - at / f0)),
            AxisKind::Wavelength => rest_m.map(|w0| LIGHT_M_S * (at / w0 - 1.0)),
            _ => None,
        }
    }
}

/// 値の並びと、その横軸。
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    pub values: Arc<[f32]>,
    pub axis: Axis,
    /// 値の単位（FITS の `BUNIT`）。分からなければ空。
    pub value_unit: String,
}

/// 光の速さ（m/s）。
pub const LIGHT_M_S: f64 = 299_792_458.0;

impl Spectrum {
    /// 値のある点（NaN でない）の最小と最大。値のある点がなければ `None`。
    pub fn value_range(&self) -> Option<(f32, f32)> {
        self.values
            .iter()
            .filter(|v| v.is_finite())
            .fold(None, |r, &v| match r {
                None => Some((v, v)),
                Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spectrum(kind: AxisKind) -> Spectrum {
        Spectrum {
            values: Arc::from([1.0, f32::NAN, -2.0, 5.0]),
            axis: Axis::linear(kind, 100.0, 2.0, 10.0, 4),
            value_unit: String::new(),
        }
    }

    #[test]
    fn linear_axis_counts_pixels_from_one_like_fits() {
        let s = spectrum(AxisKind::Frequency);
        assert_eq!(s.axis.value(1), 100.0);
        assert_eq!(s.axis.value(0), 90.0);
        assert_eq!(s.axis.value(3), 120.0);
    }

    #[test]
    fn nearest_point_follows_the_values_even_when_they_fall() {
        let mut axis = Axis::linear(AxisKind::Velocity, 0.0, 1.0, -10.0, 5);
        assert_eq!(axis.nearest(-21.0), 2);
        axis.values = Arc::from([1.0, 2.0, 4.0, 8.0]);
        assert_eq!(axis.nearest(5.5), 2);
    }

    #[test]
    fn value_range_skips_missing_points() {
        assert_eq!(
            spectrum(AxisKind::Velocity).value_range(),
            Some((-2.0, 5.0))
        );
    }

    #[test]
    fn frequency_axes_give_radio_velocity_from_the_rest_frequency() {
        let mut axis = Axis::linear(AxisKind::Frequency, 110e9, 1.0, 1e6, 3);
        assert_eq!(axis.velocity(110e9), None);
        axis.rest_frequency = Some(110e9);
        assert_eq!(axis.velocity(110e9), Some(0.0));
        assert!(axis.velocity(109e9).unwrap() > 0.0);
    }

    #[test]
    fn wavelength_axes_give_optical_velocity_from_the_rest_wavelength() {
        let mut axis = Axis::linear(AxisKind::Wavelength, 656.28e-9, 1.0, 1e-10, 3);
        axis.rest_wavelength = Some(656.28e-9);
        let v = axis.velocity(656.28e-9 * 1.001).unwrap();
        assert!((v - LIGHT_M_S * 0.001).abs() < 1e-3, "{v}");
    }

    #[test]
    fn a_rest_frequency_also_serves_a_wavelength_axis() {
        let mut axis = Axis::linear(AxisKind::Wavelength, 0.21, 1.0, 1e-5, 3);
        axis.rest_frequency = Some(LIGHT_M_S / 0.21);
        assert!(axis.velocity(0.21).unwrap().abs() < 1e-6);
    }

    #[test]
    fn velocity_and_other_axes_have_no_derived_velocity() {
        let mut axis = Axis::linear(AxisKind::Velocity, 0.0, 1.0, 1.0, 3);
        axis.rest_frequency = Some(1e9);
        assert_eq!(axis.velocity(5.0), None);
    }
}
