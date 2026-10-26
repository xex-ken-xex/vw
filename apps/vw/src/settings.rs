//! 設定の中身と、設定ファイル（TOML）の読み書き。
//!
//! 設定ファイルは設定画面で値を変えるたびに書き出す。ファイルが外部のエディタで
//! 書き換えられたら読み直して、すぐ画面に反映する。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

use crate::keymap::Keymap;
use crate::layout::{SlotLayout, WheelScroll};
use crate::slideshow::{EndMode, DROP_WINDOW_RANGE, FPS_RANGE};
use crate::sort::{SortKey, SortOrder};
use crate::toml_reader::Reader;
use crate::viewer::ScaleFilter;
use vw_quality::{Metric, Thresholds};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// OS のダーク／ライト設定に従う。
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "ja")]
    Japanese,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::English, Language::Japanese];

    /// 言語の選択肢に出す名前。どの言語で表示中でも選べるよう、その言語自身で書く。
    pub fn native_name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Japanese => "日本語",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Appearance {
    pub theme: ThemeChoice,
    pub language: Language,
    /// 文字やアイコンの拡大率。
    pub ui_scale: f32,
    /// 余白を詰めて表示する。
    pub compact: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            language: Language::English,
            ui_scale: 1.0,
            compact: true,
        }
    }
}

impl Appearance {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("appearance");
        let scale_range = Appearance::MIN_SCALE..=Appearance::MAX_SCALE;
        Self {
            theme: r.get("theme").unwrap_or(d.theme),
            language: r.get("language").unwrap_or(d.language),
            ui_scale: r
                .get_valid(
                    "ui_scale",
                    |v: &f32| scale_range.contains(v),
                    "must be 0.75 to 2.0",
                )
                .unwrap_or(d.ui_scale),
            compact: r.get("compact").unwrap_or(d.compact),
        }
    }
}

impl Appearance {
    pub const MIN_SCALE: f32 = 0.75;
    pub const MAX_SCALE: f32 = 2.0;

    pub fn clamped_scale(&self) -> f32 {
        if self.ui_scale.is_finite() {
            self.ui_scale.clamp(Self::MIN_SCALE, Self::MAX_SCALE)
        } else {
            1.0
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Viewer {
    /// 画像の背景色（`#RRGGBB`）。空ならテーマの既定色。
    pub background: String,
    /// フル解像度の画像に使うメモリの上限（MB）。超えた分は縮小画像で表示する。
    pub full_image_limit_mb: u32,
    /// 1 枚の画像をデコードするのに使ってよいメモリの上限（MB）。見出しにこれより大きな幅・高さを
    /// 書いたファイルは開かない。
    pub image_size_limit_mb: u32,
    /// スロットの並べ方。
    pub layout: SlotLayout,
    /// メニューの「カスタム」で選ぶマス目。`auto` は選べない（既定の項目にあるため）。
    pub custom_layout: SlotLayout,
    /// 自動レイアウトで並べるマス目の上限。超える数のスロットはページに分ける。
    /// 大量に読み込んでも 1 枠が小さくなりすぎないようにするため。
    pub auto_layout_max: SlotLayout,
    /// ページの帯の上のホイールと、スロットの上の Shift+ホイールで送る量。
    pub page_wheel: WheelScroll,
    /// スライドショーの速さ。再生バーで最後に使った値を覚える。
    pub slideshow_fps: f32,
    pub slideshow_end: EndMode,
    /// 再生バーのコマ落ちの数を、直近の何秒の分で数えるか。
    pub slideshow_drop_window_secs: u32,
    /// 複数枚のスロットで前後の画像に送るとき、倍率と位置を引き継ぐ（DICOM は常に引き継ぐ）。
    pub keep_view: bool,
    /// カメラ RAW を開いたら、まず埋め込みプレビューを出し、裏で現像して差し替える。
    pub develop_raw: bool,
    /// 画像を拡大して表示するときの補間。最近傍なら画素単位で確かめられる。
    pub magnify_filter: ScaleFilter,
    /// 画像を縮小して表示するときの補間。
    pub minify_filter: ScaleFilter,
}

impl Viewer {
    pub fn texture_options(&self) -> egui::TextureOptions {
        crate::viewer::texture_options(self.magnify_filter, self.minify_filter)
    }
}

impl Viewer {
    pub const LIMIT_RANGE_MB: std::ops::RangeInclusive<u32> = 256..=1_048_576;
    pub const IMAGE_SIZE_RANGE_MB: std::ops::RangeInclusive<u32> = 64..=1_048_576;

    pub fn full_image_limit_bytes(&self) -> usize {
        self.full_image_limit_mb as usize * 1024 * 1024
    }

    pub fn image_size_limit_bytes(&self) -> usize {
        self.image_size_limit_mb as usize * 1024 * 1024
    }
}

impl Default for Viewer {
    fn default() -> Self {
        Self {
            background: String::new(),
            full_image_limit_mb: 256,
            image_size_limit_mb: (vw_core::limits::DEFAULT_MAX_IMAGE_BYTES >> 20) as u32,
            layout: SlotLayout::Auto,
            custom_layout: SlotLayout::grid(6, 6),
            auto_layout_max: SlotLayout::grid(6, 6),
            page_wheel: WheelScroll::Page,
            slideshow_fps: 30.0,
            slideshow_end: EndMode::Loop,
            slideshow_drop_window_secs: 5,
            keep_view: true,
            develop_raw: true,
            magnify_filter: ScaleFilter::Nearest,
            minify_filter: ScaleFilter::Linear,
        }
    }
}

impl Viewer {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("viewer");
        Self {
            background: r
                .get_valid(
                    "background",
                    |v: &String| v.trim().is_empty() || parse_hex_color(v).is_some(),
                    "must be #RRGGBB or empty",
                )
                .unwrap_or(d.background),
            full_image_limit_mb: r
                .get_valid(
                    "full_image_limit_mb",
                    |v: &u32| Viewer::LIMIT_RANGE_MB.contains(v),
                    "must be 256 to 1048576",
                )
                .unwrap_or(d.full_image_limit_mb),
            image_size_limit_mb: r
                .get_valid(
                    "image_size_limit_mb",
                    |v: &u32| Viewer::IMAGE_SIZE_RANGE_MB.contains(v),
                    "must be 64 to 1048576",
                )
                .unwrap_or(d.image_size_limit_mb),
            layout: r.get("layout").unwrap_or(d.layout),
            custom_layout: r
                .get_valid(
                    "custom_layout",
                    |v: &SlotLayout| *v != SlotLayout::Auto,
                    "must be COLSxROWS",
                )
                .unwrap_or(d.custom_layout),
            auto_layout_max: r
                .get_valid(
                    "auto_layout_max",
                    |v: &SlotLayout| *v != SlotLayout::Auto,
                    "must be COLSxROWS",
                )
                .unwrap_or(d.auto_layout_max),
            page_wheel: r.get("page_wheel").unwrap_or(d.page_wheel),
            slideshow_fps: r
                .get_valid(
                    "slideshow_fps",
                    |v: &f32| FPS_RANGE.contains(v),
                    "must be 0.1 to 120",
                )
                .unwrap_or(d.slideshow_fps),
            slideshow_end: r.get("slideshow_end").unwrap_or(d.slideshow_end),
            slideshow_drop_window_secs: r
                .get_valid(
                    "slideshow_drop_window_secs",
                    |v: &u32| DROP_WINDOW_RANGE.contains(v),
                    "must be 1 to 60",
                )
                .unwrap_or(d.slideshow_drop_window_secs),
            keep_view: r.get("keep_view").unwrap_or(d.keep_view),
            develop_raw: r.get("develop_raw").unwrap_or(d.develop_raw),
            magnify_filter: r.get("magnify_filter").unwrap_or(d.magnify_filter),
            minify_filter: r.get("minify_filter").unwrap_or(d.minify_filter),
        }
    }
}

/// グループの見出し（メタデータ欄の File など）をクリックしたときの動作。
/// 開閉の ▶ はどちらでも開閉する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderClick {
    /// 選ぶだけで開閉しない。コピーしようとして選ぶたびに閉じてしまわないようにする。
    Select,
    #[default]
    SelectAndToggle,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Behavior {
    pub header_click: HeaderClick,
    /// 開いているフォルダを監視し、新しい画像や書き換えられた画像をすぐ表示に反映する。
    pub auto_reload: bool,
    /// ZIP や 7z を展開するフォルダ。空なら OS の一時フォルダ。
    pub archive_dir: String,
    /// 確かめずに展開してよい大きさ（GB）。超えるアーカイブは展開する前に尋ねる。
    pub archive_size_limit_gb: u32,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            header_click: HeaderClick::default(),
            auto_reload: true,
            archive_dir: String::new(),
            archive_size_limit_gb: crate::archive::DEFAULT_SIZE_LIMIT_GIB,
        }
    }
}

impl Behavior {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("behavior");
        Self {
            header_click: r.get("header_click").unwrap_or(d.header_click),
            auto_reload: r.get("auto_reload").unwrap_or(d.auto_reload),
            archive_dir: r
                .get_valid(
                    "archive_dir",
                    |v: &String| Behavior::valid_archive_dir(v),
                    "must be empty or an absolute folder path",
                )
                .unwrap_or(d.archive_dir),
            archive_size_limit_gb: r
                .get_valid(
                    "archive_size_limit_gb",
                    |v: &u32| Behavior::ARCHIVE_SIZE_RANGE_GB.contains(v),
                    "must be 1 to 1048576",
                )
                .unwrap_or(d.archive_size_limit_gb),
        }
    }

    pub const ARCHIVE_SIZE_RANGE_GB: std::ops::RangeInclusive<u32> = 1..=1_048_576;

    pub fn archive_size_limit_bytes(&self) -> u64 {
        u64::from(self.archive_size_limit_gb) << 30
    }

    /// 相対パスを受け付けないのは、起動した場所によって展開先が変わってしまうため。
    pub fn valid_archive_dir(dir: &str) -> bool {
        let dir = dir.trim();
        dir.is_empty() || Path::new(dir).is_absolute()
    }

    /// ZIP や 7z の展開先の親。`None` は OS の一時フォルダ。
    pub fn archive_parent(&self) -> Option<PathBuf> {
        let dir = self.archive_dir.trim();
        (!dir.is_empty()).then(|| PathBuf::from(dir))
    }
}

/// メタデータ欄のパスの行に出すもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathDisplay {
    /// フルパス。欄に収まらなければ頭を省略する。
    #[default]
    Full,
    /// ファイル名だけ。
    Name,
}

/// メタデータ欄のグループの見出しの見た目。太字の書体は入れていないので、色と下線で見分ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadingStyle {
    /// アクセント色の文字。
    #[default]
    Color,
    Underline,
    Both,
    /// 項目と同じ文字。
    Plain,
}

/// 色の値の書き方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorFormat {
    /// `R 128 G 64 B 255`
    #[default]
    Rgb,
    /// `#8040FF`（16 bit の画像は各色 4 桁）
    Hex,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetadataSettings {
    pub path: PathDisplay,
    /// 隠す項目（`グループ/項目`）とグループ（`グループ`）。言語を切り替えても効くよう、英語の名前で持つ。
    pub hidden: Vec<String>,
    pub heading_style: HeadingStyle,
    /// グループの開閉を次の起動でも同じにする。
    pub remember_open: bool,
    /// 十字（ピクセルプローブ）の値。
    pub color_format: ColorFormat,
    /// 自分で作ったグループ。欄の一番上に出す。キーは `グループ/項目`。
    /// TOML では表の配列を最後に書く必要があるので、最後に置く。
    pub groups: Vec<SearchGroup>,
}

impl Default for MetadataSettings {
    fn default() -> Self {
        Self {
            path: PathDisplay::default(),
            hidden: Vec::new(),
            heading_style: HeadingStyle::default(),
            remember_open: true,
            color_format: ColorFormat::default(),
            groups: Vec::new(),
        }
    }
}

impl MetadataSettings {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("metadata");
        Self {
            path: r.get("path").unwrap_or(d.path),
            hidden: r.get("hidden").unwrap_or_default(),
            heading_style: r.get("heading_style").unwrap_or(d.heading_style),
            remember_open: r.get("remember_open").unwrap_or(d.remember_open),
            color_format: r.get("color_format").unwrap_or(d.color_format),
            groups: read_groups(&mut r, "groups").unwrap_or_default(),
        }
    }

    /// `id`（`グループ` か `グループ/項目`）を隠しているか。
    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden.iter().any(|h| h == id)
    }
}

