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

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Viewer {
    /// 画像の背景色（`#RRGGBB`）。空ならテーマの既定色。
    pub background: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Settings {
    pub appearance: Appearance,
    pub viewer: Viewer,
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
