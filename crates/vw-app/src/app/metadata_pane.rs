//! メタデータ欄。

use eframe::egui::{self, collapsing_header::CollapsingState, vec2};

use egui_phosphor::regular as icon;

use crate::copy::{self, Focus, MetaGroup, MetaSelection};
use crate::frames;
use crate::i18n::tr;
use crate::meta_view::{self, Edit};
use crate::settings::{HeaderClick, HeadingStyle, PathDisplay};

use super::*;

impl VwApp {
    /// メタデータ欄に出すスロット。アクティブなスロットが読み込み中でまだ画像がないときは、
    /// 直前に出していたスロットのまま変えない。
    ///
    /// 読み込み中に「画像なし」を出すと、スロットを足して画像を開くたびに表が消えては現れ、
    /// フォルダの画像をまとめて開くときに点滅して見えるため。
    pub(super) fn metadata_slot(&mut self) -> Option<usize> {
        let slots: Vec<SlotView> = self
            .slots
            .iter()
            .map(|s| SlotView {
                id: s.id,
                shown: s.shown.is_some(),
                loading: s.loading,
            })
            .collect();
        let index = metadata_source(&slots, self.active, self.metadata_from);
        self.metadata_from = index.map(|i| slots[i].id);
        index
    }

    /// 画像から読んだグループ。ファイル情報、AI タグ、プラグインのメタデータの順。画像がなければ `None`。
    fn metadata_found(&mut self) -> Option<Vec<meta_view::Group>> {
        let lang = self.lang();
        let index = self.metadata_slot()?;
        let shown = self.slots[index].shown.as_ref()?;
        let info = &shown.info;
        let row =
            |k: &'static str, v: String| meta_view::Row::new(FILE, k, tr(lang, k).to_owned(), v);
        let frame = frames::split(&shown.path);
        let mut file_rows = vec![
            row(PATH, frames::file_of(&shown.path).display().to_string()),
            row("Size", format!("{} bytes", info.file_size)),
            row("Plugin", info.plugin.clone()),
            row("Pixels", format!("{} x {}", info.width, info.height)),
            row("Color", color_name(info.color).into()),
            row(
                "Sample",
                format!(
                    "{} bit ({} significant)",
                    info.bits_per_sample, info.significant_bits
                ),
            ),
            row("Texture", {
                let [w, h] = shown.texture().size();
                // スライドショーの先読みは、フル解像度の描画用の画像を縮小画像の欄に入れて出す。
                let reduced = (w as u32) < info.width && (h as u32) < info.height;
                let kind = if shown.full.is_some() || !reduced {
                    ""
                } else {
                    " (thumbnail)"
                };
                format!("{w} x {h}{kind}")
            }),
        ];
        if let Some((_, i)) = frame {
            let total = self.slots[index]
                .sequence
                .as_ref()
                .map_or(0, |q| q.paths.len());
            file_rows.insert(1, row("Frame", format!("{} / {total}", i + 1)));
        }
        let mut groups = vec![meta_view::Group::new(
            FILE,
            tr(lang, FILE).to_owned(),
            file_rows,
        )];
        let path = shown.path.clone();
        let mut metadata = info.metadata.clone();
        if let Some(p) = self.labels.get(&frames::file_of(&path)).position {
            crate::labels::apply_position(&mut metadata, p);
        }
        let entries = metadata.entries;
        if let Some(rows) = self.ai_tag_rows(&path) {
            groups.push(meta_view::Group::new(
                super::ai_tagging::AI_TAGS,
                tr(lang, super::ai_tagging::AI_TAGS).to_owned(),
                rows,
            ));
        }
        groups.extend(group_metadata(&entries).into_iter().map(|(g, rows)| {
            let rows = rows
                .into_iter()
                .map(|(k, v)| {
                    meta_view::Row::new(g, k, shown_name(lang, g, k), shown_value(lang, g, k, v))
                })
                .collect();
            meta_view::Group::new(g, shown_name(lang, g, g), rows)
        }));
        if let Some(row) = self.place_row(&entries) {
            if let Some(group) = groups.iter_mut().find(|g| g.id == vw_core::exif::LOCATION) {
                group.rows.push(row);
            }
        }
        Some(groups)
    }