/// ROI（[`crate::roi`]）。
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct RoiSettings {
    /// カラー画像の平均の色。
    pub color_format: ColorFormat,
}

impl RoiSettings {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("roi");
        Self {
            color_format: r.get("color_format").unwrap_or(d.color_format),
        }
    }
}

/// 名前とキーの組の並びを読む。誤った組だけを外し、残りは使えるようにする。
fn read_groups(r: &mut crate::toml_reader::Section<'_, '_>, key: &str) -> Option<Vec<SearchGroup>> {
    let groups = r.get::<Vec<SearchGroup>>(key)?;
    Some(
        groups
            .into_iter()
            .filter(|g| match g.problem() {
                Some(p) => {
                    r.problem(key, format!("{:?}: {p} (ignored)", g.name));
                    false
                }
                None => true,
            })
            .collect(),
    )
}

/// ログ欄の見た目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStyle {
    /// 時刻と本文。文字は普通の字体で、長い行は折り返す。
    #[default]
    Normal,
    /// 黒地に等幅の字体で、起動からの経過秒と本文。長い行は折り返さず横に流す。
    Terminal,
}

impl LogStyle {
    pub const ALL: [LogStyle; 2] = [LogStyle::Normal, LogStyle::Terminal];

    pub fn title(self) -> &'static str {
        match self {
            LogStyle::Normal => "Normal",
            LogStyle::Terminal => "Terminal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogSettings {
    pub style: LogStyle,
    /// ログをファイルにも書く。
    pub file: bool,
    /// 残すログファイルの数（起動 1 回で 1 ファイル）。
    pub keep_files: u32,
    /// 長い行を欄の幅で折り返す。
    pub wrap: bool,
}

impl LogSettings {
    pub const KEEP_RANGE: std::ops::RangeInclusive<u32> = 1..=1000;
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            style: LogStyle::Normal,
            file: true,
            keep_files: 20,
            wrap: true,
        }
    }
}

impl LogSettings {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("log");
        Self {
            style: r.get("style").unwrap_or(d.style),
            file: r.get("file").unwrap_or(d.file),
            keep_files: r
                .get_valid(
                    "keep_files",
                    |v: &u32| LogSettings::KEEP_RANGE.contains(v),
                    "must be 1 to 1000",
                )
                .unwrap_or(d.keep_files),
            wrap: r.get("wrap").unwrap_or(d.wrap),
        }
    }
}

/// 定量評価（[`crate::quality`]）の判定の境目。指標ごとに「適合」と「注意」の下限。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Quality {
    pub psnr: Thresholds,
    pub ssim: Thresholds,
    pub snr: Thresholds,
    pub cnr: Thresholds,
    pub sharpness: Thresholds,
}

impl Default for Quality {
    fn default() -> Self {
        Self {
            psnr: Metric::Psnr.default_thresholds(),
            ssim: Metric::Ssim.default_thresholds(),
            snr: Metric::Snr.default_thresholds(),
            cnr: Metric::Cnr.default_thresholds(),
            sharpness: Metric::Sharpness.default_thresholds(),
        }
    }
}

impl Quality {
    fn read(r: &mut Reader<'_>) -> Self {
        let mut r = r.at("quality");
        let mut q = Self::default();
        for m in Metric::ALL {
            if let Some(t) = r.get_valid(
                m.key(),
                |t: &Thresholds| t.valid(),
                "good and caution must be numbers with good >= caution",
            ) {
                *q.get_mut(m) = t;
            }
        }
        q
    }

    pub fn get(&self, m: Metric) -> Thresholds {
        match m {
            Metric::Psnr => self.psnr,
            Metric::Ssim => self.ssim,
            Metric::Snr => self.snr,
            Metric::Cnr => self.cnr,
            Metric::Sharpness => self.sharpness,
        }
    }

    pub fn get_mut(&mut self, m: Metric) -> &mut Thresholds {
        match m {
            Metric::Psnr => &mut self.psnr,
            Metric::Ssim => &mut self.ssim,
            Metric::Snr => &mut self.snr,
            Metric::Cnr => &mut self.cnr,
            Metric::Sharpness => &mut self.sharpness,
        }
    }
}

/// cd したフォルダの画像をスロットにどう入れるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadInto {
    /// 1 枚の画像を 1 スロットに。
    #[default]
    Images,
    /// 1 つのフォルダを 1 スロットに。スロットの中で前後の画像に送る。
    Folders,
}

impl LoadInto {
    pub fn other(self) -> Self {
        match self {
            LoadInto::Images => LoadInto::Folders,
            LoadInto::Folders => LoadInto::Images,
        }
    }
}

/// 押しながら読み込むと、[`LoadInto`] の設定と逆の入れ方になる修飾キー。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadModifier {
    #[default]
    Alt,
    Ctrl,
    None,
}

impl LoadModifier {
    pub fn held(self, modifiers: eframe::egui::Modifiers) -> bool {
        match self {
            LoadModifier::Alt => modifiers.alt,
            LoadModifier::Ctrl => modifiers.ctrl,
            LoadModifier::None => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tree {
    pub sort: SortKey,
    pub sort_descending: bool,
    /// 読み込む枚数が区切りに達するたびに、続けるか確認する。
    pub confirm_every: bool,
    /// 確認する区切りの枚数。
    pub confirm_chunk: u32,
    pub load_into: LoadInto,
    pub other_load_modifier: LoadModifier,
    /// フォルダ名のクリックで cd するとともに、そのフォルダを開く（エクスプローラと同じ）。
    pub click_expands: bool,
}

impl Default for Tree {
    fn default() -> Self {
        Self {
            sort: SortKey::Modified,
            sort_descending: true,
            confirm_every: false,
            confirm_chunk: 500,
            load_into: LoadInto::default(),
            other_load_modifier: LoadModifier::default(),
            click_expands: true,
        }
    }
}

impl Tree {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("tree");
        Self {
            sort: r.get("sort").unwrap_or(d.sort),
            sort_descending: r.get("sort_descending").unwrap_or(d.sort_descending),
            confirm_every: r.get("confirm_every").unwrap_or(d.confirm_every),
            confirm_chunk: r
                .get_valid(
                    "confirm_chunk",
                    |v: &u32| Tree::CHUNK_RANGE.contains(v),
                    "must be 1 to 100000",
                )
                .unwrap_or(d.confirm_chunk),
            load_into: r.get("load_into").unwrap_or(d.load_into),
            other_load_modifier: r
                .get("other_load_modifier")
                .unwrap_or(d.other_load_modifier),
            click_expands: r.get("click_expands").unwrap_or(d.click_expands),
        }
    }
}

impl Tree {
    pub const CHUNK_RANGE: std::ops::RangeInclusive<u32> = 1..=100_000;

    pub fn order(&self) -> SortOrder {
        SortOrder {
            key: self.sort,
            descending: self.sort_descending,
        }
    }
}

/// フラグとタグ（[`crate::labels`]）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Labels {
    /// 付けられるタグ。先頭から 9 個が数字キー 1〜9 に対応する。
    pub tags: Vec<String>,
    /// フラグやタグを付けたら次の画像へ進む。
    pub advance: bool,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            tags: Vec::new(),
            advance: true,
        }
    }
}

impl Labels {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("labels");
        Self {
            tags: r
                .get_valid(
                    "tags",
                    |t: &Vec<String>| Labels::valid_tags(t),
                    "tag names must be non-empty and unique",
                )
                .unwrap_or_else(|| d.tags.clone()),
            advance: r.get("advance").unwrap_or(d.advance),
        }
    }
}

impl Labels {
    /// タグの一覧として使えるか。空の名前や重なった名前があると、キーとタグの対応が分からなくなる。
    pub fn valid_tags(tags: &[String]) -> bool {
        let mut seen = std::collections::HashSet::new();
        tags.iter()
            .all(|t| !t.trim().is_empty() && t.trim() == t && seen.insert(t.as_str()))
    }
}

/// DICOM（[`vw_dicom`]）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Dicom {
    /// 先頭を読んで DICOM か確かめる拡張子の正規表現（[`vw_core::ProbeExtensions`]）。
    pub probe_extensions: String,
}

impl Default for Dicom {
    fn default() -> Self {
        Self {
            probe_extensions: vw_core::ProbeExtensions::DEFAULT.into(),
        }
    }
}

impl Dicom {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("dicom");
        Self {
            probe_extensions: r
                .get_valid(
                    "probe_extensions",
                    |e: &String| vw_core::ProbeExtensions::new(e).is_ok(),
                    "not a valid regular expression",
                )
                .unwrap_or_else(|| d.probe_extensions.clone()),
        }
    }
}

impl Dicom {
    pub fn probe(&self) -> vw_core::ProbeExtensions {
        vw_core::ProbeExtensions::new(&self.probe_extensions).unwrap_or_default()
    }
}

/// 天体の画像（[`vw_fits`]）の、最初の黒と白の決め方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitsStretch {
    MinMax,
    #[default]
    Percentile,
    Zscale,
}

impl FitsStretch {
    pub const ALL: [FitsStretch; 3] = [
        FitsStretch::Percentile,
        FitsStretch::Zscale,
        FitsStretch::MinMax,
    ];

    pub fn title(self) -> &'static str {
        match self {
            FitsStretch::MinMax => "Minimum to maximum",
            FitsStretch::Percentile => "Percentiles",
            FitsStretch::Zscale => "zscale",
        }
    }
}

/// 天体の画像の、黒と白の間の明るさの付け方（[`vw_core::Curve`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitsCurve {
    #[default]
    Linear,
    Sqrt,
    Log,
    Asinh,
}

impl FitsCurve {
    pub const ALL: [FitsCurve; 4] = [
        FitsCurve::Linear,
        FitsCurve::Asinh,
        FitsCurve::Log,
        FitsCurve::Sqrt,
    ];

    pub fn title(self) -> &'static str {
        match self {
            FitsCurve::Linear => "Linear",
            FitsCurve::Sqrt => "Square root",
            FitsCurve::Log => "Logarithm",
            FitsCurve::Asinh => "asinh",
        }
    }

    fn curve(self) -> vw_core::Curve {
        match self {
            FitsCurve::Linear => vw_core::Curve::Linear,
            FitsCurve::Sqrt => vw_core::Curve::Sqrt,
            FitsCurve::Log => vw_core::Curve::Log,
            FitsCurve::Asinh => vw_core::Curve::Asinh,
        }
    }
}

/// 色の表のゲージを出す場所（スロットの中の端）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GaugeSide {
    #[default]
    Right,
    Top,
    Bottom,
}

impl GaugeSide {
    pub const ALL: [GaugeSide; 3] = [GaugeSide::Right, GaugeSide::Top, GaugeSide::Bottom];

    pub fn title(self) -> &'static str {
        match self {
            GaugeSide::Right => "Right",
            GaugeSide::Top => "Top",
            GaugeSide::Bottom => "Bottom",
        }
    }
}

/// 1 つの種類の画像に、色の表の段を初めから入れるかと、その色。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ColorNode {
    pub color_node: bool,
    /// [`vw_core::palette::BUILTIN`] の名前か、[`PipelineSettings::palettes`] の名前。
    pub palette: String,
}

impl ColorNode {
    fn new(color_node: bool, palette: &str) -> Self {
        Self {
            color_node,
            palette: palette.to_owned(),
        }
    }

    /// `[section]` の 2 項目。表の名前は、組み込みか `custom` にあるものだけを受け付ける。
    fn read(r: &mut Reader<'_>, section: &'static str, d: &Self, custom: &[CustomPalette]) -> Self {
        let mut r = r.at(section);
        let known = |name: &String| {
            vw_core::palette::BUILTIN.contains(&name.as_str())
                || custom.iter().any(|c| &c.name == name)
        };
        Self {
            color_node: r.get("color_node").unwrap_or(d.color_node),
            palette: r
                .get_valid("palette", known, "must be a built-in or custom color map")
                .unwrap_or_else(|| d.palette.clone()),
        }
    }
}

/// DICOM の画像。PT はほかと既定が違うので分けて持つ。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DicomColorNodes {
    pub color_node: bool,
    pub palette: String,
    /// TOML では表になるので、値の項目より後に置く。
    pub pt: ColorNode,
}

/// 設定で足した色の表。色の点は「位置 #rrggbb」（[`vw_core::palette::parse_stop`]）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CustomPalette {
    pub name: String,
    pub colors: Vec<String>,
}

