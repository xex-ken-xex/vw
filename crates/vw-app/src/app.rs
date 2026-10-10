//! アプリ本体。ツール列と 4 つのフィールド（フォルダツリー・メタデータ・画像・ログ）の配置、
//! 設定画面への切り替え、ホットキーの実行、スロットの管理、読み込み結果の受け取りを行う。
//!
//! egui は即時モード GUI で、毎フレーム `ui()` が呼ばれて画面全体を組み立て直す。
//! 状態はすべて [`VwApp`] が持ち、ウィジェット側には持たせない。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, collapsing_header::CollapsingState, pos2, text_selection::LabelSelectionState, vec2,
    Color32, Rect, TextureFilter, TextureOptions,
};
use egui_phosphor::regular as icon;
use vw_core::{ColorModel, Registry};

use crate::autoload::{FolderLoad, Question};
use crate::copy::{self, Focus, MetaGroup, MetaSelection};
use crate::i18n::{tr, tr1};
use crate::keymap::{Action, KeyCombo};
use crate::loader::{FullImage, Job, Loaded, Loader};
use crate::log::{Level, Log};
use crate::memory;
use crate::settings::{
    parse_hex_color, FolderAction, HeaderClick, Language, OnLoadFolder, SettingsStore, TreeMode,
};
use crate::settings_ui::SettingsUi;
use crate::state::{self, AppState, Monitor, Placement, WindowState};
use crate::theme;
use crate::tree::{self, FolderClicks, FolderTree, TreeAction};
use crate::viewer::{Full, Shown, Slot};

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
    /// 終了時の状態の保存先。設定ファイルの場所が決まらない環境では保存しない。
    state_path: Option<PathBuf>,
    /// 起動時に戻すウインドウ。戻し終えたら `None`。
    restoring: Option<Restore>,
    /// 最小化・最大化していないときの最新の位置と大きさ。終了時に保存する。
    window: Option<WindowState>,
    /// メタデータ欄に出しているスロットの番号。
    metadata_from: Option<u64>,
    /// Ctrl+C でコピーする欄。
    focus: Focus,
    /// ツリーで選んでいる項目。アクティブなスロットで画像を開いたときもその画像に移す。
    tree_selected: Option<PathBuf>,
    meta_selected: Option<MetaSelection>,
    /// コピーしたことを知らせる表示。
    toast: Option<Toast>,
    /// 描いたフレームの番号。フル解像度の画像を最後に使った時期の比較に使う。
    frame: u64,
    /// 画像フィールドの大きさ（物理ピクセル）。読み込むときにフル解像度が要りそうかの見積もりに使う。
    images_area: Option<egui::Vec2>,
    /// 画像自動展開モードで読み込み中のフォルダ。
    folder_load: Option<FolderLoad>,
}

/// コピーしたときにカーソルの近くに出す短い表示。
struct Toast {
    pos: egui::Pos2,
    until: Instant,
}

const TOAST_DURATION: Duration = Duration::from_millis(900);

/// 起動時のウインドウの復元の進み具合。
struct Restore {
    saved: WindowState,
    /// 位置を送ってから経過したフレーム数。
    frames: u32,
}

/// 位置を送った後、移動先のモニタの拡大率が届くのを待つ最大フレーム数。
/// 届かない環境（拡大率が変わらない移動、X11 など）でも大きさの復元を止めないための上限。
const RESTORE_WAIT_FRAMES: u32 = 10;

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

/// メタデータ欄の出し先を決めるのに要る、スロットの様子。
pub struct SlotView {
    pub id: u64,
    pub shown: bool,
    pub loading: bool,
}

