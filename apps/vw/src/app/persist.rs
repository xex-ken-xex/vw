//! 設定の変更の反映と、ウインドウの位置などの状態の保存・復元。

use eframe::egui::{self, pos2, vec2};

use crate::state::{self, AppState, Monitor, WindowState};
use crate::{theme, window_placement};

use super::*;

impl VwApp {
    /// 検索のグループを設定に足して保存する。同じ名前の組は置き換える。
    pub(super) fn save_search_group(
        &mut self,
        ctx: &egui::Context,
        group: crate::settings::SearchGroup,
    ) {
        let groups = &mut self.settings.settings.dataset.search_groups;
        let message = format!("Search group {:?}: {}", group.name, group.keys.join("; "));
        match groups.iter_mut().find(|g| g.name == group.name) {
            Some(g) => *g = group,
            None => groups.push(group),
        }
        self.log.info(message);
        self.settings_changed(ctx);
    }

    /// 検索に名前を付けて設定に取っておく。同じ名前の検索は置き換える。
    pub(super) fn save_search(
        &mut self,
        ctx: &egui::Context,
        search: crate::settings::SavedSearch,
    ) {
        let searches = &mut self.settings.settings.dataset.saved_searches;
        let message = format!("Saved search {:?}", search.name);
        match searches.iter_mut().find(|s| s.name == search.name) {
            Some(s) => *s = search,
            None => searches.push(search),
        }
        self.log.info(message);
        self.settings_changed(ctx);
    }

    /// 設定画面で値が変わったときに、反映して保存する。
    pub(super) fn settings_changed(&mut self, ctx: &egui::Context) {
        theme::apply(ctx, &self.settings().appearance);
        self.tree.set_order(self.settings().tree.order());
        self.apply_probe_extensions();
        self.apply_fits_options();
        self.apply_netcdf_options();
        self.apply_color_rules();
        self.apply_texture_options();
        self.apply_archive_dir();
        self.apply_image_size_limit();
        self.apply_repair();
        match self.settings.save() {
            Ok(Some(backup)) => self.log.info(format!(
                "Settings: the previous file had errors and was kept as {}",
                backup.display()
            )),
            Ok(None) => {}
            Err(e) => self.log.error(format!("Settings: {e}")),
        }
    }

    /// ZIP や 7z の展開先が変わったら、これから開くものをそこへ展開する。
    pub(super) fn apply_archive_dir(&mut self) {
        let limit = self.settings().behavior.archive_size_limit_bytes();
        self.archives.set_size_limit(limit);
        self.archives
            .set_threads(self.settings().behavior.archive_threads);
        let before = self.archives.base().to_path_buf();
        let parent = self.settings().behavior.archive_parent();
        self.archives.set_parent(parent.as_deref());
        if self.archives.base() != before {
            let message = format!("Archives: extracting to {}", self.archives.base().display());
            self.log.info(message);
        }
    }

    /// DICOM か調べる拡張子が変わったら、開いているフォルダを読み直してツリーにすぐ反映する。
    pub(super) fn apply_probe_extensions(&mut self) {
        let dicom = &self.settings.settings.dicom;
        if dicom.probe_extensions == self.probe_extensions {
            return;
        }
        self.probe_extensions = dicom.probe_extensions.clone();
        let registry = self.loader.registry().clone();
        registry.set_probe_extensions(dicom.probe());
        self.tree.forget_peeks();
        for dir in self.tree.open_paths() {
            self.tree.refresh(&dir, &registry);
        }
    }

    /// 1 枚の画像に使ってよいメモリの上限を、デコーダに知らせる。
    pub(super) fn apply_image_size_limit(&self) {
        vw_core::limits::set_max_image_bytes(self.settings().viewer.image_size_limit_bytes());
    }

    /// 形式に反したファイルの補修の設定を、デコーダに知らせる。
    pub(super) fn apply_repair(&self) {
        vw_tiff::set_repair_strip_byte_counts(self.settings().repair.tiff_strip_byte_counts);
    }

    /// 天体の画像の表示や校正の決め方が変わったら、表示中の FITS を読み直してすぐ反映する。
    /// 黒と白や上下、色、校正はデコードのときに決まるので、読み直すほかない。
    pub(super) fn apply_fits_options(&mut self) {
        let options = self.settings().fits.options();
        let calibration = self.settings().fits.calibration.options();
        if options == self.fits.get() && calibration == self.calibration.options() {
            return;
        }
        self.fits.set(options);
        self.calibration.set_options(calibration);
        self.calibration_logged.clear();
        self.reload_fits_slots();
    }

    /// 地球の格子のデータの表示の決め方が変わったら、表示中の NetCDF を読み直す。
    pub(super) fn apply_netcdf_options(&mut self) {
        let options = self.settings().netcdf.options();
        if options == self.netcdf.get() {
            return;
        }
        self.netcdf.set(options);
        self.reload_plugin_slots(vw_netcdf::NAME);
    }