impl CustomPalette {
    /// 設定画面での 1 行の区切り。色の区切りにカンマを使わないのは、位置の小数点をカンマで書く人がいるため。
    const NAME_END: char = '=';
    const COLOR_SEPARATOR: char = ';';

    pub fn palette(&self) -> Result<vw_core::Palette, String> {
        if self.name.trim().is_empty() {
            return Err("name is empty".to_owned());
        }
        if vw_core::palette::BUILTIN.contains(&self.name.as_str()) {
            return Err("name is a built-in color map".to_owned());
        }
        let stops = self
            .colors
            .iter()
            .map(|c| vw_core::palette::parse_stop(c))
            .collect::<Result<Vec<_>, _>>()?;
        vw_core::Palette::from_stops(&self.name, &stops)
    }

    /// `name = 0 #000000; 1 #ffffff` の形の 1 行を読む。
    pub fn parse_line(line: &str) -> Option<Self> {
        let (name, colors) = line.split_once(Self::NAME_END)?;
        let custom = Self {
            name: name.trim().to_owned(),
            colors: colors
                .split(Self::COLOR_SEPARATOR)
                .map(|c| c.trim().to_owned())
                .filter(|c| !c.is_empty())
                .collect(),
        };
        custom.palette().is_ok().then_some(custom)
    }

    pub fn to_line(&self) -> String {
        let sep = format!("{} ", Self::COLOR_SEPARATOR);
        format!(
            "{} {} {}",
            self.name,
            Self::NAME_END,
            self.colors.join(&sep)
        )
    }
}

/// 画素の変換（`[pipeline]`）。どの種類の画像に、どの段を初めから入れるか。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PipelineSettings {
    /// 種類ごとの `color_node` に従って、色の表の段を初めから入れる。外すと、どの画像も
    /// スロットで選ぶまで灰色のまま。
    pub color_node: bool,
    pub gauge_side: GaugeSide,
    /// 色の表を当てていない灰色の画像にもゲージを出す。
    pub gauge_on_gray: bool,
    /// TOML では表になるので、値の項目より後に置く。
    pub fits: ColorNode,
    pub dicom: DicomColorNodes,
    /// ほかの形式の灰色の画像（16bit の PNG や TIFF など）。
    pub gray: ColorNode,
    pub palettes: Vec<CustomPalette>,
}

impl Default for PipelineSettings {
    fn default() -> Self {
        Self {
            color_node: true,
            gauge_side: GaugeSide::Right,
            gauge_on_gray: false,
            fits: ColorNode::new(true, "inferno"),
            dicom: DicomColorNodes {
                color_node: false,
                palette: vw_core::palette::GRAY.to_owned(),
                pt: ColorNode::new(true, "hot_iron"),
            },
            gray: ColorNode::new(false, vw_core::palette::GRAY),
            palettes: Vec::new(),
        }
    }
}

impl PipelineSettings {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let palettes = {
            let mut s = r.at("pipeline");
            match s.get::<Vec<CustomPalette>>("palettes") {
                None => d.palettes.clone(),
                Some(list) => {
                    let mut names = std::collections::HashSet::new();
                    list.into_iter()
                        .filter(|c| match c.palette() {
                            Err(e) => {
                                s.problem("palettes", format!("{:?}: {e} (ignored)", c.name));
                                false
                            }
                            Ok(_) if !names.insert(c.name.clone()) => {
                                s.problem(
                                    "palettes",
                                    format!("{:?}: same name twice (ignored)", c.name),
                                );
                                false
                            }
                            Ok(_) => true,
                        })
                        .collect()
                }
            }
        };
        let (color_node, gauge_side, gauge_on_gray) = {
            let mut s = r.at("pipeline");
            (
                s.get("color_node").unwrap_or(d.color_node),
                s.get("gauge_side").unwrap_or(d.gauge_side),
                s.get("gauge_on_gray").unwrap_or(d.gauge_on_gray),
            )
        };
        let dicom_base = ColorNode::new(d.dicom.color_node, &d.dicom.palette);
        let dicom = ColorNode::read(r, "pipeline.dicom", &dicom_base, &palettes);
        Self {
            color_node,
            gauge_side,
            gauge_on_gray,
            fits: ColorNode::read(r, "pipeline.fits", &d.fits, &palettes),
            dicom: DicomColorNodes {
                color_node: dicom.color_node,
                palette: dicom.palette,
                pt: ColorNode::read(r, "pipeline.dicom.pt", &d.dicom.pt, &palettes),
            },
            gray: ColorNode::read(r, "pipeline.gray", &d.gray, &palettes),
            palettes,
        }
    }
}

/// 天体の画像（FITS）の表示。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Fits {
    pub stretch: FitsStretch,
    /// [`FitsStretch::Percentile`] で黒にする位置（％）。
    pub low_percent: f32,
    /// [`FitsStretch::Percentile`] で白にする位置（％）。
    pub high_percent: f32,
    pub curve: FitsCurve,
    /// `ROWORDER` のないファイルの上下を返す。
    pub flip_rows: bool,
    /// `BAYERPAT` のモザイクをカラーにする。
    pub demosaic: bool,
    /// ダーク・フラット・バイアスの当て方。TOML では表になるので、値の項目より後に置く。
    pub calibration: FitsCalibration,
}

impl Default for Fits {
    fn default() -> Self {
        let d = vw_fits::Options::default();
        Self {
            stretch: FitsStretch::Percentile,
            low_percent: d.low_percent,
            high_percent: d.high_percent,
            curve: FitsCurve::Linear,
            flip_rows: d.flip_rows,
            demosaic: d.demosaic,
            calibration: FitsCalibration::default(),
        }
    }
}

impl Fits {
    pub const PERCENT_RANGE: std::ops::RangeInclusive<f32> = 0.0..=100.0;

    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("fits");
        let percent = |v: &f32| Fits::PERCENT_RANGE.contains(v);
        let mut low_percent = r
            .get_valid("low_percent", percent, "must be 0 to 100")
            .unwrap_or(d.low_percent);
        let mut high_percent = r
            .get_valid("high_percent", percent, "must be 0 to 100")
            .unwrap_or(d.high_percent);
        if low_percent >= high_percent {
            r.problem(
                "high_percent",
                "must be larger than low_percent (both reset)".to_owned(),
            );
            (low_percent, high_percent) = (d.low_percent, d.high_percent);
        }
        Self {
            stretch: r.get("stretch").unwrap_or(d.stretch),
            low_percent,
            high_percent,
            curve: r.get("curve").unwrap_or(d.curve),
            flip_rows: r.get("flip_rows").unwrap_or(d.flip_rows),
            demosaic: r.get("demosaic").unwrap_or(d.demosaic),
            calibration: FitsCalibration::read(r.reader()),
        }
    }

    pub fn options(&self) -> vw_fits::Options {
        vw_fits::Options {
            stretch: match self.stretch {
                FitsStretch::MinMax => vw_fits::Stretch::MinMax,
                FitsStretch::Percentile => vw_fits::Stretch::Percentile,
                FitsStretch::Zscale => vw_fits::Stretch::ZScale,
            },
            low_percent: self.low_percent,
            high_percent: self.high_percent,
            curve: self.curve.curve(),
            flip_rows: self.flip_rows,
            demosaic: self.demosaic,
        }
    }
}

/// 校正用の画像が見つかったときにどうするか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationApply {
    /// 見つかれば当てる。
    #[default]
    Auto,
    Off,
}

/// 露出が違うダークをどうするか（[`vw_fits::calibration::DarkScaling`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DarkScaling {
    #[default]
    None,
    Exposure,
}

/// フラットの選び方（[`vw_fits::calibration::FlatMatch`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlatMatch {
    #[default]
    Filter,
    FilterAndNight,
}

/// フラットを 1 前後にするときの割る数（[`vw_fits::calibration::FlatNormalize`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlatNormalize {
    #[default]
    Median,
    Mean,
}

/// 天体の画像のダーク・フラット・バイアス（`[fits.calibration]`）。既定は撮影と処理のソフトの慣習どおり。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FitsCalibration {
    pub apply: CalibrationApply,
    /// 開いたフォルダの近くの種類別のフォルダ（darks、flats など）を探す。
    pub search_nearby: bool,
    /// マスターを貯めておくフォルダ。
    pub library: Vec<String>,
    pub use_dark: bool,
    pub use_flat: bool,
    pub use_bias: bool,
    pub exposure_tolerance_secs: f64,
    /// ℃。
    pub temperature_tolerance: f64,
    pub match_gain_offset: bool,
    pub dark_scaling: DarkScaling,
    pub flat_match: FlatMatch,
    pub flat_normalize: FlatNormalize,
    pub equalize_cfa: bool,
    /// バイアスがないときに引く定数。空なら引かない。
    pub synthetic_bias: String,
}

impl Default for FitsCalibration {
    fn default() -> Self {
        let d = vw_fits::calibration::Options::default();
        Self {
            apply: CalibrationApply::Auto,
            search_nearby: d.search_nearby,
            library: Vec::new(),
            use_dark: d.use_dark,
            use_flat: d.use_flat,
            use_bias: d.use_bias,
            exposure_tolerance_secs: d.exposure_tolerance_secs,
            temperature_tolerance: d.temperature_tolerance,
            match_gain_offset: d.match_gain_offset,
            dark_scaling: DarkScaling::None,
            flat_match: FlatMatch::Filter,
            flat_normalize: FlatNormalize::Median,
            equalize_cfa: d.equalize_cfa,
            synthetic_bias: String::new(),
        }
    }
}

impl FitsCalibration {
    pub const TOLERANCE_RANGE: std::ops::RangeInclusive<f64> = 0.0..=3600.0;

    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("fits.calibration");
        let tolerance = |v: &f64| FitsCalibration::TOLERANCE_RANGE.contains(v);
        Self {
            apply: r.get("apply").unwrap_or(d.apply),
            search_nearby: r.get("search_nearby").unwrap_or(d.search_nearby),
            library: r.get("library").unwrap_or_else(|| d.library.clone()),
            use_dark: r.get("use_dark").unwrap_or(d.use_dark),
            use_flat: r.get("use_flat").unwrap_or(d.use_flat),
            use_bias: r.get("use_bias").unwrap_or(d.use_bias),
            exposure_tolerance_secs: r
                .get_valid("exposure_tolerance_secs", tolerance, "must be 0 to 3600")
                .unwrap_or(d.exposure_tolerance_secs),
            temperature_tolerance: r
                .get_valid("temperature_tolerance", tolerance, "must be 0 to 3600")
                .unwrap_or(d.temperature_tolerance),
            match_gain_offset: r.get("match_gain_offset").unwrap_or(d.match_gain_offset),
            dark_scaling: r.get("dark_scaling").unwrap_or(d.dark_scaling),
            flat_match: r.get("flat_match").unwrap_or(d.flat_match),
            flat_normalize: r.get("flat_normalize").unwrap_or(d.flat_normalize),
            equalize_cfa: r.get("equalize_cfa").unwrap_or(d.equalize_cfa),
            synthetic_bias: r
                .get_valid(
                    "synthetic_bias",
                    |v: &String| Self::bias_value(v).is_ok(),
                    "must be empty or a number",
                )
                .unwrap_or(d.synthetic_bias),
        }
    }

    /// `synthetic_bias` に書ける値か（空か数）。
    pub fn valid_bias(text: &str) -> bool {
        Self::bias_value(text).is_ok()
    }

    /// `synthetic_bias` の値。空なら `None`。
    fn bias_value(text: &str) -> Result<Option<f64>, ()> {
        let t = text.trim();
        if t.is_empty() {
            return Ok(None);
        }
        t.parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .map(Some)
            .ok_or(())
    }

    pub fn options(&self) -> vw_fits::calibration::Options {
        use vw_fits::calibration as c;
        c::Options {
            apply: self.apply == CalibrationApply::Auto,
            search_nearby: self.search_nearby,
            library: self
                .library
                .iter()
                .filter(|p| !p.trim().is_empty())
                .map(PathBuf::from)
                .collect(),
            use_dark: self.use_dark,
            use_flat: self.use_flat,
            use_bias: self.use_bias,
            exposure_tolerance_secs: self.exposure_tolerance_secs,
            temperature_tolerance: self.temperature_tolerance,
            match_gain_offset: self.match_gain_offset,
            dark_scaling: match self.dark_scaling {
                DarkScaling::None => c::DarkScaling::None,
                DarkScaling::Exposure => c::DarkScaling::Exposure,
            },
            flat_match: match self.flat_match {
                FlatMatch::Filter => c::FlatMatch::Filter,
                FlatMatch::FilterAndNight => c::FlatMatch::FilterAndNight,
            },
            flat_normalize: match self.flat_normalize {
                FlatNormalize::Median => c::FlatNormalize::Median,
                FlatNormalize::Mean => c::FlatNormalize::Mean,
            },
            equalize_cfa: self.equalize_cfa,
            synthetic_bias: Self::bias_value(&self.synthetic_bias).ok().flatten(),
        }
    }
}

