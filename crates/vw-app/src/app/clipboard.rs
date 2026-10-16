//! コピー（テキスト・画像・メタデータの選択範囲）とフォーカス。

use std::time::Instant;

use eframe::egui::{self, text_selection::LabelSelectionState};

use crate::copy::{self, Focus};

use super::*;

impl VwApp {
    /// Ctrl+C の対象の欄を変える。メタデータの文字列の範囲選択以外に移るときは、
    /// その範囲選択を解く。残しておくと、egui がフレームの終わりに範囲の文字列をコピーし、
    /// こちらでコピーした内容を上書きしてしまうため。
    pub(super) fn set_focus(&mut self, ctx: &egui::Context, focus: Focus) {
        self.focus = focus;
        if !matches!(focus, Focus::MetadataText | Focus::Log) {
            ctx.with_plugin(|s: &mut LabelSelectionState| s.clear_selection());
        }
    }

    pub(super) fn copy_text(&mut self, ctx: &egui::Context, text: String, what: &str) {
        ctx.copy_text(text);
        self.copied(ctx, what);
    }

    /// スロットの画像を元の解像度でコピーする。
    /// フル解像度の画像を捨ててあったときは読み直し、届いたときにコピーする。
    pub(super) fn copy_image(&mut self, ctx: &egui::Context, index: usize) {
        let s = &mut self.slots[index];
        let Some(shown) = &s.shown else {
            return;
        };
        let Some(full) = &shown.full else {
            s.copy_when_full = true;
            if s.full_request.is_none() {
                request_full(&self.loader, &mut self.next_id, s);
            }
            return;
        };
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

    /// コピーしたことをログとカーソル付近の表示で知らせる。
    pub(super) fn copied(&mut self, ctx: &egui::Context, what: &str) {
        self.log.info(format!("Copied {what}"));
        let pos = ctx
            .input(|i| i.pointer.latest_pos())
            .unwrap_or_else(|| ctx.content_rect().center());
        self.toast = Some(Toast {
            pos,
            until: Instant::now() + TOAST_DURATION,
        });
    }

    /// Ctrl+C（egui はこれを `Event::Copy` として届ける）で、対象の欄で選んでいるものをコピーする。
    pub(super) fn copy_selection(&mut self, ctx: &egui::Context) {
        let pressed = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        // 文字入力欄にいるときは、その欄のコピーに任せる。
        if !pressed || self.show_settings || ctx.egui_wants_keyboard_input() {
            return;
        }
        match self.focus {
            Focus::MetadataText => {}
            Focus::Tree => {
                if let Some(p) = self.current.clone() {
                    self.copy_text(
                        ctx,
                        p.display().to_string(),
                        &format!("path {}", p.display()),
                    );
                }
            }
            Focus::Metadata => {
                let Some(sel) = self.meta_selected.clone() else {
                    return;
                };
                let text = self
                    .metadata_groups()
                    .and_then(|g| copy::metadata_text(&g, &sel));
                if let Some(text) = text {
                    self.copy_text(ctx, text, &format!("metadata {}", selection_name(&sel)));
                }
            }
            Focus::Images => {
                let slot = &self.slots[self.active];
                if let Some(text) = slot.selected_mark_text() {
                    self.copy_text(ctx, text, "probe point");
                } else if let Some(text) = slot.selected_roi_text() {
                    self.copy_text(ctx, text, "ROI statistics");
                } else {
                    self.copy_image(ctx, self.active);
                }
            }
            // egui がラベルで選んだ範囲をコピーする。知らせだけを出す。
            Focus::Log => self.copied(ctx, "log text"),
        }
    }
}
