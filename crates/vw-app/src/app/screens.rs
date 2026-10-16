//! 主画面を置き換える画面（設定・データセット・全画面）と、別ウインドウ（画素の変換・ヒストグラム・ズームマップ）の開閉。

use std::path::PathBuf;

use eframe::egui;

use super::*;

impl VwApp {
    /// 右クリックの「送信 ▸」で選ばれたものを処理する。送るのは確かめてから。
    pub(super) fn toggle_pipeline(&mut self) {
        if self.pipeline.open {
            let commands = self.pipeline.close();
            self.apply_pipeline(commands);
        } else {
            self.pipeline.open = true;
        }
    }

    /// 画素の変換の図。アクティブなスロットの DICOM 画像を見せ、手で変えた値を表示に当てる。
    pub(super) fn pipeline_ui(&mut self, ctx: &egui::Context) {
        if !self.pipeline.open {
            return;
        }
        let lang = self.lang();
        let target = self.slots.get(self.active).and_then(|slot| {
            let shown = slot.shown.as_ref().filter(|s| s.info.plugin == "DICOM")?;
            Some(crate::pipeline_view::Target {
                path: &shown.path,
                image: shown.full.as_ref().map(|f| &f.image),
                marks: slot
                    .marks
                    .iter()
                    .filter(|m| m.path == shown.path)
                    .map(|m| (m.x, m.y))
                    .collect(),
                evaluation: Some((self.evaluation.report(), &self.settings.settings.quality)),
                params: slot.derive.as_ref().map(|d| d.params),
                preset: slot
                    .derive
                    .as_ref()
                    .and_then(|d| Some((d.preset.as_deref()?, d.modified))),
                saved: self
                    .presets
                    .presets()
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect(),
            })
        });
        let commands = self.pipeline.ui(ctx, lang, target);
        self.apply_pipeline(commands);
    }

    /// 色導出のウインドウでの操作を、アクティブなスロットと保存したパイプラインに当てる。
    pub(super) fn apply_pipeline(&mut self, commands: Vec<crate::pipeline_view::Command>) {
        use crate::pipeline_view::Command;
        for command in commands {
            let stored = match command {
                Command::Edit { params, at_file } => {
                    if let Some(slot) = self.slots.get_mut(self.active) {
                        crate::derive::edit(
                            &mut slot.derive,
                            params,
                            at_file,
                            self.presets.presets(),
                        );
                    }
                    continue;
                }
                Command::Closed => {
                    for slot in &mut self.slots {
                        crate::derive::revert(&mut slot.derive, self.presets.presets());
                    }
                    continue;
                }
                Command::SaveAs(name, params) => {
                    let result = self.presets.put(&name, params);
                    if let (Ok(()), Some(slot)) = (&result, self.slots.get_mut(self.active)) {
                        let d = slot
                            .derive
                            .get_or_insert_with(|| crate::derive::Derive::new(None, params));
                        d.preset = Some(name.clone());
                        d.params = params;
                        d.modified = false;
                    }
                    result.map(|()| format!("Pipeline saved as \"{name}\""))
                }
                Command::Overwrite(name, params) => {
                    let result = self.presets.put(&name, params);
                    if let (Ok(()), Some(d)) = (
                        &result,
                        self.slots
                            .get_mut(self.active)
                            .and_then(|s| s.derive.as_mut()),
                    ) {
                        d.params = params;
                        d.modified = false;
                    }
                    result.map(|()| format!("Pipeline \"{name}\" overwritten"))
                }
                Command::Delete(name) => self
                    .presets
                    .remove(&name)
                    .map(|()| format!("Pipeline \"{name}\" deleted")),
            };
            match stored {
                Ok(message) => self.log.info(message),
                Err(e) => self.log.error(format!("Pipelines: {e}")),
            }
            self.sync_derived_slots();
        }
    }

    fn sync_derived_slots(&mut self) {
        let presets = self.presets.presets();
        for d in self.slots.iter_mut().filter_map(|s| s.derive.as_mut()) {
            d.sync(presets);
        }
    }