/// 「送る」の送り先のアプリ（[`crate::send_app`]）。
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct SendSettings {
    /// `None` はまだ一覧を決めていないこと。そのときは OS の関連付けから作った一覧を使い、
    /// 設定ファイルに書くまでは起動のたびに作り直す（関連付けを後から変えても追いつくように）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<SendApp>>,
}

impl SendSettings {
    fn read(r: &mut Reader<'_>) -> Self {
        let mut r = r.at("send");
        let apps = r.get::<Vec<SendApp>>("apps").map(|apps| {
            apps.into_iter()
                .filter(|app| match app.problem() {
                    // 誤ったアプリだけを外し、残りは使えるようにする。
                    Some(p) => {
                        r.problem("apps", format!("{:?}: {p} (ignored)", app.name));
                        false
                    }
                    None => true,
                })
                .collect()
        });
        Self { apps }
    }
}

/// 送り先のアプリ 1 つ。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SendApp {
    pub name: String,
    pub program: String,
    /// [`vw_launch::args`] の書き方。`{path}` `{paths}` `{dir}` `{name}` を差し込める。
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub accepts: Accepts,
    /// 受け付ける拡張子の正規表現（`nef|arw|jpg`、`cr[23]`）。`.` を除いた拡張子全体と、
    /// 大文字小文字を区別せずに照らす。空なら何でも。
    #[serde(default)]
    pub extensions: String,
}

/// 送り先のアプリが受け付けるもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accepts {
    #[default]
    Files,
    Folders,
    Both,
}

impl Accepts {
    pub const ALL: [Accepts; 3] = [Accepts::Files, Accepts::Folders, Accepts::Both];

    pub fn title(self) -> &'static str {
        match self {
            Accepts::Files => "Files",
            Accepts::Folders => "Folders",
            Accepts::Both => "Files and folders",
        }
    }
}

impl SendApp {
    /// 入力に誤りがあれば、その説明。
    pub fn problem(&self) -> Option<&'static str> {
        if self.name.trim().is_empty() {
            Some("name is empty")
        } else if self.program.trim().is_empty() {
            Some("program is empty")
        } else if vw_launch::args::split(&self.args).is_err() {
            Some("a \" in the arguments is not closed")
        } else if self.extension_pattern().is_err() {
            Some("extensions is not a valid regular expression")
        } else {
            None
        }
    }

    /// 引数の書き方を区切ったもの。
    pub fn template(&self) -> Vec<String> {
        vw_launch::args::split(&self.args).unwrap_or_default()
    }

    /// フォルダ（`folder`）か、拡張子 `extension` のファイルを受け付けるか。
    pub fn takes(&self, folder: bool, extension: Option<&str>) -> bool {
        if folder {
            return self.accepts != Accepts::Files;
        }
        if self.accepts == Accepts::Folders {
            return false;
        }
        match self.extension_pattern() {
            Ok(None) => true,
            Ok(Some(pattern)) => extension.is_some_and(|ext| pattern.is_match(ext)),
            Err(_) => false,
        }
    }

    /// 拡張子の正規表現。空なら `None`。`nef` が `nefx` に当たらないよう、全体の一致にする。
    fn extension_pattern(&self) -> Result<Option<regex::Regex>, regex::Error> {
        let text = self.extensions.trim();
        if text.is_empty() {
            return Ok(None);
        }
        regex::Regex::new(&format!("(?i)^(?:{text})$")).map(Some)
    }
}

/// 他の機器への送信（[`crate::dicom_send`]）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DicomSend {
    /// 自分の AE タイトル（Calling AE）。
    pub ae_title: String,
    /// 接続と応答を待つ秒数。
    pub timeout_secs: u32,
    pub destinations: Vec<Destination>,
}

/// 送信先 1 つ。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Destination {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub ae_title: String,
}

impl Destination {
    /// 入力に誤りがあれば、その説明。
    pub fn problem(&self) -> Option<&'static str> {
        if self.name.trim().is_empty() {
            Some("name is empty")
        } else if self.host.trim().is_empty() {
            Some("host is empty")
        } else if self.port == 0 {
            Some("port must be 1 to 65535")
        } else if !vw_dicom::net::valid_ae_title(&self.ae_title) {
            Some("AE title must be 1 to 16 of A-Z, 0-9, space, _ and -")
        } else {
            None
        }
    }

    /// ログや確認に出す `TESTPACS@192.168.1.20:104`。
    pub fn address(&self) -> String {
        format!("{}@{}:{}", self.ae_title, self.host, self.port)
    }
}

impl DicomSend {
    pub const TIMEOUT_RANGE: std::ops::RangeInclusive<u32> = 1..=600;

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.into())
    }
}

impl Default for DicomSend {
    fn default() -> Self {
        Self {
            ae_title: "VW".into(),
            timeout_secs: 30,
            destinations: Vec::new(),
        }
    }
}

impl DicomSend {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("dicom_send");
        Self {
            ae_title: r
                .get_valid(
                    "ae_title",
                    |t: &String| vw_dicom::net::valid_ae_title(t),
                    "must be 1 to 16 of A-Z, 0-9, space, _ and -",
                )
                .unwrap_or_else(|| d.ae_title.clone()),
            timeout_secs: r
                .get_valid(
                    "timeout_secs",
                    |v: &u32| DicomSend::TIMEOUT_RANGE.contains(v),
                    "must be 1 to 600",
                )
                .unwrap_or(d.timeout_secs),
            destinations: r
                .get::<Vec<Destination>>("destinations")
                .unwrap_or_default()
                .into_iter()
                .filter(|dest| match dest.problem() {
                    // 誤った送信先だけを外し、残りは使えるようにする。
                    Some(p) => {
                        r.problem("destinations", format!("{:?}: {p} (ignored)", dest.name));
                        false
                    }
                    None => true,
                })
                .collect(),
        }
    }
}

/// データセット（[`crate::catalog`]）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Dataset {
    /// データセットを探しにいくフォルダ。
    pub roots: Vec<String>,
    /// 名前・メモ・タグのほかに、データセットに書き込む項目（入手元など）。
    pub fields: Vec<String>,
    /// 詳細検索で選べる、メタデータの条件の組。
    pub search_groups: Vec<SearchGroup>,
}

impl Default for Dataset {
    fn default() -> Self {
        let group = |name: &str, keys: &[&str]| SearchGroup {
            name: name.into(),
            keys: keys.iter().map(|&k| k.into()).collect(),
        };
        Self {
            roots: Vec::new(),
            fields: Vec::new(),
            search_groups: vec![
                group("Patient", &["PatientID", "PatientName", "PatientBirthDate"]),
                group("Study", &["StudyDate", "Modality", "StudyDescription"]),
                group("Series", &["Modality", "SeriesNumber", "SeriesDescription"]),
                group(
                    "AI tags",
                    &["ai tags:person", "ai tags:character", "ai tags:tag"],
                ),
            ],
        }
    }
}

impl Dataset {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("dataset");
        Self {
            roots: r.get("roots").unwrap_or_else(|| d.roots.clone()),
            fields: r
                .get_valid(
                    "fields",
                    |t: &Vec<String>| Labels::valid_tags(t),
                    "field names must be non-empty and unique",
                )
                .unwrap_or_else(|| d.fields.clone()),
            search_groups: read_groups(&mut r, "search_groups")
                .unwrap_or_else(|| d.search_groups.clone()),
        }
    }
}

/// 名前を付けたメタデータのキーの組。選ぶとキーごとに値の一致（equals）の条件が並び、値を入れるだけで探せる。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SearchGroup {
    pub name: String,
    /// `PatientID`・`0010,0020` のようなキー。種類を限るときは `exif:Model` のように頭に付ける。
    pub keys: Vec<String>,
}

impl SearchGroup {
    /// 設定画面での 1 行の区切り。キーの区切りにカンマを使わないのは、DICOM のタグ番号（`0010,0020`）にカンマが入るため。
    const NAME_END: char = '=';
    const KEY_SEPARATOR: char = ';';

    pub fn problem(&self) -> Option<&'static str> {
        if self.name.trim().is_empty() {
            Some("name is empty")
        } else if self.keys.iter().all(|k| k.trim().is_empty()) {
            Some("no keys")
        } else {
            None
        }
    }

    /// `exif:Model` を `("exif", "Model")` に、`PatientID` を `("", "PatientID")` に分ける。
    pub fn split_key(key: &str) -> (&str, &str) {
        match key.split_once(':') {
            Some((kind, key)) => (kind.trim(), key.trim()),
            None => ("", key.trim()),
        }
    }

    /// `Patient = PatientID; PatientName` の形の 1 行を読む。
    pub fn parse_line(line: &str) -> Option<Self> {
        let (name, keys) = line.split_once(Self::NAME_END)?;
        let group = Self {
            name: name.trim().to_owned(),
            keys: keys
                .split(Self::KEY_SEPARATOR)
                .map(|k| k.trim().to_owned())
                .filter(|k| !k.is_empty())
                .collect(),
        };
        group.problem().is_none().then_some(group)
    }

    pub fn to_line(&self) -> String {
        let sep = format!("{} ", Self::KEY_SEPARATOR);
        format!("{} {} {}", self.name, Self::NAME_END, self.keys.join(&sep))
    }
}

/// 全画面スライドショーの進め方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FullscreenMode {
    /// 画像のあるフォルダを順に、その中の画像を順に見せる。
    #[default]
    Sequence,
    /// 起点のフォルダ以下で最も新しいファイルを見せ、新しいファイルが来たら切り替える（テザー撮影）。
    Newest,
}

impl FullscreenMode {
    pub const ALL: [FullscreenMode; 2] = [FullscreenMode::Sequence, FullscreenMode::Newest];

    pub fn title(self) -> &'static str {
        match self {
            FullscreenMode::Sequence => "In order",
            FullscreenMode::Newest => "Follow the newest file",
        }
    }
}

/// 全画面スライドショー（[`crate::fullscreen`]）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Fullscreen {
    pub mode: FullscreenMode,
    /// フォルダとファイルを並べる基準。
    pub sort: SortKey,
    pub folders_descending: bool,
    pub files_descending: bool,
    /// 次の画像へ自動で送るまでの秒数。0 なら自動では送らない。
    pub interval_secs: f32,
    /// 最後の画像の次は最初の画像に戻る。
    pub loop_at_end: bool,
    /// 右下にパスを出す。
    pub show_path: bool,
    /// パスの文字色（`#RRGGBB`）。
    pub path_color: String,
    /// パスの文字の不透明度（%）。
    pub path_opacity: u32,
    /// パスの文字の大きさ（ポイント）。
    pub path_size: f32,
}

impl Fullscreen {
    pub const INTERVAL_RANGE: std::ops::RangeInclusive<f32> = 0.0..=3600.0;
    pub const OPACITY_RANGE: std::ops::RangeInclusive<u32> = 0..=100;
    pub const PATH_SIZE_RANGE: std::ops::RangeInclusive<f32> = 6.0..=72.0;

    pub fn path_text_color(&self) -> Color32 {
        let rgb = parse_hex_color(&self.path_color).unwrap_or(Color32::GRAY);
        let alpha = self.path_opacity.min(100) as f32 / 100.0;
        Color32::from_rgba_unmultiplied(rgb.r(), rgb.g(), rgb.b(), (alpha * 255.0).round() as u8)
    }
}

impl Default for Fullscreen {
    fn default() -> Self {
        Self {
            mode: FullscreenMode::Sequence,
            sort: SortKey::NameNatural,
            folders_descending: false,
            files_descending: false,
            interval_secs: 0.0,
            loop_at_end: false,
            show_path: true,
            path_color: "#808080".into(),
            path_opacity: 50,
            path_size: 14.0,
        }
    }
}

