//! [`VwImage`] から描画用メモリ [`DisplayImage`]（RGBA8）への変換。
//!
//! 階調変換（ウィンドウ幅/レベル）と、GPU のテクスチャ上限に収めるための縮小をここで行う。

use crate::image::{ColorModel, Curve, PixelData, VwImage};

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
    let window = params.window.or(img.window);
    let curve = img.curve;
    match &img.data {
        PixelData::U8(v) => match window {
            None if curve == Curve::Linear => convert(img, params, |i| v[i], |i| v[i]),
            _ => {
                let lut = build_lut(256, window.unwrap_or((0.0, 255.0)), curve);
                convert(img, params, |i| lut[v[i] as usize], |i| v[i])
            }
        },
        PixelData::U16(v) => {
            let max = ((1u32 << img.significant_bits.clamp(1, 16)) - 1) as f32;
            let lut = build_lut(65536, window.unwrap_or((0.0, max)), curve);
            convert(img, params, |i| lut[v[i] as usize], |i| (v[i] >> 8) as u8)
        }
        PixelData::F32(v) => {
            let shade = Shade::new(window.unwrap_or_else(|| min_max(v)), curve);
            convert(
                img,
                params,
                |i| shade.of(v[i]),
                |i| (v[i].clamp(0.0, 1.0) * 255.0) as u8,
            )
        }
    }
}

/// [`to_display`] が黒と白に使う値 `(黒, 白)`。
pub fn window_of(img: &VwImage, params: &DisplayParams) -> (f32, f32) {
    params
        .window
        .or(img.window)
        .unwrap_or_else(|| match &img.data {
            PixelData::U8(_) => (0.0, 255.0),
            PixelData::U16(_) => (
                0.0,
                ((1u32 << img.significant_bits.clamp(1, 16)) - 1) as f32,
            ),
            PixelData::F32(v) => min_max(v),
        })
}

/// 灰色の画像の、NaN を除いた値の最小と最大。灰色でない画像と、値が 1 つもない画像は `None`。
pub fn gray_range(img: &VwImage) -> Option<(f32, f32)> {
    if !matches!(img.color, ColorModel::Gray | ColorModel::GrayAlpha) {
        return None;
    }
    let step = img.color.channels();
    let (lo, hi) = match &img.data {
        PixelData::U8(v) => fold_range(v.iter().step_by(step).map(|&x| f32::from(x))),
        PixelData::U16(v) => fold_range(v.iter().step_by(step).map(|&x| f32::from(x))),
        PixelData::F32(v) => fold_range(v.iter().step_by(step).copied()),
    };
    (lo <= hi).then_some((lo, hi))
}

fn fold_range(values: impl Iterator<Item = f32>) -> (f32, f32) {
    values
        .filter(|x| x.is_finite())
        .fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)))
}

/// 黒 `lo`〜白 `hi` の値を 0〜255 に写す。範囲外は 0 / 255 に張り付き、NaN（欠損）は黒。
struct Shade {
    lo: f32,
    /// 値 1 あたりの明るさ（0〜255）。
    scale: f32,
    curve: Curve,
}

impl Shade {
    fn new((lo, hi): (f32, f32), curve: Curve) -> Self {
        let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
        Self { lo, scale, curve }
    }

