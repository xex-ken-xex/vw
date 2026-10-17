//! 人物：顔の枠から人物を登録する操作と、「人物」ウインドウ（一覧・名前の変更・削除）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui;

use super::VwApp;
use crate::i18n::{tr, tr1};

/// 「人物」ウインドウと、名前を聞く小さなウインドウの状態。
#[derive(Default)]
pub(super) struct PeopleWindow {
    pub open: bool,
    /// 名前を変えている人物と、入力中の名前。
    renaming: Option<(String, String)>,
    /// 削除を確かめている人物。
    deleting: Option<String>,
    /// 「新しい人物として登録」で名前を聞いている顔（画像と枠の番号）と、入力中の名前。
    naming: Option<(PathBuf, usize, String)>,
    /// 名前を聞くウインドウを開いた直後。入力欄に一度だけ入力を向けるため
    /// （毎フレーム向けるとボタンが押せなくなる）。
    focus_name: bool,
    thumbs: HashMap<(String, usize), egui::TextureHandle>,
}

impl PeopleWindow {
    pub(super) fn ask_name(&mut self, image: PathBuf, face: usize) {
        self.naming = Some((image, face, String::new()));
        self.focus_name = true;
    }
}

impl VwApp {
    /// 画像の `index` 番目の顔を人物 `name` として登録する（いなければ作る）。
    pub(super) fn assign_face(&mut self, image: &Path, index: usize, name: &str) {
        let Some(sample) = self.face_store.get(image, index) else {
            self.log.error(format!(
                "The face is no longer recorded; find faces in {} again",
                image.display()
            ));
            return;
        };
        self.people.add(name, sample);
        if let Err(e) = self.people.save() {
            self.log.error(format!("Cannot save people: {e}"));
            return;
        }
        let saved = self.ai_tags.update(image, |e| {
            if let Some(f) = e.faces.get_mut(index) {
                f.person = Some(name.to_owned());
                f.similarity = 1.0;
                f.unlinked = false;
            }
        });
        match saved {
            Err(e) => self.log.error(format!("Cannot save AI tags: {e}")),
            Ok(()) => self
                .log
                .info(format!("Registered a face as {name}: {}", image.display())),
        }
        self.people_window.thumbs.clear();
        self.rematch_faces(None);
    }

    /// 前に見つけた顔に付いた名前を、今の人物の登録に合わせ直す。
    /// 消えた人物の名前は外し、名前のない顔は保存してある特徴で照合する。
    /// `renamed` は (古い名前, 新しい名前)。
    ///
    /// 顔をもう一度探す（モデルにかける）のでなく保存した特徴を使うのは、
    /// 何千枚あっても比べるだけで済み、元の画像が読めない場所にあっても照合できるため。
    pub(super) fn rematch_faces(&mut self, renamed: Option<(&str, &str)>) {
        let threshold = self.settings().ai.face_threshold;
        let mut matched = 0;
        let mut problems = Vec::new();
        for (image, samples) in self.face_store.all() {
            let marks = self.ai_tags.faces(&image);
            if marks.is_empty() {
                continue;
            }
            let (changed, n) = self.people.rematch(&marks, &samples, threshold, renamed);
            matched += n;
            if changed != marks {
                if let Err(e) = self.ai_tags.update(&image, |e| e.faces = changed) {
                    problems.push(e);
                }
            }
        }
        if matched > 0 {
            self.log.info(format!(
                "Matched {matched} earlier face(s) to registered people"
            ));
        }
        for e in problems.into_iter().take(5) {
            self.log.error(format!("Cannot save AI tags: {e}"));
        }
    }

    /// 顔の枠から人物を外す（人物の登録はそのまま）。
    pub(super) fn clear_face(&mut self, image: &Path, index: usize) {
        let saved = self.ai_tags.update(image, |e| {
            if let Some(f) = e.faces.get_mut(index) {
                f.person = None;
                f.similarity = 0.0;
                f.unlinked = true;
            }
        });
        if let Err(e) = saved {
            self.log.error(format!("Cannot save AI tags: {e}"));
        }
    }

    pub(super) fn people_ui(&mut self, ctx: &egui::Context) {
        self.name_dialog(ctx);
        self.people_window_ui(ctx);
    }