/// メタデータ欄に出すスロットの添字。`previous` は直前に出していたスロットの番号。
/// アクティブなスロットに画像があればそれ、読み込み中なら直前のもの、どちらでもなければなし。
pub fn metadata_source(slots: &[SlotView], active: usize, previous: Option<u64>) -> Option<usize> {
    let a = &slots[active];
    if a.shown {
        return Some(active);
    }
    if !a.loading {
        return None;
    }
    slots.iter().position(|s| Some(s.id) == previous && s.shown)
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
            log.error(format!("Settings: {n}"));
        }
        let state_path = settings.path.as_deref().map(state::state_path);
        let (saved, notes) = match &state_path {
            Some(p) => AppState::load(p),
            None => (AppState::default(), Vec::new()),
        };
        for n in notes {
            log.error(format!("State: {n}"));
        }
        // ウインドウ作成後の今なら、Windows でも拡大率を反映した座標で取れる。
        let restoring = match state::placement(saved.window.as_ref(), &state::current_monitors()) {
            Placement::Default => None,
            Placement::Restore(saved) => Some(Restore { saved, frames: 0 }),
        };
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

        let mut tree = FolderTree::new(root, settings.settings.tree.order());
        tree.restore_open(&saved.open_folders, &registry);

        let mut app = Self {
            loader: Loader::new(registry, cc.egui_ctx.clone()),
            tree,
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
            state_path,
            restoring,
            window: None,
            metadata_from: None,
            focus: Focus::Images,
            tree_selected: None,
            meta_selected: None,
            toast: None,
            frame: 0,
            images_area: None,
            folder_load: None,
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

    /// すべてのスロットを閉じ、空のスロットを 1 つ残す。
    fn close_all_slots(&mut self) {
        for s in std::mem::take(&mut self.slots) {
            self.cancel_requests(&s);
        }
        self.add_slot();
    }

    /// スロットの読み込み中の要求を取り消す。待ち行列に残ったまま、閉じたスロットのために
    /// デコードするのを避けるため。
    fn cancel_requests(&self, slot: &Slot) {
        if slot.loading {
            self.loader.cancel(slot.request);
        }
        if let Some((request, _)) = slot.full_request {
            self.loader.cancel(request);
        }
    }

    /// スロットを閉じる。最後の 1 つを閉じたときは空のスロットを残す。
    fn close_slot(&mut self, index: usize) {
        let slot = self.slots.remove(index);
        self.cancel_requests(&slot);
        if self.slots.is_empty() {
            self.add_slot();
            return;
        }
        self.active = active_after_close(self.active, index, self.slots.len());
    }

    /// アクティブなスロットで `path` を読み込み始める。結果は [`Self::receive`] で受け取る。
    /// 前の画像は新しい画像が届くまで表示したままにして、切り替え時のちらつきを避ける。
    fn open_in_active(&mut self, path: PathBuf) {
        self.tree_selected = Some(path.clone());
        let full = self.cell_needs_full();
        self.start_load(self.active, path, full);
    }

    /// `index` 番目のスロットで `path` を読み込み始める。
    fn start_load(&mut self, index: usize, path: PathBuf, full: bool) {
        let request = self.next_id;
        self.next_id += 1;
        self.cancel_requests(&self.slots[index]);
        let slot = &mut self.slots[index];
        slot.path = Some(path.clone());
        slot.request = request;
        slot.loading = true;
        slot.error = None;
        slot.full_request = None;
        slot.copy_when_full = false;
        self.loader.request(Job {
            slot: slot.id,
            request,
            path,
            thumb: true,
            full,
        });
    }

    /// フォルダの画像をスロットに読み込み始める（画像自動展開モード）。
    /// 前の読み込みが続いていれば、そこで打ち切る。
    fn load_folder(&mut self, ctx: &egui::Context, path: PathBuf) {
        let tree = self.settings.settings.tree.clone();
        if tree.on_load_other_folder == OnLoadFolder::Replace {
            self.close_all_slots();
        }
        self.log.info(format!("Loading folder {}", path.display()));
        let ctx = ctx.clone();
        self.folder_load = Some(FolderLoad::start(
            path,
            self.loader.registry().clone(),
            tree.order(),
            tree.confirm_every,
            tree.confirm_chunk as usize,
            move || ctx.request_repaint(),
        ));
    }

    /// 読み込み中のフォルダで見つかった画像を、末尾のスロットに足していく。
    fn pump_folder_load(&mut self) {
        let Some(load) = &mut self.folder_load else {
            return;
        };
        let paths = load.take_ready();
        let finished = load.finished();
        if !paths.is_empty() {
            // 先にスロットを全部足してから枠の大きさを見積もる。1 枚ずつ見積もると、
            // スロットが少ない最初のうちだけフル解像度を読んでしまうため。
            let last = &self.slots[self.slots.len() - 1];
            let mut reuse_last = last.path.is_none() && !last.loading;
            let mut targets = Vec::with_capacity(paths.len());
            for path in paths {
                if !std::mem::take(&mut reuse_last) {
                    self.slots.push(Slot::new(self.next_id));
                    self.next_id += 1;
                }
                targets.push((self.slots.len() - 1, path));
            }
            let full = self.cell_needs_full();
            for (index, path) in targets {
                self.start_load(index, path, full);
            }
        }
        if finished {
            self.finish_folder_load();
        }
    }

    fn finish_folder_load(&mut self) {
        if let Some(load) = self.folder_load.take() {
            self.log.info(format!(
                "Loaded {} images from {}",
                load.loaded,
                load.root.display()
            ));
        }
    }

    /// 読み込み中のフォルダの確認（サブフォルダも読むか、次の区切りまで続けるか）。
    fn folder_load_dialog(&mut self, ctx: &egui::Context) {
        let Some(load) = &self.folder_load else {
            return;
        };
        let Some(question) = load.question() else {
            return;
        };
        let lang = self.lang();
        let name = load.root.file_name().map_or_else(
            || load.root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let chunk = self.settings.settings.tree.confirm_chunk;
        let (message, yes, no) = match question {
            Question::Subfolders => (
                tr1(
                    lang,
                    "\"{}\" has subfolders. Load the images in all of them too?",
                    &name,
                ),
                tr(lang, "Load subfolders"),
                tr(lang, "This folder only"),
            ),
            Question::More => (
                tr1(
                    lang,
                    "{} images loaded. Load the next {}?",
                    &load.loaded.to_string(),
                )
                .replacen("{}", &chunk.to_string(), 1),
                tr(lang, "Load more"),
                tr(lang, "Stop"),
            ),
        };
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("folder_load_question")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.label(message);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(yes).clicked() {
                    answer = Some(true);
                }
                if ui.button(no).clicked() {
                    answer = Some(false);
                }
            });
        });
        // Esc や枠の外のクリックは、続けない方の答えとして扱う。
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        let Some(yes) = answer else {
            return;
        };
        let Some(load) = &mut self.folder_load else {
            return;
        };
        match question {
            Question::Subfolders => load.answer_subfolders(yes),
            Question::More => {
                if !load.answer_more(yes) {
                    self.finish_folder_load();
                }
            }
        }
    }

    /// 今のスロット数で並べたとき、1 枠が縮小画像より大きいか。
    ///
    /// 大きければ、最初の読み込みでフル解像度も一緒に作る。縮小画像だけ先に読んで
    /// 後からフル解像度を読み直すと、1 枚を大きく見る普段の使い方でデコードが 2 回になるため。
    /// 画像の大きさは読むまで分からないので、枠の大きさだけで決める。
    fn cell_needs_full(&self) -> bool {
        let Some(area) = self.images_area else {
            return true;
        };
        let (cols, rows) = grid_dims(self.slots.len());
        let cell = (area.x / cols as f32).max(area.y / rows as f32);
        cell > memory::THUMB_SIDE as f32
    }

    /// アクティブなスロットの画像と同じフォルダにある、前後の対応画像に切り替える。
    ///
    /// 毎回フォルダを読み直す。一覧をキャッシュしないのは、表示中にファイルが
    /// 追加・削除されることがあり、数千件程度なら読み直しても体感できる遅れにならないため。
    fn step_active(&mut self, delta: isize) {
        let Some(path) = self.slots[self.active].path.clone() else {
            return;
        };
        let order = self.settings.settings.tree.order();
        let files = tree::sibling_images(&path, self.loader.registry(), order);
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

    fn run(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::PreviousImage => self.step_active(-1),
            Action::NextImage => self.step_active(1),
            Action::NewSlot => self.add_slot(),
            Action::CloseSlot => self.close_slot(self.active),
            Action::CloseAllSlots => {
                // 読み込み中のフォルダの残りが、閉じた後のスロットに入り続けないようにする。
                self.finish_folder_load();
                self.close_all_slots();
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
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
            self.run(ctx, action);
        }
    }

    /// 設定画面で値が変わったときに、反映して保存する。
    fn settings_changed(&mut self, ctx: &egui::Context) {
        theme::apply(ctx, &self.settings.settings.appearance);
        self.tree.set_order(self.settings.settings.tree.order());
        match self.settings.save() {
            Ok(Some(backup)) => self.log.info(format!(
                "Settings: the previous file had errors and was kept as {}",
                backup.display()
            )),
            Ok(None) => {}
            Err(e) => self.log.error(format!("Settings: {e}")),
        }
    }

    /// 設定ファイルが外部のエディタで書き換えられていたら読み直して反映する。
    fn reload_settings(&mut self, ctx: &egui::Context) {
        match self.settings.reload_if_changed() {
            None => {}
            Some(Ok(problems)) => {
                self.log.info("Settings reloaded from file");
                for p in problems {
                    self.log.error(format!("Settings: {p}"));
                }
                theme::apply(ctx, &self.settings.settings.appearance);
                self.tree.set_order(self.settings.settings.tree.order());
            }
            Some(Err(e)) => self.log.error(format!("Settings not reloaded: {e}")),
        }
    }

    /// 前回のウインドウの位置と大きさに戻す。
    ///
    /// 位置を送った後、移動先のモニタの拡大率がウインドウに反映されるのを待ってから大きさを送る。
    /// 同じフレームで送ると、移動前のモニタの拡大率で大きさが換算され、拡大率の違うモニタに
    /// 戻すときに大きさがずれるため。
    fn restore_window(&mut self, ctx: &egui::Context) {
        let Some(r) = &mut self.restoring else {
            return;
        };
        let native = ctx.input(|i| i.viewport().native_pixels_per_point);
        let zoom = ctx.zoom_factor();
        if r.frames == 0 {
            if let Some([x, y]) = r.saved.position {
                let ppp = zoom * native.unwrap_or(1.0);
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(
                    x as f32 / ppp,
                    y as f32 / ppp,
                )));
            }
        }
        r.frames += 1;
        let arrived = native.is_some_and(|n| (n - r.saved.scale).abs() < 0.01);
        if !arrived && r.frames < RESTORE_WAIT_FRAMES {
            ctx.request_repaint();
            return;
        }
        let ppp = zoom * native.unwrap_or(r.saved.scale);
        let [w, h] = r.saved.size;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(
            w as f32 / ppp,
            h as f32 / ppp,
        )));
        if r.saved.maximized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        self.window = Some(r.saved.clone());
        self.restoring = None;
    }

    /// 最小化・最大化していないときのウインドウの位置と大きさを覚えておく。
    /// 最大化中は最大化していることだけを覚え、戻したときの大きさは最大化前のものを残す。
    fn track_window(&mut self, ctx: &egui::Context) {
        if self.restoring.is_some() {
            return;
        }
        let (info, content) = ctx.input(|i| (i.viewport().clone(), i.content_rect()));
        if info.minimized == Some(true) {
            return;
        }
        let Some(native) = info.native_pixels_per_point else {
            return;
        };
        if info.maximized == Some(true) {
            if let Some(w) = &mut self.window {
                w.maximized = true;
            }
            return;
        }
        let ppp = native * ctx.zoom_factor();
        let px = |v: f32| (v * ppp).round() as i32;
        // Wayland では位置が取れず内側の範囲も届かないので、描画領域の大きさで代える。
        let inner = info.inner_rect.unwrap_or(content);
        self.window = Some(WindowState {
            position: info.outer_rect.map(|r| [px(r.min.x), px(r.min.y)]),
            size: [px(inner.width()) as u32, px(inner.height()) as u32],
            scale: native,
            maximized: false,
            monitors: Vec::new(),
        });
    }

    /// 終了時の状態を保存する。モニタ構成は起動中に変わることがあるので、ここで取り直す。
    fn save_state(&mut self) {
        let Some(path) = &self.state_path else {
            return;
        };
        let monitors: Vec<Monitor> = state::current_monitors();
        let window = self.window.clone().map(|w| WindowState { monitors, ..w });
        let state = AppState {
            window,
            open_folders: self.tree.open_paths(),
        };
        // 終了処理中でログ欄はもう見えないので、標準エラーに出す。
        if let Err(e) = state.save(path) {
            eprintln!("vw: cannot save {}: {e}", path.display());
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
        while let Ok(r) = self.loader.rx.try_recv() {
            // 読み込み中にスロットが閉じられた、または別の画像に切り替えられた場合は捨てる。
            let Some(index) = self.slots.iter().position(|s| {
                s.id == r.slot
                    && (s.request == r.request
                        || s.full_request.is_some_and(|(q, _)| q == r.request))
            }) else {
                continue;
            };
            if self.slots[index].request == r.request {
                self.receive_image(ctx, index, r.path, r.result);
            } else {
                self.slots[index].full_request = None;
                match r.result {
                    Ok(loaded) => {
                        if let Some(full) = loaded.full {
                            self.attach_full(ctx, index, full);
                        }
                    }
                    Err(e) => self.log.error(format!("{}: {e}", r.path.display())),
                }
            }
        }
    }

    /// スロットで開いた画像の読み込み結果を受け取る。
    fn receive_image(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        path: PathBuf,
        result: Result<Loaded, String>,
    ) {
        let s = &mut self.slots[index];
        s.loading = false;
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(e) => {
                self.log.error(format!("{}: {e}", path.display()));
                s.shown = None;
                s.error = Some(e);
                return;
            }
        };
        let t = Instant::now();
        let thumb = loaded
            .thumb
            .expect("the first load always asks for a thumbnail");
        let thumb = ctx.load_texture(
            format!("{}#thumb", path.display()),
            thumb,
            texture_options(),
        );
        let i = &loaded.info;
        self.log.info(format!(
            "{}: {}x{} {} | read {} / decode {} / convert {} / texture {}",
            path.display(),
            i.width,
            i.height,
            i.plugin,
            ms(i.read),
            ms(i.decode),
            ms(i.convert),
            ms(t.elapsed()),
        ));
        s.shown = Some(Shown {
            path,
            thumb,
            info: loaded.info,
            full: None,
        });
        s.error = None;
        s.reset_view();
        if let Some(full) = loaded.full {
            self.attach_full(ctx, index, full);
        }
    }

    /// フル解像度の画像をスロットに持たせる。上限を超えるときは持たずに縮小画像のままにする
    /// （コピー待ちならコピーだけ済ませる）。
    fn attach_full(&mut self, ctx: &egui::Context, index: usize, full: FullImage) {
        let s = &mut self.slots[index];
        let Some(shown) = &mut s.shown else {
            return;
        };
        if std::mem::take(&mut s.copy_when_full) {
            let image = copy::full_resolution(&full.image);
            let what = format!(
                "image {} ({}x{})",
                shown.path.display(),
                image.size[0],
                image.size[1]
            );
            ctx.copy_image(image);
            self.copied(ctx, &what);
        }
        let bytes = full.bytes();
        let limit = self.settings.settings.viewer.full_image_limit_bytes();
        if !memory::fits(&self.full_states(), bytes, limit) {
            return;
        }
        let s = &mut self.slots[index];
        let Some(shown) = &mut s.shown else {
            return;
        };
        let path = &shown.path;
        let texture = ctx.load_texture(path.to_string_lossy(), full.display, texture_options());
        if texture.size()[0] < shown.info.width as usize {
            self.log.warn(format!(
                "{}: shown downsampled to {}x{} (texture size limit)",
                path.display(),
                texture.size()[0],
                texture.size()[1]
            ));
        }
        shown.full = Some(Full {
            image: full.image,
            texture,
            bytes,
            last_used: self.frame,
        });
    }

    /// 各スロットのフル解像度の画像の状態（[`memory::plan`] の入力）。
    fn full_states(&self) -> Vec<memory::FullState> {
        self.slots
            .iter()
            .filter_map(|s| {
                let shown = s.shown.as_ref()?;
                Some(memory::FullState {
                    id: s.id,
                    wanted: s.wants_full_now,
                    resident: shown.full.as_ref().map(|f| f.bytes),
                    pending: s.full_request.map(|(_, bytes)| bytes),
                    last_used: shown.full.as_ref().map_or(0, |f| f.last_used),
                    estimate: shown.info.full_bytes(),
                })
            })
            .collect()
    }

    /// このフレームで必要になったフル解像度の画像を読み込み、上限を超える分を捨てる。
    fn manage_full_images(&mut self) {
        let limit = self.settings.settings.viewer.full_image_limit_bytes();
        let plan = memory::plan(&self.full_states(), limit);
        for s in &mut self.slots {
            let Some(shown) = &mut s.shown else {
                continue;
            };
            if plan.evict.contains(&s.id) {
                shown.full = None;
            }
            if plan.request.contains(&s.id) {
                let request = self.next_id;
                self.next_id += 1;
                s.full_request = Some((request, shown.info.full_bytes()));
                self.loader.request(Job {
                    slot: s.id,
                    request,
                    path: shown.path.clone(),
                    thumb: false,
                    full: true,
                });
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let ctx = ui.ctx().clone();
        let mut chosen = None;
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(tr(lang, "File"), |ui| {
                for action in [Action::NewSlot, Action::CloseSlot, Action::CloseAllSlots] {
                    if self.menu_item(ui, action, None) {
                        chosen = Some(action);
                    }
                }
                ui.separator();
                if self.menu_item(ui, Action::Quit, None) {
                    chosen = Some(Action::Quit);
                }
            });
            ui.menu_button(tr(lang, "View"), |ui| {
                let toggles = [
                    (Action::ToggleFolderTree, self.fields.tree),
                    (Action::ToggleMetadata, self.fields.metadata),
                    (Action::ToggleImages, self.fields.images),
                    (Action::ToggleLog, self.fields.log),
                ];
                for (action, shown) in toggles {
                    if self.menu_item(ui, action, Some(shown)) {
                        chosen = Some(action);
                    }
                }
                ui.separator();
                if self.menu_item(ui, Action::ToggleSettings, Some(self.show_settings)) {
                    chosen = Some(Action::ToggleSettings);
                }
            });
        });
        if let Some(action) = chosen {
            self.run(&ctx, action);
        }
    }

    /// メニューの 1 項目。右側に操作の最初のキーを出す。`checked` があれば左にチェック欄を付ける。
    ///
    /// 表示切り替えに egui のチェックボックスを使わないのは、チェックボックスには
    /// ショートカットの表示欄がなく、他の項目とキーの列がそろわないため。
    fn menu_item(&self, ui: &mut egui::Ui, action: Action, checked: Option<bool>) -> bool {
        let title = tr(self.lang(), menu_title(action));
        let text = match checked {
            Some(true) => format!("{} {title}", icon::CHECK_SQUARE),
            Some(false) => format!("{} {title}", icon::SQUARE),
            None => title.to_owned(),
        };
        let button =
            egui::Button::new(text).shortcut_text(self.shortcut(action).unwrap_or_default());
        ui.add(button).clicked()
    }

    /// メタデータ欄に出すスロット。アクティブなスロットが読み込み中でまだ画像がないときは、
    /// 直前に出していたスロットのまま変えない。
    ///
    /// 読み込み中に「画像なし」を出すと、スロットを足して画像を開くたびに表が消えては現れ、
    /// フォルダの画像をまとめて開くときに点滅して見えるため。
    fn metadata_slot(&mut self) -> Option<usize> {
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

    /// メタデータ欄に出す表。ファイル情報のグループの後に、プラグインのメタデータのグループが続く。
    /// 画像がなければ `None`。
    fn metadata_groups(&mut self) -> Option<Vec<MetaGroup>> {
        let lang = self.lang();
        let index = self.metadata_slot()?;
        let shown = self.slots[index].shown.as_ref()?;
        let info = &shown.info;
        let row = |k: &'static str, v: String| (tr(lang, k).to_owned(), v);
        let file_rows = vec![
            row("Path", shown.path.display().to_string()),
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
                let kind = if shown.full.is_some() {
                    ""
                } else {
                    " (thumbnail)"
                };
                format!("{w} x {h}{kind}")
            }),
        ];
        let mut groups = vec![(tr(lang, "File").to_owned(), file_rows)];
        groups.extend(
            group_metadata(&info.metadata.entries)
                .into_iter()
                .map(|(g, rows)| {
                    let rows = rows
                        .into_iter()
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .collect();
                    (g.to_owned(), rows)
                }),
        );
        Some(groups)
    }

    /// スロットの画像について、ファイル情報とプラグインのメタデータを表で出す。
    /// 見出しと行はクリックで選んでコピーでき、値の文字列はドラッグで範囲選択できる。
    fn metadata_ui(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let Some(groups) = self.metadata_groups() else {
            ui.weak(tr(lang, "No image"));
            return;
        };
        let header_click = self.settings.settings.behavior.header_click;
        let mut picked: Option<MetaSelection> = None;
        let mut copy: Option<MetaSelection> = None;
        let mut text_drag = false;
        let selected = self.meta_selected.clone();
        let highlight = ui.visuals().selection.bg_fill;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (group, rows) in &groups {
                    let id = ui.make_persistent_id(("metadata", group));
                    let state = CollapsingState::load_with_default_open(ui.ctx(), id, true);
                    let group_sel = MetaSelection::Group(group.clone());
                    let is_selected = selected.as_ref() == Some(&group_sel);
                    let (_, head, _) = state
                        .show_header(ui, |ui| ui.selectable_label(is_selected, group.as_str()))
                        .body(|ui| {
                            let row_of = |i: usize| MetaSelection::Row(group.clone(), i);
                            let selected_row = match &selected {
                                Some(MetaSelection::Row(g, i)) if g == group => Some(*i),
                                _ => None,
                            };
                            egui::Grid::new(group)
                                .num_columns(2)
                                // striped(true) と併用できないので、縞も自分で塗る。
                                .with_row_color(move |r, style| {
                                    if Some(r) == selected_row {
                                        Some(highlight)
                                    } else {
                                        (r % 2 == 1).then_some(style.visuals.faint_bg_color)
                                    }
                                })
                                .show(ui, |ui| {
                                    for (i, (k, v)) in rows.iter().enumerate() {
                                        let key = ui.add(
                                            egui::Label::new(k)
                                                .selectable(false)
                                                .sense(egui::Sense::click()),
                                        );
                                        let value = ui.add(egui::Label::new(v).wrap());
                                        ui.end_row();
                                        if key.clicked() || value.clicked() {
                                            picked = Some(row_of(i));
                                        }
                                        if value.drag_started() {
                                            text_drag = true;
                                        }
                                        for r in [&key, &value] {
                                            r.context_menu(|ui| {
                                                if ui.button(tr(lang, "Copy")).clicked() {
                                                    copy = Some(row_of(i));
                                                    ui.close();
                                                }
                                            });
                                        }
                                    }
                                });
                        });
                    let label = head.inner;
                    if label.clicked() {
                        picked = Some(group_sel.clone());
                        // ▶ を押したときと同じく、保存された開閉状態を反転して次のフレームから効かせる。
                        if header_click == HeaderClick::SelectAndToggle {
                            let mut state =
                                CollapsingState::load_with_default_open(ui.ctx(), id, true);
                            state.toggle(ui);
                            state.store(ui.ctx());
                        }
                    }
                    label.context_menu(|ui| {
                        if ui.button(tr(lang, "Copy")).clicked() {
                            copy = Some(group_sel.clone());
                            ui.close();
                        }
                    });
                }
            });
        let ctx = ui.ctx().clone();
        if text_drag {
            self.set_focus(&ctx, Focus::MetadataText);
        }
        if let Some(p) = picked {
            self.meta_selected = Some(p);
            self.set_focus(&ctx, Focus::Metadata);
        }
        if let Some(c) = copy {
            if let Some(text) = copy::metadata_text(&groups, &c) {
                let what = selection_name(&c);
                self.copy_text(&ctx, text, &format!("metadata {what}"));
            }
        }
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

    /// Ctrl+C の対象の欄を変える。メタデータの文字列の範囲選択以外に移るときは、
    /// その範囲選択を解く。残しておくと、egui がフレームの終わりに範囲の文字列をコピーし、
    /// こちらでコピーした内容を上書きしてしまうため。
    fn set_focus(&mut self, ctx: &egui::Context, focus: Focus) {
        self.focus = focus;
        if focus != Focus::MetadataText {
            ctx.with_plugin(|s: &mut LabelSelectionState| s.clear_selection());
        }
    }

    fn copy_text(&mut self, ctx: &egui::Context, text: String, what: &str) {
        ctx.copy_text(text);
        self.copied(ctx, what);
    }

    /// スロットの画像を元の解像度でコピーする。
    /// フル解像度の画像を捨ててあったときは読み直し、届いたときにコピーする。
    fn copy_image(&mut self, ctx: &egui::Context, index: usize) {
        let s = &mut self.slots[index];
        let Some(shown) = &s.shown else {
            return;
        };
        let Some(full) = &shown.full else {
            s.copy_when_full = true;
            if s.full_request.is_none() {
                let request = self.next_id;
                self.next_id += 1;
                s.full_request = Some((request, shown.info.full_bytes()));
                self.loader.request(Job {
                    slot: s.id,
                    request,
                    path: shown.path.clone(),
                    thumb: false,
                    full: true,
                });
            }
            return;
        };
        let image = copy::full_resolution(&full.image);
        let what = format!(
            "image {} ({}x{})",
            shown.path.display(),
            image.size[0],
            image.size[1]
        );
        ctx.copy_image(image);
        self.copied(ctx, &what);
    }

    /// コピーしたことをログとカーソル付近の表示で知らせる。
    fn copied(&mut self, ctx: &egui::Context, what: &str) {
        self.log.info(format!("Copied {what}"));
        let pos = ctx
            .input(|i| i.pointer.latest_pos())
            .unwrap_or_else(|| ctx.content_rect().center());
        self.toast = Some(Toast {
            pos,
            until: Instant::now() + TOAST_DURATION,
        });
    }

    /// Ctrl+C（egui はこれを `Event::Copy` として届ける）で、対象の欄で選んでいるものをコピーする。
    fn copy_selection(&mut self, ctx: &egui::Context) {
        let pressed = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        // 文字入力欄にいるときは、その欄のコピーに任せる。
        if !pressed || self.show_settings || ctx.egui_wants_keyboard_input() {
            return;
        }
        match self.focus {
            Focus::MetadataText => {}
            Focus::Tree => {
                if let Some(p) = self.tree_selected.clone() {
                    self.copy_text(
                        ctx,
                        p.display().to_string(),
                        &format!("path {}", p.display()),
                    );
                }
            }
            Focus::Metadata => {
                let Some(sel) = self.meta_selected.clone() else {
                    return;
                };
                let text = self
                    .metadata_groups()
                    .and_then(|g| copy::metadata_text(&g, &sel));
                if let Some(text) = text {
                    self.copy_text(ctx, text, &format!("metadata {}", selection_name(&sel)));
                }
            }
            Focus::Images => self.copy_image(ctx, self.active),
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some(toast) = &self.toast else {
            return;
        };
        let now = Instant::now();
        if now >= toast.until {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::new("copied_toast"))
            .order(egui::Order::Tooltip)
            .fixed_pos(toast.pos + vec2(12.0, 12.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(tr(self.lang(), "Copied"));
                });
            });
        ctx.request_repaint_after(toast.until - now);
    }

    /// スロットをグリッドに並べて描く。
    ///
    /// egui の Grid ウィジェットを使わず矩形を自分で割り当てるのは、Grid が中身の大きさに
    /// 合わせて伸び縮みするため。画像の大きさに関係なく枠を等分したい。
    fn images_ui(&mut self, ui: &mut egui::Ui) {
        let area = ui.available_rect_before_wrap();
        self.images_area = Some(area.size() * ui.ctx().pixels_per_point());
        self.frame += 1;
        let n = self.slots.len();
        let (cols, rows) = grid_dims(n);
        let cell = vec2(area.width() / cols as f32, area.height() / rows as f32);
        let mut close = None;
        let background = self.canvas_color(ui.ctx());
        let lang = self.lang();
        let hint = empty_slot_hint(&self.settings.settings.tree);
        for i in 0..n {
            let (c, r) = (i % cols, i / cols);
            let rect = Rect::from_min_size(
                pos2(
                    area.min.x + c as f32 * cell.x,
                    area.min.y + r as f32 * cell.y,
                ),
                cell,
            );
            let resp = self.slots[i].ui(
                ui,
                rect.shrink(1.0),
                i == self.active,
                background,
                lang,
                hint,
            );
            let slot = &mut self.slots[i];
            slot.wants_full_now = resp.wants_full;
            if let Some(full) = slot.shown.as_mut().and_then(|s| s.full.as_mut()) {
                if resp.wants_full {
                    full.last_used = self.frame;
                }
            }
            if resp.clicked {
                self.active = i;
                self.tree_selected = self.slots[i].path.clone();
                self.set_focus(ui.ctx(), Focus::Images);
            }
            if resp.copy {
                self.copy_image(ui.ctx(), i);
            }
            if resp.close {
                close = Some(i);
            }
        }
        // 描画ループの途中で削除すると添字がずれるので、ループの後で閉じる。
        if let Some(i) = close {
            self.close_slot(i);
        }
        self.manage_full_images();
        ui.allocate_rect(area, egui::Sense::hover());
    }
}

