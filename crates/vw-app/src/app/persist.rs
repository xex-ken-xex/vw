//! 設定の変更の反映と、ウインドウの位置などの状態の保存・復元。

use eframe::egui::{self, pos2, vec2};

use crate::state::{self, AppState, Monitor, WindowState};
use crate::theme;

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

    /// 設定画面で値が変わったときに、反映して保存する。
    pub(super) fn settings_changed(&mut self, ctx: &egui::Context) {
        theme::apply(ctx, &self.settings().appearance);
        self.tree.set_order(self.settings().tree.order());
        self.apply_probe_extensions();
        self.apply_texture_options();
        match self.settings.save() {
            Ok(Some(backup)) => self.log.info(format!(
                "Settings: the previous file had errors and was kept as {}",
                backup.display()
            )),
            Ok(None) => {}
            Err(e) => self.log.error(format!("Settings: {e}")),
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

    /// 拡大・縮小の補間が変わったら、表示中の画像にもすぐ当てる。
    pub(super) fn apply_texture_options(&mut self) {
        let options = self.settings().viewer.texture_options();
        if options == self.texture_options {
            return;
        }
        self.texture_options = options;
        for shown in self.slots.iter_mut().filter_map(|s| s.shown.as_mut()) {
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
                self.apply_texture_options();
            }
            Some(Err(e)) => self.log.error(format!("Settings not reloaded: {e}")),
        }
    }

    /// 前回のウインドウの位置と大きさに戻す。
    ///
    /// 位置を送った後、移動先のモニタの拡大率がウインドウに反映されるのを待ってから大きさを送る。
    /// 同じフレームで送ると、移動前のモニタの拡大率で大きさが換算され、拡大率の違うモニタに
    /// 戻すときに大きさがずれるため。
    pub(super) fn restore_window(&mut self, ctx: &egui::Context) {
        let Some(r) = &mut self.restoring else {
            return;
        };
        let native = ctx.input(|i| i.viewport().native_pixels_per_point);
        let zoom = ctx.zoom_factor();
        if r.frames == 0 {
            if let Some([x, y]) = r.saved.position {
                let ppp = zoom * native.unwrap_or(1.0);
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(
                    x as f32 / ppp,
                    y as f32 / ppp,
                )));
            }
        }
        r.frames += 1;
        let arrived = native.is_some_and(|n| (n - r.saved.scale).abs() < 0.01);
        if !arrived && r.frames < RESTORE_WAIT_FRAMES {
            ctx.request_repaint();
            return;
        }
        let ppp = zoom * native.unwrap_or(r.saved.scale);
        let [w, h] = r.saved.size;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(
            w as f32 / ppp,
            h as f32 / ppp,
        )));
        if r.saved.maximized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        self.window = Some(r.saved.clone());
        self.restoring = None;
    }

    /// 最小化・最大化していないときのウインドウの位置と大きさを覚えておく。
    /// 最大化中は最大化していることだけを覚え、戻したときの大きさは最大化前のものを残す。
    pub(super) fn track_window(&mut self, ctx: &egui::Context) {
        // 全画面の大きさを覚えると、次の起動でウインドウが画面いっぱいの大きさで開いてしまう。
        if self.restoring.is_some() || self.fullscreen.is_some() {
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
            sub_windows: [
                ("histogram", self.histogram.window.saved()),
                ("zoom_map", self.zoom_map.window.saved()),
            ]
            .into_iter()
            .filter(|(_, saved)| saved.rect.is_some() || saved.on_top)
            .map(|(name, saved)| (name.to_owned(), saved))
            .collect(),
        };
        // 終了処理中でログ欄はもう見えないので、標準エラーに出す。
        if let Err(e) = state.save(path) {
            eprintln!("vw: cannot save {}: {e}", path.display());
        }
    }
}