    /// 撮影地点に一番近い町の名前（地名の表があるときだけ）。
    fn place_row(&self, entries: &[(String, String, String)]) -> Option<meta_view::Row> {
        let places = self.place_names.as_ref()?;
        let m = vw_core::Metadata {
            entries: entries.to_vec(),
        };
        let gps = vw_core::exif::Gps::from_metadata(&m)?;
        let address = places.address(gps.latitude, gps.longitude)?;
        let lang = self.lang();
        Some(meta_view::Row::new(
            vw_core::exif::LOCATION,
            "Place",
            tr(lang, "Place").to_owned(),
            address.text(lang),
        ))
    }

    /// メタデータ欄に出すグループ。隠した項目と自分で作ったグループの設定を当てたもの。
    pub(super) fn pane_groups(&mut self) -> Option<Vec<meta_view::Group>> {
        let found = self.metadata_found()?;
        Some(meta_view::arrange(
            found,
            &self.settings().metadata,
            self.meta_show_hidden,
        ))
    }

    /// コピーに使う、見えているとおりの表。
    pub(super) fn metadata_groups(&mut self) -> Option<Vec<MetaGroup>> {
        Some(copy_groups(&self.pane_groups()?))
    }

    /// 欄の見出しの行。隠した項目があるときだけ、それを一時的に出す切り替えを右に置く。
    pub(super) fn metadata_header_ui(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        ui.horizontal(|ui| {
            vw_ui::panel::header(ui, tr(lang, "Metadata"));
            if !meta_view::any_hidden(&self.settings().metadata) {
                return;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let on = self.meta_show_hidden;
                let glyph = if on { icon::EYE } else { icon::EYE_SLASH };
                if ui
                    .add(egui::Button::new(glyph).small().selected(on))
                    .on_hover_text(tr(
                        lang,
                        if on {
                            "Hidden items are shown faint. Click to hide them again."
                        } else {
                            "Show hidden items faint"
                        },
                    ))
                    .clicked()
                {
                    self.meta_show_hidden = !on;
                }
            });
        });
    }

    /// スロットの画像について、ファイル情報とプラグインのメタデータを表で出す。
    /// 見出しと行はクリックで選んでコピーでき、値の文字列はドラッグで範囲選択できる。
    pub(super) fn metadata_ui(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let Some(groups) = self.pane_groups() else {
            ui.weak(tr(lang, "No image"));
            return;
        };
        let header_click = self.settings().behavior.header_click;
        let path_display = self.settings().metadata.path;
        let heading_style = self.settings().metadata.heading_style;
        let custom_names: Vec<String> = self
            .settings()
            .metadata
            .groups
            .iter()
            .map(|g| g.name.clone())
            .collect();
        let mut picked: Option<MetaSelection> = None;
        let mut copy: Option<MetaSelection> = None;
        let mut text_drag = false;
        let mut tag_action = None;
        let mut edit: Option<Edit> = None;
        let mut closed = self.meta_closed.clone();
        let selected = self.meta_selected.clone();
        let highlight = ui.visuals().selection.bg_fill;
        let accent = ui.visuals().hyperlink_color;
        let faint = ui.visuals().weak_text_color();
        let widest_key = groups
            .iter()
            .flat_map(|g| &g.rows)
            .map(|r| {
                egui::WidgetText::from(r.key.as_str())
                    .into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        egui::TextStyle::Body,
                    )
                    .size()
                    .x
            })
            .fold(0.0, f32::max);
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for group in &groups {
                    let title = &group.title;
                    let open_key = group.id.clone();
                    let id = ui.make_persistent_id(("metadata", &group.id, group.custom));
                    let default_open = !closed.contains(&open_key);
                    let state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
                    let group_sel = MetaSelection::Group(title.clone());
                    let is_selected = selected.as_ref() == Some(&group_sel);
                    let heading =
                        heading_text(title, heading_style, accent, group.hidden.then_some(faint));
                    let (_, head, _) = state
                        .show_header(ui, |ui| {
                            // 欄より長い見出しで欄が押し広げられ、その幅のまま戻らなくなるため省略する。
                            ui.add(egui::Button::selectable(is_selected, heading).truncate())
                        })
                        .body(|ui| {
                            let avail = ui.available_width();
                            let gap = ui.spacing().item_spacing.x;
                            let gap_y = ui.spacing().item_spacing.y;
                            let key_width = key_column_width(widest_key, avail - gap);
                            let value_width = (avail - gap - key_width).max(0.0);
                            let row_of = |i: usize| MetaSelection::Row(title.clone(), i);
                            let selected_row = match &selected {
                                Some(MetaSelection::Row(g, i)) if g == title => Some(*i),
                                _ => None,
                            };
                            for (i, r) in group.rows.iter().enumerate() {
                                let tint = |t: &str| {
                                    let t = egui::RichText::new(t);
                                    if r.hidden {
                                        t.color(faint)
                                    } else {
                                        t
                                    }
                                };
                                // 縞と選択の色は行の高さが決まってから塗るので、先に場所だけ取る。
                                let background = ui.painter().add(egui::Shape::Noop);
                                let row = ui.horizontal_top(|ui| {
                                    let key = fixed_width(ui, key_width, |ui| {
                                        ui.add(
                                            egui::Label::new(tint(&r.key))
                                                .truncate()
                                                .selectable(false)
                                                .sense(egui::Sense::click()),
                                        )
                                    });
                                    // 折り返すと深いフォルダのパスが何行にもなり、表の頭が押し下げられる。
                                    let value = fixed_width(ui, value_width, |ui| {
                                        if r.id == PATH_ID {
                                            path_label(ui, &r.value, path_display)
                                        } else {
                                            ui.add(egui::Label::new(tint(&r.value)).wrap())
                                        }
                                    });
                                    (key, value)
                                });
                                let fill = if selected_row == Some(i) {
                                    Some(highlight)
                                } else {
                                    (i % 2 == 1).then_some(ui.visuals().faint_bg_color)
                                };
                                if let Some(fill) = fill {
                                    let rect = row.response.rect.expand2(vec2(0.0, gap_y / 2.0));
                                    ui.painter().set(
                                        background,
                                        egui::epaint::RectShape::filled(rect, 0.0, fill),
                                    );
                                }
                                let (key, value) = row.inner;
                                if key.clicked() || value.clicked() {
                                    picked = Some(row_of(i));
                                }
                                if value.drag_started() {
                                    text_drag = true;
                                }
                                for response in [&key, &value] {
                                    vw_ui::menu::context(response, |ui| {
                                        if ui.button(tr(lang, "Copy")).clicked() {
                                            copy = Some(row_of(i));
                                            ui.close();
                                        }
                                        ui.separator();
                                        if let Some(e) = row_menu(ui, lang, group, r, &custom_names)
                                        {
                                            edit = Some(e);
                                            ui.close();
                                        }
                                        if let Some(tag) = &r.tag {
                                            ui.separator();
                                            if let Some(a) = tag_menu(ui, lang, tag) {
                                                tag_action = Some(a);
                                                ui.close();
                                            }
                                        }
                                    });
                                }
                            }
                        });
                    let label = head.inner;
                    if label.clicked() {
                        picked = Some(group_sel.clone());
                        // ▶ を押したときと同じく、保存された開閉状態を反転して次のフレームから効かせる。
                        if header_click == HeaderClick::SelectAndToggle {
                            let mut state =
                                CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
                            state.toggle(ui);
                            state.store(ui.ctx());
                        }
                    }
                    if let Some(state) = CollapsingState::load(ui.ctx(), id) {
                        if state.is_open() {
                            closed.remove(&open_key);
                        } else {
                            closed.insert(open_key);
                        }
                    }
                    vw_ui::menu::context(&label, |ui| {
                        if ui.button(tr(lang, "Copy")).clicked() {
                            copy = Some(group_sel.clone());
                            ui.close();
                        }
                        ui.separator();
                        if let Some(e) = group_menu(ui, lang, group) {
                            edit = Some(e);
                            ui.close();
                        }
                    });
                }
            });
        self.meta_closed = closed;
        let ctx = ui.ctx().clone();
        if text_drag {
            self.set_focus(&ctx, Focus::MetadataText);
        }
        if let Some(p) = picked {
            self.meta_selected = Some(p);
            self.set_focus(&ctx, Focus::Metadata);
        }
        if let Some(action) = tag_action {
            let image = self
                .metadata_slot()
                .and_then(|i| self.slots[i].shown.as_ref())
                .map(|s| s.path.clone());
            if let Some(image) = image {
                self.tag_action(&ctx, &image, action);
            }
        }
        if let Some(edit) = edit {
            self.metadata_edit(&ctx, edit);
        }
        if let Some(c) = copy {
            if let Some(text) = copy::metadata_text(&copy_groups(&groups), &c) {
                let what = selection_name(&c);
                self.copy_text(&ctx, text, &format!("metadata {what}"));
            }
        }
    }

    /// 右クリックで選んだ、表示の設定の変更を当てて保存する。
    fn metadata_edit(&mut self, ctx: &egui::Context, edit: Edit) {
        if let Edit::NewGroup(row) = edit {
            self.meta_new_group = Some((row, String::new(), true));
            return;
        }
        let m = &mut self.settings.settings.metadata;
        let changed = meta_view::apply(m, edit);
        if changed {
            self.settings_changed(ctx);
        }
    }

    /// 「新しいグループ…」で名前を聞く。
    pub(super) fn meta_new_group_ui(&mut self, ctx: &egui::Context) {
        let Some((row, mut name, focus)) = self.meta_new_group.take() else {
            return;
        };
        let lang = self.lang();
        let taken = self
            .settings()
            .metadata
            .groups
            .iter()
            .any(|g| g.name == name.trim());
        let mut answer = None;
        let mut focus = focus;
        let mut open = true;
        egui::Window::new(tr(lang, "New group"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(tr(lang, "Name of the new group"));
                let r = ui.text_edit_singleline(&mut name);
                // 毎フレーム焦点を求めると、ボタンのクリックが取られてしまう。開いたときだけにする。
                if focus {
                    r.request_focus();
                    focus = false;
                }
                if taken {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        tr(lang, "A group with this name already exists."),
                    );
                }
                let ok = !name.trim().is_empty() && !taken;
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(ok, egui::Button::new(tr(lang, "Create")))
                        .clicked()
                        || (ok && enter)
                    {
                        answer = Some(true);
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        answer = Some(false);
                    }
                });
            });
        if !open {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                let edit = Edit::AddToGroup(row, name.trim().to_owned());
                self.metadata_edit(ctx, edit);
            }
            Some(false) => {}
            None => self.meta_new_group = Some((row, name, focus)),
        }
    }
}

