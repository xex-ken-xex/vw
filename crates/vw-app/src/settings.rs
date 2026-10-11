//! 設定の中身と、設定ファイル（TOML）の読み書き。
//!
//! 設定ファイルは設定画面で値を変えるたびに書き出す。ファイルが外部のエディタで
//! 書き換えられたら読み直して、すぐ画面に反映する。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

use crate::keymap::Keymap;
use crate::layout::SlotLayout;
use crate::slideshow::{EndMode, FPS_RANGE};
use crate::sort::{SortKey, SortOrder};
use crate::toml_reader::Reader;

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
    /// スライドショーの速さ。再生バーで最後に使った値を覚える。
    pub slideshow_fps: f32,
    pub slideshow_end: EndMode,
    /// 複数枚のスロットで前後の画像に送るとき、倍率と位置を引き継ぐ（DICOM は常に引き継ぐ）。
    pub keep_view: bool,
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
            full_image_limit_mb: 4096,
            layout: SlotLayout::Auto,
            custom_layout: SlotLayout::grid(3, 2),
            slideshow_fps: 10.0,
            slideshow_end: EndMode::Loop,
            keep_view: true,
        }
    }
}

/// グループの見出し（メタデータ欄の File など）をクリックしたときの動作。
/// 開閉の ▶ はどちらでも開閉する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderClick {
    /// 選ぶだけで開閉しない。コピーしようとして選ぶたびに閉じてしまわないようにする。
    #[default]
    Select,
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
            sort: SortKey::default(),
            sort_descending: false,
            confirm_every: true,
            confirm_chunk: 500,
            load_into: LoadInto::default(),
            other_load_modifier: LoadModifier::default(),
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
    pub fn probe(&self) -> vw_core::ProbeExtensions {
        vw_core::ProbeExtensions::new(&self.probe_extensions).unwrap_or_default()
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
    pub keys: Keymap,
}

