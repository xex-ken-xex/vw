//! スロットの管理。追加と閉じる、移動、読み込み結果の受け取り、フル解像度の出し入れ、画像欄の描画。

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, pos2, vec2, Rect};

use crate::copy::{self, Focus};
use crate::frames;
use crate::i18n::{tr, tr1};
use crate::layout::{self, SlotLayout};
use crate::loader::{FullImage, Loaded};
use crate::memory;
use crate::slideshow::Frame;
use crate::tree::{self};
use crate::viewer::{Full, Sequence, Shown, Slot, SlotLook};

use super::*;

impl VwApp {
    /// 空のスロットを末尾に足し、アクティブにする。
    pub(super) fn add_slot(&mut self) {
        self.slots.push(Slot::new(self.next_id));
        self.next_id += 1;
        self.active = self.slots.len() - 1;
    }

    /// すべてのスロットを閉じ、空のスロットを 1 つ残す。
    pub(super) fn close_all_slots(&mut self) {
        self.maximized = None;
        self.loaded_folders.clear();
        self.explores.clear();
        for s in std::mem::take(&mut self.slots) {
            self.cancel_requests(&s);
        }
        self.add_slot();
    }

    /// スロットの読み込み中の要求を取り消す。待ち行列に残ったまま、閉じたスロットのために
    /// デコードするのを避けるため。
    pub(super) fn cancel_requests(&self, slot: &Slot) {
        if slot.loading {
            self.loader.cancel(slot.request);
        }
        if let Some((request, _)) = slot.full_request {
            self.loader.cancel(request);
        }
    }

    /// スロットを閉じる。最後の 1 つを閉じたときは空のスロットを残す。
    pub(super) fn close_slot(&mut self, index: usize) {
        let slot = self.slots.remove(index);
        self.cancel_requests(&slot);
        self.explores.retain(|(id, _)| *id != slot.id);
        if self.maximized == Some(slot.id) {
            self.maximized = None;
        }
        if self.slots.is_empty() {
            self.add_slot();
            return;
        }
        self.active = active_after_close(self.active, index, self.slots.len());
    }

    /// アクティブなスロットで `path` を読み込み始める。結果は [`Self::receive`] で受け取る。
    /// 前の画像は新しい画像が届くまで表示したままにして、切り替え時のちらつきを避ける。
    pub(super) fn open_in_active(&mut self, path: PathBuf) {
        // フォルダスロットに並びの外の画像を開いたら、普通のスロットに戻す。
        let slot = &mut self.slots[self.active];
        // 読み込みで並べたスロットも、別の画像を開いたらそのフォルダの 1 枚ではなくなる。
        slot.from_folder = false;
        if let Some(seq) = &mut slot.sequence {
            match seq.paths.iter().position(|p| *p == path) {
                Some(i) => seq.index = i,
                None => slot.sequence = None,
            }
        }
        let full = self.cell_needs_full();
        self.start_load(self.active, path, full);
    }

    /// 今のスロット数で並べたとき、1 枠が縮小画像より大きいか。
    ///
    /// 大きければ、最初の読み込みでフル解像度も一緒に作る。縮小画像だけ先に読んで
    /// 後からフル解像度を読み直すと、1 枚を大きく見る普段の使い方でデコードが 2 回になるため。
    /// 画像の大きさは読むまで分からないので、枠の大きさだけで決める。
    pub(super) fn cell_needs_full(&self) -> bool {
        let Some(area) = self.images_area else {
            return true;
        };
        let view = self.view();
        let cell = (area.x / view.cols as f32).max(area.y / view.rows as f32);
        cell > memory::THUMB_SIDE as f32
    }

    /// 画面に出すスロットとマス目。
    /// 画像フィールドにスロットを 1 つだけ出している（最大化か 1x1）。
    pub(super) fn shows_one_slot(&self) -> bool {
        let view = self.view();
        view.cols * view.rows == 1
    }

