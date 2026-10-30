//! スロットの管理。追加と閉じる、移動、読み込み結果の受け取り、フル解像度の出し入れ、画像欄の描画。

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, pos2, vec2, Rect};

use crate::copy::{self, Focus};
use crate::frames;
use crate::i18n::{tr, tr1};
use crate::layout::{self, SlotLayout};
use crate::loader::{FullImage, Loaded};
use crate::log::Level;
use crate::memory;
use crate::slideshow::Frame;
use crate::tree::{self};
use crate::viewer::{Full, RawFace, Sequence, Shown, Slot, SlotLook};

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
    ///
    /// 最初のフレームの前（枠の大きさがまだ分からない）は縮小画像だけにする。起動時の
    /// フォルダの復元や引数で大きなフォルダに入ると、見えない枠まで全部フル解像度で頼み、
    /// 数千枚で何 GB も使っていたため。画面が出てから、要る枠だけが後から頼む。
    pub(super) fn cell_needs_full(&self) -> bool {
        let Some(area) = self.images_area else {
            return false;
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
        layout::view(
            self.layout(),
            self.slots.len(),
            self.active,
            self.slot_first,
            maximized,
        )
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
        let page = self.view().page;
        let first = layout::page_step(self.layout(), self.slots.len(), page, delta);
        self.slot_first = first;
        self.active = first;
    }

    /// `page` 番目（0 から）のページの最初のスロットをアクティブにする。
    pub(super) fn go_to_page(&mut self, page: usize) {
        let n = self.slots.len();
        let last = self.layout().page_count(n).saturating_sub(1);
        let first = self.layout().page_range(page.min(last), n).start;
        self.slot_first = first;
        self.active = first;
    }

    /// ホイールの `steps` 目盛り分、出すスロットを設定の量ずつ送る。アクティブなスロットが
    /// 画面の外に出たら、画面に残る端のスロットに移す。
    ///
    /// アクティブなスロットを画面の中に移すのは、出す範囲はアクティブなスロットに合わせ直される
    /// （[`layout::view`]）ため。外に置いたままだと、送った画面がすぐ元に戻る。
    pub(super) fn scroll_slots(&mut self, steps: isize) {
        let n = self.slots.len();
        let unit = self.settings().viewer.page_wheel;
        let first = layout::scroll(self.layout(), n, self.view().range.start, steps, unit);
        let last = (first + self.layout().per_page(n)).min(n).saturating_sub(1);
        self.slot_first = first;
        self.active = self.active.clamp(first, last.max(first));
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
                        self.load_failed(&r.path, &e);
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
                    Err(e) => {
                        self.load_failed(&r.path, &e);
                        let shown = self.slots[index].shown.as_mut();
                        if shown.is_some_and(|s| s.give_up_developing()) {
                            self.log.warn(format!(
                                "{}: could not develop the RAW; showing the embedded preview",
                                r.path.display()
                            ));
                        }
                    }
                }
            }
        }
    }

    /// 読み込めなかった 1 枚。数は状態の行に、理由はログファイルに。
    fn load_failed(&mut self, path: &Path, error: &str) {
        self.log
            .detail(Level::Error, format!("{}: {error}", path.display()));
        self.load_stats.fail(file_label(path));
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
        let keep_preview = self.settings().viewer.keep_raw_preview;
        let Some(shown) = &mut self.slots[index].shown else {
            return;
        };
        if !shown.developing() {
            return;
        }
        let info = &loaded.info;
        let fingerprint = shown.info.fingerprint;
        let preview_info = std::mem::replace(&mut shown.info, info.clone());
        shown.info.fingerprint = info.fingerprint.or(fingerprint);
        let preview_full = shown.full.take();
        if let Some(thumb) = loaded.thumb.take() {
            let developed = ctx.load_texture(format!("{}#thumb", path.display()), thumb, options);
            let preview_thumb = std::mem::replace(&mut shown.thumb, developed);
            if keep_preview {
                shown.other = Some(Box::new(RawFace {
                    thumb: preview_thumb,
                    info: preview_info,
                    full: preview_full,
                    developed: false,
                }));
            }
        }
        self.log.detail(
            Level::Info,
            format!(
                "{}: developed {}x{} | read {} / develop {} / convert {}",
                path.display(),
                info.width,
                info.height,
                ms(info.read),
                ms(info.decode),
                ms(info.convert),
            ),
        );
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
                s.shown = None;
                s.error = Some(e.clone());
                self.load_failed(&path, &e);
                return;
            }
        };
        let t = Instant::now();
        let thumb = loaded
            .thumb
            .expect("the first load always asks for a thumbnail");
        let thumb = ctx.load_texture(format!("{}#thumb", path.display()), thumb, options);
        let i = &loaded.info;
        let texture = t.elapsed();
        self.load_stats
            .record(file_label(&path), [i.read, i.decode, i.convert, texture]);
        self.log.detail(
            Level::Info,
            format!(
                "{}: {}x{} {} | read {} / decode {} / convert {} / texture {}",
                path.display(),
                i.width,
                i.height,
                i.plugin,
                ms(i.read),
                ms(i.decode),
                ms(i.convert),
                ms(texture),
            ),
        );
        let calibration_note = calibration_note(&path, &loaded.info.metadata);
        // 1 枚のスロットで開いたマルチフレームのファイルは、フレームの並びのスロットにする。
        // フォルダスロットの中では並べ替えない。フォルダの中を送れなくなるため。
        let mut path = path;
        let multiframe = loaded.info.frames > 1 && frames::split(&path).is_none();
        if multiframe && s.sequence.is_none() {
            let fps = frame_rate(&loaded.info.metadata);
            let mut seq = Sequence::frames(path.clone(), loaded.info.frames, fps);
            seq.delays = loaded.info.frame_delays.clone();
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
            other: None,
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
        if let Some((dir, note)) = calibration_note {
            if self.calibration_logged.insert(dir) {
                self.log.info(note);
            }
        }
    }

    /// フル解像度の画像をスロットに持たせる。上限を超えるときは持たずに縮小画像のままにする
    /// （コピー待ちならコピーだけ済ませる）。
    pub(super) fn attach_full(&mut self, ctx: &egui::Context, index: usize, full: FullImage) {
        if self.slots[index].shown.is_none() {
            return;
        }
        let bytes = full.bytes();
        let limit = self.settings().viewer.full_image_limit_bytes();
        if !memory::fits(&self.full_states(), bytes, limit) {
            // 持てないときは、コピー待ちなら標準の表示（と色の表）だけ写しておく。
            let s = &mut self.slots[index];
            if let (true, Some(shown)) = (std::mem::take(&mut s.copy_when_full), &s.shown) {
                let palette = shown.info.look.as_ref().and_then(|a| a.palette.as_deref());
                let image = copy::full_resolution(&full.image, palette);
                let what = format!(
                    "image {} ({}x{})",
                    shown.path.display(),
                    image.size[0],
                    image.size[1]
                );
                ctx.copy_image(image);
                self.copied(ctx, &what);
            }
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
                    resident: resident_bytes(shown),
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
                if let Some(other) = &mut shown.other {
                    other.full = None;
                }
            }
            if plan.request.contains(&s.id) {
                request_full(&self.loader, &self.color_rules, &mut self.next_id, s);
            }
        }
    }

    /// スロットのカメラ RAW を現像し、届いたら埋め込みプレビューと差し替える。
    /// 現像できない画像と、現像を頼んだあとの画像では何もしない。
    /// スロットの FITS の、ダーク・フラットを当てるかを切り替えて読み直す。
    pub(super) fn toggle_calibration(&mut self, index: usize) {
        let Some(shown) = &self.slots[index].shown else {
            return;
        };
        if shown
            .info
            .metadata
            .get(vw_fits::calibration::GROUP, vw_fits::calibration::STATE)
            .is_none_or(|s| s == vw_fits::calibration::NOT_APPLIED)
        {
            return;
        }
        let path = shown.path.clone();
        let off = self.calibration.toggle(&frames::file_of(&path));
        self.log.info(format!(
            "Calibration {} for {}",
            if off { "off" } else { "on" },
            path.display()
        ));
        let full = self.cell_needs_full();
        self.start_load(index, path, full);
    }

    /// スロット `index` の色の表を `pick` にする。
    pub(super) fn set_color_pick(&mut self, index: usize, pick: crate::colormap::Pick) {
        let slot = &mut self.slots[index];
        if slot.color.pick == pick {
            return;
        }
        slot.color.pick = pick;
        self.reload_slot(index);
    }

    /// スロット `index` の色の表を、メニューの並びで次のものにする。
    pub(super) fn next_color_map(&mut self, index: usize) {
        let Some(applied) = self.slots[index]
            .shown
            .as_ref()
            .and_then(|s| s.info.look.as_ref())
        else {
            return;
        };
        let next = self.color_rules.next_after(applied.palette_name());
        if let Some(name) = next.map(str::to_owned) {
            self.log
                .info(format!("Color map: {}", vw_core::palette::title(&name)));
            self.set_color_pick(index, crate::colormap::Pick::Palette(name));
        }
    }

    /// スロット `index` の黒と白の値をゲージで変えた値にする。`None` はファイルの値に戻す。
    /// 合成のスロット `index` の、`channel` 色目の黒と白を変える。
    pub(super) fn set_channel_window(
        &mut self,
        index: usize,
        channel: usize,
        window: Option<(f32, f32)>,
    ) {
        let slot = &mut self.slots[index];
        if slot.color.channels[channel] == window {
            return;
        }
        slot.color.channels[channel] = window;
        self.reload_slot(index);
    }

    /// スロット `index` の今の見た目を、パイプラインのファイルに入れる形にする。灰色の画像でなければ `None`。
    /// 設定で決まっている値（FITS の黒と白の決め方、既定の色の表）も、そのときの値として書き込む
    /// （ほかの PC やまとめてかけるツールでも、同じ見た目になるように）。
    pub(super) fn slot_look(&self, index: usize) -> Option<crate::pipelines::Look> {
        use vw_pipeline::look::{BlackWhite, CurveName, COLOR_MAP_OFF};
        let slot = self.slots.get(index)?;
        let shown = slot.shown.as_ref()?;
        let applied = shown.info.look.as_ref()?;
        let choice = &slot.color;
        let fits = &self.settings().fits;
        let black_white = match (choice.window, choice.black_white) {
            (Some((black, white)), _) => BlackWhite::Values { black, white },
            (None, BlackWhite::File) if applied.kind == crate::colormap::Kind::Fits => {
                match fits.stretch {
                    crate::settings::FitsStretch::Percentile => BlackWhite::Percentile {
                        black: fits.low_percent,
                        white: fits.high_percent,
                    },
                    crate::settings::FitsStretch::Zscale => BlackWhite::Zscale,
                    crate::settings::FitsStretch::MinMax => BlackWhite::MinMax,
                    crate::settings::FitsStretch::Symmetric => BlackWhite::Symmetric {
                        white: fits.high_percent,
                    },
                }
            }
            (None, method) => method,
        };
        let color_map = applied.palette_name().unwrap_or(COLOR_MAP_OFF).to_owned();
        let color_map_stops = self
            .settings()
            .pipeline
            .palettes
            .iter()
            .find(|p| p.name == color_map)
            .map(|p| p.colors.clone())
            .unwrap_or_default();
        Some(crate::pipelines::Look {
            black_white,
            curve: Some(CurveName::from(applied.curve)),
            color_map: Some(color_map),
            color_map_stops,
        })
    }

    /// 保存したパイプラインの見た目を、スロット `index` に当てて読み直す。
    pub(super) fn apply_look(&mut self, index: usize, look: &crate::pipelines::Look) {
        use crate::colormap::Pick;
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };
        let mut choice = slot.color.clone();
        choice.window = None;
        choice.black_white = look.black_white;
        if let Some(curve) = look.curve {
            choice.curve = Some(curve.into());
        }
        match look.color_map.as_deref() {
            None => {}
            Some(vw_pipeline::look::COLOR_MAP_OFF) => choice.pick = Pick::Off,
            Some(name) => choice.pick = Pick::Palette(name.to_owned()),
        }
        if choice != slot.color {
            slot.color = choice;
            self.reload_slot(index);
        }
    }

    pub(super) fn set_slot_window(&mut self, index: usize, window: Option<(f32, f32)>) {
        let curve = self.slots[index].color.curve;
        self.set_slot_levels(index, window, curve);
    }

    pub(super) fn set_slot_curve(&mut self, index: usize, curve: Option<vw_core::Curve>) {
        let window = self.slots[index].color.window;
        self.set_slot_levels(index, window, curve);
    }

    /// スロットの黒と白と曲線を変えて読み直す。`None` はファイル（と設定）のまま。
    pub(super) fn set_slot_levels(
        &mut self,
        index: usize,
        window: Option<(f32, f32)>,
        curve: Option<vw_core::Curve>,
    ) {
        let slot = &mut self.slots[index];
        if (slot.color.window, slot.color.curve) == (window, curve) {
            return;
        }
        slot.color.window = window;
        slot.color.curve = curve;
        self.reload_slot(index);
    }

    /// 黒と白と曲線を、保存したパイプラインで決めた決め方も含めて、読み込んだときのものに戻す。
    pub(super) fn reset_slot_levels(&mut self, index: usize) {
        let slot = &mut self.slots[index];
        if slot.color.black_white.is_file() {
            self.set_slot_levels(index, None, None);
            return;
        }
        slot.color.window = None;
        slot.color.curve = None;
        slot.color.black_white = vw_pipeline::look::BlackWhite::File;
        self.reload_slot(index);
    }

    /// スロット `index` のライトのフォルダで、`role` のマスターを `file` に替える（`None` は自動）。
    /// 同じフォルダのライトを全部これで見るので、表示中の FITS を読み直す。
    pub(super) fn choose_calibration(
        &mut self,
        index: usize,
        role: vw_fits::calibration::Role,
        file: Option<PathBuf>,
    ) {
        let Some(shown) = &self.slots[index].shown else {
            return;
        };
        let light = frames::file_of(&shown.path);
        self.log.info(format!(
            "Calibration {} for the lights in {}: {}",
            role.key(),
            light.parent().unwrap_or(&light).display(),
            file.as_ref()
                .map_or("auto".to_owned(), |f| f.display().to_string())
        ));
        self.calibration.choose(&light, role, file);
        self.reload_fits_slots();
    }

    pub(super) fn develop_slot(&mut self, index: usize) {
        let s = &mut self.slots[index];
        let Some(shown) = &mut s.shown else {
            return;
        };
        if shown.showing_raw_preview() {
            self.swap_raw_face(index);
            return;
        }
        if shown.info.develop_size.is_none() || shown.developed {
            return;
        }
        shown.developed = true;
        // 読み込み中のプレビューは要らなくなる。届くと現像の結果を待たずにそれを出してしまう。
        if let Some((request, _)) = s.full_request.take() {
            self.loader.cancel(request);
        }
        request_full(&self.loader, &self.color_rules, &mut self.next_id, s);
    }

    /// 現像したカメラ RAW の、埋め込みのプレビューと現像した画像を入れ替える。
    /// 読み込み中のフル解像度の画像は、入れ替える前の方のものなので取り消す。
    pub(super) fn swap_raw_face(&mut self, index: usize) {
        let s = &mut self.slots[index];
        let Some(shown) = &mut s.shown else {
            return;
        };
        if !shown.swap_raw_face() {
            return;
        }
        if let Some((request, _)) = s.full_request.take() {
            self.loader.cancel(request);
        }
        let what = if shown.showing_raw_preview() {
            "the camera's embedded preview"
        } else {
            "the image developed from the sensor data"
        };
        self.log
            .info(format!("{}: showing {what}", shown.path.display()));
    }

    /// アクティブなスロットの画素の変換に、確かめるためのノード（白飛びの印など）を足す。
    /// もう入っていれば外す。値は設定の `[check]` のもの。
    pub(super) fn toggle_check_node(&mut self, kind: &str) {
        let Some(node) = self.settings().check.node(kind) else {
            return;
        };
        let presets = self.presets.presets().to_vec();
        let Some(slot) = self.slots.get_mut(self.active) else {
            return;
        };
        if slot.shown.is_none() {
            return;
        }
        let mut values = slot
            .derive
            .as_ref()
            .map(|d| d.values.clone())
            .unwrap_or_default();
        let title = vw_pipeline::nodes::Node::title(kind);
        match values.chain.iter().position(|n| n.kind() == kind) {
            Some(i) => {
                values.chain.remove(i);
                self.log.info(format!("{title}: off"));
            }
            None => {
                values.chain.push(node);
                self.log.info(format!("{title}: on"));
            }
        }
        crate::derive::edit(&mut slot.derive, values, &presets);
        // 比較や差の表示にしていると、足したノードの効き目が半分しか見えないため。
        if let Some(d) = &mut slot.derive {
            d.view = crate::derive::View::Derived;
        }
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
        if !maximized {
            self.slot_first = view.range.start;
        }
        let mut area = whole;
        // 画像の欄は枠なしのパネルなので、下の帯は自分で塗らないとウインドウの地（黒）が透けて、
        // ライトのテーマでは灰色の字が黒の上に乗って読めなくなる。
        let bar_fill = ui.visuals().panel_fill;
        if maximized {
            let (rest, bar) = area.split_top_bottom_at_y(area.max.y - PLAY_BAR);
            area = rest;
            ui.painter().rect_filled(bar, 0.0, bar_fill);
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
            ui.painter().rect_filled(bar, 0.0, bar_fill);
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
        let mut page_step = 0;
        let colors = crate::viewer::Colors {
            probe: self.settings().metadata.color_format,
            roi: self.settings().roi.color_format,
            face_known: self.settings().ai.face_known(),
            face_unknown: self.settings().ai.face_unknown(),
        };
        let background = self.canvas_color(ui.ctx());
        let lang = self.lang();
        let hint = empty_slot_hint(self.current.is_some());
        let destinations = self.destination_names();
        let apps = self.send_apps().to_vec();
        let ai_ready = self.ai_ready();
        let people = self.people.names();
        let mut send = None;
        self.update_similar();
        let mut cells: Vec<(Rect, usize)> = Vec::new();
        let deep_target = self.update_deep(ui.ctx(), &view);
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
            let faces = match (&self.slots[i].shown, self.show_faces) {
                (Some(s), true) => self.ai_tags.faces(&crate::frames::file_of(&s.path)),
                _ => Vec::new(),
            };
            let similar = self.similar.mark(self.slots[i].id);
            let beam = self.settings().fits.beam;
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
                    pan: matches!(self.tool, Tool::Pan | Tool::PixelProbe),
                    zoom: self.tool == Tool::Zoom,
                    destinations: &destinations,
                    apps: &apps,
                    presets: self.presets.presets(),
                    texture_options: self.texture_options,
                    faces: &faces,
                    people: &people,
                    ai_ready,
                    colors,
                    gauge: self.show_gauge.then_some(crate::viewer::Gauge {
                        side: self.color_settings.gauge_side,
                        on_gray: self.color_settings.gauge_on_gray,
                    }),
                    color_rules: &self.color_rules,
                    similar,
                    beam,
                },
            );
            cells.push((rect, i));
            if resp.dragging && self.slot_drag.is_none() {
                self.slot_drag = Some(self.slots[i].id);
            }
            if let Some(event) = resp.derive {
                self.log_derive(event);
            }
            if let Some(look) = &resp.look {
                self.apply_look(i, look);
            }
            if resp.pipeline {
                self.active = i;
                if !self.pipeline.open {
                    self.toggle_pipeline();
                }
            }
            if resp.reference {
                self.set_quality_reference(i);
            }
            if resp.compose {
                self.open_compose_dialog(i);
            }
            if resp.compare {
                self.open_compare_dialog(i);
            }
            if resp.compare_similar {
                self.compare_similar(i);
            }
            if resp.read_qr {
                if let Some(path) = self.slots[i].shown.as_ref().map(|s| s.path.clone()) {
                    self.read_qr_codes(ui.ctx(), path);
                }
            }
            if resp.ai_tag {
                if let Some(path) = self.slots[i].shown.as_ref().map(|s| s.path.clone()) {
                    self.ai_tag_paths(ui.ctx(), vec![path]);
                }
            }
            if resp.remove_ai_tags {
                if let Some(path) = self.slots[i].shown.as_ref().map(|s| s.path.clone()) {
                    self.remove_ai_tags(vec![crate::frames::file_of(&path).to_path_buf()]);
                }
            }
            if resp.find_faces {
                if let Some(path) = self.slots[i].shown.as_ref().map(|s| s.path.clone()) {
                    self.find_faces_paths(ui.ctx(), vec![path]);
                }
            }
            if let Some((path, action)) = resp.face {
                let file = crate::frames::file_of(&path);
                match action {
                    crate::viewer::FaceAction::Assign(f, name) => self.assign_face(&file, f, &name),
                    crate::viewer::FaceAction::NewPerson(f) => self.people_window.ask_name(file, f),
                    crate::viewer::FaceAction::Clear(f) => self.clear_face(&file, f),
                }
            }
            // 変換の図と解析は画素を読むので、開いている間はアクティブなスロットと
            // 評価の基準のスロットにフル解像度の画像を持たせる。
            let analysis_open = self.pipeline.open || self.histogram.open;
            let is_reference = self.quality_reference.is_some()
                && self.slots[i].shown.as_ref().map(|s| &s.path) == self.quality_reference.as_ref();
            let pipeline_wants =
                analysis_open && (i == self.active || is_reference) || deep_target == Some(i);
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
            if resp.swap_raw_face {
                self.swap_raw_face(i);
            }
            if resp.toggle_calibration {
                self.toggle_calibration(i);
            }
            if let Some(pick) = resp.color_pick {
                self.set_color_pick(i, pick);
            }
            if let Some(window) = resp.window {
                self.set_slot_window(i, window);
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
            page_step += resp.page_step;
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
                        pan: false,
                        zoom: false,
                        destinations: &[],
                        apps: &[],
                        presets: &[],
                        texture_options: self.texture_options,
                        faces: &[],
                        people: &[],
                        ai_ready: false,
                        colors,
                        gauge: None,
                        color_rules: &self.color_rules,
                        similar: None,
                        beam: self.settings().fits.beam,
                    },
                );
                cells.push((rect, view.range.start + cell_index));
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
        self.drag_to_reorder(ui, &cells);
        if view.pages > 1 {
            match self.page_bar(ui.ctx(), area, &view) {
                Some(PageMove::Step(delta)) => self.step_page(delta),
                Some(PageMove::To(page)) => self.go_to_page(page),
                Some(PageMove::Scroll(steps)) => self.scroll_slots(steps),
                None if page_step != 0 && self.maximized.is_none() => self.scroll_slots(page_step),
                None => {}
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

    /// 画像スロットのドラッグを、離した位置のマスへの配置にする。置いた先のスロットとは入れ替え、
    /// 空きマスへ置いたら元の場所は空きのまま残す。フォルダのスロットの上は画像の移動・コピーの
    /// 落とし先なので、配置の先にしない。
    ///
    /// 空きマスを、スロットごとのマスの番号ではなく空のスロットで表すのは、ページ分けや ← → の
    /// 辿り方を、スロットの並びのまま使えるようにするため（空のスロットは ← → で飛ばされる）。
    fn drag_to_reorder(&mut self, ui: &egui::Ui, cells: &[(Rect, usize)]) {
        self.reorder_hover = false;
        let Some(id) = self.slot_drag else {
            return;
        };
        // 別のウインドウで離されたドラッグは、ここでは離したことが届かない。持ち物が消えたら終える。
        let carrying =
            egui::DragAndDrop::has_payload_of_type::<crate::viewer::DraggedFile>(ui.ctx());
        let Some(from) = self
            .slots
            .iter()
            .position(|s| s.id == id)
            .filter(|_| carrying)
        else {
            self.slot_drag = None;
            return;
        };
        let (pos, released) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_released()));
        let target = pos
            .and_then(|p| cells.iter().find(|(rect, _)| rect.contains(p)))
            .filter(|(_, to)| {
                *to != from && self.slots.get(*to).is_none_or(|s| s.folder.is_none())
            });
        if let Some((rect, to)) = target {
            self.reorder_hover = true;
            let color = ui.visuals().selection.stroke.color;
            ui.ctx()
                .layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    ui.id().with("reorder"),
                ))
                .rect_stroke(
                    *rect,
                    0.0,
                    egui::Stroke::new(2.0, color),
                    egui::StrokeKind::Inside,
                );
            if released {
                self.place_slot(from, *to);
            }
        }
        if released {
            self.slot_drag = None;
        }
    }

    /// `from` 番目のスロットを `to` 番目のマスに置く。
    fn place_slot(&mut self, from: usize, to: usize) {
        let active = self.slots[self.active].id;
        while self.slots.len() <= to {
            self.slots.push(Slot::new(self.next_id));
            self.next_id += 1;
        }
        self.slots.swap(from, to);
        // 末尾の空きは並びの終わりと見分けがつかないので、ページ数を増やさないよう捨てる。
        while self.slots.len() > 1 && self.slots.last().is_some_and(is_blank) {
            self.slots.pop();
        }
        self.active = self.slots.iter().position(|s| s.id == active).unwrap_or(0);
    }

    /// スロットがページに分かれているときに、画像フィールドの右下に出すページの帯。
    /// スロットの総数、今のページとページ数、前後のページへのボタンを並べる。
    /// 帯の上でホイールを回すと前後のページへ、ページの数字を左右にドラッグすると遠くのページへ移る。
    pub(super) fn page_bar(
        &mut self,
        ctx: &egui::Context,
        area: Rect,
        view: &layout::View,
    ) -> Option<PageMove> {
        let lang = self.lang();
        let total = tr1(lang, "{} slots", &self.slots.len().to_string());
        let wheel_unit = self.settings().viewer.page_wheel;
        let shown_page = match self.page_drag {
            Some((from, dx)) => layout::page_after_drag(from, dx, view.pages),
            None => view.page,
        };
        let inner = egui::Area::new(egui::Id::new("page_bar"))
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
                            let number = ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(format!(
                                            "{} / {}",
                                            shown_page + 1,
                                            view.pages
                                        ))
                                        .small(),
                                    )
                                    .sense(egui::Sense::drag())
                                    .selectable(false),
                                )
                                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                                .on_hover_text(format!(
                                    "{}\n{}",
                                    tr(
                                        lang,
                                        "Drag left or right to jump pages. Turn the wheel here, or Shift+wheel over the slots, to scroll.",
                                    ),
                                    tr1(lang, "Wheel: {}", tr(lang, wheel_unit.title())),
                                ));
                            let next = ui
                                .small_button(egui_phosphor::regular::CARET_RIGHT)
                                .on_hover_text(tr(lang, "Next page"));
                            (previous.clicked(), next.clicked(), number)
                        })
                        .inner
                    })
            });
        let (previous, next, number) = inner.inner.inner;
        let bar = inner.response.rect;
        if number.drag_started() {
            self.page_drag = Some((view.page, 0.0));
        }
        if let Some((from, dx)) = &mut self.page_drag {
            *dx += number.drag_delta().x;
            let target = layout::page_after_drag(*from, *dx, view.pages);
            if number.drag_stopped() || !number.dragged() {
                self.page_drag = None;
            }
            return (target != view.page).then_some(PageMove::To(target));
        }
        if previous {
            return Some(PageMove::Step(-1));
        }
        if next {
            return Some(PageMove::Step(1));
        }
        if ctx.rect_contains_pointer(inner.response.layer_id, bar) {
            // 帯の上では Shift の有無を問わない。Shift を押したまま帯に来ても止まらないように。
            let wheel = ctx.input(|i| {
                let (plain, shifted) =
                    crate::viewer::split_wheel(&i.events, i.viewport_rect().height());
                plain + shifted
            });
            let steps = crate::viewer::wheel_steps(&mut self.page_wheel, wheel);
            if steps != 0 {
                return Some(PageMove::Scroll(steps));
            }
        }
        None
    }

    /// フォルダを OS のファイラで開く。展開した ZIP の中なら、ZIP のあるフォルダを開く。
    ///
    /// 展開先そのものを開かないのは、そこが vw の一時フォルダで、消したり足したりしても
    /// ZIP には戻らず、vw を閉じれば消えてしまうため。
    /// 設定フォルダやその下のフォルダを開く。まだなければ作ってから開く（初回は空のこともあるため）。
    pub(super) fn open_settings_subfolder(&mut self, dir: Option<PathBuf>) {
        let Some(dir) = dir else {
            self.log.error("The settings folder is unknown");
            return;
        };
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = vw_launch::open_folder(&dir) {
            self.log
                .error(format!("Cannot open {}: {e}", dir.display()));
        }
    }

    pub(super) fn open_in_file_manager(&mut self, dir: &Path) {
        let target = match self.archives.mount_of(dir) {
            Some(m) => m.zip.parent().unwrap_or(dir).to_path_buf(),
            None => dir.to_path_buf(),
        };
        if let Err(e) = vw_launch::open_folder(&target) {
            self.log
                .error(format!("Cannot open {}: {e}", target.display()));
        }
    }
}

