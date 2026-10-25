//! Datasets の欄。データセットを探して一覧する。ほかの欄と同じく本体に並べるか別ウインドウに出す。

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui;
use egui_phosphor::regular as icon;

use crate::dataset_search::{self, Condition, Hit, Index, Op, Progress, Query, Target};
use crate::i18n::{tr, tr1};
use crate::settings::{Language, SearchGroup};

/// 詳しい情報を表の横に並べる、欄の幅の下限。これより細ければ表の下に出す。
const SIDE_DETAIL_MIN_WIDTH: f32 = 640.0;

/// 画面から頼まれること。
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
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
    /// 送る。検索でファイルを絞り込んでいれば、当たったファイルのパスも渡す。
    Send(PathBuf, Option<Vec<String>>, crate::send_app::Choice),
    /// 今のメタデータの条件のキーを、検索のグループとして設定に足す（同じ名前なら置き換える）。
    SaveGroup(SearchGroup),
}

/// 「送る ▸」に並べるもの。
pub struct SendTo<'a> {
    pub destinations: &'a [String],
    pub apps: &'a [crate::settings::SendApp],
}

impl SendTo<'_> {
    /// データセットの「送る ▸」。`hits` があれば当たったファイル、なければデータセットのフォルダを送る。
    fn menu(
        &self,
        ui: &mut egui::Ui,
        lang: Language,
        hits: Option<&[String]>,
    ) -> Option<crate::send_app::Choice> {
        let first = hits.and_then(|h| h.first()).map(Path::new);
        let offer = crate::send_app::Offer::for_path(
            self.apps,
            Some(self.destinations),
            first.unwrap_or(Path::new("")),
            first.is_none(),
        );
        crate::send_app::menu(ui, lang, &offer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Column {
    Name,
    Tags,
    Field(usize),
    Hits,
    Location,
}

/// 1 つのデータセットのフォルダを探しているスレッド。
struct Walk {
    root: PathBuf,
    progress: Arc<Progress>,
    rx: Receiver<Index>,
    done: Option<Index>,
}

pub struct DatasetsScreen {
    index: Option<Index>,
    /// データセットのフォルダごとに探しているもの。全部そろったら 1 つにまとめる。
    loading: Vec<Walk>,
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
    /// 条件をグループとして保存するときに入力中の名前。
    group_name: Option<String>,
    /// 名前の欄を出したフレーム。そのときだけ入力を欄へ向ける（毎フレーム向けると
    /// 「保存」ボタンの押下を奪うため）。
    focus_group_name: bool,
}

impl Default for DatasetsScreen {
    fn default() -> Self {
        Self {
            index: None,
            loading: Vec::new(),
            query: Query::default(),
            advanced: true,
            hits: Vec::new(),
            error: None,
            searched: None,
            selected: None,
            hovered: None,
            sort: Column::Location,
            descending: false,
            group_name: None,
            focus_group_name: false,
        }
    }
}

impl DatasetsScreen {
    /// 画面を開いた。開くたびに探し直す。
    ///
    /// 前に探した結果を使い回さないのは、画面を閉じている間にエクスプローラなどで
    /// カタログを消したり足したりしても、開き直せば反映されるようにするため。
    /// 変わっていないカタログは読み直さないので、開き直すときはフォルダをたどる分だけかかる。
    /// 探している間は前の結果を出しておくので、開いた直後に一覧が空になることはない。
    pub fn opened(&mut self, roots: &[String], ctx: &egui::Context) {
        self.reload(roots, ctx);
    }

    /// データセットのフォルダ以下を別スレッドで探し直す。ネットワークドライブでは数秒かかるため。
    /// フォルダごとにスレッドを分けるのは、遅いドライブのフォルダがほかのフォルダを待たせないため。
    pub fn reload(&mut self, roots: &[String], ctx: &egui::Context) {
        self.loading = roots
            .iter()
            .map(|r| {
                let root = PathBuf::from(r);
                let progress = Arc::new(Progress::default());
                let rx = {
                    let (root, progress) = (root.clone(), progress.clone());
                    crate::background::compute(ctx, move || Index::walk_root(&root, &progress))
                };
                Walk {
                    root,
                    progress,
                    rx,
                    done: None,
                }
            })
            .collect();
        if self.loading.is_empty() {
            self.finish(Index::default());
        }
    }

    /// 探しているデータセットのフォルダごとの、（フォルダ, 見たフォルダの数, 見つけたカタログの数,
    /// 探し終えたか）。ほかのフォルダを待っていて、まだ一覧に出していないものも入る。
    pub fn progress(&self) -> Vec<(&Path, usize, usize, bool)> {
        self.loading
            .iter()
            .map(|w| {
                (
                    w.root.as_path(),
                    w.progress.folders.load(Ordering::Relaxed),
                    w.progress.found.load(Ordering::Relaxed),
                    w.done.is_some(),
                )
            })
            .collect()
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
        if self.loading.is_empty() {
            return;
        }
        for w in &mut self.loading {
            if w.done.is_none() {
                w.done = w.rx.try_recv().ok();
            }
        }
        if self.loading.iter().all(|w| w.done.is_some()) {
            let parts = std::mem::take(&mut self.loading)
                .into_iter()
                .filter_map(|w| w.done)
                .collect();
            self.finish(Index::merge(parts));
        }
    }

    fn finish(&mut self, index: Index) {
        if let Some(dir) = &self.selected {
            if !index.entries.iter().any(|e| &e.dir == dir) {
                self.selected = None;
            }
        }
        self.index = Some(index);
        self.searched = None;
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
        groups: &[SearchGroup],
        send: &SendTo,
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
            // 欄を細くしても、ボタンが切れずに次の行へ回るようにする。
            ui.horizontal_wrapped(|ui| {
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
                if let Some(a) = self.conditions_ui(ui, lang, &field_names, groups) {
                    action = Some(a);
                }
            }
            ui.add_space(4.0);
        });
        if let Some(a) = self.detail_panel(ui, lang, &field_names, send) {
            action = Some(a);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            self.status_line(ui, lang, roots);
            if let Some(a) = self.table_ui(ui, lang, &field_names, send) {
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
            if !self.loading.is_empty() {
                let (folders, found) = self
                    .progress()
                    .iter()
                    .fold((0, 0), |(f, n), (_, a, b, _)| (f + a, n + b));
                ui.spinner();
                ui.weak(tr(lang, "Looking for datasets…"));
                ui.weak(format!(
                    "{}, {}",
                    tr1(lang, "{} folders", &folders.to_string()),
                    tr1(lang, "{} datasets", &found.to_string())
                ));
                return;
            }
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return;
            }
            let Some(index) = &self.index else {
                return;
            };
            if index.entries.is_empty() && index.problems.is_empty() {
                ui.weak(tr(
                    lang,
                    "No datasets in the dataset folders yet. Right-click a folder in the folder tree and choose Register as dataset... to make one.",
                ));
                return;
            }
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

    fn conditions_ui(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        field_names: &[String],
        groups: &[SearchGroup],
    ) -> Option<Action> {
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
                vw_ui::menu::combo(("dataset-op", i))
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
                                vw_ui::menu::button(ui, icon::CARET_DOWN, |ui| {
                                    value_list(ui, lang, choices, &mut c.value)
                                });
                            });
                        } else if let (Target::Meta { kind, key }, Some(index)) =
                            (&c.target, &self.index)
                        {
                            if !key.trim().is_empty() {
                                ui.push_id(("dataset-values", i), |ui| {
                                    // 値はファイルの数だけあり得るので、メニューを開いたときだけ集める。
                                    vw_ui::menu::button(ui, icon::CARET_DOWN, |ui| {
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
        let mut action = None;
        ui.horizontal(|ui| {
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
            vw_ui::menu::button(ui,
                format!("{} {} {}", icon::STACK, tr(lang, "Group"), icon::CARET_DOWN),
                |ui| {
                    if groups.is_empty() {
                        ui.weak(tr(lang, "Add search groups in Settings > Datasets."));
                    }
                    for g in groups {
                        if ui
                            .button(&g.name)
                            .on_hover_text(g.keys.join("\n"))
                            .clicked()
                        {
                            add_group(&mut self.query.conditions, g);
                            ui.close();
                        }
                    }
                },
            );
            let savable = group_from(&self.query.conditions, "x").is_some();
            let focus = std::mem::take(&mut self.focus_group_name);
            match &mut self.group_name {
                None => {
                    if ui
                        .add_enabled(
                            savable,
                            egui::Button::new(tr(lang, "Save as group...")),
                        )
                        .on_hover_text(tr(
                            lang,
                            "Save the keys of the metadata conditions as a search group in the settings.",
                        ))
                        .clicked()
                    {
                        self.group_name = Some(String::new());
                        self.focus_group_name = true;
                    }
                }
                Some(name) => {
                    let edit = ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text(tr(lang, "Group name"))
                            .desired_width(140.0),
                    );
                    if focus {
                        edit.request_focus();
                    }
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.group_name = None;
                        return;
                    }
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let group = group_from(&self.query.conditions, name);
                    let save = ui.add_enabled(group.is_some(), egui::Button::new(tr(lang, "Save")));
                    if save.clicked() || (enter && group.is_some()) {
                        action = group.map(Action::SaveGroup);
                        self.group_name = None;
                    } else if ui.button(tr(lang, "Cancel")).clicked() {
                        self.group_name = None;
                    }
                }
            }
        });
        action
    }

    fn detail_panel(
        &self,
        ui: &mut egui::Ui,
        lang: Language,
        field_names: &[String],
        send: &SendTo,
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
        let hits = narrowed_paths(index, hit);
        let mut action = None;
        // 細い欄に入れたときは、横に並べると表が隠れるので、下に出す。
        let panel = if ui.available_width() < SIDE_DETAIL_MIN_WIDTH {
            egui::Panel::bottom("dataset_detail_below")
                .default_size(300.0)
                .min_size(160.0)
        } else {
            egui::Panel::right("dataset_detail")
                .default_size(300.0)
                .min_size(220.0)
        };
        panel.show(ui, |ui| {
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
                    if let Some(choice) = send.menu(ui, lang, hits.as_deref()) {
                        action = Some(Action::Send(entry.dir.clone(), hits.clone(), choice));
                    }
                });
                if let Some(hits) = &hits {
                    if ui
                        .button(tr1(
                            lang,
                            "Show the {} files found",
                            &hits.len().to_string(),
                        ))
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
        send_to: &SendTo,
    ) -> Option<Action> {
        let index = self.index.as_ref()?;
        let mut send = None;
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
        // 場所は深いフォルダほど長く、そのまま出すと表が窓より広がって右に大きな空きができる。
        // 違いの出る末尾を残して頭を省き、列の幅を窓の幅の一部に収める。
        let location_width = (ui.available_width() * LOCATION_SHARE).max(LOCATION_MIN);
        let location_font = egui::TextStyle::Body.resolve(ui.style());
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
                        let full = e.dir.display().to_string();
                        let shown = vw_ui::text::tail_that_fits(&full, |t| {
                            ui.painter()
                                .layout_no_wrap(
                                    t.to_owned(),
                                    location_font.clone(),
                                    egui::Color32::PLACEHOLDER,
                                )
                                .size()
                                .x
                                <= location_width
                        });
                        let cut = shown != full;
                        let location = ui.label(egui::RichText::new(shown).weak());
                        let last = location.rect;
                        if cut {
                            location.on_hover_text(full);
                        }
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
                        vw_ui::menu::context(&row, |ui| {
                            let hits = narrowed_paths(index, h);
                            if let Some(choice) = send_to.menu(ui, lang, hits.as_deref()) {
                                send = Some(Action::Send(e.dir.clone(), hits, choice));
                            }
                        });
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
        send.or(show.map(Action::Show))
    }
}

/// 結果の表で場所の列に使う、窓の幅の割合と最小の幅。
const LOCATION_SHARE: f32 = 0.4;
const LOCATION_MIN: f32 = 180.0;

/// 検索で絞り込んだときだけ、当たったファイルのパス。すべてのファイルが当たったときは、
/// データセットそのものを渡すのと同じため `None`。
/// グループのキーごとに、値の一致の条件を足す。同じキーの条件がもうあれば足さない。
fn add_group(conditions: &mut Vec<Condition>, group: &SearchGroup) {
    for k in &group.keys {
        let (kind, key) = SearchGroup::split_key(k);
        let present = conditions.iter().any(|c| {
            matches!(&c.target, Target::Meta { kind: ck, key: kk }
                if ck.trim().eq_ignore_ascii_case(kind) && kk.trim().eq_ignore_ascii_case(key))
        });
        if !present {
            conditions.push(Condition {
                target: Target::Meta {
                    kind: kind.to_owned(),
                    key: key.to_owned(),
                },
                op: Op::Equals,
                value: String::new(),
                max: String::new(),
            });
        }
    }
}

/// 今の条件のうち、キーのあるメタデータの条件を `name` のグループにする。なければ `None`。
fn group_from(conditions: &[Condition], name: &str) -> Option<SearchGroup> {
    let keys: Vec<String> = conditions
        .iter()
        .filter_map(|c| match &c.target {
            Target::Meta { kind, key } if !key.trim().is_empty() => Some(match kind.trim() {
                "" => key.trim().to_owned(),
                kind => format!("{kind}:{}", key.trim()),
            }),
            _ => None,
        })
        .collect();
    let group = SearchGroup {
        name: name.trim().to_owned(),
        keys,
    };
    group.problem().is_none().then_some(group)
}

fn narrowed_paths(index: &Index, hit: &Hit) -> Option<Vec<String>> {
    let files = &index.entries[hit.entry].catalog.files;
    hit.narrowed
        .then(|| hit.files.iter().map(|&f| files[f].path.clone()).collect())
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
    vw_ui::menu::combo(("dataset-target", row))
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
    egui::ScrollArea::vertical()
        .id_salt("dataset-hits")
        .max_height(height)
        .auto_shrink([false, true])
        .show_rows(ui, row, hits.len(), |ui, range| {
            for path in &hits[range] {
                let name = path.rsplit_once('/').map_or(path.as_str(), |(_, n)| n);
                vw_ui::text::tail_label(ui, name, Some(path));
            }
        });
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
        vw_ui::menu::button(ui, icon::CARET_DOWN, |ui| {
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
        while !screen.loading.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            screen.poll();
        }
        screen.index().expect("discovery finished")
    }

    fn meta(kind: &str, key: &str, op: Op, value: &str) -> Condition {
        Condition {
            target: Target::Meta {
                kind: kind.into(),
                key: key.into(),
            },
            op,
            value: value.into(),
            max: String::new(),
        }
    }

    #[test]
    fn picking_a_group_adds_an_equals_condition_per_key_it_does_not_have_yet() {
        let group = SearchGroup {
            name: "Patient".into(),
            keys: vec!["PatientID".into(), "exif:Model".into(), "0010,0010".into()],
        };
        let mut conditions = vec![meta("", "patientid", Op::Contains, "12")];
        add_group(&mut conditions, &group);
        assert_eq!(
            conditions,
            [
                meta("", "patientid", Op::Contains, "12"),
                meta("exif", "Model", Op::Equals, ""),
                meta("", "0010,0010", Op::Equals, ""),
            ]
        );
    }

    #[test]
    fn saving_conditions_as_a_group_keeps_only_metadata_keys() {
        let conditions = vec![
            meta("", "PatientID", Op::Equals, "12"),
            Condition {
                target: Target::Name,
                op: Op::Contains,
                value: "ct".into(),
                max: String::new(),
            },
            meta("exif", "Model", Op::Equals, ""),
            meta("", " ", Op::Equals, ""),
        ];
        let group = group_from(&conditions, " Mine ").unwrap();
        assert_eq!(group.name, "Mine");
        assert_eq!(group.keys, ["PatientID", "exif:Model"]);
        assert_eq!(group_from(&conditions, ""), None);
        assert_eq!(group_from(&conditions[1..2], "Mine"), None);
    }

    #[test]
    fn a_group_filled_with_exact_values_finds_only_the_matching_files() {
        let dir = crate::test_util::TempDir::new();
        let files: &[&[(&str, &str, &str)]] = &[
            &[("dicom", "0010,0020", "P1"), ("dicom", "0008,0060", "CT")],
            &[("dicom", "0010,0020", "P12"), ("dicom", "0008,0060", "CT")],
            &[("dicom", "0010,0020", "P1"), ("dicom", "0008,0060", "MR")],
        ];
        let mut c = Catalog {
            name: "set".into(),
            ..Catalog::default()
        };
        for (n, triples) in files.iter().enumerate() {
            let mut ids = Vec::new();
            for &(k, key, v) in *triples {
                let t = (k.to_owned(), key.to_owned(), v.to_owned());
                let i = c.meta.iter().position(|x| *x == t).unwrap_or_else(|| {
                    c.meta.push(t);
                    c.meta.len() - 1
                });
                ids.push(i as u32);
            }
            c.files.push(catalog::FileEntry {
                path: format!("{n}.dcm"),
                size: 0,
                modified: String::new(),
                meta: ids,
            });
        }
        catalog::save(dir.path(), &c).unwrap();
        let index = Index::discover(&[dir.path().to_path_buf()]);
        let group = SearchGroup {
            name: "Patient".into(),
            keys: vec!["PatientID".into(), "Modality".into()],
        };
        let mut query = Query::default();
        add_group(&mut query.conditions, &group);
        query.conditions[0].value = "P1".into();
        query.conditions[1].value = "ct".into();
        let hits = dataset_search::search(&index, &query).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].files, [0]);
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
