//! 見出し（上の帯）と下の帯に出すもの。何を出すかは、設定の並び（[`SlotBar`]）と
//! スロットの今の状態（[`Facts`]）から純粋関数で決め、描く側はその列を順に置くだけにする。
//!
//! 操作のボタンと状態の印を同じ「チップ」として扱うのは、設定画面で同じ仕組み
//! （ドラッグ＆ドロップ）で置き場所を決められるようにするため。見た目は
//! [`Chip::is_indicator`] で分ける（印は弱い文字色で枠なし、ボタンは乗せたときだけ枠）。

use serde::{Deserialize, Serialize};

use egui_phosphor::regular as icon;

/// 見出しか下の帯に置ける 1 つの部品。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Chip {
    Close,
    /// フォルダのスロットの「このフォルダへ移動」。
    OpenFolder,
    FitImage,
    ActualSize,
    DevelopRaw,
    /// 「JPEG」「RAW」の切り替え。
    SwapRawPreview,
    ToggleCalibration,
    /// 「色の表 ▸」。
    ColorMap,
    /// 色導出の「標準 / 保存したパイプライン」の選択。
    DeriveSelect,
    /// 色導出の「Edit」（画素の変換を開く）。
    EditPipeline,
    DeriveCompare,
    DeriveDifference,
    /// 印: カメラ RAW を現像中。
    Developing,
    /// 印: ダーク・フラットを当てているか。
    Calibrated,
    /// 印: 色導出を当てている。
    Derived,
    /// 印: 似た画像の組の番号。
    Similar,
}

/// 置ける辺。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

impl Chip {
    /// 設定画面に並べる順。
    pub const ALL: [Chip; 16] = [
        Chip::Close,
        Chip::OpenFolder,
        Chip::FitImage,
        Chip::ActualSize,
        Chip::DevelopRaw,
        Chip::SwapRawPreview,
        Chip::ToggleCalibration,
        Chip::ColorMap,
        Chip::DeriveSelect,
        Chip::EditPipeline,
        Chip::DeriveCompare,
        Chip::DeriveDifference,
        Chip::Developing,
        Chip::Calibrated,
        Chip::Derived,
        Chip::Similar,
    ];

    /// 状態を示すだけの印（押せない）。
    pub fn is_indicator(self) -> bool {
        matches!(
            self,
            Chip::Developing | Chip::Calibrated | Chip::Derived | Chip::Similar
        )
    }

    /// 見出しに置けるか。一覧を開くもの（色の表、色導出の選択）と見比べの切り替えは
    /// 見出しの高さに収まる絵がないので下の帯だけ。
    pub fn fits(self, edge: Edge) -> bool {
        match edge {
            Edge::Top => !matches!(
                self,
                Chip::ColorMap | Chip::DeriveSelect | Chip::DeriveCompare | Chip::DeriveDifference
            ),
            Edge::Bottom => true,
        }
    }

    /// 画面に出す名前（英語。翻訳は [`crate::i18n`]）。
    pub fn title(self) -> &'static str {
        match self {
            Chip::Close => "Close slot",
            Chip::OpenFolder => "Open this folder",
            Chip::FitImage => "Fit image to slot",
            Chip::ActualSize => "Actual size (100%)",
            Chip::DevelopRaw => "Develop RAW from sensor data",
            Chip::SwapRawPreview => "JPEG / RAW",
            Chip::ToggleCalibration => "Apply dark and flat",
            Chip::ColorMap => "Color map",
            Chip::DeriveSelect => "Derived display",
            Chip::EditPipeline => "Edit",
            Chip::DeriveCompare => "Compare",
            Chip::DeriveDifference => "Difference",
            Chip::Developing => "Developing RAW...",
            Chip::Calibrated => "Dark and flat applied",
            Chip::Derived => "Derived",
            Chip::Similar => "Similar images",
        }
    }

    /// 見出しと、細い帯に出す絵。文字のものは短い。
    pub fn icon(self) -> &'static str {
        match self {
            Chip::Close => "×",
            Chip::OpenFolder => icon::ARROW_SQUARE_IN,
            Chip::FitImage => icon::FRAME_CORNERS,
            Chip::ActualSize => "1:1",
            Chip::DevelopRaw => icon::MAGIC_WAND,
            Chip::SwapRawPreview => "RAW",
            Chip::ToggleCalibration => icon::APERTURE,
            Chip::ColorMap => icon::PALETTE,
            Chip::DeriveSelect => icon::SWATCHES,
            Chip::EditPipeline => icon::SLIDERS_HORIZONTAL,
            Chip::DeriveCompare => icon::COLUMNS,
            Chip::DeriveDifference => icon::MINUS_SQUARE,
            Chip::Developing => icon::FILM_STRIP,
            Chip::Calibrated => icon::APERTURE,
            Chip::Derived => icon::SWATCHES,
            Chip::Similar => icon::LINK_SIMPLE,
        }
    }

    /// 狭いスロットで印を 1 つにまとめるときの順。大きいほど先に残す。
    fn priority(self) -> u8 {
        match self {
            Chip::Developing => 3,
            Chip::Derived => 2,
            Chip::Calibrated => 1,
            _ => 0,
        }
    }
}

