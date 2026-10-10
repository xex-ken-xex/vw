//! 設定画面。VSCode のように、検索欄の語で項目を絞り込める。
//!
//! 値を変えたらその場で反映・保存する（呼び出し側が [`Response::changed`] を見て行う）。
//! 「適用」ボタンを置かないのは、変更の結果を見ながら調整できるようにするため。

use eframe::egui::{self, Align, Color32, Event, RichText};
use egui_phosphor::regular as icon;

use crate::i18n::{tr, tr1};
use crate::keymap::{is_modifier, os_reserved, Action, KeyCombo, Keymap, Os, Refusal};
use crate::layout::SlotLayout;
use crate::settings::{
    parse_hex_color, FolderAction, HeaderClick, Language, OnLoadFolder, Settings, ThemeChoice,
    Tree, TreeMode, Viewer,
};
use crate::sort::SortKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Appearance,
    Viewer,
    Tree,
    Behavior,
    Keys,
}

impl Section {
    const ALL: [Section; 5] = [
        Section::Appearance,
        Section::Viewer,
        Section::Tree,
        Section::Behavior,
        Section::Keys,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Viewer => "Viewer",
            Section::Tree => "Folder tree",
            Section::Behavior => "Behavior",
            Section::Keys => "Keyboard shortcuts",
        }
    }
}

/// 設定項目 1 つ。`widget` は値を編集する部品を描き、変更があれば true を返す。
struct Item {
    section: Section,
    /// 設定ファイルでの場所。
    key: &'static str,
    title: &'static str,
    description: &'static str,
    widget: fn(&mut Settings, &mut egui::Ui, Language) -> bool,
}

const ITEMS: [Item; 17] = [
    Item {
        section: Section::Appearance,
        key: "appearance.theme",
        title: "Theme",
        description: "Follow the OS dark/light setting, or always use one.",
        widget: theme_widget,
    },
    Item {
        section: Section::Appearance,
        key: "appearance.language",
        title: "Language",
        description: "Language of menus and labels.",
        widget: language_widget,
    },
    Item {
        section: Section::Appearance,
        key: "appearance.ui_scale",
        title: "UI scale",
        description: "Size of text and icons.",
        widget: scale_widget,
    },
    Item {
        section: Section::Appearance,
        key: "appearance.compact",
        title: "Compact panels",
        description: "Reduce padding to show more information.",
        widget: compact_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.background",
        title: "Background color",
        description: "Color behind images (#RRGGBB). Leave empty for the theme default.",
        widget: background_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.full_image_limit_mb",
        title: "Full-resolution memory limit",
        description: "Memory used for full-resolution images (MB). Beyond this, images are shown from 512 px thumbnails until memory frees up.",
        widget: limit_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.layout",
        title: "Slot layout",
        description: "How slots are arranged. Auto fits every slot on one page; a fixed grid shows the rest on further pages (Page Up / Page Down).",
        widget: layout_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.custom_layout",
        title: "Custom layout",
        description: "Columns x rows used by the Custom layout in the View menu.",
        widget: custom_layout_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.sort",
        title: "Sort order",
        description: "Order of files and folders in the tree, also used for previous/next image. Folders always come first.",
        widget: sort_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.sort_descending",
        title: "Descending",
        description: "Reverse the sort order (Z to A, newest first).",
        widget: descending_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.mode",
        title: "Tree mode",
        description: "Normal shows folders and images; click an image to open it. Auto-load shows folders only and loads all images of a folder into slots.",
        widget: mode_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.folder_action",
        title: "Folder click (auto-load)",
        description: "How a folder is expanded/collapsed and how it is loaded in auto-load mode. Images directly in the folder load at once; subfolders are asked about first.",
        widget: folder_action_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.double_click_ms",
        title: "Double-click time",
        description: "Two clicks within this time (ms) count as a double-click.",
        widget: double_click_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.on_load_other_folder",
        title: "Loading another folder",
        description: "What happens to the open slots when another folder is loaded. Expanding or collapsing a folder never closes slots.",
        widget: on_load_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.confirm_every",
        title: "Confirm large loads",
        description: "Ask before continuing each time the number of loaded images reaches the chunk size.",
        widget: confirm_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.confirm_chunk",
        title: "Chunk size",
        description: "Number of images to load before asking again.",
        widget: chunk_widget,
    },
    Item {
        section: Section::Behavior,
        key: "behavior.header_click",
        title: "Group heading click",
        description: "What clicking a group heading (such as File in the metadata panel) does. The arrow always expands or collapses.",
        widget: header_click_widget,
    },
];

