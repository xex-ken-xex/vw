//! スライドショーの再生と再生バー。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_phosphor::regular as icon;

use crate::i18n::{tr, tr1};
use crate::keymap::Action;
use crate::loader::Job;
use crate::settings_ui::fps_input;
use crate::slideshow::{self, Clock, EndMode, Frame, Playback};
use crate::tree::{self};
use crate::viewer::{SequenceKind, Shown};

use super::*;

impl VwApp {
    /// アクティブなスロットを最大化して、その並び（フォルダスロットならその中身、
    /// 普通のスロットなら同じフォルダの画像）をスライドショーで再生する。
    pub(super) fn start_playback(&mut self) {
        let slot = &self.slots[self.active];
        let file_delays = slot.sequence.as_ref().and_then(|q| q.delays.clone());
        let (frames, start) = match &slot.sequence {
            Some(seq) => (seq.paths.clone(), seq.index),
            None => {
                let Some(path) = slot.path.clone() else {
                    return;
                };
                let order = self.settings().tree.order();
                let files = tree::sibling_images(&path, self.loader.registry(), order);
                let start = files.iter().position(|p| *p == path).unwrap_or(0);
                (files, start)
            }
        };
        let current = frames.get(start).cloned();
        let matcher = self.matcher();
        let frames: Vec<PathBuf> = frames
            .into_iter()
            .filter(|p| Some(p) == current.as_ref() || matcher.matches(p))
            .collect();
        let start = frames
            .iter()
            .position(|p| Some(p) == current.as_ref())
            .unwrap_or(0);
        if frames.len() < 2 {
            self.log.warn("Slideshow: there is only one image to show");
            return;
        }
        if self.maximized != Some(slot.id) {
            self.toggle_maximize(self.active);
        }
        let v = &self.settings().viewer;
        let fps = self.playback_fps();
        let delays = file_delays.and_then(|d| delays_of(&frames, &d));
        let clock = Clock::new(start, frames.len(), v.slideshow_end, fps, Instant::now())
            .paced_by(delays.clone().filter(|_| self.use_file_speed));
        let loader_slot = self.next_id;
        self.next_id += 1;
        self.stopped_clock = None;
        let mut playback =
            Playback::new(self.slots[self.active].id, loader_slot, frames, clock, fps);
        playback.delays = delays;
        self.playback = Some(playback);
    }

    /// スロットを 1 つだけ出しているとき、そこに開いた GIF・WebP のアニメーションを再生する。
    /// 並べて見ているときに回さないのは、比べている最中に他のスロットが動くと気が散り、
    /// 再生の仕組みも 1 つのスロットを最大化して回す作りのため。
    pub(super) fn autoplay_animation(&mut self) {
        if !self.shows_one_slot() {
            self.autoplayed = None;
            return;
        }
        if self.playback.is_some() {
            return;
        }
        let key = self.slots.get(self.active).and_then(|slot| {
            slot.sequence
                .as_ref()
                .filter(|q| q.kind == SequenceKind::Frames && q.delays.is_some())
                .map(|q| (slot.id, q.folder.clone()))
        });
        if key == self.autoplayed {
            return;
        }
        // 別の画像に移ったら忘れる。戻ってきたときにまた回すため。
        self.autoplayed = key;
        if self.autoplayed.is_some() {
            self.start_playback();
        }
    }

    pub(super) fn stop_playback(&mut self) {
        let Some(pb) = self.playback.take() else {
            return;
        };
        for request in pb.pending.keys() {
            self.loader.cancel(*request);
        }
        if pb.clock.dropped > 0 {
            self.log.warn(format!(
                "Slideshow: {} frames were skipped because images could not be decoded fast enough",
                pb.clock.dropped
            ));
        }
        self.stopped_clock = Some(pb.clock);
    }

