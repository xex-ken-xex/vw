//! 画像に、保存したパイプラインの値（ファイルの段の値とノードの列）を当てる。
//! ビューアの色導出と、まとめてかけるツールが同じ結果になるよう、ここに 1 つだけ置く。

use vw_core::{ColorModel, PixelData, VwImage};
use vw_dicom::pipeline::Gray;

use crate::nodes::{self, Float};
use crate::pixel::{self, Decoded, Mismatch};
use crate::preset::Values;

/// 当てた結果。色の表はまだ当てていない（灰色のまま）。
pub struct Rendering {
    /// 表示の値（0〜1）。
    pub float: Float,
    /// ファイルの値のままの標準の表示（RGBA）。下描きのときは `float` と同じ大きさに間引いてある。
    pub standard: Vec<u8>,
    /// ファイルの値で描いたものが標準の表示と一致したか（一致しなければ、この画像には当てられない）。
    pub checked: Result<(), Mismatch>,
    /// ノードが当てられなかった（ノードの番号と理由）。
    pub error: Option<(usize, String)>,
    /// ノードが知らせること（ノードの番号と知らせ）。下描きでも元の画素の値に直してある。
    pub notes: Vec<(usize, nodes::Note)>,
    /// 値のない画素。標準の表示と同じく、描いた結果のその画素をこの色で塗る。
    pub missing: Option<Missing>,
}

/// 値のない画素（NaN）と、標準の表示でそこを塗る色（[`VwImage::missing`]）。
pub struct Missing {
    pub color: [u8; 3],
    /// `float` と同じ並びで、値のない画素なら `true`。
    pub pixels: Vec<bool>,
}

impl Missing {
    /// `image` の値のない画素を、`factor` 画素おきに拾う（下描きの [`Float::shrink`] と同じ大きさ）。
    /// 標準の表示が塗らない画像（色のない灰色の F32 以外、塗る色がない、値のない画素がない）は `None`。
    fn of(image: &VwImage, factor: usize) -> Option<Self> {
        let color = image.missing.filter(|_| image.color == ColorModel::Gray)?;
        let PixelData::F32(v) = &image.data else {
            return None;
        };
        let (w, h) = (image.width as usize, image.height as usize);
        let factor = factor.max(1);
        let pixels: Vec<bool> = (0..h)
            .step_by(factor)
            .flat_map(|y| (0..w).step_by(factor).map(move |x| v[y * w + x].is_nan()))
            .collect();
        pixels.iter().any(|&m| m).then_some(Self { color, pixels })
    }
}

/// ノードに渡す最初の値。
enum Start {
    /// ファイルの値の表示の値（標準の表示と同じ。確かめに使ったものをそのまま使う）。
    AtFile,
    /// 図で変えたファイルの段の値で描いた表示の値。
    Display(Float),
    /// 鎖に伸縮ノードがあるときの、伸縮の前の線形の値と、伸縮ノードで当てる伸縮。
    Linear((Float, pixel::Stretch)),
}