impl Chip {
    /// 設定ファイルに書く名前（`swap_raw_preview` のような snake_case）。
    pub fn id(self) -> String {
        match toml::Value::try_from(self) {
            Ok(toml::Value::String(s)) => s,
            _ => unreachable!("Chip serializes as a string"),
        }
    }

    pub fn from_id(id: &str) -> Option<Chip> {
        toml::Value::String(id.to_owned()).try_into().ok()
    }
}

/// 見出しと下の帯の並び（設定 `[slot_bar]`）。見出しの並びは左から右の順。見出しでは、
/// 印は倍率の左、ボタンは倍率の右に分けて置く（[`header_indicators`]・[`header_buttons`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlotBar {
    pub top: Vec<Chip>,
    pub bottom: Vec<Chip>,
}

impl Default for SlotBar {
    fn default() -> Self {
        Self {
            top: vec![
                Chip::Similar,
                Chip::Calibrated,
                Chip::Derived,
                Chip::Developing,
                Chip::OpenFolder,
                Chip::Close,
            ],
            bottom: vec![
                Chip::FitImage,
                Chip::ActualSize,
                Chip::DevelopRaw,
                Chip::SwapRawPreview,
                Chip::ToggleCalibration,
                Chip::ColorMap,
                Chip::DeriveSelect,
                Chip::EditPipeline,
                Chip::DeriveCompare,
                Chip::DeriveDifference,
            ],
        }
    }
}

impl SlotBar {
    pub fn edge(&self, edge: Edge) -> &[Chip] {
        match edge {
            Edge::Top => &self.top,
            Edge::Bottom => &self.bottom,
        }
    }

    pub fn edge_mut(&mut self, edge: Edge) -> &mut Vec<Chip> {
        match edge {
            Edge::Top => &mut self.top,
            Edge::Bottom => &mut self.bottom,
        }
    }

    /// どちらの辺にも置いていない部品。
    pub fn hidden(&self) -> Vec<Chip> {
        Chip::ALL
            .into_iter()
            .filter(|c| !self.top.contains(c) && !self.bottom.contains(c))
            .collect()
    }

    /// 同じ部品を 2 回置かない。置けない辺の部品は外す。外したものを返す（設定の報告用）。
    pub fn tidy(&mut self) -> Vec<Chip> {
        let mut seen = Vec::new();
        let mut dropped = Vec::new();
        for edge in [Edge::Top, Edge::Bottom] {
            self.edge_mut(edge).retain(|&c| {
                let keep = c.fits(edge) && !seen.contains(&c);
                if keep {
                    seen.push(c);
                } else {
                    dropped.push(c);
                }
                keep
            });
        }
        dropped
    }

    /// `chip` を `edge` の `index` 番目に置く（別の場所にあれば外してから）。置けない辺なら何もしない。
    pub fn place(&mut self, chip: Chip, edge: Edge, index: usize) -> bool {
        if !chip.fits(edge) {
            return false;
        }
        self.remove(chip);
        let list = self.edge_mut(edge);
        list.insert(index.min(list.len()), chip);
        true
    }

