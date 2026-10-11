//! Datasets 画面。設定画面と同じくメインの領域を置き換え、データセットを探して一覧する。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use eframe::egui;
use egui_phosphor::regular as icon;

use crate::dataset_search::{self, Condition, Hit, Index, Op, Query, Target};
use crate::i18n::{tr, tr1};
use crate::settings::Language;

/// 画面から頼まれること。
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Close,
    Edit(PathBuf),
    /// 並べたデータセットを順に更新する。
    Update(Vec<PathBuf>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Column {
    Name,
    Tags,
    Field(usize),
    Hits,
    Location,
}

pub struct DatasetsScreen {
    index: Option<Index>,
    loading: Option<Receiver<Index>>,
    query: Query,
    advanced: bool,
    hits: Vec<Hit>,
    error: Option<String>,
    /// 結果を出したときの条件。変わったら出し直す。
    searched: Option<Query>,
    selected: Option<PathBuf>,
    sort: Column,
    descending: bool,
}

impl Default for DatasetsScreen {
    fn default() -> Self {
        Self {
            index: None,
            loading: None,
            query: Query::default(),
            advanced: false,
            hits: Vec::new(),
            error: None,
            searched: None,
            selected: None,
            sort: Column::Location,
            descending: false,
        }
    }
}

impl DatasetsScreen {
    /// 画面を開いた。まだ一度も探していなければ探し始める。
    pub fn opened(&mut self, roots: &[String], ctx: &egui::Context) {
        if self.index.is_none() && self.loading.is_none() {
            self.reload(roots, ctx);
        }
    }

    /// データセットのフォルダ以下を別スレッドで探し直す。ネットワークドライブでは数秒かかるため。
    pub fn reload(&mut self, roots: &[String], ctx: &egui::Context) {
        let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Index::discover(&roots));
            ctx.request_repaint();
        });
        self.loading = Some(rx);
    }

    /// 編集や更新が終わったデータセットを読み直す。
    pub fn refresh(&mut self, dir: &Path) {
        if let Some(index) = &mut self.index {
            index.reload(dir);
            self.searched = None;
        }
    }

    /// 探し終えた一覧。登録のダイアログで今ある値を候補に出すのに使う。
    pub fn index(&self) -> Option<&Index> {
        self.index.as_ref()
    }

    fn poll(&mut self) {
        let Some(rx) = &self.loading else {
            return;
        };
        if let Ok(index) = rx.try_recv() {
            self.index = Some(index);
            self.loading = None;
            self.searched = None;
        }
    }

    fn search(&mut self) {
        if self.searched.as_ref() == Some(&self.query) {
            return;
        }
        let Some(index) = &self.index else {
            return;
        };
        match dataset_search::search(index, &self.query) {
            Ok(hits) => {
                self.hits = hits;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.searched = Some(self.query.clone());
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        roots: &[String],
        fields: &[String],
    ) -> Option<Action> {
        self.poll();
        self.search();
        let mut action = None;
        let field_names = self
            .index
            .as_ref()
            .map(|i| i.field_names(fields))
            .unwrap_or_else(|| fields.to_vec());
        egui::Panel::top("datasets_header").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} {}", icon::ARROW_LEFT, tr(lang, "Back")))
                    .clicked()
                {
                    action = Some(Action::Close);
                }
                ui.heading(tr(lang, "Datasets"));
            });
            ui.horizontal(|ui| {
                ui.label(icon::MAGNIFYING_GLASS);
                ui.add(
                    egui::TextEdit::singleline(&mut self.query.text)
                        .hint_text(tr(lang, "Search names, memos, tags, fields and metadata"))
                        .desired_width((ui.available_width() - 340.0).max(160.0)),
                );
                let arrow = if self.advanced {
                    icon::CARET_UP
                } else {
                    icon::CARET_DOWN
                };
                if ui
                    .selectable_label(self.advanced, format!("{} {arrow}", tr(lang, "Advanced")))
                    .clicked()
                {
                    self.advanced ^= true;
                }
                if ui.button(tr(lang, "Reload")).clicked() {
                    let ctx = ui.ctx().clone();
                    self.reload(roots, &ctx);
                }
                let shown: Vec<PathBuf> = self.hit_dirs();
                if ui
                    .add_enabled(!shown.is_empty(), egui::Button::new(tr(lang, "Update all")))
                    .on_hover_text(tr(
                        lang,
                        "Read all files of the datasets shown again and rebuild their search catalogs.",
                    ))
                    .clicked()
                {
                    action = Some(Action::Update(shown));
                }
            });
            if self.advanced {
                self.conditions_ui(ui, lang, &field_names);
            }
            ui.add_space(4.0);
        });
        if let Some(a) = self.detail_panel(ui, lang, &field_names) {
            action = Some(a);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            self.status_line(ui, lang, roots);
            self.table_ui(ui, lang, &field_names);
        });
        action
    }

    fn hit_dirs(&self) -> Vec<PathBuf> {
        let Some(index) = &self.index else {
            return Vec::new();
        };
        self.hits
            .iter()
            .map(|h| index.entries[h.entry].dir.clone())
            .collect()
    }

    fn status_line(&self, ui: &mut egui::Ui, lang: Language, roots: &[String]) {
        ui.horizontal(|ui| {
            if roots.is_empty() {
                ui.weak(tr(lang, "Add dataset folders in Settings > Datasets."));
                return;
            }
            if self.loading.is_some() {
                ui.spinner();
                ui.weak(tr(lang, "Looking for datasets…"));
                return;
            }
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return;
            }
            let Some(index) = &self.index else {
                return;
            };
            ui.weak(format!(
                "{} / {}",
                tr1(lang, "{} datasets", &self.hits.len().to_string()),
                index.entries.len()
            ));
            if !index.problems.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    tr1(
                        lang,
                        "{} search catalogs could not be read",
                        &index.problems.len().to_string(),
                    ),
                )
                .on_hover_text(index.problems.join("\n"));
            }
        });
    }

    fn conditions_ui(&mut self, ui: &mut egui::Ui, lang: Language, field_names: &[String]) {
        let tags = self.index.as_ref().map(Index::tags).unwrap_or_default();
        let values = self
            .index
            .as_ref()
            .map(Index::field_values)
            .unwrap_or_default();
        let keys: &[(String, String)] = self.index.as_ref().map_or(&[], |i| i.meta_keys.as_slice());
        let mut remove = None;
        for (i, c) in self.query.conditions.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                target_combo(ui, lang, i, &mut c.target, field_names);
                if let Target::Meta { kind, key } = &mut c.target {
                    ui.add(
                        egui::TextEdit::singleline(kind)
                            .hint_text(tr(lang, "kind"))
                            .desired_width(70.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(key)
                            .hint_text(tr(lang, "key"))
                            .desired_width(140.0),
                    );
                    key_menu(ui, i, kind, key, keys);
                }
                egui::ComboBox::from_id_salt(("dataset-op", i))
                    .selected_text(tr(lang, c.op.title()))
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for op in Op::ALL {
                            ui.selectable_value(&mut c.op, op, tr(lang, op.title()));
                        }
                    });
                match c.op {
                    Op::Exists => {}
                    Op::Range => {
                        ui.add(
                            egui::TextEdit::singleline(&mut c.value)
                                .hint_text(tr(lang, "min"))
                                .desired_width(80.0),
                        );
                        ui.label("–");
                        ui.add(
                            egui::TextEdit::singleline(&mut c.max)
                                .hint_text(tr(lang, "max"))
                                .desired_width(80.0),
                        );
                    }
                    _ => {
                        ui.add(egui::TextEdit::singleline(&mut c.value).desired_width(200.0));
                        let choices: &[String] = match &c.target {
                            Target::Tags => &tags,
                            Target::Field(f) => values.get(f).map_or(&[], Vec::as_slice),
                            _ => &[],
                        };
                        if !choices.is_empty() {
                            ui.menu_button(icon::CARET_DOWN, |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(300.0)
                                    .show(ui, |ui| {
                                        for v in choices {
                                            if ui.button(v).clicked() {
                                                c.value.clone_from(v);
                                                ui.close();
                                            }
                                        }
                                    });
                            });
                        }
                    }
                }
                if ui.button(icon::X).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.query.conditions.remove(i);
        }
        if ui
            .button(format!("{} {}", icon::PLUS, tr(lang, "Condition")))
            .clicked()
        {
            self.query.conditions.push(Condition {
                target: Target::Meta {
                    kind: String::new(),
                    key: String::new(),
                },
                op: Op::Contains,
                value: String::new(),
                max: String::new(),
            });
        }
    }

    fn detail_panel(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        field_names: &[String],
    ) -> Option<Action> {
        let index = self.index.as_ref()?;
        let dir = self.selected.as_ref()?;
        let entry = index.entries.iter().find(|e| &e.dir == dir)?;
        let c = &entry.catalog;
        let mut action = None;
        egui::Panel::right("dataset_detail")
            .default_size(300.0)
            .min_size(220.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.heading(&c.name);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(entry.dir.display().to_string())
                                .monospace()
                                .weak(),
                        )
                        .wrap(),
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr(lang, "Edit...")).clicked() {
                            action = Some(Action::Edit(entry.dir.clone()));
                        }
                        if ui.button(tr(lang, "Update")).clicked() {
                            action = Some(Action::Update(vec![entry.dir.clone()]));
                        }
                    });
                    ui.separator();
                    if !c.memo.is_empty() {
                        ui.label(&c.memo);
                        ui.separator();
                    }
                    egui::Grid::new("dataset-detail-grid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label(tr(lang, "Tags"));
                            ui.label(c.tags.join(", "));
                            ui.end_row();
                            for f in field_names {
                                ui.label(f);
                                ui.label(c.fields.get(f).map_or("", String::as_str));
                                ui.end_row();
                            }
                            ui.label(tr(lang, "Files"));
                            ui.label(c.files.len().to_string());
                            ui.end_row();
                            ui.label(tr(lang, "Updated"));
                            ui.label(&c.updated);
                            ui.end_row();
                            if let Some(s) = &c.source {
                                ui.label(tr(lang, "Extracted from"));
                                ui.label(format!("{} ({})", s.name, s.path));
                                ui.end_row();
                            }
                        });
                    if !c.skipped.is_empty() {
                        ui.separator();
                        egui::CollapsingHeader::new(tr1(
                            lang,
                            "Not read ({})",
                            &c.skipped.len().to_string(),
                        ))
                        .show(ui, |ui| {
                            for s in &c.skipped {
                                ui.label(s.to_string());
                            }
                        });
                    }
                });
            });
        action
    }

    fn table_ui(&mut self, ui: &mut egui::Ui, lang: Language, field_names: &[String]) {
        let Some(index) = &self.index else {
            return;
        };
        let mut order: Vec<&Hit> = self.hits.iter().collect();
        let entry = |h: &Hit| &index.entries[h.entry];
        let key = |h: &Hit| -> String {
            let e = entry(h);
            match self.sort {
                Column::Name => e.catalog.name.to_lowercase(),
                Column::Tags => e.catalog.tags.join(", ").to_lowercase(),
                Column::Field(i) => field_names
                    .get(i)
                    .and_then(|f| e.catalog.fields.get(f))
                    .cloned()
                    .unwrap_or_default()
                    .to_lowercase(),
                Column::Hits => format!("{:012}", h.files),
                Column::Location => e.dir.display().to_string(),
            }
        };
        order.sort_by_cached_key(|h| key(h));
        if self.descending {
            order.reverse();
        }
        // 場所の順のときだけ、内側のデータセットを外側の下に字下げする。
        let nest = self.sort == Column::Location && !self.descending;
        let dirs: Vec<&Path> = order.iter().map(|h| entry(h).dir.as_path()).collect();
        let mut clicked = None;
        let mut sort_by = None;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("datasets-table")
                .num_columns(4 + field_names.len())
                .striped(true)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    let mut header = |ui: &mut egui::Ui, title: &str, column: Column| {
                        let mark = match (self.sort == column, self.descending) {
                            (true, false) => format!(" {}", icon::CARET_UP),
                            (true, true) => format!(" {}", icon::CARET_DOWN),
                            _ => String::new(),
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(format!("{title}{mark}")).strong(),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            sort_by = Some(column);
                        }
                    };
                    header(ui, tr(lang, "Name"), Column::Name);
                    header(ui, tr(lang, "Tags"), Column::Tags);
                    for (i, f) in field_names.iter().enumerate() {
                        header(ui, f, Column::Field(i));
                    }
                    header(ui, tr(lang, "Hits / files"), Column::Hits);
                    header(ui, tr(lang, "Location"), Column::Location);
                    ui.end_row();
                    for h in &order {
                        let e = entry(h);
                        let depth = if nest {
                            dirs.iter()
                                .filter(|d| **d != e.dir && e.dir.starts_with(d))
                                .count()
                        } else {
                            0
                        };
                        let selected = self.selected.as_ref() == Some(&e.dir);
                        let name = format!("{}{}", "    ".repeat(depth), e.catalog.name);
                        if ui.selectable_label(selected, name).clicked() {
                            clicked = Some(e.dir.clone());
                        }
                        ui.label(e.catalog.tags.join(", "));
                        for f in field_names {
                            ui.label(e.catalog.fields.get(f).map_or("", String::as_str));
                        }
                        ui.label(format!("{} / {}", h.files, e.catalog.files.len()));
                        ui.label(egui::RichText::new(e.dir.display().to_string()).weak());
                        ui.end_row();
                    }
                });
        });
        if let Some(column) = sort_by {
            if self.sort == column {
                self.descending ^= true;
            } else {
                self.sort = column;
                self.descending = false;
            }
        }
        if let Some(dir) = clicked {
            self.selected = Some(dir);
        }
    }
}