impl eframe::App for VwApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_state();
    }

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
        self.restore_window(&ctx);
        self.track_window(&ctx);
        self.receive(&ctx);
        self.pump_folder_load();
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
                    let selected = self.tree_selected.clone();
                    let t = &self.settings.settings.tree;
                    let how = FolderClicks {
                        mode: t.mode,
                        action: t.folder_action,
                        double_click: t.double_click(),
                    };
                    let action = egui::ScrollArea::both()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            self.tree
                                .ui(ui, self.loader.registry(), selected.as_deref(), lang, how)
                        })
                        .inner;
                    let Some(action) = action else {
                        return;
                    };
                    let ctx = ui.ctx().clone();
                    match action {
                        TreeAction::Open(p) => self.open_in_active(p),
                        TreeAction::Add(p) => {
                            self.add_slot();
                            self.open_in_active(p);
                        }
                        TreeAction::Select(p) => self.tree_selected = Some(p),
                        TreeAction::Load(p) => {
                            self.tree_selected = Some(p.clone());
                            self.load_folder(&ctx, p);
                        }
                        TreeAction::Copy(p) => {
                            let what = format!("path {}", p.display());
                            self.copy_text(&ctx, p.display().to_string(), &what);
                            return;
                        }
                    }
                    self.set_focus(&ctx, Focus::Tree);
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
        self.copy_selection(&ctx);
        self.toast_ui(&ctx);
        self.folder_load_dialog(&ctx);
    }
}

