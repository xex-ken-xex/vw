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
use crate::layout::SlotLayout;
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
    /// スロットの並べ方。
    pub layout: SlotLayout,
    /// メニューの「カスタム」で選ぶマス目。`auto` は選べない（既定の項目にあるため）。
    pub custom_layout: SlotLayout,
    /// 自動レイアウトで並べるマス目の上限。超える数のスロットはページに分ける。
    /// 大量に読み込んでも 1 枠が小さくなりすぎないようにするため。
    pub auto_layout_max: SlotLayout,
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

    pub fn full_image_limit_bytes(&self) -> usize {
        self.full_image_limit_mb as usize * 1024 * 1024
    }
}

impl Default for Viewer {
    fn default() -> Self {
        Self {
            background: String::new(),
            full_image_limit_mb: 256,
            layout: SlotLayout::Auto,
            custom_layout: SlotLayout::grid(6, 6),
            auto_layout_max: SlotLayout::grid(6, 6),
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
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            header_click: HeaderClick::default(),
            auto_reload: true,
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
        }
    }
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
            search_groups: match r.get::<Vec<SearchGroup>>("search_groups") {
                Some(groups) => groups
                    .into_iter()
                    .filter(|g| match g.problem() {
                        // 誤った組だけを外し、残りは使えるようにする。
                        Some(p) => {
                            r.problem("search_groups", format!("{:?}: {p} (ignored)", g.name));
                            false
                        }
                        None => true,
                    })
                    .collect(),
                None => d.search_groups.clone(),
            },
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
        }
    }

    pub fn thresholds(&self) -> crate::tagger::Thresholds {
        crate::tagger::Thresholds {
            general: self.general_threshold,
            character: self.character_threshold,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Settings {
    pub appearance: Appearance,
    pub viewer: Viewer,
    pub tree: Tree,
    pub behavior: Behavior,
    pub labels: Labels,
    pub dicom: Dicom,
    pub dicom_send: DicomSend,
    pub dataset: Dataset,
    pub fullscreen: Fullscreen,
    pub log: LogSettings,
    pub quality: Quality,
    pub ai: Ai,
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
        let labels = Labels::read(&mut r);
        let dicom = Dicom::read(&mut r);
        let dicom_send = DicomSend::read(&mut r);
        let dataset = Dataset::read(&mut r);
        let fullscreen = Fullscreen::read(&mut r);
        let log = LogSettings::read(&mut r);
        let quality = Quality::read(&mut r);
        let ai = Ai::read(&mut r);
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
                labels,
                dicom,
                dicom_send,
                dataset,
                fullscreen,
                log,
                quality,
                ai,
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
        crate::launch::open_file(&path).map_err(|e| format!("{}: {e}", path.display()))
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