fn theme_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.appearance.theme;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (ThemeChoice::System, "System"),
            (ThemeChoice::Dark, "Dark"),
            (ThemeChoice::Light, "Light"),
        ] {
            ui.radio_value(&mut s.appearance.theme, choice, tr(lang, label));
        }
    });
    s.appearance.theme != before
}

fn language_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let before = s.appearance.language;
    egui::ComboBox::from_id_salt("language")
        .selected_text(before.native_name())
        .show_ui(ui, |ui| {
            for l in Language::ALL {
                ui.selectable_value(&mut s.appearance.language, l, l.native_name());
            }
        });
    s.appearance.language != before
}

fn scale_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    use crate::settings::Appearance;
    let r = ui.add(
        egui::DragValue::new(&mut s.appearance.ui_scale)
            .range(Appearance::MIN_SCALE..=Appearance::MAX_SCALE)
            .speed(0.01)
            .fixed_decimals(2),
    );
    // ドラッグ中は反映しない。倍率が変わるとつまんでいる部品自体が動いてしまうため、離したときに反映する。
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn compact_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.appearance.compact, tr(lang, "Enabled"))
        .changed()
}

fn limit_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let r = ui.add(
        egui::DragValue::new(&mut s.viewer.full_image_limit_mb)
            .range(Viewer::LIMIT_RANGE_MB)
            .speed(64.0)
            .suffix(" MB"),
    );
    // ドラッグ中に反映すると、値が下がるたびに画像を捨てて読み直してしまうため、離したときに反映する。
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

/// レイアウトの選択肢に出す名前。
pub fn layout_title(layout: SlotLayout, lang: Language) -> String {
    match layout {
        SlotLayout::Auto => tr(lang, "Auto").to_owned(),
        SlotLayout::Grid { cols, rows } => format!("{cols} x {rows}"),
    }
}

fn layout_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.viewer.layout;
    let custom = s.viewer.custom_layout;
    egui::ComboBox::from_id_salt("viewer_layout")
        .selected_text(layout_title(before, lang))
        .show_ui(ui, |ui| {
            for layout in SlotLayout::PRESETS {
                ui.selectable_value(&mut s.viewer.layout, layout, layout_title(layout, lang));
            }
            if !SlotLayout::PRESETS.contains(&custom) {
                let text = format!("{} ({})", tr(lang, "Custom"), layout_title(custom, lang));
                ui.selectable_value(&mut s.viewer.layout, custom, text);
            }
        });
    s.viewer.layout != before
}

fn custom_layout_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let SlotLayout::Grid { mut cols, mut rows } = s.viewer.custom_layout else {
        return false;
    };
    let side = 1..=SlotLayout::MAX_SIDE;
    let changed = ui
        .horizontal(|ui| {
            let c = ui.add(egui::DragValue::new(&mut cols).range(side.clone()));
            ui.label("x");
            let r = ui.add(egui::DragValue::new(&mut rows).range(side));
            c.changed() || r.changed()
        })
        .inner;
    if !changed {
        return false;
    }
    let old = s.viewer.custom_layout;
    s.viewer.custom_layout = SlotLayout::grid(cols, rows);
    // カスタムを選んでいる間は、マス目を変えたらすぐ画面に反映する。
    if s.viewer.layout == old {
        s.viewer.layout = s.viewer.custom_layout;
    }
    true
}

fn sort_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.tree.sort;
    egui::ComboBox::from_id_salt("tree_sort")
        .selected_text(tr(lang, before.title()))
        .show_ui(ui, |ui| {
            for key in SortKey::ALL {
                ui.selectable_value(&mut s.tree.sort, key, tr(lang, key.title()));
            }
        });
    s.tree.sort != before
}

