//! 画素の変換の、画面に出す部分（egui の画像、差のヒートマップ、ファイルの段の読み込み）。
//! 値と計算は [`vw_pipeline::pixel`] にある。

use eframe::egui::{Color32, ColorImage};
use vw_dicom::pipeline::Pipeline;
pub use vw_pipeline::pixel::*;

use crate::frames;
use crate::nodes::Float;

/// `float` を丸めた表示と、標準の表示のアルファを合わせた画像。
pub fn to_color_image(float: &Float, standard_rgba: &[u8]) -> ColorImage {
    let rgb = float.to_rgb8();
    let pixels = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .zip(standard_rgba.as_chunks::<4>().0)
        .map(|(c, s)| Color32::from_rgba_premultiplied(c[0], c[1], c[2], s[3]))
        .collect();
    ColorImage::new([float.width, float.height], pixels)
}

/// 変更後と標準の明るさの差 |変更後 − 標準|（RGB の最大）のヒートマップと、差の最大。
/// 色の目盛りは差の最大に合わせる（差が小さくても見えるように）。
pub fn difference(derived: &ColorImage, standard_rgba: &[u8]) -> (ColorImage, u8) {
    let diffs: Vec<u8> = derived
        .pixels
        .iter()
        .zip(standard_rgba.as_chunks::<4>().0)
        .map(|(c, s)| {
            c.r()
                .abs_diff(s[0])
                .max(c.g().abs_diff(s[1]))
                .max(c.b().abs_diff(s[2]))
        })
        .collect();
    let max = diffs.iter().copied().max().unwrap_or(0);
    let pixels = diffs
        .iter()
        .map(|&d| {
            heat(if max == 0 {
                0.0
            } else {
                f32::from(d) / f32::from(max)
            })
        })
        .collect();
    (ColorImage::new(derived.size, pixels), max)
}
/// 0..=1 を 黒 → 赤 → 黄 → 白 の色にする。
pub fn heat(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0) * 3.0;
    let ramp = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(ramp(t), ramp(t - 1.0), ramp(t - 2.0))
}

/// ファイルの段（DICOM のヘッダ）を裏のスレッドで読む。画像の画素は別に届くので、ここでは読まない。
pub fn read_in_background(
    ctx: &eframe::egui::Context,
    path: &std::path::Path,
) -> std::sync::mpsc::Receiver<Result<Pipeline, String>> {
    let path = path.to_path_buf();
    crate::background::try_compute(ctx, move || read(&path))
}

/// ファイルの段を読む（フレームのパスなら、そのフレームの段）。
pub fn read(path: &std::path::Path) -> Result<Pipeline, String> {
    let (file, frame) = frames::split(path).unwrap_or_else(|| (path.to_path_buf(), 0));
    std::fs::read(&file)
        .map_err(|e| e.to_string())
        .and_then(|bytes| vw_dicom::pipeline::read(&bytes, frame).map_err(|e| e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn difference_scales_the_heat_to_the_largest_change() {
        let derived = ColorImage::new(
            [3, 1],
            vec![
                Color32::from_gray(10),
                Color32::from_gray(60),
                Color32::from_gray(100),
            ],
        );
        let standard = |g: [u8; 3]| -> Vec<u8> { g.iter().flat_map(|&v| [v, v, v, 255]).collect() };
        let (heatmap, max) = difference(&derived, &standard([10, 40, 140]));
        assert_eq!(max, 40);
        assert_eq!(heatmap.pixels[0], Color32::BLACK);
        assert_eq!(heatmap.pixels[2], Color32::WHITE);
        let (none, zero) = difference(&derived, &standard([10, 60, 100]));
        assert_eq!(zero, 0);
        assert!(none.pixels.iter().all(|&c| c == Color32::BLACK));
    }

    #[test]
    fn heat_goes_from_black_through_red_and_yellow_to_white() {
        assert_eq!(heat(0.0), Color32::BLACK);
        assert_eq!(heat(1.0 / 3.0), Color32::from_rgb(255, 0, 0));
        assert_eq!(heat(2.0 / 3.0), Color32::from_rgb(255, 255, 0));
        assert_eq!(heat(1.0), Color32::WHITE);
    }
}
