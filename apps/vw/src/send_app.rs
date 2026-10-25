//! 右クリックの「送る ▸」と、選んだファイルやフォルダを別のアプリに渡すこと。
//!
//! メニューの上にアプリ、下にネットワークの送信先（[`crate::dicom_send`]）を並べる。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use eframe::egui;

use crate::i18n::tr;
use crate::settings::{Language, SendApp};

/// 関連付けを調べる拡張子。送り先の一覧の最初の中身になる。
pub const ASSOCIATED_EXTENSIONS: [&str; 14] = [
    "jpg", "jpeg", "nef", "nrw", "arw", "srf", "sr2", "raf", "cr2", "cr3", "dng", "orf", "rw2",
    "pef",
];

/// これ以上の回数アプリを起動するときは、先に確かめる。
pub const CONFIRM_LAUNCHES: usize = 6;

/// 「送る」のメニューで選ばれたもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// 拡張子に OS が関連付けているアプリで開く。
    OpenDefault,
    /// 設定の送り先のアプリの何番目か。
    App(usize),
    /// 送り先のアプリの設定を開く。
    SetUpApps,
    Network(crate::dicom_send::Choice),
}

/// 「送る」のメニューに出すもの。
pub struct Offer<'a> {
    pub apps: &'a [SendApp],
    /// ネットワークの送信先の名前。`None` はここでは出さない（DICOM でない画像など）。
    pub destinations: Option<&'a [String]>,
    /// 送るものがフォルダか。ZIP はファイルとして扱う。
    pub folder: bool,
    /// 送るファイルの拡張子。
    pub extension: Option<String>,
}

impl<'a> Offer<'a> {
    /// `path` 1 つ（またはその仲間の並び）を送るときのもの。
    pub fn for_path(
        apps: &'a [SendApp],
        destinations: Option<&'a [String]>,
        path: &Path,
        folder: bool,
    ) -> Self {
        Self {
            apps,
            destinations,
            folder,
            extension: (!folder).then(|| extension(path)).flatten(),
        }
    }
}

/// 小文字の拡張子。
pub fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_string_lossy().to_lowercase())
}

/// 右クリックメニューの「送る ▸」。
pub fn menu(ui: &mut egui::Ui, lang: Language, offer: &Offer<'_>) -> Option<Choice> {
    let mut chosen = None;
    ui.menu_button(tr(lang, "Send to"), |ui| {
        if !offer.folder && ui.button(tr(lang, "Open with the default app")).clicked() {
            chosen = Some(Choice::OpenDefault);
            ui.close();
        }
        let mut any = false;
        for (i, app) in offer.apps.iter().enumerate() {
            if !app.takes(offer.folder, offer.extension.as_deref()) {
                continue;
            }
            any = true;
            if ui.button(&app.name).clicked() {
                chosen = Some(Choice::App(i));
                ui.close();
            }
        }
        if !any && ui.button(tr(lang, "Set up apps...")).clicked() {
            chosen = Some(Choice::SetUpApps);
            ui.close();
        }
        if let Some(destinations) = offer.destinations {
            ui.separator();
            if let Some(c) = crate::dicom_send::menu_items(ui, lang, destinations) {
                chosen = Some(Choice::Network(c));
            }
        }
    });
    chosen
}

/// 送り先のアプリの一覧に入れる、関連付けられたアプリ。
pub fn from_associated(app: vw_launch::assoc::AssociatedApp) -> SendApp {
    SendApp {
        name: app.name,
        program: app.program.to_string_lossy().into_owned(),
        args: app.args,
        accepts: crate::settings::Accepts::Files,
        extensions: app.extensions.join("|"),
    }
}

/// `known` にまだ無いプログラムのアプリだけを足す。
pub fn merge(known: &mut Vec<SendApp>, found: Vec<SendApp>) -> usize {
    let before = known.len();
    for app in found {
        if !known.iter().any(|k| same_program(&k.program, &app.program)) {
            known.push(app);
        }
    }
    known.len() - before
}

fn same_program(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// 起動の予定。確かめてから起動するときは、これを持っておく。
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    /// 既定のアプリで 1 件ずつ開く。
    Default(Vec<PathBuf>),
    App {
        name: String,
        program: PathBuf,
        /// 起動 1 回ごとの引数。
        runs: Vec<Vec<OsString>>,
    },
}

impl Launch {
    pub fn app(app: &SendApp, paths: &[PathBuf]) -> Self {
        Launch::App {
            name: app.name.clone(),
            program: PathBuf::from(app.program.trim()),
            runs: vw_launch::args::launches(&app.template(), paths),
        }
    }

    /// アプリを起動する回数。
    pub fn count(&self) -> usize {
        match self {
            Launch::Default(paths) => paths.len(),
            Launch::App { runs, .. } => runs.len(),
        }
    }

    /// ログや確かめに出す、渡す先の名前。
    pub fn name(&self, lang: Language) -> String {
        match self {
            Launch::Default(_) => tr(lang, "the default app").to_owned(),
            Launch::App { name, .. } => name.clone(),
        }
    }

    /// 起動する。失敗の説明を返す。
    pub fn run(self) -> Vec<String> {
        let mut failures = Vec::new();
        match self {
            Launch::Default(paths) => {
                for path in paths {
                    if let Err(e) = vw_launch::open_file(&path) {
                        failures.push(format!("{}: {e}", path.display()));
                    }
                }
            }
            Launch::App { program, runs, .. } => {
                for args in runs {
                    if let Err(e) = vw_launch::run(&program, &args) {
                        failures.push(format!("{}: {e}", program.display()));
                    }
                }
            }
        }
        failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, program: &str) -> SendApp {
        SendApp {
            name: name.into(),
            program: program.into(),
            ..Default::default()
        }
    }

    #[test]
    fn merge_adds_only_new_programs() {
        let mut known = vec![app("Mine", "/usr/bin/gimp")];
        let added = merge(
            &mut known,
            vec![app("GIMP", "/usr/bin/gimp"), app("Viewer", "/usr/bin/eog")],
        );
        assert_eq!(added, 1);
        let names: Vec<_> = known.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Mine", "Viewer"]);
    }

    #[test]
    fn associated_app_takes_only_its_extensions() {
        let app = from_associated(vw_launch::assoc::AssociatedApp {
            name: "Raw".into(),
            program: "/usr/bin/raw".into(),
            args: "{paths}".into(),
            extensions: vec!["nef".into(), "arw".into()],
        });
        assert!(app.takes(false, Some("arw")));
        assert!(!app.takes(false, Some("jpg")));
        assert!(!app.takes(true, None));
    }

    #[test]
    fn launch_counts_one_run_per_path_with_path() {
        let mut a = app("A", "a");
        a.args = "{path}".into();
        let paths: Vec<PathBuf> = (0..7).map(|i| PathBuf::from(format!("/{i}"))).collect();
        assert_eq!(Launch::app(&a, &paths).count(), 7);
        a.args = "{paths}".into();
        assert_eq!(Launch::app(&a, &paths).count(), 1);
    }

    #[test]
    fn folder_offer_has_no_extension() {
        let offer = Offer::for_path(&[], None, Path::new("/a/b.c"), true);
        assert_eq!(offer.extension, None);
        let offer = Offer::for_path(&[], None, Path::new("/a/B.NEF"), false);
        assert_eq!(offer.extension.as_deref(), Some("nef"));
    }
}