impl Fullscreen {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("fullscreen");
        Self {
            mode: r.get("mode").unwrap_or(d.mode),
            sort: r.get("sort").unwrap_or(d.sort),
            folders_descending: r.get("folders_descending").unwrap_or(d.folders_descending),
            files_descending: r.get("files_descending").unwrap_or(d.files_descending),
            interval_secs: r
                .get_valid(
                    "interval_secs",
                    |v: &f32| Fullscreen::INTERVAL_RANGE.contains(v),
                    "must be 0 to 3600",
                )
                .unwrap_or(d.interval_secs),
            loop_at_end: r.get("loop_at_end").unwrap_or(d.loop_at_end),
            show_path: r.get("show_path").unwrap_or(d.show_path),
            path_color: r
                .get_valid(
                    "path_color",
                    |v: &String| parse_hex_color(v).is_some(),
                    "must be #RRGGBB",
                )
                .unwrap_or_else(|| d.path_color.clone()),
            path_opacity: r
                .get_valid(
                    "path_opacity",
                    |v: &u32| Fullscreen::OPACITY_RANGE.contains(v),
                    "must be 0 to 100",
                )
                .unwrap_or(d.path_opacity),
            path_size: r
                .get_valid(
                    "path_size",
                    |v: &f32| Fullscreen::PATH_SIZE_RANGE.contains(v),
                    "must be 6 to 72",
                )
                .unwrap_or(d.path_size),
        }
    }
}

/// AI タグ（[`crate::tagger`]）。しきい値は確信度 0〜1。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ai {
    pub general_threshold: f32,
    pub character_threshold: f32,
    /// 推論に使うスレッド数。0 なら ONNX Runtime に任せる。
    pub threads: usize,
    /// 顔を登録済みの人物とみなすコサイン類似度の下限。
    pub face_threshold: f32,
    /// 検出されても付けないタグ。
    pub excluded_tags: Vec<String>,
    /// 人物が分かった顔の枠の色（`#RRGGBB`）。
    pub face_known_color: String,
    /// 人物が分からない顔の枠の色。
    pub face_unknown_color: String,
}

impl Default for Ai {
    fn default() -> Self {
        let t = crate::tagger::Thresholds::default();
        Self {
            general_threshold: t.general,
            character_threshold: t.character,
            threads: 0,
            // SFace の作者が示す、同じ人とみなすコサイン類似度の目安。
            face_threshold: 0.363,
            excluded_tags: Vec::new(),
            face_known_color: "#50DC78".into(),
            face_unknown_color: "#F0C83C".into(),
        }
    }
}

impl Ai {
    pub const THREADS_RANGE: std::ops::RangeInclusive<usize> = 0..=64;

    fn read(r: &mut Reader<'_>) -> Self {
        let mut r = r.at("ai");
        let d = Self::default();
        let unit = |v: &f32| (0.0..=1.0).contains(v);
        Self {
            general_threshold: r
                .get_valid("general_threshold", unit, "must be between 0 and 1")
                .unwrap_or(d.general_threshold),
            character_threshold: r
                .get_valid("character_threshold", unit, "must be between 0 and 1")
                .unwrap_or(d.character_threshold),
            threads: r
                .get_valid(
                    "threads",
                    |v| Self::THREADS_RANGE.contains(v),
                    "must be between 0 and 64",
                )
                .unwrap_or(d.threads),
            face_threshold: r
                .get_valid("face_threshold", unit, "must be between 0 and 1")
                .unwrap_or(d.face_threshold),
            excluded_tags: r.get("excluded_tags").unwrap_or(d.excluded_tags),
            face_known_color: read_color(&mut r, "face_known_color", d.face_known_color),
            face_unknown_color: read_color(&mut r, "face_unknown_color", d.face_unknown_color),
        }
    }

    pub fn face_known(&self) -> Color32 {
        parse_hex_color(&self.face_known_color).unwrap_or(Color32::GREEN)
    }

    pub fn face_unknown(&self) -> Color32 {
        parse_hex_color(&self.face_unknown_color).unwrap_or(Color32::YELLOW)
    }

    pub fn thresholds(&self) -> crate::tagger::Thresholds {
        crate::tagger::Thresholds {
            general: self.general_threshold,
            character: self.character_threshold,
        }
    }
}

/// 星図のウインドウ（天体の画像の写った範囲を枠で出す）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SkySettings {
    /// メニューに星図を出す。星図はネットを使わないので、既定で出しておく。
    pub enabled: bool,
    /// ヘッダの WCS から分かった範囲の枠の色（`#RRGGBB`）。
    pub certain_color: String,
    /// 架台の向きから見積もった範囲の枠の色。
    pub approximate_color: String,
}

impl Default for SkySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            certain_color: "#F0C83C".into(),
            approximate_color: "#EA4335".into(),
        }
    }
}

impl SkySettings {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("sky");
        Self {
            enabled: r.get("enabled").unwrap_or(d.enabled),
            certain_color: read_color(&mut r, "certain_color", d.certain_color),
            approximate_color: read_color(&mut r, "approximate_color", d.approximate_color),
        }
    }

    pub fn certain(&self) -> Color32 {
        parse_hex_color(&self.certain_color).unwrap_or(Color32::YELLOW)
    }

    pub fn approximate(&self) -> Color32 {
        parse_hex_color(&self.approximate_color).unwrap_or(Color32::RED)
    }
}

/// `#RRGGBB` の色の設定。読めなければ問題を記して既定の色にする。
fn read_color(r: &mut crate::toml_reader::Section<'_, '_>, key: &str, default: String) -> String {
    r.get_valid(
        key,
        |v: &String| parse_hex_color(v).is_some(),
        "must be #RRGGBB",
    )
    .unwrap_or(default)
}

/// 地図のウインドウ（撮影地点を点で出す）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapSettings {
    /// 地図を使う。外のサーバーから地図の絵を取るので、既定は切っておき、使う人が選ぶ。
    pub enabled: bool,
    /// 地図の絵（タイル）の URL。`{z}`・`{x}`・`{y}` をタイルの番号に置き換える。
    pub tile_url: String,
    /// 地図の右下に出す、地図の絵の出典。提供元の条件で、見える所に出す決まりがある。
    pub attribution: String,
    pub attribution_url: String,
}

impl Default for MapSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            tile_url: "https://tile.openstreetmap.org/{z}/{x}/{y}.png".into(),
            attribution: "© OpenStreetMap contributors".into(),
            attribution_url: "https://www.openstreetmap.org/copyright".into(),
        }
    }
}

