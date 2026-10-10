//! アプリ本体。ツール列と 4 つのフィールド（フォルダツリー・メタデータ・画像・ログ）の配置、
//! 設定画面への切り替え、ホットキーの実行、スロットの管理、読み込み結果の受け取りを行う。
//!
//! egui は即時モード GUI で、毎フレーム `ui()` が呼ばれて画面全体を組み立て直す。
//! 状態はすべて [`VwApp`] が持ち、ウィジェット側には持たせない。

use std::collections::{HashMap, HashSet};
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
use crate::layout::{self, SlotLayout};
use crate::loader::{FullImage, Job, Loaded, Loader};
use crate::log::{Level, Log};
use crate::memory;
use crate::settings::{
    parse_hex_color, FolderAction, HeaderClick, Language, LoadInto, OnLoadFolder, SettingsStore,
    TreeMode,
};
use crate::settings_ui::{fps_input, layout_title, SettingsUi};
use crate::slideshow::{self, Clock, EndMode, Frame, Playback};
use crate::state::{self, AppState, Monitor, Placement, WindowState};
use crate::theme;
use crate::tree::{self, FolderClicks, FolderTree, TreeAction};
use crate::viewer::{Full, Sequence, Shown, Slot, KEY_ZOOM_FACTOR};
use crate::watch::{self, ChangeKind, FolderWatcher};

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
    /// 画像領域いっぱいに出しているスロットの番号。
    maximized: Option<u64>,
    /// 読み込み中のフォルダの入れ方。
    load_into: LoadInto,
    /// 読み込み中のフォルダのために作った最初のスロットの番号。これ以降の番号のスロットにだけ
    /// 画像を足す。前からあるフォルダスロットに、同じフォルダの画像を重ねて足さないため。
    load_slots_from: u64,
    playback: Option<Playback>,
    /// 最後に止めたスライドショーで飛ばしたコマ数。再生バーに出し続ける。
    last_dropped: u64,
    /// フォルダの監視。自動リロードが無効なら `None`。
    watcher: Option<FolderWatcher>,
    /// 監視を始められなかった。毎フレーム作り直してログを埋めないよう、設定を切り替えるまで諦める。
    watcher_failed: bool,
}

/// 再生バーの高さ。
const PLAY_BAR: f32 = 30.0;

/// 再生バーで行われた操作。
enum BarAction {
    TogglePlay,
    Fps { fps: f32, save: bool },
    End(EndMode),
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
            maximized: None,
            load_into: LoadInto::Images,
            load_slots_from: 0,
            playback: None,
            last_dropped: 0,
            watcher: None,
            watcher_failed: false,
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
        self.maximized = None;
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
        if self.maximized == Some(slot.id) {
            self.maximized = None;
        }
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
        // フォルダスロットに並びの外の画像を開いたら、普通のスロットに戻す。
        let slot = &mut self.slots[self.active];
        // 読み込みで並べたスロットも、別の画像を開いたらそのフォルダの 1 枚ではなくなる。
        slot.from_folder = false;
        if let Some(seq) = &mut slot.sequence {
            match seq.paths.iter().position(|p| *p == path) {
                Some(i) => seq.index = i,
                None => slot.sequence = None,
            }
        }
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
    fn load_folder(&mut self, ctx: &egui::Context, path: PathBuf, into: LoadInto) {
        let tree = self.settings.settings.tree.clone();
        if tree.on_load_other_folder == OnLoadFolder::Replace {
            self.close_all_slots();
        }
        self.load_into = into;
        // 末尾の空のスロットは読み込みで使い回すので、この読み込みのスロットに含める。
        let last = &self.slots[self.slots.len() - 1];
        self.load_slots_from = if last.path.is_none() && !last.loading {
            last.id
        } else {
            self.next_id
        };
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
                if self.load_into == LoadInto::Folders {
                    let last = self.slots.last_mut().expect("there is always a slot");
                    let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
                    if let Some(seq) = last
                        .sequence
                        .as_mut()
                        .filter(|s| s.folder == folder && last.id >= self.load_slots_from)
                    {
                        seq.paths.push(path);
                        continue;
                    }
                }
                if !std::mem::take(&mut reuse_last) {
                    self.slots.push(Slot::new(self.next_id));
                    self.next_id += 1;
                }
                let index = self.slots.len() - 1;
                self.slots[index].from_folder = self.load_into == LoadInto::Images;
                if self.load_into == LoadInto::Folders {
                    let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
                    self.slots[index].sequence = Some(Sequence::new(folder, path.clone()));
                }
                targets.push((index, path));
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
        let view = self.view();
        let cell = (area.x / view.cols as f32).max(area.y / view.rows as f32);
        cell > memory::THUMB_SIDE as f32
    }