fn target_title(target: &Target, lang: Language) -> String {
    match target {
        Target::Name => tr(lang, "Name").to_owned(),
        Target::Memo => tr(lang, "Memo").to_owned(),
        Target::Tags => tr(lang, "Tags").to_owned(),
        Target::Field(f) => f.clone(),
        Target::Meta { .. } => tr(lang, "Metadata").to_owned(),
    }
}

fn target_combo(
    ui: &mut egui::Ui,
    lang: Language,
    row: usize,
    target: &mut Target,
    field_names: &[String],
) {
    let mut choices = vec![Target::Name, Target::Memo, Target::Tags];
    choices.extend(field_names.iter().map(|f| Target::Field(f.clone())));
    choices.push(Target::Meta {
        kind: String::new(),
        key: String::new(),
    });
    egui::ComboBox::from_id_salt(("dataset-target", row))
        .selected_text(target_title(target, lang))
        .width(110.0)
        .show_ui(ui, |ui| {
            for choice in choices {
                let same = std::mem::discriminant(&choice) == std::mem::discriminant(target)
                    && !matches!((&choice, &*target), (Target::Field(a), Target::Field(b)) if a != b);
                if ui
                    .selectable_label(same, target_title(&choice, lang))
                    .clicked()
                    && !same
                {
                    *target = choice;
                }
            }
        });
}

