//! 画像に、保存したパイプラインの値（ファイルの段の値とノードの列）を当てる。
//! ビューアの色導出と、まとめてかけるツールが同じ結果になるよう、ここに 1 つだけ置く。

use vw_core::VwImage;
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
    let checked = pixel::check_standard(&at_file, &standard);
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
    let error = match nodes::split_at_stretch(&chain) {
        Err(e) => Some(e),
        Ok(None) => nodes::apply_noting(&chain, &mut float, &mut notes).err(),
        Ok(Some(staged)) => nodes::apply_noting(staged.before, &mut float, &mut notes)
            .err()
            .or_else(|| {
                if let Some(s) = &stretch {
                    s.apply(&mut float);
                }
                let mut after = Vec::new();
                let error = nodes::apply_noting(staged.after, &mut float, &mut after)
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
    }
}
