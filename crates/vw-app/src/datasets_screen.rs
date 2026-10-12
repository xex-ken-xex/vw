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
    /// データセットのフォルダへ移ってファイルを読み込む。
    Show(PathBuf),
    /// データセットの中の、検索で当たったファイル（カタログのパス）だけをスロットに並べる。
    ShowFiles(PathBuf, Vec<String>),
    /// 取り出す。検索でファイルを絞り込んでいれば、当たったファイルのパスも渡す。
    Extract(PathBuf, Option<Vec<String>>),
    Edit(PathBuf),
    /// 並べたデータセットを順に更新する。
    Update(Vec<PathBuf>),
    /// 検索カタログを探し直す。
    Reload,
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
    /// 前のフレームでポインタが乗っていた行。行の色はグリッドを描く前に決めるため。
    hovered: Option<PathBuf>,
    sort: Column,
    descending: bool,
}

impl Default for DatasetsScreen {
    fn default() -> Self {
        Self {
            index: None,
            loading: None,
            query: Query::default(),
            advanced: true,
            hits: Vec::new(),
            error: None,
            searched: None,
            selected: None,
            hovered: None,
            sort: Column::Location,
            descending: false,
        }
    }
}

impl DatasetsScreen {
    /// 画面を開いた。開くたびに探し直す。
    ///
    /// 前に探した結果を使い回さないのは、画面を閉じている間にエクスプローラなどで
    /// カタログを消したり足したりしても、開き直せば反映されるようにするため。
    /// 探している間は前の結果を出しておくので、開いた直後に一覧が空になることはない。
    pub fn opened(&mut self, roots: &[String], ctx: &egui::Context) {
        self.reload(roots, ctx);
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
            if let Some(dir) = &self.selected {
                if !index.entries.iter().any(|e| &e.dir == dir) {
                    self.selected = None;
                }
            }
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
                if ui
                    .button(format!("{} {}", icon::ARROW_CLOCKWISE, tr(lang, "Reload")))
                    .on_hover_text(tr(
                        lang,
                        "Look for search catalogs again, to show catalogs added or deleted outside vw.",
                    ))
                    .clicked()
                {
                    action = Some(Action::Reload);
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
            if let Some(a) = self.table_ui(ui, lang, &field_names) {
                action = Some(a);
            }
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
                            ui.push_id(("dataset-values", i), |ui| {
                                ui.menu_button(icon::CARET_DOWN, |ui| {
                                    value_list(ui, lang, choices, &mut c.value)
                                });
                            });
                        } else if let (Target::Meta { kind, key }, Some(index)) =
                            (&c.target, &self.index)
                        {
                            if !key.trim().is_empty() {
                                ui.push_id(("dataset-values", i), |ui| {
                                    // 値はファイルの数だけあり得るので、メニューを開いたときだけ集める。
                                    ui.menu_button(icon::CARET_DOWN, |ui| {
                                        let values = index.meta_values(kind, key);
                                        value_list(ui, lang, &values, &mut c.value);
                                    });
                                });
                            }
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
        // 検索の結果に出ていないデータセットは、選んだままでも詳しく出さない。
        let hit = self
            .hits
            .iter()
            .find(|h| index.entries[h.entry].dir == *dir)?;
        let entry = &index.entries[hit.entry];
        let c = &entry.catalog;
        // 絞り込んだときだけ。すべてのファイルが当たったときは、データセットを開くのと同じため。
        let hits: Option<Vec<String>> = hit
            .narrowed
            .then(|| hit.files.iter().map(|&f| c.files[f].path.clone()).collect());
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
                        if ui.button(tr(lang, "Show")).clicked() {
                            action = Some(Action::Show(entry.dir.clone()));
                        }
                        if ui.button(tr(lang, "Extract...")).clicked() {
                            action = Some(Action::Extract(entry.dir.clone(), hits.clone()));
                        }
                    });
                    if let Some(hits) = &hits {
                        if ui
                            .button(tr1(lang, "Show the {} files found", &hits.len().to_string()))
                            .on_hover_text(tr(
                                lang,
                                "Show only the files that meet the search conditions, one slot each.",
                            ))
                            .clicked()
                        {
                            action = Some(Action::ShowFiles(entry.dir.clone(), hits.clone()));
                        }
                    }
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
                    if let Some(hits) = &hits {
                        ui.separator();
                        // パネルの下まで使う。読めなかったファイルの見出しの分だけ空けておく。
                        let row = ui.spacing().interact_size.y;
                        let reserve = if c.skipped.is_empty() { 2.0 } else { 4.0 } * row;
                        let height = (ui.available_height() - reserve).max(8.0 * row);
                        egui::CollapsingHeader::new(tr1(
                            lang,
                            "Files found ({})",
                            &hits.len().to_string(),
                        ))
                        .default_open(true)
                        .show(ui, |ui| hit_list(ui, hits, height));
                    }
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

