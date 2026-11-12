//! 「モーメント図を作る」の小さなウインドウと、メモリの上の図を FITS で保存する窓。

use std::path::{Path, PathBuf};

use eframe::egui;
use vw_fits::moments::{Channels, Options, Order};

use super::VwApp;
use crate::i18n::tr;
use crate::moments::Spec;
use crate::viewer::Origin;

pub(super) struct MomentsDialog {
    /// 図を並べるスロット（[`crate::viewer::Slot::id`]）。その後ろに 1 枚ずつ足す。
    slot: u64,
    file: PathBuf,
    frame: usize,
    hdu: Option<usize>,
    channels: Option<Channels>,
    orders: [bool; 3],
    /// 使うチャンネル（0 から、両端を含む）。
    from: usize,
    to: usize,
    clip: f64,
}

/// 「FITS で保存...」の窓と、保存する図のパス。
pub(super) struct FitsSave {
    path: PathBuf,
    picker: Box<vw_tree::picker::Picker>,
}

const ORDER_NAMES: [&str; 3] = [
    "Moment 0 (integrated intensity)",
    "Moment 1 (velocity)",
    "Moment 2 (velocity spread)",
];

impl VwApp {
    /// スロット `index` の FITS のキューブのモーメント図を作る窓を開く。チャンネルの範囲は、
    /// 十字のスペクトルのグラフで選んだ範囲があればそれ、なければ全部。
    pub(super) fn open_moments_dialog(&mut self, index: usize) {
        let Some(shown) = self.slots[index].shown() else {
            return;
        };
        if !crate::viewer::is_fits_cube(&shown.info) {
            return;
        }
        let (file, frame) =
            crate::frames::split(&shown.path).unwrap_or_else(|| (shown.path.clone(), 0));
        let hdu = shown
            .info
            .metadata
            .get(vw_fits::GROUP, vw_fits::HDU)
            .and_then(|name| name.split_whitespace().next()?.parse().ok());
        let channels = vw_fits::moments::channels_of_file(&file, hdu);
        let last = channels.as_ref().map_or(0, |c| c.count.saturating_sub(1));
        let (from, to) = match &self.point_spectrum.range {
            Some((f, a, b)) if *f == file && *b <= last => (*a, *b),
            _ => (0, last),
        };
        let settings = &self.settings().moments;
        let orders = std::array::from_fn(|o| settings.orders.contains(&(o as u8)));
        let clip = settings.clip;
        self.moments_dialog = Some(MomentsDialog {
            slot: self.slots[index].id,
            file,
            frame,
            hdu,
            channels,
            orders,
            from,
            to,
            clip,
        });
    }

