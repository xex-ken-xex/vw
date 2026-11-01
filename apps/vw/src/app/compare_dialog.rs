//! 「位置合わせして比べる」の小さなウインドウ。スロットに出ている画像から、比べるものと
//! 基準を選ぶ。選んだ画像の元の解像度のもの（色導出の結果）が揃ったら、比べる欄を開く。

use eframe::egui;

use super::{VwApp, request_full};
use crate::compare::{self, Input};
use crate::i18n::{tr, tr1};

/// 一度に比べる枚数の上限（並べたときに 1 枚がまだ見分けられる数）。
const MAX_IMAGES: usize = 9;

pub(super) struct Candidate {
    /// スロットの番号（[`crate::viewer::Slot::id`]）。
    slot: u64,
    name: String,
    chosen: bool,
}

pub(super) struct CompareDialog {
    candidates: Vec<Candidate>,
    /// 基準（`candidates` の添字）。
    reference: usize,
    match_brightness: bool,
    /// 「比べる」を押したあと、元の解像度の画像を待っている。
    waiting: bool,
    /// 揃った画像（スロットの番号ごと）。揃ったときに写しておくのは、ほかの画像を読む間に
    /// 元の解像度の画像がメモリの上限で捨てられても、待ち続けないため。
    gathered: Vec<(u64, Input)>,
}

impl VwApp {
    /// スロット `index` の右クリックの「位置合わせして比べる」。右クリックした画像を基準にし、
    /// ほかのスロットに出ている画像を全部選んでおく。
    pub(super) fn open_compare_dialog(&mut self, index: usize) {
        let order = std::iter::once(index).chain((0..self.slots.len()).filter(|&i| i != index));
        let candidates: Vec<Candidate> = order
            .filter_map(|i| {
                let s = &self.slots[i];
                let shown = s.shown()?;
                Some(Candidate {
                    slot: s.id,
                    name: compare::file_name(&shown.path),
                    chosen: false,
                })
            })
            .enumerate()
            .map(|(n, c)| Candidate {
                chosen: n < MAX_IMAGES,
                ..c
            })
            .collect();
        self.compare_dialog = Some(CompareDialog {
            candidates,
            reference: 0,
            match_brightness: true,
            waiting: false,
            gathered: Vec::new(),
        });
    }

