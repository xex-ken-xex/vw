//! 違いの地図を色にすることと、並べた画像を 1 枚に描くこと（コピーとレポート用）。

use crate::analysis::Analysis;
use crate::plane::Rgb;

/// RGBA8 の画像。
#[derive(Debug, Clone, PartialEq)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// 0〜1 を 黒 → 赤 → 黄 → 白 の色にする（vw の色導出の差のヒートマップと同じ色）。
pub fn heat(t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0) * 3.0;
    let ramp = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    [ramp(t), ramp(t - 1.0), ramp(t - 2.0)]
}

/// 違いの地図。目盛りは違いの最大に合わせる。重ならない所は透明。
pub fn difference(a: &Analysis, width: usize, height: usize) -> Rgba {
    let max = a.difference_max;
    let data = a
        .difference
        .iter()
        .flat_map(|d| match d {
            Some(v) => {
                let [r, g, b] = heat(if max > 0.0 { v / max } else { 0.0 });
                [r, g, b, 255]
            }
            None => [0, 0, 0, 0],
        })
        .collect();
    Rgba {
        width,
        height,
        data,
    }
}

/// SSIM の地図（1 − SSIM を、いちばん低い所が白になるように）。
pub fn ssim(a: &Analysis) -> Option<Rgba> {
    let map = a.ssim_map.as_ref()?;
    let worst = map.values.iter().fold(0.0f32, |m, &v| m.max(1.0 - v));
    let data = map
        .values
        .iter()
        .flat_map(|&v| {
            let [r, g, b] = heat(if worst > 0.0 { (1.0 - v) / worst } else { 0.0 });
            [r, g, b, 255]
        })
        .collect();
    Some(Rgba {
        width: map.width as usize,
        height: map.height as usize,
        data,
    })
}

/// 写した画像を、重ならない所を `background` にして RGBA にする。
pub fn masked(image: &Rgb, valid: &[bool], background: [u8; 3]) -> Rgba {
    let data = image
        .data
        .iter()
        .zip(valid)
        .flat_map(|(p, &ok)| {
            let [r, g, b] = match ok {
                true => p.map(|v| v.round().clamp(0.0, 255.0) as u8),
                false => background,
            };
            [r, g, b, 255]
        })
        .collect();
    Rgba {
        width: image.width,
        height: image.height,
        data,
    }
}

/// `over` を不透明度 `alpha` で `base` に重ねる（`over` の透明な所は `base` のまま）。
pub fn overlay(base: &Rgba, over: &Rgba, alpha: f32) -> Rgba {
    let data = base
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(over.data.as_chunks::<4>().0)
        .flat_map(|(b, o)| {
            let a = alpha * f32::from(o[3]) / 255.0;
            let mix = |x: u8, y: u8| (f32::from(x) * (1.0 - a) + f32::from(y) * a).round() as u8;
            [mix(b[0], o[0]), mix(b[1], o[1]), mix(b[2], o[2]), 255]
        })
        .collect();
    Rgba {
        width: base.width,
        height: base.height,
        data,
    }
}

/// 同じ大きさの画像を、`gap` 画素の隙間を `background` で空けて横に並べる。
pub fn side_by_side(images: &[&Rgba], gap: usize, background: [u8; 3]) -> Rgba {
    let height = images.iter().map(|i| i.height).max().unwrap_or(0);
    let width =
        images.iter().map(|i| i.width).sum::<usize>() + gap * images.len().saturating_sub(1);
    let mut data = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for (k, img) in images.iter().enumerate() {
            if k > 0 {
                for _ in 0..gap {
                    data.extend_from_slice(&[background[0], background[1], background[2], 255]);
                }
            }
            if y < img.height {
                data.extend_from_slice(&img.data[y * img.width * 4..(y + 1) * img.width * 4]);
            } else {
                for _ in 0..img.width {
                    data.extend_from_slice(&[background[0], background[1], background[2], 255]);
                }
            }
        }
    }
    Rgba {
        width,
        height,
        data,
    }
}

/// 左を `left`、右を `right` にした 1 枚（境目は左からの割合 `split`）。
pub fn swipe(left: &Rgba, right: &Rgba, split: f32) -> Rgba {
    let at = (left.width as f32 * split.clamp(0.0, 1.0)).round() as usize;
    let mut data = left.data.clone();
    for y in 0..left.height {
        let row = y * left.width * 4;
        data[row + at * 4..row + left.width * 4]
            .copy_from_slice(&right.data[row + at * 4..row + left.width * 4]);
    }
    Rgba {
        width: left.width,
        height: left.height,
        data,
    }
}

/// PNG にする。
pub fn png(img: &Rgba) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.width as u32, img.height as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    // 書き出しの失敗はメモリ上の Vec では起きない（大きさは揃えて渡している）。
    if let Ok(mut w) = enc.write_header() {
        let _ = w.write_image_data(&img.data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, c: u8) -> Rgba {
        Rgba {
            width: w,
            height: h,
            data: [c, c, c, 255].repeat(w * h),
        }
    }

    #[test]
    fn heat_goes_from_black_through_red_and_yellow_to_white() {
        assert_eq!(heat(0.0), [0, 0, 0]);
        assert_eq!(heat(1.0 / 3.0), [255, 0, 0]);
        assert_eq!(heat(2.0 / 3.0), [255, 255, 0]);
        assert_eq!(heat(1.0), [255, 255, 255]);
    }

    #[test]
    fn images_are_put_side_by_side_with_a_gap() {
        let out = side_by_side(&[&solid(2, 1, 10), &solid(3, 2, 20)], 1, [0, 0, 0]);
        assert_eq!((out.width, out.height), (6, 2));
        assert_eq!(&out.data[..4], &[10, 10, 10, 255]);
        assert_eq!(&out.data[8..12], &[0, 0, 0, 255]);
        assert_eq!(&out.data[12..16], &[20, 20, 20, 255]);
        // 1 枚目は 1 行しかないので、2 行目は背景。
        assert_eq!(&out.data[24..28], &[0, 0, 0, 255]);
    }

    #[test]
    fn the_swipe_takes_the_right_side_from_the_second_image() {
        let out = swipe(&solid(4, 1, 10), &solid(4, 1, 90), 0.5);
        let reds: Vec<u8> = out.data.chunks(4).map(|p| p[0]).collect();
        assert_eq!(reds, [10, 10, 90, 90]);
    }

    #[test]
    fn a_transparent_overlay_leaves_the_base() {
        let base = solid(1, 1, 100);
        let clear = Rgba {
            width: 1,
            height: 1,
            data: vec![255, 0, 0, 0],
        };
        assert_eq!(overlay(&base, &clear, 0.5), base);
        let red = Rgba {
            width: 1,
            height: 1,
            data: vec![200, 0, 0, 255],
        };
        assert_eq!(overlay(&base, &red, 0.5).data, [150, 50, 50, 255]);
    }

    #[test]
    fn a_png_reads_back_with_its_size() {
        let bytes = png(&solid(3, 2, 7));
        let dec = png::Decoder::new(std::io::Cursor::new(bytes));
        let info = dec.read_info().unwrap().info().clone();
        assert_eq!((info.width, info.height), (3, 2));
    }
}