    pub(super) fn moments_dialog_ui(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let Some(dialog) = &mut self.moments_dialog else {
            return;
        };
        let mut answer: Option<bool> = None;
        egui::Window::new(tr(lang, "Make moment maps"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(tr(
                    lang,
                    "Sums the planes of the cube pixel by pixel along its spectral axis. Each map opens in a new slot; the file is not changed.",
                ));
                ui.add_space(4.0);
                for (o, on) in dialog.orders.iter_mut().enumerate() {
                    ui.checkbox(on, tr(lang, ORDER_NAMES[o]));
                }
                ui.add_space(4.0);
                match &dialog.channels {
                    Some(channels) => {
                        let last = channels.count - 1;
                        egui::Grid::new("moment_channels")
                            .num_columns(3)
                            .show(ui, |ui| {
                                for (label, value) in [
                                    ("First channel", &mut dialog.from),
                                    ("Last channel", &mut dialog.to),
                                ] {
                                    ui.label(tr(lang, label));
                                    let mut shown = *value + 1;
                                    ui.add(egui::DragValue::new(&mut shown).range(1..=last + 1));
                                    *value = shown - 1;
                                    ui.weak(crate::spectrum_view::point_text(
                                        &channels.axis,
                                        *value,
                                    ));
                                    ui.end_row();
                                }
                            });
                        if dialog.from > dialog.to {
                            std::mem::swap(&mut dialog.from, &mut dialog.to);
                        }
                        ui.horizontal(|ui| {
                            if ui.button(tr(lang, "All channels")).clicked() {
                                (dialog.from, dialog.to) = (0, last);
                            }
                            ui.weak(tr(
                                lang,
                                "Drag across the Spectrum at crosses graph to choose them there.",
                            ));
                        });
                    }
                    None => {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            tr(lang, "This file has no planes to follow."),
                        );
                    }
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(tr(lang, "Threshold"));
                    ui.add(
                        egui::DragValue::new(&mut dialog.clip)
                            .range(crate::settings::MomentSettings::CLIP_RANGE)
                            .speed(0.1)
                            .max_decimals(2),
                    );
                    ui.label(tr(lang, "x noise"));
                });
                ui.weak(tr(
                    lang,
                    "Only values above the median plus this many times the noise are summed. 0 sums every value.",
                ));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let ready = dialog.channels.is_some() && dialog.orders.iter().any(|o| *o);
                    if ui
                        .add_enabled(ready, egui::Button::new(tr(lang, "Make maps")))
                        .clicked()
                    {
                        answer = Some(true);
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        answer = Some(false);
                    }
                });
            });
        let Some(make) = answer else {
            return;
        };
        let Some(dialog) = self.moments_dialog.take() else {
            return;
        };
        if make {
            self.show_moments(&dialog);
        }
    }

    /// 選んだ次数の図を、元のスロットの後ろに 1 枚ずつ足して読み込む。
    fn show_moments(&mut self, dialog: &MomentsDialog) {
        let Some(mut at) = self.slots.iter().position(|s| s.id == dialog.slot) else {
            return;
        };
        let count = dialog.channels.as_ref().map_or(0, |c| c.count);
        let channels =
            (dialog.from > 0 || dialog.to + 1 < count).then_some((dialog.from, dialog.to));
        let full = self.cell_needs_full();
        for order in Order::ALL {
            if !dialog.orders[usize::from(order.number())] {
                continue;
            }
            let path = crate::moments::path(&Spec {
                file: dialog.file.clone(),
                options: Options {
                    order,
                    channels,
                    clip: dialog.clip,
                    frame: dialog.frame,
                    hdu: dialog.hdu,
                },
            });
            at += 1;
            let mut slot = crate::viewer::Slot::new(self.next_id);
            self.next_id += 1;
            slot.origin = Origin::Free;
            self.slots.insert(at, slot);
            self.start_load(at, path, full);
            self.active = at;
        }
    }

    /// 右クリックの「FITS で保存...」。名前を選ぶ窓を出す。
    pub(super) fn open_fits_save(&mut self, index: usize) {
        let Some(path) = self.slots[index].shown().map(|s| s.path.clone()) else {
            return;
        };
        let Some(spec) = crate::moments::split(&path) else {
            return;
        };
        let stem = spec
            .file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = format!("{stem}-moment{}.fits", spec.options.order.number());
        let lang = self.lang();
        let picker = vw_tree::picker::Picker::new(
            tr(lang, "Save as FITS"),
            vw_tree::picker::Mode::SaveFile {
                extensions: ["fits", "fit"].map(String::from).to_vec(),
                name,
            },
            spec.file.parent(),
            crate::settings_ui::picker_labels(lang),
        );
        self.fits_save = Some(FitsSave {
            path,
            picker: Box::new(picker),
        });
    }

    pub(super) fn fits_save_ui(&mut self, ctx: &egui::Context) {
        let Some(save) = &mut self.fits_save else {
            return;
        };
        match save.picker.show(ctx) {
            vw_tree::picker::Outcome::Open => {}
            vw_tree::picker::Outcome::Cancelled => self.fits_save = None,
            vw_tree::picker::Outcome::Chosen(to) => {
                let from = save.path.clone();
                self.fits_save = None;
                match save_fits(&from, &to) {
                    Ok(()) => self.log.info(format!("Saved {}", to.display())),
                    Err(e) => self.log.error(e),
                }
            }
        }
    }
}

/// メモリの上の図 `from` を `to` に書く。同じ名前のファイルがあれば書かない。
fn save_fits(from: &Path, to: &Path) -> Result<(), String> {
    let bytes = crate::moments::bytes(from)?;
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(format!("{}: already exists, not overwritten", to.display()));
        }
        Err(e) => return Err(format!("{}: {e}", to.display())),
    };
    std::io::Write::write_all(&mut file, &bytes).map_err(|e| format!("{}: {e}", to.display()))
}