/// メニューでの操作の名前。表示切り替えはメニューの中では対象の名前だけで分かるので短くする。
fn menu_title(action: Action) -> &'static str {
    match action {
        Action::ToggleFolderTree => "Folder tree",
        Action::ToggleMetadata => "Metadata",
        Action::ToggleImages => "Images",
        Action::ToggleLog => "Log",
        Action::ToggleSettings => "Settings",
        other => other.title(),
    }
}

/// 空のスロットに出す案内。ツリーのモードとフォルダの操作に合わせる。
fn empty_slot_hint(tree: &crate::settings::Tree) -> &'static str {
    match (tree.mode, tree.folder_action) {
        (TreeMode::Normal, _) => "Select an image in the folder tree (Ctrl+click adds a slot)",
        (TreeMode::AutoLoad, FolderAction::ShiftClick) => {
            "Shift+click a folder in the tree to load its images"
        }
        (TreeMode::AutoLoad, _) => "Double-click a folder in the tree to load its images",
    }
}

/// パネルの見出し。本文と区別できる小さな大文字にして、縦の領域を取りすぎないようにする。
fn panel_header(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text.to_uppercase()).small().weak());
}

/// ログに出す、メタデータ欄で選んだものの名前。
fn selection_name(sel: &MetaSelection) -> String {
    match sel {
        MetaSelection::Group(g) => g.clone(),
        MetaSelection::Row(g, i) => format!("{g} row {}", i + 1),
    }
}

