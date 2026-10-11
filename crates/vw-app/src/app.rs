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

use crate::archive::{self, Archives};
use crate::autoload::{FolderLoad, Question};
use crate::copy::{self, Focus, MetaGroup, MetaSelection};
use crate::discard::{self, TrashJob};
use crate::export::{self, CopyJob, Filter, ListFormat, Matcher};
use crate::frames;
use crate::i18n::{tr, tr1};
use crate::keymap::{Action, KeyCombo};
use crate::labels::{Flag, Label, LabelStore};
use crate::layout::{self, SlotLayout};
use crate::loader::{FullImage, Job, Loaded, Loader};
use crate::log::{Level, Log};
use crate::memory;
use crate::series::{Scan, SeriesIndex};
use crate::settings::{parse_hex_color, HeaderClick, Language, LoadInto, SettingsStore};
use crate::settings_ui::{fps_input, layout_title, SettingsUi};
use crate::slideshow::{self, Clock, EndMode, Frame, Playback};
use crate::state::{self, AppState, Monitor, Placement, WindowState};
use crate::theme;
use crate::transfer::{Op, Transfer};
use crate::tree::{self, FolderTree, TreeAction};
use crate::viewer::{
    DraggedFile, Full, Sequence, SequenceKind, Shown, Slot, SlotLook, KEY_ZOOM_FACTOR,
};
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
    /// レジストリに渡した、DICOM か調べる拡張子の式。変わったときだけフォルダを読み直すため。
    probe_extensions: String,
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
    meta_selected: Option<MetaSelection>,
    /// ログ欄で選んでいる行。
    log_selected: Option<usize>,
    /// コピーしたことを知らせる表示。
    toast: Option<Toast>,
    /// 描いたフレームの番号。フル解像度の画像を最後に使った時期の比較に使う。
    frame: u64,
    /// 画像フィールドの大きさ（物理ピクセル）。読み込むときにフル解像度が要りそうかの見積もりに使う。
    images_area: Option<egui::Vec2>,
    /// cd しているフォルダ。スロットにはその中のフォルダと画像を並べる。
    current: Option<PathBuf>,
    /// cd したフォルダの画像の読み込み。
    folder_load: Option<FolderLoad>,
    /// 開いたフォルダのスロットの中身の読み込みと、そのスロットの番号。
    explores: Vec<(u64, FolderLoad)>,
    /// フォルダの DICOM の検査とシリーズ。ツリーと、cd したフォルダのスロットの両方で使う。
    series: SeriesIndex,
    /// ドロップされ、確認を待っている画像の移動。
    pending_move: Option<Transfer>,
    /// ドラッグ中に Ctrl が押されていた（コピーとして見せていた）か。
    drag_copy: bool,
    /// OS からドロップされた画像。cd した先で読み込まれたら、そのスロットを選ぶ。
    select_when_loaded: Option<PathBuf>,
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
    /// cd したフォルダと、画像の入れ方。画像がなかったフォルダも監視して、
    /// 後から来た画像をスロットに足すために覚えておく。
    loaded_folders: Vec<(PathBuf, LoadInto)>,
    /// ツリーで開いた ZIP。
    archives: Archives,
    labels: LabelStore,
    /// タグ付けモード。↑ ↓ と数字キーでフラグとタグを付ける。
    tagging: bool,
    /// ツリー・画像送り・フォルダの読み込み・スライドショーで対象にする画像。
    filter: Filter,
    export: Option<ExportDialog>,
    /// ごみ箱へ送るか確かめている悪いフラグの画像。
    pending_trash: Option<discard::Pending>,
    trash_job: Option<TrashJob>,
    /// 監視を始められなかった。毎フレーム作り直してログを埋めないよう、設定を切り替えるまで諦める。
    watcher_failed: bool,
}

/// 再生バーの高さ。タグ付けモードのバーも同じ高さにする。
const PLAY_BAR: f32 = 30.0;

/// 書き出しの種類。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExportKind {
    Copy,
    List(ListFormat),
}

/// 画像の書き出しの画面の状態。
struct ExportDialog {
    root: PathBuf,
    recursive: bool,
    kind: ExportKind,
    dest: String,
    keep_structure: bool,
    list_path: String,
    /// 実行中・実行後のコピー。
    job: Option<CopyJob>,
}

/// タグ付けモードのバーで押されたもの。
enum TagClick {
    Flag(Flag),
    Tag(usize),
}

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

/// スロットの上でホイールを回したときに送るもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelStep {
    /// スロットの中の画像を送る。
    Image,
    /// アクティブなスロットを送る。
    Slot,
    Nothing,
}

/// ホイールで何を送るか。塊（フォルダごと）のスロットは中の画像を送る。単体のスロットは
/// 1 枚だけ見ているとき（1x1 か最大化）に限って送る。並べて見ているときに送ると、手元の
/// スロットだけ知らないうちに別の画像に変わってしまうため。1 枚だけ見ていてスロットが
/// ほかにもあれば、隣のスロットへ移る。
pub fn wheel_step(chunk: bool, one_up: bool, slots: usize) -> WheelStep {
    match (chunk, one_up) {
        (true, _) => WheelStep::Image,
        (false, true) if slots > 1 => WheelStep::Slot,
        (false, true) => WheelStep::Image,
        (false, false) => WheelStep::Nothing,
    }
}

/// `paths` のうち `index` から一番近い、条件に合う位置。同じ距離なら後ろを選ぶ。
pub fn nearest_match(
    paths: &[PathBuf],
    index: usize,
    matches: impl Fn(&Path) -> bool,
) -> Option<usize> {
    (0..paths.len())
        .filter(|&i| matches(&paths[i]))
        .min_by_key(|&i| (i.abs_diff(index), i < index))
}

/// `len` 件の中で `pos` から `delta` 進んだ位置。端まで行ったら反対側に回り込む。
pub fn wrap_step(pos: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (pos as isize + delta).rem_euclid(len as isize) as usize
}