    pub fn remove(&mut self, chip: Chip) {
        self.top.retain(|&c| c != chip);
        self.bottom.retain(|&c| c != chip);
    }
}

/// スロットの今の状態のうち、部品を出す出さないに関わるもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Facts {
    /// 画像を出している。
    pub image: bool,
    /// まだ開いていないフォルダのスロット。
    pub folder: bool,
    /// 現像できるカメラ RAW で、まだ現像していない。
    pub developable: bool,
    /// 現像したカメラ RAW で、埋め込みプレビューと切り替えられる。
    pub raw_faces: bool,
    /// ダーク・フラットを当てられる画像なら、今当てているか。
    pub calibration: Option<bool>,
    /// 色の表を当てている（灰色の画像）。
    pub color_map: bool,
    /// 色導出を当てられる（画像で、グラフでない）。
    pub derivable: bool,
    /// 色導出を当てている。
    pub derived: bool,
    /// カメラ RAW を現像中。
    pub developing: bool,
    /// 似た画像の組に入っている。
    pub similar: bool,
}

/// 部品 `chip` を今出すか。使えない操作は配置してあっても隠す。
pub fn shows(chip: Chip, f: &Facts) -> bool {
    match chip {
        Chip::Close => true,
        Chip::OpenFolder => f.folder,
        Chip::FitImage | Chip::ActualSize => f.image,
        Chip::DevelopRaw => f.developable,
        Chip::SwapRawPreview => f.raw_faces,
        Chip::ToggleCalibration | Chip::Calibrated => f.calibration.is_some(),
        Chip::ColorMap => f.color_map,
        Chip::DeriveSelect | Chip::EditPipeline => f.derivable,
        Chip::DeriveCompare | Chip::DeriveDifference => f.derivable,
        Chip::Developing => f.developing,
        Chip::Derived => f.derived,
        Chip::Similar => f.similar,
    }
}

/// 辺 `edge` に今出す部品を、設定の順で。
pub fn items(bar: &SlotBar, edge: Edge, f: &Facts) -> Vec<Chip> {
    bar.edge(edge)
        .iter()
        .copied()
        .filter(|&c| shows(c, f))
        .collect()
}

/// 見出しの印。幅 160 px 未満のスロットでは、いちばん大事な 1 つだけにする
/// （残りはその印のツールチップに出す）。
pub fn header_indicators(items: &[Chip], narrow: bool) -> Vec<Chip> {
    let indicators: Vec<Chip> = items.iter().copied().filter(|c| c.is_indicator()).collect();
    if !narrow || indicators.len() <= 1 {
        return indicators;
    }
    let top = indicators
        .iter()
        .copied()
        .max_by_key(|c| c.priority())
        .expect("at least two indicators");
    vec![top]
}

/// 見出しのボタン（印でないもの）。
pub fn header_buttons(items: &[Chip]) -> Vec<Chip> {
    items
        .iter()
        .copied()
        .filter(|c| !c.is_indicator())
        .collect()
}

