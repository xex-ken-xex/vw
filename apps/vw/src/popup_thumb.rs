//! 地図や星図の点をクリックしたときに出す、1 枚の縮小画像。別のスレッドで読む。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, TextureHandle};
use vw_core::Registry;

use crate::i18n::tr;
use crate::settings::Language;

enum State {
    Loading(PathBuf, Receiver<Option<egui::ColorImage>>),
    Ready(PathBuf, TextureHandle),
    Failed(PathBuf),
}

impl State {
    fn path(&self) -> &Path {
        match self {
            State::Loading(p, _) | State::Ready(p, _) | State::Failed(p) => p,
        }
    }
}

/// 最後に出した 1 枚だけを持つ。別の画像を頼まれたら読み直す。
#[derive(Default)]
pub struct PopupThumb(Option<State>);

impl PopupThumb {
    /// `path` の縮小画像を、`side` 四方に収めて出す。読めるまではくるくる回る印。
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        registry: &Registry,
        path: &Path,
        side: f32,
    ) {
        self.load(ui.ctx(), registry, path);
        match &self.0 {
            Some(State::Ready(_, texture)) => {
                let size = texture.size_vec2();
                let fit = (side / size.x).min(side / size.y).min(1.0);
                ui.image((texture.id(), size * fit));
            }
            Some(State::Failed(_)) => {
                ui.weak(tr(lang, "Failed to load"));
            }
            _ => {
                ui.add_sized([side, side * 0.66], egui::Spinner::new());
            }
        }
    }

    fn load(&mut self, ctx: &egui::Context, registry: &Registry, path: &Path) {
        if self.0.as_ref().is_none_or(|t| t.path() != path) {
            let (tx, rx) = channel();
            let (registry, file, ctx2) = (registry.clone(), path.to_path_buf(), ctx.clone());
            std::thread::spawn(move || {
                let thumb = crate::loader::load_fit(&registry, &file, true, false, None)
                    .ok()
                    .and_then(|l| l.thumb);
                let _ = tx.send(thumb);
                ctx2.request_repaint();
            });
            self.0 = Some(State::Loading(path.to_path_buf(), rx));
        }
        if let Some(State::Loading(p, rx)) = &self.0
            && let Ok(image) = rx.try_recv()
        {
            let p = p.clone();
            self.0 = Some(match image {
                Some(image) => State::Ready(
                    p,
                    ctx.load_texture("popup_thumb", image, egui::TextureOptions::LINEAR),
                ),
                None => State::Failed(p),
            });
        }
    }
}