    fn name_dialog(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let Some((image, index, mut name)) = self.people_window.naming.take() else {
            return;
        };
        let mut open = true;
        let mut done = None;
        let focus = std::mem::take(&mut self.people_window.focus_name);
        egui::Window::new(tr(lang, "New person"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(tr(lang, "Name"));
                let edit = ui.text_edit_singleline(&mut name);
                if focus {
                    edit.request_focus();
                }
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let exists = self.people.list.iter().any(|p| p.name == name.trim());
                if exists {
                    ui.weak(tr(
                        lang,
                        "This name is already registered; the face is added to that person.",
                    ));
                }
                ui.horizontal(|ui| {
                    let ok = ui.add_enabled(
                        !name.trim().is_empty(),
                        egui::Button::new(tr(lang, "Register")),
                    );
                    if ok.clicked() || (enter && !name.trim().is_empty()) {
                        done = Some(name.trim().to_owned());
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        done = Some(String::new());
                    }
                });
            });
        match done {
            Some(n) if !n.is_empty() => self.assign_face(&image, index, &n),
            Some(_) => {}
            None if open => self.people_window.naming = Some((image, index, name)),
            None => {}
        }
    }

    fn people_window_ui(&mut self, ctx: &egui::Context) {
        if !self.people_window.open {
            return;
        }
        let lang = self.lang();
        let mut open = true;
        let mut rename: Option<(String, String)> = None;
        let mut remove: Option<String> = None;
        let mut reveal: Option<PathBuf> = None;
        let people = self.people.list.clone();
        let w = &mut self.people_window;
        egui::Window::new(tr(lang, "People"))
            .open(&mut open)
            .collapsible(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                if people.is_empty() {
                    ui.weak(tr(
                        lang,
                        "No one is registered yet. Right-click an image > Find faces, then right-click a face frame.",
                    ));
                    return;
                }
                ui.weak(tr(lang, "Stored only on this PC (settings folder people.json)."));
                egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                    for person in &people {
                        ui.separator();
                        ui.horizontal(|ui| {
                            match &mut w.renaming {
                                Some((who, text)) if *who == person.name => {
                                    ui.text_edit_singleline(text);
                                    if ui.button("OK").clicked() {
                                        rename = Some((who.clone(), text.trim().to_owned()));
                                    }
                                }
                                _ => {
                                    ui.strong(&person.name);
                                    ui.weak(tr1(lang, "{} face(s)", &person.faces.len().to_string()));
                                    if ui.small_button(tr(lang, "Rename")).clicked() {
                                        w.renaming = Some((person.name.clone(), person.name.clone()));
                                    }
                                    if w.deleting.as_deref() == Some(person.name.as_str()) {
                                        if ui.small_button(tr(lang, "Really delete")).clicked() {
                                            remove = Some(person.name.clone());
                                        }
                                    } else if ui.small_button(tr(lang, "Delete")).clicked() {
                                        w.deleting = Some(person.name.clone());
                                    }
                                }
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            for (i, s) in person.faces.iter().enumerate().take(12) {
                                let key = (person.name.clone(), i);
                                let tex = w.thumbs.entry(key).or_insert_with(|| {
                                    let img = crate::people::thumb_image(&s.thumb)
                                        .unwrap_or_else(|| egui::ColorImage::filled([1, 1], egui::Color32::GRAY));
                                    ui.ctx().load_texture(format!("person-{}-{i}", person.name), img, Default::default())
                                });
                                let r = ui
                                    .add(egui::Image::new(&*tex).fit_to_exact_size(egui::vec2(48.0, 48.0)).sense(egui::Sense::click()));
                                if let Some(src) = &s.source {
                                    let r = r.on_hover_text(src.display().to_string());
                                    if r.clicked() {
                                        reveal = Some(src.clone());
                                    }
                                }
                            }
                        });
                    }
                });
            });
        if !open {
            self.people_window.open = false;
            self.people_window.renaming = None;
            self.people_window.deleting = None;
        }
        if let Some((from, to)) = rename {
            if to.is_empty() {
                self.people_window.renaming = None;
            } else {
                match self.people.rename(&from, &to) {
                    Ok(()) => {
                        self.people_window.renaming = None;
                        self.people_window.thumbs.clear();
                        self.save_people();
                        self.log.info(format!("Renamed {from} to {to}"));
                        self.rematch_faces(Some((&from, &to)));
                    }
                    Err(e) => self.log.error(e),
                }
            }
        }
        if let Some(name) = remove {
            self.people.remove(&name);
            self.people_window.deleting = None;
            self.people_window.thumbs.clear();
            self.save_people();
            self.log.info(format!("Deleted {name}"));
            self.rematch_faces(None);
        }
        if let Some(path) = reveal {
            if let Some(dir) = path.parent() {
                self.open_in_file_manager(dir);
            }
        }
    }

    fn save_people(&mut self) {
        if let Err(e) = self.people.save() {
            self.log.error(format!("Cannot save people: {e}"));
        }
    }
}