/// ファイル情報のグループの英語の名前と、パスの行。パスはコピーにはどの表示でもフルパスを使う。
const FILE: &str = "File";
const PATH: &str = "Path";
const PATH_ID: &str = "File/Path";

/// 形式のメタデータの名前は、ファイルに書かれたとおり英語で出す。訳すのは vw が作る
/// 撮影地点のグループだけ（EXIF の Color などを、ほかの訳語で置き換えてしまわないため）。
fn shown_name(lang: crate::settings::Language, group: &str, name: &str) -> String {
    if group == vw_core::exif::LOCATION {
        // 英語の "Location" はデータセットの一覧で「場所（フォルダ）」に訳しているので、
        // 訳の表を引かずにここで決める。
        match (lang, name == group) {
            (crate::settings::Language::Japanese, true) => "撮影地点".to_owned(),
            _ => crate::i18n::tr_text(lang, name),
        }
    } else {
        name.to_owned()
    }
}

/// 値も、vw が書く「地図で置いた」だけを訳す。
fn shown_value(lang: crate::settings::Language, group: &str, name: &str, value: &str) -> String {
    if group == vw_core::exif::LOCATION && name == crate::labels::POSITION_FROM {
        crate::i18n::tr_text(lang, value)
    } else {
        value.to_owned()
    }
}

