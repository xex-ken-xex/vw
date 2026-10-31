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

/// 横軸。`index` 番目（0 から）の値は `reference + (index + 1 - reference_pixel) × step`。
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub kind: AxisKind,
    pub reference: f64,
    pub reference_pixel: f64,
    pub step: f64,
    /// 静止周波数（Hz）。分かれば、周波数の軸から視線速度を出せる。
    pub rest_frequency: Option<f64>,
}

impl Axis {
    /// `index` 番目（0 から、小数も可）の点の横軸の値。
    pub fn value(&self, index: f64) -> f64 {
        self.reference + (index + 1.0 - self.reference_pixel) * self.step
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
    /// 周波数の軸で静止周波数が分かるとき、`hz` に当たる電波の定義の視線速度（m/s）。
    pub fn radio_velocity(&self, hz: f64) -> Option<f64> {
        match self.axis.kind {
            AxisKind::Frequency => self
                .axis
                .rest_frequency
                .filter(|f| *f > 0.0)
                .map(|f0| LIGHT_M_S * (1.0 - hz / f0)),
            _ => None,
        }
    }

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

    fn spectrum(kind: AxisKind, rest: Option<f64>) -> Spectrum {
        Spectrum {
            values: Arc::from([1.0, f32::NAN, -2.0, 5.0]),
            axis: Axis {
                kind,
                reference: 100.0,
                reference_pixel: 2.0,
                step: 10.0,
                rest_frequency: rest,
            },
            value_unit: String::new(),
        }
    }

    #[test]
    fn axis_values_count_pixels_from_one_like_fits() {
        let s = spectrum(AxisKind::Frequency, None);
        assert_eq!(s.axis.value(1.0), 100.0);
        assert_eq!(s.axis.value(0.0), 90.0);
        assert_eq!(s.axis.value(3.0), 120.0);
    }

    #[test]
    fn value_range_skips_missing_points() {
        assert_eq!(
            spectrum(AxisKind::Velocity, None).value_range(),
            Some((-2.0, 5.0))
        );
    }

    #[test]
    fn radio_velocity_needs_a_frequency_axis_and_a_rest_frequency() {
        let s = spectrum(AxisKind::Frequency, Some(110e9));
        assert_eq!(s.radio_velocity(110e9), Some(0.0));
        assert!(s.radio_velocity(109e9).unwrap() > 0.0);
        assert_eq!(
            spectrum(AxisKind::Frequency, None).radio_velocity(1e9),
            None
        );
        assert_eq!(
            spectrum(AxisKind::Velocity, Some(1e9)).radio_velocity(1e9),
            None
        );
    }
}