fn descending_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.tree.sort_descending, tr(lang, "Enabled"))
        .changed()
}

fn mode_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.tree.mode;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (TreeMode::Normal, "Normal"),
            (TreeMode::AutoLoad, "Auto-load images"),
        ] {
            ui.radio_value(&mut s.tree.mode, choice, tr(lang, label));
        }
    });
    s.tree.mode != before
}

fn folder_action_title(a: FolderAction) -> &'static str {
    match a {
        FolderAction::Delayed => "Click: expand after the double-click time / Double-click: load",
        FolderAction::Immediate => "Click: expand at once / Double-click: undo and load",
        FolderAction::ShiftClick => "Click: expand / Shift+click: load",
    }
}

fn folder_action_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.tree.folder_action;
    egui::ComboBox::from_id_salt("folder_action")
        .selected_text(tr(lang, folder_action_title(before)))
        .show_ui(ui, |ui| {
            for a in [
                FolderAction::Delayed,
                FolderAction::Immediate,
                FolderAction::ShiftClick,
            ] {
                ui.selectable_value(
                    &mut s.tree.folder_action,
                    a,
                    tr(lang, folder_action_title(a)),
                );
            }
        });
    s.tree.folder_action != before
}

fn double_click_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let r = ui.add(
        egui::DragValue::new(&mut s.tree.double_click_ms)
            .range(Tree::DOUBLE_CLICK_RANGE_MS)
            .speed(5.0)
            .suffix(" ms"),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn on_load_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.tree.on_load_other_folder;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (OnLoadFolder::Replace, "Replace the slots"),
            (OnLoadFolder::Append, "Add after the slots"),
        ] {
            ui.radio_value(&mut s.tree.on_load_other_folder, choice, tr(lang, label));
        }
    });
    s.tree.on_load_other_folder != before
}

fn confirm_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.tree.confirm_every, tr(lang, "Enabled"))
        .changed()
}

fn chunk_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let r = ui.add_enabled(
        s.tree.confirm_every,
        egui::DragValue::new(&mut s.tree.confirm_chunk)
            .range(Tree::CHUNK_RANGE)
            .speed(10.0)
            .suffix(tr(lang, " images")),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn header_click_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.behavior.header_click;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (HeaderClick::Select, "Select only"),
            (HeaderClick::SelectAndToggle, "Select and expand/collapse"),
        ] {
            ui.radio_value(&mut s.behavior.header_click, choice, tr(lang, label));
        }
    });
    s.behavior.header_click != before
}

fn background_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed = ui
            .add(
                egui::TextEdit::singleline(&mut s.viewer.background)
                    .hint_text(tr(lang, "Theme default"))
                    .desired_width(110.0),
            )
            .changed();
        let text = s.viewer.background.trim();
        match parse_hex_color(text) {
            Some(c) => {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, c);
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    ui.visuals().widgets.noninteractive.bg_stroke,
                    egui::StrokeKind::Inside,
                );
            }
            None if !text.is_empty() => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    tr(lang, "Use the #RRGGBB format."),
                );
            }
            None => {}
        }
    });
    changed
}

