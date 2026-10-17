//! アプリ本体。ツール列と 4 つのフィールド（フォルダツリー・メタデータ・画像・ログ）の配置、
//! 設定画面への切り替え、ホットキーの実行、スロットの管理、読み込み結果の受け取りを行う。
//!
//! egui は即時モード GUI で、毎フレーム `ui()` が呼ばれて画面全体を組み立て直す。
//! 状態はすべて [`VwApp`] が持ち、ウィジェット側には持たせない。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, Color32, TextureOptions};
use egui_phosphor::regular as icon;
use vw_core::{ColorModel, Registry};

use crate::archive::{self, Archives};
use crate::autoload::FolderLoad;
use crate::copy::{Focus, MetaSelection};
use crate::discard::{self, TrashJob};
use crate::export::{CopyJob, Filter, ListFormat};
use crate::frames;
use crate::i18n::tr;
use crate::keymap::{Action, KeyCombo, Scope};
use crate::labels::{Flag, LabelStore};
use crate::loader::{Job, Loader};
use crate::log::{self, Level, Log, LogFile};
use crate::series::SeriesIndex;
use crate::settings::{parse_hex_color, Language, LoadInto, LogStyle, SettingsStore};
use crate::settings_ui::SettingsUi;
use crate::slideshow::{Clock, EndMode, Playback};
use crate::state::{self, AppState, Placement, WindowState};
use crate::sub_window::SubWindow;
use crate::theme;
use crate::transfer::Transfer;
use crate::tree::{self, FolderTree, TreeAction};
use crate::viewer::{Sequence, Slot, KEY_ZOOM_FACTOR};
use crate::watch::FolderWatcher;

mod activity;
mod ai_tagging;
mod clipboard;
mod dataset_dialogs;
mod export_dialog;
mod folders;
mod labeling;
mod log_pane;
mod menu;
mod metadata_pane;
mod people_ui;
mod persist;
mod playback;
mod screens;
mod send;
mod slots;
mod transfers;

/// 左端のツール列のツール。Browse と PixelProbe 以外はまだ機能がなく、押せない状態で置いてある。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Browse,
    Pan,
    Zoom,
    PixelProbe,
    Roi,
    Measure,
}

impl Tool {
    const ALL: [Tool; 6] = [
        Tool::Browse,
        Tool::Pan,
        Tool::Zoom,
        Tool::PixelProbe,
        Tool::Roi,
        Tool::Measure,
    ];

    fn icon(self) -> &'static str {
        match self {
            Tool::Browse => icon::CURSOR,
            Tool::Pan => icon::HAND_GRABBING,
            Tool::Zoom => icon::MAGNIFYING_GLASS_PLUS,
            Tool::PixelProbe => icon::EYEDROPPER,
            Tool::Roi => icon::SELECTION,
            Tool::Measure => icon::RULER,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Tool::Browse => "Browse",
            Tool::Pan => "Pan",
            Tool::Zoom => "Zoom",
            Tool::PixelProbe => "Pixel probe",
            Tool::Roi => "ROI",
            Tool::Measure => "Measure",
        }
    }

    fn available(self) -> bool {
        matches!(self, Tool::Browse | Tool::PixelProbe | Tool::Roi)
    }

    /// キーで切り替えられるツールの操作。
    fn action(self) -> Option<Action> {
        match self {
            Tool::PixelProbe => Some(Action::TogglePixelProbe),
            Tool::Roi => Some(Action::ToggleRoi),
            _ => None,
        }
    }

    /// ツールの説明に添える使い方。
    fn hint(self) -> Option<&'static str> {
        match self {
            Tool::Roi => Some("Drag to draw the target ROI, Shift+drag for the background ROI"),
            _ => None,
        }
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

/// パスワードを尋ねている暗号化されたアーカイブ。
struct PasswordPrompt {
    zip: PathBuf,
    input: String,
    /// 前に入力されたパスワードが違った。
    wrong: bool,
    /// 入力欄にフォーカスを移した。
    focused: bool,
}

