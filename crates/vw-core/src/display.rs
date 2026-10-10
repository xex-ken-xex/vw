//! Conversion from [`VwImage`] to the RGBA8 buffer that is uploaded as a texture.

use crate::image::{ColorModel, PixelData, VwImage};

/// RGBA8 (non-premultiplied), row-major, ready for GPU upload.
#[derive(Clone, Debug)]
pub struct DisplayImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct DisplayParams {
    /// Sample values mapped to black and white. `None` uses the full range of
    /// the significant bits (U8/U16) or the data's min/max (F32).
    pub window: Option<(f32, f32)>,
    /// Images larger than this on either side are downsampled (GPU texture limit).
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

/// `sample` maps a color sample index to 0..=255; `alpha` does the same for alpha
/// samples, which are never windowed.
fn convert(
    img: &VwImage,
    params: &DisplayParams,
    sample: impl Fn(usize) -> u8 + Sync,
    alpha: impl Fn(usize) -> u8 + Sync,
) -> DisplayImage {
    let ch = img.color.channels();
    let (w, h) = (img.width as usize, img.height as usize);
    // Integer nearest-neighbour step keeps the conversion a single cheap pass.
    let step = (img.width.max(img.height) as usize)
        .div_ceil(params.max_side.max(1) as usize)
        .max(1);
    let (ow, oh) = (w.div_ceil(step), h.div_ceil(step));
    let mut rgba = vec![0u8; ow * oh * 4];

    let convert_rows = |first_row: usize, out: &mut [u8]| {
        for (r, out_row) in out.chunks_exact_mut(ow * 4).enumerate() {
            let row = (first_row + r) * step * w;
            for (ox, px) in out_row.chunks_exact_mut(4).enumerate() {
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

    // Split rows across threads for large images; small ones are not worth the spawn.
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

fn build_lut(size: usize, (lo, hi): (f32, f32)) -> Vec<u8> {
    let scale = if hi > lo { 255.0 / (hi - lo) } else { 0.0 };
    (0..size)
        .map(|v| ((v as f32 - lo) * scale).round().clamp(0.0, 255.0) as u8)
        .collect()
}

fn min_max(v: &[f32]) -> (f32, f32) {
    v.iter()
        .filter(|x| x.is_finite())
        .fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .chunks_exact(4)
            .zip(&data)
            .all(|(px, &g)| px == [g, g, g, 255]));
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
