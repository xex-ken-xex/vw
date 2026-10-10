//! アプリ本体。ツール列と 4 つのフィールド（フォルダツリー・メタデータ・画像・ログ）の配置、
//! 設定画面への切り替え、ホットキーの実行、スロットの管理、読み込み結果の受け取りを行う。
//!
//! egui は即時モード GUI で、毎フレーム `ui()` が呼ばれて画面全体を組み立て直す。
//! 状態はすべて [`VwApp`] が持ち、ウィジェット側には持たせない。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2, Color32, Rect, TextureFilter, TextureOptions};
use egui_phosphor::regular as icon;
use vw_core::{ColorModel, Registry};

use crate::i18n::tr;
use crate::keymap::{Action, KeyCombo};
use crate::loader::{LoadResult, Loader};
use crate::log::{Level, Log};
use crate::settings::{parse_hex_color, Language, SettingsStore};
use crate::settings_ui::SettingsUi;
use crate::theme;
use crate::tree::{self, FolderTree, TreeAction};
use crate::viewer::{Shown, Slot};

/// 左端のツール列のツール。Browse 以外はまだ機能がなく、押せない状態で置いてある。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Browse,
    Pan,
    Zoom,
    PixelProbe,
    Measure,
}

impl Tool {
    const ALL: [Tool; 5] = [
        Tool::Browse,
        Tool::Pan,
        Tool::Zoom,
        Tool::PixelProbe,
        Tool::Measure,
    ];

    fn icon(self) -> &'static str {
        match self {
            Tool::Browse => icon::CURSOR,
            Tool::Pan => icon::HAND_GRABBING,
            Tool::Zoom => icon::MAGNIFYING_GLASS_PLUS,
            Tool::PixelProbe => icon::EYEDROPPER,
            Tool::Measure => icon::RULER,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Tool::Browse => "Browse",
            Tool::Pan => "Pan",
            Tool::Zoom => "Zoom",
            Tool::PixelProbe => "Pixel probe",
            Tool::Measure => "Measure",
        }
    }

    fn available(self) -> bool {
        self == Tool::Browse
    }
}

/// ツール列のボタン 1 つの大きさ（正方形の一辺）。
const TOOL_BUTTON: f32 = 26.0;
/// ツール列の左右の余白。
const TOOL_PADDING: i8 = 1;

/// 4 つのフィールドそれぞれの表示・非表示（View メニュー）。
struct Fields {
    tree: bool,
    metadata: bool,
    images: bool,
    log: bool,
}

pub struct VwApp {
    loader: Loader,
    tree: FolderTree,
    /// 画像フィールドの枠。常に 1 つ以上ある。
    slots: Vec<Slot>,
    /// 操作対象のスロットの添字。メタデータ表示とツリーのクリックはこのスロットに対して働く。
    active: usize,
    /// スロット番号と読み込み要求番号の両方に使う連番。
    next_id: u64,
    fields: Fields,
    tool: Tool,
    settings: SettingsStore,
    settings_ui: SettingsUi,
    /// 設定画面を開いている間は、ツール列とメニュー以外をすべて設定画面に置き換える。
    show_settings: bool,
    log: Log,
    /// プロセス起動時刻。最初のフレームまでの時間をログに出すために持つ。
    started: Instant,
    first_frame: bool,
}

/// `n` 枚のスロットを並べるグリッドの (列数, 行数)。正方形に近く、横長を優先する。
pub fn grid_dims(n: usize) -> (usize, usize) {
    let n = n.max(1);
    let cols = (n as f64).sqrt().ceil() as usize;
    (cols, n.div_ceil(cols))
}

/// `len` 件の中で `pos` から `delta` 進んだ位置。端まで行ったら反対側に回り込む。
pub fn wrap_step(pos: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (pos as isize + delta).rem_euclid(len as isize) as usize
}

/// `removed` 番目のスロットを閉じた後の、アクティブなスロットの添字。
/// アクティブより前が閉じられたら 1 つ詰め、アクティブ自身なら同じ位置（なければ末尾）を選ぶ。
pub fn active_after_close(active: usize, removed: usize, remaining: usize) -> usize {
    let active = if removed < active { active - 1 } else { active };
    active.min(remaining.saturating_sub(1))
}