    fn of(&self, v: f32) -> u8 {
        let x = (v - self.lo) * self.scale;
        // 直線は曲線を通さずに丸める。0〜1 に直してから戻すと浮動小数点の誤差で 1 段ずれる
        // 値があり、画素の変換の表示が標準の表示と食い違ってしまうため。
        let x = match self.curve {
            Curve::Linear => x,
            curve => curve.apply(x / 255.0) * 255.0,
        };
        x.round().clamp(0.0, 255.0) as u8
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

/// 縦横どちらも `max_side` 以下になるよう、整数倍で縮小する。縮小しないで済むときは複製を返す。
///
/// 縮小した 1 画素は、元の `n × n` 画素（端は残りの画素）の平均にする。
/// [`to_display`] の縮小（間引き）を使わないのは、大きく縮める一覧用の縮小画像で
/// 細かい模様がちらついたり、細い線が消えたりするため。
pub fn shrink(img: &DisplayImage, max_side: u32) -> DisplayImage {
    let n = (img.width.max(img.height) as usize)
        .div_ceil(max_side.max(1) as usize)
        .max(1);
    if n == 1 {
        return img.clone();
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let (ow, oh) = (w.div_ceil(n), h.div_ceil(n));
    let mut rgba = vec![0u8; ow * oh * 4];
    let mut sums = vec![0u32; ow * 4];
    for oy in 0..oh {
        sums.fill(0);
        let rows = (oy * n)..((oy + 1) * n).min(h);
        let row_count = rows.len();
        for y in rows {
            let row = &img.rgba[y * w * 4..(y + 1) * w * 4];
            for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
                let sum = &mut sums[(x / n) * 4..(x / n) * 4 + 4];
                for (s, &v) in sum.iter_mut().zip(px) {
                    *s += u32::from(v);
                }
            }
        }
        for ox in 0..ow {
            let cols = ((ox + 1) * n).min(w) - ox * n;
            let count = (cols * row_count) as u32;
            for c in 0..4 {
                let sum = sums[ox * 4 + c];
                rgba[(oy * ow + ox) * 4 + c] = ((sum + count / 2) / count) as u8;
            }
        }
    }
    DisplayImage {
        width: ow as u32,
        height: oh as u32,
        rgba,
    }
}

/// 0..size の値を、`lo` → 0、`hi` → 255 の間を `curve` で写す変換表。範囲外は 0 / 255 に張り付く。
fn build_lut(size: usize, window: (f32, f32), curve: Curve) -> Vec<u8> {
    let shade = Shade::new(window, curve);
    (0..size).map(|v| shade.of(v as f32)).collect()
}

/// NaN や無限大を除いた最小値と最大値。
fn min_max(v: &[f32]) -> (f32, f32) {
    fold_range(v.iter().copied())
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
    fn window_falls_back_to_the_full_range_of_the_samples() {
        let mut img = VwImage::new(2, 1, ColorModel::Gray, PixelData::U16(vec![0, 9])).unwrap();
        img.significant_bits = 12;
        assert_eq!(window_of(&img, &DisplayParams::default()), (0.0, 4095.0));
        img.window = Some((1.0, 5.0));
        assert_eq!(window_of(&img, &DisplayParams::default()), (1.0, 5.0));
        let f = VwImage::new(
            3,
            1,
            ColorModel::Gray,
            PixelData::F32(vec![-2.0, f32::NAN, 7.5]),
        )
        .unwrap();
        assert_eq!(window_of(&f, &DisplayParams::default()), (-2.0, 7.5));
    }

    #[test]
    fn gray_range_skips_alpha_and_missing_values() {
        let img = VwImage::new(
            2,
            1,
            ColorModel::GrayAlpha,
            PixelData::F32(vec![3.0, 100.0, f32::NAN, 0.0]),
        )
        .unwrap();
        assert_eq!(gray_range(&img), Some((3.0, 3.0)));
        let rgb = VwImage::new(1, 1, ColorModel::Rgb, PixelData::U8(vec![1, 2, 3])).unwrap();
        assert_eq!(gray_range(&rgb), None);
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
    fn window_suggested_by_the_file_is_used_unless_overridden() {
        let mut img = gray8(&[100, 150, 200]);
        img.window = Some((100.0, 200.0));
        let d = to_display(&img, &DisplayParams::default());
        let grays: Vec<u8> = d.rgba.iter().step_by(4).copied().collect();
        assert_eq!(grays, vec![0, 128, 255]);
        let params = DisplayParams {
            window: Some((0.0, 255.0)),
            ..Default::default()
        };
        let d = to_display(&img, &params);
        assert_eq!(d.rgba[4], 150);
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
        assert_eq!(&grays[..3], &[0, 128, 255]);
    }

    fn grays_with(curve: Curve, data: PixelData, window: (f32, f32)) -> Vec<u8> {
        let mut img = VwImage::new(data.len() as u32, 1, ColorModel::Gray, data).unwrap();
        img.window = Some(window);
        img.curve = curve;
        to_display(&img, &DisplayParams::default())
            .rgba
            .iter()
            .step_by(4)
            .copied()
            .collect()
    }

    #[test]
    fn curves_keep_black_and_white_and_lift_the_quarter_point() {
        let quarter = |curve| {
            let g = grays_with(curve, PixelData::U16(vec![0, 250, 1000]), (0.0, 1000.0));
            assert_eq!((g[0], g[2]), (0, 255), "{curve:?}");
            g[1]
        };
        assert_eq!(quarter(Curve::Linear), 64);
        assert_eq!(quarter(Curve::Sqrt), 128);
        let (log, asinh) = (quarter(Curve::Log), quarter(Curve::Asinh));
        assert!(log > 128 && asinh > 128, "log {log}, asinh {asinh}");
    }

    #[test]
    fn curves_apply_to_every_sample_type() {
        let window = (0.0, 100.0);
        let u8 = grays_with(Curve::Sqrt, PixelData::U8(vec![25]), window);
        let u16 = grays_with(Curve::Sqrt, PixelData::U16(vec![25]), window);
        let f32 = grays_with(Curve::Sqrt, PixelData::F32(vec![25.0]), window);
        assert_eq!((u8[0], u16[0], f32[0]), (128, 128, 128));
    }

    #[test]
    fn curved_u8_without_a_window_uses_the_full_range() {
        let mut img = gray8(&[0, 64, 255]);
        img.curve = Curve::Sqrt;
        let d = to_display(&img, &DisplayParams::default());
        assert_eq!(
            d.rgba.iter().step_by(4).copied().collect::<Vec<_>>(),
            [0, 128, 255]
        );
    }

    #[test]
    fn constant_image_does_not_divide_by_zero() {
        let img = VwImage::new(2, 1, ColorModel::Gray, PixelData::F32(vec![5.0, 5.0])).unwrap();
        let d = to_display(&img, &DisplayParams::default());
        assert_eq!(d.rgba.len(), 8);
    }

    fn rgba(width: u32, height: u32, px: &[[u8; 4]]) -> DisplayImage {
        DisplayImage {
            width,
            height,
            rgba: px.concat(),
        }
    }

    #[test]
    fn shrink_averages_each_block() {
        let img = rgba(
            2,
            2,
            &[
                [0, 0, 0, 255],
                [100, 0, 0, 255],
                [0, 200, 0, 255],
                [0, 0, 40, 255],
            ],
        );
        let small = shrink(&img, 1);
        assert_eq!((small.width, small.height), (1, 1));
        assert_eq!(small.rgba, [25, 50, 10, 255]);
    }

    #[test]
    fn shrink_averages_partial_blocks_at_the_edges() {
        let img = rgba(3, 1, &[[10, 0, 0, 255], [30, 0, 0, 255], [200, 0, 0, 255]]);
        let small = shrink(&img, 2);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.rgba, [20, 0, 0, 255, 200, 0, 0, 255]);
    }

    #[test]
    fn shrink_fits_the_longer_side_within_max_side() {
        let img = rgba(1000, 10, &vec![[1, 2, 3, 4]; 10_000]);
        let small = shrink(&img, 512);
        assert!(small.width <= 512 && small.height >= 1);
        assert_eq!(small.rgba.len(), (small.width * small.height * 4) as usize);
        assert_eq!(&small.rgba[..4], [1, 2, 3, 4]);
    }

    #[test]
    fn shrink_keeps_small_images_as_they_are() {
        let img = rgba(2, 1, &[[1, 2, 3, 4], [5, 6, 7, 8]]);
        assert_eq!(shrink(&img, 512).rgba, img.rgba);
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
