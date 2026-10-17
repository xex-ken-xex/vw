//! AI タグの整理：フォルダの画像に付いたタグを枚数つきで並べ、選んだタグをまとめて消す・
//! 今後付けないようにする。メタデータ欄のタグの右クリックもここを通す。
//!
//! 画像を複数選んで消す形（Ctrl+クリック）にしないのは、スロットに複数選択の仕組みがなく、
//! 足すとドラッグでの移動やラベル付けなど選択を使う処理を全部見直すことになるため。
//! 消したいのはたいてい「フォルダ中のこのタグ」なので、フォルダ単位で選べれば足りる。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use eframe::egui;

use super::VwApp;
use crate::i18n::{tr, tr1};

/// メタデータ欄の AI タグの行の右クリックで選ばれた操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TagAction {
    RemoveHere(String),
    RemoveInFolder(String),
    Exclude(String),
}

pub(super) struct TidyWindow {
    dir: PathBuf,
    /// 記録のある画像。
    images: Vec<PathBuf>,
    /// タグと、付いている画像の枚数。多い順。
    counts: Vec<(String, usize)>,
    checked: BTreeSet<String>,
    query: String,
    /// 除外リストに入れた直後に出す一言。
    note: Option<String>,
}

/// タグごとの、付いている画像の枚数。多い順、同じなら名前順。
pub fn tag_counts<'a>(tag_lists: impl IntoIterator<Item = Vec<&'a str>>) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for tags in tag_lists {
        for t in tags {
            *counts.entry(t).or_insert(0) += 1;
        }
    }
    let mut out: Vec<(String, usize)> =
        counts.into_iter().map(|(t, n)| (t.to_owned(), n)).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

impl VwApp {
    /// フォルダの「AI タグの整理」を開く。`check` のタグは最初から選んでおく。
    pub(super) fn open_ai_tidy(&mut self, dir: &Path, check: Option<String>, note: Option<String>) {
        let images: Vec<PathBuf> = self
            .folder_images(dir)
            .into_iter()
            .filter(|p| self.ai_tags.get(p).is_some())
            .collect();
        let entries: Vec<_> = images.iter().filter_map(|p| self.ai_tags.get(p)).collect();
        let counts = tag_counts(entries.iter().map(|e| e.tags.names().collect()));
        if counts.is_empty() && note.is_none() {
            self.log.warn(format!("No AI tags in {}", dir.display()));
            return;
        }
        self.ai_tidy = Some(TidyWindow {
            dir: dir.to_path_buf(),
            images,
            checked: check
                .filter(|t| counts.iter().any(|(c, _)| c == t))
                .into_iter()
                .collect(),
            counts,
            query: String::new(),
            note,
        });
    }

    pub(super) fn tidy_current_folder(&mut self) {
        match self.current.clone() {
            Some(dir) => self.open_ai_tidy(&dir, None, None),
            None => self.log.warn("No folder is open"),
        }
    }

    /// 画像からタグを消す。
    fn remove_tags(&mut self, images: &[PathBuf], names: &[String]) {
        match self.ai_tags.edit_all(images, |e| e.tags.remove(names)) {
            Ok(n) => self
                .log
                .info(format!("Removed {} from {n} image(s)", names.join(", "))),
            Err(e) => self.log.error(format!("Cannot save AI tags: {e}")),
        }
    }

    /// メタデータ欄のタグの右クリックの操作。`image` は表示中の画像。
    pub(super) fn tag_action(&mut self, ctx: &egui::Context, image: &Path, action: TagAction) {
        let image = crate::frames::file_of(image).to_path_buf();
        let Some(dir) = image.parent().map(Path::to_path_buf) else {
            return;
        };
        match action {
            TagAction::RemoveHere(tag) => self.remove_tags(&[image], &[tag]),
            TagAction::RemoveInFolder(tag) => {
                let images = self.folder_images(&dir);
                self.remove_tags(&images, &[tag]);
            }
            TagAction::Exclude(tag) => {
                self.exclude_tags(ctx, std::slice::from_ref(&tag));
                // 付け直し（モデルにかけ直す）は要らない。記録から消せば同じ結果になるので、
                // フォルダの画像からも消すかをここで選んでもらう。
                let note = tr1(
                    self.lang(),
                    "\"{}\" will not be added from now on. Also remove it from the images in this folder?",
                    &tag,
                );
                self.open_ai_tidy(&dir, Some(tag), Some(note));
            }
        }
    }

    fn exclude_tags(&mut self, ctx: &egui::Context, tags: &[String]) {
        let list = &mut self.settings.settings.ai.excluded_tags;
        let mut added = false;
        for t in tags {
            if !list.contains(t) {
                list.push(t.clone());
                added = true;
            }
        }
        if added {
            self.settings_changed(ctx);
            self.log
                .info(format!("Excluded AI tags: {}", tags.join(", ")));
        }
    }

    pub(super) fn ai_tidy_ui(&mut self, ctx: &egui::Context) {
        let Some(w) = &mut self.ai_tidy else {
            return;
        };
        let lang = self.settings.settings.appearance.language;
        let mut open = true;
        let mut remove = false;
        let mut exclude = false;
        egui::Window::new(tr(lang, "Tidy AI tags"))
            .open(&mut open)
            .collapsible(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.weak(w.dir.display().to_string());
                if let Some(note) = &w.note {
                    ui.label(note);
                }
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut w.query)
                            .hint_text(tr(lang, "Search tags"))
                            .desired_width(160.0),
                    );
                    if ui.small_button(tr(lang, "Clear selection")).clicked() {
                        w.checked.clear();
                    }
                });
                let query = w.query.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for (tag, n) in w
                            .counts
                            .iter()
                            .filter(|(t, _)| t.to_lowercase().contains(&query))
                        {
                            let mut on = w.checked.contains(tag);
                            if ui
                                .checkbox(
                                    &mut on,
                                    format!(
                                        "{tag}  ({})",
                                        tr1(lang, "{} image(s)", &n.to_string())
                                    ),
                                )
                                .changed()
                            {
                                if on {
                                    w.checked.insert(tag.clone());
                                } else {
                                    w.checked.remove(tag);
                                }
                            }
                        }
                    });
                ui.separator();
                ui.weak(tr1(
                    lang,
                    "{} tag(s) selected",
                    &w.checked.len().to_string(),
                ));
                ui.horizontal(|ui| {
                    let any = !w.checked.is_empty();
                    remove = ui
                        .add_enabled(any, egui::Button::new(tr(lang, "Remove from this folder")))
                        .clicked();
                    exclude = ui
                        .add_enabled(
                            any,
                            egui::Button::new(tr(lang, "Remove and never add again")),
                        )
                        .on_hover_text(tr(
                            lang,
                            "Also adds them to Settings > AI tags > Excluded tags.",
                        ))
                        .clicked();
                });
            });
        if !open {
            self.ai_tidy = None;
            return;
        }
        if remove || exclude {
            let w = self.ai_tidy.take().expect("shown above");
            let names: Vec<String> = w.checked.into_iter().collect();
            if exclude {
                self.exclude_tags(ctx, &names);
            }
            self.remove_tags(&w.images, &names);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_each_tag_once_per_image_most_first() {
        let counts = tag_counts([
            vec!["cat", "sky"],
            vec!["cat"],
            vec!["sky", "cat"],
            vec!["dog"],
        ]);
        assert_eq!(
            counts,
            [
                ("cat".to_owned(), 3),
                ("sky".to_owned(), 2),
                ("dog".to_owned(), 1)
            ]
        );
    }
}
