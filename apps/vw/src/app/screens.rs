//! 主画面を置き換える画面（設定・データセット・全画面）と、道具の欄（画素の変換・ヒストグラム・ズームマップ・地図）の中身。

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

    /// 画素の変換の欄。アクティブなスロットの画像を見せ、手で変えた値とノードを表示に当てる。
    pub(super) fn pipeline_pane(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let target = self.slots.get(self.active).and_then(|slot| {
            let shown = slot.shown.as_ref()?;
            Some(crate::pipeline_view::Target {
                path: &shown.path,
                image: shown.full.as_ref().map(|f| &f.image),
                info: &shown.info,
                marks: slot
                    .marks
                    .iter()
                    .filter(|m| m.path == shown.path)
                    .map(|m| (m.x, m.y))
                    .collect(),
                evaluation: Some((self.evaluation.report(), &self.settings.settings.quality)),
                values: slot.derive.as_ref().map(|d| &d.values),
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
        let commands = self.pipeline.ui(ui, lang, target);
        self.apply_pipeline(commands);
    }

    /// 色導出のウインドウでの操作を、アクティブなスロットと保存したパイプラインに当てる。
    pub(super) fn apply_pipeline(&mut self, commands: Vec<crate::pipeline_view::Command>) {
        use crate::pipeline_view::Command;
        for command in commands {
            let stored = match command {
                Command::Edit(values) => {
                    if let Some(slot) = self.slots.get_mut(self.active) {
                        crate::derive::edit(&mut slot.derive, values, self.presets.presets());
                    }
                    continue;
                }
                Command::Closed => {
                    for slot in &mut self.slots {
                        crate::derive::revert(&mut slot.derive, self.presets.presets());
                    }
                    continue;
                }
                Command::SaveAs(name, values) => {
                    let result = self.presets.put(&name, values.clone());
                    if let (Ok(()), Some(slot)) = (&result, self.slots.get_mut(self.active)) {
                        let d = slot.derive.get_or_insert_with(|| {
                            crate::derive::Derive::new(None, values.clone())
                        });
                        d.preset = Some(name.clone());
                        d.values = values;
                        d.modified = false;
                    }
                    result.map(|()| format!("Pipeline saved as \"{name}\""))
                }
                Command::Overwrite(name, values) => {
                    let result = self.presets.put(&name, values.clone());
                    if let (Ok(()), Some(d)) = (
                        &result,
                        self.slots
                            .get_mut(self.active)
                            .and_then(|s| s.derive.as_mut()),
                    ) {
                        d.values = values;
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
                edited: slot
                    .derive
                    .as_ref()
                    .and_then(|d| d.ready_for(shown))
                    .map(|r| &r.full),
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

    /// 解析の欄（画像全体のヒストグラムと評価）。色導出画面で値を変えていれば、変更後も重ねる。
    pub(super) fn histogram_pane(&mut self, ui: &mut egui::Ui) {
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
                derived: slot
                    .derive
                    .as_ref()
                    .and_then(|d| d.ready_for(shown))
                    .map(|r| &r.full),
                target_roi: slot.roi_of(crate::roi::Kind::Target),
                background_roi: slot.roi_of(crate::roi::Kind::Background),
            })
        });
        let evaluated = crate::histogram_view::Evaluated {
            report: self.evaluation.report(),
            computing: self.evaluation.computing(),
            thresholds: &self.settings.settings.quality,
        };
        match self.histogram.ui(ui, lang, target, evaluated) {
            Some(crate::quality_view::Command::Copy(text)) => {
                self.copy_text(&ui.ctx().clone(), text, "evaluation as CSV");
            }
            Some(crate::quality_view::Command::ClearReference) => {
                self.quality_reference = None;
            }
            Some(crate::quality_view::Command::OpenHelp) => {
                self.open_help(crate::help::Page::Quality);
            }
            None => {}
        }
    }

    /// ズームマップの欄。枠やつまみの操作をアクティブなスロットの表示に当てる。
    pub(super) fn zoom_map_pane(&mut self, ui: &mut egui::Ui) {
        use crate::zoom_map::Command;
        let lang = self.lang();
        let target = self.slots.get(self.active).and_then(|slot| {
            let shown = slot.shown.as_ref()?;
            let after = slot
                .derive
                .as_ref()
                .and_then(|d| d.ready_for(shown))
                .map(|r| &r.derived);
            Some(crate::zoom_map::Target {
                before: &shown.thumb,
                after,
                size: [shown.info.width, shown.info.height],
                view: slot.view_window()?,
            })
        });
        let commands = self.zoom_map.ui(ui, lang, target);
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

    /// 地図の欄が閉じていても毎フレームすること。
    pub(super) fn map_tick(&mut self, ctx: &egui::Context) {
        self.keep_place_names(ctx);
        self.drop_on_map(ctx);
        if !self.settings.settings.map.enabled {
            self.map.open = false;
        }
        self.map.forget_while_closed();
    }

    /// 地図の欄。点から開いた画像は、そのフォルダへ移って選ぶ。
    pub(super) fn map_pane(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let settings = &self.settings.settings;
        let cache = self
            .settings
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|d| d.join("map-tiles"));
        let registry = self.loader.registry().clone();
        let inputs = crate::map_window::Inputs {
            lang: self.lang(),
            settings: &settings.map,
            folder: self.current.as_deref(),
            roots: &settings.dataset.roots,
            registry: &registry,
            order: settings.tree.order(),
            cache: cache.as_deref(),
            places: self.place_names.as_deref(),
            labels: &self.labels,
        };
        let commands = self.map.ui(ui, &inputs);
        for problem in self.map.take_problems() {
            self.log.error(format!("Map: {problem}"));
        }
        for command in commands {
            match command {
                crate::map_window::Command::Open(path) => self.open_and_select(&ctx, path),
                crate::map_window::Command::ShowDownloads => self.ai_models.open = true,
                crate::map_window::Command::ClearPosition(path) => {
                    self.set_position(&path, None, true)
                }
                crate::map_window::Command::Undo => self.undo_position(),
            }
        }
    }

    /// 星図の欄が閉じていても毎フレームすること。
    pub(super) fn sky_tick(&mut self) {
        if !self.settings.settings.sky.enabled {
            self.sky.open = false;
        }
        self.sky.forget_while_closed();
    }

    /// 星図の欄。枠から開いた画像は、地図と同じくそのフォルダへ移って選ぶ。
    pub(super) fn sky_pane(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let settings = &self.settings.settings;
        let registry = self.loader.registry().clone();
        let active = self
            .slots
            .get(self.active)
            .and_then(|s| s.shown.as_ref())
            .map(|s| s.path.clone());
        let inputs = crate::sky_window::Inputs {
            lang: self.lang(),
            settings: &settings.sky,
            folder: self.current.as_deref(),
            roots: &settings.dataset.roots,
            registry: &registry,
            order: settings.tree.order(),
            active: active.as_deref(),
        };
        let commands = self.sky.ui(ui, &inputs);
        for problem in self.sky.take_problems() {
            self.log.error(format!("Sky chart: {problem}"));
        }
        for command in commands {
            match command {
                crate::sky_window::Command::Open(path) => self.open_and_select(&ctx, path),
            }
        }
    }

    /// 地図や星図から開いた画像のフォルダへ移り、その画像を選ぶ。読み込みの列ではその画像を
    /// 先にする（数百枚のフォルダで、選んだ画像が最後に読まれるのを待たないように）。
    fn open_and_select(&mut self, ctx: &egui::Context, path: PathBuf) {
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        // 今のフォルダなら読み直さずに選ぶ（スロットの表示や拡大を崩さないため）。
        if self.current.as_deref() != Some(dir.as_path()) {
            self.open_folder(ctx, dir);
        }
        self.select_when_loaded = Some(path);
    }

    /// 地図を使う設定で地名の表があれば読み込み、使わなくなったら手放す。
    fn keep_place_names(&mut self, ctx: &egui::Context) {
        use crate::ai_models::Item;
        let file = self
            .ai_store
            .as_ref()
            .filter(|s| self.settings.settings.map.enabled && s.has(Item::PlaceNames))
            .map(|s| s.place_names());
        if let Some(rx) = &self.place_names_load {
            let Ok(result) = rx.try_recv() else {
                return;
            };
            self.place_names_load = None;
            match result {
                Ok(g) => {
                    self.log
                        .info(format!("Place names: {} towns", g.town_count()));
                    self.place_names = Some(std::sync::Arc::new(g));
                }
                Err(e) => self.log.error(format!("Place names: {e}")),
            }
            crate::geonames::set_current(self.place_names.clone());
            return;
        }
        match (&file, &self.place_names) {
            (Some(path), None) => {
                let path = path.clone();
                self.place_names_load = Some(crate::background::compute(ctx, move || {
                    crate::geonames::Gazetteer::load(&path)
                }));
                // 読めないファイルを毎フレーム読み直さないよう、空の表を置いておく。
                self.place_names = Some(Default::default());
            }
            (None, Some(_)) => {
                self.place_names = None;
                crate::geonames::set_current(None);
            }
            _ => {}
        }
    }

    /// 地名の表をダウンロードし直したら、読み込み直す。
    pub(super) fn reload_place_names(&mut self) {
        self.place_names = None;
        crate::geonames::set_current(None);
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

    /// データセットの欄の中身。
    pub(super) fn datasets_pane(&mut self, ui: &mut egui::Ui) {
        use crate::datasets_screen::Action;
        let ctx = ui.ctx().clone();
        let lang = self.lang();
        let dataset = &self.settings.settings.dataset;
        let destinations = self.destination_names();
        let action = self.datasets.ui(
            ui,
            lang,
            &dataset.roots,
            &dataset.fields,
            &dataset.search_groups,
            &destinations,
        );
        match action {
            Some(Action::Show(dir)) => self.show_dataset(&ctx, dir),
            Some(Action::ShowFiles(dir, files)) => self.show_dataset_files(&dir, &files),
            Some(Action::Extract(dir, hits)) => {
                self.open_dataset_dialog(&ctx, crate::dataset_ui::Request::Extract(dir, hits));
            }
            Some(Action::Edit(dir)) => {
                self.open_dataset_dialog(&ctx, crate::dataset_ui::Request::Edit(dir));
            }
            Some(Action::Update(dirs)) => self.dataset_queue.extend(dirs),
            Some(Action::Send(dir, hits, choice)) => {
                let targets = crate::dicom_send::dataset_targets(&dir, hits.as_deref());
                self.send_targets(choice, targets);
            }
            Some(Action::SaveGroup(group)) => self.save_search_group(&ctx, group),
            Some(Action::Reload) => {
                self.datasets.reload(&dataset.roots, &ctx);
                // ツリーのデータセットの色も、カタログの有無に合わせて付け直す。
                self.tree.forget_peeks();
            }
            None => {}
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
