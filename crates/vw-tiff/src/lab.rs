//! L*a*b* の TIFF（CIELab・ICCLab・ITULab）を sRGB にする。
//!
//! tiff クレートは Lab を色の種類として断るので、写しの PhotometricInterpretation を RGB に書き換えて
//! 3 つのサンプルをそのまま読み、ここで色に直す。ほどく処理と面の扱いを自前で持たずに済むため。

use vw_core::{DecodeError, PixelData};

/// PhotometricInterpretation の Lab の値。
pub const CIELAB: u16 = 8;
pub const ICCLAB: u16 = 9;
pub const ITULAB: u16 = 10;

/// RGB として読んだ Lab の値（`channels` が 4 なら 4 つ目はアルファで、そのまま残す）を sRGB にする。
///
/// 値の決まりは種類で違う: CIELab の a*・b* は符号付き、ICCLab は 128（16bit なら 32768）を 0 とする
/// 符号なし、ITULab は TIFF-FX の既定の範囲（L* 0〜100、a* −85〜85、b* −75〜125）を全段階に割り当てる。
/// ITULab の範囲を変える Decode タグは読まない。書くものがほぼないため。
pub fn to_srgb(data: PixelData, channels: usize, kind: u16) -> Result<PixelData, DecodeError> {
    // L* は全段階で 0〜100。a*・b* はここで実際の値に直す。`unit` は値を 0〜1 にしたもの。
    let ab = |unit: f32, signed: f32, range: (f32, f32)| match kind {
        CIELAB => signed,
        ICCLAB => unit * 255.0 - 128.0,
        _ => range.0 + unit * (range.1 - range.0),
    };
    let lab = |l: f32, a: [f32; 2], b: [f32; 2]| {
        [
            l * 100.0,
            ab(a[0], a[1], (-85.0, 85.0)),
            ab(b[0], b[1], (-75.0, 125.0)),
        ]
    };
    Ok(match data {
        PixelData::U8(v) => {
            let ab = |x: u8| [f32::from(x) / 255.0, f32::from(x as i8)];
            let mut out = Vec::with_capacity(v.len());
            for px in v.chunks_exact(channels) {
                let [l, a, b] = lab(f32::from(px[0]) / 255.0, ab(px[1]), ab(px[2]));
                out.extend(srgb(l, a, b).map(|c| (c * 255.0).round() as u8));
                out.extend(&px[3..]);
            }
            PixelData::U8(out)
        }
        PixelData::U16(v) => {
            // CIELab の 16bit の a*・b* は 1/256 刻みの符号付き。
            let ab = |x: u16| [f32::from(x) / 65535.0, f32::from(x as i16) / 256.0];
            let mut out = Vec::with_capacity(v.len());
            for px in v.chunks_exact(channels) {
                let [l, a, b] = lab(f32::from(px[0]) / 65535.0, ab(px[1]), ab(px[2]));
                out.extend(srgb(l, a, b).map(|c| (c * 65535.0).round() as u16));
                out.extend(&px[3..]);
            }
            PixelData::U16(out)
        }
        PixelData::F32(_) => {
            return Err(DecodeError::Unsupported(
                "TIFF L*a*b* in this sample depth".into(),
            ));
        }
    })
}

/// L*a*b*（白は D50。TIFF の決まり）を、0〜1 の sRGB にする。
fn srgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let inverse = |t: f32| {
        if t > 6.0 / 29.0 {
            t * t * t
        } else {
            3.0 * (6.0f32 / 29.0).powi(2) * (t - 4.0 / 29.0)
        }
    };
    let (x, y, z) = (0.9642 * inverse(fx), inverse(fy), 0.8249 * inverse(fz));
    // D50 の XYZ から sRGB（D65）へ。白の違いは Bradford の変換で合わせた行列。
    let linear = [
        3.133_856 * x - 1.616_867 * y - 0.490_615 * z,
        -0.978_768 * x + 1.916_142 * y + 0.033_454 * z,
        0.071_945 * x - 0.228_991 * y + 1.405_243 * z,
    ];
    linear.map(|c| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.003_130_8 {
            12.92 * c
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_white_is_srgb_white() {
        let [r, g, b] = srgb(100.0, 0.0, 0.0);
        for c in [r, g, b] {
            assert!((c - 1.0).abs() < 0.002, "{c}");
        }
    }

    #[test]
    fn lab_red_is_srgb_red() {
        // sRGB の (255, 0, 0) は L*a*b*（D50）で 54.29, 80.80, 69.89。
        let [r, g, b] = srgb(54.29, 80.80, 69.89);
        assert!(r > 0.99 && g < 0.03 && b < 0.03, "{r} {g} {b}");
    }
}