    /// スライドショーを 1 フレーム分進める。今のコマの画像が届いていれば表示し、
    /// この先のコマを先読みする。
    pub(super) fn drive_playback(&mut self, ctx: &egui::Context) {
        let Some(pb) = &self.playback else {
            return;
        };
        let index = self.slots.iter().position(|s| s.id == pb.slot);
        let Some(index) = index.filter(|_| self.maximized == Some(pb.slot)) else {
            // 最大化を解いた、別のスロットに移った、スロットを閉じた。
            self.stop_playback();
            return;
        };
        let now = Instant::now();
        let full = self.cell_needs_full();
        // 先読みには、表示中の画像のフル解像度と同じ上限の半分まで使う。
        let budget = self.settings().viewer.full_image_limit_bytes() / 2;
        let look = look_of(&self.color_rules, &self.slots[index]);
        let pb = self.playback.as_mut().expect("checked above");
        let step = pb.clock.advance(now);
        let mut show = None;
        if !pb.clock.shown() {
            if let Some(frame) = pb.cache.get(&step.index).cloned() {
                pb.clock.mark_shown();
                show = Some(frame);
            } else if pb.failed.contains(&step.index) {
                pb.clock.mark_shown();
            }
        }
        let (start, n, mode) = (pb.clock.start, pb.clock.n, pb.clock.mode);
        let mut wanted = slideshow::upcoming(start, pb.clock.tick(), n, mode, pb.ahead(budget));
        if !wanted.contains(&step.index) {
            wanted.insert(0, step.index);
        }
        pb.cache.retain(|i, _| wanted.contains(i));
        let loader = &self.loader;
        pb.pending.retain(|request, i| {
            let keep = wanted.contains(i);
            if !keep {
                loader.cancel(*request);
            }
            keep
        });
        let mut requests = Vec::new();
        for &i in &wanted {
            let busy = pb.cache.contains_key(&i) || pb.pending.values().any(|p| *p == i);
            if !busy && !pb.failed.contains(&i) {
                requests.push(i);
            }
        }
        for i in requests {
            let request = self.next_id;
            self.next_id += 1;
            pb.pending.insert(request, i);
            self.loader.request(Job {
                slot: pb.loader_slot,
                request,
                path: pb.frames[i].clone(),
                thumb: !full,
                full,
                develop: false,
                fit_side: None,
                look: Some(look.clone()),
            });
        }
        let finished = step.finished && pb.clock.shown();
        let wait = pb.clock.until_next(now);
        if let Some(frame) = show {
            self.show_frame(index, step.index, frame);
        }
        if finished {
            self.stop_playback();
        } else {
            ctx.request_repaint_after(wait);
        }
    }

    /// 先読みした画像を、読み込みを通さずにスロットへ出す。表示倍率と位置はそのまま保つ
    /// （拡大した部分を見ながら再生できるように）。
    pub(super) fn show_frame(&mut self, index: usize, position: usize, frame: Frame) {
        let request = self.next_id;
        self.next_id += 1;
        self.cancel_requests(&self.slots[index]);
        let s = &mut self.slots[index];
        s.path = Some(frame.path.clone());
        s.request = request;
        s.loading = false;
        s.error = None;
        s.full_request = None;
        s.copy_when_full = false;
        if let Some(seq) = &mut s.sequence {
            seq.index = position;
        }
        s.shown = Some(Shown {
            path: frame.path,
            thumb: frame.texture,
            info: frame.info,
            full: None,
            developed: false,
        });
    }

    /// 選んでいるスロットのファイルに書かれた速さ（シネやアニメーション）。
    pub(super) fn file_fps(&self) -> Option<f32> {
        self.slots
            .get(self.active)
            .and_then(|s| s.sequence.as_ref())
            .and_then(|q| q.fps)
    }

    /// 再生に使う速さ。「ファイルの速さ」が入っていて、ファイルに速さが書かれていればその速さ。
    /// シネを既定でファイルの速さにするのは、撮影時の速さで見ないと動きの評価を誤るため。
    pub(super) fn playback_fps(&self) -> f32 {
        self.file_fps()
            .filter(|_| self.use_file_speed)
            .unwrap_or(self.settings().viewer.slideshow_fps)
    }

