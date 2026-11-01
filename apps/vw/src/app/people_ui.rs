//! 人物：顔の枠から人物を登録する操作と、「人物」ウインドウ（一覧・名前の変更・削除、
//! 未登録の顔のまとめ）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui;

use super::VwApp;
use crate::i18n::{tr, tr1};
use crate::people::{arrange, image_counts, Order, Person, Sample};

/// 画像と、その中の顔の番号。
type FaceRef = (PathBuf, usize);

/// 「未登録の顔」に一度に出すまとまりの数と、1 つのまとまりに出す顔の数。
/// 何千もの顔を毎フレーム描くと重くなるので、多いものから順に区切る。
const SHOWN_GROUPS: usize = 100;
const SHOWN_FACES: usize = 40;

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
    /// 顔の縮小画像のテクスチャ。キーの番号が `None` なら代表の顔。
    thumbs: HashMap<(String, Option<usize>), egui::TextureHandle>,
    query: String,
    order: Order,
    size: CardSize,
    /// 下に詳しく出している人物。
    selected: Option<String>,
    /// 人物ごとの写っている画像の枚数。顔や人物が変わったら `None` にして数え直す。
    /// 未登録の顔のまとまりも、これが `None` になったときに作り直す。
    pub counts: Option<HashMap<String, usize>>,
    /// 2 人が同じ画像に写った枚数（相関図の線の太さ）。`counts` と一緒に作り直す。
    pub together: Option<HashMap<(String, String), usize>>,
    /// 一覧を絞り込む相関図の名前。
    chart: Option<String>,
    tab: Tab,
    /// 名前のない顔の、似たものどうしのまとまり。
    unknown: Vec<Vec<(FaceRef, Sample)>>,
    /// まとまりに入力中の名前。キーはまとまりの最初の顔。
    group_names: HashMap<FaceRef, String>,
    /// 「このまとまりから外す」を選んだ顔。まとめ直しても 1 つだけで出す。
    split: HashSet<FaceRef>,
    unknown_thumbs: HashMap<FaceRef, egui::TextureHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Registered,
    Unregistered,
}

/// 人物の一覧の、代表の顔の大きさ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CardSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl CardSize {
    const ALL: [CardSize; 3] = [CardSize::Small, CardSize::Medium, CardSize::Large];

    fn title(self) -> &'static str {
        match self {
            CardSize::Small => "Small",
            CardSize::Medium => "Medium",
            CardSize::Large => "Large",
        }
    }

    fn side(self) -> f32 {
        match self {
            CardSize::Small => 48.0,
            CardSize::Medium => 80.0,
            CardSize::Large => 128.0,
        }
    }
}

enum Command {
    Rename(String, String),
    Delete(String),
    Cover(String, usize),
    Reveal(PathBuf),
    OpenSettingsFolder,
    NameFaces(Vec<FaceRef>, String),
    HideFaces(Vec<FaceRef>),
    Split(FaceRef),
    OpenCharts,
}

fn texture(ctx: &egui::Context, name: String, thumb: &[u8]) -> egui::TextureHandle {
    let img = crate::people::thumb_image(thumb)
        .unwrap_or_else(|| egui::ColorImage::filled([1, 1], egui::Color32::GRAY));
    ctx.load_texture(name, img, Default::default())
}

fn cover_texture(
    ctx: &egui::Context,
    thumbs: &mut HashMap<(String, Option<usize>), egui::TextureHandle>,
    person: &Person,
) -> egui::TextureHandle {
    thumbs
        .entry((person.name.clone(), None))
        .or_insert_with(|| {
            let thumb = person
                .cover()
                .map(|s| s.thumb.as_slice())
                .unwrap_or_default();
            texture(ctx, format!("person-{}-cover", person.name), thumb)
        })
        .clone()
}

