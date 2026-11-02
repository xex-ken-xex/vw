//! フル解像度の上限に当たったときに、画像の上に出す「上限を上げますか」の帯。

use std::time::{Duration, Instant};

use eframe::egui::{self, vec2};

use crate::cache_hint;
use crate::frame_store;
use crate::i18n::tr;
use crate::settings::Viewer;

use super::*;

/// 空きメモリを調べ直す間隔。上限に当たり続けている間、毎フレーム調べないため。
const RECHECK: Duration = Duration::from_secs(30);

/// 出している勧め。
pub(super) struct MemoryHint {
    free_mb: u64,
    suggested_mb: u32,
}

/// 勧めの帯の状態。
#[derive(Default)]
pub(super) struct MemoryHintState {
    /// このフレームで上限に当たった。
    pub hit: bool,
    shown: Option<MemoryHint>,
    /// 「今はしない」を押した。次に起動するまで出さない。
    quiet: bool,
    checked: Option<Instant>,
}

impl VwApp {
    /// 上限に当たっていれば、空きメモリを調べて勧めを用意する。
    pub(super) fn memory_hint_tick(&mut self) {
        let viewer = &self.settings().viewer;
        let limit_mb = viewer.full_image_limit_mb;
        let enabled = viewer.suggest_full_image_limit;
        frame_store::STORE.set_budget(viewer.full_image_limit_bytes());
        let hit = std::mem::take(&mut self.memory_hint.hit) | frame_store::STORE.take_limited();
        let state = &mut self.memory_hint;
        if !hit || !enabled || state.quiet || state.shown.is_some() {
            return;
        }
        let now = Instant::now();
        if state.checked.is_some_and(|t| now - t < RECHECK) {
            return;
        }
        state.checked = Some(now);
        let Some(free) = cache_hint::available_memory() else {
            return;
        };
        let max = *Viewer::LIMIT_RANGE_MB.end();
        if let Some(suggested_mb) = cache_hint::suggestion(limit_mb, free, max) {
            state.shown = Some(MemoryHint {
                free_mb: free >> 20,
                suggested_mb,
            });
        }
    }

    /// 勧めの帯。ウインドウの下の中央に出す。上に出すとスロットのヘッダとページの数を隠すため。
    pub(super) fn memory_hint_ui(&mut self, ctx: &egui::Context) {
        let Some(hint) = &self.memory_hint.shown else {
            return;
        };
        let lang = self.lang();
        let limit = self.settings().viewer.full_image_limit_mb;
        let text = tr(
            lang,
            "Free memory is available ({free} GB). Raise the full-resolution memory limit from {from} MB to {to} MB?",
        )
        .replace("{free}", &format!("{:.1}", hint.free_mb as f64 / 1024.0))
        .replace("{from}", &limit.to_string())
        .replace("{to}", &hint.suggested_mb.to_string());
        let suggested = hint.suggested_mb;
        let mut answer = None;
        egui::Area::new(egui::Id::new("memory_hint"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -48.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(text);
                        if ui.button(tr(lang, "Raise")).clicked() {
                            answer = Some(Answer::Raise);
                        }
                        if ui.button(tr(lang, "Not now")).clicked() {
                            answer = Some(Answer::NotNow);
                        }
                        if ui.button(tr(lang, "Don't ask again")).clicked() {
                            answer = Some(Answer::Never);
                        }
                    });
                });
            });
        let Some(answer) = answer else {
            return;
        };
        self.memory_hint.shown = None;
        match answer {
            Answer::Raise => {
                self.settings.settings.viewer.full_image_limit_mb = suggested;
                self.log.info(format!(
                    "Full-resolution memory limit raised from {limit} MB to {suggested} MB"
                ));
                self.settings_changed(ctx);
            }
            Answer::NotNow => self.memory_hint.quiet = true,
            Answer::Never => {
                self.settings.settings.viewer.suggest_full_image_limit = false;
                self.settings_changed(ctx);
            }
        }
    }
}

enum Answer {
    Raise,
    NotNow,
    Never,
}
