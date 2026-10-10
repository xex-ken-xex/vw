//! [`VwImage`] から描画用メモリ [`DisplayImage`]（RGBA8）への変換。
//!
//! 階調変換（ウィンドウ幅/レベル）と、GPU のテクスチャ上限に収めるための縮小をここで行う。

use crate::image::{ColorModel, PixelData, VwImage};

/// GPU に転送する RGBA8 画像。行優先、アルファは乗算済みでない（ストレート）。
#[derive(Clone, Debug)]
pub struct DisplayImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// 表示用変換のパラメータ。
#[derive(Clone, Copy, Debug)]
pub struct DisplayParams {
    /// 黒と白に割り当てるサンプル値 `(黒, 白)`。DICOM のウィンドウ幅/レベル調整に使う。
    /// `None` のときは、U8/U16 は有効ビットの全範囲、F32 はデータの最小値〜最大値を使う。
    pub window: Option<(f32, f32)>,
    /// 縦横どちらかがこれを超える画像は縮小する。
    /// 既定値 8192 は、多くの GPU が対応するテクスチャの一辺の上限に合わせている。
    pub max_side: u32,
}

impl Default for DisplayParams {
    fn default() -> Self {
        Self {
            window: None,
            max_side: 8192,
        }
    }
}

/// `img` を描画用の RGBA8 に変換する。アルファチャンネルには階調変換をかけない。
///
/// U8/U16 は 256 / 65536 要素の変換表（LUT）を先に作って引く。
/// 画素ごとに浮動小数点で計算するより速く、16bit 画像でも表は 64KB で済むため。
pub fn to_display(img: &VwImage, params: &DisplayParams) -> DisplayImage {
    match &img.data {
        PixelData::U8(v) => match params.window {
            None => convert(img, params, |i| v[i], |i| v[i]),
            Some(win) => {
                let lut = build_lut(256, win);
                convert(img, params, |i| lut[v[i] as usize], |i| v[i])
            }
        },
        PixelData::U16(v) => {
            let max = ((1u32 << img.significant_bits.clamp(1, 16)) - 1) as f32;
            let lut = build_lut(65536, params.window.unwrap_or((0.0, max)));
            convert(img, params, |i| lut[v[i] as usize], |i| (v[i] >> 8) as u8)
        }
        PixelData::F32(v) => {
            let (lo, hi) = params.window.unwrap_or_else(|| min_max(v));
            let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
            convert(
                img,
                params,
                |i| ((v[i] - lo) * scale).clamp(0.0, 255.0) as u8,
                |i| (v[i].clamp(0.0, 1.0) * 255.0) as u8,
            )
        }
    }
}

/// `sample` は色サンプルの添字を 0..=255 に、`alpha` はアルファサンプルの添字を 0..=255 に写す。
///
/// 型ごとの差を閉包に閉じ込めて、ループ本体を 1 つにしている。
/// `Box<dyn Fn>` にしないのは、画素ごとの動的呼び出しが変換時間の大半を占めてしまうため
/// （ジェネリクスならインライン展開される）。
fn convert(
    img: &VwImage,
    params: &DisplayParams,
    sample: impl Fn(usize) -> u8 + Sync,
    alpha: impl Fn(usize) -> u8 + Sync,
) -> DisplayImage {
    let ch = img.color.channels();
    let (w, h) = (img.width as usize, img.height as usize);
    // 縮小は整数間引き（最近傍）。面積平均の方が画質は良いが、1 パスで済まず遅くなる。
    // 縮小表示時の見た目は GPU 側のミップマップで補っている。
    let step = (img.width.max(img.height) as usize)
        .div_ceil(params.max_side.max(1) as usize)
        .max(1);
    let (ow, oh) = (w.div_ceil(step), h.div_ceil(step));
    let mut rgba = vec![0u8; ow * oh * 4];

    let convert_rows = |first_row: usize, out: &mut [u8]| {
        for (r, out_row) in out.chunks_exact_mut(ow * 4).enumerate() {
            let row = (first_row + r) * step * w;
            for (ox, px) in out_row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let s = (row + ox * step) * ch;
                let rgba_px = match img.color {
                    ColorModel::Gray => {
                        let g = sample(s);
                        [g, g, g, 255]
                    }
                    ColorModel::GrayAlpha => {
                        let g = sample(s);
                        [g, g, g, alpha(s + 1)]
                    }
                    ColorModel::Rgb => [sample(s), sample(s + 1), sample(s + 2), 255],
                    ColorModel::Rgba => [sample(s), sample(s + 1), sample(s + 2), alpha(s + 3)],
                };
                px.copy_from_slice(&rgba_px);
            }
        }
    };

    // 大きな画像は行単位でスレッドに分ける。小さな画像はスレッド生成の方が高くつくので分けない。
    // rayon などのスレッドプールを使わないのは、依存を増やさず std だけで足りるため。
    let threads = if ow * oh < 512 * 512 {
        1
    } else {
        std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(oh)
    };
    if threads <= 1 {
        convert_rows(0, &mut rgba);
    } else {
        let rows_per = oh.div_ceil(threads);
        std::thread::scope(|scope| {
            for (i, chunk) in rgba.chunks_mut(rows_per * ow * 4).enumerate() {
                let convert_rows = &convert_rows;
                scope.spawn(move || convert_rows(i * rows_per, chunk));
            }
        });
    }
    DisplayImage {
        width: ow as u32,
        height: oh as u32,
        rgba,
    }
}