pub struct VwApp {
    loader: Loader,
    tree: FolderTree,
    /// レジストリに渡した、DICOM か調べる拡張子の式。変わったときだけフォルダを読み直すため。
    probe_extensions: String,
    /// いまのテクスチャの拡大・縮小の設定。設定が変わったら読み込み済みのテクスチャにも当てる。
    texture_options: TextureOptions,
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
    /// 他の機器への送信（数えている・確かめている・送っている）。一度に 1 つだけ。
    send_job: Option<crate::dicom_send::Job>,
    /// 画素の変換の図のウインドウ。
    pipeline: crate::pipeline_view::PipelineView,
    histogram: crate::histogram_view::HistogramView,
    zoom_map: crate::zoom_map::ZoomMap,
    evaluation: crate::quality::Evaluation,
    /// 「評価の基準にする」で選んだスロットの画像。PSNR・SSIM をこれと比べる。
    quality_reference: Option<PathBuf>,
    /// 名前を付けて保存した色導出の値。
    presets: crate::pipelines::PresetStore,
    about: crate::about::About,
    /// 「AI のモデル」ウインドウとダウンロード。
    ai_models: crate::ai_models::Dialog,
    /// モデルの置き場。設定ファイルの場所が決まらない環境では `None`。
    ai_store: Option<crate::ai_models::Store>,
    ai_tags: crate::ai_tags::AiTagStore,
    /// 走っている AI タグ付け。一度に 1 つ。
    ai_job: Option<ai_tagging::Job>,
    /// 走っている仕事が終わったら処理する画像。
    ai_queue: Vec<(ai_tagging::Work, PathBuf)>,
    /// 人物（設定フォルダの people.json）。
    people: crate::people::People,
    /// 見つけた顔の特徴（設定フォルダの faces）。
    face_store: crate::face_store::FaceStore,
    people_window: people_ui::PeopleWindow,
    /// 顔の枠をスロットに描く。
    show_faces: bool,
    /// 設定画面を開いている間は、ツール列とメニュー以外をすべて設定画面に置き換える。
    show_settings: bool,
    /// Datasets 画面を出している（設定画面と同じくメインの領域を置き換える）。
    show_datasets: bool,
    datasets: crate::datasets_screen::DatasetsScreen,
    /// 全画面スライドショーの準備の画面を出している（メインの領域を置き換える）。
    show_fullscreen_setup: bool,
    fullscreen_setup: crate::fullscreen::Setup,
    /// 全画面スライドショーの最中。
    fullscreen: Option<crate::fullscreen::Show>,
    /// 最後に全画面スライドショーを始めた起点。次に準備の画面を開いたときの初期値。
    last_fullscreen_root: Option<PathBuf>,
    /// 「すべて更新」で順に更新するデータセット。
    dataset_queue: Vec<PathBuf>,
    /// 取り出した先を含むデータセット。更新するか聞いている間だけ。
    outer_to_update: Option<PathBuf>,
    /// 最後にデータセットを取り出した出力先の親フォルダ。次の出力先の初期値に使う。
    last_extract_dir: Option<PathBuf>,
    log: Log,
    /// プロセス起動時刻。最初のフレームまでの時間をログに出すために持つ。
    started: Instant,
    /// 読み込みの流れ。ターミナル風のログ欄の上の行に出す。
    throughput: crate::status::Throughput,
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
    /// 止めたスライドショーの時計。止めた後も、直近のコマ落ちの数が消えるまで再生バーに出すため。
    stopped_clock: Option<Clock>,
    /// 再生バーの「ファイルの速さ」。シネやアニメーションをファイルに書かれた速さで再生する。
    use_file_speed: bool,
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
    /// パスワードを尋ねている暗号化された ZIP / 7z。
    password_prompt: Option<PasswordPrompt>,
    dataset_dialog: Option<crate::dataset_ui::DatasetDialog>,
    trash_job: Option<TrashJob>,
    /// 監視を始められなかった。毎フレーム作り直してログを埋めないよう、設定を切り替えるまで諦める。
    watcher_failed: bool,
}