    pub(super) fn view(&self) -> layout::View {
        let maximized = self
            .maximized
            .and_then(|id| self.slots.iter().position(|s| s.id == id));
        layout::view(self.layout(), self.slots.len(), self.active, maximized)
    }

    /// 今のスロットの数で使うマス目。自動レイアウトは上限の設定の中で決まる。
    pub(super) fn layout(&self) -> SlotLayout {
        let viewer = &self.settings().viewer;
        viewer
            .layout
            .resolve(self.slots.len(), viewer.auto_layout_max)
    }

    /// ページを `delta` だけ送り、そのページの先頭のスロットをアクティブにする。
    pub(super) fn step_page(&mut self, delta: isize) {
        self.active = layout::page_step(self.layout(), self.slots.len(), self.active, delta);
    }

    /// アクティブなスロットを `delta` だけ前後に移す。最大化中は最大化するスロットも移す。
    pub(super) fn step_slot(&mut self, delta: isize) {
        self.active = wrap_step(self.active, self.slots.len(), delta);
        if self.maximized.is_some() {
            self.maximized = Some(self.slots[self.active].id);
        }
    }

    /// 画像のスロット（フォルダや空のスロットでないもの）だけを辿って、アクティブなスロットを移す。
    /// 進めたら `true`。← → はスロットの中身を差し替えずにこれで移る。差し替えると、並んだ別の
    /// スロットと同じ画像になったり、どのスロットに何があったか分からなくなったりするため。
    /// スロットの中の画像はホイールで送る。
    pub(super) fn step_image_slot(&mut self, delta: isize, wrap: bool) -> bool {
        let images: Vec<bool> = self
            .slots
            .iter()
            .map(|s| s.folder.is_none() && s.path.is_some())
            .collect();
        let Some(next) = image_slot_step(&images, self.active, delta, wrap) else {
            return false;
        };
        self.active = next;
        if self.maximized.is_some() {
            self.maximized = Some(self.slots[next].id);
        }
        true
    }

    /// `index` 番目のスロットの最大化を切り替える。
    pub(super) fn toggle_maximize(&mut self, index: usize) {
        let id = self.slots[index].id;
        self.active = index;
        self.maximized = if self.maximized == Some(id) {
            None
        } else {
            Some(id)
        };
    }

    /// レイアウトを変えて設定に保存する。最大化は解く（変えた結果が見えるように）。
    pub(super) fn set_layout(&mut self, ctx: &egui::Context, layout: SlotLayout) {
        self.maximized = None;
        if self.settings().viewer.layout != layout {
            self.settings.settings.viewer.layout = layout;
            self.settings_changed(ctx);
        }
    }

    /// アクティブなスロットの画像と同じフォルダにある、前後の対応画像に切り替える。
    ///
    /// 毎回フォルダを読み直す。一覧をキャッシュしないのは、表示中にファイルが
    /// 追加・削除されることがあり、数千件程度なら読み直しても体感できる遅れにならないため。
    ///
    /// 絞り込み中は条件に合う画像だけを辿る。今の画像が条件に合わなくても、そこから前後に送れる。
    pub(super) fn step_active(&mut self, delta: isize) {
        let slot = &self.slots[self.active];
        let Some(path) = slot.path.clone() else {
            return;
        };
        let files = match &slot.sequence {
            Some(seq) => seq.paths.clone(),
            None => {
                let order = self.settings().tree.order();
                tree::sibling_images(&path, self.loader.registry(), order)
            }
        };
        let matcher = self.matcher();
        let shown: Vec<&PathBuf> = files
            .iter()
            .filter(|p| **p == path || matcher.matches(p))
            .collect();
        let Some(pos) = shown.iter().position(|p| **p == path) else {
            return;
        };
        let next = wrap_step(pos, shown.len(), delta);
        if next != pos {
            let target = shown[next].clone();
            self.open_in_active(target);
        }
    }