    /// 画素の変換の設定（色の表の段）が変わったら、表示中の灰色の画像を読み直す。
    pub(super) fn apply_color_rules(&mut self) {
        if self.settings().pipeline == self.color_settings {
            return;
        }
        self.color_settings = self.settings().pipeline.clone();
        self.color_rules = Arc::new(crate::colormap::Rules::new(&self.color_settings));
        let gray: Vec<usize> = (0..self.slots.len())
            .filter(|&i| self.slots[i].shown().is_some_and(|d| d.info.look.is_some()))
            .collect();
        for index in gray {
            self.reload_slot(index);
        }
    }

    /// スロットの表示中の画像を、同じ大きさのまま読み直す（色の表や黒と白を変えたとき）。
    pub(super) fn reload_slot(&mut self, index: usize) {
        let Some(shown) = self.slots[index].shown() else {
            return;
        };
        let path = shown.path.clone();
        let full = self.cell_needs_full() || shown.full.is_some();
        self.start_load(index, path, full);
    }

    /// 表示中の FITS を読み直す。
    pub(super) fn reload_fits_slots(&mut self) {
        self.reload_plugin_slots("FITS");
    }

    /// プラグイン `plugin` で読んだ、表示中の画像を読み直す。
    pub(super) fn reload_plugin_slots(&mut self, plugin: &str) {
        let full = self.cell_needs_full();
        let shown: Vec<(usize, PathBuf)> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.shown().is_some_and(|d| d.info.plugin == plugin))
            .filter_map(|(i, s)| Some((i, s.path.clone()?)))
            .collect();
        for (index, path) in shown {
            self.start_load(index, path, full);
        }
    }

    /// 拡大・縮小の補間が変わったら、表示中の画像にもすぐ当てる。
    pub(super) fn apply_texture_options(&mut self) {
        let options = self.settings().viewer.texture_options();
        if options == self.texture_options {
            return;
        }
        self.texture_options = options;
        for d in self.slots.iter_mut().filter_map(|s| s.derive.as_mut()) {
            for texture in d.textures_mut() {
                crate::viewer::set_texture_options(texture, options);
            }
        }
        for shown in self.slots.iter_mut().filter_map(|s| s.shown_mut()) {
            crate::viewer::set_texture_options(&mut shown.thumb, options);
            if let Some(full) = &mut shown.full {
                crate::viewer::set_texture_options(&mut full.texture, options);
            }
        }
        if let Some(pb) = &mut self.playback {
            for frame in pb.cache.values_mut() {
                crate::viewer::set_texture_options(&mut frame.texture, options);
            }
        }
    }

    /// 設定ファイルが外部のエディタで書き換えられていたら読み直して反映する。
    pub(super) fn reload_settings(&mut self, ctx: &egui::Context) {
        match self.settings.reload_if_changed() {
            None => {}
            Some(Ok(problems)) => {
                self.log.info("Settings reloaded from file");
                for p in problems {
                    self.log.error(format!("Settings: {p}"));
                }
                theme::apply(ctx, &self.settings().appearance);
                self.tree.set_order(self.settings().tree.order());
                self.apply_probe_extensions();
                self.apply_fits_options();
                self.apply_netcdf_options();
                self.apply_color_rules();
                self.apply_texture_options();
                self.apply_archive_dir();
                self.apply_image_size_limit();
                self.apply_repair();
            }
            Some(Err(e)) => self.log.error(format!("Settings not reloaded: {e}")),
        }
    }

    /// 前回のウインドウの位置と大きさに戻す。
    ///
    /// 位置を送った後、移動先のモニタの拡大率がウインドウに反映されるのを待ってから大きさを送る。
    /// 同じフレームで送ると、移動前のモニタの拡大率で大きさが換算され、拡大率の違うモニタに
    /// 戻すときに大きさがずれるため。Windows で通常の矩形を覚えていれば、矩形ごと OS に渡す
    /// （最初は隠したまま動かし、拡大率が移ってからもう一度渡して大きさを合わせる）。
    pub(super) fn restore_window(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(r) = &mut self.restoring else {
            return;
        };
        let native = ctx.input(|i| i.viewport().native_pixels_per_point);
        let zoom = ctx.zoom_factor();
        let normal = r.saved.normal;
        if r.frames == 0 {
            match (normal, r.saved.position) {
                (Some(rect), _) => {
                    window_placement::set(frame, rect, false);
                }
                (None, Some([x, y])) => {
                    let ppp = zoom * native.unwrap_or(1.0);
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(
                        x as f32 / ppp,
                        y as f32 / ppp,
                    )));
                }
                (None, None) => {}
            }
        }
        r.frames += 1;
        let arrived = native.is_some_and(|n| (n - r.saved.scale).abs() < 0.01);
        if !arrived && r.frames < RESTORE_WAIT_FRAMES {
            ctx.request_repaint();
            return;
        }
        let placed = normal.is_some_and(|rect| window_placement::set(frame, rect, true));
        if !placed {
            let ppp = zoom * native.unwrap_or(r.saved.scale);
            let [w, h] = r.saved.size;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(
                w as f32 / ppp,
                h as f32 / ppp,
            )));
        }
        if r.saved.maximized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        self.log.info(format!(
            "Window: restored {}{}",
            match (normal, r.saved.position) {
                (Some([l, t, r, b]), _) => format!("{}x{} at {l}, {t}", r - l, b - t),
                (None, Some([x, y])) =>
                    format!("{}x{} at {x}, {y}", r.saved.size[0], r.saved.size[1]),
                (None, None) => format!("{}x{}", r.saved.size[0], r.saved.size[1]),
            },
            if r.saved.maximized { ", maximized" } else { "" }
        ));
        self.window = Some(r.saved.clone());
        self.restoring = None;
    }

    /// 最小化・最大化していないときのウインドウの位置と大きさを覚えておく。
    /// 最大化中は最大化していることだけを覚え、戻したときの大きさは最大化前のものを残す。
    /// Windows では OS の通常の矩形も覚える（最大化・最小化中でも取れる）。
    pub(super) fn track_window(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        // 全画面の大きさを覚えると、次の起動でウインドウが画面いっぱいの大きさで開いてしまう。
        if self.restoring.is_some() || self.fullscreen.is_some() {
            return;
        }
        if let Some(n) = window_placement::get(frame) {
            let scale = ctx.input(|i| i.viewport().native_pixels_per_point);
            let [l, t, r, b] = n.rect;
            let previous = self.window.take();
            self.window = Some(WindowState {
                position: Some([l, t]),
                size: [(r - l).max(0) as u32, (b - t).max(0) as u32],
                // 最小化中は拡大率が取れないことがあるので、前の値を残す。
                scale: scale.or(previous.as_ref().map(|w| w.scale)).unwrap_or(1.0),
                maximized: n.maximized,
                normal: Some(n.rect),
                monitors: Vec::new(),
            });
            return;
        }
        let (info, content) = ctx.input(|i| (i.viewport().clone(), i.content_rect()));
        if info.minimized == Some(true) {
            return;
        }
        let Some(native) = info.native_pixels_per_point else {
            return;
        };
        if info.maximized == Some(true) {
            if let Some(w) = &mut self.window {
                w.maximized = true;
            }
            return;
        }
        let ppp = native * ctx.zoom_factor();
        let px = |v: f32| (v * ppp).round() as i32;
        // Wayland では位置が取れず内側の範囲も届かないので、描画領域の大きさで代える。
        let inner = info.inner_rect.unwrap_or(content);
        self.window = Some(WindowState {
            position: info.outer_rect.map(|r| [px(r.min.x), px(r.min.y)]),
            size: [px(inner.width()) as u32, px(inner.height()) as u32],
            scale: native,
            maximized: false,
            normal: None,
            monitors: Vec::new(),
        });
    }

    /// 終了時の状態を保存する。モニタ構成は起動中に変わることがあるので、ここで取り直す。
    pub(super) fn save_state(&self) {
        let Some(path) = &self.state_path else {
            return;
        };
        let monitors: Vec<Monitor> = state::current_monitors();
        let window = self.window.clone().map(|w| WindowState { monitors, ..w });
        let state = AppState {
            window,
            open_folders: self.tree.open_paths(),
            // 展開した ZIP の中は終了時に消えるので、次回そこへは戻れない。
            current: self
                .current
                .clone()
                .filter(|c| self.archives.mount_of(c).is_none()),
            extract_dir: self.last_extract_dir.clone(),
            fullscreen_root: self.last_fullscreen_root.clone(),
            pipeline_nodes: self.pipeline.node_positions(),
            metadata_closed: if self.settings().metadata.remember_open {
                let mut closed: Vec<String> = self.meta_closed.iter().cloned().collect();
                closed.sort();
                closed
            } else {
                Vec::new()
            },
            dock: self.dock.as_ref().map(vw_dock::Dock::layout),
            tool: saved_tool(self.base_tool),
            // 道具の位置は欄の並び（`dock`）が持つ。前の版の値は最初の起動で読んだら要らない。
            sub_windows: Default::default(),
        };
        // 終了処理中でログ欄はもう見えないので、標準エラーに出す。
        if let Err(e) = state.save(path) {
            eprintln!("vw: cannot save {}: {e}", path.display());
        }
    }
}