    /// `pipelines.toml` が外部で書き換えられていたら読み直し、選んでいるスロットにも当てる。
    pub(super) fn reload_presets(&mut self) {
        let Some(problems) = self.presets.reload_if_changed() else {
            return;
        };
        self.log.info("Pipelines reloaded from file");
        for p in problems {
            self.log.error(format!("Pipelines: {p}"));
        }
        self.sync_derived_slots();
    }

    pub(super) fn log_derive(&mut self, event: crate::derive::Event) {
        use crate::derive::Event;
        match event {
            Event::Refused {
                path,
                mismatch,
                left,
            } => {
                let what = if left {
                    "Derived view not entered"
                } else {
                    "Shown in the standard display"
                };
                self.log.error(format!(
                    "{what}: {}: the file values do not reproduce the standard display \
                     ({} pixels differ, by up to {})",
                    path.display(),
                    mismatch.pixels,
                    mismatch.max
                ));
            }
            Event::Failed(path, e) => self.log.error(format!(
                "Derived view: {}: could not read the pipeline: {e}",
                path.display()
            )),
        }
    }

    /// 定量評価。解析か色導出のウインドウが開いている間、アクティブなスロットの画像を評価する。
    pub(super) fn update_evaluation(&mut self, ctx: &egui::Context) {
        if !(self.pipeline.open || self.histogram.open) {
            self.evaluation.clear();
            return;
        }
        let reference = self.quality_reference.as_deref().and_then(|path| {
            let shown = self
                .slots
                .iter()
                .filter_map(|s| s.shown.as_ref())
                .find(|s| s.path == path)?;
            Some(crate::quality::Reference {
                path,
                image: shown.full.as_ref().map(|f| &f.image),
            })
        });
        let input = self.slots.get(self.active).and_then(|slot| {
            let shown = slot.shown.as_ref()?;
            Some(crate::quality::Input {
                path: &shown.path,
                image: shown.full.as_ref().map(|f| &f.image),
                edited: slot.derive.as_ref().and_then(|d| d.edit_for(&shown.path)),
                target_roi: slot.roi_of(crate::roi::Kind::Target),
                background_roi: slot.roi_of(crate::roi::Kind::Background),
                // 基準が自分なら変更前と比べるのと同じなので、基準なしとして扱う。
                reference: reference.filter(|r| r.path != shown.path),
            })
        });
        self.evaluation.update(ctx, input.as_ref());
    }

    /// スロット `index` の画像を PSNR・SSIM の基準にする。
    pub(super) fn set_quality_reference(&mut self, index: usize) {
        let Some(shown) = self.slots.get(index).and_then(|s| s.shown.as_ref()) else {
            return;
        };
        self.quality_reference = Some(shown.path.clone());
        self.log
            .info(format!("Evaluation reference: {}", shown.path.display()));
        if !self.histogram.open {
            self.histogram.open = true;
        }
    }

    pub(super) fn toggle_histogram(&mut self) {
        if self.histogram.open {
            self.histogram.close();
        } else {
            self.histogram.open = true;
        }
    }

