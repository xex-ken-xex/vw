//! 番号で色を引く画像の、色にする前の値。
//!
//! - [`Indices`]: 番号を色に変えた画像（DICOM の PALETTE COLOR）の、画素ごとの元の番号。
//!   十字で番号を出すのに使う
//! - [`ValueColors`]: 灰色の画像のうち、決まった範囲の値だけに当てる色（DICOM の
//!   Supplemental Palette）。灰色のまま持ち、表示のときに色を塗る

/// 画素ごとの番号。番号の範囲に合う一番小さい型で持つ。
#[derive(Debug, Clone, PartialEq)]
pub enum Indices {
    U8(Vec<u8>),
    U16(Vec<u16>),
    I32(Vec<i32>),
}

impl Indices {
    pub fn from_values(values: &[i64]) -> Self {
        let (lo, hi) = values
            .iter()
            .fold((i64::MAX, i64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        if values.is_empty() || (lo >= 0 && hi <= i64::from(u8::MAX)) {
            Self::U8(values.iter().map(|&v| v as u8).collect())
        } else if lo >= 0 && hi <= i64::from(u16::MAX) {
            Self::U16(values.iter().map(|&v| v as u16).collect())
        } else {
            Self::I32(
                values
                    .iter()
                    .map(|&v| v.clamp(i32::MIN.into(), i32::MAX.into()) as i32)
                    .collect(),
            )
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::U8(v) => v.len(),
            Self::U16(v) => v.len(),
            Self::I32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `i` 番目の画素の番号。
    pub fn get(&self, i: usize) -> Option<i64> {
        match self {
            Self::U8(v) => v.get(i).map(|&x| x.into()),
            Self::U16(v) => v.get(i).map(|&x| x.into()),
            Self::I32(v) => v.get(i).map(|&x| x.into()),
        }
    }
}

/// 灰色の画像の、格納値 `first` から `colors.len()` 個の値に当てる色。
#[derive(Debug, Clone, PartialEq)]
pub struct ValueColors {
    /// 色を当てる最初の格納値。
    pub first: i64,
    /// 16bit の RGB。8bit の表は 257 倍して入れる。
    pub colors: Vec<[u16; 3]>,
    /// 表の 1 色のビット数（8 か 16）。十字で色を表の深さのまま書くのに使う。
    pub bits: u32,
    /// 画像の値から格納値に戻すための、画像の値 = `sign` × (格納値 × `slope` + `intercept`)。
    pub slope: f64,
    pub intercept: f64,
    pub sign: f64,
}

impl ValueColors {
    /// 画像の値 `value` の色。範囲の外なら `None`（灰色のまま）。
    pub fn color_of(&self, value: f32) -> Option<[u16; 3]> {
        if !value.is_finite() || self.slope == 0.0 {
            return None;
        }
        let stored = ((f64::from(value) * self.sign - self.intercept) / self.slope).round() as i64;
        let i = usize::try_from(stored.checked_sub(self.first)?).ok()?;
        self.colors.get(i).copied()
    }

    /// 値 `value` の色を、表の深さ（[`ValueColors::bits`]）の値で。
    pub fn table_color_of(&self, value: f32) -> Option<[u16; 3]> {
        let c = self.color_of(value)?;
        Some(if self.bits > 8 { c } else { c.map(|v| v / 257) })
    }

    /// 色を当てる格納値の範囲（両端を含む）。
    pub fn range(&self) -> (i64, i64) {
        (self.first, self.first + self.colors.len() as i64 - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_use_the_smallest_type_that_holds_them() {
        assert!(matches!(Indices::from_values(&[0, 255]), Indices::U8(_)));
        assert!(matches!(Indices::from_values(&[0, 256]), Indices::U16(_)));
        assert!(matches!(Indices::from_values(&[-1, 3]), Indices::I32(_)));
        assert_eq!(Indices::from_values(&[-1, 300]).get(1), Some(300));
    }

    fn red_from_100() -> ValueColors {
        ValueColors {
            first: 100,
            colors: vec![[65535, 0, 0]; 10],
            bits: 8,
            slope: 1.0,
            intercept: 0.0,
            sign: 1.0,
        }
    }

    #[test]
    fn only_values_inside_the_table_get_a_color() {
        let c = red_from_100();
        assert_eq!(c.color_of(99.0), None);
        assert_eq!(c.color_of(100.0), Some([65535, 0, 0]));
        assert_eq!(c.color_of(109.0), Some([65535, 0, 0]));
        assert_eq!(c.color_of(110.0), None);
        assert_eq!(c.range(), (100, 109));
        assert_eq!(c.table_color_of(100.0), Some([255, 0, 0]));
    }

    #[test]
    fn rescaled_values_are_turned_back_into_stored_values() {
        let c = ValueColors {
            slope: 2.0,
            intercept: -1000.0,
            ..red_from_100()
        };
        // 格納値 100 は 100 × 2 - 1000 = -800。
        assert_eq!(c.color_of(-800.0), Some([65535, 0, 0]));
        assert_eq!(c.color_of(-802.0), None);
        let inverted = ValueColors {
            sign: -1.0,
            ..red_from_100()
        };
        assert_eq!(inverted.color_of(-105.0), Some([65535, 0, 0]));
    }
}