/// 0..size の値を、`lo` → 0、`hi` → 255 の直線で写す変換表。範囲外は 0 / 255 に張り付く。
fn build_lut(size: usize, (lo, hi): (f32, f32)) -> Vec<u8> {
    let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
    (0..size)
        .map(|v| ((v as f32 - lo) * scale).round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// NaN や無限大を除いた最小値と最大値。
fn min_max(v: &[f32]) -> (f32, f32) {
    v.iter()
        .filter(|x| x.is_finite())
        .fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray8(values: &[u8]) -> VwImage {
        VwImage::new(
            values.len() as u32,
            1,
            ColorModel::Gray,
            PixelData::U8(values.to_vec()),
        )
        .unwrap()
    }

    #[test]
    fn rgb8_passthrough() {
        let img =
            VwImage::new(2, 1, ColorModel::Rgb, PixelData::U8(vec![1, 2, 3, 4, 5, 6])).unwrap();
        let d = to_display(&img, &DisplayParams::default());
        assert_eq!(d.rgba, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn gray16_uses_significant_bits() {
        let mut img = VwImage::new(2, 1, ColorModel::Gray, PixelData::U16(vec![0, 4095])).unwrap();
        img.significant_bits = 12;
        let d = to_display(&img, &DisplayParams::default());
        assert_eq!(d.rgba, vec![0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn threaded_conversion_matches_source() {
        let (w, h) = (1031u32, 777u32);
        let data: Vec<u8> = (0..w * h).map(|i| (i % 251) as u8).collect();
        let img = VwImage::new(w, h, ColorModel::Gray, PixelData::U8(data.clone())).unwrap();
        let d = to_display(&img, &DisplayParams::default());
        assert!(d
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(&data)
            .all(|(px, &g)| *px == [g, g, g, 255]));
    }

    #[test]
    fn gray8_expands_to_opaque_rgb() {
        let d = to_display(&gray8(&[0, 128, 255]), &DisplayParams::default());
        assert_eq!(
            d.rgba,
            vec![0, 0, 0, 255, 128, 128, 128, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn gray_alpha_keeps_alpha() {
        let img = VwImage::new(1, 1, ColorModel::GrayAlpha, PixelData::U8(vec![10, 20])).unwrap();
        let d = to_display(&img, &DisplayParams::default());
        assert_eq!(d.rgba, vec![10, 10, 10, 20]);
    }

    #[test]
    fn rgba8_passthrough() {
        let img = VwImage::new(1, 1, ColorModel::Rgba, PixelData::U8(vec![1, 2, 3, 4])).unwrap();
        let d = to_display(&img, &DisplayParams::default());
        assert_eq!(d.rgba, vec![1, 2, 3, 4]);
    }

    #[test]
    fn window_maps_low_to_black_and_high_to_white_and_clamps() {
        let params = DisplayParams {
            window: Some((100.0, 200.0)),
            ..Default::default()
        };
        let d = to_display(&gray8(&[50, 100, 150, 200, 250]), &params);
        let grays: Vec<u8> = d.rgba.iter().step_by(4).copied().collect();
        assert_eq!(grays, vec![0, 0, 128, 255, 255]);
    }

    #[test]
    fn window_is_not_applied_to_alpha() {
        let img = VwImage::new(
            1,
            1,
            ColorModel::Rgba,
            PixelData::U8(vec![150, 150, 150, 150]),
        )
        .unwrap();
        let params = DisplayParams {
            window: Some((100.0, 200.0)),
            ..Default::default()
        };
        let d = to_display(&img, &params);
        assert_eq!(d.rgba, vec![128, 128, 128, 150]);
    }

    #[test]
    fn rgba16_alpha_uses_high_byte() {
        let img = VwImage::new(
            1,
            1,
            ColorModel::Rgba,
            PixelData::U16(vec![0, 0, 65535, 0x8000]),
        )
        .unwrap();
        let d = to_display(&img, &DisplayParams::default());
        assert_eq!(d.rgba, vec![0, 0, 255, 128]);
    }

    #[test]
    fn f32_defaults_to_data_min_max_and_ignores_nan() {
        let img = VwImage::new(
            4,
            1,
            ColorModel::Gray,
            PixelData::F32(vec![-1000.0, 0.0, 1000.0, f32::NAN]),
        )
        .unwrap();
        let d = to_display(&img, &DisplayParams::default());
        let grays: Vec<u8> = d.rgba.iter().step_by(4).copied().collect();
        assert_eq!(&grays[..3], &[0, 127, 255]);
    }

    #[test]
    fn constant_image_does_not_divide_by_zero() {
        let img = VwImage::new(2, 1, ColorModel::Gray, PixelData::F32(vec![5.0, 5.0])).unwrap();
        let d = to_display(&img, &DisplayParams::default());
        assert_eq!(d.rgba.len(), 8);
    }

    #[test]
    fn image_within_max_side_is_not_resized() {
        let d = to_display(
            &gray8(&[1; 100]),
            &DisplayParams {
                window: None,
                max_side: 100,
            },
        );
        assert_eq!((d.width, d.height), (100, 1));
    }

    #[test]
    fn downsampling_takes_every_nth_pixel() {
        let d = to_display(
            &gray8(&[0, 1, 2, 3, 4, 5]),
            &DisplayParams {
                window: None,
                max_side: 3,
            },
        );
        let grays: Vec<u8> = d.rgba.iter().step_by(4).copied().collect();
        assert_eq!(grays, vec![0, 2, 4]);
    }

    #[test]
    fn downsamples_to_max_side() {
        let img = VwImage::new(10, 4, ColorModel::Gray, PixelData::U8(vec![7; 40])).unwrap();
        let d = to_display(
            &img,
            &DisplayParams {
                window: None,
                max_side: 5,
            },
        );
        assert_eq!((d.width, d.height), (5, 2));
    }
}