/// 印を 1 つにまとめる幅の境。
pub const NARROW: f32 = 160.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Facts {
        Facts {
            image: true,
            derivable: true,
            ..Default::default()
        }
    }

    #[test]
    fn every_chip_is_placed_somewhere_by_default() {
        let bar = SlotBar::default();
        assert!(bar.hidden().is_empty(), "{:?}", bar.hidden());
        let mut all: Vec<Chip> = bar.top.iter().chain(&bar.bottom).copied().collect();
        all.sort_by_key(|c| c.id_for_sort());
        let mut expected = Chip::ALL.to_vec();
        expected.sort_by_key(|c| c.id_for_sort());
        assert_eq!(all, expected);
    }

    #[test]
    fn the_default_top_edge_has_only_indicators_and_close_and_open_folder() {
        for c in SlotBar::default().top {
            assert!(
                c.is_indicator() || matches!(c, Chip::Close | Chip::OpenFolder),
                "{c:?}"
            );
        }
    }

    #[test]
    fn unavailable_operations_are_hidden_even_when_placed() {
        let bar = SlotBar::default();
        let f = image();
        let bottom = items(&bar, Edge::Bottom, &f);
        assert!(!bottom.contains(&Chip::DevelopRaw));
        assert!(!bottom.contains(&Chip::SwapRawPreview));
        assert!(!bottom.contains(&Chip::ToggleCalibration));
        assert!(!bottom.contains(&Chip::ColorMap));
        assert!(bottom.contains(&Chip::FitImage));
        assert!(bottom.contains(&Chip::DeriveSelect));
        let raw = Facts {
            developable: true,
            ..image()
        };
        assert!(items(&bar, Edge::Bottom, &raw).contains(&Chip::DevelopRaw));
    }

    #[test]
    fn an_empty_slot_shows_only_close() {
        let bar = SlotBar::default();
        assert_eq!(items(&bar, Edge::Top, &Facts::default()), vec![Chip::Close]);
        assert!(items(&bar, Edge::Bottom, &Facts::default()).is_empty());
    }

    #[test]
    fn a_closed_folder_slot_offers_open_folder() {
        let bar = SlotBar::default();
        let f = Facts {
            folder: true,
            ..Default::default()
        };
        assert_eq!(
            items(&bar, Edge::Top, &f),
            vec![Chip::OpenFolder, Chip::Close]
        );
    }

    #[test]
    fn a_narrow_slot_keeps_the_most_important_indicator() {
        let all = [
            Chip::Similar,
            Chip::Calibrated,
            Chip::Derived,
            Chip::Developing,
        ];
        assert_eq!(header_indicators(&all, true), vec![Chip::Developing]);
        assert_eq!(
            header_indicators(&[Chip::Similar, Chip::Calibrated], true),
            vec![Chip::Calibrated]
        );
        assert_eq!(header_indicators(&all, false).len(), 4);
        assert_eq!(
            header_indicators(&[Chip::Similar], true),
            vec![Chip::Similar]
        );
    }

    #[test]
    fn dropdown_chips_fit_only_the_bottom_edge() {
        for c in [
            Chip::ColorMap,
            Chip::DeriveSelect,
            Chip::DeriveCompare,
            Chip::DeriveDifference,
        ] {
            assert!(!c.fits(Edge::Top), "{c:?}");
            assert!(c.fits(Edge::Bottom), "{c:?}");
        }
        assert!(Chip::FitImage.fits(Edge::Top));
    }

    #[test]
    fn placing_a_chip_moves_it_and_refuses_an_edge_it_does_not_fit() {
        let mut bar = SlotBar::default();
        assert!(bar.place(Chip::FitImage, Edge::Top, 0));
        assert_eq!(bar.top[0], Chip::FitImage);
        assert!(!bar.bottom.contains(&Chip::FitImage));
        assert!(!bar.place(Chip::ColorMap, Edge::Top, 0));
        assert!(bar.bottom.contains(&Chip::ColorMap));
        assert!(bar.place(Chip::Close, Edge::Bottom, 99));
        assert_eq!(bar.bottom.last(), Some(&Chip::Close));
    }

    #[test]
    fn tidy_drops_duplicates_and_misplaced_chips() {
        let mut bar = SlotBar {
            top: vec![Chip::Close, Chip::ColorMap, Chip::Close],
            bottom: vec![Chip::Close, Chip::FitImage],
        };
        let dropped = bar.tidy();
        assert_eq!(bar.top, vec![Chip::Close]);
        assert_eq!(bar.bottom, vec![Chip::FitImage]);
        assert_eq!(dropped, vec![Chip::ColorMap, Chip::Close, Chip::Close]);
    }

    #[test]
    fn chip_ids_round_trip() {
        for c in Chip::ALL {
            assert_eq!(Chip::from_id(&c.id()), Some(c), "{c:?}");
        }
        assert_eq!(Chip::from_id("nonsense"), None);
    }

    #[test]
    fn chip_names_in_the_settings_file_are_snake_case() {
        assert_eq!(
            toml::to_string(&SlotBar {
                top: vec![Chip::SwapRawPreview],
                bottom: vec![]
            })
            .unwrap()
            .trim(),
            "top = [\"swap_raw_preview\"]\nbottom = []"
        );
    }

    impl Chip {
        fn id_for_sort(self) -> usize {
            Chip::ALL.iter().position(|&c| c == self).unwrap()
        }
    }
}
