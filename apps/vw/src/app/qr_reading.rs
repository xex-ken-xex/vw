//! 画像の QR コードを裏のスレッドで読み、ログとメタデータ欄に出す。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;

use super::VwApp;
use crate::i18n::tr;
use crate::qr::{self, Found};

/// メタデータ欄の QR コードのグループの英語の名前。
pub(super) const QR_CODES: &str = "QR codes";

type Answer = (PathBuf, Result<Found, String>);

pub(super) struct QrReading {
    /// 読んだ結果。表示中の画像（フレームまで含めたパス）ごと。起動している間だけ持つ。
    results: HashMap<PathBuf, Found>,
    tx: Sender<Answer>,
    rx: Receiver<Answer>,
    /// 読んでいる最中の画像。同じ画像を続けて頼まれても 1 度だけ読む。
    reading: Vec<PathBuf>,
}

impl Default for QrReading {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            results: HashMap::new(),
            tx,
            rx,
            reading: Vec::new(),
        }
    }
}

impl VwApp {
    /// アクティブなスロットに表示中の画像の QR コードを読む。
    pub(super) fn read_qr_codes_active(&mut self, ctx: &egui::Context) {
        match self.slots.get(self.active).and_then(|s| s.shown()) {
            Some(shown) => {
                let path = shown.path.clone();
                self.read_qr_codes(ctx, path);
            }
            None => self.log.warn("No image to read QR codes from"),
        }
    }

    pub(super) fn read_qr_codes(&mut self, ctx: &egui::Context, path: PathBuf) {
        if self.qr.reading.contains(&path) {
            return;
        }
        self.qr.reading.push(path.clone());
        let registry = self.loader.registry().clone();
        let tx = self.qr.tx.clone();
        let ctx = ctx.clone();
        // 縮めずに読む。写真の隅に小さく写った QR コードは、縮めるとモジュールがつぶれて読めなくなるため。
        std::thread::spawn(move || {
            let found =
                crate::loader::load_fit(&registry, &path, false, true, None).and_then(|l| {
                    l.full
                        .map(|f| qr::read(&f.display))
                        .ok_or_else(|| "no image".to_owned())
                });
            let _ = tx.send((path, found));
            ctx.request_repaint();
        });
    }

    /// 届いた結果をログに出し、メタデータ欄に出せるように持つ。
    pub(super) fn qr_ui(&mut self) {
        let answers: Vec<Answer> = self.qr.rx.try_iter().collect();
        for (path, found) in answers {
            self.qr.reading.retain(|p| p != &path);
            let name = display_name(&path);
            match found {
                Err(e) => self
                    .log
                    .error(format!("Cannot read QR codes in {name}: {e}")),
                Ok(found) => {
                    if found.codes.is_empty() && found.unreadable.is_empty() {
                        self.log.info(format!("No QR code found in {name}"));
                    }
                    for c in &found.codes {
                        let [x, y] = c.corners[0];
                        self.log.done(
                            "Read",
                            format!("QR code at ({x}, {y}) in {name}: {}", c.text),
                        );
                    }
                    for u in &found.unreadable {
                        let [x, y] = u.corners[0];
                        self.log.warn(format!(
                            "Found a QR code at ({x}, {y}) in {name} but cannot read it: {}",
                            u.reason
                        ));
                    }
                    self.qr.results.insert(path, found);
                }
            }
        }
    }

    /// メタデータ欄の QR コードのグループの行。読んでいなければ `None`。
    pub(super) fn qr_rows(&self, path: &Path) -> Option<Vec<crate::meta_view::Row>> {
        let lang = self.lang();
        let found = self.qr.results.get(path)?;
        let mut rows: Vec<_> = found
            .codes
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let key = format!("QR {}", i + 1);
                crate::meta_view::Row::new(QR_CODES, &key, key.clone(), c.text.clone())
            })
            .collect();
        if !found.unreadable.is_empty() {
            rows.push(crate::meta_view::Row::new(
                QR_CODES,
                "Unreadable",
                tr(lang, "Unreadable").to_owned(),
                found.unreadable.len().to_string(),
            ));
        }
        if rows.is_empty() {
            rows.push(crate::meta_view::Row::new(
                QR_CODES,
                "Found",
                tr(lang, "Found").to_owned(),
                tr(lang, "None").to_owned(),
            ));
        }
        Some(rows)
    }
}

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}
