//! クリップボードへのコピー。どの欄で選ばれた何を、どんな形でコピーするかを決める。
//!
//! 実際にクリップボードへ書くのは egui（`Context::copy_text` / `copy_image`）に任せる。
//! arboard などを直接使わないのは、eframe がすでに同梱していて、Wayland の
//! テキストクリップボードの扱いも済んでいるため。

use eframe::egui::ColorImage;
use vw_core::{display, DisplayParams, VwImage};

/// Ctrl+C の対象になる欄。最後にクリックした欄が対象になる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Metadata,
    /// メタデータ欄の文字列をドラッグで範囲選択している。コピーは egui のラベル選択に任せる。
    MetadataText,
    Images,
}

/// メタデータ欄で選んでいるもの。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetaSelection {
    Group(String),
    /// グループ名と、その中での行の位置。同じ項目名が複数あることがあるので名前では指さない。
    Row(String, usize),
}

/// メタデータの 1 グループ（グループ名と、項目名・値の行）。
pub type MetaGroup = (String, Vec<(String, String)>);

/// 選んでいるグループまたは行を、`項目名<TAB>値` の行にする。選んだものがもうなければ `None`。
pub fn metadata_text(groups: &[MetaGroup], selection: &MetaSelection) -> Option<String> {
    let (name, index) = match selection {
        MetaSelection::Group(g) => (g, None),
        MetaSelection::Row(g, i) => (g, Some(*i)),
    };
    let (_, rows) = groups.iter().find(|(g, _)| g == name)?;
    let rows = match index {
        None => rows.as_slice(),
        Some(i) => std::slice::from_ref(rows.get(i)?),
    };
    Some(tsv(rows))
}

/// 1 行 1 項目のタブ区切り。値の中のタブと改行は空白にする。
/// 表計算ソフトに貼ったときに列や行がずれないようにするため（引用符で囲む CSV 流の
/// 書き方は、貼り付け先によって解釈がまちまちなので採らない）。
pub fn tsv(rows: &[(String, String)]) -> String {
    let clean = |s: &str| s.replace(['\t', '\r', '\n'], " ");
    rows.iter()
        .map(|(k, v)| format!("{}\t{}", clean(k), clean(v)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// コピー用の画像。元画像の全体を縮小せずに RGBA8 にする。
///
/// 表示用のテクスチャを使い回さないのは、GPU の上限に合わせて縮小されていることがあるため。
pub fn full_resolution(image: &VwImage) -> ColorImage {
    let params = DisplayParams {
        max_side: u32::MAX,
        ..DisplayParams::default()
    };
    let d = display::to_display(image, &params);
    ColorImage::from_rgba_unmultiplied([d.width as usize, d.height as usize], &d.rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_core::{ColorModel, PixelData};

    fn groups() -> Vec<MetaGroup> {
        let row = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        vec![
            (
                "File".into(),
                vec![row("Path", "/a/b.png"), row("Size", "12 bytes")],
            ),
            ("PNG".into(), vec![row("Gamma", "0.45")]),
        ]
    }

    #[test]
    fn group_copies_every_row_as_tab_separated_lines() {
        let text = metadata_text(&groups(), &MetaSelection::Group("File".into()));
        assert_eq!(text.as_deref(), Some("Path\t/a/b.png\nSize\t12 bytes"));
    }

    #[test]
    fn row_copies_only_that_row() {
        let text = metadata_text(&groups(), &MetaSelection::Row("File".into(), 1));
        assert_eq!(text.as_deref(), Some("Size\t12 bytes"));
    }

    #[test]
    fn selection_that_no_longer_exists_copies_nothing() {
        let g = groups();
        assert_eq!(
            metadata_text(&g, &MetaSelection::Group("EXIF".into())),
            None
        );
        assert_eq!(
            metadata_text(&g, &MetaSelection::Row("PNG".into(), 1)),
            None
        );
    }

    #[test]
    fn tabs_and_newlines_in_values_become_spaces() {
        let rows = [("Comment".to_owned(), "a\tb\r\nc".to_owned())];
        assert_eq!(tsv(&rows), "Comment\ta b  c");
    }

    #[test]
    fn copied_image_keeps_full_resolution_beyond_the_texture_limit() {
        let width = DisplayParams::default().max_side + 1;
        let image = VwImage::new(
            width,
            1,
            ColorModel::Gray,
            PixelData::U8(vec![128; width as usize]),
        )
        .unwrap();
        let copied = full_resolution(&image);
        assert_eq!(copied.size, [width as usize, 1]);
        assert_eq!(copied.pixels[0], eframe::egui::Color32::from_gray(128));
    }
}