impl MapSettings {
    fn read(r: &mut Reader<'_>) -> Self {
        let d = Self::default();
        let mut r = r.at("map");
        Self {
            enabled: r.get("enabled").unwrap_or(d.enabled),
            tile_url: r
                .get_valid(
                    "tile_url",
                    |v: &String| crate::map_tiles::valid_template(v),
                    "must be an http(s) URL with {z}, {x} and {y}",
                )
                .unwrap_or(d.tile_url),
            attribution: r.get("attribution").unwrap_or(d.attribution),
            attribution_url: r.get("attribution_url").unwrap_or(d.attribution_url),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Settings {
    pub appearance: Appearance,
    pub viewer: Viewer,
    pub tree: Tree,
    pub behavior: Behavior,
    pub metadata: MetadataSettings,
    pub roi: RoiSettings,
    pub labels: Labels,
    pub dicom: Dicom,
    pub fits: Fits,
    pub pipeline: PipelineSettings,
    pub send: SendSettings,
    pub dicom_send: DicomSend,
    pub dataset: Dataset,
    pub fullscreen: Fullscreen,
    pub log: LogSettings,
    pub quality: Quality,
    pub ai: Ai,
    pub map: MapSettings,
    pub sky: SkySettings,
    pub keys: Keymap,
}

impl Settings {
    /// TOML を読む。書かれていない項目は既定値になる。読めない項目は、その項目
    /// （キー割り当ては関係する操作も）だけを既定値にして、残りは生かす。
    /// 戻り値の 2 つ目は、既定値にした項目の説明（ログ用）。
    /// TOML として読めない（構文の誤り）ときだけ `Err` を返す。
    pub fn from_toml(text: &str) -> Result<(Self, Vec<String>), String> {
        let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let mut r = Reader::new(&table);
        let appearance = Appearance::read(&mut r);
        let viewer = Viewer::read(&mut r);
        let tree = Tree::read(&mut r);
        let behavior = Behavior::read(&mut r);
        let metadata = MetadataSettings::read(&mut r);
        let roi = RoiSettings::read(&mut r);
        let labels = Labels::read(&mut r);
        let dicom = Dicom::read(&mut r);
        let fits = Fits::read(&mut r);
        let pipeline = PipelineSettings::read(&mut r);
        let send = SendSettings::read(&mut r);
        let dicom_send = DicomSend::read(&mut r);
        let dataset = Dataset::read(&mut r);
        let fullscreen = Fullscreen::read(&mut r);
        let log = LogSettings::read(&mut r);
        let quality = Quality::read(&mut r);
        let ai = Ai::read(&mut r);
        let map = MapSettings::read(&mut r);
        let sky = SkySettings::read(&mut r);
        let mut problems = std::mem::take(&mut r.problems);
        let keys = match r.section("keys") {
            Some(t) => {
                let (keys, p) = Keymap::from_table(t);
                problems.extend(p);
                keys
            }
            None => Keymap::default(),
        };
        problems.append(&mut r.problems);
        Ok((
            Settings {
                appearance,
                viewer,
                tree,
                behavior,
                metadata,
                roi,
                labels,
                dicom,
                fits,
                pipeline,
                send,
                dicom_send,
                dataset,
                fullscreen,
                log,
                quality,
                ai,
                map,
                sky,
                keys,
            },
            problems,
        ))
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("settings are always representable in TOML")
    }
}

/// `#RRGGBB` を色にする。先頭の `#` は省略してもよい。
pub fn parse_hex_color(s: &str) -> Option<Color32> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// 設定ファイルの場所。Windows は `%APPDATA%\vw`、それ以外は XDG の規約に従う。
/// `env` は環境変数を引く関数（テストで差し替えるため）。
pub fn config_path(windows: bool, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| env(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let dir = if windows {
        absolute("APPDATA")?
    } else {
        absolute("XDG_CONFIG_HOME").or_else(|| absolute("HOME").map(|h| h.join(".config")))?
    };
    Some(dir.join("vw").join("settings.toml"))
}

/// 外部での書き換えを確かめる間隔。毎フレーム stat するほどの即時性は要らない。
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 設定と、その保存先のファイル。
pub struct SettingsStore {
    pub path: Option<PathBuf>,
    pub settings: Settings,
    /// 最後に読み書きしたときのファイルの更新時刻。これと違えば外部で書き換えられた。
    known_mtime: Option<SystemTime>,
    last_poll: Instant,
    /// ファイルに読めない項目があった。次に保存するとその内容は既定値で上書きされるので、
    /// 上書きする前に元のファイルを `.bak` に残す。
    needs_backup: bool,
}

impl SettingsStore {
    pub fn default_path() -> Option<PathBuf> {
        config_path(cfg!(windows), |name| std::env::var_os(name))
    }

    /// `path` から読み込む。ファイルがなければ既定値で始める（ファイルは作らない）。
    /// 戻り値の 2 つ目はログに出す注意事項。
    pub fn open(path: Option<PathBuf>) -> (Self, Vec<String>) {
        let mut notes = Vec::new();
        let mut settings = Settings::default();
        let mut known_mtime = None;
        let mut needs_backup = false;
        if let Some(p) = &path {
            match std::fs::read_to_string(p) {
                Ok(text) => {
                    known_mtime = mtime(p);
                    match Settings::from_toml(&text) {
                        Ok((s, problems)) => {
                            settings = s;
                            needs_backup = !problems.is_empty();
                            notes.extend(
                                problems
                                    .into_iter()
                                    .map(|e| format!("{}: {e}", p.display())),
                            );
                        }
                        Err(e) => {
                            needs_backup = true;
                            notes.push(format!("{}: {e} (using defaults)", p.display()));
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => notes.push(format!("{}: {e}", p.display())),
            }
        }
        let store = Self {
            path,
            settings,
            known_mtime,
            last_poll: Instant::now(),
            needs_backup,
        };
        (store, notes)
    }

    /// 設定ファイルに書き出す。読めない項目があったファイルを上書きするときは、
    /// 先に元のファイルを残し、その場所を `Some` で返す。
    pub fn save(&mut self) -> Result<Option<PathBuf>, String> {
        let Some(path) = &self.path else {
            return Err("no settings directory (HOME / APPDATA is not set)".into());
        };
        let backup = backup_path(path);
        let keep_original = self.needs_backup && path.exists();
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            if keep_original {
                std::fs::copy(path, &backup)?;
            }
            std::fs::write(path, self.settings.to_toml())
        };
        write().map_err(|e| format!("{}: {e}", path.display()))?;
        self.known_mtime = mtime(path);
        self.needs_backup = false;
        Ok(keep_original.then_some(backup))
    }

    /// ファイルが外部で書き換えられていたら読み直す。
    /// 読み直したら `Some(注意事項)`、読めなかったら `Some(Err)`。変化がなければ `None`。
    /// 読めなかったときは今の設定を保ち、書きかけのファイルで設定が消えないようにする。
    pub fn reload_if_changed(&mut self) -> Option<Result<Vec<String>, String>> {
        if self.last_poll.elapsed() < POLL_INTERVAL {
            return None;
        }
        self.last_poll = Instant::now();
        let path = self.path.as_ref()?;
        let now = mtime(path);
        if now.is_none() || now == self.known_mtime {
            return None;
        }
        self.known_mtime = now;
        let result = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|t| Settings::from_toml(&t));
        Some(match result {
            Ok((s, problems)) => {
                self.settings = s;
                self.needs_backup = !problems.is_empty();
                Ok(problems)
            }
            Err(e) => {
                self.needs_backup = true;
                Err(format!("{}: {e}", path.display()))
            }
        })
    }

    /// 設定ファイルを OS の既定のアプリで開く。まだなければ今の設定で作ってから開く。
    ///
    /// `open` などのクレートを使わないのは、Linux と Windows の 2 通りしかなく、
    /// どちらも 1 行のコマンドで済むため。
    pub fn open_in_editor(&mut self) -> Result<(), String> {
        let path = self.path.clone().ok_or("no settings directory")?;
        if !path.exists() {
            self.save()?;
        }
        vw_launch::open_file(&path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// テスト用: 次の [`Self::reload_if_changed`] で必ずファイルを確かめる。
    #[cfg(test)]
    fn poll_now(&mut self) {
        self.last_poll = Instant::now() - POLL_INTERVAL;
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Action, KeyCombo};
    use crate::test_util::TempDir;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    /// テスト用の絶対パス。"/home/ken" は Windows では絶対パスにならないので、
    /// 一時フォルダを起点にして両方の OS で通るようにする。
    fn abs(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    fn s(p: &Path) -> &str {
        p.to_str().unwrap()
    }

    #[test]
    fn linux_config_goes_under_dot_config() {
        let home = abs("home");
        let p = config_path(false, env(&[("HOME", s(&home))]));
        assert_eq!(
            p,
            Some(home.join(".config").join("vw").join("settings.toml"))
        );
    }

    #[test]
    fn xdg_config_home_wins_over_home() {
        let (home, cfg) = (abs("home"), abs("cfg"));
        let p = config_path(
            false,
            env(&[("HOME", s(&home)), ("XDG_CONFIG_HOME", s(&cfg))]),
        );
        assert_eq!(p, Some(cfg.join("vw").join("settings.toml")));
    }

    #[test]
    fn relative_xdg_config_home_is_ignored() {
        let home = abs("home");
        let p = config_path(
            false,
            env(&[("HOME", s(&home)), ("XDG_CONFIG_HOME", "cfg")]),
        );
        assert_eq!(
            p,
            Some(home.join(".config").join("vw").join("settings.toml"))
        );
    }

    #[test]
    fn windows_config_goes_under_appdata() {
        let appdata = abs("AppData");
        let p = config_path(true, env(&[("APPDATA", s(&appdata))]));
        assert_eq!(p, Some(appdata.join("vw").join("settings.toml")));
    }

    #[test]
    fn no_home_means_no_config_file() {
        assert_eq!(config_path(false, env(&[])), None);
        assert_eq!(config_path(true, env(&[])), None);
    }

    #[test]
    fn empty_file_gives_defaults() {
        let (s, problems) = Settings::from_toml("").unwrap();
        assert_eq!(s, Settings::default());
        assert!(problems.is_empty());
    }

    #[test]
    fn partial_file_keeps_defaults_for_the_rest() {
        let (s, _) = Settings::from_toml("[appearance]\ntheme = \"light\"\n").unwrap();
        assert_eq!(s.appearance.theme, ThemeChoice::Light);
        assert_eq!(s.appearance.language, Language::English);
        assert_eq!(s.keys, Keymap::default());
    }

    #[test]
    fn settings_round_trip_through_toml() {
        let mut s = Settings::default();
        s.appearance.language = Language::Japanese;
        s.appearance.ui_scale = 1.25;
        s.viewer.background = "#202020".into();
        s.behavior.header_click = HeaderClick::Select;
        s.behavior.auto_reload = false;
        s.labels.tags = vec!["cat".into(), "dog".into()];
        s.labels.advance = false;
        s.dicom.probe_extensions = "^(|raw)$".into();
        s.viewer.full_image_limit_mb = 8192;
        s.viewer.layout = SlotLayout::grid(1, 2);
        s.viewer.custom_layout = SlotLayout::grid(5, 3);
        s.viewer.slideshow_fps = 24.0;
        s.viewer.slideshow_end = EndMode::PingPong;
        s.viewer.slideshow_drop_window_secs = 10;
        s.viewer.keep_view = false;
        s.viewer.magnify_filter = ScaleFilter::Linear;
        s.viewer.minify_filter = ScaleFilter::Nearest;
        s.tree.load_into = LoadInto::Folders;
        s.tree.other_load_modifier = LoadModifier::Ctrl;
        s.tree.sort = SortKey::NameString;
        s.tree.sort_descending = false;
        s.tree.confirm_every = true;
        s.tree.confirm_chunk = 200;
        s.dataset.roots = vec!["/data/sets".into(), r"D:\test data".into()];
        s.quality.psnr = Thresholds {
            good: 40.0,
            caution: 30.0,
        };
        s.dataset.fields = vec!["Source".into(), "入手元".into()];
        s.fullscreen = Fullscreen {
            mode: FullscreenMode::Newest,
            sort: SortKey::Modified,
            folders_descending: true,
            files_descending: true,
            interval_secs: 2.5,
            loop_at_end: true,
            show_path: false,
            path_color: "#ffcc00".into(),
            path_opacity: 80,
            path_size: 20.0,
        };
        s.dicom_send = DicomSend {
            ae_title: "VW_TEST".into(),
            timeout_secs: 5,
            destinations: vec![
                Destination {
                    name: "検証用PACS".into(),
                    host: "192.168.1.20".into(),
                    port: 104,
                    ae_title: "TESTPACS".into(),
                },
                Destination {
                    name: "Analysis".into(),
                    host: "analysis.local".into(),
                    port: 11112,
                    ae_title: "ANALYSIS".into(),
                },
            ],
        };
        s.keys
            .unassign(Action::NextImage, KeyCombo::plain(eframe::egui::Key::Space));
        let (back, problems) = Settings::from_toml(&s.to_toml()).unwrap();
        assert_eq!(back, s);
        assert!(problems.is_empty());
    }

    #[test]
    fn metadata_display_settings_survive_a_save() {
        let mut s = Settings::default();
        s.metadata.hidden = vec!["File/Texture".into(), "AI tags".into()];
        s.metadata.groups = vec![SearchGroup {
            name: "Camera".into(),
            keys: vec!["EXIF/Model".into(), "File/Path".into()],
        }];
        s.metadata.heading_style = HeadingStyle::Underline;
        s.metadata.remember_open = false;
        s.metadata.color_format = ColorFormat::Hex;
        s.roi.color_format = ColorFormat::Hex;
        let (back, problems) = Settings::from_toml(&s.to_toml()).unwrap();
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(back, s);
    }

    #[test]
    fn metadata_group_without_keys_is_dropped_alone() {
        let text = "[metadata]\n[[metadata.groups]]\nname = \"Empty\"\nkeys = []\n[[metadata.groups]]\nname = \"Ok\"\nkeys = [\"File/Path\"]\n";
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert_eq!(s.metadata.groups.len(), 1);
        assert_eq!(s.metadata.groups[0].name, "Ok");
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn saved_file_is_readable_by_hand() {
        let text = Settings::default().to_toml();
        assert!(text.contains("theme = \"system\""), "{text}");
        assert!(text.contains("language = \"en\""), "{text}");
        assert!(text.contains("next_image = ["), "{text}");
    }

    #[test]
    fn search_groups_are_written_and_read_back_including_an_emptied_list() {
        let mut s = Settings::default();
        s.dataset.search_groups[0].keys.push("exif:Model".into());
        let (back, _) = Settings::from_toml(&s.to_toml()).unwrap();
        assert_eq!(back.dataset.search_groups, s.dataset.search_groups);
        s.dataset.search_groups.clear();
        let (back, _) = Settings::from_toml(&s.to_toml()).unwrap();
        assert!(back.dataset.search_groups.is_empty());
    }

    #[test]
    fn pipeline_settings_are_written_and_read_back_with_custom_color_maps() {
        let mut s = Settings::default();
        s.pipeline.palettes.push(CustomPalette {
            name: "Ice".into(),
            colors: vec!["0 #000000".into(), "1 #80c0ff".into()],
        });
        s.pipeline.dicom.pt.palette = "Ice".into();
        s.pipeline.gauge_side = GaugeSide::Bottom;
        s.pipeline.fits.color_node = false;
        let text = s.to_toml();
        assert!(text.contains("[pipeline.dicom.pt]"), "{text}");
        let (back, problems) = Settings::from_toml(&text).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(back.pipeline, s.pipeline);
    }

    #[test]
    fn unknown_color_maps_and_broken_custom_ones_fall_back_one_by_one() {
        let text = r#"
            [pipeline.fits]
            palette = "nonexistent"
            [pipeline.dicom.pt]
            palette = "viridis"
            [[pipeline.palettes]]
            name = "bad"
            colors = ["0 #000000"]
            [[pipeline.palettes]]
            name = "inferno"
            colors = ["0 #000000", "1 #ffffff"]
        "#;
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert_eq!(s.pipeline.fits.palette, "inferno");
        assert_eq!(s.pipeline.dicom.pt.palette, "viridis");
        assert!(s.pipeline.palettes.is_empty());
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    #[test]
    fn custom_color_map_line_is_a_name_and_stops() {
        let c = CustomPalette::parse_line("Ice = 0 #000000; 1 #80c0ff").unwrap();
        assert_eq!(c.name, "Ice");
        assert_eq!(c.colors, ["0 #000000", "1 #80c0ff"]);
        assert_eq!(CustomPalette::parse_line(&c.to_line()), Some(c));
        assert_eq!(CustomPalette::parse_line("Ice = 0 #000000"), None);
        assert_eq!(
            CustomPalette::parse_line("gray = 0 #000000; 1 #ffffff"),
            None
        );
    }

    #[test]
    fn search_groups_without_a_name_or_keys_are_dropped_one_by_one() {
        let text = r#"
[[dataset.search_groups]]
name = "ok"
keys = ["PatientID"]
[[dataset.search_groups]]
name = ""
keys = ["PatientID"]
[[dataset.search_groups]]
name = "no keys"
keys = []
"#;
        let (s, problems) = Settings::from_toml(text).unwrap();
        let names: Vec<&str> = s
            .dataset
            .search_groups
            .iter()
            .map(|g| g.name.as_str())
            .collect();
        assert_eq!(names, ["ok"]);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn a_search_group_line_keeps_tag_numbers_with_commas_as_one_key() {
        let g = SearchGroup::parse_line("Patient = 0010,0020; PatientName ;exif:Model").unwrap();
        assert_eq!(g.name, "Patient");
        assert_eq!(g.keys, ["0010,0020", "PatientName", "exif:Model"]);
        assert_eq!(SearchGroup::parse_line(&g.to_line()), Some(g));
        assert_eq!(SearchGroup::parse_line("no separator"), None);
        assert_eq!(SearchGroup::parse_line("empty = ;"), None);
        assert_eq!(SearchGroup::split_key("exif:Model"), ("exif", "Model"));
        assert_eq!(SearchGroup::split_key("0010,0020"), ("", "0010,0020"));
    }

    #[test]
    fn send_apps_are_unset_until_written() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.send.apps, None);
        assert!(!s.to_toml().contains("[[send.apps]]"));
    }

    #[test]
    fn an_empty_send_app_list_stays_empty() {
        let (s, _) = Settings::from_toml("[send]\napps = []\n").unwrap();
        assert_eq!(s.send.apps, Some(vec![]));
        let (again, _) = Settings::from_toml(&s.to_toml()).unwrap();
        assert_eq!(again.send.apps, Some(vec![]));
    }

    #[test]
    fn send_apps_round_trip_and_broken_ones_are_dropped() {
        let text = r#"
[[send.apps]]
name = "Editor"
program = 'C:\Program Files\Editor\editor.exe'
args = '--open {paths}'
accepts = "both"
extensions = "nef|arw"
[[send.apps]]
name = "no program"
program = ""
[[send.apps]]
name = "bad quote"
program = "x"
args = '"{path}'
"#;
        let (s, problems) = Settings::from_toml(text).unwrap();
        let apps = s.send.apps.clone().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].accepts, Accepts::Both);
        assert_eq!(apps[0].template(), ["--open", "{paths}"]);
        assert_eq!(problems.len(), 2);
        let (again, _) = Settings::from_toml(&s.to_toml()).unwrap();
        assert_eq!(again.send.apps, Some(apps));
    }

    #[test]
    fn send_app_takes_listed_extensions_in_any_case() {
        let app = SendApp {
            extensions: "NEF|arw".into(),
            ..Default::default()
        };
        assert!(app.takes(false, Some("nef")));
        assert!(app.takes(false, Some("ARW")));
        assert!(!app.takes(false, Some("jpg")));
        assert!(!app.takes(false, None));
    }

    #[test]
    fn send_app_extensions_are_a_regex_matched_against_the_whole_extension() {
        let app = SendApp {
            extensions: "cr[23]|jpe?g".into(),
            ..Default::default()
        };
        assert!(app.takes(false, Some("cr3")));
        assert!(app.takes(false, Some("JPEG")));
        assert!(!app.takes(false, Some("crw")));
        assert!(!app.takes(false, Some("jpgx")));
    }

    #[test]
    fn send_app_with_a_broken_extension_regex_is_a_problem() {
        let app = SendApp {
            name: "a".into(),
            program: "a".into(),
            extensions: "nef(".into(),
            ..Default::default()
        };
        assert!(app.problem().is_some());
        assert!(!app.takes(false, Some("nef")));
    }

    #[test]
    fn send_app_without_extensions_takes_any_file() {
        let app = SendApp::default();
        assert!(app.takes(false, Some("png")));
        assert!(app.takes(false, None));
    }

    #[test]
    fn send_app_takes_folders_only_when_it_accepts_them() {
        let mut app = SendApp {
            extensions: "jpg".into(),
            ..Default::default()
        };
        assert!(!app.takes(true, None));
        app.accepts = Accepts::Folders;
        assert!(app.takes(true, None));
        assert!(!app.takes(false, Some("jpg")));
        app.accepts = Accepts::Both;
        assert!(app.takes(true, None));
        assert!(app.takes(false, Some("jpg")));
    }

    #[test]
    fn broken_destinations_are_dropped_one_by_one() {
        let text = r#"
[dicom_send]
ae_title = "lower"
[[dicom_send.destinations]]
name = "ok"
host = "pacs"
port = 104
ae_title = "PACS"
[[dicom_send.destinations]]
name = "bad title"
host = "pacs"
port = 104
ae_title = "pacs"
[[dicom_send.destinations]]
name = "no port"
host = "pacs"
port = 0
ae_title = "PACS"
"#;
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert_eq!(s.dicom_send.ae_title, "VW");
        let names: Vec<_> = s.dicom_send.destinations.iter().map(|d| &d.name).collect();
        assert_eq!(names, ["ok"]);
        assert_eq!(problems.len(), 3);
    }

    #[test]
    fn duplicate_dataset_fields_fall_back_to_none() {
        let (s, problems) =
            Settings::from_toml("[dataset]\nfields = [\"a\", \"a\"]\nroots = [\"/x\"]\n").unwrap();
        assert!(s.dataset.fields.is_empty());
        assert_eq!(s.dataset.roots, ["/x"]);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn broken_fullscreen_values_fall_back_one_by_one() {
        let (s, problems) = Settings::from_toml(
            "[fullscreen]\ninterval_secs = -1\npath_color = \"grey\"\nloop_at_end = true\n",
        )
        .unwrap();
        assert_eq!(s.fullscreen.interval_secs, 0.0);
        assert_eq!(s.fullscreen.path_color, "#808080");
        assert!(s.fullscreen.loop_at_end);
        assert_eq!(problems.len(), 2);
    }

    #[test]
    fn path_text_color_carries_the_opacity() {
        let f = Fullscreen {
            path_color: "#ff0000".into(),
            path_opacity: 50,
            ..Fullscreen::default()
        };
        let c = f.path_text_color();
        assert_eq!(c.a(), 128);
        assert_eq!(c.to_srgba_unmultiplied()[0], 255);
    }

    #[test]
    fn broken_toml_is_an_error() {
        assert!(Settings::from_toml("[appearance\n").is_err());
    }

    #[test]
    fn bad_key_names_are_reported_but_rest_loads() {
        let (s, problems) = Settings::from_toml(
            "[appearance]\ncompact = false\n[keys]\nnew_slot = [\"Ctrl+Nope\"]\n",
        )
        .unwrap();
        assert!(!s.appearance.compact);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn broken_value_falls_back_to_its_default_only() {
        let text = "[appearance]\ntheme = \"purple\"\nlanguage = \"ja\"\ncompact = false\n";
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert_eq!(s.appearance.theme, ThemeChoice::System);
        assert_eq!(s.appearance.language, Language::Japanese);
        assert!(!s.appearance.compact);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].starts_with("appearance.theme"),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn wrong_type_falls_back_to_default() {
        let (s, problems) = Settings::from_toml("[appearance]\ncompact = \"yes\"\n").unwrap();
        assert!(s.appearance.compact);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn out_of_range_scale_falls_back_to_default() {
        let (s, problems) = Settings::from_toml("[appearance]\nui_scale = 9.0\n").unwrap();
        assert_eq!(s.appearance.ui_scale, 1.0);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn integer_scale_is_accepted() {
        let (s, problems) = Settings::from_toml("[appearance]\nui_scale = 2\n").unwrap();
        assert_eq!(s.appearance.ui_scale, 2.0);
        assert!(problems.is_empty());
    }

    #[test]
    fn malformed_background_falls_back_to_theme_default() {
        let (s, problems) = Settings::from_toml("[viewer]\nbackground = \"#12\"\n").unwrap();
        assert_eq!(s.viewer.background, "");
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn image_size_limit_is_4gib_by_default_and_rejects_tiny_values() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.viewer.image_size_limit_bytes(), 4 << 30);
        let (s, problems) = Settings::from_toml("[viewer]\nimage_size_limit_mb = 1\n").unwrap();
        assert_eq!(s.viewer.image_size_limit_mb, 4096);
        assert!(
            problems.iter().any(|p| p.contains("image_size_limit_mb")),
            "{problems:?}"
        );
    }

    #[test]
    fn full_image_memory_limit_is_256mb_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.viewer.full_image_limit_mb, 256);
        assert_eq!(s.viewer.full_image_limit_bytes(), 256 << 20);
    }

    #[test]
    fn slots_are_laid_out_automatically_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.viewer.layout, SlotLayout::Auto);
    }

    #[test]
    fn layout_is_written_as_cols_x_rows() {
        let (s, problems) =
            Settings::from_toml("[viewer]\nlayout = \"2x2\"\ncustom_layout = \"4x1\"\n").unwrap();
        assert!(problems.is_empty());
        assert_eq!(s.viewer.layout, SlotLayout::grid(2, 2));
        assert_eq!(s.viewer.custom_layout, SlotLayout::grid(4, 1));
    }

    #[test]
    fn auto_layout_max_is_read_as_cols_x_rows_and_rejects_auto() {
        let (s, _) = Settings::from_toml("[viewer]\nauto_layout_max = \"4x3\"\n").unwrap();
        assert_eq!(s.viewer.auto_layout_max, SlotLayout::grid(4, 3));
        let (s, problems) = Settings::from_toml("[viewer]\nauto_layout_max = \"auto\"\n").unwrap();
        assert_eq!(s.viewer.auto_layout_max, SlotLayout::grid(6, 6));
        assert!(!problems.is_empty());
    }

    #[test]
    fn broken_layout_falls_back_to_default_with_a_problem() {
        let (s, problems) =
            Settings::from_toml("[viewer]\nlayout = \"0x3\"\ncustom_layout = \"auto\"\n").unwrap();
        assert_eq!(s.viewer.layout, SlotLayout::Auto);
        assert_eq!(s.viewer.custom_layout, SlotLayout::grid(6, 6));
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn magnifies_with_nearest_and_minifies_linearly_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.viewer.magnify_filter, ScaleFilter::Nearest);
        assert_eq!(s.viewer.minify_filter, ScaleFilter::Linear);
    }

    #[test]
    fn unknown_scale_filter_falls_back_to_default_with_a_problem() {
        let (s, problems) = Settings::from_toml(
            "[viewer]\nmagnify_filter = \"lanczos\"\nminify_filter = \"nearest\"\n",
        )
        .unwrap();
        assert_eq!(s.viewer.magnify_filter, ScaleFilter::Nearest);
        assert_eq!(s.viewer.minify_filter, ScaleFilter::Nearest);
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn page_wheel_defaults_to_one_page_and_reads_slots_and_half_pages() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.viewer.page_wheel, WheelScroll::Page);
        for (text, unit) in [
            ("slot", WheelScroll::Slot),
            ("half_page", WheelScroll::HalfPage),
        ] {
            let (s, _) =
                Settings::from_toml(&format!("[viewer]\npage_wheel = \"{text}\"\n")).unwrap();
            assert_eq!(s.viewer.page_wheel, unit);
        }
        let (s, problems) = Settings::from_toml("[viewer]\npage_wheel = \"row\"\n").unwrap();
        assert_eq!(s.viewer.page_wheel, WheelScroll::Page);
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn slideshow_starts_at_30_fps_looping() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.viewer.slideshow_fps, 30.0);
        assert_eq!(s.viewer.slideshow_end, EndMode::Loop);
    }

    #[test]
    fn dropped_frames_are_counted_over_the_last_5_seconds_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.viewer.slideshow_drop_window_secs, 5);
    }

    #[test]
    fn out_of_range_drop_window_falls_back_to_default() {
        let (s, problems) =
            Settings::from_toml("[viewer]\nslideshow_drop_window_secs = 0\n").unwrap();
        assert_eq!(s.viewer.slideshow_drop_window_secs, 5);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn out_of_range_fps_falls_back_to_default() {
        let (s, problems) = Settings::from_toml("[viewer]\nslideshow_fps = 500.0\n").unwrap();
        assert_eq!(s.viewer.slideshow_fps, 30.0);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn folders_load_one_image_per_slot_and_alt_switches_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.tree.load_into, LoadInto::Images);
        assert_eq!(s.tree.other_load_modifier, LoadModifier::Alt);
    }

    #[test]
    fn too_small_memory_limit_falls_back_to_default() {
        let (s, problems) = Settings::from_toml("[viewer]\nfull_image_limit_mb = 10\n").unwrap();
        assert_eq!(s.viewer.full_image_limit_mb, 256);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn tree_click_expands_the_folder_unless_turned_off() {
        assert!(Settings::default().tree.click_expands);
        let (s, problems) = Settings::from_toml("[tree]\nclick_expands = false\n").unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert!(!s.tree.click_expands);
    }

    #[test]
    fn tree_is_sorted_newest_first_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.tree.sort, SortKey::Modified);
        assert!(s.tree.sort_descending);
    }