    /// アクティブなスロットの画像。
    pub(super) fn active_image(&self) -> Option<PathBuf> {
        self.slots[self.active].path.clone()
    }

    /// 読み込みスレッドからの結果をすべて取り出し、テクスチャにしてスロットへ渡す。
    pub(super) fn receive(&mut self, ctx: &egui::Context) {
        let options = self.texture_options;
        while let Ok(r) = self.loader.rx.try_recv() {
            self.throughput.record(Instant::now());
            if let Some(show) = self
                .fullscreen
                .as_mut()
                .filter(|f| f.loader_slot() == r.slot)
            {
                if let Some(problem) = show.receive(ctx, r.request, r.path, r.result) {
                    self.log.error(problem);
                }
                continue;
            }
            if let Some(pb) = self.playback.as_mut().filter(|pb| pb.loader_slot == r.slot) {
                let Some(index) = pb.pending.remove(&r.request) else {
                    continue;
                };
                match r.result {
                    Ok(loaded) => {
                        let image = loaded.full.map(|f| f.display).or(loaded.thumb);
                        if let Some(image) = image {
                            pb.frame_bytes = image.pixels.len() * 4;
                            let texture = ctx.load_texture(
                                format!("{}#play", r.path.display()),
                                image,
                                options,
                            );
                            pb.cache.insert(
                                index,
                                Frame {
                                    path: r.path,
                                    texture,
                                    info: loaded.info,
                                },
                            );
                        }
                    }
                    Err(e) => {
                        pb.failed.insert(index);
                        self.log.error(format!("{}: {e}", r.path.display()));
                    }
                }
                continue;
            }
            // 読み込み中にスロットが閉じられた、または別の画像に切り替えられた場合は捨てる。
            let Some(index) = self.slots.iter().position(|s| {
                s.id == r.slot
                    && (s.request == r.request
                        || s.full_request.is_some_and(|(q, _)| q == r.request))
            }) else {
                continue;
            };
            if self.slots[index].request == r.request {
                self.receive_image(ctx, index, r.path, r.result);
            } else {
                self.slots[index].full_request = None;
                match r.result {
                    Ok(mut loaded) => {
                        self.receive_developed(ctx, index, &r.path, &mut loaded);
                        if let Some(full) = loaded.full {
                            self.attach_full(ctx, index, full);
                        }
                    }
                    Err(e) => self.log.error(format!("{}: {e}", r.path.display())),
                }
            }
        }
    }

    /// 現像した画像が届いたら、表示中の画像の情報（大きさ・メタデータ）と縮小画像を差し替える。
    /// 縮小画像も差し替えるのは、並べて見たときにプレビューと現像の色が混ざらないようにするため。
    /// 現像し直した（捨てたあと読み直した）ときは同じなので、何もしない。
    pub(super) fn receive_developed(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        path: &Path,
        loaded: &mut Loaded,
    ) {
        let options = self.texture_options;
        let Some(shown) = &mut self.slots[index].shown else {
            return;
        };
        if !shown.developing() {
            return;
        }
        let info = &loaded.info;
        shown.info = info.clone();
        if let Some(thumb) = loaded.thumb.take() {
            shown.thumb = ctx.load_texture(format!("{}#thumb", path.display()), thumb, options);
        }
        self.log.info(format!(
            "{}: developed {}x{} | read {} / develop {} / convert {}",
            path.display(),
            info.width,
            info.height,
            ms(info.read),
            ms(info.decode),
            ms(info.convert),
        ));
    }