/// `query` の語（空白区切り）がすべて `texts` のどれかに含まれるか。大文字小文字は区別しない。
pub fn matches(query: &str, texts: &[&str]) -> bool {
    let haystack = texts.join("\n").to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

/// キー割り当ての操作の結果。該当する操作の行の下に出す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notice {
    Refused(Refusal),
    /// 割り当てたが、OS がそのキーを使っていることが多い。
    OsReserved(&'static str),
    Clipboard,
}

/// 設定画面の 1 フレーム分の結果。
#[derive(Default)]
pub struct Response {
    /// 設定が変わった。反映と保存が要る。
    pub changed: bool,
    pub open_file: bool,
    pub close: bool,
}

/// 設定画面の状態。
#[derive(Default)]
pub struct SettingsUi {
    query: String,
    /// キー入力を待っている操作。
    capturing: Option<Action>,
    notice: Option<(Action, Notice)>,
    jump: Option<Section>,
    focus_search: bool,
}

impl SettingsUi {
    /// 画面を開いたときに呼ぶ。すぐ検索語を打てるようにする。
    pub fn opened(&mut self) {
        self.focus_search = true;
        self.capturing = None;
        self.notice = None;
    }

    /// キー入力待ちなら、このフレームのキー入力を割り当てに使い、他の処理に渡らないよう取り除く。
    /// 割り当てが変わったら true。
    pub fn capture_keys(&mut self, ctx: &egui::Context, keymap: &mut Keymap) -> bool {
        let Some(action) = self.capturing else {
            return false;
        };
        let events = ctx.input_mut(|i| {
            let (keys, rest) = std::mem::take(&mut i.events).into_iter().partition(|e| {
                matches!(
                    e,
                    Event::Key { .. } | Event::Text(_) | Event::Copy | Event::Cut | Event::Paste(_)
                )
            });
            i.events = rest;
            keys
        });
        let (outcome, notice) = capture(action, &events, keymap, Os::CURRENT);
        match outcome {
            Capture::Waiting => {}
            Capture::Cancelled => self.capturing = None,
            Capture::Done => {
                self.capturing = None;
                self.notice = notice.map(|n| (action, n));
                return true;
            }
            Capture::Rejected => {
                self.capturing = None;
                self.notice = notice.map(|n| (action, n));
            }
        }
        false
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut Settings,
        file: Option<&str>,
        file_exists: bool,
    ) -> Response {
        let lang = settings.appearance.language;
        let mut out = Response::default();

        egui::Panel::top("settings_header").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} {}", icon::ARROW_LEFT, tr(lang, "Back")))
                    .clicked()
                {
                    out.close = true;
                }
                ui.heading(tr(lang, "Settings"));
            });
            ui.horizontal(|ui| {
                ui.label(icon::MAGNIFYING_GLASS);
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text(tr(lang, "Search settings"))
                        .desired_width((ui.available_width() - 200.0).max(120.0)),
                );
                if std::mem::take(&mut self.focus_search) {
                    search.request_focus();
                }
                let open = ui.add_enabled(
                    file.is_some(),
                    egui::Button::new(format!(
                        "{} {}",
                        icon::FILE_TEXT,
                        tr(lang, "Open settings file")
                    )),
                );
                if open.clicked() {
                    out.open_file = true;
                }
            });
            if let Some(path) = file {
                let note = if file_exists {
                    path.to_owned()
                } else {
                    format!("{path} ({})", tr(lang, "Not saved yet"))
                };
                ui.weak(RichText::new(note).small().monospace());
            }
            ui.add_space(2.0);
        });

        egui::Panel::left("settings_nav")
            .exact_size(170.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                for s in Section::ALL {
                    if ui.selectable_label(false, tr(lang, s.title())).clicked() {
                        self.query.clear();
                        self.jump = Some(s);
                    }
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.set_max_width(760.0);
                    let mut any = false;
                    for section in Section::ALL {
                        any |= self.section_ui(ui, section, settings, &mut out);
                    }
                    if !any {
                        ui.add_space(8.0);
                        ui.weak(tr(lang, "No settings match"));
                    }
                });
        });
        out
    }

    /// 1 つの節を描く。検索で 1 項目も残らなければ見出しも出さず false を返す。
    fn section_ui(
        &mut self,
        ui: &mut egui::Ui,
        section: Section,
        settings: &mut Settings,
        out: &mut Response,
    ) -> bool {
        let lang = settings.appearance.language;
        let items: Vec<&Item> = ITEMS
            .iter()
            .filter(|i| i.section == section)
            .filter(|i| {
                matches(
                    &self.query,
                    &[
                        i.key,
                        i.title,
                        i.description,
                        tr(lang, i.title),
                        tr(lang, i.description),
                    ],
                )
            })
            .collect();
        let actions: Vec<Action> = if section == Section::Keys {
            Action::ALL
                .into_iter()
                .filter(|a| self.action_matches(*a, &settings.keys, lang))
                .collect()
        } else {
            Vec::new()
        };
        if items.is_empty() && actions.is_empty() {
            return false;
        }

        ui.add_space(8.0);
        let heading = ui.label(
            RichText::new(tr(lang, section.title()).to_uppercase())
                .small()
                .strong(),
        );
        if self.jump == Some(section) {
            heading.scroll_to_me(Some(Align::TOP));
            self.jump = None;
        }
        ui.separator();

        for item in items {
            item_frame(ui, |ui| {
                title_line(ui, tr(lang, item.title), item.key);
                ui.weak(tr(lang, item.description));
                if (item.widget)(settings, ui, lang) {
                    out.changed = true;
                }
            });
        }
        for action in actions {
            if self.action_ui(ui, action, &mut settings.keys, lang) {
                out.changed = true;
            }
        }
        true
    }

    fn action_matches(&self, action: Action, keymap: &Keymap, lang: Language) -> bool {
        let key = format!("keys.{}", action.id());
        let combos: Vec<String> = keymap
            .keys(action)
            .iter()
            .map(KeyCombo::to_string)
            .collect();
        let mut texts = vec![key.as_str(), action.title(), tr(lang, action.title())];
        texts.extend(combos.iter().map(String::as_str));
        matches(&self.query, &texts)
    }

    /// ホットキー 1 行。キーの一覧（クリックで削除）、追加、既定に戻すボタン。
    fn action_ui(
        &mut self,
        ui: &mut egui::Ui,
        action: Action,
        keymap: &mut Keymap,
        lang: Language,
    ) -> bool {
        let mut changed = false;
        let duplicates = keymap.duplicates();
        item_frame(ui, |ui| {
            title_line(
                ui,
                tr(lang, action.title()),
                &format!("keys.{}", action.id()),
            );
            ui.horizontal_wrapped(|ui| {
                let keys = keymap.keys(action).to_vec();
                if keys.is_empty() {
                    ui.weak(tr(lang, "No keys"));
                }
                for k in keys {
                    let duplicated = duplicates.iter().any(|(c, _)| *c == k);
                    let reserved = os_reserved(Os::CURRENT, k);
                    // 等幅にしない。アイコンフォントは本文（プロポーショナル）にしか組み込んでいないため。
                    let mut text = RichText::new(format!("{k}  {}", icon::X));
                    let mut tip = tr(lang, "Click to remove").to_owned();
                    if duplicated {
                        text = text.color(ui.visuals().error_fg_color);
                        tip = format!("{}\n{tip}", tr(lang, "Also assigned to another action."));
                    } else if let Some(r) = reserved {
                        text = text.color(ui.visuals().warn_fg_color);
                        tip = format!(
                            "{}\n{tip}",
                            tr1(lang, "Used by the OS for \"{}\".", tr(lang, r))
                        );
                    }
                    if ui.button(text).on_hover_text(tip).clicked() {
                        keymap.unassign(action, k);
                        self.notice = None;
                        changed = true;
                    }
                }
                if self.capturing == Some(action) {
                    let waiting = ui.add(
                        egui::Button::new(tr(lang, "Press a key... (Esc to cancel)"))
                            .selected(true),
                    );
                    if waiting.clicked() {
                        self.capturing = None;
                    }
                } else if ui
                    .button(icon::PLUS)
                    .on_hover_text(tr(lang, "Add key"))
                    .clicked()
                {
                    self.capturing = Some(action);
                    self.notice = None;
                }
                let reset = ui
                    .add_enabled(
                        !keymap.is_default(action),
                        egui::Button::new(icon::ARROW_COUNTER_CLOCKWISE),
                    )
                    .on_hover_text(tr(lang, "Reset to default"));
                if reset.clicked() {
                    keymap.reset(action);
                    self.notice = None;
                    changed = true;
                }
            });
            if let Some((a, notice)) = self.notice {
                if a == action {
                    let (color, text) = notice_text(ui, notice, lang);
                    ui.colored_label(color, text);
                }
            }
        });
        changed
    }
}