fn face_texture(
    ctx: &egui::Context,
    thumbs: &mut HashMap<(String, Option<usize>), egui::TextureHandle>,
    name: &str,
    index: usize,
    sample: &Sample,
) -> egui::TextureHandle {
    thumbs
        .entry((name.to_owned(), Some(index)))
        .or_insert_with(|| texture(ctx, format!("person-{name}-{index}"), &sample.thumb))
        .clone()
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
        self.assign_faces(&[(image.to_path_buf(), index)], name);
    }

    /// 顔をまとめて人物 `name` として登録する。保存と照合し直しは最後に 1 回だけ行う
    /// （顔ごとにすると、照合し直すたびに記録してある全部の顔を読むため）。
    fn assign_faces(&mut self, faces: &[FaceRef], name: &str) {
        let mut problems = Vec::new();
        let mut registered = Vec::new();
        for (image, index) in faces {
            let Some(sample) = self.face_store.get(image, *index) else {
                problems.push(format!(
                    "The face is no longer recorded; find faces in {} again",
                    image.display()
                ));
                continue;
            };
            self.people.add(name, sample);
            registered.push((image, *index));
        }
        if !registered.is_empty()
            && let Err(e) = self.people.save() {
                self.log.error(format!("Cannot save people: {e}"));
                return;
            }
        for (image, index) in &registered {
            let saved = self.ai_tags.update(image, |e| {
                if let Some(f) = e.faces.get_mut(*index) {
                    f.person = Some(name.to_owned());
                    f.similarity = 1.0;
                    f.unlinked = false;
                    f.hidden = false;
                }
            });
            if let Err(e) = saved {
                problems.push(format!("Cannot save AI tags: {e}"));
            }
        }
        match registered.as_slice() {
            [] => {}
            [(image, _)] => self
                .log
                .info(format!("Registered a face as {name}: {}", image.display())),
            many => self
                .log
                .info(format!("Registered {} faces as {name}", many.len())),
        }
        for e in problems.into_iter().take(5) {
            self.log.error(e);
        }
        self.people_window.thumbs.clear();
        self.rematch_faces(None);
    }

    /// 顔を「未登録の顔」から隠す。顔の枠から登録すれば、また名前を付けられる。
    fn hide_faces(&mut self, faces: &[FaceRef]) {
        for (image, index) in faces {
            let saved = self.ai_tags.update(image, |e| {
                if let Some(f) = e.faces.get_mut(*index) {
                    f.hidden = true;
                }
            });
            if let Err(e) = saved {
                self.log.error(format!("Cannot save AI tags: {e}"));
                break;
            }
        }
        self.people_window.counts = None;
    }

    /// 名前のない顔を、似たものどうしにまとめる。
    fn unknown_groups(&self) -> Vec<Vec<(FaceRef, Sample)>> {
        let split = &self.people_window.split;
        let mut faces = Vec::new();
        let mut alone = Vec::new();
        for (image, samples) in self.face_store.all() {
            for (i, mark) in self.ai_tags.faces(&image).iter().enumerate() {
                let Some(sample) = samples.get(i) else {
                    continue;
                };
                if mark.person.is_some() || mark.hidden {
                    continue;
                }
                let face = ((image.clone(), i), sample.clone());
                if split.contains(&face.0) {
                    alone.push(vec![face]);
                } else {
                    faces.push(face);
                }
            }
        }
        let features: Vec<&[f32]> = faces.iter().map(|(_, s)| s.feature.as_slice()).collect();
        let mut groups: Vec<Vec<(FaceRef, Sample)>> =
            crate::face_groups::group(&features, self.settings().ai.face_threshold)
                .into_iter()
                .map(|g| g.into_iter().map(|i| faces[i].clone()).collect())
                .collect();
        groups.extend(alone);
        groups
    }

    /// 前に見つけた顔に付いた名前を、今の人物の登録に合わせ直す。
    /// 消えた人物の名前は外し、名前のない顔は保存してある特徴で照合する。
    /// `renamed` は (古い名前, 新しい名前)。
    ///
    /// 顔をもう一度探す（モデルにかける）のでなく保存した特徴を使うのは、
    /// 何千枚あっても比べるだけで済み、元の画像が読めない場所にあっても照合できるため。
    pub(super) fn rematch_faces(&mut self, renamed: Option<(&str, &str)>) {
        self.people_window.counts = None;
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
            if changed != marks
                && let Err(e) = self.ai_tags.update(&image, |e| e.faces = changed) {
                    problems.push(e);
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
        self.people_window.counts = None;
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
        self.refresh_face_counts();
        let mut open = true;
        let mut command = None;
        let people = self.people.list.clone();
        let graphs: Vec<(String, Vec<String>)> = self
            .people
            .graphs
            .iter()
            .map(|g| {
                (
                    g.name.clone(),
                    g.members.iter().map(|m| m.name.clone()).collect(),
                )
            })
            .collect();
        let w = &mut self.people_window;
        let counts = w.counts.clone().unwrap_or_default();
        egui::Window::new(tr(lang, "People"))
            .open(&mut open)
            .collapsible(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut w.tab, Tab::Registered, tr(lang, "Registered people"));
                    let faces: usize = w.unknown.iter().map(Vec::len).sum();
                    ui.selectable_value(
                        &mut w.tab,
                        Tab::Unregistered,
                        tr1(lang, "Unregistered faces ({})", &faces.to_string()),
                    );
                });
                ui.separator();
                if w.tab == Tab::Unregistered {
                    unregistered_tab(ui, w, &people, lang, &mut command);
                    return;
                }
                if people.is_empty() {
                    ui.weak(tr(
                        lang,
                        "No one is registered yet. Right-click an image > Find faces, then right-click a face frame.",
                    ));
                    return;
                }
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut w.query)
                            .hint_text(tr(lang, "Search by name"))
                            .desired_width(140.0),
                    );
                    vw_ui::menu::combo("people_order")
                        .selected_text(tr(lang, w.order.title()))
                        .show_ui(ui, |ui| {
                            for o in Order::ALL {
                                ui.selectable_value(&mut w.order, o, tr(lang, o.title()));
                            }
                        });
                    for size in CardSize::ALL {
                        ui.selectable_value(&mut w.size, size, tr(lang, size.title()));
                    }
                });
                ui.horizontal(|ui| {
                    if w.chart.as_ref().is_some_and(|c| !graphs.iter().any(|(n, _)| n == c)) {
                        w.chart = None;
                    }
                    let all = tr(lang, "All people");
                    vw_ui::menu::combo("people_chart")
                        .selected_text(w.chart.clone().unwrap_or_else(|| all.to_owned()))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut w.chart, None, all);
                            for (name, _) in &graphs {
                                ui.selectable_value(&mut w.chart, Some(name.clone()), name);
                            }
                        })
                        .response
                        .on_hover_text(tr(lang, "Show only the people in a relationship chart"));
                    if ui.button(tr(lang, "Relationship charts...")).clicked() {
                        command = Some(Command::OpenCharts);
                    }
                });
                ui.horizontal(|ui| {
                    ui.weak(tr(lang, "Stored only on this PC (settings folder people.json)."));
                    if ui.small_button(tr(lang, "Open folder")).clicked() {
                        command = Some(Command::OpenSettingsFolder);
                    }
                });
                let members = w
                    .chart
                    .as_ref()
                    .and_then(|c| graphs.iter().find(|(n, _)| n == c))
                    .map(|(_, m)| m);
                let in_chart: Vec<Person> = people
                    .iter()
                    .filter(|p| members.is_none_or(|m| m.contains(&p.name)))
                    .cloned()
                    .collect();
                let shown = arrange(&in_chart, &w.query, &counts, w.order);
                let side = w.size.side();
                egui::ScrollArea::vertical()
                    .id_salt("people_cards")
                    .max_height(360.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for person in &shown {
                                let selected = w.selected.as_deref() == Some(person.name.as_str());
                                let tex = cover_texture(ui.ctx(), &mut w.thumbs, person);
                                let images = counts.get(&person.name).copied().unwrap_or(0);
                                let card = ui.vertical(|ui| {
                                    ui.set_width(side);
                                    let mut image = egui::Image::new(&tex)
                                        .fit_to_exact_size(egui::vec2(side, side));
                                    if selected {
                                        image = image.bg_fill(ui.visuals().selection.bg_fill);
                                    }
                                    let r = ui.add(egui::Button::image(image).selected(selected).frame(selected));
                                    ui.add(egui::Label::new(&person.name).truncate());
                                    ui.weak(tr1(lang, "{} image(s)", &images.to_string()));
                                    r
                                });
                                if card.inner.clicked() {
                                    w.selected = Some(person.name.clone());
                                    w.renaming = None;
                                    w.deleting = None;
                                }
                            }
                        });
                    });
                let Some(person) = w
                    .selected
                    .as_ref()
                    .and_then(|n| people.iter().find(|p| &p.name == n))
                else {
                    return;
                };
                ui.separator();
                ui.horizontal(|ui| match &mut w.renaming {
                    Some((who, text)) if *who == person.name => {
                        ui.text_edit_singleline(text);
                        if ui.button("OK").clicked() {
                            command = Some(Command::Rename(who.clone(), text.trim().to_owned()));
                        }
                    }
                    _ => {
                        ui.strong(&person.name);
                        if ui.small_button(tr(lang, "Rename")).clicked() {
                            w.renaming = Some((person.name.clone(), person.name.clone()));
                        }
                        if w.deleting.as_deref() == Some(person.name.as_str()) {
                            if ui.small_button(tr(lang, "Really delete")).clicked() {
                                command = Some(Command::Delete(person.name.clone()));
                            }
                        } else if ui.small_button(tr(lang, "Delete")).clicked() {
                            w.deleting = Some(person.name.clone());
                        }
                    }
                });
                let added = (person.added > 0)
                    .then(|| chrono::DateTime::from_timestamp(person.added, 0))
                    .flatten()
                    .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "-".to_owned());
                ui.weak(format!(
                    "{}: {added}   {}   {}",
                    tr(lang, "First registered"),
                    tr1(lang, "{} image(s)", &counts.get(&person.name).copied().unwrap_or(0).to_string()),
                    tr1(lang, "{} face(s)", &person.faces.len().to_string()),
                ));
                ui.weak(tr(lang, "Click a face to open its folder. Right-click to make it the cover."));
                egui::ScrollArea::vertical()
                    .id_salt("people_faces")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (i, s) in person.faces.iter().enumerate() {
                                let tex = face_texture(ui.ctx(), &mut w.thumbs, &person.name, i, s);
                                let cover = i == person.cover.min(person.faces.len() - 1);
                                let image = egui::Image::new(&tex).fit_to_exact_size(egui::vec2(48.0, 48.0));
                                let mut r = ui.add(egui::Button::image(image).selected(cover).frame(cover));
                                if let Some(src) = &s.source {
                                    r = r.on_hover_text(src.display().to_string());
                                    if r.clicked() {
                                        command = Some(Command::Reveal(src.clone()));
                                    }
                                }
                                vw_ui::menu::context(&r, |ui| {
                                    if ui.button(tr(lang, "Use as cover")).clicked() {
                                        command = Some(Command::Cover(person.name.clone(), i));
                                        ui.close();
                                    }
                                });
                            }
                        });
                    });
            });
        if !open {
            self.people_window.open = false;
            self.people_window.renaming = None;
            self.people_window.deleting = None;
        }
        match command {
            Some(Command::Rename(from, to)) if to.is_empty() => {
                let _ = from;
                self.people_window.renaming = None;
            }
            Some(Command::Rename(from, to)) => match self.people.rename(&from, &to) {
                Ok(()) => {
                    self.people_window.renaming = None;
                    self.people_window.thumbs.clear();
                    self.people_window.selected = Some(to.clone());
                    self.save_people();
                    self.log.info(format!("Renamed {from} to {to}"));
                    self.rematch_faces(Some((&from, &to)));
                }
                Err(e) => self.log.error(e),
            },
            Some(Command::Delete(name)) => {
                self.people.remove(&name);
                self.people_window.deleting = None;
                self.people_window.selected = None;
                self.people_window.thumbs.clear();
                self.save_people();
                self.log.info(format!("Deleted {name}"));
                self.rematch_faces(None);
            }
            Some(Command::Cover(name, i)) => {
                self.people.set_cover(&name, i);
                self.people_window.thumbs.clear();
                self.save_people();
            }
            Some(Command::Reveal(path)) => {
                if let Some(dir) = path.parent() {
                    self.open_in_file_manager(dir);
                }
            }
            Some(Command::NameFaces(faces, name)) => {
                if let Some(first) = faces.first() {
                    self.people_window.group_names.remove(first);
                }
                self.assign_faces(&faces, &name);
            }
            Some(Command::HideFaces(faces)) => self.hide_faces(&faces),
            Some(Command::OpenCharts) => self.graph_window.open = true,
            Some(Command::Split(face)) => {
                self.people_window.split.insert(face);
                self.people_window.counts = None;
            }
            Some(Command::OpenSettingsFolder) => {
                let dir = self
                    .settings
                    .path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf);
                self.open_settings_subfolder(dir);
            }
            None => {}
        }
    }

    /// 顔や人物が変わった後（`counts` が `None`）なら、人物ごとの画像の枚数、
    /// 2 人が一緒に写った枚数、未登録の顔のまとまりを、記録してあるすべての顔の枠から作り直す。
    pub(super) fn refresh_face_counts(&mut self) {
        if self.people_window.counts.is_some() {
            return;
        }
        let marks: Vec<Vec<crate::faces::FaceMark>> = self
            .face_store
            .all()
            .into_iter()
            .map(|(image, _)| self.ai_tags.faces(&image))
            .collect();
        let w = &mut self.people_window;
        w.counts = Some(image_counts(marks.iter().map(Vec::as_slice)));
        w.together = Some(crate::people_graph::together(
            marks.iter().map(Vec::as_slice),
        ));
        self.people_window.unknown = self.unknown_groups();
        self.people_window.unknown_thumbs.clear();
    }

    pub(super) fn save_people(&mut self) {
        if let Err(e) = self.people.save() {
            self.log.error(format!("Cannot save people: {e}"));
        }
    }
}

