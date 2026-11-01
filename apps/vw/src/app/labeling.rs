//! フラグとタグの付与、絞り込み、タグバー。

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32};

use crate::discard;
use crate::export::{self, Filter, Matcher};
use crate::i18n::{tr, tr1};
use crate::keymap::Action;
use crate::labels::Flag;
use crate::settings::{Language, LoadInto};
use crate::theme;
use crate::tree::{self};
use crate::viewer::{Content, Origin, Slot};

use super::*;

/// フラグを消してよいか尋ねている画像と、フォルダ全体か（偽なら見えている画像だけ）。
pub(super) struct ClearFlags {
    pending: discard::Pending,
    whole: bool,
}

impl VwApp {
    /// 絞り込みの条件。
    pub(super) fn matcher(&self) -> Matcher<'_> {
        Matcher {
            filter: &self.filter,
            labels: &self.labels,
        }
    }

    /// 絞り込みを変え、読み込んだフォルダのスロットにもすぐ効かせる。
    pub(super) fn set_filter(&mut self, ctx: &egui::Context, filter: Filter) {
        self.log.info(format!(
            "Filter: {}",
            filter_name(&filter, Language::English)
        ));
        self.filter = filter;
        self.apply_filter_to_slots();
        ctx.request_repaint();
    }

    /// 絞り込みを変えたときに、フォルダの読み込みで並べたスロットを条件に合わせる。
    /// 自分で開いた画像のスロットはそのままにする。
    pub(super) fn apply_filter_to_slots(&mut self) {
        let registry = self.loader.registry().clone();
        let order = self.settings().tree.order();
        let roots: Vec<PathBuf> = self
            .loaded_folders
            .iter()
            .filter(|(_, into)| *into == LoadInto::Images)
            .map(|(p, _)| p.clone())
            .collect();
        for root in roots {
            let matcher = self.matcher();
            let wanted: Vec<PathBuf> = tree::list_folder(&root, &registry, order)
                .images
                .into_iter()
                .filter(|p| matcher.matches(p))
                .collect();
            self.rearrange_folder_slots(&root, wanted);
        }
        let full = self.cell_needs_full();
        for index in 0..self.slots.len() {
            let Some(seq) = &self.slots[index].sequence else {
                continue;
            };
            let matcher = self.matcher();
            let target = nearest_match(&seq.paths, seq.index, |p| matcher.matches(p));
            let Some(target) = target else {
                // 合う画像がなければ空にする。条件に合わない画像を出したままにすると、
                // 絞り込みが効いていないように見えるため。並びは残して、条件を戻したら出し直す。
                self.cancel_requests(&self.slots[index]);
                let slot = &mut self.slots[index];
                slot.path = None;
                slot.content = Content::Empty;
                slot.loading = false;
                slot.full_request = None;
                continue;
            };
            let path = seq.paths[target].clone();
            if self.slots[index].path.as_ref() == Some(&path) {
                continue;
            }
            if let Some(seq) = &mut self.slots[index].sequence {
                seq.index = target;
            }
            self.start_load(index, path, full);
        }
    }

    /// `root` の画像を 1 枚 1 スロットに並べたスロットを、`wanted` の画像に並べ直す。
    /// 残る画像のスロットは読み直さずにそのまま使う。
    pub(super) fn rearrange_folder_slots(&mut self, root: &Path, wanted: Vec<PathBuf>) {
        let in_root = |s: &Slot| {
            s.is_folder_image() && s.path.as_deref().and_then(Path::parent) == Some(root)
        };
        let active_id = self.slots[self.active].id;
        let mut at = self.slots.iter().position(in_root);
        let mut kept = HashMap::new();
        let mut rest = Vec::with_capacity(self.slots.len());
        for slot in std::mem::take(&mut self.slots) {
            if in_root(&slot) {
                kept.insert(slot.path.clone().expect("checked by in_root"), slot);
            } else {
                rest.push(slot);
            }
        }
        // 前に全部隠れていたときは、cd したときと同じくフォルダとシリーズのスロットの後ろに並べる。
        // 末尾の空のスロットがあれば、そこから並べる。
        let at = *at.get_or_insert_with(|| {
            if rest.last().is_some_and(is_blank) && !wanted.is_empty() {
                rest.pop();
            }
            rest.iter()
                .rposition(|s| s.folder().is_some() || s.sequence.is_some())
                .map_or(0, |i| i + 1)
        });
        let mut new = Vec::with_capacity(wanted.len());
        let mut loads = Vec::new();
        for path in wanted {
            match kept.remove(&path) {
                Some(slot) => new.push(slot),
                None => {
                    let mut slot = Slot::new(self.next_id);
                    self.next_id += 1;
                    slot.origin = Origin::FolderImage;
                    loads.push((slot.id, path));
                    new.push(slot);
                }
            }
        }
        let first_id = new.first().map(|s| s.id);
        rest.splice(at..at, new);
        self.slots = rest;
        for slot in kept.into_values() {
            self.cancel_requests(&slot);
        }
        if self.slots.is_empty() {
            self.slots.push(Slot::new(self.next_id));
            self.next_id += 1;
        }
        let find = |id| self.slots.iter().position(|s| s.id == id);
        self.active = find(active_id)
            .or_else(|| first_id.and_then(find))
            .unwrap_or(at.min(self.slots.len() - 1));
        if let Some(id) = self.maximized
            && find(id).is_none()
        {
            self.maximized = Some(self.slots[self.active].id);
        }
        let full = self.cell_needs_full();
        for (id, path) in loads {
            if let Some(index) = self.slots.iter().position(|s| s.id == id) {
                self.start_load(index, path, full);
            }
        }
    }

    /// サイドバーの絞り込みボタンの色。絞り込んでいなければ `None`（普段の色）。
    pub(super) fn filter_color(&self) -> Option<Color32> {
        match &self.filter {
            Filter::All => None,
            Filter::Flag(Some(flag)) => Some(theme::flag_color(*flag)),
            Filter::Flag(None) => Some(theme::UNFLAGGED),
            Filter::Tag(tag) => {
                let tags = &self.settings().labels.tags;
                tags.iter().position(|t| t == tag).map(theme::tag_color)
            }
        }
    }

    /// ラベルを付けられる画像か。ZIP の中はラベルファイルを書く場所がなく、閉じると消えるので付けない。
    pub(super) fn labelable(&mut self, path: &Path) -> bool {
        if let Some(mount) = self.archives.mount_of(path) {
            let zip = mount.zip.display().to_string();
            self.log.warn(format!(
                "Flags and tags cannot be put on images inside {zip}"
            ));
            return false;
        }
        true
    }

    /// フラグを付ける・外す。
    pub(super) fn flag(&mut self, flag: Flag) {
        let Some(path) = self.active_image().filter(|p| !p.is_dir()) else {
            return;
        };
        if !self.labelable(&path) {
            return;
        }
        match self.labels.toggle_flag(&path, flag) {
            Ok(now) => {
                let what = match now {
                    Some(Flag::Good) => "flagged good",
                    Some(Flag::Bad) => "flagged bad",
                    None => "flag removed",
                };
                self.log.info(format!("{}: {what}", path.display()));
                if now.is_some() {
                    self.advance_after_label();
                }
            }
            Err(e) => self.log.error(e),
        }
    }

    /// 今いるフォルダのフラグ付きの画像を集め、フラグを消してよいか尋ねる。`whole` が偽なら、
    /// 今の絞り込みに合う画像（見えている画像）だけにする。悪いフラグだけを見せていれば、
    /// 悪いフラグだけを消せる。サブフォルダまで辿らないのは、ごみ箱へ送るときと同じく、
    /// 開いていないフォルダの画像に触れないため。
    pub(super) fn ask_clear_flags(&mut self, whole: bool) {
        let lang = self.lang();
        let Some(dir) = self.current.clone() else {
            self.log
                .warn(tr(lang, "Select a folder in the tree first."));
            return;
        };
        let order = self.settings().tree.order();
        let all = Filter::All;
        let matcher = if whole {
            Matcher {
                filter: &all,
                labels: &self.labels,
            }
        } else {
            self.matcher()
        };
        let shown = export::collect(&dir, false, self.loader.registry(), order, &matcher).images;
        let files: Vec<PathBuf> = shown
            .into_iter()
            .filter(|f| self.labels.get(f).flag.is_some())
            .collect();
        if files.is_empty() {
            self.log
                .info(format!("No flagged images to clear in {}", dir.display()));
            return;
        }
        self.pending_clear_flags = Some(ClearFlags {
            pending: discard::Pending { dir, files },
            whole,
        });
    }

    pub(super) fn clear_flags_dialog(&mut self, ctx: &egui::Context) {
        let Some(ClearFlags { pending, whole }) = &self.pending_clear_flags else {
            return;
        };
        let lang = self.lang();
        let (names, rest) = pending.preview(10);
        let filtered = !whole && self.filter != Filter::All;
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("clear_flags_question")).show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.label(tr1(
                lang,
                "Clear the flags of {} images?",
                &pending.files.len().to_string(),
            ));
            ui.label(egui::RichText::new(pending.dir.display().to_string()).monospace());
            if filtered {
                ui.label(tr(lang, "Only the images shown by the current filter."));
            }
            ui.add_space(4.0);
            for name in names {
                ui.label(name);
            }
            if rest > 0 {
                ui.label(tr1(lang, "...and {} more", &rest.to_string()));
            }
            ui.add_space(4.0);
            ui.weak(tr(lang, "Ctrl+Z puts them back."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr(lang, "Clear flags")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr(lang, "Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        let Some(yes) = answer else {
            return;
        };
        let Some(ClearFlags { pending, .. }) = self.pending_clear_flags.take().filter(|_| yes)
        else {
            return;
        };
        let mut cleared = Vec::new();
        for file in pending.files {
            let Some(flag) = self.labels.get(&file).flag else {
                continue;
            };
            match self.labels.set_flag(&file, None) {
                Ok(()) => cleared.push((file, flag)),
                Err(e) => self.log.error(format!("{}: {e}", file.display())),
            }
        }
        let count = cleared.len();
        if !cleared.is_empty() {
            self.remember_undo(super::undo::Undo::Flags(cleared));
        }
        self.log.info(format!(
            "Cleared the flags of {count} images in {} (Ctrl+Z to undo)",
            pending.dir.display()
        ));
        // フラグで絞り込んでいれば、消した画像はもう条件に合わないので並びから外す。
        self.apply_filter_to_slots();
    }

    /// 設定の `index` 番目のタグを付ける・外す。
    pub(super) fn tag(&mut self, index: usize) {
        let Some(tag) = self.settings().labels.tags.get(index).cloned() else {
            self.log.warn(format!(
                "No tag for key {} (add tags in Settings > Flags and tags)",
                index + 1
            ));
            return;
        };
        let Some(path) = self.active_image().filter(|p| !p.is_dir()) else {
            return;
        };
        if !self.labelable(&path) {
            return;
        }
        match self.labels.toggle_tag(&path, &tag) {
            Ok(added) => {
                let what = if added { "tagged" } else { "untagged" };
                self.log.info(format!("{}: {what} {tag}", path.display()));
                if added {
                    self.advance_after_label();
                }
            }
            Err(e) => self.log.error(e),
        }
    }

    /// ラベルを付けた後に次の画像へ進む（設定で止められる）。1 枚 1 スロットに並べているときは
    /// 次のスロットへ、そうでなければスロットの中で次の画像へ。最後まで来たら先頭に戻らずに止まる。
    /// 戻ると、付け終わったことに気づかずに最初の画像のラベルを付け替えてしまうため。
    pub(super) fn advance_after_label(&mut self) {
        if !self.settings().labels.advance {
            return;
        }
        let slot = &self.slots[self.active];
        if slot.sequence.is_none() && self.slots.len() > 1 {
            self.step_image_slot(1, false);
            return;
        }
        let at_end = match &slot.sequence {
            Some(seq) => seq.index + 1 >= seq.paths.len(),
            None => slot.path.as_ref().is_none_or(|p| {
                let order = self.settings().tree.order();
                let files = tree::sibling_images(p, self.loader.registry(), order);
                files.last() == Some(p)
            }),
        };
        if !at_end {
            self.step_active(1);
        }
    }

    /// タグ付けモードの間、画像の下に出すバー。アクティブなスロットの画像のフラグとタグを示し、
    /// クリックでも付け外しできる。
    pub(super) fn tag_bar(&self, ui: &mut egui::Ui) -> Option<TagClick> {
        let lang = self.lang();
        let label = self.slots[self.active]
            .path
            .as_deref()
            .map(|p| self.labels.get(p))
            .unwrap_or_default();
        let in_zip = self.slots[self.active]
            .path
            .as_deref()
            .and_then(|p| self.archives.mount_of(p));
        let mut clicked = None;
        let frame = egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(6, 3));
        frame.show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.horizontal_centered(|ui| {
                if in_zip.is_some() {
                    ui.disable();
                }
                for (flag, action, title) in [
                    (Flag::Good, Action::FlagGood, "Good"),
                    (Flag::Bad, Action::FlagBad, "Bad"),
                ] {
                    let text = egui::RichText::new(format!(
                        "{} {}",
                        crate::labels::flag_icon(flag),
                        tr(lang, title)
                    ))
                    .color(theme::flag_color(flag));
                    let tip = self.with_key(tr(lang, action.title()), action);
                    if ui
                        .add(egui::Button::selectable(label.flag == Some(flag), text))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        clicked = Some(TagClick::Flag(flag));
                    }
                }
                ui.separator();
                let tags = &self.settings().labels.tags;
                if tags.is_empty() {
                    ui.weak(tr(lang, "Add tags in Settings > Flags and tags"));
                }
                if in_zip.is_some() {
                    ui.separator();
                    ui.weak(tr(lang, "Images inside a ZIP cannot be labeled"));
                }
                for (i, tag) in tags.iter().enumerate() {
                    let key = Action::ALL
                        .iter()
                        .copied()
                        .find(|a| a.tag_index() == Some(i));
                    let text = match key {
                        Some(_) => format!("{} {tag}", i + 1),
                        None => tag.clone(),
                    };
                    let text = egui::RichText::new(text).color(theme::tag_color(i));
                    let on = label.tags.contains(tag);
                    if ui.add(egui::Button::selectable(on, text)).clicked() {
                        clicked = Some(TagClick::Tag(i));
                    }
                }
            });
        });
        clicked
    }
}