/// `active` から `delta` 個先の画像のスロット（`images` が `true` のもの）。フォルダや空の
/// スロットは飛ばす。`wrap` なら端で反対側に回り込み、そうでなければ端で止まる。
/// 進める先がなければ `None`。
pub fn image_slot_step(images: &[bool], active: usize, delta: isize, wrap: bool) -> Option<usize> {
    let len = images.len() as isize;
    let dir = delta.signum();
    let mut pos = active as isize;
    for _ in 0..delta.unsigned_abs() {
        let mut next = pos;
        loop {
            next += dir;
            if wrap {
                next = next.rem_euclid(len);
            } else if !(0..len).contains(&next) {
                return (pos != active as isize).then_some(pos as usize);
            }
            if next == pos {
                return (pos != active as isize).then_some(pos as usize);
            }
            if images[next as usize] {
                break;
            }
        }
        pos = next;
    }
    (pos != active as isize).then_some(pos as usize)
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
        // ZIP はフォルダとして扱う（「プログラムから開く」で ZIP を渡されることがあるため）。
        let (dirs, files): (Vec<PathBuf>, Vec<PathBuf>) = initial
            .into_iter()
            .partition(|p| p.is_dir() || archive::is_zip(p));
        let first_dir = dirs.into_iter().next();
        let root = first_dir.clone().filter(|d| d.is_dir()).or_else(|| {
            files
                .first()
                .or(first_dir.as_ref())?
                .parent()
                .map(Path::to_path_buf)
        });

        registry.set_probe_extensions(settings.settings.dicom.probe());
        let mut tree = FolderTree::new(root, settings.settings.tree.order());
        tree.restore_open(&saved.open_folders, &registry);

        // 書き込めないフォルダのラベルは、設定ファイルの隣の labels フォルダに置く。
        let label_fallback = settings
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(|p| p.join("labels"));
        let mut app = Self {
            loader: Loader::new(registry, cc.egui_ctx.clone()),
            tree,
            probe_extensions: settings.settings.dicom.probe_extensions.clone(),
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
            meta_selected: None,
            log_selected: None,
            toast: None,
            frame: 0,
            images_area: None,
            current: None,
            folder_load: None,
            explores: Vec::new(),
            series: SeriesIndex::default(),
            pending_move: None,
            drag_copy: false,
            select_when_loaded: None,
            maximized: None,
            load_into: LoadInto::Images,
            load_slots_from: 0,
            playback: None,
            last_dropped: 0,
            watcher: None,
            watcher_failed: false,
            loaded_folders: Vec::new(),
            archives: Archives::in_temp_dir(),
            labels: LabelStore::new(label_fallback),
            tagging: false,
            filter: Filter::All,
            export: None,
            pending_trash: None,
            trash_job: None,
        };
        // 引数のフォルダか、前回 cd していたフォルダへ移る。ファイルが渡されたときは
        // ファイルを開くのが目的なので、前回のフォルダには移らない。
        let start_dir = match (&first_dir, files.is_empty()) {
            (Some(dir), _) => Some(dir.clone()),
            (None, true) => saved.current.filter(|d| d.is_dir()),
            (None, false) => None,
        };
        if let Some(dir) = start_dir {
            app.open_folder(&cc.egui_ctx, dir);
        }
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
        self.loaded_folders.clear();
        self.explores.clear();
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
        self.explores.retain(|(id, _)| *id != slot.id);
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
            develop: false,
        });
    }

    /// フォルダ `path` へ移る。スロットを閉じて、中のサブフォルダと ZIP をフォルダのスロットとして並べ、
    /// DICOM のシリーズと画像をその後ろに読み込んでいく。前の読み込みが続いていれば、そこで打ち切る。
    fn cd(&mut self, ctx: &egui::Context, path: PathBuf, into: LoadInto) {
        let tree = self.settings.settings.tree.clone();
        let registry = self.loader.registry().clone();
        self.finish_folder_load();
        self.close_all_slots();
        self.tree.reveal(&path, &registry);
        self.current = Some(path.clone());
        self.load_into = into;
        self.loaded_folders.push((path.clone(), into));
        let listing = tree::list_folder(&path, &registry, tree.order());
        for folder in listing.folders.into_iter().chain(listing.archives) {
            let index = self.push_slot();
            self.slots[index].folder = Some(folder);
        }
        self.scan_series(ctx, &path, &listing.images);
        self.group_series();
        self.active = 0;
        let last = &self.slots[self.slots.len() - 1];
        self.load_slots_from = if is_blank(last) {
            last.id
        } else {
            self.next_id
        };
        self.log.info(format!("Opened folder {}", path.display()));
        let ctx = ctx.clone();
        self.folder_load = Some(FolderLoad::start(
            path,
            registry,
            tree.order(),
            false,
            tree.confirm_every,
            tree.confirm_chunk as usize,
            move || ctx.request_repaint(),
        ));
    }

    /// 末尾の空のスロットを使うか、なければ足して、その添字を返す。
    fn push_slot(&mut self) -> usize {
        if !is_blank(&self.slots[self.slots.len() - 1]) {
            self.slots.push(Slot::new(self.next_id));
            self.next_id += 1;
        }
        self.slots.len() - 1
    }

    /// `dir` の DICOM らしいファイルのヘッダを、まだ読んでいなければ読み始める。
    fn scan_series(&mut self, ctx: &egui::Context, dir: &Path, images: &[PathBuf]) {
        if !matches!(self.series.scan(dir), Scan::NotStarted) {
            return;
        }
        let registry = self.loader.registry().clone();
        let extensions = tree::dicom_extensions(&registry);
        let files: Vec<PathBuf> = images
            .iter()
            .filter(|p| tree::may_be_dicom(&registry, extensions, p))
            .cloned()
            .collect();
        if files.is_empty() {
            return;
        }
        let ctx = ctx.clone();
        self.series
            .start(dir.to_path_buf(), files, registry, move || {
                ctx.request_repaint()
            });
    }

    /// cd したフォルダの DICOM のシリーズ（2 枚以上のもの）。ヘッダを読み終えていなければ空。
    fn current_series(&self) -> Vec<crate::series::Series> {
        let Some(dir) = &self.current else {
            return Vec::new();
        };
        match self.series.scan(dir) {
            Scan::Done(studies) => studies
                .iter()
                .flat_map(|s| s.series.iter())
                .filter(|q| q.files.len() > 1)
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// cd したフォルダの DICOM のシリーズを、1 シリーズ 1 スロットにまとめる。
    /// シリーズの画像を 1 枚ずつ並べたスロットは閉じる。CT は 1 シリーズが数百枚あり、
    /// 1 枚 1 スロットではスロットがそれで埋まってしまうため。
    ///
    /// ヘッダを読み終えるのは画像を並べ始めた後のことが多いので、読み終えたときにも呼ぶ。
    fn group_series(&mut self) {
        let Some(dir) = self.current.clone() else {
            return;
        };
        let active_id = self.slots[self.active].id;
        let mut first_new = None;
        for series in self.current_series() {
            let has_slot = self.slots.iter().any(|s| {
                s.sequence
                    .as_ref()
                    .is_some_and(|q| q.kind == SequenceKind::Series && q.paths == series.files)
            });
            if has_slot {
                continue;
            }
            for index in (0..self.slots.len()).rev() {
                let slot = &self.slots[index];
                if slot.from_folder && slot.path.as_ref().is_some_and(|p| series.files.contains(p))
                {
                    self.close_slot(index);
                }
            }
            // フォルダのスロットと、先に作ったシリーズのスロットの後ろに挟む。
            let at = self
                .slots
                .iter()
                .rposition(|s| s.folder.is_some() || s.sequence.is_some())
                .map_or(0, |i| i + 1);
            if !self.slots.get(at).is_some_and(is_blank) {
                self.insert_folder_slot(at);
            }
            let slot = &mut self.slots[at];
            slot.from_folder = false;
            let first = series.files[0].clone();
            slot.sequence = Some(Sequence::series(dir.clone(), series.title(), series.files));
            let full = self.cell_needs_full();
            self.start_load(at, first, full);
            first_new.get_or_insert(self.slots[at].id);
        }
        // 見ていた画像のスロットがシリーズにまとめられて消えたら、最初のシリーズを見せる。
        let alive = |id| self.slots.iter().position(|s| s.id == id);
        if alive(active_id).is_none() {
            if let Some(index) = first_new.and_then(alive) {
                self.active = index;
            }
        }
    }

    /// フォルダのスロットを開く。サブフォルダまで含めた画像をそのスロットに読み込む
    /// （サブフォルダがあれば先に確認する）。ZIP はその中へ移る。
    fn explore(&mut self, ctx: &egui::Context, index: usize) {
        let Some(folder) = self.slots[index].folder.clone() else {
            return;
        };
        if archive::is_zip(&folder) {
            self.open_folder(ctx, folder);
            return;
        }
        let tree = self.settings.settings.tree.clone();
        let slot = &mut self.slots[index];
        slot.sequence = Some(Sequence::subtree(folder.clone()));
        slot.loading = true;
        let id = slot.id;
        let ctx = ctx.clone();
        let load = FolderLoad::start(
            folder,
            self.loader.registry().clone(),
            tree.order(),
            true,
            tree.confirm_every,
            tree.confirm_chunk as usize,
            move || ctx.request_repaint(),
        );
        self.explores.push((id, load));
    }

    /// フォルダ（または ZIP）の中へ移る。ZIP は展開してから移る。
    fn open_folder(&mut self, ctx: &egui::Context, folder: PathBuf) {
        if archive::is_zip(&folder) {
            self.mount(ctx, &folder);
            return;
        }
        let tree = &self.settings.settings.tree;
        let other = ctx.input(|i| tree.other_load_modifier.held(i.modifiers));
        let into = if other {
            tree.load_into.other()
        } else {
            tree.load_into
        };
        self.cd(ctx, folder, into);
    }

    /// 1 つ上のフォルダへ移る。展開した ZIP のルートからは、ZIP のあるフォルダへ戻る。
    fn parent_folder(&mut self, ctx: &egui::Context) {
        let Some(current) = self.current.clone() else {
            return;
        };
        let parent = match self.archives.mount_of(&current) {
            Some(mount) if mount.dir == current => mount.zip.parent().map(Path::to_path_buf),
            _ => current.parent().map(Path::to_path_buf),
        };
        if let Some(parent) = parent {
            let into = self.settings.settings.tree.load_into;
            self.cd(ctx, parent, into);
        }
    }

    /// 読み込み中のフォルダで見つかった画像を、末尾のスロットに足していく。
    fn pump_folder_load(&mut self) {
        let Some(load) = &mut self.folder_load else {
            return;
        };
        let paths = load.take_ready();
        let finished = load.finished();
        let root = load.root.clone();
        let matcher = Matcher {
            filter: &self.filter,
            labels: &self.labels,
        };
        let in_series: HashSet<PathBuf> = if paths.is_empty() {
            HashSet::new()
        } else {
            self.current_series()
                .into_iter()
                .flat_map(|q| q.files)
                .collect()
        };
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| matcher.matches(p) && !in_series.contains(p))
            .collect();
        if !paths.is_empty() {
            // 先にスロットを全部足してから枠の大きさを見積もる。1 枚ずつ見積もると、
            // スロットが少ない最初のうちだけフル解像度を読んでしまうため。
            let mut reuse_last = is_blank(&self.slots[self.slots.len() - 1]);
            let mut targets = Vec::with_capacity(paths.len());
            for path in paths {
                let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
                let chunk = self.load_into == LoadInto::Folders || folder != root;
                if chunk {
                    let last = self.slots.last_mut().expect("there is always a slot");
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
                self.slots[index].from_folder = !chunk;
                if chunk {
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

    /// 開いたフォルダのスロットに、見つかった画像を足していく。
    fn pump_explores(&mut self) {
        let full = self.cell_needs_full();
        let mut i = 0;
        while i < self.explores.len() {
            let (id, load) = &mut self.explores[i];
            let id = *id;
            let paths = load.take_ready();
            let finished = load.finished();
            let Some(index) = self.slots.iter().position(|s| s.id == id) else {
                self.explores.remove(i);
                continue;
            };
            let matcher = self.matcher();
            let slot = &self.slots[index];
            let first = (slot.path.is_none())
                .then(|| paths.iter().position(|p| matcher.matches(p)))
                .flatten();
            let seq = self.slots[index]
                .sequence
                .as_mut()
                .expect("explored slots have a sequence");
            let offset = seq.paths.len();
            seq.paths.extend(paths);
            if let Some(at) = first {
                seq.index = offset + at;
                let path = seq.paths[offset + at].clone();
                self.start_load(index, path, full);
            }
            if finished {
                let (_, load) = self.explores.remove(i);
                let slot = &mut self.slots[index];
                if slot.path.is_none() {
                    // 合う画像がなかった。読み込み中のまま残さない。
                    slot.loading = false;
                    if slot.sequence.as_ref().is_some_and(|s| s.paths.is_empty()) {
                        // 画像が 1 枚もないフォルダは、空の並び（「1/0」）ではなくフォルダの姿に戻す。
                        slot.sequence = None;
                    }
                }
                self.log.info(format!(
                    "Loaded {} images from {}",
                    load.loaded,
                    load.root.display()
                ));
            } else {
                i += 1;
            }
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
    /// スロットからドラッグした画像 `file` がフォルダ `dir` にドロップされた。Ctrl を押していれば
    /// コピーしてすぐ終える。移動は確認してから行う。
    fn drop_file(&mut self, ctx: &egui::Context, file: PathBuf, dir: PathBuf) {
        // ドロップした瞬間の Ctrl だけで決めないのは、ボタンと Ctrl をほぼ同時に離すと
        // Ctrl を先に離したことになり、コピーと見せていたものが移動になるため。
        let copy = self.drag_copy || ctx.input(|i| i.modifiers.command);
        let op = if copy { Op::Copy } else { Op::Move };
        let t = Transfer {
            file,
            to_dir: dir,
            op,
        };
        if t.is_noop() {
            return;
        }
        // 展開した ZIP は一時フォルダの写しなので、中へ入れても ZIP には入らず、中から移しても
        // ZIP からは消えない。コピーで取り出すことだけ許す。
        let into_zip = self.archives.mount_of(&t.to_dir).is_some();
        let out_of_zip = op == Op::Move && self.archives.mount_of(&t.file).is_some();
        if into_zip || out_of_zip {
            self.log.warn(format!(
                "{}: files in a ZIP are read-only (Ctrl+drag copies them out)",
                t.file.display()
            ));
            return;
        }
        match op {
            Op::Copy => self.run_transfer(&t),
            Op::Move => self.pending_move = Some(t),
        }
    }

    fn run_transfer(&mut self, t: &Transfer) {
        let verb = match t.op {
            Op::Move => "Moved",
            Op::Copy => "Copied",
        };
        match t.run(&mut self.labels) {
            Ok(done) => {
                self.log.info(format!(
                    "{verb} {} to {}",
                    t.file.display(),
                    done.dest.display()
                ));
                if let Some(e) = done.label_error {
                    self.log.error(format!("Labels were not carried over: {e}"));
                }
            }
            Err(e) => self.log.error(format!("Cannot move or copy: {e}")),
        }
    }

    /// 今いるフォルダの悪いフラグの画像を集め、ごみ箱へ送ってよいか尋ねる。
    /// サブフォルダまで辿らないのは、見えていない画像まで消してしまわないため。
    fn ask_trash_bad(&mut self) {
        let lang = self.lang();
        let Some(dir) = self.current.clone() else {
            self.log
                .warn(tr(lang, "Select a folder in the tree first."));
            return;
        };
        if self.archives.mount_of(&dir).is_some() {
            self.log
                .warn(format!("{}: files in a ZIP are read-only", dir.display()));
            return;
        }
        let bad = Filter::Flag(Some(Flag::Bad));
        let matcher = Matcher {
            filter: &bad,
            labels: &self.labels,
        };
        let order = self.settings.settings.tree.order();
        let files = export::collect(&dir, false, self.loader.registry(), order, &matcher);
        if files.is_empty() {
            self.log.info(format!("No bad images in {}", dir.display()));
            return;
        }
        self.pending_trash = Some(discard::Pending { dir, files });
    }

    fn trash_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending_trash else {
            return;
        };
        let lang = self.lang();
        let (names, rest) = pending.preview(10);
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("trash_question")).show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.label(tr1(
                lang,
                "Move {} bad images to the trash?",
                &pending.files.len().to_string(),
            ));
            ui.label(egui::RichText::new(pending.dir.display().to_string()).monospace());
            ui.add_space(4.0);
            for name in names {
                ui.label(name);
            }
            if rest > 0 {
                ui.label(tr1(lang, "...and {} more", &rest.to_string()));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr(lang, "Move to trash")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr(lang, "Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        let Some(yes) = answer else {
            return;
        };
        if let Some(pending) = self.pending_trash.take().filter(|_| yes) {
            let ctx = ctx.clone();
            self.trash_job = Some(TrashJob::start(pending.files, move || {
                ctx.request_repaint()
            }));
        }
    }

    /// ごみ箱へ送り終えた画像のラベルを消し、結果をログに出す。スロットは
    /// フォルダの監視が消えた画像として閉じる。
    fn finish_trash_job(&mut self) {
        let Some(outcomes) = self.trash_job.as_ref().and_then(TrashJob::poll) else {
            return;
        };
        self.trash_job = None;
        let mut moved = 0;
        for (file, result) in outcomes {
            match result {
                Ok(()) => {
                    moved += 1;
                    if let Err(e) = self.labels.set(&file, Label::default()) {
                        self.log.error(format!("{}: {e}", file.display()));
                    }
                }
                Err(e) => self
                    .log
                    .error(format!("Cannot move {} to the trash: {e}", file.display())),
            }
        }
        self.log
            .info(format!("Moved {moved} bad images to the trash"));
    }

    /// ドロップした画像を移してよいか尋ねる。
    fn move_dialog(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.pending_move else {
            return;
        };
        let lang = self.lang();
        let name = t.file.file_name().unwrap_or_default().to_string_lossy();
        let message = tr1(lang, "Move \"{}\" to", &name);
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("move_question")).show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.label(message);
            ui.label(egui::RichText::new(t.to_dir.display().to_string()).monospace());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr(lang, "Move")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr(lang, "Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        let Some(yes) = answer else {
            return;
        };
        if let Some(t) = self.pending_move.take().filter(|_| yes) {
            self.run_transfer(&t);
        }
    }

    /// ドラッグ中の画像の名前と、移動かコピーかをマウスの横に出す。
    fn drag_preview(&mut self, ctx: &egui::Context) {
        let Some(file) = egui::DragAndDrop::payload::<DraggedFile>(ctx) else {
            self.drag_copy = false;
            return;
        };
        self.drag_copy = ctx.input(|i| i.modifiers.command);
        let Some(pointer) = ctx.pointer_latest_pos() else {
            return;
        };
        let lang = self.lang();
        let (icon, verb) = if self.drag_copy {
            (icon::COPY, tr(lang, "Copy"))
        } else {
            (icon::ARROW_RIGHT, tr(lang, "Move"))
        };
        let name = file.0.file_name().unwrap_or_default().to_string_lossy();
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        egui::Area::new(egui::Id::new("drag_preview"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pointer + vec2(14.0, 14.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(format!("{icon} {verb}: {name}"));
                });
            });
    }

    /// エクスプローラなどからドロップされたパスへ移る。ファイルならそのフォルダへ移って、その画像を選ぶ。
    /// ZIP は展開してその中へ移る。複数ドロップされたときは先頭だけを使う。
    fn receive_os_drop(&mut self, ctx: &egui::Context) {
        let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()))
        else {
            return;
        };
        if path.is_dir() || archive::is_zip(&path) {
            self.open_folder(ctx, path);
            return;
        }
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        self.open_folder(ctx, dir);
        self.select_when_loaded = Some(path);
    }

    fn select_dropped_image(&mut self) {
        let Some(path) = &self.select_when_loaded else {
            return;
        };
        if let Some(i) = self
            .slots
            .iter()
            .position(|s| s.path.as_ref() == Some(path))
        {
            self.active = i;
            self.select_when_loaded = None;
        } else if self.folder_load.is_none() {
            // 読み込みを終えても出てこない（対応していない形式など）。
            self.select_when_loaded = None;
        }
    }

    fn folder_load_dialog(&mut self, ctx: &egui::Context) {
        // cd したフォルダの読み込みを先に、次に開いたフォルダのスロットを開いた順に尋ねる。
        let asking = if self
            .folder_load
            .as_ref()
            .and_then(FolderLoad::question)
            .is_some()
        {
            Some(None)
        } else {
            self.explores
                .iter()
                .position(|(_, l)| l.question().is_some())
                .map(Some)
        };
        let Some(which) = asking else {
            return;
        };
        let load = match which {
            None => self.folder_load.as_ref(),
            Some(i) => Some(&self.explores[i].1),
        };
        let Some(load) = load else {
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
        let load = match which {
            None => self.folder_load.as_mut(),
            Some(i) => Some(&mut self.explores[i].1),
        };
        let Some(load) = load else {
            return;
        };
        match question {
            Question::Subfolders => load.answer_subfolders(yes),
            Question::More => {
                if !load.answer_more(yes) {
                    match which {
                        None => self.finish_folder_load(),
                        Some(i) => {
                            let (id, _) = self.explores.remove(i);
                            if let Some(slot) = self.slots.iter_mut().find(|s| s.id == id) {
                                slot.loading &= slot.path.is_some();
                            }
                        }
                    }
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
        let current = frames.get(start).cloned();
        let matcher = self.matcher();
        let frames: Vec<PathBuf> = frames
            .into_iter()
            .filter(|p| Some(p) == current.as_ref() || matcher.matches(p))
            .collect();
        let start = frames
            .iter()
            .position(|p| Some(p) == current.as_ref())
            .unwrap_or(0);
        if frames.len() < 2 {
            self.log.warn("Slideshow: there is only one image to show");
            return;
        }
        if self.maximized != Some(slot.id) {
            self.toggle_maximize(self.active);
        }
        let v = &self.settings.settings.viewer;
        // シネはファイルに書かれた速さで再生する。撮影時の速さで見ないと動きの評価を誤るため。
        let fps = self.slots[self.active]
            .sequence
            .as_ref()
            .and_then(|q| q.fps)
            .unwrap_or(v.slideshow_fps);
        let clock = Clock::new(start, frames.len(), v.slideshow_end, fps, Instant::now());
        let loader_slot = self.next_id;
        self.next_id += 1;
        self.last_dropped = 0;
        self.playback = Some(Playback::new(
            self.slots[self.active].id,
            loader_slot,
            frames,
            clock,
            fps,
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
                develop: false,
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
            developed: false,
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
            // 止まっているときも、再生したら使う速さ（シネやアニメーションならファイルの速さ）を出す。
            let current_fps = self.playback.as_ref().map_or_else(
                || {
                    self.slots
                        .get(self.active)
                        .and_then(|s| s.sequence.as_ref())
                        .and_then(|q| q.fps)
                        .unwrap_or(viewer.slideshow_fps)
                },
                |pb| pb.fps,
            );
            let mut fps = current_fps;
            let r = ui.add(fps_input(&mut fps));
            if r.changed() || r.drag_stopped() {
                // ドラッグ中は再生にだけ反映し、ファイルには離したときに書く。
                let save = !r.dragged();
                action = Some(BarAction::Fps { fps, save });
            }
            for preset in slideshow::FPS_PRESETS {
                let selected = (current_fps - preset).abs() < 0.05;
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
                if let Some(pb) = &mut self.playback {
                    pb.fps = fps;
                }
                // 止まっているときに変えた速さも、シネやアニメーションのファイルの速さより優先する。
                let seq = self
                    .slots
                    .get_mut(self.active)
                    .and_then(|s| s.sequence.as_mut());
                if let Some(q) = seq.filter(|q| q.fps.is_some()) {
                    q.fps = Some(fps);
                }
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
                .restart(pb.fps, viewer.slideshow_end, Instant::now());
        }
    }

    /// 画面に出すスロットとマス目。
    /// 画像フィールドにスロットを 1 つだけ出している（最大化か 1x1）。
    fn shows_one_slot(&self) -> bool {
        let view = self.view();
        view.cols * view.rows == 1
    }

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
    }

    /// 画像のスロット（フォルダや空のスロットでないもの）だけを辿って、アクティブなスロットを移す。
    /// 進めたら `true`。← → はスロットの中身を差し替えずにこれで移る。差し替えると、並んだ別の
    /// スロットと同じ画像になったり、どのスロットに何があったか分からなくなったりするため。
    /// スロットの中の画像はホイールで送る。
    fn step_image_slot(&mut self, delta: isize, wrap: bool) -> bool {
        let images: Vec<bool> = self
            .slots
            .iter()
            .map(|s| s.folder.is_none() && s.path.is_some())
            .collect();
        let Some(next) = image_slot_step(&images, self.active, delta, wrap) else {
            return false;
        };
        self.active = next;
        if self.maximized.is_some() {
            self.maximized = Some(self.slots[next].id);
        }
        true
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
    ///
    /// 絞り込み中は条件に合う画像だけを辿る。今の画像が条件に合わなくても、そこから前後に送れる。
    fn step_active(&mut self, delta: isize) {
        let slot = &self.slots[self.active];
        let Some(path) = slot.path.clone() else {
            return;
        };
        let files = match &slot.sequence {
            Some(seq) => seq.paths.clone(),
            None => {
                let order = self.settings.settings.tree.order();
                tree::sibling_images(&path, self.loader.registry(), order)
            }
        };
        let matcher = self.matcher();
        let shown: Vec<&PathBuf> = files
            .iter()
            .filter(|p| **p == path || matcher.matches(p))
            .collect();
        let Some(pos) = shown.iter().position(|p| **p == path) else {
            return;
        };
        let next = wrap_step(pos, shown.len(), delta);
        if next != pos {
            let target = shown[next].clone();
            self.open_in_active(target);
        }
    }

    /// 監視するフォルダ。ツリーで開いているフォルダと、スロットに出している画像のフォルダ。
    fn watched_dirs(&self) -> HashSet<PathBuf> {
        let mut dirs: HashSet<PathBuf> = self.tree.open_paths().into_iter().collect();
        dirs.extend(self.loaded_folders.iter().map(|(p, _)| p.clone()));
        for slot in &self.slots {
            if let Some(seq) = slot
                .sequence
                .as_ref()
                .filter(|q| matches!(q.kind, SequenceKind::Folder | SequenceKind::Subtree))
            {
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
            self.apply_changes(ctx, changes);
        }
    }

    fn apply_changes(&mut self, ctx: &egui::Context, changes: Vec<watch::Change>) {
        let registry = self.loader.registry().clone();
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut created = HashSet::new();
        let mut modified = HashSet::new();
        let mut removed = HashSet::new();
        for c in changes {
            // 閉じた ZIP の展開先を消したときの変化。画像が消えたとログに出さない。
            if self.archives.is_closed(&c.path) {
                continue;
            }
            if let Some(dir) = c.path.parent().filter(|d| !dirs.iter().any(|x| x == d)) {
                dirs.push(dir.to_path_buf());
            }
            // ラベルファイルが外で書き換えられたら（別の PC との同期など）、次に使うときに読み直す。
            if c.path
                .file_name()
                .is_some_and(|n| n == crate::labels::FILE_NAME)
            {
                if let Some(dir) = c.path.parent() {
                    self.labels.forget(dir);
                }
                continue;
            }
            // 消えたファイルは中身を読めないので、拡張子で決まらないものは画像だったとみなす。
            // 画像でないファイルが消えても、フォルダを読み直して件数をログに出すだけで済む。
            let image = match c.kind {
                ChangeKind::Removed => {
                    registry.supports_path(&c.path) || registry.needs_probe(&c.path)
                }
                _ => registry.supports_file(&c.path),
            };
            if !image {
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
            self.series.forget(dir);
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
        if let Some(current) = self.current.clone().filter(|c| dirs.contains(c)) {
            self.sync_folder_slots(&current, &registry);
            let images =
                tree::list_folder(&current, &registry, self.settings.settings.tree.order()).images;
            self.scan_series(ctx, &current, &images);
        }
        // フォルダを読み込み中は、スロットの並びを読み込みのほうに任せる。並びを読み直すと、
        // まだ読み込みが足していない画像と重なるため。
        if self.folder_load.is_some() {
            return;
        }
        let matcher = self.matcher();
        created.retain(|p| matcher.matches(p));
        let order = self.settings.settings.tree.order();
        for dir in &dirs {
            self.sync_loaded_folder(dir, &created, &removed, &registry);
        }
        let playing = self.playback.as_ref().map(|pb| pb.slot);
        let one_up = self.shows_one_slot();
        let active = self.active;
        let mut siblings: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        let mut loads = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if playing == Some(slot.id) || slot.from_folder {
                continue;
            }
            let Some(current) = slot.path.clone() else {
                continue;
            };
            // フレームの並びは元のファイルの中のもの。フォルダの画像に並べ替えず、ファイルが
            // 書き換わったときに今のフレームを読み直すだけにする。
            if let Some((file, _)) = frames::split(&current) {
                if modified.contains(&file) {
                    loads.push((index, current));
                }
                continue;
            }
            // シリーズの並びは UID で決まり、フォルダの並び順とは関係ない。ファイルが
            // 書き換わったときに読み直すだけにする。
            if slot
                .sequence
                .as_ref()
                .is_some_and(|q| q.kind == SequenceKind::Series)
            {
                if modified.contains(&current) {
                    loads.push((index, current));
                }
                continue;
            }
            // 開いたフォルダのスロットはサブフォルダの画像も持つので、フォルダを読み直して並べ直す
            // ことはせず、増えた画像を後ろに足し、消えた画像を除くだけにする。
            if let Some(seq) = slot
                .sequence
                .as_mut()
                .filter(|q| q.kind == SequenceKind::Subtree)
            {
                seq.paths.retain(|p| !removed.contains(p));
                let mut new: Vec<&PathBuf> = created
                    .iter()
                    .filter(|p| p.starts_with(&seq.folder) && !seq.paths.contains(p))
                    .collect();
                new.sort();
                seq.paths.extend(new.into_iter().cloned());
                match seq.paths.iter().position(|p| *p == current) {
                    Some(at) => {
                        seq.index = at;
                        if modified.contains(&current) {
                            loads.push((index, current));
                        }
                    }
                    None if !seq.paths.is_empty() => {
                        seq.index = seq.index.min(seq.paths.len() - 1);
                        loads.push((index, seq.paths[seq.index].clone()));
                    }
                    None => {}
                }
                continue;
            }
            let Some(dir) = current.parent().filter(|d| dirs.iter().any(|x| x == d)) else {
                continue;
            };
            // 同じフォルダの画像を出しているスロットが多くても、フォルダは 1 回だけ読む。
            let paths = siblings
                .entry(dir.to_path_buf())
                .or_insert_with(|| tree::list_folder(dir, &registry, order).images);
            if paths.is_empty() {
                continue;
            }
            // 1 枚だけ表示しているときは、見ている位置に関わらず新しい画像を出す。見比べる
            // 相手がいないので、来た画像をすぐ見られるほうが役に立つ。
            let newest = (one_up && index == active)
                .then(|| paths.iter().rposition(|p| created.contains(p)))
                .flatten();
            let at = newest
                .or_else(|| watch::follow_newest(paths, &current, &created))
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
            self.start_load(index, path, full);
        }
    }

    /// cd したフォルダのサブフォルダと ZIP のスロットを、フォルダの今の中身に合わせる。
    /// 消えたもののスロットは閉じ、新しいものはフォルダのスロットの最後に足す。
    fn sync_folder_slots(&mut self, current: &Path, registry: &Registry) {
        let listing = tree::list_folder(current, registry, self.settings.settings.tree.order());
        let wanted: Vec<PathBuf> = listing
            .folders
            .into_iter()
            .chain(listing.archives)
            .collect();
        for index in (0..self.slots.len()).rev() {
            let gone = self.slots[index]
                .folder
                .as_ref()
                .is_some_and(|f| !wanted.contains(f));
            if gone {
                self.close_slot(index);
            }
        }
        for folder in wanted {
            if self
                .slots
                .iter()
                .any(|s| s.folder.as_ref() == Some(&folder))
            {
                continue;
            }
            let at = self
                .slots
                .iter()
                .rposition(|s| s.folder.is_some())
                .map_or(0, |i| i + 1);
            if self.slots.get(at).is_some_and(is_blank) {
                self.slots[at].folder = Some(folder);
            } else {
                self.insert_folder_slot(at);
                let slot = &mut self.slots[at];
                slot.from_folder = false;
                slot.folder = Some(folder);
            }
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
        let last = self.slots.iter().rposition(in_dir);
        let root = self
            .loaded_folders
            .iter()
            .find(|(p, _)| p == dir)
            .map(|(_, into)| *into);
        // 読み込んだ時点で画像がなかったフォルダには、まだこのフォルダのスロットがない。
        if last.is_none() && root.is_none() {
            return;
        }
        let order = self.settings.settings.tree.order();
        let new: Vec<PathBuf> = tree::list_folder(dir, registry, order)
            .images
            .into_iter()
            .filter(|p| created.contains(p))
            .collect();
        if new.is_empty() {
            return;
        }
        if last.is_none() && root == Some(LoadInto::Folders) {
            let has_slot = self
                .slots
                .iter()
                .any(|s| s.sequence.as_ref().is_some_and(|q| q.folder == dir));
            if !has_slot {
                self.add_folder_slot(dir, registry);
            }
            return;
        }
        // 最後の画像か、まだ何も出していないスロットを見ていたなら、新しい画像に移る。
        let follow = match last {
            Some(last) => self.active == last || self.shows_one_slot(),
            None => is_blank(&self.slots[self.active]),
        };
        let followed_id = self.slots[self.active].id;
        let mut index = last.map_or(self.slots.len(), |l| l + 1);
        // 読み込みで使い回すのと同じく、末尾の空のスロットは新しい画像に使う。
        if last.is_none() && is_blank(&self.slots[self.slots.len() - 1]) {
            index -= 1;
            self.slots[index].from_folder = true;
        } else {
            self.insert_folder_slot(index);
        }
        let full = self.cell_needs_full();
        for (i, path) in new.into_iter().enumerate() {
            if i > 0 {
                index += 1;
                self.insert_folder_slot(index);
            }
            self.start_load(index, path, full);
        }
        if follow {
            self.active = index;
            if self.maximized == Some(followed_id) {
                self.maximized = Some(self.slots[index].id);
            }
        }
    }

    /// 1 枚 1 スロットの読み込みのスロットを `index` に挟む。
    fn insert_folder_slot(&mut self, index: usize) {
        let mut slot = Slot::new(self.next_id);
        self.next_id += 1;
        slot.from_folder = true;
        self.slots.insert(index, slot);
        if self.active >= index && self.active + 1 < self.slots.len() {
            self.active += 1;
        }
    }

    /// 読み込んだ時点で空だったフォルダに画像が来たら、フォルダスロットを作って最新の画像を出す。
    fn add_folder_slot(&mut self, dir: &Path, registry: &Registry) {
        let order = self.settings.settings.tree.order();
        let paths = tree::list_folder(dir, registry, order).images;
        let Some(newest) = paths.last().cloned() else {
            return;
        };
        let follow = is_blank(&self.slots[self.active]);
        let index = self.push_slot();
        let mut seq = Sequence::new(dir.to_path_buf(), newest.clone());
        seq.index = paths.len() - 1;
        seq.paths = paths;
        self.slots[index].sequence = Some(seq);
        if follow {
            self.active = index;
        }
        let full = self.cell_needs_full();
        self.start_load(index, newest, full);
    }

    /// 絞り込みの条件。
    fn matcher(&self) -> Matcher<'_> {
        Matcher {
            filter: &self.filter,
            labels: &self.labels,
        }
    }

    /// 絞り込みを変え、読み込んだフォルダのスロットにもすぐ効かせる。
    fn set_filter(&mut self, ctx: &egui::Context, filter: Filter) {
        self.log.info(format!(
            "Filter: {}",
            filter_name(&filter, Language::English)
        ));
        self.filter = filter;
        self.apply_filter_to_slots();
        ctx.request_repaint();
    }

    /// 絞り込みを変えたときに、フォルダの読み込みで並べたスロットを条件に合わせる。
    /// 自分で開いた画像のスロットはそのままにする。
    fn apply_filter_to_slots(&mut self) {
        let registry = self.loader.registry().clone();
        let order = self.settings.settings.tree.order();
        let roots: Vec<PathBuf> = self
            .loaded_folders
            .iter()
            .filter(|(_, into)| *into == LoadInto::Images)
            .map(|(p, _)| p.clone())
            .collect();
        for root in roots {
            let matcher = self.matcher();
            let wanted: Vec<PathBuf> = tree::list_folder(&root, &registry, order)
                .images
                .into_iter()
                .filter(|p| matcher.matches(p))
                .collect();
            self.rearrange_folder_slots(&root, wanted);
        }
        let full = self.cell_needs_full();
        for index in 0..self.slots.len() {
            let Some(seq) = &self.slots[index].sequence else {
                continue;
            };
            let matcher = self.matcher();
            let target = nearest_match(&seq.paths, seq.index, |p| matcher.matches(p));
            let Some(target) = target else {
                // 合う画像がなければ空にする。条件に合わない画像を出したままにすると、
                // 絞り込みが効いていないように見えるため。並びは残して、条件を戻したら出し直す。
                self.cancel_requests(&self.slots[index]);
                let slot = &mut self.slots[index];
                slot.path = None;
                slot.shown = None;
                slot.loading = false;
                slot.full_request = None;
                continue;
            };
            let path = seq.paths[target].clone();
            if self.slots[index].path.as_ref() == Some(&path) {
                continue;
            }
            if let Some(seq) = &mut self.slots[index].sequence {
                seq.index = target;
            }
            self.start_load(index, path, full);
        }
    }

    /// `root` の画像を 1 枚 1 スロットに並べたスロットを、`wanted` の画像に並べ直す。
    /// 残る画像のスロットは読み直さずにそのまま使う。
    fn rearrange_folder_slots(&mut self, root: &Path, wanted: Vec<PathBuf>) {
        let in_root =
            |s: &Slot| s.from_folder && s.path.as_deref().and_then(Path::parent) == Some(root);
        let active_id = self.slots[self.active].id;
        let mut at = self.slots.iter().position(in_root);
        let mut kept = HashMap::new();
        let mut rest = Vec::with_capacity(self.slots.len());
        for slot in std::mem::take(&mut self.slots) {
            if in_root(&slot) {
                kept.insert(slot.path.clone().expect("checked by in_root"), slot);
            } else {
                rest.push(slot);
            }
        }
        // 前に全部隠れていたときは、cd したときと同じくフォルダとシリーズのスロットの後ろに並べる。
        // 末尾の空のスロットがあれば、そこから並べる。
        let at = *at.get_or_insert_with(|| {
            if rest.last().is_some_and(is_blank) && !wanted.is_empty() {
                rest.pop();
            }
            rest.iter()
                .rposition(|s| s.folder.is_some() || s.sequence.is_some())
                .map_or(0, |i| i + 1)
        });
        let mut new = Vec::with_capacity(wanted.len());
        let mut loads = Vec::new();
        for path in wanted {
            match kept.remove(&path) {
                Some(slot) => new.push(slot),
                None => {
                    let mut slot = Slot::new(self.next_id);
                    self.next_id += 1;
                    slot.from_folder = true;
                    loads.push((slot.id, path));
                    new.push(slot);
                }
            }
        }
        let first_id = new.first().map(|s| s.id);
        rest.splice(at..at, new);
        self.slots = rest;
        for slot in kept.into_values() {
            self.cancel_requests(&slot);
        }
        if self.slots.is_empty() {
            self.slots.push(Slot::new(self.next_id));
            self.next_id += 1;
        }
        let find = |id| self.slots.iter().position(|s| s.id == id);
        self.active = find(active_id)
            .or_else(|| first_id.and_then(find))
            .unwrap_or(at.min(self.slots.len() - 1));
        if let Some(id) = self.maximized {
            if find(id).is_none() {
                self.maximized = Some(self.slots[self.active].id);
            }
        }
        let full = self.cell_needs_full();
        for (id, path) in loads {
            if let Some(index) = self.slots.iter().position(|s| s.id == id) {
                self.start_load(index, path, full);
            }
        }
    }

    /// サイドバーの絞り込みボタンの色。絞り込んでいなければ `None`（普段の色）。
    fn filter_color(&self) -> Option<Color32> {
        match &self.filter {
            Filter::All => None,
            Filter::Flag(Some(flag)) => Some(theme::flag_color(*flag)),
            Filter::Flag(None) => Some(theme::UNFLAGGED),
            Filter::Tag(tag) => {
                let tags = &self.settings.settings.labels.tags;
                tags.iter().position(|t| t == tag).map(theme::tag_color)
            }
        }
    }

    /// cd したフォルダ（なければアクティブな画像のフォルダ）を対象に、書き出しの画面を開く。
    fn open_export(&mut self) {
        let root = self.current.clone().or_else(|| {
            self.active_image()
                .and_then(|p| p.parent().map(Path::to_path_buf))
        });
        let Some(root) = root else {
            self.log
                .warn(tr(self.lang(), "Select a folder in the tree first."));
            return;
        };
        let name = root
            .file_name()
            .map_or_else(|| "images".into(), |n| n.to_string_lossy().into_owned());
        let suffix = match &self.filter {
            Filter::All => "export".to_owned(),
            Filter::Flag(Some(Flag::Good)) => "good".to_owned(),
            Filter::Flag(Some(Flag::Bad)) => "bad".to_owned(),
            Filter::Flag(None) => "unflagged".to_owned(),
            Filter::Tag(t) => t.clone(),
        };
        let beside = root.parent().unwrap_or(&root);
        self.export = Some(ExportDialog {
            dest: beside
                .join(format!("{name}-{suffix}"))
                .display()
                .to_string(),
            list_path: beside
                .join(format!("{name}-{suffix}.csv"))
                .display()
                .to_string(),
            root,
            recursive: true,
            kind: ExportKind::Copy,
            keep_structure: true,
            job: None,
        });
    }

    /// 書き出しの画面。
    fn export_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.export else {
            return;
        };
        let lang = self.settings.settings.appearance.language;
        let filter_text = filter_name(&self.filter, lang);
        let mut open = true;
        let mut run = false;
        let mut cancel_job = false;
        egui::Window::new(tr(lang, "Export images"))
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("export-grid")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label(tr(lang, "Folder"));
                        ui.label(dialog.root.display().to_string());
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut dialog.recursive, tr(lang, "Include subfolders"));
                        ui.end_row();
                        ui.label(tr(lang, "Images shown"));
                        ui.label(format!("{}: {filter_text}", tr(lang, "Filter")));
                        ui.end_row();
                    });
                ui.separator();
                for (kind, title) in [
                    (ExportKind::Copy, "Copy files to a folder"),
                    (ExportKind::List(ListFormat::Csv), "CSV list"),
                    (ExportKind::List(ListFormat::Jsonl), "JSONL list"),
                ] {
                    if ui
                        .radio_value(&mut dialog.kind, kind, tr(lang, title))
                        .changed()
                    {
                        let ext = match kind {
                            ExportKind::List(ListFormat::Jsonl) => "jsonl",
                            _ => "csv",
                        };
                        dialog.list_path = Path::new(&dialog.list_path)
                            .with_extension(ext)
                            .display()
                            .to_string();
                    }
                }
                // ファイル選択ダイアログを使わず文字で入力させるのは、Linux で OS のダイアログを
                // 出すには GTK などへの依存が増え、起動の速さと配布の手軽さを損なうため。
                match dialog.kind {
                    ExportKind::Copy => {
                        ui.label(tr(lang, "Destination folder"));
                        ui.add(egui::TextEdit::singleline(&mut dialog.dest).desired_width(420.0));
                        ui.checkbox(
                            &mut dialog.keep_structure,
                            tr(lang, "Keep folder structure"),
                        );
                    }
                    ExportKind::List(_) => {
                        ui.label(tr(lang, "Output file"));
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.list_path).desired_width(420.0),
                        );
                    }
                }
                ui.separator();
                match &dialog.job {
                    Some(job) if !job.finished.load(std::sync::atomic::Ordering::Relaxed) => {
                        let done = job.done.load(std::sync::atomic::Ordering::Relaxed);
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / job.total.max(1) as f32)
                                    .text(format!("{done} / {}", job.total))
                                    .desired_width(300.0),
                            );
                            cancel_job = ui.button(tr(lang, "Cancel")).clicked();
                        });
                    }
                    _ => {
                        run = ui.button(tr(lang, "Export")).clicked();
                    }
                }
            });
        if cancel_job {
            if let Some(job) = &dialog.job {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if !open {
            if let Some(job) = &dialog.job {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.export = None;
            return;
        }
        if run {
            self.run_export(ctx);
        }
        self.finish_copy_job();
    }

    /// 書き出しを始める。一覧はその場で書き、コピーは別スレッドで行う。
    fn run_export(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.export else {
            return;
        };
        let order = self.settings.settings.tree.order();
        let files = export::collect(
            &dialog.root,
            dialog.recursive,
            self.loader.registry(),
            order,
            &self.matcher(),
        );
        let dialog = self.export.as_mut().expect("checked above");
        match dialog.kind {
            ExportKind::List(format) => {
                let items: Vec<_> = files
                    .into_iter()
                    .map(|f| {
                        let label = self.labels.get(&f);
                        (f, label)
                    })
                    .collect();
                let out = PathBuf::from(dialog.list_path.trim());
                match export::write_list(&out, format, &items) {
                    Ok(()) => self.log.info(format!(
                        "Export: wrote {} images to {}",
                        items.len(),
                        out.display()
                    )),
                    Err(e) => self.log.error(format!("Export: {e}")),
                }
            }
            ExportKind::Copy => {
                let dest = PathBuf::from(dialog.dest.trim());
                let mut taken = HashSet::new();
                let pairs: Vec<_> = files
                    .into_iter()
                    .map(|f| {
                        let to = export::copy_target(
                            &dialog.root,
                            &f,
                            &dest,
                            dialog.keep_structure,
                            &mut taken,
                        );
                        (f, to)
                    })
                    .collect();
                self.log.info(format!(
                    "Export: copying {} images to {}",
                    pairs.len(),
                    dest.display()
                ));
                let ctx = ctx.clone();
                dialog.job = Some(CopyJob::start(pairs, move || ctx.request_repaint()));
            }
        }
    }

    /// 終わったコピーの結果をログに出す。
    fn finish_copy_job(&mut self) {
        let Some(dialog) = &mut self.export else {
            return;
        };
        let Some(job) = &dialog.job else {
            return;
        };
        if !job.finished.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let failed = std::mem::take(&mut *job.failed.lock().unwrap());
        let done = job.done.load(std::sync::atomic::Ordering::Relaxed);
        dialog.job = None;
        self.log.info(format!(
            "Export: copied {} of {} images",
            done - failed.len(),
            done
        ));
        for e in failed {
            self.log.error(format!("Export: {e}"));
        }
    }

    /// アクティブなスロットの画像。
    fn active_image(&self) -> Option<PathBuf> {
        self.slots[self.active].path.clone()
    }

    /// ツリーで DICOM のシリーズを選んだ。そのフォルダへ移り、シリーズのスロットを選ぶ。
    fn show_series(&mut self, ctx: &egui::Context, folder: PathBuf, files: &[PathBuf]) {
        if self.current.as_ref() != Some(&folder) {
            let into = self.settings.settings.tree.load_into;
            self.cd(ctx, folder, into);
        }
        let found = self.slots.iter().position(|s| match &s.sequence {
            Some(q) => q.kind == SequenceKind::Series && q.paths == files,
            None => s.path.as_ref().is_some_and(|p| files.contains(p)),
        });
        if let Some(index) = found {
            self.active = index;
            if self.maximized.is_some() {
                self.maximized = Some(self.slots[index].id);
            }
        }
    }

    /// ZIP をツリーで開く。展開は別スレッドで行い、終わったら [`Self::receive_archives`] で
    /// ルートに足す。
    fn mount(&mut self, ctx: &egui::Context, zip: &Path) {
        if let Some(mount) = self.archives.mounted_zip(zip) {
            let (dir, into) = (mount.dir.clone(), self.settings.settings.tree.load_into);
            self.cd(ctx, dir, into);
            return;
        }
        let ctx = ctx.clone();
        if self.archives.mount(zip, move || ctx.request_repaint()) {
            self.log.info(format!("Extracting {}…", zip.display()));
        }
    }

    /// 展開し終えた ZIP をツリーのルートに足し、その中へ移る。
    fn receive_archives(&mut self, ctx: &egui::Context) {
        for event in self.archives.poll() {
            match event {
                archive::Event::Mounted { mount, files } => {
                    self.log
                        .info(format!("Opened {} ({files} files)", mount.zip.display()));
                    self.tree.add_mounted_root(mount.dir.clone(), mount.name());
                    let into = self.settings.settings.tree.load_into;
                    self.cd(ctx, mount.dir, into);
                }
                archive::Event::Failed { zip, error } => {
                    self.log
                        .error(format!("Cannot open {}: {error}", zip.display()));
                }
            }
        }
    }

    /// 展開した ZIP を閉じる。中の画像を出しているスロットも閉じる。展開したファイルを消すので、
    /// スロットに残しておいても読み直せなくなるため。ZIP の中にいたら、ZIP のあるフォルダへ戻る。
    fn unmount(&mut self, ctx: &egui::Context, dir: &Path) {
        if self.current.as_deref().is_some_and(|c| c.starts_with(dir)) {
            let back = self
                .archives
                .mount_of(dir)
                .and_then(|m| m.zip.parent())
                .map(Path::to_path_buf);
            self.finish_folder_load();
            self.close_all_slots();
            self.current = None;
            if let Some(back) = back {
                let into = self.settings.settings.tree.load_into;
                self.cd(ctx, back, into);
            }
        }
        for index in (0..self.slots.len()).rev() {
            let slot = &self.slots[index];
            let inside = slot.path.as_deref().is_some_and(|p| p.starts_with(dir))
                || slot
                    .sequence
                    .as_ref()
                    .is_some_and(|q| q.folder.starts_with(dir));
            if inside {
                self.close_slot(index);
            }
        }
        self.loaded_folders.retain(|(p, _)| !p.starts_with(dir));
        self.tree.remove_root(dir);
        if let Some(mount) = self.archives.unmount(dir) {
            self.log.info(format!("Closed {}", mount.zip.display()));
        }
    }

    /// ラベルを付けられる画像か。ZIP の中はラベルファイルを書く場所がなく、閉じると消えるので付けない。
    fn labelable(&mut self, path: &Path) -> bool {
        if let Some(mount) = self.archives.mount_of(path) {
            let zip = mount.zip.display().to_string();
            self.log.warn(format!(
                "Flags and tags cannot be put on images inside {zip}"
            ));
            return false;
        }
        true
    }

    /// フラグを付ける・外す。
    fn flag(&mut self, flag: Flag) {
        let Some(path) = self.active_image().filter(|p| !p.is_dir()) else {
            return;
        };
        if !self.labelable(&path) {
            return;
        }
        match self.labels.toggle_flag(&path, flag) {
            Ok(now) => {
                let what = match now {
                    Some(Flag::Good) => "flagged good",
                    Some(Flag::Bad) => "flagged bad",
                    None => "flag removed",
                };
                self.log.info(format!("{}: {what}", path.display()));
                if now.is_some() {
                    self.advance_after_label();
                }
            }
            Err(e) => self.log.error(e),
        }
    }

    /// 設定の `index` 番目のタグを付ける・外す。
    fn tag(&mut self, index: usize) {
        let Some(tag) = self.settings.settings.labels.tags.get(index).cloned() else {
            self.log.warn(format!(
                "No tag for key {} (add tags in Settings > Flags and tags)",
                index + 1
            ));
            return;
        };
        let Some(path) = self.active_image().filter(|p| !p.is_dir()) else {
            return;
        };
        if !self.labelable(&path) {
            return;
        }
        match self.labels.toggle_tag(&path, &tag) {
            Ok(added) => {
                let what = if added { "tagged" } else { "untagged" };
                self.log.info(format!("{}: {what} {tag}", path.display()));
                if added {
                    self.advance_after_label();
                }
            }
            Err(e) => self.log.error(e),
        }
    }

    /// ラベルを付けた後に次の画像へ進む（設定で止められる）。1 枚 1 スロットに並べているときは
    /// 次のスロットへ、そうでなければスロットの中で次の画像へ。最後まで来たら先頭に戻らずに止まる。
    /// 戻ると、付け終わったことに気づかずに最初の画像のラベルを付け替えてしまうため。
    fn advance_after_label(&mut self) {
        if !self.settings.settings.labels.advance {
            return;
        }
        let slot = &self.slots[self.active];
        if slot.sequence.is_none() && self.slots.len() > 1 {
            self.step_image_slot(1, false);
            return;
        }
        let at_end = match &slot.sequence {
            Some(seq) => seq.index + 1 >= seq.paths.len(),
            None => slot.path.as_ref().is_none_or(|p| {
                let order = self.settings.settings.tree.order();
                let files = tree::sibling_images(p, self.loader.registry(), order);
                files.last() == Some(p)
            }),
        };
        if !at_end {
            self.step_active(1);
        }
    }

    /// タグ付けモードの間、画像の下に出すバー。アクティブなスロットの画像のフラグとタグを示し、
    /// クリックでも付け外しできる。
    fn tag_bar(&self, ui: &mut egui::Ui) -> Option<TagClick> {
        let lang = self.lang();
        let label = self.slots[self.active]
            .path
            .as_deref()
            .map(|p| self.labels.get(p))
            .unwrap_or_default();
        let in_zip = self.slots[self.active]
            .path
            .as_deref()
            .and_then(|p| self.archives.mount_of(p));
        let mut clicked = None;
        let frame = egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(6, 3));
        frame.show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.horizontal_centered(|ui| {
                if in_zip.is_some() {
                    ui.disable();
                }
                for (flag, action, title) in [
                    (Flag::Good, Action::FlagGood, "Good"),
                    (Flag::Bad, Action::FlagBad, "Bad"),
                ] {
                    let text = egui::RichText::new(format!(
                        "{} {}",
                        crate::labels::flag_icon(flag),
                        tr(lang, title)
                    ))
                    .color(theme::flag_color(flag));
                    let tip = self.with_key(tr(lang, action.title()), action);
                    if ui
                        .add(egui::Button::selectable(label.flag == Some(flag), text))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        clicked = Some(TagClick::Flag(flag));
                    }
                }
                ui.separator();
                let tags = &self.settings.settings.labels.tags;
                if tags.is_empty() {
                    ui.weak(tr(lang, "Add tags in Settings > Flags and tags"));
                }
                if in_zip.is_some() {
                    ui.separator();
                    ui.weak(tr(lang, "Images inside a ZIP cannot be labeled"));
                }
                for (i, tag) in tags.iter().enumerate() {
                    let key = Action::ALL.into_iter().find(|a| a.tag_index() == Some(i));
                    let text = match key {
                        Some(_) => format!("{} {tag}", i + 1),
                        None => tag.clone(),
                    };
                    let text = egui::RichText::new(text).color(theme::tag_color(i));
                    let on = label.tags.contains(tag);
                    if ui.add(egui::Button::selectable(on, text)).clicked() {
                        clicked = Some(TagClick::Tag(i));
                    }
                }
            });
        });
        clicked
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
                self.step_image_slot(-1, true);
            }
            Action::NextImage => {
                self.stop_playback();
                self.step_image_slot(1, true);
            }
            Action::ToggleTagging => self.tagging ^= true,
            Action::FilterAll => self.set_filter(ctx, Filter::All),
            Action::FilterGood => self.set_filter(ctx, Filter::Flag(Some(Flag::Good))),
            Action::FilterBad => self.set_filter(ctx, Filter::Flag(Some(Flag::Bad))),
            Action::FilterUnflagged => self.set_filter(ctx, Filter::Flag(None)),
            Action::ExportImages => self.open_export(),
            Action::TrashBadImages => self.ask_trash_bad(),
            Action::FlagGood => self.flag(Flag::Good),
            Action::FlagBad => self.flag(Flag::Bad),
            Action::Tag1
            | Action::Tag2
            | Action::Tag3
            | Action::Tag4
            | Action::Tag5
            | Action::Tag6
            | Action::Tag7
            | Action::Tag8
            | Action::Tag9 => {
                if let Some(i) = action.tag_index() {
                    self.tag(i);
                }
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
            // フォルダのスロットで Enter を押したらその中へ移る。別の操作にしないのは、キーの
            // 割り当てが操作ごとに重ならない決まりで、同じ Enter を両方に割り当てられないため。
            Action::ToggleMaximize => match self.slots[self.active].folder.clone() {
                Some(folder) => self.open_folder(ctx, folder),
                None => self.toggle_maximize(self.active),
            },
            Action::ParentFolder => self.parent_folder(ctx),
            Action::FitImage => self.slots[self.active].reset_view(),
            Action::ActualSize => self.slots[self.active].actual_size(),
            Action::DevelopRaw => self.develop_slot(self.active),
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
            if action.needs_tagging_mode() && !self.tagging {
                continue;
            }
            self.run(ctx, action);
        }
    }

    /// 設定画面で値が変わったときに、反映して保存する。
    fn settings_changed(&mut self, ctx: &egui::Context) {
        theme::apply(ctx, &self.settings.settings.appearance);
        self.tree.set_order(self.settings.settings.tree.order());
        self.apply_probe_extensions();
        match self.settings.save() {
            Ok(Some(backup)) => self.log.info(format!(
                "Settings: the previous file had errors and was kept as {}",
                backup.display()
            )),
            Ok(None) => {}
            Err(e) => self.log.error(format!("Settings: {e}")),
        }
    }

    /// DICOM か調べる拡張子が変わったら、開いているフォルダを読み直してツリーにすぐ反映する。
    fn apply_probe_extensions(&mut self) {
        let dicom = &self.settings.settings.dicom;
        if dicom.probe_extensions == self.probe_extensions {
            return;
        }
        self.probe_extensions = dicom.probe_extensions.clone();
        let registry = self.loader.registry().clone();
        registry.set_probe_extensions(dicom.probe());
        self.tree.forget_peeks();
        for dir in self.tree.open_paths() {
            self.tree.refresh(&dir, &registry);
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
                self.apply_probe_extensions();
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
            // 展開した ZIP の中は終了時に消えるので、次回そこへは戻れない。
            current: self
                .current
                .clone()
                .filter(|c| self.archives.mount_of(c).is_none()),
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
            let tip = self.with_key(tr(lang, "Tagging mode"), Action::ToggleTagging);
            let button = egui::Button::selectable(self.tagging, big(icon::TAG)).min_size(size);
            if ui.add(button).on_hover_text(tip).clicked() {
                self.tagging ^= true;
            }
            let mut text = big(icon::FUNNEL);
            if let Some(color) = self.filter_color() {
                text = text.color(color);
            }
            let tip = format!(
                "{}: {}\n{}",
                tr(lang, "Filter"),
                filter_name(&self.filter, lang),
                tr(lang, "Click to change")
            );
            let button = egui::Button::selectable(!self.filter.is_all(), text).min_size(size);
            if ui.add(button).on_hover_text(tip).clicked() {
                let next = self.filter.next(&self.settings.settings.labels.tags);
                self.set_filter(ui.ctx(), next);
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
                    Ok(mut loaded) => {
                        self.receive_developed(ctx, index, &r.path, &mut loaded);
                        if let Some(full) = loaded.full {
                            self.attach_full(ctx, index, full);
                        }
                    }
                    Err(e) => self.log.error(format!("{}: {e}", r.path.display())),
                }
            }
        }
    }

    /// 現像した画像が届いたら、表示中の画像の情報（大きさ・メタデータ）と縮小画像を差し替える。
    /// 縮小画像も差し替えるのは、並べて見たときにプレビューと現像の色が混ざらないようにするため。
    /// 現像し直した（捨てたあと読み直した）ときは同じなので、何もしない。
    fn receive_developed(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        path: &Path,
        loaded: &mut Loaded,
    ) {
        let Some(shown) = &mut self.slots[index].shown else {
            return;
        };
        if !shown.developing() {
            return;
        }
        let info = &loaded.info;
        shown.info = info.clone();
        if let Some(thumb) = loaded.thumb.take() {
            shown.thumb = ctx.load_texture(
                format!("{}#thumb", path.display()),
                thumb,
                texture_options(),
            );
        }
        self.log.info(format!(
            "{}: developed {}x{} | read {} / develop {} / convert {}",
            path.display(),
            info.width,
            info.height,
            ms(info.read),
            ms(info.decode),
            ms(info.convert),
        ));
    }

    /// スロットで開いた画像の読み込み結果を受け取る。
    fn receive_image(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        path: PathBuf,
        result: Result<Loaded, String>,
    ) {
        let keep_view_setting = self.settings.settings.viewer.keep_view;
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
        // 1 枚のスロットで開いたマルチフレームのファイルは、フレームの並びのスロットにする。
        // フォルダスロットの中では並べ替えない。フォルダの中を送れなくなるため。
        let mut path = path;
        let multiframe = loaded.info.frames > 1 && frames::split(&path).is_none();
        if multiframe && s.sequence.is_none() {
            let fps = frame_rate(&loaded.info.metadata);
            let seq = Sequence::frames(path.clone(), loaded.info.frames, fps);
            path = seq.paths[0].clone();
            s.path = Some(path.clone());
            s.sequence = Some(seq);
            s.from_folder = false;
        }
        let stepped = keeps_view(
            s.shown.as_ref().map(|shown| shown.path.as_path()),
            &path,
            s.sequence.as_ref(),
            keep_view_setting || loaded.info.plugin == "DICOM",
        );
        s.shown = Some(Shown {
            path,
            thumb,
            info: loaded.info,
            full: None,
            developed: false,
        });
        s.error = None;
        // 同じ画像の読み直し（上書きされたとき）は、拡大している場所を見続けられるよう倍率と位置を保つ。
        if !reloaded && !stepped {
            s.reset_view();
        }
        if let Some(full) = loaded.full {
            self.attach_full(ctx, index, full);
        }
        if self.settings.settings.viewer.develop_raw {
            self.develop_slot(index);
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
                request_full(&self.loader, &mut self.next_id, s);
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
                if self.menu_item(ui, Action::ParentFolder, None) {
                    chosen = Some(Action::ParentFolder);
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
            ui.menu_button(tr(lang, "Labels"), |ui| {
                if let Some(action) = self.labels_menu(ui) {
                    chosen = Some(action);
                }
            });
            ui.menu_button(tr(lang, "Image"), |ui| {
                let groups: [&[Action]; 5] = [
                    &[Action::PreviousImage, Action::NextImage],
                    &[Action::PreviousSlot, Action::NextSlot],
                    &[Action::PreviousPage, Action::NextPage],
                    &[
                        Action::FitImage,
                        Action::ActualSize,
                        Action::ZoomIn,
                        Action::ZoomOut,
                    ],
                    &[Action::DevelopRaw],
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

    /// Labels メニュー。タグでの絞り込みはタグの数だけ並ぶので、キーを割り当てる操作にはせず
    /// ここで直接切り替える。
    fn labels_menu(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let lang = self.lang();
        let mut chosen = None;
        if self.menu_item(ui, Action::ToggleTagging, Some(self.tagging)) {
            chosen = Some(Action::ToggleTagging);
        }
        for action in [Action::FlagGood, Action::FlagBad] {
            if self.menu_item(ui, action, None) {
                chosen = Some(action);
            }
        }
        ui.separator();
        let filters = [
            (Action::FilterAll, Filter::All),
            (Action::FilterGood, Filter::Flag(Some(Flag::Good))),
            (Action::FilterBad, Filter::Flag(Some(Flag::Bad))),
            (Action::FilterUnflagged, Filter::Flag(None)),
        ];
        for (action, filter) in filters {
            if self.menu_item(ui, action, Some(self.filter == filter)) {
                chosen = Some(action);
            }
        }
        let tags = self.settings.settings.labels.tags.clone();
        if !tags.is_empty() {
            ui.menu_button(tr(lang, "Show only tag"), |ui| {
                for tag in tags {
                    let filter = Filter::Tag(tag.clone());
                    if ui
                        .add(egui::Button::selectable(self.filter == filter, &tag))
                        .clicked()
                    {
                        self.set_filter(ui.ctx(), filter);
                        ui.close();
                    }
                }
            });
        }
        ui.separator();
        if self.menu_item(ui, Action::ExportImages, None) {
            chosen = Some(Action::ExportImages);
        }
        if self.menu_item(ui, Action::TrashBadImages, None) {
            chosen = Some(Action::TrashBadImages);
        }
        chosen
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
        let frame = frames::split(&shown.path);
        let mut file_rows = vec![
            row("Path", frames::file_of(&shown.path).display().to_string()),
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
        let widest_key = groups
            .iter()
            .flat_map(|(_, rows)| rows)
            .map(|(k, _)| {
                egui::WidgetText::from(k.as_str())
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
                for (group, rows) in &groups {
                    let id = ui.make_persistent_id(("metadata", group));
                    let state = CollapsingState::load_with_default_open(ui.ctx(), id, true);
                    let group_sel = MetaSelection::Group(group.clone());
                    let is_selected = selected.as_ref() == Some(&group_sel);
                    let (_, head, _) = state
                        .show_header(ui, |ui| ui.selectable_label(is_selected, group.as_str()))
                        .body(|ui| {
                            let avail = ui.available_width();
                            let gap = ui.spacing().item_spacing.x;
                            let gap_y = ui.spacing().item_spacing.y;
                            let key_width = key_column_width(widest_key, avail - gap);
                            let value_width = (avail - gap - key_width).max(0.0);
                            let row_of = |i: usize| MetaSelection::Row(group.clone(), i);
                            let selected_row = match &selected {
                                Some(MetaSelection::Row(g, i)) if g == group => Some(*i),
                                _ => None,
                            };
                            for (i, (k, v)) in rows.iter().enumerate() {
                                // 縞と選択の色は行の高さが決まってから塗るので、先に場所だけ取る。
                                let background = ui.painter().add(egui::Shape::Noop);
                                let row = ui.horizontal_top(|ui| {
                                    let key = fixed_width(ui, key_width, |ui| {
                                        ui.add(
                                            egui::Label::new(k)
                                                .truncate()
                                                .selectable(false)
                                                .sense(egui::Sense::click()),
                                        )
                                    });
                                    let value = fixed_width(ui, value_width, |ui| {
                                        ui.add(egui::Label::new(v).wrap())
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
    /// ログ欄。行をクリックで選び（Ctrl+C でコピー）、右クリックで 1 行かすべてをコピーする。
    ///
    /// 文字のドラッグでの範囲選択にしないのは、行が増えるたびに下へ流れて選択がずれるのと、
    /// 他の欄と同じ「選んでコピー」の操作に揃えるため。
    fn log_ui(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let mut picked = None;
        let mut copy = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, (_, level, _)) in self.log.lines.iter().enumerate() {
                    let color = match level {
                        Level::Info => ui.visuals().text_color(),
                        Level::Warn => ui.visuals().warn_fg_color,
                        Level::Error => ui.visuals().error_fg_color,
                    };
                    let text = self.log.line_text(i).unwrap_or_default();
                    let fill = if self.log_selected == Some(i) {
                        ui.visuals().selection.bg_fill
                    } else {
                        Color32::TRANSPARENT
                    };
                    let r = egui::Frame::new()
                        .fill(fill)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(text).monospace().color(color),
                                )
                                .wrap()
                                .selectable(false)
                                .sense(egui::Sense::click()),
                            )
                        })
                        .inner;
                    if r.clicked() || r.secondary_clicked() {
                        picked = Some(i);
                    }
                    r.context_menu(|ui| {
                        if ui.button(tr(lang, "Copy")).clicked() {
                            copy = Some(Some(i));
                            ui.close();
                        }
                        if ui.button(tr(lang, "Copy all")).clicked() {
                            copy = Some(None);
                            ui.close();
                        }
                    });
                }
            });
        let ctx = ui.ctx().clone();
        if let Some(i) = picked {
            self.log_selected = Some(i);
            self.set_focus(&ctx, Focus::Log);
        }
        match copy {
            Some(Some(i)) => {
                if let Some(text) = self.log.line_text(i) {
                    self.copy_text(&ctx, text, "log line");
                }
            }
            Some(None) => {
                let text = self.log.all_text();
                self.copy_text(&ctx, text, "log");
            }
            None => {}
        }
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

    /// スロットのカメラ RAW を現像し、届いたら埋め込みプレビューと差し替える。
    /// 現像できない画像と、現像を頼んだあとの画像では何もしない。
    fn develop_slot(&mut self, index: usize) {
        let s = &mut self.slots[index];
        let Some(shown) = &mut s.shown else {
            return;
        };
        if shown.info.develop_size.is_none() || shown.developed {
            return;
        }
        shown.developed = true;
        // 読み込み中のプレビューは要らなくなる。届くと現像の結果を待たずにそれを出してしまう。
        if let Some((request, _)) = s.full_request.take() {
            self.loader.cancel(request);
        }
        request_full(&self.loader, &mut self.next_id, s);
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
                request_full(&self.loader, &mut self.next_id, s);
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
                if let Some(p) = self.current.clone() {
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
            Focus::Log => {
                if let Some(text) = self.log_selected.and_then(|i| self.log.line_text(i)) {
                    self.copy_text(ctx, text, "log line");
                }
            }
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
        let mut area = whole;
        if maximized {
            let (rest, bar) = area.split_top_bottom_at_y(area.max.y - PLAY_BAR);
            area = rest;
            let action = ui
                .scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| self.play_bar(ui))
                .inner;
            if let Some(action) = action {
                self.apply_bar_action(ui.ctx(), action);
            }
        }
        if self.tagging {
            let (rest, bar) = area.split_top_bottom_at_y(area.max.y - PLAY_BAR);
            area = rest;
            let click = ui
                .scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| self.tag_bar(ui))
                .inner;
            match click {
                Some(TagClick::Flag(flag)) => self.flag(flag),
                Some(TagClick::Tag(i)) => self.tag(i),
                None => {}
            }
        }
        self.images_area = Some(area.size() * ui.ctx().pixels_per_point());
        let (cols, rows) = (view.cols, view.rows);
        let cell = vec2(area.width() / cols as f32, area.height() / rows as f32);
        let mut close = None;
        let mut maximize = None;
        let mut explore = None;
        let mut open_folder = None;
        let mut dropped = None;
        let mut step = None;
        let background = self.canvas_color(ui.ctx());
        let lang = self.lang();
        let hint = empty_slot_hint(self.current.is_some());
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
            let empty_hint = match &self.slots[i].sequence {
                Some(seq) if seq.paths.is_empty() => "No image",
                Some(_) => "No images match the filter",
                None => hint,
            };
            let label = self.slots[i]
                .path
                .as_deref()
                .map(|p| self.labels.get(p))
                .unwrap_or_default();
            let resp = self.slots[i].ui(
                ui,
                rect.shrink(1.0),
                i == self.active,
                SlotLook {
                    background,
                    lang,
                    empty_hint,
                    label: &label,
                },
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
                self.set_focus(ui.ctx(), Focus::Images);
            }
            if resp.copy {
                self.copy_image(ui.ctx(), i);
            }
            if resp.develop {
                self.develop_slot(i);
            }
            if resp.close {
                close = Some(i);
            }
            if resp.toggle_maximize {
                if self.slots[i].is_closed_folder() {
                    explore = Some(i);
                } else {
                    maximize = Some(i);
                }
            }
            if resp.open_folder {
                open_folder = self.slots[i].folder.clone();
            }
            if let (Some(file), Some(dir)) = (resp.dropped, self.slots[i].folder.clone()) {
                dropped = Some((file, dir));
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
        if let Some((file, dir)) = dropped {
            let ctx = ui.ctx().clone();
            self.drop_file(&ctx, file, dir);
        } else if let Some(folder) = open_folder {
            let ctx = ui.ctx().clone();
            self.open_folder(&ctx, folder);
        } else if let Some(i) = close {
            self.close_slot(i);
        } else if let Some(i) = explore {
            let ctx = ui.ctx().clone();
            self.active = i;
            self.explore(&ctx, i);
        } else if let Some(i) = maximize {
            self.toggle_maximize(i);
        } else if let Some((i, delta)) = step {
            let chunk = self.slots[i].sequence.is_some();
            let one_up = self.maximized.is_some() || self.shows_one_slot();
            match wheel_step(chunk, one_up, self.slots.len()) {
                WheelStep::Image => {
                    self.active = i;
                    self.step_active(delta);
                }
                WheelStep::Slot => {
                    self.step_image_slot(delta, true);
                }
                WheelStep::Nothing => {}
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
        self.receive_archives(&ctx);
        self.pump_folder_load();
        self.pump_explores();
        if self.series.poll() {
            self.group_series();
        }
        self.watch_folders(&ctx);
        for problem in self.labels.take_problems() {
            self.log.error(problem);
        }
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
                    // 絞り込み中は、ファイルが隠れていることに気づけるよう見出しに条件を出す。
                    if self.filter.is_all() {
                        panel_header(ui, tr(lang, "Folders"));
                    } else {
                        let text = format!(
                            "{}  ({}: {})",
                            tr(lang, "Folders"),
                            tr(lang, "Filter"),
                            filter_name(&self.filter, lang)
                        );
                        panel_header(ui, &text);
                    }
                    // フレームを出しているときは、そのファイルのシリーズを選択として見せる。
                    let active_image = self.active_image().map(|p| frames::file_of(&p));
                    let drawn = egui::ScrollArea::both()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            self.tree.ui(
                                ui,
                                tree::Context {
                                    registry: self.loader.registry(),
                                    series: &self.series,
                                    current: self.current.as_deref(),
                                    active_image: active_image.as_deref(),
                                    lang,
                                },
                            )
                        })
                        .inner;
                    let ctx = ui.ctx().clone();
                    for (dir, files) in drawn.scans {
                        let registry = self.loader.registry().clone();
                        let wake = ctx.clone();
                        self.series
                            .start(dir, files, registry, move || wake.request_repaint());
                    }
                    let Some(action) = drawn.action else {
                        return;
                    };
                    match action {
                        TreeAction::Cd(p) => self.open_folder(&ctx, p),
                        TreeAction::Mount(zip) => self.mount(&ctx, &zip),
                        TreeAction::Unmount(dir) => self.unmount(&ctx, &dir),
                        TreeAction::Series { folder, files } => {
                            self.show_series(&ctx, folder, &files)
                        }
                        TreeAction::Drop { file, dir } => {
                            self.drop_file(&ctx, file, dir);
                            return;
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
        self.export_window(&ctx);
        self.toast_ui(&ctx);
        self.folder_load_dialog(&ctx);
        self.move_dialog(&ctx);
        self.trash_dialog(&ctx);
        self.finish_trash_job();
        self.drag_preview(&ctx);
        self.receive_os_drop(&ctx);
        self.select_dropped_image();
    }
}

/// 複数枚のスロットの中で `previous` から `next` に送ったときに、倍率と位置を引き継ぐか。
/// スライスやフレームを送って同じ場所を見比べるため。別の並びに入れ替えたときは引き継がない。
fn keeps_view(
    previous: Option<&Path>,
    next: &Path,
    sequence: Option<&Sequence>,
    enabled: bool,
) -> bool {
    let (Some(previous), Some(seq)) = (previous, sequence) else {
        return false;
    };
    enabled && seq.paths.iter().any(|p| p == previous) && seq.paths.iter().any(|p| p == next)
}

/// シネやアニメーションのフレームの速さ（毎秒のフレーム数）。DICOM の Frame Time（ミリ秒）を
/// 優先し、なければ推奨の表示速度か Cine Rate を使う。DICOM でなければアニメーションの
/// 1 フレームの平均の表示時間を使う。
fn frame_rate(metadata: &vw_core::Metadata) -> Option<f32> {
    let number = |group, key| {
        let v = metadata.get(group, key)?;
        // 複数値（`33.3\33.3`）なら最初の値。
        v.split('\\')
            .next()?
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|x| *x > 0.0)
    };
    number("DICOM", "Frame time (ms)")
        .map(|ms| 1000.0 / ms)
        .or_else(|| number("DICOM", "Recommended frame rate"))
        .or_else(|| number("DICOM", "Cine rate"))
        .or_else(|| number("Animation", "Frame time (ms)").map(|ms| 1000.0 / ms))
}

/// 絞り込みの名前。タグはそのまま（翻訳しない）。
fn filter_name(filter: &Filter, lang: Language) -> String {
    let name = match filter {
        Filter::All => "all",
        Filter::Flag(Some(Flag::Good)) => "good",
        Filter::Flag(Some(Flag::Bad)) => "bad",
        Filter::Flag(None) => "unflagged",
        Filter::Tag(tag) => return format!("#{tag}"),
    };
    tr(lang, name).to_owned()
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
fn empty_slot_hint(in_folder: bool) -> &'static str {
    if in_folder {
        "No images or folders here"
    } else {
        "Click a folder in the tree to show its contents"
    }
}

/// 何も出しておらず、何の役目もないスロット。フォルダの中身を並べるときに使い回す。
fn is_blank(slot: &Slot) -> bool {
    slot.path.is_none() && !slot.loading && slot.folder.is_none() && slot.sequence.is_none()
}

/// パネルの見出し。本文と区別できる小さな大文字にして、縦の領域を取りすぎないようにする。
/// メタデータ欄のキーの列の幅。いちばん長いキーに合わせるが、欄の幅の一定の割合までにする。
///
/// 列の幅を中身に任せない（egui の Grid の既定）のは、長いキーや値で列が広がると
/// 欄そのものが押し広げられ、画像を選び替えるたびに欄の幅が変わってしまうため。
/// 収まらないキーは末尾を省略し、マウスを乗せると全体を出す。
fn key_column_width(widest_key: f32, available: f32) -> f32 {
    widest_key.min(available * KEY_COLUMN_SHARE).max(0.0)
}

/// メタデータ欄でキーの列に使える幅の割合。残りは値の列（折り返す）に使う。
const KEY_COLUMN_SHARE: f32 = 0.45;

/// ちょうど `width` の幅の中に `add` で 1 つ置く。中身がそれより広くても外へは広げない。
fn fixed_width(
    ui: &mut egui::Ui,
    width: f32,
    add: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    ui.allocate_ui_with_layout(
        vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            add(ui)
        },
    )
    .inner
}

fn panel_header(ui: &mut egui::Ui, text: &str) {
    // 絞り込みの条件などで長くなっても、欄を押し広げないよう省略する。
    ui.add(egui::Label::new(egui::RichText::new(text.to_uppercase()).small().weak()).truncate());
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
/// スロットの表示中の画像のフル解像度を読み始める。現像を頼んだ画像は現像する。
fn request_full(loader: &Loader, next_id: &mut u64, s: &mut Slot) {
    let Some(shown) = &s.shown else {
        return;
    };
    let request = *next_id;
    *next_id += 1;
    let bytes = match shown.developed {
        true => shown.info.developed_bytes(),
        false => None,
    };
    s.full_request = Some((request, bytes.unwrap_or_else(|| shown.info.full_bytes())));
    loader.request(Job {
        slot: s.id,
        request,
        path: shown.path.clone(),
        thumb: false,
        full: true,
        develop: shown.developed,
    });
}

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
    fn metadata_key_column_fits_the_widest_key_when_there_is_room() {
        assert_eq!(key_column_width(80.0, 300.0), 80.0);
    }

    #[test]
    fn metadata_key_column_is_capped_to_a_share_of_the_panel() {
        assert_eq!(key_column_width(500.0, 200.0), 200.0 * KEY_COLUMN_SHARE);
    }

    #[test]
    fn view_is_kept_only_when_stepping_inside_the_same_sequence() {
        let p = |n: &str| PathBuf::from(n);
        let seq = Sequence::series(p("dir"), "s".into(), vec![p("a"), p("b")]);
        assert!(keeps_view(Some(&p("a")), &p("b"), Some(&seq), true));
        assert!(!keeps_view(Some(&p("a")), &p("b"), Some(&seq), false));
        // 別の並びから来た、初めて出した、並びのないスロットでは全体表示に戻す。
        assert!(!keeps_view(Some(&p("x")), &p("b"), Some(&seq), true));
        assert!(!keeps_view(None, &p("b"), Some(&seq), true));
        assert!(!keeps_view(Some(&p("a")), &p("b"), None, true));
    }

    #[test]
    fn cine_frame_rate_comes_from_frame_time_first() {
        let mut m = vw_core::Metadata::default();
        m.push("DICOM", "Cine rate", "15");
        assert_eq!(frame_rate(&m), Some(15.0));
        m.push("DICOM", "Frame time (ms)", "33.333\\33.333");
        assert!((frame_rate(&m).unwrap() - 30.0).abs() < 0.01);
        assert_eq!(frame_rate(&vw_core::Metadata::default()), None);
    }

    #[test]
    fn animation_frame_rate_comes_from_the_average_frame_time() {
        let mut m = vw_core::Metadata::default();
        m.push("Animation", "Frame time (ms)", "50.0");
        assert_eq!(frame_rate(&m), Some(20.0));
    }

    #[test]
    fn stepping_image_slots_skips_folder_slots() {
        // フォルダ, フォルダ, 画像, 画像, 画像
        let images = [false, false, true, true, true];
        assert_eq!(image_slot_step(&images, 2, 1, true), Some(3));
        assert_eq!(image_slot_step(&images, 2, -1, true), Some(4));
        assert_eq!(image_slot_step(&images, 4, 1, true), Some(2));
    }

    #[test]
    fn stepping_image_slots_without_wrap_stops_at_the_last_image() {
        let images = [false, true, true];
        assert_eq!(image_slot_step(&images, 2, 1, false), None);
        assert_eq!(image_slot_step(&images, 1, -1, false), None);
        assert_eq!(image_slot_step(&images, 1, 5, false), Some(2));
    }

    #[test]
    fn stepping_image_slots_with_no_other_image_does_nothing() {
        assert_eq!(image_slot_step(&[false, true, false], 1, 1, true), None);
    }

    #[test]
    fn wheel_steps_images_in_a_chunk_slot_even_when_slots_are_tiled() {
        assert_eq!(wheel_step(true, false, 4), WheelStep::Image);
    }

    #[test]
    fn wheel_does_nothing_on_a_single_image_slot_when_slots_are_tiled() {
        assert_eq!(wheel_step(false, false, 4), WheelStep::Nothing);
    }

    #[test]
    fn wheel_moves_to_the_next_slot_when_one_of_several_is_shown() {
        assert_eq!(wheel_step(false, true, 4), WheelStep::Slot);
        assert_eq!(wheel_step(false, true, 1), WheelStep::Image);
    }

    #[test]
    fn nearest_match_prefers_the_closest_image_and_then_the_later_one() {
        let paths: Vec<PathBuf> = ["a", "b", "c", "d", "e"].map(PathBuf::from).into();
        let only = |names: &'static [&str]| move |p: &Path| names.iter().any(|n| p == Path::new(n));
        assert_eq!(nearest_match(&paths, 2, only(&["c"])), Some(2));
        assert_eq!(nearest_match(&paths, 2, only(&["b", "d"])), Some(3));
        assert_eq!(nearest_match(&paths, 2, only(&["a", "e"])), Some(4));
        assert_eq!(nearest_match(&paths, 2, only(&[])), None);
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