    #[test]
    fn tree_sort_reads_key_and_direction() {
        let (s, problems) =
            Settings::from_toml("[tree]\nsort = \"name_string\"\nsort_descending = false\n")
                .unwrap();
        assert_eq!(s.tree.sort, SortKey::NameString);
        assert!(!s.tree.sort_descending);
        assert!(problems.is_empty());
    }

    #[test]
    fn unknown_tree_sort_falls_back_to_default() {
        let (s, problems) = Settings::from_toml("[tree]\nsort = \"size\"\n").unwrap();
        assert_eq!(s.tree.sort, SortKey::Modified);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn folder_loads_do_not_stop_to_confirm_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert!(!s.tree.confirm_every);
        assert_eq!(s.tree.confirm_chunk, 500);
    }

    #[test]
    fn settings_of_the_removed_tree_modes_are_ignored_silently() {
        let text = "[tree]\nmode = \"auto_load\"\nfolder_action = \"immediate\"\n\
                    double_click_ms = 5\non_load_other_folder = \"append\"\nconfirm_chunk = 100\n";
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert_eq!(s.tree.confirm_chunk, 100);
        assert!(problems.is_empty());
    }

    #[test]
    fn out_of_range_chunk_falls_back_to_default() {
        let text = "[tree]\nconfirm_chunk = 0\n";
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert_eq!(s.tree.confirm_chunk, 500);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn header_click_selects_and_toggles_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.behavior.header_click, HeaderClick::SelectAndToggle);
    }

    #[test]
    fn duplicate_tag_names_fall_back_to_the_default_list() {
        let (s, problems) = Settings::from_toml("[labels]\ntags = [\"cat\", \"cat\"]\n").unwrap();
        assert!(s.labels.tags.is_empty());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn dicom_probes_files_without_extension_or_with_digits_dcm_dicom_or_ima_by_default() {
        assert_eq!(
            Settings::default().dicom.probe_extensions,
            "^(|[0-9]+|dcm|dicom|ima)$"
        );
    }

    #[test]
    fn invalid_dicom_extension_pattern_falls_back_to_the_default() {
        let (s, problems) = Settings::from_toml("[dicom]\nprobe_extensions = \"(\"\n").unwrap();
        assert_eq!(s.dicom, Dicom::default());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn auto_reload_is_on_unless_turned_off() {
        assert!(Settings::default().behavior.auto_reload);
        let (s, _) = Settings::from_toml("[behavior]\nauto_reload = false\n").unwrap();
        assert!(!s.behavior.auto_reload);
    }

    #[test]
    fn archives_are_extracted_in_the_os_temp_folder_unless_a_folder_is_set() {
        assert_eq!(Settings::default().behavior.archive_parent(), None);
        let dir = if cfg!(windows) { r"D:\work" } else { "/work" };
        let (s, problems) =
            Settings::from_toml(&format!("[behavior]\narchive_dir = '{dir}'\n")).unwrap();
        assert_eq!(s.behavior.archive_parent(), Some(PathBuf::from(dir)));
        assert!(problems.is_empty());
    }

    #[test]
    fn archives_up_to_20gb_are_extracted_without_asking_by_default() {
        let s = Settings::default();
        assert_eq!(s.behavior.archive_size_limit_bytes(), 20 << 30);
        let (s, problems) = Settings::from_toml("[behavior]\narchive_size_limit_gb = 0\n").unwrap();
        assert_eq!(s.behavior.archive_size_limit_gb, 20);
        assert!(
            problems.iter().any(|p| p.contains("archive_size_limit_gb")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_relative_archive_folder_falls_back_to_the_os_temp_folder() {
        let (s, problems) = Settings::from_toml("[behavior]\narchive_dir = \"work\"\n").unwrap();
        assert_eq!(s.behavior.archive_parent(), None);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn header_click_can_select_only() {
        let (s, problems) = Settings::from_toml("[behavior]\nheader_click = \"select\"\n").unwrap();
        assert_eq!(s.behavior.header_click, HeaderClick::Select);
        assert!(problems.is_empty());
    }

    #[test]
    fn unknown_header_click_falls_back_to_the_default() {
        let (s, problems) = Settings::from_toml("[behavior]\nheader_click = \"toggle\"\n").unwrap();
        assert_eq!(s.behavior.header_click, HeaderClick::SelectAndToggle);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn metadata_path_shows_the_full_path_unless_set_to_the_name() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.metadata.path, PathDisplay::Full);
        let (s, problems) = Settings::from_toml("[metadata]\npath = \"name\"\n").unwrap();
        assert_eq!(s.metadata.path, PathDisplay::Name);
        assert!(problems.is_empty());
    }

    #[test]
    fn section_that_is_not_a_table_falls_back_entirely() {
        let (s, problems) = Settings::from_toml("appearance = 3\nkeys = 1\n").unwrap();
        assert_eq!(s, Settings::default());
        assert_eq!(problems.len(), 2);
    }

    #[test]
    fn hex_colors_parse_with_or_without_hash() {
        assert_eq!(
            parse_hex_color("#102030"),
            Some(Color32::from_rgb(0x10, 0x20, 0x30))
        );
        assert_eq!(
            parse_hex_color(" a0B0c0 "),
            Some(Color32::from_rgb(0xa0, 0xb0, 0xc0))
        );
    }

    #[test]
    fn malformed_hex_colors_are_rejected() {
        for s in ["", "#12345", "#1234567", "#12345g", "#ｱｲｳｴｵｶ"] {
            assert_eq!(parse_hex_color(s), None, "{s}");
        }
    }

    #[test]
    fn ui_scale_is_kept_in_a_usable_range() {
        let mut a = Appearance::default();
        for (given, used) in [(0.1, 0.75), (1.5, 1.5), (9.0, 2.0), (f32::NAN, 1.0)] {
            a.ui_scale = given;
            assert_eq!(a.clamped_scale(), used);
        }
    }

    #[test]
    fn missing_file_starts_with_defaults_and_creates_nothing() {
        let dir = TempDir::new();
        let path = dir.path().join("vw").join("settings.toml");
        let (store, notes) = SettingsStore::open(Some(path.clone()));
        assert_eq!(store.settings, Settings::default());
        assert!(notes.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn saved_settings_are_loaded_next_time() {
        let dir = TempDir::new();
        let path = dir.path().join("vw").join("settings.toml");
        let (mut store, _) = SettingsStore::open(Some(path.clone()));
        store.settings.appearance.theme = ThemeChoice::Dark;
        store.save().unwrap();
        let (again, notes) = SettingsStore::open(Some(path));
        assert_eq!(again.settings.appearance.theme, ThemeChoice::Dark);
        assert!(notes.is_empty());
    }

    #[test]
    fn broken_file_falls_back_to_defaults_with_a_note() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "not = [valid").unwrap();
        let (store, notes) = SettingsStore::open(Some(path));
        assert_eq!(store.settings, Settings::default());
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn overwriting_a_broken_file_keeps_the_original_as_backup() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "[appearance]\ntheme = \"purple\"\n").unwrap();
        let (mut store, notes) = SettingsStore::open(Some(path.clone()));
        assert_eq!(notes.len(), 1);
        let backup = store.save().unwrap().expect("backup made");
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "[appearance]\ntheme = \"purple\"\n"
        );
        assert_eq!(store.save().unwrap(), None, "only the first save backs up");
    }

    #[test]
    fn saving_a_healthy_file_makes_no_backup() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.toml");
        let (mut store, _) = SettingsStore::open(Some(path.clone()));
        assert_eq!(store.save().unwrap(), None);
        assert_eq!(store.save().unwrap(), None);
        assert!(!dir.path().join("settings.toml.bak").exists());
    }

    #[test]
    fn own_save_is_not_reported_as_external_change() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.toml");
        let (mut store, _) = SettingsStore::open(Some(path));
        store.save().unwrap();
        store.poll_now();
        assert!(store.reload_if_changed().is_none());
    }

    #[test]
    fn external_edit_is_picked_up() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.toml");
        let (mut store, _) = SettingsStore::open(Some(path.clone()));
        store.save().unwrap();
        // 更新時刻の分解能が粗いファイルシステムでも差が出るよう、時刻を明示して進める。
        std::fs::write(&path, "[appearance]\ntheme = \"light\"\n").unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        store.poll_now();
        assert!(matches!(store.reload_if_changed(), Some(Ok(_))));
        assert_eq!(store.settings.appearance.theme, ThemeChoice::Light);
    }

    #[test]
    fn broken_external_edit_keeps_current_settings() {
        let dir = TempDir::new();
        let path = dir.path().join("settings.toml");
        let (mut store, _) = SettingsStore::open(Some(path.clone()));
        store.settings.appearance.theme = ThemeChoice::Dark;
        store.save().unwrap();
        std::fs::write(&path, "[appearance").unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        store.poll_now();
        assert!(matches!(store.reload_if_changed(), Some(Err(_))));
        assert_eq!(store.settings.appearance.theme, ThemeChoice::Dark);
    }

    #[test]
    fn saving_without_a_settings_directory_fails_cleanly() {
        let (mut store, _) = SettingsStore::open(None);
        assert!(store.save().is_err());
    }
}