/// 拡大時は最近傍（画素の境界をそのまま見せる）、縮小時は線形＋ミップマップ。
/// 拡大まで線形にすると、画素単位で確認したい医用画像などがぼやけるため。
fn texture_options() -> TextureOptions {
    TextureOptions {
        magnification: TextureFilter::Nearest,
        minification: TextureFilter::Linear,
        mipmap_mode: Some(TextureFilter::Linear),
        ..TextureOptions::LINEAR
    }
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

    fn view(id: u64, shown: bool, loading: bool) -> SlotView {
        SlotView { id, shown, loading }
    }

    #[test]
    fn metadata_follows_the_active_slot_with_an_image() {
        let slots = [view(0, true, false), view(1, true, false)];
        assert_eq!(metadata_source(&slots, 1, Some(0)), Some(1));
    }

    #[test]
    fn metadata_stays_on_previous_slot_while_new_slot_loads() {
        let slots = [view(0, true, false), view(1, false, true)];
        assert_eq!(metadata_source(&slots, 1, Some(0)), Some(0));
    }

    #[test]
    fn empty_slot_that_is_not_loading_has_no_metadata() {
        let slots = [view(0, true, false), view(1, false, false)];
        assert_eq!(metadata_source(&slots, 1, Some(0)), None);
    }

    #[test]
    fn closed_previous_slot_leaves_metadata_empty_while_loading() {
        let slots = [view(1, false, true)];
        assert_eq!(metadata_source(&slots, 0, Some(0)), None);
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