/// メタデータのキーの候補。入力中の種類とキーを含むものを出し、DICOM のタグには名前を添える。
fn key_menu(
    ui: &mut egui::Ui,
    row: usize,
    kind: &mut String,
    key: &mut String,
    keys: &[(String, String)],
) {
    const MAX: usize = 200;
    let (k, q) = (kind.trim().to_lowercase(), key.trim().to_lowercase());
    ui.push_id(("dataset-keys", row), |ui| {
        ui.menu_button(icon::CARET_DOWN, |ui| {
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    let matching = keys.iter().filter(|(kk, key)| {
                        let name = vw_dicom::tag_name(key).unwrap_or_default().to_lowercase();
                        (k.is_empty() || kk.contains(&k))
                            && (q.is_empty()
                                || key.to_lowercase().contains(&q)
                                || name.contains(&q))
                    });
                    for (kk, kkey) in matching.take(MAX) {
                        let label = match vw_dicom::tag_name(kkey).filter(|_| kk == "dicom") {
                            Some(name) => format!("{kk}: {kkey}  {name}"),
                            None => format!("{kk}: {kkey}"),
                        };
                        if ui.button(label).clicked() {
                            kind.clone_from(kk);
                            key.clone_from(kkey);
                            ui.close();
                        }
                    }
                });
        });
    });
}
