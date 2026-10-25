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

    /// スロットに見えている画像（色の表、色導出の結果）を元の解像度でコピーする。
    /// フル解像度の画像を捨ててあったときは読み直し、色導出を描き直しているときはそれを待って、
    /// 揃ったときにコピーする（[`Self::copy_pending`]）。
    pub(super) fn copy_image(&mut self, ctx: &egui::Context, index: usize) {
        let s = &mut self.slots[index];
        if s.shown.is_none() {
            return;
        }
        s.copy_when_full = true;
        if s.shown.as_ref().is_some_and(|sh| sh.full.is_none()) && s.full_request.is_none() {
            request_full(&self.loader, &self.color_rules, &mut self.next_id, s);
        }
        self.copy_pending(ctx);
    }

    /// 頼まれていたコピーのうち、画像が揃ったものをコピーする。毎フレーム呼ぶ。
    pub(super) fn copy_pending(&mut self, ctx: &egui::Context) {
        for index in 0..self.slots.len() {
            if !self.slots[index].copy_when_full {
                continue;
            }
            let Some((path, image)) = self.slot_copy_image(index) else {
                continue;
            };
            self.slots[index].copy_when_full = false;
            let what = format!(
                "image {} ({}x{})",
                path.display(),
                image.size[0],
                image.size[1]
            );
            ctx.copy_image(image);
            self.copied(ctx, &what);
        }
    }

    /// スロット `index` に見えている画像の元の解像度のもの。まだ揃っていなければ `None`。
    fn slot_copy_image(&self, index: usize) -> Option<(std::path::PathBuf, egui::ColorImage)> {
        let slot = &self.slots[index];
        let shown = slot.shown.as_ref()?;
        let full = shown.full.as_ref()?;
        if let Some(d) = &slot.derive {
            if let Some(full) = d.full_for(shown) {
                return Some((shown.path.clone(), (**full).clone()));
            }
            // 描き直している間は待つ（標準の表示を写すと、見えているものと違ってしまう）。
            if d.busy() {
                return None;
            }
        }
        let palette = shown.info.look.as_ref().and_then(|a| a.palette.as_deref());
        Some((
            shown.path.clone(),
            copy::full_resolution(&full.image, palette),
        ))
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
        // 文字入力欄や道具の欄の上にいるときは、そちらのコピーに任せる。
        if !pressed || self.show_settings || ctx.egui_wants_keyboard_input() || self.tool_hovered {
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