    /// 画像全体のヒストグラム。色導出画面で値を変えていれば、変更後も重ねる。
    pub(super) fn histogram_ui(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.pipeline.open_histogram) {
            self.histogram.open = true;
        }
        if !self.histogram.open {
            return;
        }
        let lang = self.lang();
        let target = self.slots.get(self.active).and_then(|slot| {
            let shown = slot.shown.as_ref()?;
            Some(crate::histogram_view::Target {
                path: &shown.path,
                image: shown.full.as_ref().map(|f| &f.image),
                dicom: shown.info.plugin == "DICOM",
                edited: slot
                    .derive
                    .as_ref()
                    .and_then(|d| d.edit_for(&shown.path))
                    .map(|(p, _)| p),
                target_roi: slot.roi_of(crate::roi::Kind::Target),
                background_roi: slot.roi_of(crate::roi::Kind::Background),
            })
        });
        let evaluated = crate::histogram_view::Evaluated {
            report: self.evaluation.report(),
            computing: self.evaluation.computing(),
            thresholds: &self.settings.settings.quality,
        };
        match self.histogram.ui(ctx, lang, target, evaluated) {
            Some(crate::quality_view::Command::Copy(text)) => {
                self.copy_text(ctx, text, "evaluation as CSV");
            }
            Some(crate::quality_view::Command::ClearReference) => {
                self.quality_reference = None;
            }
            None => {}
        }
    }

    /// ズームマップ。枠やつまみの操作をアクティブなスロットの表示に当てる。
    pub(super) fn zoom_map_ui(&mut self, ctx: &egui::Context) {
        use crate::zoom_map::Command;
        if std::mem::take(&mut self.pipeline.open_zoom_map) {
            self.zoom_map.open = true;
        }
        if !self.zoom_map.open {
            return;
        }
        let lang = self.lang();
        let target = self.slots.get(self.active).and_then(|slot| {
            let shown = slot.shown.as_ref()?;
            let after = slot
                .derive
                .as_ref()
                .and_then(|d| d.ready_for(shown))
                .map(|r| &r.derived);
            let name = crate::frames::file_of(&shown.path);
            Some(crate::zoom_map::Target {
                title: name
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                before: &shown.thumb,
                after,
                size: [shown.info.width, shown.info.height],
                view: slot.view_window()?,
            })
        });
        let commands = self.zoom_map.ui(ctx, lang, target);
        let Some(slot) = self.slots.get_mut(self.active) else {
            return;
        };
        for c in commands {
            match c {
                Command::CenterAt(at) => slot.center_at(at),
                Command::SetScale(scale) => slot.set_scale(scale),
                Command::Fit => slot.reset_view(),
                Command::ActualSize => slot.actual_size(),
            }
        }
    }

    pub(super) fn toggle_datasets(&mut self, ctx: &egui::Context) {
        self.show_datasets = !self.show_datasets;
        if self.show_datasets {
            self.show_settings = false;
            self.show_fullscreen_setup = false;
            self.datasets
                .opened(&self.settings.settings.dataset.roots, ctx);
            self.tree.forget_peeks();
        }
    }

    pub(super) fn toggle_settings(&mut self) {
        self.show_settings = !self.show_settings;
        if self.show_settings {
            self.settings_ui.opened();
        }
    }

    pub(super) fn toggle_fullscreen_setup(&mut self) {
        self.show_fullscreen_setup = !self.show_fullscreen_setup;
        if self.show_fullscreen_setup {
            self.show_settings = false;
            self.show_datasets = false;
            self.fullscreen_setup.open(
                self.last_fullscreen_root.as_deref(),
                self.current.as_deref(),
            );
        }
    }

    pub(super) fn start_fullscreen(&mut self, ctx: &egui::Context, root: PathBuf) {
        self.stop_playback();
        self.next_id += 1;
        let (show, problems) = crate::fullscreen::Show::start(
            root.clone(),
            &self.settings().fullscreen,
            self.loader.registry(),
            self.next_id,
            ctx,
        );
        for p in problems {
            self.log.error(p);
        }
        self.log
            .info(format!("Fullscreen slideshow: {}", root.display()));
        self.fullscreen = Some(show);
        self.last_fullscreen_root = Some(root);
        self.show_fullscreen_setup = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
    }

    pub(super) fn stop_fullscreen(&mut self, ctx: &egui::Context) {
        if let Some(mut show) = self.fullscreen.take() {
            show.stop(&self.loader);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }

    /// 全画面スライドショーの 1 フレーム。
    pub(super) fn fullscreen_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let lang = self.lang();
        let Some(show) = &mut self.fullscreen else {
            return;
        };
        let requests = egui::CentralPanel::no_frame()
            .show(ui, |ui| {
                show.ui(
                    ui,
                    &self.settings.settings.keys,
                    &self.loader,
                    &mut self.next_id,
                    lang,
                )
            })
            .inner;
        if requests
            .iter()
            .any(|r| matches!(r, crate::fullscreen::Request::Exit))
        {
            self.stop_fullscreen(&ctx);
        }
    }
}
