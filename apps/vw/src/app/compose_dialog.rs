//! 「色を合成」の小さなウインドウ。スロットに出ている灰色の画像から、赤・緑・青に当てるものを選ぶ。

use std::path::PathBuf;

use eframe::egui;
use vw_core::ColorModel;

use super::VwApp;
use crate::compose::{self, Files, CHANNELS};
use crate::i18n::tr;

/// 選べる画像。
pub(super) struct Candidate {
    path: PathBuf,
    filter: Option<String>,
}

pub(super) struct ComposeDialog {
    /// 合成を出すスロットの番号（[`crate::viewer::Slot::id`]）。
    slot: u64,
    candidates: Vec<Candidate>,
    /// 赤・緑・青に選んだもの（`candidates` の添字）。
    picks: [Option<usize>; 3],
}

impl VwApp {
    /// スロット `index` の右クリックの「色を合成」。スロットに出ている灰色の画像を候補にし、
    /// FITS の `FILTER` で色を決めておく。合成を出しているスロットなら、今の割り当てから始める。
    pub(super) fn open_compose_dialog(&mut self, index: usize) {
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut push = |path: PathBuf, filter: Option<String>| {
            if !candidates.iter().any(|c| c.path == path) {
                candidates.push(Candidate { path, filter });
            }
        };
        let current = self.slots[index]
            .shown
            .as_ref()
            .and_then(|s| s.info.compose.clone());
        if let Some(c) = &current {
            for ch in c.channels.iter().flatten() {
                push(ch.path.clone(), ch.filter.clone());
            }
        }
        // 右クリックしたスロットを先に、残りは並んでいる順に。
        let order = std::iter::once(index).chain((0..self.slots.len()).filter(|&i| i != index));
        for i in order {
            let Some(shown) = &self.slots[i].shown else {
                continue;
            };
            if shown.info.compose.is_none() && shown.info.color == ColorModel::Gray {
                push(shown.path.clone(), compose::filter_of(&shown.info.metadata));
            }
        }
        let picks = match &current {
            Some(c) => c
                .files()
                .map(|f| f.and_then(|f| candidates.iter().position(|c| c.path == f))),
            None => {
                let listed: Vec<(PathBuf, Option<String>)> = candidates
                    .iter()
                    .map(|c| (c.path.clone(), c.filter.clone()))
                    .collect();
                compose::assign(&listed)
                    .map(|f| f.and_then(|f| candidates.iter().position(|c| c.path == f)))
            }
        };
        self.compose_dialog = Some(ComposeDialog {
            slot: self.slots[index].id,
            candidates,
            picks,
        });
    }

    pub(super) fn compose_dialog_ui(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let Some(dialog) = &mut self.compose_dialog else {
            return;
        };
        let mut answer: Option<Option<Files>> = None;
        egui::Window::new(tr(lang, "Compose colors"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(tr(
                    lang,
                    "Choose the gray images for red, green and blue. They are put on top of each other in the slot.",
                ));
                if dialog.candidates.len() < 2 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        tr(lang, "Show two or three gray images in slots first."),
                    );
                }
                let title = |c: &Candidate| {
                    let name = c
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    match &c.filter {
                        Some(f) => format!("{name}  ({f})"),
                        None => name,
                    }
                };
                egui::Grid::new("compose_picks").num_columns(2).show(ui, |ui| {
                    for (c, pick) in dialog.picks.iter_mut().enumerate() {
                        ui.label(tr(lang, CHANNELS[c]));
                        let shown = pick.map_or_else(
                            || tr(lang, "None").to_owned(),
                            |i| title(&dialog.candidates[i]),
                        );
                        vw_ui::menu::combo(("compose_pick", c))
                            .selected_text(shown)
                            .width(320.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(pick, None, tr(lang, "None"));
                                for (i, cand) in dialog.candidates.iter().enumerate() {
                                    ui.selectable_value(pick, Some(i), title(cand));
                                }
                            });
                        ui.end_row();
                    }
                });
                if ui
                    .button(tr(lang, "Choose by the FILTER of the files"))
                    .clicked()
                {
                    let chosen: Vec<(PathBuf, Option<String>)> = dialog
                        .picks
                        .iter()
                        .flatten()
                        .map(|&i| {
                            let c = &dialog.candidates[i];
                            (c.path.clone(), c.filter.clone())
                        })
                        .collect();
                    dialog.picks = compose::assign(&chosen).map(|f| {
                        f.and_then(|f| dialog.candidates.iter().position(|c| c.path == f))
                    });
                }
                ui.weak(tr(
                    lang,
                    "The first image sets the size. Images with sky coordinates (WCS) are moved, turned and scaled onto it; others are centered on it.",
                ));
                let files: Files = dialog
                    .picks
                    .map(|p| p.map(|i| dialog.candidates[i].path.clone()));
                let count = files.iter().flatten().count();
                let distinct = {
                    let mut f: Vec<_> = files.iter().flatten().collect();
                    f.sort();
                    f.dedup();
                    f.len() == count
                };
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            count >= 2 && distinct,
                            egui::Button::new(tr(lang, "Compose")),
                        )
                        .clicked()
                    {
                        answer = Some(Some(files));
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        answer = Some(None);
                    }
                });
            });
        let Some(answer) = answer else {
            return;
        };
        let slot = self.compose_dialog.take().map(|d| d.slot);
        let (Some(files), Some(index)) = (
            answer,
            slot.and_then(|id| self.slots.iter().position(|s| s.id == id)),
        ) else {
            return;
        };
        self.show_composite(index, files);
    }

    /// スロット `index` に `files` を重ねた画像を出す。色ごとの黒と白は、同じファイルを
    /// 同じ色のまま使うときだけ引き継ぐ。
    pub(super) fn show_composite(&mut self, index: usize, files: Files) {
        let Some(path) = compose::path(&files) else {
            return;
        };
        let before = self.slots[index]
            .shown
            .as_ref()
            .and_then(|s| s.info.compose.as_ref().map(|c| c.files()));
        let slot = &mut self.slots[index];
        let kept = slot.color.channels;
        slot.color.channels = std::array::from_fn(|c| {
            let same = before.as_ref().is_some_and(|b| b[c] == files[c]);
            if same {
                kept[c]
            } else {
                // 別の色に移したファイルは、その色の黒と白を持っていく。
                before
                    .as_ref()
                    .and_then(|b| b.iter().position(|f| f.is_some() && *f == files[c]))
                    .and_then(|from| kept[from])
            }
        });
        slot.sequence = None;
        slot.from_folder = false;
        let full = self.cell_needs_full();
        self.start_load(index, path, full);
    }
}