/// 処理中を示す、ウィンドウの一番下の線の太さ。
const ACTIVITY_LINE: f32 = 2.0;

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
    /// 画面に出す `root`。ZIP の中なら一時フォルダではなく ZIP のパスで見せる。
    root_label: String,
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
    FileSpeed(bool),
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
        let presets_path = settings
            .path
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(|dir| dir.join(crate::pipelines::FILE_NAME));
        let (presets, notes) = crate::pipelines::PresetStore::open(presets_path);
        for n in notes {
            log.error(format!("Pipelines: {n}"));
        }
        let log_settings = &settings.settings.log;
        if let (true, Some(path)) = (log_settings.file, settings.path.as_deref()) {
            let dir = log::log_dir(path);
            match LogFile::create(&dir, log_settings.keep_files as usize, chrono::Local::now()) {
                Ok(file) => {
                    file.install_panic_hook();
                    log.write_to(file);
                }
                Err(e) => log.error(format!("Log file: {}: {e}", dir.display())),
            }
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
            .partition(|p| p.is_dir() || archive::is_archive(p));
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
        let ai_store = crate::ai_models::Store::new(settings.path.as_deref());
        let settings_dir = settings
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let (people, people_problem) = crate::people::People::open(settings_dir.as_deref());
        if let Some(p) = people_problem {
            log.error(p);
        }
        let face_store = crate::face_store::FaceStore::new(settings_dir.map(|d| d.join("faces")));
        let ai_tag_fallback = settings
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(|p| p.join("ai-tags"));
        let mut app = Self {
            loader: Loader::new(registry, cc.egui_ctx.clone()),
            tree,
            probe_extensions: settings.settings.dicom.probe_extensions.clone(),
            texture_options: settings.settings.viewer.texture_options(),
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
            send_job: None,
            about: Default::default(),
            ai_models: Default::default(),
            ai_store,
            ai_tags: crate::ai_tags::AiTagStore::new(ai_tag_fallback),
            ai_job: None,
            ai_queue: Vec::new(),
            people,
            face_store,
            people_window: Default::default(),
            show_faces: true,
            pipeline: {
                let mut view = crate::pipeline_view::PipelineView::default();
                view.restore_node_positions(&saved.pipeline_nodes);
                view
            },
            histogram: {
                let mut view = crate::histogram_view::HistogramView::default();
                view.window = SubWindow::restored(
                    saved
                        .sub_windows
                        .get("histogram")
                        .copied()
                        .unwrap_or_default(),
                );
                view
            },
            evaluation: Default::default(),
            quality_reference: None,
            presets,
            zoom_map: {
                let mut view = crate::zoom_map::ZoomMap::default();
                view.window = SubWindow::restored(
                    saved
                        .sub_windows
                        .get("zoom_map")
                        .copied()
                        .unwrap_or_default(),
                );
                view
            },
            show_settings: false,
            show_datasets: false,
            datasets: Default::default(),
            show_fullscreen_setup: false,
            fullscreen_setup: Default::default(),
            fullscreen: None,
            last_fullscreen_root: saved.fullscreen_root.clone(),
            dataset_queue: Vec::new(),
            outer_to_update: None,
            last_extract_dir: saved.extract_dir.clone(),
            log,
            started,
            throughput: crate::status::Throughput::new(started),
            first_frame: true,
            state_path,
            restoring,
            window: None,
            metadata_from: None,
            focus: Focus::Images,
            meta_selected: None,
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
            stopped_clock: None,
            use_file_speed: true,
            watcher: None,
            watcher_failed: false,
            loaded_folders: Vec::new(),
            archives: Archives::in_temp_dir(),
            labels: LabelStore::new(label_fallback),
            tagging: false,
            filter: Filter::All,
            export: None,
            pending_trash: None,
            password_prompt: None,
            dataset_dialog: None,
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

    /// 今の設定。`SettingsStore` はファイルとの同期も持つので、値を読むだけのときはこちら。
    fn settings(&self) -> &crate::settings::Settings {
        &self.settings.settings
    }

    fn toggle_tool(&mut self, tool: Tool) {
        self.tool = if self.tool == tool {
            Tool::Browse
        } else {
            tool
        };
    }

    fn lang(&self) -> Language {
        self.settings().appearance.language
    }

    /// 画像の背景色。設定が空か読めない形式ならテーマの既定色。
    fn canvas_color(&self, ctx: &egui::Context) -> Color32 {
        parse_hex_color(&self.settings().viewer.background)
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
            Action::TogglePixelProbe => self.toggle_tool(Tool::PixelProbe),
            Action::ToggleRoi => self.toggle_tool(Tool::Roi),
            Action::DeleteProbePoint => {
                let slot = &mut self.slots[self.active];
                if !slot.delete_selected_mark() {
                    slot.delete_selected_roi();
                }
            }
            Action::FilterAll => self.set_filter(ctx, Filter::All),
            Action::FilterGood => self.set_filter(ctx, Filter::Flag(Some(Flag::Good))),
            Action::FilterBad => self.set_filter(ctx, Filter::Flag(Some(Flag::Bad))),
            Action::FilterUnflagged => self.set_filter(ctx, Filter::Flag(None)),
            Action::ExportImages => self.open_export(),
            Action::TrashBadImages => self.ask_trash_bad(),
            Action::CancelSending => self.cancel_send(),
            Action::TogglePixelPipeline => self.toggle_pipeline(),
            Action::ToggleHistogram => self.toggle_histogram(),
            Action::ToggleZoomMap => self.zoom_map.open = !self.zoom_map.open,
            Action::ShowAbout => self.about.open = true,
            Action::ShowAiModels => self.ai_models.open = true,
            Action::AiTagFolder => self.ai_tag_current_folder(ctx, ai_tagging::Work::Tag),
            Action::FindFacesFolder => self.ai_tag_current_folder(ctx, ai_tagging::Work::Faces),
            Action::ShowPeople => self.people_window.open = true,
            Action::ToggleFaceFrames => self.show_faces = !self.show_faces,
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
            Action::ToggleDatasets => self.toggle_datasets(ctx),
            Action::FullscreenSlideshow => self.toggle_fullscreen_setup(),
            // 全画面スライドショーの間だけ働く（[`crate::fullscreen::Show::ui`]）。
            Action::FullscreenExit
            | Action::FullscreenNext
            | Action::FullscreenPrevious
            | Action::FullscreenNextFolder
            | Action::FullscreenPreviousFolder
            | Action::FullscreenPause => {}
            Action::PreviousSlot => self.step_slot(-1),
            Action::NextSlot => self.step_slot(1),
            Action::PreviousPage | Action::NextPage => {
                let delta = if action == Action::NextPage { 1 } else { -1 };
                if self.maximized.is_none() {
                    self.step_page(delta);
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
            Action::LayoutCustom => self.set_layout(ctx, self.settings().viewer.custom_layout),
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
        let keys = &self.settings().keys;
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
        for action in keys.actions_in(Scope::Main, &events) {
            // 設定画面の間は画像やパネルの操作をしない。見えていない画面が変わるのを避けるため。
            if self.show_settings && action != Action::ToggleSettings {
                continue;
            }
            if self.show_datasets
                && !matches!(action, Action::ToggleDatasets | Action::ToggleSettings)
            {
                continue;
            }
            if self.show_fullscreen_setup
                && !matches!(
                    action,
                    Action::FullscreenSlideshow | Action::ToggleDatasets | Action::ToggleSettings
                )
            {
                continue;
            }
            if action.needs_tagging_mode() && !self.tagging {
                continue;
            }
            self.run(ctx, action);
        }
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
        self.reload_presets();
        if self.fullscreen.is_some() {
            self.fullscreen_ui(ui);
            return;
        }

        // キー入力待ちのときは、押されたキーを割り当てに使い切る（ホットキーとして実行しない）。
        if self
            .settings_ui
            .capture_keys(&ctx, &mut self.settings.settings.keys)
        {
            self.settings_changed(&ctx);
        }
        self.run_hotkeys(&ctx);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        self.activity_line(ui);
        self.send_ui(&ctx);
        self.update_evaluation(&ctx);
        self.pipeline_ui(&ctx);
        self.histogram_ui(&ctx);
        self.zoom_map_ui(&ctx);
        self.ai_ui(&ctx);
        if let Some(e) = self.about.ui(&ctx, self.lang()) {
            self.log
                .error(format!("Third-party licenses: cannot open {e}"));
        }
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
            for (ok, line) in r.logs {
                if ok {
                    self.log.info(line);
                } else {
                    self.log.error(line);
                }
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

        if self.show_fullscreen_setup {
            let current = self.current.clone();
            let action =
                self.fullscreen_setup
                    .ui(ui, &mut self.settings.settings, current.as_deref());
            match action {
                Some(crate::fullscreen::SetupAction::Close) => self.show_fullscreen_setup = false,
                Some(crate::fullscreen::SetupAction::Changed) => self.settings_changed(&ctx),
                Some(crate::fullscreen::SetupAction::Start(root)) => {
                    self.start_fullscreen(&ctx, root)
                }
                None => {}
            }
            return;
        }

        if self.show_datasets {
            let lang = self.lang();
            let dataset = &self.settings.settings.dataset;
            let destinations = self.destination_names();
            let action = self.datasets.ui(
                ui,
                lang,
                &dataset.roots,
                &dataset.fields,
                &dataset.search_groups,
                &destinations,
            );
            match action {
                Some(crate::datasets_screen::Action::Close) => self.show_datasets = false,
                Some(crate::datasets_screen::Action::Show(dir)) => {
                    self.show_dataset(&ctx, dir);
                }
                Some(crate::datasets_screen::Action::ShowFiles(dir, files)) => {
                    self.show_dataset_files(&dir, &files);
                }
                Some(crate::datasets_screen::Action::Extract(dir, hits)) => {
                    self.open_dataset_dialog(&ctx, crate::dataset_ui::Request::Extract(dir, hits));
                }
                Some(crate::datasets_screen::Action::Edit(dir)) => {
                    self.open_dataset_dialog(&ctx, crate::dataset_ui::Request::Edit(dir));
                }
                Some(crate::datasets_screen::Action::Update(dirs)) => {
                    self.dataset_queue.extend(dirs);
                }
                Some(crate::datasets_screen::Action::Send(dir, hits, choice)) => {
                    let targets = crate::dicom_send::dataset_targets(&dir, hits.as_deref());
                    self.send_targets(choice, targets);
                }
                Some(crate::datasets_screen::Action::SaveGroup(group)) => {
                    self.save_search_group(&ctx, group);
                }
                Some(crate::datasets_screen::Action::Reload) => {
                    self.datasets.reload(&dataset.roots, &ctx);
                    // ツリーのデータセットの色も、カタログの有無に合わせて付け直す。
                    self.tree.forget_peeks();
                }
                None => {}
            }
            self.next_queued_update(&ctx);
            self.dataset_window(&ctx);
            self.outer_update_dialog(&ctx);
            self.password_dialog(&ctx);
            return;
        }

        let lang = self.lang();
        if self.fields.log {
            egui::Panel::bottom("log")
                .resizable(true)
                .default_size(140.0)
                .show(ui, |ui| {
                    let sending = self.send_job.as_ref().and_then(|j| j.progress());
                    let (cancel, header) = ui
                        .horizontal(|ui| {
                            panel_header(ui, tr(lang, "Log"));
                            let mut cancel = false;
                            if let Some((done, total)) = sending {
                                ui.weak(format!(
                                    "{} {} / {}",
                                    tr(lang, "Sending"),
                                    thousands(done),
                                    thousands(total)
                                ));
                                cancel = ui.small_button(tr(lang, "Cancel sending")).clicked();
                            }
                            (cancel, self.log_header(ui))
                        })
                        .inner;
                    if cancel {
                        self.cancel_send();
                    }
                    if let Some(action) = header {
                        self.log_action(&ctx, action);
                    }
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
                    let destinations = self.destination_names();
                    let ai_ready = self.ai_ready();
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
                                    destinations: &destinations,
                                    ai_ready,
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
                        TreeAction::Dataset(request) => self.open_dataset_dialog(&ctx, request),
                        TreeAction::Send { choice, targets } => self.send_to(choice, targets),
                        TreeAction::Mount(zip) => self.mount(&ctx, &zip),
                        TreeAction::Unmount(dir) => self.unmount(&ctx, &dir),
                        TreeAction::Series { folder, files } => {
                            self.show_series(&ctx, folder, &files)
                        }
                        TreeAction::Drop { file, dir } => {
                            self.drop_file(&ctx, file, dir);
                            return;
                        }
                        TreeAction::OpenInFileManager(p) => {
                            self.open_in_file_manager(&p);
                            return;
                        }
                        TreeAction::AiTagFolder(p) => {
                            self.ai_tag_folder(&ctx, &p);
                            return;
                        }
                        TreeAction::FindFaces(p) => {
                            self.find_faces_folder(&ctx, &p);
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
        self.password_dialog(&ctx);
        self.dataset_window(&ctx);
        self.outer_update_dialog(&ctx);
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
pub fn frame_rate(metadata: &vw_core::Metadata) -> Option<f32> {
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
        Action::ToggleDatasets => "Datasets",
        Action::ToggleMaximize => "Maximize slot",
        Action::TogglePixelPipeline => "Pixel pipeline",
        Action::ToggleHistogram => "Analysis",
        Action::ToggleZoomMap => "Zoom map",
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

/// `1234` を `1,234` にする。送る件数の確認で桁を読み違えないように。
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// ログ欄の見出しのボタンと右クリックのメニューで頼むこと。
#[derive(Clone, Copy)]
enum LogAction {
    Style(LogStyle),
    OpenFile,
    OpenFolder,
    CopyAll,
}

/// ターミナル風のログ欄の地の色。テーマによらず黒地にする。
const TERMINAL_BACKGROUND: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);

fn terminal_line(line: &log::Line) -> egui::Label {
    let color = match line.level {
        Level::Info => Color32::from_rgb(0xcc, 0xcc, 0xcc),
        Level::Warn => Color32::from_rgb(0xe5, 0xc0, 0x7b),
        Level::Error => Color32::from_rgb(0xf0, 0x71, 0x78),
    };
    egui::Label::new(
        egui::RichText::new(line.terminal())
            .monospace()
            .color(color),
    )
    .extend()
}

fn status_job(line: &[crate::status::Segment]) -> egui::text::LayoutJob {
    use crate::status::Tone;
    let font = egui::FontId::monospace(12.0);
    let mut job = egui::text::LayoutJob::default();
    for (text, tone) in line {
        let color = match tone {
            Tone::Label => Color32::from_rgb(0x8a, 0x8a, 0x8a),
            Tone::Value => Color32::from_rgb(0xe6, 0xe6, 0xe6),
            Tone::Accent => Color32::from_rgb(0xe0, 0x8a, 0x5a),
            Tone::Dim => Color32::from_rgb(0x5c, 0x5c, 0x5c),
        };
        job.append(text, 0.0, egui::TextFormat::simple(font.clone(), color));
    }
    job
}

/// 時刻を薄く、本文を重さの色で。1 つのラベルにするのは、範囲選択でコピーしたときに時刻と本文が 1 行に並ぶようにするため。
fn normal_line(ui: &egui::Ui, line: &log::Line) -> egui::Label {
    let visuals = ui.visuals();
    let color = match line.level {
        Level::Info => visuals.text_color(),
        Level::Warn => visuals.warn_fg_color,
        Level::Error => visuals.error_fg_color,
    };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &format!("{}  ", line.clock()),
        0.0,
        egui::TextFormat::simple(font.clone(), visuals.weak_text_color()),
    );
    job.append(&line.text, 0.0, egui::TextFormat::simple(font, color));
    egui::Label::new(job).wrap()
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
        fit_side: None,
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

/// 書き出し先の名前の元。
fn folder_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| "images".into(), |n| n.to_string_lossy().into_owned())
}

/// 書き出し先の名前の元。`data.zip` の中なら `data-export` のように拡張子を除く。
fn zip_stem(zip: &Path) -> String {
    zip.file_stem()
        .map_or_else(|| "images".into(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_counts_are_grouped_by_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1234), "1,234");
        assert_eq!(thousands(1234567), "1,234,567");
    }

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