#[cfg(test)]
mod quality_tests {
    use super::*;

    #[test]
    fn quality_thresholds_are_read_per_metric_and_bad_ones_fall_back() {
        let text =
            "[quality]\npsnr = { good = 40, caution = 30 }\nssim = { good = 0.5, caution = 0.9 }\n";
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert_eq!(
            s.quality.psnr,
            Thresholds {
                good: 40.0,
                caution: 30.0
            }
        );
        assert_eq!(s.quality.ssim, Metric::Ssim.default_thresholds());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("quality.ssim:"), "{}", problems[0]);
    }
}

#[cfg(test)]
mod fits_tests {
    use super::*;

    #[test]
    fn fits_settings_become_decoder_options() {
        let text =
            "[fits]\nstretch = \"zscale\"\ncurve = \"asinh\"\nflip_rows = true\ndemosaic = false\n";
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        let o = s.fits.options();
        assert_eq!(o.stretch, vw_fits::Stretch::ZScale);
        assert_eq!(o.curve, vw_core::Curve::Asinh);
        assert!(o.flip_rows && !o.demosaic);
    }

    #[test]
    fn frame_colors_are_read_and_bad_ones_fall_back() {
        let text = "[sky]\ncertain_color = \"#102030\"\napproximate_color = \"red\"\n\
                    [ai]\nface_known_color = \"#00ff00\"\n";
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert_eq!(s.sky.certain(), Color32::from_rgb(0x10, 0x20, 0x30));
        assert_eq!(
            s.sky.approximate_color,
            SkySettings::default().approximate_color
        );
        assert_eq!(s.ai.face_known(), Color32::from_rgb(0, 255, 0));
        assert_eq!(s.ai.face_unknown_color, Ai::default().face_unknown_color);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].starts_with("sky.approximate_color:"),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn sky_chart_is_on_by_default() {
        assert!(Settings::default().sky.enabled);
    }

    #[test]
    fn calibration_defaults_follow_the_usual_practice() {
        let o = Settings::default().fits.calibration.options();
        assert!(o.apply && o.search_nearby && o.use_dark && o.use_flat && o.use_bias);
        assert_eq!(o.dark_scaling, vw_fits::calibration::DarkScaling::None);
        assert_eq!(
            o.flat_normalize,
            vw_fits::calibration::FlatNormalize::Median
        );
        assert_eq!(o.exposure_tolerance_secs, 0.5);
        assert_eq!(o.temperature_tolerance, 2.0);
        assert!(!o.equalize_cfa);
        assert_eq!(o.synthetic_bias, None);
    }

    #[test]
    fn calibration_settings_are_read_from_their_own_table() {
        let text = "[fits.calibration]\napply = \"off\"\nlibrary = [\"/masters\"]\n\
                    dark_scaling = \"exposure\"\nflat_match = \"filter_and_night\"\n\
                    flat_normalize = \"mean\"\nsynthetic_bias = \"2048\"\n";
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        let o = s.fits.calibration.options();
        assert!(!o.apply);
        assert_eq!(o.library, vec![PathBuf::from("/masters")]);
        assert_eq!(o.dark_scaling, vw_fits::calibration::DarkScaling::Exposure);
        assert_eq!(
            o.flat_match,
            vw_fits::calibration::FlatMatch::FilterAndNight
        );
        assert_eq!(o.flat_normalize, vw_fits::calibration::FlatNormalize::Mean);
        assert_eq!(o.synthetic_bias, Some(2048.0));
        let again = Settings::from_toml(&s.to_toml()).unwrap().0;
        assert_eq!(again.fits.calibration, s.fits.calibration);
    }

    #[test]
    fn a_constant_bias_that_is_not_a_number_falls_back_to_none() {
        let text = "[fits.calibration]\nsynthetic_bias = \"about 2000\"\n\
                    temperature_tolerance = -1.0\n";
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert_eq!(s.fits.calibration, FitsCalibration::default());
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().all(|p| p.starts_with("fits.calibration.")));
    }

    #[test]
    fn black_percentile_above_white_resets_both() {
        let text = "[fits]\nlow_percent = 90.0\nhigh_percent = 10.0\n";
        let (s, problems) = Settings::from_toml(text).unwrap();
        assert_eq!(s.fits, Fits::default());
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].starts_with("fits.high_percent:"),
            "{}",
            problems[0]
        );
    }
}