    /// 最大化したスロットの下の再生バー。
    pub(super) fn play_bar(&self, ui: &mut egui::Ui) -> Option<BarAction> {
        let lang = self.lang();
        let viewer = &self.settings().viewer;
        let mut action = None;
        ui.horizontal_centered(|ui| {
            ui.add_space(6.0);
            let playing = self.playback.is_some();
            let (glyph, tip) = if playing {
                (icon::PAUSE, "Pause")
            } else {
                (icon::PLAY, "Play")
            };
            let tip = self.with_key(tr(lang, tip), Action::TogglePlay);
            if ui.button(glyph).on_hover_text(tip).clicked() {
                action = Some(BarAction::TogglePlay);
            }
            let file_fps = self.file_fps();
            let mut use_file = self.use_file_speed;
            let r = ui
                .add_enabled(
                    file_fps.is_some(),
                    egui::Checkbox::new(&mut use_file, tr(lang, "File speed")),
                )
                .on_hover_text(tr(
                    lang,
                    "Play at the speed written in the file. Changing the FPS turns this off.",
                ))
                .on_disabled_hover_text(tr(lang, "This file has no speed information."));
            if r.changed() {
                action = Some(BarAction::FileSpeed(use_file));
            }
            // 止まっているときも、再生したら使う速さを出す。
            let current_fps = self.playback.as_ref().map_or_else(|| self.playback_fps(), |pb| pb.fps);
            let following_file = file_fps.is_some() && self.use_file_speed;
            let mut fps = current_fps;
            // ファイルの速さで再生するときは、手で変えられることは残しつつ、数字を薄く出す。
            // 入力を無効にしないのは、そこで速さを変えればチェックが外れて、その速さになるようにするため。
            let r = ui
                .scope(|ui| {
                    if following_file {
                        let weak = ui.visuals().weak_text_color();
                        ui.visuals_mut().override_text_color = Some(weak);
                    }
                    ui.add(fps_input(&mut fps))
                })
                .inner;
            if r.changed() || r.drag_stopped() {
                // ドラッグ中は再生にだけ反映し、ファイルには離したときに書く。
                let save = !r.dragged();
                action = Some(BarAction::Fps { fps, save });
            }
            for preset in slideshow::FPS_PRESETS {
                let selected = (current_fps - preset).abs() < 0.05;
                if ui.selectable_label(selected, format!("{preset}")).clicked() {
                    action = Some(BarAction::Fps {
                        fps: preset,
                        save: true,
                    });
                }
            }
            ui.separator();
            let mut end = viewer.slideshow_end;
            vw_ui::menu::combo("slideshow_end")
                .selected_text(tr(lang, end.title()))
                .show_ui(ui, |ui| {
                    for mode in EndMode::ALL {
                        ui.selectable_value(&mut end, mode, tr(lang, mode.title()));
                    }
                });
            if end != viewer.slideshow_end {
                action = Some(BarAction::End(end));
            }
            ui.separator();
            if let Some(seq) = self
                .slots
                .get(self.active)
                .and_then(|s| s.sequence.as_ref())
            {
                ui.label(format!("{} / {}", seq.index + 1, seq.paths.len()));
            }
            let window = Duration::from_secs(u64::from(viewer.slideshow_drop_window_secs));
            let dropped = self
                .playback
                .as_ref()
                .map(|pb| &pb.clock)
                .or(self.stopped_clock.as_ref())
                .map_or(0, |clock| clock.recent_dropped(Instant::now(), window));
            if dropped > 0 {
                // 止めている間は描き直しが起きないので、期間を過ぎた数が消えるよう描き直しを頼む。
                ui.ctx().request_repaint_after(Duration::from_millis(500));
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    tr1(lang, "dropped {}", &dropped.to_string()),
                )
                .on_hover_text(tr1(
                    lang,
                    "Frames skipped in the last {} seconds because images could not be decoded fast enough.",
                    &viewer.slideshow_drop_window_secs.to_string(),
                ));
            }
        });
        action
    }

    pub(super) fn apply_bar_action(&mut self, ctx: &egui::Context, action: BarAction) {
        let viewer = &mut self.settings.settings.viewer;
        match action {
            BarAction::TogglePlay => return self.run(ctx, Action::TogglePlay),
            BarAction::Fps { fps, save } => {
                viewer.slideshow_fps = fps;
                self.use_file_speed = false;
                if let Some(pb) = &mut self.playback {
                    pb.fps = fps;
                }
                if !save {
                    self.restart_clock();
                    return;
                }
            }
            BarAction::FileSpeed(on) => {
                self.use_file_speed = on;
                let fps = self.playback_fps();
                if let Some(pb) = &mut self.playback {
                    pb.fps = fps;
                }
                self.restart_clock();
                return;
            }
            BarAction::End(mode) => viewer.slideshow_end = mode,
        }
        self.restart_clock();
        self.settings_changed(ctx);
    }

    /// 再生中なら、変えた FPS と終端の扱いで数え直す。
    pub(super) fn restart_clock(&mut self) {
        let viewer = &self.settings.settings.viewer;
        let file_speed = self.use_file_speed;
        if let Some(pb) = &mut self.playback {
            let delays = pb.delays.clone().filter(|_| file_speed);
            pb.clock
                .restart(pb.fps, delays, viewer.slideshow_end, Instant::now());
        }
    }
}

/// フレームの表示時間 `delays`（フレーム番号ごと）を、再生する並び `frames` の位置ごとに並べ直す。
/// 絞り込みで並びからフレームが抜けることがあるため。フレームでないパスが混じれば `None`。
fn delays_of(frames: &[PathBuf], delays: &[u32]) -> Option<Arc<[u32]>> {
    frames
        .iter()
        .map(|p| crate::frames::split(p).and_then(|(_, i)| delays.get(i).copied()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_delays_follow_the_frames_left_after_filtering() {
        let all = crate::frames::frame_paths(Path::new("/a.gif"), 4);
        let shown = vec![all[0].clone(), all[2].clone(), all[3].clone()];
        assert_eq!(
            delays_of(&shown, &[10, 20, 30, 40]).as_deref(),
            Some(&[10, 30, 40][..])
        );
    }

    #[test]
    fn a_sequence_of_files_has_no_frame_delays() {
        let files = vec![PathBuf::from("/a.gif"), PathBuf::from("/b.gif")];
        assert_eq!(delays_of(&files, &[10, 20]), None);
    }
}