fn copy_groups(groups: &[meta_view::Group]) -> Vec<MetaGroup> {
    groups
        .iter()
        .map(|g| {
            let rows = g
                .rows
                .iter()
                .map(|r| (r.key.clone(), r.value.clone()))
                .collect();
            (g.title.clone(), rows)
        })
        .collect()
}

/// グループの見出しの字。隠したグループを一時的に出しているときは薄くする。
fn heading_text(
    title: &str,
    style: HeadingStyle,
    accent: egui::Color32,
    faint: Option<egui::Color32>,
) -> egui::RichText {
    let text = egui::RichText::new(title);
    let text = match style {
        HeadingStyle::Color | HeadingStyle::Both => text.color(accent),
        HeadingStyle::Underline | HeadingStyle::Plain => text,
    };
    let text = match style {
        HeadingStyle::Underline | HeadingStyle::Both => text.underline(),
        HeadingStyle::Color | HeadingStyle::Plain => text,
    };
    match faint {
        Some(c) => text.color(c),
        None => text,
    }
}

/// 項目の右クリックに足す、表示の設定の項目。
fn row_menu(
    ui: &mut egui::Ui,
    lang: crate::settings::Language,
    group: &meta_view::Group,
    row: &meta_view::Row,
    custom_names: &[String],
) -> Option<Edit> {
    let mut edit = None;
    if group.custom {
        if ui.button(tr(lang, "Remove from this group")).clicked() {
            edit = Some(Edit::RemoveFromGroup(row.id.clone(), group.id.clone()));
        }
    } else if row.hidden {
        if ui.button(tr(lang, "Show this item")).clicked() {
            edit = Some(Edit::Show(row.id.clone()));
        }
    } else if ui.button(tr(lang, "Hide this item")).clicked() {
        edit = Some(Edit::Hide(row.id.clone()));
    }
    vw_ui::menu::submenu(ui, tr(lang, "Add to group"), |ui| {
        for name in custom_names {
            if *name == group.id && group.custom {
                continue;
            }
            if ui.button(name).clicked() {
                edit = Some(Edit::AddToGroup(row.id.clone(), name.clone()));
                ui.close();
            }
        }
        if !custom_names.is_empty() {
            ui.separator();
        }
        if ui.button(tr(lang, "New group...")).clicked() {
            edit = Some(Edit::NewGroup(row.id.clone()));
            ui.close();
        }
    });
    edit
}