/// プラグインのメタデータをグループごとにまとめる。グループの順は最初に現れた順。
pub fn group_metadata(entries: &[(String, String, String)]) -> Vec<(&str, Vec<(&str, &str)>)> {
    let mut groups: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for (group, key, value) in entries {
        match groups.iter_mut().find(|(g, _)| g == group) {
            Some((_, rows)) => rows.push((key, value)),
            None => groups.push((group, vec![(key, value)])),
        }
    }
    groups
}

impl VwApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        registry: Registry,
        initial: Vec<PathBuf>,
        started: Instant,
    ) -> Self {
        crate::fonts::install_icons(&cc.egui_ctx);
        crate::fonts::load_cjk_in_background(&cc.egui_ctx);
        let mut log = Log::new(started);
        let (settings, notes) = SettingsStore::open(SettingsStore::default_path());
        for n in notes {
            log.warn(format!("Settings: {n}"));
        }
        for (k, actions) in settings.settings.keys.duplicates() {
            log.warn(format!(
                "Settings: {k} is assigned to several actions; using {}",
                actions[0].id()
            ));
        }
        theme::apply(&cc.egui_ctx, &settings.settings.appearance);
        log.info(format!(
            "Plugins: {}",
            registry
                .plugins()
                .iter()
                .map(|p| p.name())
                .collect::<Vec<_>>()
                .join(", ")
        ));

        // 引数のフォルダはツリーの先頭に開いた状態で置き、ファイルはそれぞれ別のスロットで開く。
        // ファイルだけが渡されたときは、その親フォルダをツリーに出して前後の画像を辿りやすくする。
        let (dirs, files): (Vec<PathBuf>, Vec<PathBuf>) =
            initial.into_iter().partition(|p| p.is_dir());
        let root = dirs.into_iter().next().or_else(|| {
            files
                .first()
                .and_then(|f| f.parent().map(Path::to_path_buf))
        });

        let mut app = Self {
            loader: Loader::new(registry, cc.egui_ctx.clone()),
            tree: FolderTree::new(root),
            slots: vec![Slot::new(0)],
            active: 0,
            next_id: 1,
            fields: Fields {
                tree: true,
                metadata: true,
                images: true,
                log: true,
            },
            tool: Tool::Browse,
            settings,
            settings_ui: SettingsUi::default(),
            show_settings: false,
            log,
            started,
            first_frame: true,
        };
        for (i, f) in files.into_iter().enumerate() {
            if i > 0 {
                app.add_slot();
            }
            app.open_in_active(f);
        }
        app
    }

    /// 空のスロットを末尾に足し、アクティブにする。
    fn add_slot(&mut self) {
        self.slots.push(Slot::new(self.next_id));
        self.next_id += 1;
        self.active = self.slots.len() - 1;
    }

    /// スロットを閉じる。最後の 1 つを閉じたときは空のスロットを残す。
    fn close_slot(&mut self, index: usize) {
        self.slots.remove(index);
        if self.slots.is_empty() {
            self.add_slot();
            return;
        }
        self.active = active_after_close(self.active, index, self.slots.len());
    }

    /// アクティブなスロットで `path` を読み込み始める。結果は [`Self::receive`] で受け取る。
    /// 前の画像は新しい画像が届くまで表示したままにして、切り替え時のちらつきを避ける。
    fn open_in_active(&mut self, path: PathBuf) {
        let request = self.next_id;
        self.next_id += 1;
        let slot = &mut self.slots[self.active];
        slot.path = Some(path.clone());
        slot.request = request;
        slot.loading = true;
        slot.error = None;
        self.loader.request(slot.id, request, path);
    }

    /// アクティブなスロットの画像と同じフォルダにある、前後の対応画像に切り替える。
    ///
    /// 毎回フォルダを読み直す。一覧をキャッシュしないのは、表示中にファイルが
    /// 追加・削除されることがあり、数千件程度なら読み直しても体感できる遅れにならないため。
    fn step_active(&mut self, delta: isize) {
        let Some(path) = self.slots[self.active].path.clone() else {
            return;
        };
        let files = tree::sibling_images(&path, self.loader.registry());
        let Some(pos) = files.iter().position(|p| p == &path) else {
            return;
        };
        let next = wrap_step(pos, files.len(), delta);
        if next != pos {
            self.open_in_active(files[next].clone());
        }
    }

    fn lang(&self) -> Language {
        self.settings.settings.appearance.language
    }

    /// 画像の背景色。設定が空か読めない形式ならテーマの既定色。
    fn canvas_color(&self, ctx: &egui::Context) -> Color32 {
        parse_hex_color(&self.settings.settings.viewer.background)
            .unwrap_or_else(|| theme::canvas_color(ctx.theme()))
    }

    /// 操作の最初のキーをツールチップ用に「名前 (キー)」の形にする。
    fn with_key(&self, title: &str, action: Action) -> String {
        match self.shortcut(action) {
            Some(k) => format!("{title} ({k})"),
            None => title.to_owned(),
        }
    }

    fn shortcut(&self, action: Action) -> Option<String> {
        self.settings
            .settings
            .keys
            .keys(action)
            .first()
            .map(KeyCombo::to_string)
    }

    fn toggle_settings(&mut self) {
        self.show_settings = !self.show_settings;
        if self.show_settings {
            self.settings_ui.opened();
        }
    }

    fn run(&mut self, action: Action) {
        match action {
            Action::PreviousImage => self.step_active(-1),
            Action::NextImage => self.step_active(1),
            Action::NewSlot => self.add_slot(),
            Action::CloseSlot => self.close_slot(self.active),
            Action::ToggleFolderTree => self.fields.tree ^= true,
            Action::ToggleMetadata => self.fields.metadata ^= true,
            Action::ToggleImages => self.fields.images ^= true,
            Action::ToggleLog => self.fields.log ^= true,
            Action::ToggleSettings => self.toggle_settings(),
        }
    }

    /// このフレームのキー入力に割り当てられた操作を実行する。
    fn run_hotkeys(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let keys = &self.settings.settings.keys;
        // テキスト入力中は、文字入力と区別できる Ctrl / Alt 付きのキーだけを実行する。
        // 全部止めると、設定画面の検索欄にいるときに Ctrl+, で設定を閉じられない。
        let events: Vec<egui::Event> = if ctx.egui_wants_keyboard_input() {
            events
                .into_iter()
                .filter(|e| matches!(e, egui::Event::Key { modifiers, .. } if modifiers.ctrl || modifiers.alt))
                .collect()
        } else {
            events
        };
        for action in keys.actions_in(&events) {
            // 設定画面の間は画像やパネルの操作をしない。見えていない画面が変わるのを避けるため。
            if self.show_settings && action != Action::ToggleSettings {
                continue;
            }
            self.run(action);
        }
    }

    /// 設定画面で値が変わったときに、反映して保存する。
    fn settings_changed(&mut self, ctx: &egui::Context) {
        theme::apply(ctx, &self.settings.settings.appearance);
        if let Err(e) = self.settings.save() {
            self.log.error(format!("Settings: {e}"));
        }
    }

    /// 設定ファイルが外部のエディタで書き換えられていたら読み直して反映する。
    fn reload_settings(&mut self, ctx: &egui::Context) {
        match self.settings.reload_if_changed() {
            None => {}
            Some(Ok(problems)) => {
                self.log.info("Settings reloaded from file");
                for p in problems {
                    self.log.warn(format!("Settings: {p}"));
                }
                theme::apply(ctx, &self.settings.settings.appearance);
            }
            Some(Err(e)) => self.log.error(format!("Settings not reloaded: {e}")),
        }
    }

    /// 左端の、アイコンが縦に並ぶ固定幅の列。
    fn tool_column(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let big = |s: &str| egui::RichText::new(s).size(17.0);
        let size = vec2(TOOL_BUTTON, TOOL_BUTTON);
        ui.vertical_centered(|ui| {
            ui.add_space(4.0);
            for tool in Tool::ALL {
                let button =
                    egui::Button::selectable(self.tool == tool, big(tool.icon())).min_size(size);
                let r = ui
                    .add_enabled(tool.available(), button)
                    .on_hover_text(tr(lang, tool.title()))
                    .on_disabled_hover_text(format!(
                        "{}\n{}",
                        tr(lang, tool.title()),
                        tr(lang, "Not available yet")
                    ));
                if r.clicked() {
                    self.tool = tool;
                }
            }
            ui.separator();
            let tip = self.with_key(tr(lang, "New slot"), Action::NewSlot);
            if ui
                .add(egui::Button::new(big(icon::PLUS_SQUARE)).min_size(size))
                .on_hover_text(tip)
                .clicked()
            {
                self.add_slot();
            }
        });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            let tip = self.with_key(tr(lang, "Settings"), Action::ToggleSettings);
            let button =
                egui::Button::selectable(self.show_settings, big(icon::GEAR_SIX)).min_size(size);
            if ui.add(button).on_hover_text(tip).clicked() {
                self.toggle_settings();
            }
        });
    }

    /// 読み込みスレッドからの結果をすべて取り出し、テクスチャにしてスロットへ渡す。
    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(LoadResult {
            slot,
            request,
            path,
            result,
        }) = self.loader.rx.try_recv()
        {
            // 読み込み中にスロットが閉じられた、または別の画像に切り替えられた場合は捨てる。
            let Some(s) = self
                .slots
                .iter_mut()
                .find(|s| s.id == slot && s.request == request)
            else {
                continue;
            };
            s.loading = false;
            match result {
                Ok(loaded) => {
                    let t = Instant::now();
                    // 拡大時は最近傍（画素の境界をそのまま見せる）、縮小時は線形＋ミップマップ。
                    // 拡大まで線形にすると、画素単位で確認したい医用画像などがぼやけるため。
                    let texture = ctx.load_texture(
                        path.to_string_lossy(),
                        loaded.display,
                        TextureOptions {
                            magnification: TextureFilter::Nearest,
                            minification: TextureFilter::Linear,
                            mipmap_mode: Some(TextureFilter::Linear),
                            ..TextureOptions::LINEAR
                        },
                    );
                    let i = &loaded.info;
                    self.log.info(format!(
                        "{}: {}x{} {} | read {} / decode {} / convert {} / texture {}",
                        path.display(),
                        i.image.width,
                        i.image.height,
                        i.plugin,
                        ms(i.read),
                        ms(i.decode),
                        ms(i.convert),
                        ms(t.elapsed()),
                    ));
                    if texture.size()[0] < i.image.width as usize {
                        self.log.warn(format!(
                            "{}: shown downsampled to {}x{} (texture size limit)",
                            path.display(),
                            texture.size()[0],
                            texture.size()[1]
                        ));
                    }
                    s.shown = Some(Shown {
                        texture,
                        info: loaded.info,
                    });
                    s.error = None;
                    s.reset_view();
                }
                Err(e) => {
                    self.log.error(format!("{}: {e}", path.display()));
                    s.shown = None;
                    s.error = Some(e);
                }
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(tr(lang, "File"), |ui| {
                let new_slot = egui::Button::new(tr(lang, "New slot"))
                    .shortcut_text(self.shortcut(Action::NewSlot).unwrap_or_default());
                if ui.add(new_slot).clicked() {
                    self.add_slot();
                }
                if ui.button(tr(lang, "Close all slots")).clicked() {
                    self.slots.clear();
                    self.add_slot();
                }
                ui.separator();
                if ui.button(tr(lang, "Quit")).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(tr(lang, "View"), |ui| {
                ui.checkbox(&mut self.fields.tree, tr(lang, "Folder tree"));
                ui.checkbox(&mut self.fields.metadata, tr(lang, "Metadata"));
                ui.checkbox(&mut self.fields.images, tr(lang, "Images"));
                ui.checkbox(&mut self.fields.log, tr(lang, "Log"));
                ui.separator();
                let settings = egui::Button::new(tr(lang, "Settings"))
                    .shortcut_text(self.shortcut(Action::ToggleSettings).unwrap_or_default());
                if ui.add(settings).clicked() {
                    self.toggle_settings();
                }
            });
        });
    }

    /// アクティブなスロットの画像について、ファイル情報とプラグインのメタデータを表で出す。
    fn metadata_ui(&self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let slot = &self.slots[self.active];
        let Some(shown) = &slot.shown else {
            ui.weak(tr(lang, "No image"));
            return;
        };
        let info = &shown.info;
        let img = &info.image;
        let file_rows: Vec<(String, String)> = vec![
            (
                tr(lang, "Path").into(),
                slot.path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
            (tr(lang, "Size").into(), format!("{} bytes", info.file_size)),
            (tr(lang, "Plugin").into(), info.plugin.clone()),
            (
                tr(lang, "Pixels").into(),
                format!("{} x {}", img.width, img.height),
            ),
            (tr(lang, "Color").into(), color_name(img.color).into()),
            (
                tr(lang, "Sample").into(),
                format!(
                    "{} bit ({} significant)",
                    img.data.bits_per_sample(),
                    img.significant_bits
                ),
            ),
            (
                tr(lang, "Texture").into(),
                format!("{} x {}", shown.texture.size()[0], shown.texture.size()[1]),
            ),
        ];
        let mut groups: Vec<(&str, Vec<(&str, &str)>)> = vec![(
            tr(lang, "File"),
            file_rows
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
        )];
        groups.extend(group_metadata(&img.metadata.entries));
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (group, rows) in groups {
                    egui::CollapsingHeader::new(group)
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::Grid::new(group)
                                .num_columns(2)
                                .striped(true)
                                .show(ui, |ui| {
                                    for (k, v) in rows {
                                        ui.label(k);
                                        ui.add(egui::Label::new(v).wrap());
                                        ui.end_row();
                                    }
                                });
                        });
                }
            });
    }

    /// ログを新しいものが下に来る順で出し、末尾に自動で追従する。
    fn log_ui(&self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (t, level, msg) in &self.log.lines {
                    let color = match level {
                        Level::Info => ui.visuals().text_color(),
                        Level::Warn => ui.visuals().warn_fg_color,
                        Level::Error => ui.visuals().error_fg_color,
                    };
                    ui.label(
                        egui::RichText::new(format!("[{t:8.3}] {msg}"))
                            .monospace()
                            .color(color),
                    );
                }
            });
    }

    /// スロットをグリッドに並べて描く。
    ///
    /// egui の Grid ウィジェットを使わず矩形を自分で割り当てるのは、Grid が中身の大きさに
    /// 合わせて伸び縮みするため。画像の大きさに関係なく枠を等分したい。
    fn images_ui(&mut self, ui: &mut egui::Ui) {
        let area = ui.available_rect_before_wrap();
        let n = self.slots.len();
        let (cols, rows) = grid_dims(n);
        let cell = vec2(area.width() / cols as f32, area.height() / rows as f32);
        let mut close = None;
        let background = self.canvas_color(ui.ctx());
        let lang = self.lang();
        for i in 0..n {
            let (c, r) = (i % cols, i / cols);
            let rect = Rect::from_min_size(
                pos2(
                    area.min.x + c as f32 * cell.x,
                    area.min.y + r as f32 * cell.y,
                ),
                cell,
            );
            let resp = self.slots[i].ui(ui, rect.shrink(1.0), i == self.active, background, lang);
            if resp.clicked {
                self.active = i;
            }
            if resp.close {
                close = Some(i);
            }
        }
        // 描画ループの途中で削除すると添字がずれるので、ループの後で閉じる。
        if let Some(i) = close {
            self.close_slot(i);
        }
        ui.allocate_rect(area, egui::Sense::hover());
    }
}