/// `image` に `values` を当てる。`info` は DICOM のグレースケールのファイルの段、`window` はスロットで
/// 変えた黒と白（ファイルの VOI の代わり）、`preview_side` は下描きの長辺（縮めて計算する）。
pub fn render(
    image: &VwImage,
    info: Option<&Gray>,
    values: &Values,
    window: Option<(f32, f32)>,
    preview_side: Option<usize>,
) -> Rendering {
    let mut standard = pixel::standard_display(image);
    let split = nodes::split_at_stretch(&values.chain);
    let staged = matches!(split, Ok(Some(_)));
    let (at_file, start) = match info {
        Some(g) => {
            let decoded = Decoded::of(g);
            let file = pixel::file_params(g, image, &decoded);
            let file = match window {
                Some(w) => pixel::with_window(file, image, &decoded, w),
                None => file,
            };
            let at_file = pixel::float_of_params(image, &decoded, &file);
            let p = values.params.unwrap_or(file);
            let start = if staged {
                Start::Linear(pixel::linear_of_params(image, &decoded, &p))
            } else if p != file {
                Start::Display(pixel::float_of_params(image, &decoded, &p))
            } else {
                Start::AtFile
            };
            (at_file, start)
        }
        None => {
            let start = match staged {
                true => Start::Linear(pixel::linear_of_display(image)),
                false => Start::AtFile,
            };
            (pixel::float_of_display(image), start)
        }
    };
    // 標準の表示は値のない画素を決まった色で塗るが、表示の値ではそこが黒になる。塗った画素は
    // 値から描いたものではないので比べない（比べると、値のない画素のある画像に何も当てられない）。
    let checked = pixel::check_standard_except(
        &at_file,
        &standard,
        Missing::of(image, 1).as_ref().map(|m| m.pixels.as_slice()),
    );
    let (mut float, stretch) = match start {
        Start::AtFile => (at_file, None),
        Start::Display(f) => (f, None),
        Start::Linear((f, s)) => (f, Some(s)),
    };
    let mut chain = std::borrow::Cow::Borrowed(&values.chain);
    let mut factor = 1;
    if let Some(side) = preview_side {
        factor = float.shrink([side; 2]);
        standard = pixel::shrink_rgba(&standard, image.width as usize, factor);
        chain = std::borrow::Cow::Owned(chain.iter().map(|n| n.scaled(factor)).collect());
    }
    let mut notes = Vec::new();
    let mut kept = nodes::Kept::for_chain(&chain);
    let error = match nodes::split_at_stretch(&chain) {
        Err(e) => Some(e),
        Ok(None) => nodes::apply_keeping(&chain, &mut float, &mut notes, &mut kept).err(),
        Ok(Some(staged)) => nodes::apply_keeping(staged.before, &mut float, &mut notes, &mut kept)
            .err()
            .or_else(|| {
                if let Some(s) = &stretch {
                    s.apply(&mut float);
                    kept.map(|f| s.apply(f));
                }
                let mut after = Vec::new();
                let error = nodes::apply_keeping(staged.after, &mut float, &mut after, &mut kept)
                    .err()
                    .map(|(i, e)| (i + staged.offset, e));
                notes.extend(after.into_iter().map(|(i, n)| (i + staged.offset, n)));
                error
            }),
    };
    let notes = notes
        .into_iter()
        .map(|(i, n)| (i, n.unscaled(factor)))
        .collect();
    Rendering {
        float,
        standard,
        checked,
        error,
        notes,
        missing: Missing::of(image, factor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 左上の 1 画素だけ値のない、灰色の F32 の画像。値のない画素を `missing` の色で塗る。
    fn with_hole(missing: Option<[u8; 3]>) -> VwImage {
        let mut values: Vec<f32> = (0..16).map(|i| i as f32).collect();
        values[0] = f32::NAN;
        let mut image =
            VwImage::new(4, 4, ColorModel::Gray, PixelData::F32(values)).expect("4x4 gray");
        image.missing = missing;
        image
    }

    #[test]
    fn images_whose_missing_pixels_are_painted_can_take_nodes() {
        let r = render(
            &with_hole(Some([128, 128, 128])),
            None,
            &Values::default(),
            None,
            None,
        );
        assert!(r.checked.is_ok(), "{:?}", r.checked.err());
        let missing = r.missing.expect("the hole is reported");
        assert_eq!(missing.color, [128, 128, 128]);
        assert_eq!(missing.pixels.iter().filter(|&&m| m).count(), 1);
        assert!(missing.pixels[0]);
    }

    #[test]
    fn missing_pixels_are_not_reported_when_the_standard_display_does_not_paint_them() {
        let r = render(&with_hole(None), None, &Values::default(), None, None);
        assert!(r.checked.is_ok());
        assert!(r.missing.is_none());
    }

    #[test]
    fn missing_pixels_follow_the_preview_size() {
        let r = render(
            &with_hole(Some([0, 0, 255])),
            None,
            &Values::default(),
            None,
            Some(2),
        );
        let missing = r.missing.expect("the hole is reported");
        assert_eq!(missing.pixels.len(), r.float.width * r.float.height);
        assert!(missing.pixels[0]);
    }
}