    /// アクティブなスロットを最大化して、その並び（フォルダスロットならその中身、
    /// 普通のスロットなら同じフォルダの画像）をスライドショーで再生する。
    fn start_playback(&mut self) {
        let slot = &self.slots[self.active];
        let (frames, start) = match &slot.sequence {
            Some(seq) => (seq.paths.clone(), seq.index),
            None => {
                let Some(path) = slot.path.clone() else {
                    return;
                };
                let order = self.settings.settings.tree.order();
                let files = tree::sibling_images(&path, self.loader.registry(), order);
                let start = files.iter().position(|p| *p == path).unwrap_or(0);
                (files, start)
            }
        };
        if frames.len() < 2 {
            self.log.warn("Slideshow: there is only one image to show");
            return;
        }
        if self.maximized != Some(slot.id) {
            self.toggle_maximize(self.active);
        }
        let v = &self.settings.settings.viewer;
        let clock = Clock::new(
            start,
            frames.len(),
            v.slideshow_end,
            v.slideshow_fps,
            Instant::now(),
        );
        let loader_slot = self.next_id;
        self.next_id += 1;
        self.last_dropped = 0;
        self.playback = Some(Playback::new(
            self.slots[self.active].id,
            loader_slot,
            frames,
            clock,
        ));
    }

    fn stop_playback(&mut self) {
        let Some(pb) = self.playback.take() else {
            return;
        };
        for request in pb.pending.keys() {
            self.loader.cancel(*request);
        }
        self.last_dropped = pb.clock.dropped;
        if pb.clock.dropped > 0 {
            self.log.warn(format!(
                "Slideshow: {} frames were skipped because images could not be decoded fast enough",
                pb.clock.dropped
            ));
        }
    }

    /// スライドショーを 1 フレーム分進める。今のコマの画像が届いていれば表示し、
    /// この先のコマを先読みする。
    fn drive_playback(&mut self, ctx: &egui::Context) {
        let Some(pb) = &self.playback else {
            return;
        };
        let index = self.slots.iter().position(|s| s.id == pb.slot);
        let Some(index) = index.filter(|_| self.maximized == Some(pb.slot)) else {
            // 最大化を解いた、別のスロットに移った、スロットを閉じた。
            self.stop_playback();
            return;
        };
        let now = Instant::now();
        let full = self.cell_needs_full();
        // 先読みには、表示中の画像のフル解像度と同じ上限の半分まで使う。
        let budget = self.settings.settings.viewer.full_image_limit_bytes() / 2;
        let pb = self.playback.as_mut().expect("checked above");
        let step = pb.clock.advance(now);
        let mut show = None;
        if !pb.clock.shown() {
            if let Some(frame) = pb.cache.get(&step.index).cloned() {
                pb.clock.mark_shown();
                show = Some(frame);
            } else if pb.failed.contains(&step.index) {
                pb.clock.mark_shown();
            }
        }
        let (start, n, mode) = (pb.clock.start, pb.clock.n, pb.clock.mode);
        let mut wanted = slideshow::upcoming(start, pb.clock.tick(), n, mode, pb.ahead(budget));
        if !wanted.contains(&step.index) {
            wanted.insert(0, step.index);
        }
        pb.cache.retain(|i, _| wanted.contains(i));
        let loader = &self.loader;
        pb.pending.retain(|request, i| {
            let keep = wanted.contains(i);
            if !keep {
                loader.cancel(*request);
            }
            keep
        });
        let mut requests = Vec::new();
        for &i in &wanted {
            let busy = pb.cache.contains_key(&i) || pb.pending.values().any(|p| *p == i);
            if !busy && !pb.failed.contains(&i) {
                requests.push(i);
            }
        }
        for i in requests {
            let request = self.next_id;
            self.next_id += 1;
            pb.pending.insert(request, i);
            self.loader.request(Job {
                slot: pb.loader_slot,
                request,
                path: pb.frames[i].clone(),
                thumb: !full,
                full,
            });
        }
        let finished = step.finished && pb.clock.shown();
        let wait = pb.clock.until_next(now);
        if let Some(frame) = show {
            self.show_frame(index, step.index, frame);
        }
        if finished {
            self.stop_playback();
        } else {
            ctx.request_repaint_after(wait);
        }
    }