impl eframe::App for VwApp {
    /// 毎フレーム呼ばれる。パネルは外側から順に追加する必要があるので、
    /// メニュー → ツール列（左端）→ ログ（下）→ ツリー（左）→ メタデータ（右）→ 画像（中央）の順に置く。
    /// 設定画面を開いているときは、ツール列より内側をすべて設定画面にする。
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.first_frame {
            self.first_frame = false;
            self.log.info(format!(
                "Startup to first frame: {}",
                ms(self.started.elapsed())
            ));
        }
        self.receive(&ctx);
        self.reload_settings(&ctx);

        // キー入力待ちのときは、押されたキーを割り当てに使い切る（ホットキーとして実行しない）。
        if self
            .settings_ui
            .capture_keys(&ctx, &mut self.settings.settings.keys)
        {
            self.settings_changed(&ctx);
        }
        self.run_hotkeys(&ctx);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        // パネル既定の枠は左右に 8px の余白を取り、ボタンより内側の幅が狭くなって
        // ボタンが片側にはみ出す。余白を自分で決めて、列の幅をボタン + 両側の余白にする。
        let tool_frame = egui::Frame::side_top_panel(ui.style())
            .inner_margin(egui::Margin::symmetric(TOOL_PADDING, 2));
        egui::Panel::left("tools")
            .frame(tool_frame)
            // 右端の 1px はパネルの区切り線が使うので、その分を足す。
            .exact_size(TOOL_BUTTON + 2.0 * f32::from(TOOL_PADDING) + 1.0)
            .resizable(false)
            .show(ui, |ui| self.tool_column(ui));