/// 「未登録の顔」：似た顔のまとまりごとに、名前を付けて登録するか、隠す。
fn unregistered_tab(
    ui: &mut egui::Ui,
    w: &mut PeopleWindow,
    people: &[Person],
    lang: crate::settings::Language,
    command: &mut Option<Command>,
) {
    if w.unknown.is_empty() {
        ui.weak(tr(
            lang,
            "No unregistered faces. Right-click an image > Find faces to look for faces.",
        ));
        return;
    }
    ui.weak(tr(
        lang,
        "Similar faces are grouped. Name a group to register all its faces as that person.",
    ));
    let PeopleWindow {
        unknown,
        group_names,
        unknown_thumbs,
        ..
    } = w;
    egui::ScrollArea::vertical()
        .id_salt("unknown_groups")
        .max_height(480.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for group in unknown.iter().take(SHOWN_GROUPS) {
                let refs: Vec<FaceRef> = group.iter().map(|(r, _)| r.clone()).collect();
                let name = group_names.entry(refs[0].clone()).or_default();
                ui.horizontal(|ui| {
                    ui.label(tr1(lang, "{} face(s)", &group.len().to_string()));
                    let edit = ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text(tr(lang, "Name"))
                            .desired_width(140.0),
                    );
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let ok = ui.add_enabled(
                        !name.trim().is_empty(),
                        egui::Button::new(tr(lang, "Register")),
                    );
                    if ok.clicked() || (enter && !name.trim().is_empty()) {
                        *command = Some(Command::NameFaces(refs.clone(), name.trim().to_owned()));
                    }
                    if ui
                        .button(tr(lang, "Hide"))
                        .on_hover_text(tr(
                            lang,
                            "Hide these faces from this list (for example passers-by).",
                        ))
                        .clicked()
                    {
                        *command = Some(Command::HideFaces(refs.clone()));
                    }
                    if people.iter().any(|p| p.name == name.trim()) {
                        ui.weak(tr(lang, "Added to the registered person."));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for (face, sample) in group.iter().take(SHOWN_FACES) {
                        let tex = unknown_thumbs
                            .entry(face.clone())
                            .or_insert_with(|| {
                                texture(
                                    ui.ctx(),
                                    format!("unknown-{}-{}", face.0.display(), face.1),
                                    &sample.thumb,
                                )
                            })
                            .clone();
                        let image =
                            egui::Image::new(&tex).fit_to_exact_size(egui::vec2(48.0, 48.0));
                        let resp = ui
                            .add(egui::Button::image(image).frame(false))
                            .on_hover_text(face.0.display().to_string());
                        if resp.clicked() {
                            *command = Some(Command::Reveal(face.0.clone()));
                        }
                        vw_ui::menu::context(&resp, |ui| {
                            if group.len() > 1 && ui.button(tr(lang, "Not in this group")).clicked()
                            {
                                *command = Some(Command::Split(face.clone()));
                                ui.close();
                            }
                            if ui.button(tr(lang, "Hide this face")).clicked() {
                                *command = Some(Command::HideFaces(vec![face.clone()]));
                                ui.close();
                            }
                        });
                    }
                    if group.len() > SHOWN_FACES {
                        ui.weak(format!("+{}", group.len() - SHOWN_FACES));
                    }
                });
                ui.separator();
            }
            if unknown.len() > SHOWN_GROUPS {
                ui.weak(tr1(
                    lang,
                    "{} more group(s). Register or hide the ones above to see them.",
                    &(unknown.len() - SHOWN_GROUPS).to_string(),
                ));
            }
        });
}