    /// 先読みした画像を、読み込みを通さずにスロットへ出す。表示倍率と位置はそのまま保つ
    /// （拡大した部分を見ながら再生できるように）。
    fn show_frame(&mut self, index: usize, position: usize, frame: Frame) {
        let request = self.next_id;
        self.next_id += 1;
        self.cancel_requests(&self.slots[index]);
        let s = &mut self.slots[index];
        s.path = Some(frame.path.clone());
        s.request = request;
        s.loading = false;
        s.error = None;
        s.full_request = None;
        s.copy_when_full = false;
        if let Some(seq) = &mut s.sequence {
            seq.index = position;
        }
        s.shown = Some(Shown {
            path: frame.path,
            thumb: frame.texture,
            info: frame.info,
            full: None,
        });
    }

    /// 最大化したスロットの下の再生バー。
    fn play_bar(&self, ui: &mut egui::Ui) -> Option<BarAction> {
        let lang = self.lang();
        let viewer = &self.settings.settings.viewer;
        let mut action = None;
        ui.horizontal_centered(|ui| {
            ui.add_space(6.0);
            let playing = self.playback.is_some();
            let (glyph, tip) = if playing {
                (icon::PAUSE, "Pause")
            } else {
                (icon::PLAY, "Play")
            };
            let tip = self.with_key(tr(lang, tip), Action::TogglePlay);
            if ui.button(glyph).on_hover_text(tip).clicked() {
                action = Some(BarAction::TogglePlay);
            }
            let mut fps = viewer.slideshow_fps;
            let r = ui.add(fps_input(&mut fps));
            if r.changed() || r.drag_stopped() {
                // ドラッグ中は再生にだけ反映し、ファイルには離したときに書く。
                let save = !r.dragged();
                action = Some(BarAction::Fps { fps, save });
            }
            for preset in slideshow::FPS_PRESETS {
                let selected = (viewer.slideshow_fps - preset).abs() < 0.05;
                if ui.selectable_label(selected, format!("{preset}")).clicked() {
                    action = Some(BarAction::Fps {
                        fps: preset,
                        save: true,
                    });
                }
            }
            ui.separator();
            let mut end = viewer.slideshow_end;
            egui::ComboBox::from_id_salt("slideshow_end")
                .selected_text(tr(lang, end.title()))
                .show_ui(ui, |ui| {
                    for mode in EndMode::ALL {
                        ui.selectable_value(&mut end, mode, tr(lang, mode.title()));
                    }
                });
            if end != viewer.slideshow_end {
                action = Some(BarAction::End(end));
            }
            ui.separator();
            if let Some(seq) = self
                .slots
                .get(self.active)
                .and_then(|s| s.sequence.as_ref())
            {
                ui.label(format!("{} / {}", seq.index + 1, seq.paths.len()));
            }
            let dropped = self
                .playback
                .as_ref()
                .map_or(self.last_dropped, |pb| pb.clock.dropped);
            if dropped > 0 {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    tr1(lang, "dropped {}", &dropped.to_string()),
                )
                .on_hover_text(tr(
                    lang,
                    "Images could not be decoded fast enough; some frames were skipped.",
                ));
            }
        });
        action
    }

    fn apply_bar_action(&mut self, ctx: &egui::Context, action: BarAction) {
        let viewer = &mut self.settings.settings.viewer;
        match action {
            BarAction::TogglePlay => return self.run(ctx, Action::TogglePlay),
            BarAction::Fps { fps, save } => {
                viewer.slideshow_fps = fps;
                if !save {
                    self.restart_clock();
                    return;
                }
            }
            BarAction::End(mode) => viewer.slideshow_end = mode,
        }
        self.restart_clock();
        self.settings_changed(ctx);
    }

    /// 再生中なら、変えた FPS と終端の扱いで数え直す。
    fn restart_clock(&mut self) {
        let viewer = &self.settings.settings.viewer;
        if let Some(pb) = &mut self.playback {
            pb.clock
                .restart(viewer.slideshow_fps, viewer.slideshow_end, Instant::now());
        }
    }

    /// 画面に出すスロットとマス目。
    fn view(&self) -> layout::View {
        let maximized = self
            .maximized
            .and_then(|id| self.slots.iter().position(|s| s.id == id));
        layout::view(
            self.settings.settings.viewer.layout,
            self.slots.len(),
            self.active,
            maximized,
        )
    }

    /// アクティブなスロットを `delta` だけ前後に移す。最大化中は最大化するスロットも移す。
    fn step_slot(&mut self, delta: isize) {
        self.active = wrap_step(self.active, self.slots.len(), delta);
        if self.maximized.is_some() {
            self.maximized = Some(self.slots[self.active].id);
        }
        self.tree_selected = self.slots[self.active].path.clone();
    }

    /// `index` 番目のスロットの最大化を切り替える。
    fn toggle_maximize(&mut self, index: usize) {
        let id = self.slots[index].id;
        self.active = index;
        self.maximized = if self.maximized == Some(id) {
            None
        } else {
            Some(id)
        };
    }

    /// レイアウトを変えて設定に保存する。最大化は解く（変えた結果が見えるように）。
    fn set_layout(&mut self, ctx: &egui::Context, layout: SlotLayout) {
        self.maximized = None;
        if self.settings.settings.viewer.layout != layout {
            self.settings.settings.viewer.layout = layout;
            self.settings_changed(ctx);
        }
    }

    /// アクティブなスロットの画像と同じフォルダにある、前後の対応画像に切り替える。
    ///
    /// 毎回フォルダを読み直す。一覧をキャッシュしないのは、表示中にファイルが
    /// 追加・削除されることがあり、数千件程度なら読み直しても体感できる遅れにならないため。
    fn step_active(&mut self, delta: isize) {
        if let Some(seq) = &self.slots[self.active].sequence {
            let next = wrap_step(seq.index, seq.paths.len(), delta);
            if next != seq.index {
                self.open_in_active(seq.paths[next].clone());
            }
            return;
        }
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

    /// 監視するフォルダ。ツリーで開いているフォルダと、スロットに出している画像のフォルダ。
    fn watched_dirs(&self) -> HashSet<PathBuf> {
        let mut dirs: HashSet<PathBuf> = self.tree.open_paths().into_iter().collect();
        for slot in &self.slots {
            if let Some(seq) = &slot.sequence {
                dirs.insert(seq.folder.clone());
            } else if let Some(dir) = slot.path.as_deref().and_then(Path::parent) {
                dirs.insert(dir.to_path_buf());
            }
        }
        dirs
    }

    /// フォルダの変化を受け取り、ツリーとスロットに反映する。
    fn watch_folders(&mut self, ctx: &egui::Context) {
        if !self.settings.settings.behavior.auto_reload {
            self.watcher = None;
            self.watcher_failed = false;
            return;
        }
        if self.watcher.is_none() && !self.watcher_failed {
            match FolderWatcher::new(ctx.clone()) {
                Ok(w) => self.watcher = Some(w),
                Err(e) => {
                    self.log
                        .error(format!("Auto reload: cannot watch folders ({e})"));
                    self.watcher_failed = true;
                }
            }
        }
        let dirs = self.watched_dirs();
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        for problem in watcher.set_dirs(dirs) {
            self.log.warn(format!("Auto reload: {problem}"));
        }
        let changes = watcher.poll(Instant::now());
        if watcher.waiting() {
            ctx.request_repaint_after(watch::QUIET);
        }
        if !changes.is_empty() {
            self.apply_changes(changes);
        }
    }

    fn apply_changes(&mut self, changes: Vec<watch::Change>) {
        let registry = self.loader.registry().clone();
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut created = HashSet::new();
        let mut modified = HashSet::new();
        let mut removed = HashSet::new();
        for c in changes {
            if let Some(dir) = c.path.parent().filter(|d| !dirs.iter().any(|x| x == d)) {
                dirs.push(dir.to_path_buf());
            }
            if !registry.supports_path(&c.path) {
                continue;
            }
            match c.kind {
                ChangeKind::Created => created.insert(c.path),
                ChangeKind::Modified => modified.insert(c.path),
                ChangeKind::Removed => removed.insert(c.path),
            };
        }
        for dir in &dirs {
            self.tree.refresh(dir, &registry);
            let count =
                |set: &HashSet<PathBuf>| set.iter().filter(|p| p.parent() == Some(dir)).count();
            let (new, gone) = (count(&created), count(&removed));
            if new + gone > 0 {
                self.log.info(format!(
                    "Auto reload: {new} new / {gone} removed image(s) in {}",
                    dir.display()
                ));
            }
        }
        // フォルダを読み込み中は、スロットの並びを読み込みのほうに任せる。並びを読み直すと、
        // まだ読み込みが足していない画像と重なるため。
        if self.folder_load.is_some() {
            return;
        }
        let order = self.settings.settings.tree.order();
        for dir in &dirs {
            self.sync_loaded_folder(dir, &created, &removed, &registry);
        }
        let playing = self.playback.as_ref().map(|pb| pb.slot);
        let mut siblings: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        let mut loads = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if playing == Some(slot.id) || slot.from_folder {
                continue;
            }
            let Some(current) = slot.path.clone() else {
                continue;
            };
            let Some(dir) = current.parent().filter(|d| dirs.iter().any(|x| x == d)) else {
                continue;
            };
            // 同じフォルダの画像を出しているスロットが多くても、フォルダは 1 回だけ読む。
            let paths = siblings
                .entry(dir.to_path_buf())
                .or_insert_with(|| tree::list_folder(dir, &registry, order).1);
            if paths.is_empty() {
                continue;
            }
            let at = watch::follow_newest(paths, &current, &created)
                .or_else(|| paths.iter().position(|p| *p == current))
                .unwrap_or_else(|| watch::position_of_removed(paths, &current, order));
            if let Some(seq) = &mut slot.sequence {
                seq.paths = paths.clone();
                seq.index = at;
            }
            let target = paths[at].clone();
            if target != current || modified.contains(&current) {
                loads.push((index, target));
            }
        }
        let full = self.cell_needs_full();
        for (index, path) in loads {
            if index == self.active {
                self.tree_selected = Some(path.clone());
            }
            self.start_load(index, path, full);
        }
    }

    /// フォルダの読み込み（1 枚 1 スロット）で並べたスロットを、フォルダの今の中身に合わせる。
    /// 消えた画像のスロットは閉じ、新しい画像はそのフォルダの最後のスロットの後ろに足す。
    /// 1 つのスロットの中身を差し替えないのは、並べて見ている他の画像が入れ替わってしまうため。
    fn sync_loaded_folder(
        &mut self,
        dir: &Path,
        created: &HashSet<PathBuf>,
        removed: &HashSet<PathBuf>,
        registry: &Registry,
    ) {
        let in_dir =
            |s: &Slot| s.from_folder && s.path.as_deref().and_then(Path::parent) == Some(dir);
        for index in (0..self.slots.len()).rev() {
            let slot = &self.slots[index];
            if in_dir(slot) && slot.path.as_ref().is_some_and(|p| removed.contains(p)) {
                self.close_slot(index);
            }
        }
        let Some(last) = self.slots.iter().rposition(in_dir) else {
            return;
        };
        let order = self.settings.settings.tree.order();
        let new: Vec<PathBuf> = tree::list_folder(dir, registry, order)
            .1
            .into_iter()
            .filter(|p| created.contains(p))
            .collect();
        if new.is_empty() {
            return;
        }
        let follow = self.active == last;
        let last_id = self.slots[last].id;
        let full = self.cell_needs_full();
        for (i, path) in new.into_iter().enumerate() {
            let index = last + 1 + i;
            let mut slot = Slot::new(self.next_id);
            self.next_id += 1;
            slot.from_folder = true;
            self.slots.insert(index, slot);
            if self.active >= index {
                self.active += 1;
            }
            self.start_load(index, path, full);
        }
        if follow {
            self.active = self.slots.iter().rposition(in_dir).unwrap_or(self.active);
            if self.maximized == Some(last_id) {
                self.maximized = Some(self.slots[self.active].id);
            }
            self.tree_selected = self.slots[self.active].path.clone();
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
            Action::PreviousImage => {
                self.stop_playback();
                self.step_active(-1);
            }
            Action::NextImage => {
                self.stop_playback();
                self.step_active(1);
            }
            Action::TogglePlay => {
                if self.playback.is_some() {
                    self.stop_playback();
                } else {
                    self.start_playback();
                }
            }
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
            Action::PreviousSlot => self.step_slot(-1),
            Action::NextSlot => self.step_slot(1),
            Action::PreviousPage | Action::NextPage => {
                let delta = if action == Action::NextPage { 1 } else { -1 };
                if self.maximized.is_none() {
                    let layout = self.settings.settings.viewer.layout;
                    self.active = layout::page_step(layout, self.slots.len(), self.active, delta);
                }
            }
            Action::ToggleMaximize => self.toggle_maximize(self.active),
            Action::FitImage => self.slots[self.active].reset_view(),
            Action::ActualSize => self.slots[self.active].actual_size(),
            Action::ZoomIn => self.slots[self.active].zoom_by(KEY_ZOOM_FACTOR),
            Action::ZoomOut => self.slots[self.active].zoom_by(1.0 / KEY_ZOOM_FACTOR),
            Action::LayoutCustom => {
                self.set_layout(ctx, self.settings.settings.viewer.custom_layout)
            }
            Action::LayoutAuto
            | Action::Layout1x1
            | Action::Layout1x2
            | Action::Layout2x1
            | Action::Layout2x2
            | Action::Layout3x3
            | Action::Layout4x4 => {
                if let Some(layout) = action.layout() {
                    self.set_layout(ctx, layout);
                }
            }
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
            if let Some(pb) = self.playback.as_mut().filter(|pb| pb.loader_slot == r.slot) {
                let Some(index) = pb.pending.remove(&r.request) else {
                    continue;
                };
                match r.result {
                    Ok(loaded) => {
                        let image = loaded.full.map(|f| f.display).or(loaded.thumb);
                        if let Some(image) = image {
                            pb.frame_bytes = image.pixels.len() * 4;
                            let texture = ctx.load_texture(
                                format!("{}#play", r.path.display()),
                                image,
                                texture_options(),
                            );
                            pb.cache.insert(
                                index,
                                Frame {
                                    path: r.path,
                                    texture,
                                    info: loaded.info,
                                },
                            );
                        }
                    }
                    Err(e) => {
                        pb.failed.insert(index);
                        self.log.error(format!("{}: {e}", r.path.display()));
                    }
                }
                continue;
            }
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
        let reloaded = s.shown.as_ref().is_some_and(|shown| shown.path == path);
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
        // 同じ画像の読み直し（上書きされたとき）は、拡大している場所を見続けられるよう倍率と位置を保つ。
        if !reloaded {
            s.reset_view();
        }
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
                let maximized = self.maximized.is_some();
                if self.menu_item(ui, Action::ToggleMaximize, Some(maximized)) {
                    chosen = Some(Action::ToggleMaximize);
                }
                ui.menu_button(tr(lang, "Layout"), |ui| {
                    if let Some(action) = self.layout_menu(ui) {
                        chosen = Some(action);
                    }
                });
                ui.separator();
                if self.menu_item(ui, Action::ToggleSettings, Some(self.show_settings)) {
                    chosen = Some(Action::ToggleSettings);
                }
            });
            ui.menu_button(tr(lang, "Image"), |ui| {
                let groups: [&[Action]; 4] = [
                    &[Action::PreviousImage, Action::NextImage],
                    &[Action::PreviousSlot, Action::NextSlot],
                    &[Action::PreviousPage, Action::NextPage],
                    &[
                        Action::FitImage,
                        Action::ActualSize,
                        Action::ZoomIn,
                        Action::ZoomOut,
                    ],
                ];
                for (i, group) in groups.into_iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    for &action in group {
                        if self.menu_item(ui, action, None) {
                            chosen = Some(action);
                        }
                    }
                }
            });
        });
        if let Some(action) = chosen {
            self.run(&ctx, action);
        }
    }

    /// View > Layout の項目。今のレイアウトに印を付ける。
    fn layout_menu(&self, ui: &mut egui::Ui) -> Option<Action> {
        let lang = self.lang();
        let viewer = &self.settings.settings.viewer;
        let mut chosen = None;
        let presets = [
            Action::LayoutAuto,
            Action::Layout1x1,
            Action::Layout1x2,
            Action::Layout2x1,
            Action::Layout2x2,
            Action::Layout3x3,
            Action::Layout4x4,
        ];
        let items = presets
            .into_iter()
            .filter_map(|a| Some((a, a.layout()?, layout_title(a.layout()?, lang))))
            .chain(std::iter::once((
                Action::LayoutCustom,
                viewer.custom_layout,
                format!(
                    "{} ({})",
                    tr(lang, "Custom"),
                    layout_title(viewer.custom_layout, lang)
                ),
            )));
        let mut marked = false;
        for (action, layout, title) in items {
            // カスタムが既定の項目と同じマス目のときに、2 つに印が付かないようにする。
            let current = !marked && viewer.layout == layout;
            marked |= current;
            let mark = if current { icon::CHECK } else { "  " };
            let button = egui::Button::new(format!("{mark} {title}"))
                .shortcut_text(self.shortcut(action).unwrap_or_default());
            if ui.add(button).clicked() {
                chosen = Some(action);
            }
        }
        chosen
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
        let whole = ui.available_rect_before_wrap();
        self.frame += 1;
        let view = self.view();
        let maximized = self.maximized.is_some() && view.range.len() == 1;
        let area = if maximized {
            let (area, bar) = whole.split_top_bottom_at_y(whole.max.y - PLAY_BAR);
            let action = ui
                .scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| self.play_bar(ui))
                .inner;
            if let Some(action) = action {
                self.apply_bar_action(ui.ctx(), action);
            }
            area
        } else {
            whole
        };
        self.images_area = Some(area.size() * ui.ctx().pixels_per_point());
        let (cols, rows) = (view.cols, view.rows);
        let cell = vec2(area.width() / cols as f32, area.height() / rows as f32);
        let mut close = None;
        let mut maximize = None;
        let mut step = None;
        let background = self.canvas_color(ui.ctx());
        let lang = self.lang();
        let hint = empty_slot_hint(&self.settings.settings.tree);
        // 隠れたスロットのフル解像度の画像は、メモリが足りなくなったら先に捨ててよい。
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if !view.range.contains(&i) {
                slot.wants_full_now = false;
            }
        }
        for (cell_index, i) in view.range.clone().enumerate() {
            let (c, r) = (cell_index % cols, cell_index / cols);
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
            if resp.toggle_maximize {
                maximize = Some(i);
            }
            if resp.step != 0 {
                step = Some((i, resp.step));
            }
        }
        if view.pages > 1 {
            let text = format!("{} / {}", view.page + 1, view.pages);
            ui.painter().text(
                area.right_bottom() - vec2(8.0, 6.0),
                egui::Align2::RIGHT_BOTTOM,
                text,
                egui::FontId::proportional(12.0),
                ui.visuals().weak_text_color(),
            );
        }
        // 描画ループの途中で削除すると添字がずれるので、ループの後で閉じる。
        if let Some(i) = close {
            self.close_slot(i);
        } else if let Some(i) = maximize {
            self.toggle_maximize(i);
        } else if let Some((i, delta)) = step {
            // 最大化中はスロットを順に送る。スロットが 1 つしかなければ、送る先がないので
            // そのスロットのフォルダの画像を送る。
            if self.maximized.is_some() && self.slots.len() > 1 {
                self.step_slot(delta);
            } else {
                self.active = i;
                self.step_active(delta);
            }
        }
        self.manage_full_images();
        ui.allocate_rect(whole, egui::Sense::hover());
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
        self.watch_folders(&ctx);
        self.drive_playback(&ctx);
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
                            let tree = &self.settings.settings.tree;
                            let other = ctx.input(|i| tree.other_load_modifier.held(i.modifiers));
                            let into = if other {
                                tree.load_into.other()
                            } else {
                                tree.load_into
                            };
                            self.load_folder(&ctx, p, into);
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
        Action::ToggleMaximize => "Maximize slot",
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