        if self.show_settings {
            let path = self.settings.path.as_ref().map(|p| p.display().to_string());
            let exists = self.settings.path.as_ref().is_some_and(|p| p.exists());
            let r = self
                .settings_ui
                .ui(ui, &mut self.settings.settings, path.as_deref(), exists);
            if r.changed {
                self.settings_changed(&ctx);
            }
            if r.open_file {
                if let Err(e) = self.settings.open_in_editor() {
                    self.log.error(format!("Settings: cannot open file: {e}"));
                }
            }
            if r.close {
                self.toggle_settings();
            }
            // 外部のエディタでの書き換えを、ウィンドウに触れなくても拾えるようにする。
            ctx.request_repaint_after(Duration::from_secs(1));
            return;
        }

        let lang = self.lang();
        if self.fields.log {
            egui::Panel::bottom("log")
                .resizable(true)
                .default_size(140.0)
                .show(ui, |ui| {
                    panel_header(ui, tr(lang, "Log"));
                    self.log_ui(ui);
                });
        }
        if self.fields.tree {
            egui::Panel::left("tree")
                .default_size(260.0)
                .show(ui, |ui| {
                    panel_header(ui, tr(lang, "Folders"));
                    let selected = self.slots[self.active].path.clone();
                    let action = egui::ScrollArea::both()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            self.tree
                                .ui(ui, self.loader.registry(), selected.as_deref())
                        })
                        .inner;
                    match action {
                        Some(TreeAction::Open(p)) => self.open_in_active(p),
                        Some(TreeAction::Add(p)) => {
                            self.add_slot();
                            self.open_in_active(p);
                        }
                        None => {}
                    }
                });
        }
        if self.fields.metadata {
            egui::Panel::right("metadata")
                .default_size(300.0)
                .min_size(220.0)
                .show(ui, |ui| {
                    panel_header(ui, tr(lang, "Metadata"));
                    self.metadata_ui(ui);
                });
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if self.fields.images {
                self.images_ui(ui);
            } else {
                let background = self.canvas_color(ui.ctx());
                ui.painter().rect_filled(ui.max_rect(), 0.0, background);
            }
        });
    }
}

