//! 色の値（R・G・B）の書き方。十字（[`crate::probe`]）と ROI（[`crate::roi`]）で使う。

use crate::histogram::number;
use crate::settings::ColorFormat;

/// R・G・B（A があれば A も）を `format` で書く。色コードにできない値（浮動小数の画像や
/// 範囲の外の値）は数値で書く。
pub fn text(values: &[f64], bits: u32, format: ColorFormat) -> String {
    match format {
        ColorFormat::Hex => hex(values, bits).unwrap_or_else(|| numbers(values)),
        ColorFormat::Rgb => numbers(values),
    }
}

/// `R 128 G 64 B 255`（A があれば ` A 255`）。
pub fn numbers(values: &[f64]) -> String {
    ["R", "G", "B", "A"]
        .iter()
        .zip(values)
        .map(|(name, &v)| format!("{name} {}", number(v)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `#8040FF`。8 bit を超える画像は各色 4 桁（`#80A04000FFFF`）にする。
/// 2 桁に縮めないのは、16 bit の値の違いが消えてしまうため。
/// 16 bit を超える（浮動小数の）画像は、値の範囲が決まっていないので `None`。
pub fn hex(values: &[f64], bits: u32) -> Option<String> {
    let (max, width) = match bits {
        0 => return None,
        1..=8 => (255.0, 2),
        9..=16 => (65535.0, 4),
        _ => return None,
    };
    let digits = values
        .iter()
        .map(|&v| {
            let r = v.round();
            (0.0..=max)
                .contains(&r)
                .then(|| format!("{:0width$X}", r as u32))
        })
        .collect::<Option<String>>()?;
    Some(format!("#{digits}"))
}

/// 見本の四角に塗る 8 bit の色。`bits` の範囲を 0〜255 に縮める。浮動小数の画像では `None`。
pub fn swatch(values: &[f64], bits: u32) -> Option<[u8; 3]> {
    if values.len() < 3 || bits == 0 || bits > 16 {
        return None;
    }
    let max = f64::from((1u32 << bits) - 1);
    let mut rgb = [0u8; 3];
    for (out, &v) in rgb.iter_mut().zip(values) {
        if !(0.0..=max).contains(&v) {
            return None;
        }
        *out = (v / max * 255.0).round() as u8;
    }
    Some(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_bit_colors_become_six_digit_codes() {
        assert_eq!(hex(&[128.0, 64.0, 255.0], 8).as_deref(), Some("#8040FF"));
        assert_eq!(text(&[128.0, 64.0, 255.0], 8, ColorFormat::Hex), "#8040FF");
    }

    #[test]
    fn sixteen_bit_colors_keep_four_digits_per_channel() {
        assert_eq!(
            hex(&[0x80A0 as f64, 0x4000 as f64, 65535.0], 16).as_deref(),
            Some("#80A04000FFFF")
        );
    }

    #[test]
    fn alpha_is_written_after_blue() {
        assert_eq!(
            hex(&[1.0, 2.0, 3.0, 255.0], 8).as_deref(),
            Some("#010203FF")
        );
        assert_eq!(numbers(&[1.0, 2.0, 3.0, 255.0]), "R 1 G 2 B 3 A 255");
    }

    #[test]
    fn mean_values_are_rounded_into_the_code() {
        assert_eq!(hex(&[127.6, 0.4, 10.0], 8).as_deref(), Some("#80000A"));
    }

    #[test]
    fn float_images_and_values_out_of_range_fall_back_to_numbers() {
        assert_eq!(hex(&[-1.0, 0.0, 0.0], 8), None);
        assert_eq!(
            text(&[0.5, 0.25, 1.5], 32, ColorFormat::Hex),
            "R 0.50 G 0.25 B 1.50"
        );
    }

    #[test]
    fn rgb_format_writes_numbers() {
        assert_eq!(
            text(&[128.0, 64.0, 255.0], 8, ColorFormat::Rgb),
            "R 128 G 64 B 255"
        );
    }

    #[test]
    fn swatch_scales_the_bit_depth_to_eight_bits() {
        assert_eq!(swatch(&[4095.0, 0.0, 2048.0], 12), Some([255, 0, 128]));
        assert_eq!(swatch(&[0.5, 0.5, 0.5], 0), None);
    }
}