    /// スロットで開いた画像の読み込み結果を受け取る。
    pub(super) fn receive_image(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        path: PathBuf,
        result: Result<Loaded, String>,
    ) {
        let keep_view_setting = self.settings().viewer.keep_view;
        let options = self.texture_options;
        let s = &mut self.slots[index];
        s.loading = false;
        let reloaded = s.shown.as_ref().is_some_and(|shown| shown.path == path);
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(e) => {
                self.log.error(format!("{}: {e}", path.display()));
                s.shown = None;
                s.error = Some(e);
                return;
            }
        };
        let t = Instant::now();
        let thumb = loaded
            .thumb
            .expect("the first load always asks for a thumbnail");
        let thumb = ctx.load_texture(format!("{}#thumb", path.display()), thumb, options);
        let i = &loaded.info;
        self.log.info(format!(
            "{}: {}x{} {} | read {} / decode {} / convert {} / texture {}",
            path.display(),
            i.width,
            i.height,
            i.plugin,
            ms(i.read),
            ms(i.decode),
            ms(i.convert),
            ms(t.elapsed()),
        ));
        // 1 枚のスロットで開いたマルチフレームのファイルは、フレームの並びのスロットにする。
        // フォルダスロットの中では並べ替えない。フォルダの中を送れなくなるため。
        let mut path = path;
        let multiframe = loaded.info.frames > 1 && frames::split(&path).is_none();
        if multiframe && s.sequence.is_none() {
            let fps = frame_rate(&loaded.info.metadata);
            let seq = Sequence::frames(path.clone(), loaded.info.frames, fps);
            path = seq.paths[0].clone();
            s.path = Some(path.clone());
            s.sequence = Some(seq);
            s.from_folder = false;
        }
        let stepped = keeps_view(
            s.shown.as_ref().map(|shown| shown.path.as_path()),
            &path,
            s.sequence.as_ref(),
            keep_view_setting || loaded.info.plugin == "DICOM",
        );
        s.shown = Some(Shown {
            path,
            thumb,
            info: loaded.info,
            full: None,
            developed: false,
        });
        s.error = None;
        // 同じ画像の読み直し（上書きされたとき）は、拡大している場所を見続けられるよう倍率と位置を保つ。
        if !reloaded && !stepped {
            s.reset_view();
        }
        if let Some(full) = loaded.full {
            self.attach_full(ctx, index, full);
        }
        if self.settings().viewer.develop_raw {
            self.develop_slot(index);
        }
    }

    /// フル解像度の画像をスロットに持たせる。上限を超えるときは持たずに縮小画像のままにする
    /// （コピー待ちならコピーだけ済ませる）。
    pub(super) fn attach_full(&mut self, ctx: &egui::Context, index: usize, full: FullImage) {
        let s = &mut self.slots[index];
        let Some(shown) = &mut s.shown else {
            return;
        };
        if std::mem::take(&mut s.copy_when_full) {
            let image = copy::full_resolution(&full.image);
            let what = format!(
                "image {} ({}x{})",
                shown.path.display(),
                image.size[0],
                image.size[1]
            );
            ctx.copy_image(image);
            self.copied(ctx, &what);
        }
        let bytes = full.bytes();
        let limit = self.settings().viewer.full_image_limit_bytes();
        if !memory::fits(&self.full_states(), bytes, limit) {
            return;
        }
        let s = &mut self.slots[index];
        let Some(shown) = &mut s.shown else {
            return;
        };
        let path = &shown.path;
        let texture = ctx.load_texture(path.to_string_lossy(), full.display, self.texture_options);
        if texture.size()[0] < shown.info.width as usize {
            self.log.warn(format!(
                "{}: shown downsampled to {}x{} (texture size limit)",
                path.display(),
                texture.size()[0],
                texture.size()[1]
            ));
        }
        shown.full = Some(Full {
            image: full.image,
            texture,
            bytes,
            last_used: self.frame,
        });
    }

    /// 各スロットのフル解像度の画像の状態（[`memory::plan`] の入力）。
    pub(super) fn full_states(&self) -> Vec<memory::FullState> {
        self.slots
            .iter()
            .filter_map(|s| {
                let shown = s.shown.as_ref()?;
                Some(memory::FullState {
                    id: s.id,
                    wanted: s.wants_full_now,
                    resident: shown.full.as_ref().map(|f| f.bytes),
                    pending: s.full_request.map(|(_, bytes)| bytes),
                    last_used: shown.full.as_ref().map_or(0, |f| f.last_used),
                    estimate: shown.info.full_bytes(),
                })
            })
            .collect()
    }

    /// このフレームで必要になったフル解像度の画像を読み込み、上限を超える分を捨てる。
    pub(super) fn manage_full_images(&mut self) {
        let limit = self.settings().viewer.full_image_limit_bytes();
        let plan = memory::plan(&self.full_states(), limit);
        for s in &mut self.slots {
            let Some(shown) = &mut s.shown else {
                continue;
            };
            if plan.evict.contains(&s.id) {
                shown.full = None;
            }
            if plan.request.contains(&s.id) {
                request_full(&self.loader, &mut self.next_id, s);
            }
        }
    }

    /// スロットのカメラ RAW を現像し、届いたら埋め込みプレビューと差し替える。
    /// 現像できない画像と、現像を頼んだあとの画像では何もしない。
    pub(super) fn develop_slot(&mut self, index: usize) {
        let s = &mut self.slots[index];
        let Some(shown) = &mut s.shown else {
            return;
        };
        if shown.info.develop_size.is_none() || shown.developed {
            return;
        }
        shown.developed = true;
        // 読み込み中のプレビューは要らなくなる。届くと現像の結果を待たずにそれを出してしまう。
        if let Some((request, _)) = s.full_request.take() {
            self.loader.cancel(request);
        }
        request_full(&self.loader, &mut self.next_id, s);
    }

    /// スロットをグリッドに並べて描く。
    ///
    /// egui の Grid ウィジェットを使わず矩形を自分で割り当てるのは、Grid が中身の大きさに
    /// 合わせて伸び縮みするため。画像の大きさに関係なく枠を等分したい。
    pub(super) fn images_ui(&mut self, ui: &mut egui::Ui) {
        let whole = ui.available_rect_before_wrap();
        self.frame += 1;
        let view = self.view();
        let maximized = self.maximized.is_some() && view.range.len() == 1;
        let mut area = whole;
        if maximized {
            let (rest, bar) = area.split_top_bottom_at_y(area.max.y - PLAY_BAR);
            area = rest;
            let action = ui
                .scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| self.play_bar(ui))
                .inner;
            if let Some(action) = action {
                self.apply_bar_action(ui.ctx(), action);
            }
        }
        if self.tagging {
            let (rest, bar) = area.split_top_bottom_at_y(area.max.y - PLAY_BAR);
            area = rest;
            let click = ui
                .scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| self.tag_bar(ui))
                .inner;
            match click {
                Some(TagClick::Flag(flag)) => self.flag(flag),
                Some(TagClick::Tag(i)) => self.tag(i),
                None => {}
            }
        }
        self.images_area = Some(area.size() * ui.ctx().pixels_per_point());
        let (cols, rows) = (view.cols, view.rows);
        let pixel = 1.0 / ui.ctx().pixels_per_point();
        // スロットの間の 1px は区切り線の色で見せる。
        ui.painter().rect_filled(
            area,
            0.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        );
        let mut close = None;
        let mut maximize = None;
        let mut explore = None;
        let mut open_folder = None;
        let mut dropped = None;
        let mut step = None;
        let background = self.canvas_color(ui.ctx());
        let lang = self.lang();
        let hint = empty_slot_hint(self.current.is_some());
        let destinations = self.destination_names();
        let mut send = None;
        // 隠れたスロットのフル解像度の画像は、メモリが足りなくなったら先に捨ててよい。
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if !view.range.contains(&i) {
                slot.wants_full_now = false;
            }
        }
        for (cell_index, i) in view.range.clone().enumerate() {
            let (c, r) = (cell_index % cols, cell_index / cols);
            let (x0, x1) = layout::cell_span(area.min.x, area.width(), cols, c, pixel);
            let (y0, y1) = layout::cell_span(area.min.y, area.height(), rows, r, pixel);
            let rect = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
            let empty_hint = match &self.slots[i].sequence {
                Some(seq) if seq.paths.is_empty() => "No image",
                Some(_) => "No images match the filter",
                None => hint,
            };
            let label = self.slots[i]
                .path
                .as_deref()
                .map(|p| self.labels.get(p))
                .unwrap_or_default();
            let resp = self.slots[i].ui(
                ui,
                rect,
                i == self.active,
                SlotLook {
                    background,
                    lang,
                    empty_hint,
                    label: &label,
                    probe: self.tool == Tool::PixelProbe,
                    roi: self.tool == Tool::Roi,
                    destinations: &destinations,
                },
            );
            if resp.pipeline {
                self.active = i;
                if !self.pipeline.open {
                    self.toggle_pipeline();
                }
            }
            if resp.reference {
                self.set_quality_reference(i);
            }
            // 変換の図と解析は画素を読むので、開いている間はアクティブなスロットと
            // 評価の基準のスロットにフル解像度の画像を持たせる。
            let analysis_open = self.pipeline.open || self.histogram.open;
            let is_reference = self.quality_reference.is_some()
                && self.slots[i].shown.as_ref().map(|s| &s.path) == self.quality_reference.as_ref();
            let pipeline_wants = analysis_open && (i == self.active || is_reference);
            let slot = &mut self.slots[i];
            slot.wants_full_now = resp.wants_full || pipeline_wants;
            if let Some(full) = slot.shown.as_mut().and_then(|s| s.full.as_mut()) {
                if resp.wants_full || pipeline_wants {
                    full.last_used = self.frame;
                }
            }
            if resp.clicked {
                self.active = i;
                self.set_focus(ui.ctx(), Focus::Images);
            }
            if resp.copy {
                self.copy_image(ui.ctx(), i);
            }
            if let Some(text) = resp.copy_text {
                self.copy_text(ui.ctx(), text, "measurement");
            }
            if let Some(choice) = resp.send {
                send = Some((choice, self.slot_send_targets(i)));
            }
            if resp.develop {
                self.develop_slot(i);
            }
            if resp.close {
                close = Some(i);
            }
            if resp.toggle_maximize {
                if self.slots[i].is_closed_folder() {
                    explore = Some(i);
                } else {
                    maximize = Some(i);
                }
            }
            if resp.open_folder {
                open_folder = self.slots[i].folder.clone();
            }
            if let (Some(file), Some(dir)) = (resp.dropped, self.slots[i].folder.clone()) {
                dropped = Some((file, dir));
            }
            if resp.step != 0 {
                step = Some((i, resp.step));
            }
        }
        let mut filler_clicked = None;
        if !maximized && self.settings().viewer.layout != SlotLayout::Auto {
            for cell_index in view.range.len()..cols * rows {
                let (c, r) = (cell_index % cols, cell_index / cols);
                let (x0, x1) = layout::cell_span(area.min.x, area.width(), cols, c, pixel);
                let (y0, y1) = layout::cell_span(area.min.y, area.height(), rows, r, pixel);
                let rect = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
                // 空いたマスにも本物のスロットを置かず、描くときだけの空のスロットにする。
                // 本物にすると、スロットの数が増えてページ数や自動レイアウトのマス目が変わり、
                // ← → で辿る先にも混ざるため。クリックされたマスだけ本物にする。
                let mut filler = Slot::new(u64::MAX - cell_index as u64);
                let resp = filler.ui(
                    ui,
                    rect,
                    false,
                    SlotLook {
                        background,
                        lang,
                        empty_hint: hint,
                        label: &Default::default(),
                        probe: false,
                        roi: false,
                        destinations: &[],
                    },
                );
                if resp.clicked {
                    filler_clicked = Some(view.range.start + cell_index);
                }
            }
        }
        if let Some(index) = filler_clicked {
            while self.slots.len() <= index {
                self.add_slot();
            }
            self.active = index;
            self.set_focus(ui.ctx(), Focus::Images);
        }
        if view.pages > 1 {
            if let Some(delta) = self.page_bar(ui.ctx(), area, &view) {
                self.step_page(delta);
            }
        }
        if let Some((choice, targets)) = send {
            self.send_to(choice, targets);
        }
        // 描画ループの途中で削除すると添字がずれるので、ループの後で閉じる。
        if let Some((file, dir)) = dropped {
            let ctx = ui.ctx().clone();
            self.drop_file(&ctx, file, dir);
        } else if let Some(folder) = open_folder {
            let ctx = ui.ctx().clone();
            self.open_folder(&ctx, folder);
        } else if let Some(i) = close {
            self.close_slot(i);
        } else if let Some(i) = explore {
            let ctx = ui.ctx().clone();
            self.active = i;
            self.explore(&ctx, i);
        } else if let Some(i) = maximize {
            self.toggle_maximize(i);
        } else if let Some((i, delta)) = step {
            let chunk = self.slots[i].sequence.is_some();
            let one_up = self.maximized.is_some() || self.shows_one_slot();
            match wheel_step(chunk, one_up, self.slots.len()) {
                WheelStep::Image => {
                    self.active = i;
                    self.step_active(delta);
                }
                WheelStep::Slot => {
                    self.step_image_slot(delta, true);
                }
                WheelStep::Nothing => {}
            }
        }
        self.manage_full_images();
        ui.allocate_rect(whole, egui::Sense::hover());
    }

    /// スロットがページに分かれているときに、画像フィールドの右下に出すページの帯。
    /// スロットの総数、今のページとページ数、前後のページへのボタンを並べる。
    /// 押されたボタンの向き（-1 か 1）を返す。
    pub(super) fn page_bar(
        &self,
        ctx: &egui::Context,
        area: Rect,
        view: &layout::View,
    ) -> Option<isize> {
        let lang = self.lang();
        let total = tr1(lang, "{} slots", &self.slots.len().to_string());
        egui::Area::new(egui::Id::new("page_bar"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .fixed_pos(area.right_bottom() - vec2(6.0, 6.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::symmetric(4, 1))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(egui::RichText::new(total).small().weak());
                            let previous = ui
                                .small_button(egui_phosphor::regular::CARET_LEFT)
                                .on_hover_text(tr(lang, "Previous page"));
                            ui.label(
                                egui::RichText::new(format!("{} / {}", view.page + 1, view.pages))
                                    .small(),
                            );
                            let next = ui
                                .small_button(egui_phosphor::regular::CARET_RIGHT)
                                .on_hover_text(tr(lang, "Next page"));
                            if previous.clicked() {
                                Some(-1)
                            } else if next.clicked() {
                                Some(1)
                            } else {
                                None
                            }
                        })
                        .inner
                    })
                    .inner
            })
            .inner
    }

    /// フォルダを OS のファイラで開く。展開した ZIP の中なら、ZIP のあるフォルダを開く。
    ///
    /// 展開先そのものを開かないのは、そこが vw の一時フォルダで、消したり足したりしても
    /// ZIP には戻らず、vw を閉じれば消えてしまうため。
    pub(super) fn open_in_file_manager(&mut self, dir: &Path) {
        let target = match self.archives.mount_of(dir) {
            Some(m) => m.zip.parent().unwrap_or(dir).to_path_buf(),
            None => dir.to_path_buf(),
        };
        if let Err(e) = crate::launch::open_folder(&target) {
            self.log
                .error(format!("Cannot open {}: {e}", target.display()));
        }
    }
}