/// パネルの見出し。本文と区別できる小さな大文字にして、縦の領域を取りすぎないようにする。
fn panel_header(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text.to_uppercase()).small().weak());
}

/// ログ用のミリ秒表記。
fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

fn color_name(c: ColorModel) -> &'static str {
    match c {
        ColorModel::Gray => "Gray",
        ColorModel::GrayAlpha => "Gray + alpha",
        ColorModel::Rgb => "RGB",
        ColorModel::Rgba => "RGBA",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_is_close_to_square() {
        assert_eq!(grid_dims(1), (1, 1));
        assert_eq!(grid_dims(2), (2, 1));
        assert_eq!(grid_dims(3), (2, 2));
        assert_eq!(grid_dims(4), (2, 2));
        assert_eq!(grid_dims(5), (3, 2));
        assert_eq!(grid_dims(9), (3, 3));
        assert_eq!(grid_dims(10), (4, 3));
    }

    #[test]
    fn grid_always_has_room_for_every_slot() {
        for n in 1..=50 {
            let (c, r) = grid_dims(n);
            assert!(c * r >= n, "{n}: {c}x{r}");
            assert!(c * (r - 1) < n, "{n}: an empty row in {c}x{r}");
        }
    }

    #[test]
    fn zero_slots_still_gives_one_cell() {
        assert_eq!(grid_dims(0), (1, 1));
    }

    #[test]
    fn step_wraps_around_both_ends() {
        assert_eq!(wrap_step(0, 3, 1), 1);
        assert_eq!(wrap_step(2, 3, 1), 0);
        assert_eq!(wrap_step(0, 3, -1), 2);
        assert_eq!(wrap_step(0, 0, 1), 0);
    }

    #[test]
    fn closing_slot_before_active_keeps_same_slot_active() {
        // [a, b, c] で c がアクティブ。a を閉じると [b, c] になり、c は添字 1。
        assert_eq!(active_after_close(2, 0, 2), 1);
    }

    #[test]
    fn closing_active_slot_selects_next_or_last() {
        assert_eq!(active_after_close(1, 1, 2), 1);
        assert_eq!(active_after_close(2, 2, 2), 1);
    }

    #[test]
    fn closing_slot_after_active_keeps_index() {
        assert_eq!(active_after_close(0, 2, 2), 0);
    }

    #[test]
    fn metadata_is_grouped_in_first_seen_order() {
        let e = |g: &str, k: &str, v: &str| (g.to_string(), k.to_string(), v.to_string());
        let entries = vec![
            e("PNG", "Size", "1 x 1"),
            e("Text", "Title", "t"),
            e("PNG", "Bit depth", "8"),
        ];
        let groups = group_metadata(&entries);
        assert_eq!(
            groups,
            vec![
                ("PNG", vec![("Size", "1 x 1"), ("Bit depth", "8")]),
                ("Text", vec![("Title", "t")]),
            ]
        );
    }
}