fn notice_text(ui: &egui::Ui, notice: Notice, lang: Language) -> (Color32, String) {
    let v = ui.visuals();
    match notice {
        Notice::Refused(Refusal::UsedBy(other)) => (
            v.error_fg_color,
            tr1(
                lang,
                "Already used by \"{}\". Remove it there first.",
                tr(lang, other.title()),
            ),
        ),
        Notice::Refused(Refusal::AlreadyAssigned) => (
            v.warn_fg_color,
            tr(lang, "Already assigned to this action.").to_owned(),
        ),
        Notice::OsReserved(what) => (
            v.warn_fg_color,
            tr1(
                lang,
                "Assigned. Note: the OS usually uses this key for \"{}\", so it may not reach vw.",
                tr(lang, what),
            ),
        ),
        Notice::Clipboard => (
            v.error_fg_color,
            tr(
                lang,
                "Clipboard keys (Ctrl+C, Ctrl+X, Ctrl+V) are reserved by the OS.",
            )
            .to_owned(),
        ),
    }
}

/// 項目の枠。ホバーで左端に細線を出して、どの項目を触っているかを示す。
fn item_frame(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let r = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        })
        .response;
    if ui.rect_contains_pointer(r.rect) {
        let x = r.rect.left() + 0.5;
        ui.painter().vline(
            x,
            r.rect.y_range(),
            egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
        );
    }
}