/// ページの帯で選ばれた移り先。
pub(super) enum PageMove {
    /// 前後に何ページ。
    Step(isize),
    /// 何ページ目（0 から）。
    To(usize),
    /// ホイールで何目盛り（送る量は設定で決まる）。
    Scroll(isize),
}

/// 状態の行に出す名前。フォルダまで出すと長すぎて、切り替わる名前が読めないため名前だけ。
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// ダーク・フラットを当てた（当てられなかった）ことの、フォルダごとに 1 回出すログ。
/// 切ってあるときは、自分で切ったので出さない。
fn calibration_note(path: &Path, metadata: &vw_core::Metadata) -> Option<(PathBuf, String)> {
    use vw_fits::calibration::{Role, APPLIED, GROUP, NOT_APPLIED, STATE};
    let state = metadata.get(GROUP, STATE)?;
    // 候補の一覧はメタデータ欄と図で見られるので、ログを長くしないよう出さない。
    let candidate = |k: &str| Role::ALL.iter().any(|r| r.candidate_key() == k);
    if state != APPLIED && state != NOT_APPLIED {
        return None;
    }
    let dir = path.parent()?.to_path_buf();
    let details: Vec<String> = metadata
        .entries
        .iter()
        .filter(|(g, k, _)| g == GROUP && k != STATE && !candidate(k))
        .map(|(_, k, v)| format!("{k}: {v}"))
        .collect();
    let what = if state == APPLIED {
        "applied"
    } else {
        "not applied"
    };
    Some((
        dir.clone(),
        format!(
            "Calibration {what} in {} ({})",
            dir.display(),
            details.join("; ")
        ),
    ))
}

/// スロットが持っているフル解像度の画像の大きさ。現像したカメラ RAW では、出していない方の分も足す。
/// 足さないと、プレビューと現像の 2 枚を持つ RAW がメモリの上限を見かけより多く使ってしまう。
fn resident_bytes(shown: &Shown) -> Option<usize> {
    let other = shown.other.as_ref().and_then(|o| o.full.as_ref());
    match (shown.full.as_ref(), other) {
        (None, None) => None,
        (a, b) => Some(a.map_or(0, |f| f.bytes) + b.map_or(0, |f| f.bytes)),
    }
}