impl Settings {
    /// TOML を読む。書かれていない項目は既定値になる。読めない項目は、その項目
    /// （キー割り当ては関係する操作も）だけを既定値にして、残りは生かす。
    /// 戻り値の 2 つ目は、既定値にした項目の説明（ログ用）。
    /// TOML として読めない（構文の誤り）ときだけ `Err` を返す。
    pub fn from_toml(text: &str) -> Result<(Self, Vec<String>), String> {
        let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let d = Settings::default();
        let mut r = Reader::new(&table);
        let scale_range = Appearance::MIN_SCALE..=Appearance::MAX_SCALE;
        let appearance = Appearance {
            theme: r.get("appearance", "theme").unwrap_or(d.appearance.theme),
            language: r
                .get("appearance", "language")
                .unwrap_or(d.appearance.language),
            ui_scale: r
                .get_valid(
                    "appearance",
                    "ui_scale",
                    |v: &f32| scale_range.contains(v),
                    "must be 0.75 to 2.0",
                )
                .unwrap_or(d.appearance.ui_scale),
            compact: r
                .get("appearance", "compact")
                .unwrap_or(d.appearance.compact),
        };
        let viewer = Viewer {
            background: r
                .get_valid(
                    "viewer",
                    "background",
                    |v: &String| v.trim().is_empty() || parse_hex_color(v).is_some(),
                    "must be #RRGGBB or empty",
                )
                .unwrap_or(d.viewer.background),
            full_image_limit_mb: r
                .get_valid(
                    "viewer",
                    "full_image_limit_mb",
                    |v: &u32| Viewer::LIMIT_RANGE_MB.contains(v),
                    "must be 256 to 1048576",
                )
                .unwrap_or(d.viewer.full_image_limit_mb),
            layout: r.get("viewer", "layout").unwrap_or(d.viewer.layout),
            custom_layout: r
                .get_valid(
                    "viewer",
                    "custom_layout",
                    |v: &SlotLayout| *v != SlotLayout::Auto,
                    "must be COLSxROWS",
                )
                .unwrap_or(d.viewer.custom_layout),
            slideshow_fps: r
                .get_valid(
                    "viewer",
                    "slideshow_fps",
                    |v: &f32| FPS_RANGE.contains(v),
                    "must be 0.1 to 120",
                )
                .unwrap_or(d.viewer.slideshow_fps),
            slideshow_end: r
                .get("viewer", "slideshow_end")
                .unwrap_or(d.viewer.slideshow_end),
            keep_view: r.get("viewer", "keep_view").unwrap_or(d.viewer.keep_view),
        };
        let tree = Tree {
            sort: r.get("tree", "sort").unwrap_or(d.tree.sort),
            sort_descending: r
                .get("tree", "sort_descending")
                .unwrap_or(d.tree.sort_descending),
            confirm_every: r
                .get("tree", "confirm_every")
                .unwrap_or(d.tree.confirm_every),
            confirm_chunk: r
                .get_valid(
                    "tree",
                    "confirm_chunk",
                    |v: &u32| Tree::CHUNK_RANGE.contains(v),
                    "must be 1 to 100000",
                )
                .unwrap_or(d.tree.confirm_chunk),
            load_into: r.get("tree", "load_into").unwrap_or(d.tree.load_into),
            other_load_modifier: r
                .get("tree", "other_load_modifier")
                .unwrap_or(d.tree.other_load_modifier),
        };
        let behavior = Behavior {
            header_click: r
                .get("behavior", "header_click")
                .unwrap_or(d.behavior.header_click),
            auto_reload: r
                .get("behavior", "auto_reload")
                .unwrap_or(d.behavior.auto_reload),
        };
        let labels = Labels {
            tags: r
                .get_valid(
                    "labels",
                    "tags",
                    |t: &Vec<String>| Labels::valid_tags(t),
                    "tag names must be non-empty and unique",
                )
                .unwrap_or_else(|| d.labels.tags.clone()),
            advance: r.get("labels", "advance").unwrap_or(d.labels.advance),
        };
        let dicom = Dicom {
            probe_extensions: r
                .get_valid(
                    "dicom",
                    "probe_extensions",
                    |e: &String| vw_core::ProbeExtensions::new(e).is_ok(),
                    "not a valid regular expression",
                )
                .unwrap_or_else(|| d.dicom.probe_extensions.clone()),
        };
        let mut problems = std::mem::take(&mut r.problems);
        let keys = match r.section("keys") {
            Some(t) => {
                let (keys, p) = Keymap::from_table(t);
                problems.extend(p);
                keys
            }
            None => d.keys,
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
        launch(&path).map_err(|e| format!("{}: {e}", path.display()))
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

#[cfg(windows)]
fn launch(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // cmd の start は拡張子に関連付けられたアプリで開く。CREATE_NO_WINDOW で
    // cmd 自身の黒い窓を出さない。関連付けがなければ Windows が「アプリを選択」を出す。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(drop)
}

#[cfg(not(windows))]
fn launch(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(drop)
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
        s.behavior.header_click = HeaderClick::SelectAndToggle;
        s.behavior.auto_reload = false;
        s.labels.tags = vec!["cat".into(), "dog".into()];
        s.labels.advance = false;
        s.dicom.probe_extensions = "^(|raw)$".into();
        s.viewer.full_image_limit_mb = 8192;
        s.viewer.layout = SlotLayout::grid(1, 2);
        s.viewer.custom_layout = SlotLayout::grid(5, 3);
        s.viewer.slideshow_fps = 24.0;
        s.viewer.slideshow_end = EndMode::PingPong;
        s.viewer.keep_view = false;
        s.tree.load_into = LoadInto::Folders;
        s.tree.other_load_modifier = LoadModifier::Ctrl;
        s.tree.sort = SortKey::Modified;
        s.tree.sort_descending = true;
        s.tree.confirm_every = false;
        s.tree.confirm_chunk = 200;
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
    fn full_image_memory_limit_is_4gb_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.viewer.full_image_limit_mb, 4096);
        assert_eq!(s.viewer.full_image_limit_bytes(), 4 << 30);
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
    fn broken_layout_falls_back_to_default_with_a_problem() {
        let (s, problems) =
            Settings::from_toml("[viewer]\nlayout = \"0x3\"\ncustom_layout = \"auto\"\n").unwrap();
        assert_eq!(s.viewer.layout, SlotLayout::Auto);
        assert_eq!(s.viewer.custom_layout, SlotLayout::grid(3, 2));
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn slideshow_starts_at_10_fps_looping() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.viewer.slideshow_fps, 10.0);
        assert_eq!(s.viewer.slideshow_end, EndMode::Loop);
    }

    #[test]
    fn out_of_range_fps_falls_back_to_default() {
        let (s, problems) = Settings::from_toml("[viewer]\nslideshow_fps = 500.0\n").unwrap();
        assert_eq!(s.viewer.slideshow_fps, 10.0);
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
        assert_eq!(s.viewer.full_image_limit_mb, 4096);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn tree_is_sorted_by_name_with_numbers_by_value_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.tree.order(), SortOrder::default());
        assert_eq!(s.tree.sort, SortKey::NameNatural);
        assert!(!s.tree.sort_descending);
    }

    #[test]
    fn tree_sort_reads_key_and_direction() {
        let (s, problems) =
            Settings::from_toml("[tree]\nsort = \"modified\"\nsort_descending = true\n").unwrap();
        assert_eq!(s.tree.sort, SortKey::Modified);
        assert!(s.tree.sort_descending);
        assert!(problems.is_empty());
    }

    #[test]
    fn unknown_tree_sort_falls_back_to_default() {
        let (s, problems) = Settings::from_toml("[tree]\nsort = \"size\"\n").unwrap();
        assert_eq!(s.tree.sort, SortKey::NameNatural);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn folder_loads_confirm_every_500_images_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert!(s.tree.confirm_every);
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
    fn header_click_selects_only_by_default() {
        let (s, _) = Settings::from_toml("").unwrap();
        assert_eq!(s.behavior.header_click, HeaderClick::Select);
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
    fn header_click_can_also_toggle() {
        let (s, problems) =
            Settings::from_toml("[behavior]\nheader_click = \"select_and_toggle\"\n").unwrap();
        assert_eq!(s.behavior.header_click, HeaderClick::SelectAndToggle);
        assert!(problems.is_empty());
    }

    #[test]
    fn unknown_header_click_falls_back_to_select() {
        let (s, problems) = Settings::from_toml("[behavior]\nheader_click = \"toggle\"\n").unwrap();
        assert_eq!(s.behavior.header_click, HeaderClick::Select);
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