fn title_line(ui: &mut egui::Ui, title: &str, key: &str) {
    ui.horizontal(|ui| {
        ui.strong(title);
        ui.weak(RichText::new(key).small().monospace());
    });
}

/// キー入力待ちの結果。
#[derive(Debug, PartialEq, Eq)]
enum Capture {
    /// まだキーが押されていない。
    Waiting,
    Cancelled,
    /// 割り当てた。
    Done,
    /// 割り当てられなかった。
    Rejected,
}

/// キー入力待ちの `action` に、`events` の最初のキーを割り当てる。
/// Esc（修飾キーなし）は取り消し。修飾キー単独の押下は読み飛ばし、続くキーを待つ。
fn capture(
    action: Action,
    events: &[Event],
    keymap: &mut Keymap,
    os: Os,
) -> (Capture, Option<Notice>) {
    for e in events {
        match e {
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if !is_modifier(*key) => {
                let combo = KeyCombo::from_event(*key, *modifiers);
                if combo == KeyCombo::plain(egui::Key::Escape) {
                    return (Capture::Cancelled, None);
                }
                return match keymap.assign(action, combo) {
                    Ok(()) => (
                        Capture::Done,
                        os_reserved(os, combo).map(Notice::OsReserved),
                    ),
                    Err(r) => (Capture::Rejected, Some(Notice::Refused(r))),
                };
            }
            Event::Copy | Event::Cut | Event::Paste(_) => {
                return (Capture::Rejected, Some(Notice::Clipboard));
            }
            _ => {}
        }
    }
    (Capture::Waiting, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Key, Modifiers};

    fn press(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(matches("", &["Theme"]));
        assert!(matches("   ", &[]));
    }

    #[test]
    fn query_is_case_insensitive() {
        assert!(matches("THEME", &["appearance.theme"]));
    }

    #[test]
    fn every_word_must_be_found_somewhere() {
        let texts = ["Theme", "Follow the OS dark/light setting"];
        assert!(matches("theme dark", &texts));
        assert!(!matches("theme zoom", &texts));
    }

    #[test]
    fn japanese_text_is_searchable() {
        assert!(matches("テーマ", &["Theme", "テーマ"]));
    }

    #[test]
    fn every_setting_has_japanese_title_and_description() {
        for i in &ITEMS {
            assert_ne!(tr(Language::Japanese, i.title), i.title);
            assert_ne!(tr(Language::Japanese, i.description), i.description);
        }
        for s in Section::ALL {
            assert_ne!(tr(Language::Japanese, s.title()), s.title());
        }
    }

    #[test]
    fn setting_keys_are_unique() {
        for (n, i) in ITEMS.iter().enumerate() {
            assert!(!ITEMS[..n].iter().any(|j| j.key == i.key), "{}", i.key);
        }
    }

    #[test]
    fn setting_keys_point_at_real_toml_entries() {
        let toml = Settings::default().to_toml();
        for i in &ITEMS {
            let (table, field) = i.key.split_once('.').unwrap();
            assert!(toml.contains(&format!("[{table}]")), "{}", i.key);
            assert!(toml.contains(&format!("{field} = ")), "{}", i.key);
        }
    }

    #[test]
    fn capture_waits_until_a_key_arrives() {
        let mut map = Keymap::default();
        let (c, n) = capture(
            Action::NewSlot,
            &[Event::Text("x".into())],
            &mut map,
            Os::Linux,
        );
        assert_eq!((c, n), (Capture::Waiting, None));
    }

    #[test]
    fn captured_key_is_assigned() {
        let mut map = Keymap::default();
        let (c, n) = capture(
            Action::NewSlot,
            &[press(Key::Insert, Modifiers::NONE)],
            &mut map,
            Os::Linux,
        );
        assert_eq!((c, n), (Capture::Done, None));
        assert!(map
            .keys(Action::NewSlot)
            .contains(&KeyCombo::plain(Key::Insert)));
    }

    #[test]
    fn modifiers_are_captured_with_the_key() {
        let mut map = Keymap::default();
        let mods = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };
        capture(Action::NewSlot, &[press(Key::O, mods)], &mut map, Os::Linux);
        assert!(map
            .keys(Action::NewSlot)
            .contains(&"Ctrl+Shift+O".parse().unwrap()));
    }

    #[test]
    fn pressing_only_a_modifier_keeps_waiting() {
        let mut map = Keymap::default();
        let events = [
            press(Key::ControlLeft, Modifiers::CTRL),
            press(Key::AltLeft, Modifiers::ALT),
        ];
        let (c, n) = capture(Action::NewSlot, &events, &mut map, Os::Linux);
        assert_eq!((c, n), (Capture::Waiting, None));
    }

    #[test]
    fn modifier_then_key_assigns_the_combination() {
        let mut map = Keymap::default();
        let events = [
            press(Key::AltLeft, Modifiers::ALT),
            press(Key::F4, Modifiers::ALT),
        ];
        let (c, n) = capture(Action::NewSlot, &events, &mut map, Os::Linux);
        assert_eq!(
            (c, n),
            (Capture::Done, Some(Notice::OsReserved("Close window")))
        );
    }

    #[test]
    fn escape_cancels_without_assigning() {
        let mut map = Keymap::default();
        let (c, _) = capture(
            Action::NewSlot,
            &[press(Key::Escape, Modifiers::NONE)],
            &mut map,
            Os::Linux,
        );
        assert_eq!(c, Capture::Cancelled);
        assert_eq!(map, Keymap::default());
    }

    #[test]
    fn escape_with_modifier_can_be_assigned() {
        let mut map = Keymap::default();
        let (c, _) = capture(
            Action::NewSlot,
            &[press(Key::Escape, Modifiers::SHIFT)],
            &mut map,
            Os::Linux,
        );
        assert_eq!(c, Capture::Done);
    }

    #[test]
    fn key_used_elsewhere_is_rejected_with_its_owner() {
        let mut map = Keymap::default();
        let (c, n) = capture(
            Action::NewSlot,
            &[press(Key::ArrowRight, Modifiers::NONE)],
            &mut map,
            Os::Linux,
        );
        assert_eq!(c, Capture::Rejected);
        assert_eq!(n, Some(Notice::Refused(Refusal::UsedBy(Action::NextImage))));
        assert_eq!(map, Keymap::default());
    }

    #[test]
    fn os_key_is_assigned_with_a_warning() {
        let mut map = Keymap::default();
        let (c, n) = capture(
            Action::NewSlot,
            &[press(Key::F4, Modifiers::ALT)],
            &mut map,
            Os::Windows,
        );
        assert_eq!(c, Capture::Done);
        assert_eq!(n, Some(Notice::OsReserved("Close window")));
    }

    #[test]
    fn clipboard_shortcut_cannot_be_assigned() {
        let mut map = Keymap::default();
        let (c, n) = capture(Action::NewSlot, &[Event::Copy], &mut map, Os::Linux);
        assert_eq!((c, n), (Capture::Rejected, Some(Notice::Clipboard)));
    }
}