    fn table_ui(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        field_names: &[String],
    ) -> Option<Action> {
        let index = self.index.as_ref()?;
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
                Column::Hits => format!("{:012}", h.files.len()),
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
        let mut show = None;
        let mut sort_by = None;
        let mut hovered = None;
        // 0 行目は見出し。
        let row_of = |dir: Option<&PathBuf>| {
            dir.and_then(|d| dirs.iter().position(|e| *e == d.as_path()))
                .map(|i| i + 1)
        };
        let selected_row = row_of(self.selected.as_ref());
        let hovered_row = row_of(self.hovered.as_ref());
        let spacing = egui::vec2(16.0, 4.0);
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("datasets-table")
                .num_columns(4 + field_names.len())
                .with_row_color(move |row, style| {
                    if Some(row) == selected_row {
                        Some(style.visuals.selection.bg_fill)
                    } else if Some(row) == hovered_row {
                        Some(style.visuals.widgets.hovered.weak_bg_fill)
                    } else {
                        (row % 2 == 1).then_some(style.visuals.faint_bg_color)
                    }
                })
                .spacing(spacing)
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
                        let name = format!("{}{}", "    ".repeat(depth), e.catalog.name);
                        let first = ui.label(name).rect;
                        ui.label(e.catalog.tags.join(", "));
                        for f in field_names {
                            ui.label(e.catalog.fields.get(f).map_or("", String::as_str));
                        }
                        ui.label(format!("{} / {}", h.files.len(), e.catalog.files.len()));
                        let last = ui
                            .label(egui::RichText::new(e.dir.display().to_string()).weak())
                            .rect;
                        ui.end_row();
                        // セルの間のすき間も含めて、行のどこを押しても選べるようにする。
                        let rect = first.union(last).expand2(spacing / 2.0);
                        let row = ui.interact(
                            rect,
                            ui.id().with(("dataset-row", &e.dir)),
                            egui::Sense::click(),
                        );
                        if row.hovered() {
                            hovered = Some(e.dir.clone());
                        }
                        if row.clicked() {
                            clicked = Some(e.dir.clone());
                        }
                        if row.double_clicked() {
                            show = Some(e.dir.clone());
                        }
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
        if hovered != self.hovered {
            self.hovered = hovered;
            ui.ctx().request_repaint();
        }
        if let Some(dir) = clicked {
            self.selected = Some(dir);
        }
        show.map(Action::Show)
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

/// 当たったファイルのパス。数万件あっても重くならないよう、見えている行だけを描く。
fn hit_list(ui: &mut egui::Ui, hits: &[String], height: f32) {
    let row = ui.text_style_height(&egui::TextStyle::Body);
    let font = egui::TextStyle::Body.resolve(ui.style());
    egui::ScrollArea::vertical()
        .id_salt("dataset-hits")
        .max_height(height)
        .auto_shrink([false, true])
        .show_rows(ui, row, hits.len(), |ui, range| {
            for path in &hits[range] {
                let name = path.rsplit_once('/').map_or(path.as_str(), |(_, n)| n);
                let width = ui.available_width();
                let shown = tail_that_fits(name, |s| {
                    ui.painter()
                        .layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
                        .size()
                        .x
                        <= width
                });
                ui.add(egui::Label::new(shown).extend()).on_hover_text(path);
            }
        });
}

/// `text` が `fits` に収まらなければ、頭を「…」にして末尾をできるだけ残す。
///
/// 連番のファイルは名前の違いが末尾に出るので、egui の省略（末尾を切る）は使わない。
fn tail_that_fits(text: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(text) {
        return text.to_owned();
    }
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let tail = |k: usize| format!("…{}", &text[starts[k]..]);
    let (mut lo, mut hi) = (0, starts.len().saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi) / 2;
        if fits(&tail(mid)) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    tail(lo)
}

/// 条件の値の候補。選ぶと `value` に入れる。
fn value_list(ui: &mut egui::Ui, lang: Language, values: &[String], value: &mut String) {
    const MAX: usize = 500;
    if values.is_empty() {
        ui.weak(tr(lang, "No values"));
        return;
    }
    egui::ScrollArea::vertical()
        .max_height(360.0)
        .show(ui, |ui| {
            for v in values.iter().take(MAX) {
                if ui.selectable_label(v == value, v).clicked() {
                    value.clone_from(v);
                    ui.close();
                }
            }
            if values.len() > MAX {
                ui.weak(tr1(
                    lang,
                    "… and {} more",
                    &(values.len() - MAX).to_string(),
                ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{self, Catalog};

    fn wait_for_index(screen: &mut DatasetsScreen) -> &Index {
        while screen.loading.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            screen.poll();
        }
        screen.index().expect("discovery finished")
    }

    #[test]
    fn long_names_keep_their_end_behind_an_ellipsis() {
        let fits = |s: &str| s.chars().count() <= 8;
        assert_eq!(tail_that_fits("a.png", fits), "a.png");
        assert_eq!(tail_that_fits("scan-0001-0042.ima", fits), "…042.ima");
        assert_eq!(
            tail_that_fits("写真の名前がとても長い.jpg", fits),
            "…も長い.jpg"
        );
    }

    #[test]
    fn reopening_the_screen_drops_a_catalog_deleted_meanwhile() {
        let tmp = crate::test_util::TempDir::new();
        let dir = tmp.mkdir("set");
        let c = Catalog {
            name: "set".into(),
            ..Catalog::default()
        };
        catalog::save(&dir, &c).unwrap();
        let roots = [tmp.path().display().to_string()];
        let ctx = egui::Context::default();
        let mut screen = DatasetsScreen {
            selected: Some(dir.clone()),
            ..DatasetsScreen::default()
        };

        screen.opened(&roots, &ctx);
        assert_eq!(wait_for_index(&mut screen).entries.len(), 1);
        assert_eq!(screen.selected.as_deref(), Some(dir.as_path()));

        std::fs::remove_file(catalog::path_in(&dir)).unwrap();
        screen.opened(&roots, &ctx);
        assert!(wait_for_index(&mut screen).entries.is_empty());
        assert_eq!(screen.selected, None);
    }

    #[test]
    fn reopening_the_screen_finds_a_catalog_added_meanwhile() {
        let tmp = crate::test_util::TempDir::new();
        let roots = [tmp.path().display().to_string()];
        let ctx = egui::Context::default();
        let mut screen = DatasetsScreen::default();
        screen.opened(&roots, &ctx);
        assert!(wait_for_index(&mut screen).entries.is_empty());

        catalog::save(&tmp.mkdir("new"), &Catalog::default()).unwrap();
        screen.opened(&roots, &ctx);
        assert_eq!(wait_for_index(&mut screen).entries.len(), 1);
    }
}