/// グループの見出しの右クリックに足す、表示の設定の項目。
fn group_menu(
    ui: &mut egui::Ui,
    lang: crate::settings::Language,
    group: &meta_view::Group,
) -> Option<Edit> {
    if group.custom {
        return ui
            .button(tr(lang, "Delete this group"))
            .clicked()
            .then(|| Edit::DeleteGroup(group.id.clone()));
    }
    if group.hidden {
        ui.button(tr(lang, "Show this group"))
            .clicked()
            .then(|| Edit::Show(group.id.clone()))
    } else {
        ui.button(tr(lang, "Hide this group"))
            .clicked()
            .then(|| Edit::Hide(group.id.clone()))
    }
}

fn path_label(ui: &mut egui::Ui, path: &str, display: PathDisplay) -> egui::Response {
    match display {
        PathDisplay::Full => vw_ui::text::tail_label(ui, path, None),
        PathDisplay::Name => {
            let name = std::path::Path::new(path)
                .file_name()
                .map_or(path.into(), |n| n.to_string_lossy());
            vw_ui::text::tail_label(ui, &name, Some(path))
        }
    }
}

/// AI タグの行の右クリックに足す項目。
fn tag_menu(
    ui: &mut egui::Ui,
    lang: crate::settings::Language,
    tag: &str,
) -> Option<super::ai_tidy::TagAction> {
    use super::ai_tidy::TagAction;
    let items = [
        (
            "Remove this tag from this image",
            TagAction::RemoveHere(tag.to_owned()),
        ),
        (
            "Remove this tag from every image in this folder",
            TagAction::RemoveInFolder(tag.to_owned()),
        ),
        ("Never add this tag...", TagAction::Exclude(tag.to_owned())),
    ];
    items
        .into_iter()
        .find_map(|(title, action)| ui.button(tr(lang, title)).clicked().then_some(action))
}