    pub(super) fn compare_dialog_ui(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let Some(dialog) = &mut self.compare_dialog else {
            return;
        };
        let mut start = false;
        let mut cancel = false;
        egui::Window::new(tr(lang, "Align and compare"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(tr(
                    lang,
                    "Choose the images to compare and the reference. The others are moved, turned and scaled onto the reference.",
                ));
                let chosen = dialog.candidates.iter().filter(|c| c.chosen).count();
                if dialog.candidates.len() < 2 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        tr(lang, "Show two or more images in slots first."),
                    );
                }
                ui.add_enabled_ui(!dialog.waiting, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            egui::Grid::new("compare_picks")
                                .num_columns(2)
                                .show(ui, |ui| {
                                    ui.weak(tr(lang, "Reference"));
                                    ui.weak(tr(lang, "Compare"));
                                    ui.end_row();
                                    for (i, c) in dialog.candidates.iter_mut().enumerate() {
                                        if ui.radio(dialog.reference == i, "").clicked() {
                                            dialog.reference = i;
                                            c.chosen = true;
                                        }
                                        let full = chosen >= MAX_IMAGES && !c.chosen;
                                        ui.add_enabled(
                                            !full,
                                            egui::Checkbox::new(&mut c.chosen, &c.name),
                                        );
                                        ui.end_row();
                                    }
                                });
                        });
                    dialog.candidates[dialog.reference].chosen = true;
                    ui.checkbox(
                        &mut dialog.match_brightness,
                        tr(lang, "Match brightness before measuring differences"),
                    )
                    .on_hover_text(tr(
                        lang,
                        "Tone curves of RAW development and JPEG differ. Matching keeps them from counting as differences. Brightness offsets are always measured before matching.",
                    ));
                });
                if chosen >= MAX_IMAGES {
                    ui.weak(tr1(
                        lang,
                        "Up to {} images at once.",
                        &MAX_IMAGES.to_string(),
                    ));
                }
                ui.horizontal(|ui| {
                    if dialog.waiting {
                        ui.spinner();
                        ui.label(tr(lang, "Reading the images at full resolution"));
                    } else if ui
                        .add_enabled(chosen >= 2, egui::Button::new(tr(lang, "Compare")))
                        .clicked()
                    {
                        start = true;
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            self.compare_dialog = None;
            return;
        }
        if start {
            self.request_compare_images();
        }
        self.start_compare_when_ready(ctx);
    }

    /// 選んだスロットの元の解像度の画像を読ませる（捨ててあれば読み直す）。
    fn request_compare_images(&mut self) {
        let Some(dialog) = &mut self.compare_dialog else {
            return;
        };
        dialog.waiting = true;
        let ids: Vec<u64> = dialog
            .candidates
            .iter()
            .filter(|c| c.chosen)
            .map(|c| c.slot)
            .collect();
        for s in self.slots.iter_mut().filter(|s| ids.contains(&s.id)) {
            if s.shown().is_some_and(|sh| sh.full.is_none()) && s.full_request.is_none() {
                request_full(&self.loader, &self.color_rules, &mut self.next_id, s);
            }
        }
    }

    /// 選んだ画像が全部揃ったら、比べる欄を開く。スロットが消えていたら、それは外す。
    /// 元の解像度の画像をメモリに持てなかったときは、あきらめてログに出す。
    fn start_compare_when_ready(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.compare_dialog else {
            return;
        };
        if !dialog.waiting {
            return;
        }
        let reference = dialog.candidates[dialog.reference].slot;
        let mut ids: Vec<u64> = vec![reference];
        ids.extend(
            dialog
                .candidates
                .iter()
                .filter(|c| c.chosen && c.slot != reference)
                .map(|c| c.slot),
        );
        let mut ready = true;
        for &id in &ids {
            if self
                .compare_dialog
                .as_ref()
                .is_some_and(|d| d.gathered.iter().any(|(g, _)| *g == id))
            {
                continue;
            }
            let Some(index) = self.slots.iter().position(|s| s.id == id) else {
                continue;
            };
            let slot = &self.slots[index];
            let Some(shown) = slot.shown() else {
                continue;
            };
            if let Some((path, image)) = self.slot_copy_image(index) {
                if let Some(d) = &mut self.compare_dialog {
                    d.gathered.push((
                        id,
                        Input {
                            path,
                            image,
                            aligned: None,
                        },
                    ));
                }
                continue;
            }
            if shown.full.is_none() && slot.full_request.is_none() {
                self.log.error(format!(
                    "Compare: the full image of {} does not fit in memory (viewer.full_image_limit_mb)",
                    shown.path.display()
                ));
                self.compare_dialog = None;
                return;
            }
            ready = false;
        }
        if !ready {
            return;
        }
        let Some(mut dialog) = self.compare_dialog.take() else {
            return;
        };
        let match_brightness = dialog.match_brightness;
        let mut images: Vec<Input> = ids
            .iter()
            .filter_map(|id| {
                let at = dialog.gathered.iter().position(|(g, _)| g == id)?;
                Some(dialog.gathered.swap_remove(at).1)
            })
            .collect();
        if images.len() < 2 {
            return;
        }
        let (reference, rest) = images.split_first_mut().expect("two or more");
        for input in rest {
            input.aligned = self.saved_alignment(input, reference);
        }
        self.log.info(format!(
            "Compare: aligning {} images to {}",
            images.len() - 1,
            images[0].path.display()
        ));
        self.compare.start(ctx, images, match_brightness);
    }

    /// 見出しの似た画像の印から、その組を比べる。クリックした画像を基準にする。
    pub(super) fn compare_similar(&mut self, index: usize) {
        let id = self.slots[index].id;
        let Some(group) = self.similar.group(id).map(<[u64]>::to_vec) else {
            return;
        };
        let order = std::iter::once(id).chain(group.into_iter().filter(|&g| g != id));
        let candidates: Vec<Candidate> = order
            .filter_map(|g| {
                let s = self.slots.iter().find(|s| s.id == g)?;
                Some(Candidate {
                    slot: g,
                    name: compare::file_name(&s.shown()?.path),
                    chosen: true,
                })
            })
            .take(MAX_IMAGES)
            .collect();
        self.compare_dialog = Some(CompareDialog {
            candidates,
            reference: 0,
            match_brightness: true,
            waiting: false,
            gathered: Vec::new(),
        });
        self.request_compare_images();
    }

    /// 前に手で直した位置合わせ。直したときと大きさが違えば使わない。
    fn saved_alignment(&self, input: &Input, reference: &Input) -> Option<vw_compare::Affine> {
        let (path, reference_path) = alignment_keys(&input.path, &reference.path)?;
        let a = self.labels.aligned(&path, &reference_path)?;
        if a.size != input.image.size || a.reference_size != reference.image.size {
            return None;
        }
        let [a_, b, c, d, e, f] = a.transform;
        Some(vw_compare::Affine {
            a: a_,
            b,
            c,
            d,
            e,
            f,
        })
    }

    /// 手で直した位置合わせをラベルに残す。ZIP の中や合成した画像は書く場所がないので、
    /// この比べ方の間だけにする。
    fn save_alignment(
        &mut self,
        path: &std::path::Path,
        reference: &std::path::Path,
        aligned: Option<crate::labels::Aligned>,
    ) {
        let Some((path, reference)) = alignment_keys(path, reference) else {
            self.log.info(format!(
                "Compare: the alignment of {} is kept only while comparing",
                path.display()
            ));
            return;
        };
        if self.archives.mount_of(&path).is_some() {
            self.log.info(format!(
                "Compare: the alignment of {} is kept only while comparing (inside an archive)",
                path.display()
            ));
            return;
        }
        let what = if aligned.is_some() {
            "saved"
        } else {
            "removed"
        };
        match self.labels.set_aligned(&path, &reference, aligned) {
            Ok(()) => self.log.info(format!(
                "Compare: alignment of {} to {} {what}",
                path.display(),
                reference.display()
            )),
            Err(e) => self.log.error(format!("Compare: {e}")),
        }
    }

    /// 比べる欄。
    pub(super) fn compare_pane(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let lang = self.lang();
        for command in self.compare.ui(ui, lang) {
            match command {
                compare::Command::CopyImage(image, what) => {
                    ctx.copy_image(image);
                    self.copied(&ctx, &what);
                }
                compare::Command::CopyText(text, what) => self.copy_text(&ctx, text, &what),
                compare::Command::Info(text) => self.log.info(text),
                compare::Command::Error(text) => self.log.error(text),
                compare::Command::SaveAlignment {
                    path,
                    reference,
                    aligned,
                } => self.save_alignment(&path, &reference, aligned),
            }
        }
    }
}

/// 位置合わせを残すファイル（フレームを指すパスはそのファイル）。合成した画像には
/// ファイルがないので `None`。
fn alignment_keys(
    path: &std::path::Path,
    reference: &std::path::Path,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    if crate::compose::split(path).is_some() || crate::compose::split(reference).is_some() {
        return None;
    }
    Some((
        crate::frames::file_of(path),
        crate::frames::file_of(reference),
    ))
}
