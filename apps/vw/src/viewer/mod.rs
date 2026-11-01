//! 画像フィールドの 1 枠（スロット）。ヘッダ、ズーム、パン（移動）を受け持つ。
//!
//! ズームとパンの計算は [`geometry`]、マウス操作をツールごとに解釈する決まりは [`input`] に
//! 分けてあり、どちらも egui の `Ui` に依らないのでテストはそちらで確かめる。

pub mod geometry;
pub mod input;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{
    self, Color32, ColorImage, Pos2, Rect, Sense, TextureFilter, TextureHandle, TextureOptions,
    Vec2, pos2, vec2,
};
use egui_phosphor::regular as icon;
use serde::{Deserialize, Serialize};
use vw_core::{ColorModel, VwImage};

use crate::derive::{self, Derive, View};
use crate::faces::FaceMark;
use crate::i18n::{tr, tr1};
use crate::labels::{self, Label};
use crate::loader::ImageInfo;
use crate::probe::{self, Mark};
use crate::quantity::Quantity;
use crate::roi::{self, Region, Roi};
use crate::settings::{ColorFormat, Language};
pub use geometry::{
    KEY_ZOOM_FACTOR, Placed, ViewWindow, drag_zoom_factor, fit_scale, needs_full, pan_to_center,
    split_wheel, visible_part, wheel_delta, wheel_steps, zoom_at,
};
use geometry::{MARK_HIT, MAX_PIXEL_SCALE, MIN_ZOOM};
pub use input::Tool;
use input::{Click, Drag, Hits, Pointer, roi_kind};

/// スロットに出しているもの。描き方と右クリックの中身はこれで分かれる。
// `Image` だけ大きいが、スロット 1 つに 1 つしか持たず、画像を読むたびに作るものなので、
// Box に入れて間接参照を増やす意味がない。
#[allow(clippy::large_enum_variant)]
pub enum Content {
    /// 何も出していない。案内の文を出す。
    Empty,
    /// 読めなかった。理由をスロットに出す（ログ欄には失敗の数だけを出すため）。
    Failed(String),
    /// まだ開いていないフォルダ（ZIP も）。アイコンと名前だけを出す。
    Folder(PathBuf),
    Image(Shown),
}

/// スロットがある理由。中身の種類とは別の軸で、閉じたりフォルダの画像が増減したりしたときに
/// どのスロットを動かすかを決める。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// 利用者が開いた（ドロップ、ツリーの選択、合成など）。
    Free,
    /// フォルダの読み込みで 1 枚 1 スロットに並べた。フォルダの画像が増減したら、
    /// 中身を差し替えずにスロットを足したり閉じたりする。
    FolderImage,
    /// cd したフォルダの中のサブフォルダ（または ZIP）をエクスプローラのように並べたもの。
    /// 開いたあとも、このフォルダへ画像を運べる。
    Subfolder(PathBuf),
}

/// 表示中の画像。縮小画像のテクスチャと、ログ・メタデータ表示用の情報を持つ。
pub struct Shown {
    /// 表示中の画像のファイル。次の画像を読み込み中は [`Slot::path`] と食い違う。
    pub path: PathBuf,
    pub thumb: TextureHandle,
    pub info: ImageInfo,
    /// フル解像度の画像。縮小画像では足りない大きさで表示するときだけ持つ（[`crate::memory`]）。
    pub full: Option<Full>,
    /// フル解像度の画像を、普通に読む代わりに現像して作る（カメラ RAW）。
    /// 現像を頼んだときに立て、捨てたフル解像度の画像を読み直すときも現像する。
    pub developed: bool,
    /// 現像したカメラ RAW の、いま出していない方（埋め込みのプレビューか現像した画像）。
    /// [`Shown::swap_raw_face`] で出している方と入れ替える。
    pub other: Option<Box<RawFace>>,
}

/// カメラ RAW の片方の絵。埋め込みのプレビューか、現像した画像。
pub struct RawFace {
    pub thumb: TextureHandle,
    pub info: ImageInfo,
    pub full: Option<Full>,
    pub developed: bool,
}

pub struct Full {
    pub image: Arc<VwImage>,
    pub texture: TextureHandle,
    pub bytes: usize,
    /// 最後に必要とされたフレーム番号。
    pub last_used: u64,
}

impl Shown {
    /// 現像を頼んで、まだ届いていない。届くと情報が現像した画像のものになり、
    /// [`ImageInfo::develop_size`] がなくなる。
    pub fn developing(&self) -> bool {
        self.developed && self.info.develop_size.is_some()
    }

    /// 現像できなかった。普通に読む画像（埋め込みのプレビュー）で見続け、二度と現像を頼まない。
    /// 現像を頼む前に戻すだけだと、開き直すたびや D キーでまた失敗し、そのたびに印が回り出すため。
    /// 現像していなかったなら何もせず `false`。
    pub fn give_up_developing(&mut self) -> bool {
        if !self.developing() {
            return false;
        }
        self.developed = false;
        self.info.develop_size = None;
        true
    }

    /// 埋め込みのプレビューと現像した画像の両方を持っていて、いまプレビューを出している。
    pub fn showing_raw_preview(&self) -> bool {
        self.other.is_some() && !self.developed
    }

    /// 埋め込みのプレビューと現像した画像を入れ替える。片方しか持っていなければ何もせず `false`。
    pub fn swap_raw_face(&mut self) -> bool {
        let Some(other) = self.other.as_deref_mut() else {
            return false;
        };
        std::mem::swap(&mut self.thumb, &mut other.thumb);
        std::mem::swap(&mut self.info, &mut other.info);
        std::mem::swap(&mut self.full, &mut other.full);
        std::mem::swap(&mut self.developed, &mut other.developed);
        true
    }

    /// 描画に使うテクスチャ。フル解像度があればそれ、なければ縮小画像。
    pub fn texture(&self) -> &TextureHandle {
        self.full.as_ref().map_or(&self.thumb, |f| &f.texture)
    }
}

/// 画面の 1 画素に画像の画素をどう割り当てるか（拡大・縮小それぞれ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleFilter {
    /// いちばん近い画素の値をそのまま使う。画素の境界と元の値がそのまま見える。
    Nearest,
    /// まわりの画素の値を混ぜてなめらかにする。
    Linear,
}

impl ScaleFilter {
    pub const ALL: [ScaleFilter; 2] = [ScaleFilter::Nearest, ScaleFilter::Linear];

    pub fn title(self) -> &'static str {
        match self {
            ScaleFilter::Nearest => "Nearest neighbor",
            ScaleFilter::Linear => "Linear (smooth)",
        }
    }
}

/// スロットの画像のテクスチャの設定。
///
/// 縮小を線形にするときはミップマップも使う。使わないと、大きく縮めたときに間の画素が
/// 飛ばされてちらつく。最近傍の縮小ではミップマップを使わない。ミップマップの段は画素を
/// 平均した値なので、元の値だけを見せるという最近傍を選ぶ理由に反するため。
///
/// バイキュービックや Lanczos は選べない。GPU のテクスチャの補間には最近傍と線形しかなく、
/// egui の描画に自前のシェーダーを差し込む必要があって手間が大きいため。
pub fn texture_options(magnify: ScaleFilter, minify: ScaleFilter) -> TextureOptions {
    let filter = |f| match f {
        ScaleFilter::Nearest => TextureFilter::Nearest,
        ScaleFilter::Linear => TextureFilter::Linear,
    };
    TextureOptions {
        magnification: filter(magnify),
        minification: filter(minify),
        mipmap_mode: (minify == ScaleFilter::Linear).then_some(TextureFilter::Linear),
        ..TextureOptions::LINEAR
    }
}

/// 読み込み済みのテクスチャの拡大・縮小の設定だけを変える。
///
/// 0x0 の部分更新を送る。画素を送り直さずに済むよう、egui の glow の描画が部分更新のたびに
/// 補間の設定とミップマップを作り直すのを利用している。画像を読み直す方法は採らない。
/// 縮小画像の画素はテクスチャにしか残しておらず、全スロットをデコードし直すことになるため。
pub fn set_texture_options(texture: &mut TextureHandle, options: TextureOptions) {
    texture.set_partial([0, 0], ColorImage::new([0, 0], Vec::new()), options);
}

/// 塊のスロット（フォルダスロットなど）が送る画像の並び。
#[derive(Debug, Clone, PartialEq)]
pub struct Sequence {
    /// 並びの元。フォルダか、フレームの並びなら元のファイル。
    pub folder: PathBuf,
    pub kind: SequenceKind,
    pub paths: Vec<PathBuf>,
    /// 表示中（読み込み中）の画像の位置。
    pub index: usize,
    /// ファイルが勧めるスライドショーの速さ（シネのフレーム間隔から）。
    pub fps: Option<f32>,
    /// フレームの並びで、フレームごとに表示時間（ミリ秒）が書かれていればその値。`fps` はその平均。
    pub delays: Option<Arc<[u32]>>,
    /// ヘッダに出す並びの名前。`None` ならフォルダ名かファイル名。
    pub title: Option<String>,
}

/// 並びの種類。フォルダの中身が変わったときに並びを読み直すかが変わる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceKind {
    /// フォルダの画像すべて。
    Folder,
    /// 1 つのファイルのフレーム（[`crate::frames`]）。
    Frames,
    /// DICOM の 1 シリーズのファイル（[`crate::series`]）。
    Series,
    /// フォルダのスロットを開いて読んだ、サブフォルダまで含めた画像。
    Subtree,
}

impl Sequence {
    pub fn new(folder: PathBuf, first: PathBuf) -> Self {
        Self {
            folder,
            kind: SequenceKind::Folder,
            paths: vec![first],
            index: 0,
            fps: None,
            delays: None,
            title: None,
        }
    }

    /// `file` の `count` 枚のフレームの並び。
    pub fn frames(file: PathBuf, count: usize, fps: Option<f32>) -> Self {
        Self {
            paths: crate::frames::frame_paths(&file, count),
            folder: file,
            kind: SequenceKind::Frames,
            index: 0,
            fps,
            delays: None,
            title: None,
        }
    }

    /// フォルダのスロットの中身（サブフォルダまで含めた画像）。最初は空で、読み込みが足していく。
    pub fn subtree(folder: PathBuf) -> Self {
        Self {
            folder,
            kind: SequenceKind::Subtree,
            paths: Vec::new(),
            index: 0,
            fps: None,
            delays: None,
            title: None,
        }
    }

    /// フォルダ `folder` にある DICOM の 1 シリーズ。
    pub fn series(folder: PathBuf, title: String, files: Vec<PathBuf>) -> Self {
        Self {
            folder,
            kind: SequenceKind::Series,
            paths: files,
            index: 0,
            fps: None,
            delays: None,
            title: Some(title),
        }
    }
}

/// 画像フィールドの 1 枠。
pub struct Slot {
    /// スロットを識別する番号。並べ替えや削除で位置が変わっても読み込み結果を正しく届けるため、
    /// 配列の添字ではなくこれを使う。
    pub id: u64,
    pub path: Option<PathBuf>,
    /// 出しているもの。
    pub content: Content,
    /// スロットがある理由。
    pub origin: Origin,
    /// フォルダスロットなら、そのフォルダの画像の並び。普通のスロットは `None` で、
    /// 前後の画像はその都度同じフォルダから探す。
    pub sequence: Option<Sequence>,
    /// 最後に出した読み込み要求の番号。これと違う番号の結果は古いので捨てる。
    /// 前後の画像へ素早く送ったとき、遅れて届いた前の画像で上書きされるのを防ぐ。
    pub request: u64,
    /// 読み込みの要求が届いていない。`content` とは別の軸。次の画像を読んでいる間も
    /// 前の画像を出したままにするので（画像を送るたびに空にならないように）、
    /// 「読み込み中」を中身の種類にはしない。
    pub loading: bool,
    /// 読み込み中のフル解像度の画像の要求番号と、見込みバイト数。
    pub full_request: Option<(u64, usize)>,
    /// フル解像度の画像が届いたらコピーする（コピーしようとしたときに捨ててあった）。
    pub copy_when_full: bool,
    /// 直前のフレームで、縮小画像では足りない大きさで表示した。
    pub wants_full_now: bool,
    /// 「枠に収まる倍率」に掛ける倍率。1.0 で全体表示。
    /// 画像の実寸に対する倍率で持たないのは、枠の大きさが変わったときに全体表示を保つため。
    zoom: f32,
    /// 枠の中心から画像の中心までのずれ（論理ピクセル）。
    pan: Vec2,
    /// まだ 1 段に満たないホイールの回転量（論理ピクセル）。
    wheel: f32,
    /// まだ 1 目盛りに満たない Shift+ホイールの回転量。
    page_wheel: f32,
    /// 直前に描いた画像部分の大きさ。キー操作で倍率を変えるときに使う。
    last_body: Option<Vec2>,
    /// 画素値を確かめる十字の印。ファイルには書かず、スロットを閉じると消える。
    pub marks: Vec<Mark>,
    /// 十字と ROI の色の値の書き方。描くたびに [`SlotLook::colors`] から受け取り、
    /// コピー（描画の外から呼ばれる）でも同じ書き方にする。
    colors: Colors,
    /// 選んでいる十字（[`Slot::marks`] の添字）。
    selected_mark: Option<usize>,
    /// 関心領域。十字と同じく、ファイルには書かずスロットを閉じると消える。
    pub rois: Vec<Roi>,
    /// 選んでいる ROI（[`Slot::rois`] の添字）。
    selected_roi: Option<usize>,
    /// ドラッグで描いている途中の ROI（種類と始点の画素）。
    drawing: Option<(roi::Kind, (u32, u32))>,
    /// 右クリックした顔の枠（画像と、その画像の枠の番号）。
    selected_face: Option<(PathBuf, usize)>,
    /// 色導出。`None` は標準の表示。起動をまたいでは残さない。
    pub derive: Option<Derive>,
    /// 直前のフレームで、色導出の帯を画像に重ねて出したか。
    bar_floating: bool,
    /// 色の表とゲージで変えた黒と白。起動をまたいでは残さない。
    pub color: crate::colormap::Choice,
    /// ゲージの端をドラッグしている間の黒と白の値。離したら読み直しを頼む。
    gauge_drag: Option<(f32, f32)>,
    /// 8bit に落とさずに描く（10bit）か、ディザをかけて描く画像。1 枚を大きく出しているときだけ
    /// アプリが用意する（[`crate::deep`]）。
    pub deep: Option<crate::deep::Layer>,
}

/// スロットの描き方。スロットの外（設定やラベル）から決まるもの。
pub struct SlotLook<'a> {
    /// 画像の背後の色。
    pub background: Color32,
    pub lang: Language,
    /// 空のスロットに出す案内。
    pub empty_hint: &'static str,
    /// 表示中の画像のフラグとタグ。
    pub label: &'a Label,
    /// 画像の上のマウス操作の意味を決めるツール。
    pub tool: Tool,
    /// 下の帯のドロップダウンに並べる、保存したパイプライン。
    pub presets: &'a [crate::pipelines::Preset],
    /// 色導出で描いた画像のテクスチャの補間。
    pub texture_options: TextureOptions,
    /// 表示中の画像で見つけた顔の枠。空なら描かない。
    pub faces: &'a [FaceMark],
    pub colors: Colors,
    /// 色の表のゲージ。`None` はゲージを出さない。
    pub gauge: Option<Gauge>,
    pub color_rules: &'a crate::colormap::Rules,
    /// 似た画像の組に入っていれば、見出しに出す印。
    pub similar: Option<&'a SimilarMark>,
    /// 電波の画像のビームの楕円を描く。
    pub beam: bool,
    /// スペクトルを画像ではなくグラフで描く。
    pub spectrum_graph: bool,
}

/// 似た画像の組の印。同じ組のスロットには同じ番号と色を出す。
pub struct SimilarMark {
    /// フォルダの中での組の番号（1 から）。
    pub number: usize,
    pub color: Color32,
    /// 同じ組のほかの画像の名前（印にマウスを乗せたときに出す）。
    pub others: String,
    /// 組の中での残す候補の順位（1 から、[`vw_compare::pairs::ranked`]）。鮮鋭度の分からない画像だけの組では `None`。
    pub rank: Option<usize>,
    /// 組の画像の数。
    pub of: usize,
}

/// ゲージの出し方。
pub struct Gauge {
    pub side: crate::settings::GaugeSide,
    /// 色の表を当てていない灰色の画像にも出す。
    pub on_gray: bool,
}

/// 色の値の書き方（十字は `metadata.color_format`、ROI は `roi.color_format`）と、顔の枠の色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Colors {
    pub probe: ColorFormat,
    pub roi: ColorFormat,
    /// 人物が分かった顔の枠。
    pub face_known: Color32,
    /// 人物が分からない顔の枠。
    pub face_unknown: Color32,
}

/// 顔の枠の右クリックで選ばれた操作。番号は表示中の画像の枠の番号。
#[derive(Debug, Clone, PartialEq)]
pub enum FaceAction {
    Assign(usize, String),
    NewPerson(usize),
    Clear(usize),
}

/// スロットのヘッダからドラッグしている画像のファイル（egui のドラッグ＆ドロップの中身）。
pub struct DraggedFile(pub PathBuf);

/// 1 フレームの操作で起きたこと。スロットは「何が起きたか」だけを返し、実行はアプリ側
/// （`app/slots.rs`）がする。右クリックメニューはスロットでは描かず、アプリが操作の表から
/// 描く（`app/context_menu.rs`）ので、ここには見出し・本体・帯の操作だけがある。真偽値の旗の束ではなく列にしてあるのは、操作を足すときに
/// 旗と受け取り側の `if` を対で足し忘れないようにするため（`match` が網羅を強いる）。
pub enum SlotCommand {
    /// クリックやメニューの操作で、このスロットをアクティブにする。
    Activate,
    Close,
    /// ダブルクリック（最大化の切り替え。閉じたフォルダなら中を開く）。
    ToggleMaximize,
    /// ホイールで送る枚数。負なら前へ。
    Step(isize),
    /// Shift+ホイールで送るページ数。負なら前へ。
    PageStep(isize),
    /// フォルダのスロットの「このフォルダへ移動」が押された。
    OpenFolder,
    /// フォルダのスロットに画像がドロップされた。
    Drop(PathBuf),
    /// 見出しの「JPEG」「RAW」を押した（埋め込みのプレビューと現像した画像の切り替え）。
    SwapRawFace,
    /// ゲージで色の表を選んだ。
    ColorPick(crate::colormap::Pick),
    /// ゲージで黒と白の値を変えた（`None` はファイルの値に戻す）。
    Window(Option<(f32, f32)>),
    /// 見た目の入った保存したパイプラインを選んだ。スロットの黒と白・曲線・色の表に当てる。
    ApplyLook(crate::pipelines::Look),
    /// 下の帯の「Edit」で「画素の変換」を開く。
    OpenPipeline,
    /// 見出しの似た画像の印がクリックされた（組を比べる）。
    CompareSimilar,
    /// 似た画像の印のメニューで「これを残し、ほかを Bad にする」が選ばれた。
    KeepInGroup,
    /// 色導出から知らせること（ログに出す）。
    Derive(derive::Event),
}

/// 1 フレーム分の操作結果。
#[derive(Default)]
pub struct SlotFrame {
    /// 起きたことの列。起きた順。
    pub commands: Vec<SlotCommand>,
    /// 縮小画像では足りない大きさで表示している。
    pub wants_full: bool,
    /// 画像をドラッグしている（並べ替えやフォルダへの移動の元）。
    pub dragging: bool,
    /// 本体の応答。アプリが右クリックメニューを付ける。
    pub body: Option<egui::Response>,
}

impl SlotFrame {
    fn push(&mut self, command: SlotCommand) {
        self.commands.push(command);
    }
}

/// ヘッダ（ファイル名とズーム率の帯）の高さ。
const HEADER: f32 = 20.0;
/// 色導出を選ぶ下の帯の高さ。
const DERIVE_BAR: f32 = 22.0;
/// 十字の腕の長さ（論理ピクセル）。
const MARK_ARM: f32 = 8.0;

/// ROI の色。対象は赤、背景は青（品質評価の資料と同じ色分け）。
fn roi_color(kind: roi::Kind) -> Color32 {
    match kind {
        roi::Kind::Target => Color32::from_rgb(255, 80, 80),
        roi::Kind::Background => Color32::from_rgb(90, 160, 255),
    }
}

/// ホイールの回転量 `delta`（論理ピクセル、上に回すと正）を `acc` に足し、
/// 1 段分たまるごとに送る枚数を返す。上に回すと前（負）、下に回すと次（正）。
///
/// タッチパッドは細かい量を何度も送ってくるので、1 回のイベントを 1 枚にはしない。
/// 回す向きが変わったら、それまでの端数は捨てる。逆向きに回し始めたのに、
/// 前の向きの端数を打ち消すまで反応しないのを避けるため。
/// ヘッダの右側に並べる小さなボタン。マウスが乗ると面を敷いて濃くする。
fn header_button(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    icon: &str,
    size: f32,
) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(id), Sense::click());
    let visuals = ui.visuals();
    let color = if response.hovered() {
        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
        visuals.strong_text_color()
    } else {
        visuals.text_color()
    };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(size),
        color,
    );
    response
}

/// 色導出の対象。画像ならどの形式でも（ノードは標準の表示の後ろに当てる）。
fn is_derivable(_shown: &Shown) -> bool {
    true
}

/// 色導出の名前。保存したあとに変えた値は「*」を付ける。
fn derive_name(d: &Derive, lang: Language) -> String {
    match &d.preset {
        Some(name) if d.modified => format!("{name} *"),
        Some(name) => name.clone(),
        None => tr(lang, "Unsaved").to_owned(),
    }
}

/// 電波の画像のビーム（分解能）を、見えている画像の左下に楕円で描く。
fn draw_beam(
    ui: &egui::Ui,
    lang: Language,
    body: Rect,
    img_rect: Rect,
    scale: f32,
    meta: &vw_core::Metadata,
) {
    let Some(beam) = vw_fits::radio::beam(meta) else {
        return;
    };
    // 画面の y は下向き。上下を返して見せているときは FITS の y と逆になる。
    let flipped = meta.get(vw_fits::GROUP, vw_fits::ROWS) == Some(vw_fits::ROWS_FLIPPED);
    let to_screen = |[x, y]: [f64; 2]| {
        let y = if flipped { -y } else { y };
        vec2(x as f32, y as f32) * scale
    };
    let (a, b) = (to_screen(beam.major), to_screen(beam.minor));
    let reach = vec2(a.x.abs().max(b.x.abs()), a.y.abs().max(b.y.abs())).max(vec2(3.0, 3.0));
    let shown = img_rect.intersect(body);
    if shown.width() < 4.0 * reach.x || shown.height() < 4.0 * reach.y {
        return;
    }
    let center = shown.left_bottom() + vec2(reach.x + 8.0, -(reach.y + 8.0));
    let points: Vec<Pos2> = (0..48)
        .map(|i| {
            let t = i as f32 / 48.0 * std::f32::consts::TAU;
            center + a * t.cos() + b * t.sin()
        })
        .collect();
    let painter = ui.painter_at(body);
    // 電波の図の慣習どおり、四角の枠の中に塗った楕円を描く。
    let frame = Rect::from_center_size(center, 2.0 * reach + vec2(8.0, 8.0));
    painter.rect_filled(frame, 2.0, Color32::from_black_alpha(140));
    painter.add(egui::Shape::convex_polygon(
        points,
        Color32::from_white_alpha(110),
        egui::Stroke::new(1.0, Color32::from_gray(235)),
    ));
    let response = ui.interact(frame, ui.id().with("beam"), Sense::hover());
    response.on_hover_text(format!(
        "{} {:.2}\" × {:.2}\", PA {:.1}°",
        crate::i18n::tr(lang, "Beam"),
        beam.bmaj_arcsec,
        beam.bmin_arcsec,
        beam.bpa_deg
    ));
}

/// 複数枚の並びの「何枚目 / 全部で何枚」。1 枚しかなければ出さない。
pub fn page_text(seq: &Sequence) -> Option<String> {
    (seq.paths.len() > 1).then(|| format!("{} / {}", seq.index + 1, seq.paths.len()))
}

impl Slot {
    /// 中身をまだ開いていないフォルダのスロット。開き始めたら（読み込み中）は違う。
    pub fn is_closed_folder(&self) -> bool {
        matches!(self.content, Content::Folder(_)) && !self.loading
    }

    /// 表示中の画像。
    pub fn shown(&self) -> Option<&Shown> {
        match &self.content {
            Content::Image(shown) => Some(shown),
            _ => None,
        }
    }

    pub fn shown_mut(&mut self) -> Option<&mut Shown> {
        match &mut self.content {
            Content::Image(shown) => Some(shown),
            _ => None,
        }
    }

    /// サブフォルダのスロットなら、そのフォルダ（または ZIP）。
    pub fn folder(&self) -> Option<&Path> {
        match &self.origin {
            Origin::Subfolder(folder) => Some(folder),
            _ => None,
        }
    }

    /// フォルダの読み込みで 1 枚 1 スロットに並べたスロット。
    pub fn is_folder_image(&self) -> bool {
        self.origin == Origin::FolderImage
    }

    /// サブフォルダのスロットにする。中身はまだ開いていないフォルダの姿。
    pub fn set_subfolder(&mut self, folder: PathBuf) {
        self.content = Content::Folder(folder.clone());
        self.origin = Origin::Subfolder(folder);
    }

    pub fn new(id: u64) -> Self {
        Self {
            id,
            path: None,
            content: Content::Empty,
            origin: Origin::Free,
            sequence: None,
            request: 0,
            loading: false,
            full_request: None,
            copy_when_full: false,
            wants_full_now: false,
            zoom: 1.0,
            pan: Vec2::ZERO,
            wheel: 0.0,
            page_wheel: 0.0,
            last_body: None,
            marks: Vec::new(),
            colors: Colors::default(),
            selected_mark: None,
            rois: Vec::new(),
            selected_roi: None,
            drawing: None,
            selected_face: None,
            derive: None,
            bar_floating: false,
            color: Default::default(),
            gauge_drag: None,
            deep: None,
        }
    }

    /// 表示中の画像の ROI のうち、`kind` のもの。種類ごとに 1 つだけ持つ。
    pub fn roi_of(&self, kind: roi::Kind) -> Option<Region> {
        let shown = self.shown()?;
        self.rois
            .iter()
            .find(|r| r.path == shown.path && r.kind == kind)
            .map(|r| r.region)
    }

    fn selected_shown_roi(&self) -> Option<&Roi> {
        let shown = self.shown()?;
        self.selected_roi
            .and_then(|i| self.rois.get(i))
            .filter(|r| r.path == shown.path)
    }

    /// ROI の中の統計。元の画像を読み込み中なら `None`。
    fn roi_stats(&self, region: Region) -> Option<crate::histogram::Stats> {
        let shown = self.shown()?;
        let full = shown.full.as_ref()?;
        Some(roi::stats(&full.image, &shown.info.metadata, region))
    }

    /// カラー画像の ROI の中の色の平均。元の画像を読み込み中やモノクロなら `None`。
    fn roi_color_means(&self, region: Region) -> Option<[f64; 3]> {
        let full = self.shown()?.full.as_ref()?;
        roi::color_means(&full.image, region)
    }

    /// 選んでいる ROI をコピーするときの文字。
    pub fn selected_roi_text(&self) -> Option<String> {
        let r = self.selected_shown_roi()?;
        let stats = self.roi_stats(r.region);
        Some(roi::clipboard_text(
            r.kind.title(),
            r.region,
            stats.as_ref(),
        ))
    }

    /// 選んでいる ROI を消す。消したら `true`。
    /// 右クリックした顔の枠（画像と、その画像の枠の番号）。
    pub fn selected_face(&self) -> Option<(&Path, usize)> {
        self.selected_face
            .as_ref()
            .map(|(path, i)| (path.as_path(), *i))
    }

    pub fn delete_selected_roi(&mut self) -> bool {
        if self.selected_shown_roi().is_none() {
            return false;
        }
        if let Some(i) = self.selected_roi.take() {
            self.rois.remove(i);
        }
        true
    }

    /// 表示中の画像の ROI を描く。対象は赤、背景は青の破線。
    fn draw_rois(
        &self,
        ui: &egui::Ui,
        body: Rect,
        lang: Language,
        placed: &Placed,
        drawing: Option<(roi::Kind, Region)>,
    ) {
        let Some(shown) = self.shown() else { return };
        let painter = ui.painter_at(body);
        let selected = self.selected_shown_roi();
        let quantity = Quantity::of(&shown.info.metadata);
        let bits = shown.info.bits_per_sample;
        let shown_rois = self
            .rois
            .iter()
            .filter(|r| r.path == shown.path)
            .map(|r| (r.kind, r.region, selected == Some(r), true));
        let in_progress = drawing.map(|(kind, region)| (kind, region, false, false));
        for (kind, region, is_selected, finished) in shown_rois.chain(in_progress) {
            let rect = placed.region_rect(region);
            // 選んでいる ROI は太くする。選択色に変えると背景（青）の ROI と見分けがつかないため。
            let stroke_color = roi_color(kind);
            let width = if is_selected { 2.5 } else { 1.0 };
            let corners = [
                rect.left_top(),
                rect.right_top(),
                rect.right_bottom(),
                rect.left_bottom(),
                rect.left_top(),
            ];
            // 明るい画像でも暗い画像でも見えるよう、黒い縁の上に破線を描く。
            painter.add(egui::Shape::closed_line(
                corners[..4].to_vec(),
                egui::Stroke::new(3.0, Color32::from_black_alpha(140)),
            ));
            for pair in corners.windows(2) {
                painter.add(egui::Shape::dashed_line(
                    pair,
                    egui::Stroke::new(width, stroke_color),
                    6.0,
                    4.0,
                ));
            }
            let stats = finished.then(|| self.roi_stats(region)).flatten();
            let means = finished.then(|| self.roi_color_means(region)).flatten();
            let color_text = means.map(|m| crate::color_text::text(&m, bits, self.colors.roi));
            let swatch = means.and_then(|m| crate::color_text::swatch(&m, bits));
            let text = roi::label(
                tr(lang, kind.title()),
                region,
                stats.as_ref(),
                &quantity,
                color_text.as_deref(),
            );
            let galley =
                painter.layout_no_wrap(text, egui::FontId::proportional(12.0), stroke_color);
            let pad = vec2(4.0, 1.0);
            let size = galley.size() + 2.0 * pad + swatch_room(swatch, galley.size().y);
            // 矩形の上に出し、枠の上端で切れるときは矩形の中に入れる。
            let y = if rect.top() - size.y - 2.0 >= body.top() {
                rect.top() - size.y - 2.0
            } else {
                rect.top() + 2.0
            };
            let x = rect.left().max(body.left()).min(body.right() - size.x);
            let pill = Rect::from_min_size(pos2(x, y), size);
            painter.rect_filled(pill, 2.0, Color32::from_black_alpha(160));
            let text_at = draw_swatch(&painter, pill.min + pad, galley.size().y, swatch);
            painter.galley(text_at, galley, stroke_color);
        }
    }

    /// 表示中の画像に置いた十字のうち、選んでいるもの。
    fn selected_shown_mark(&self) -> Option<&Mark> {
        let shown = self.shown()?;
        self.selected_mark
            .and_then(|i| self.marks.get(i))
            .filter(|m| m.path == shown.path)
    }

    /// 十字の位置の値。元の画像を読み込み中なら `None`。
    fn mark_value(&self, mark: &Mark) -> Option<String> {
        self.mark_sample(mark).map(|(text, _)| text)
    }

    /// 十字の位置の値と、カラーならその色の見本。
    fn mark_sample(&self, mark: &Mark) -> Option<(String, Option<[u8; 3]>)> {
        let shown = self.shown()?;
        let full = shown.full.as_ref()?;
        let values = probe::sample(&full.image, &shown.info.metadata, mark.x, mark.y)?;
        let bits = shown.info.bits_per_sample;
        let text = probe::value_text(
            full.image.color,
            &shown.info.metadata,
            &values,
            self.colors.probe,
            bits,
        );
        let swatch = matches!(full.image.color, ColorModel::Rgb | ColorModel::Rgba)
            .then(|| crate::color_text::swatch(&values, bits))
            .flatten();
        Some((text, swatch))
    }

    /// 選んでいる十字をコピーするときの文字。
    pub fn selected_mark_text(&self) -> Option<String> {
        let mark = self.selected_shown_mark()?;
        let value = self.mark_value(mark);
        Some(probe::clipboard_text(mark.x, mark.y, value.as_deref()))
    }

    /// 選んでいる十字を消す。消したら `true`。
    pub fn delete_selected_mark(&mut self) -> bool {
        if self.selected_shown_mark().is_none() {
            return false;
        }
        if let Some(i) = self.selected_mark.take() {
            self.marks.remove(i);
        }
        true
    }

    /// 枠の中心を動かさずに `factor` 倍する。
    pub fn zoom_by(&mut self, factor: f32) {
        let base = self.last_base();
        (self.zoom, self.pan) = zoom_at(self.zoom, self.pan, factor, base, Some(Vec2::ZERO));
    }

    /// 実寸（表示倍率 100%）にする。枠の中心にある点はそのまま。
    pub fn actual_size(&mut self) {
        let base = self.last_base();
        let factor = 1.0 / (base * self.zoom);
        self.zoom_by(factor);
    }

    fn last_base(&self) -> f32 {
        match self.last_body {
            Some(size) => self.base_scale(Rect::from_min_size(Pos2::ZERO, size)),
            None => 1.0,
        }
    }

    /// 今の表示（まだ一度も描いていなければ `None`）。
    pub fn view_window(&self) -> Option<ViewWindow> {
        let (shown, body) = (self.shown()?, self.last_body?);
        let base = self.last_base();
        let image = vec2(shown.info.width as f32, shown.info.height as f32);
        let scale = base * self.zoom;
        Some(ViewWindow {
            visible: visible_part(body, image, scale, self.pan),
            scale,
            min_scale: base * MIN_ZOOM,
            max_scale: MAX_PIXEL_SCALE,
        })
    }

    /// 画像の `at`（[`visible_part`] と同じ割合）を枠の中心に持ってくる。倍率は変えない。
    pub fn center_at(&mut self, at: Pos2) {
        let Some(shown) = self.shown() else { return };
        let image = vec2(shown.info.width as f32, shown.info.height as f32);
        self.pan = pan_to_center(at, image, self.last_base() * self.zoom);
    }

    /// 実寸に対する表示倍率を `scale` にする。枠の中心にある点はそのまま。
    pub fn set_scale(&mut self, scale: f32) {
        let current = self.last_base() * self.zoom;
        if current > 0.0 {
            self.zoom_by(scale / current);
        }
    }

    /// 全体表示・中央に戻す。
    pub fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    fn base_scale(&self, body: Rect) -> f32 {
        let Some(s) = self.shown() else { return 1.0 };
        // テクスチャの大きさではなく元画像の大きさを使う。テクスチャは上限に合わせて
        // 縮小されていることがあり、それで計算すると表示倍率が実寸とずれるため。
        fit_scale(body.size(), vec2(s.info.width as f32, s.info.height as f32))
    }

    /// 表示中の画像に置いた十字と、その値を描く。
    fn draw_marks(&self, ui: &egui::Ui, body: Rect, lang: Language, placed: &Placed) {
        let Some(shown) = self.shown() else { return };
        let painter = ui.painter_at(body);
        let selected = self.selected_shown_mark();
        let missing = Quantity::of(&shown.info.metadata).missing().map(|m| {
            format!(
                "{} {}",
                tr(lang, "SUVbw needs:"),
                crate::quantity::missing_text(lang, m)
            )
        });
        for mark in self.marks.iter().filter(|m| m.path == shown.path) {
            let at = placed.mark_pos(mark);
            let color = if selected == Some(mark) {
                ui.visuals().selection.stroke.color
            } else {
                Color32::from_rgb(255, 210, 0)
            };
            // 明るい画像でも暗い画像でも見えるよう、黒い縁を付けて描く。
            for (width, c) in [(3.0, Color32::from_black_alpha(180)), (1.0, color)] {
                let stroke = egui::Stroke::new(width, c);
                painter.line_segment([at - vec2(MARK_ARM, 0.0), at + vec2(MARK_ARM, 0.0)], stroke);
                painter.line_segment([at - vec2(0.0, MARK_ARM), at + vec2(0.0, MARK_ARM)], stroke);
            }
            let sample = self.mark_sample(mark);
            let swatch = sample.as_ref().and_then(|(_, s)| *s);
            let value = sample.map(|(text, _)| crate::i18n::tr_text(lang, &text));
            let galley = painter.layout_no_wrap(
                probe::mark_label(mark.x, mark.y, value.as_deref()),
                egui::FontId::proportional(12.0),
                color,
            );
            let pad = vec2(4.0, 1.0);
            let size = galley.size() + 2.0 * pad + swatch_room(swatch, galley.size().y);
            let gap = MARK_ARM + 3.0;
            // ふだんは右上に出し、枠の端で切れるときは反対側に回す。
            // どちらにも入りきらない狭い枠では、左端に寄せて頭の値を残す。
            let x = if at.x + gap + size.x <= body.right() {
                at.x + gap
            } else {
                (at.x - gap - size.x).max(body.left())
            };
            let y = if at.y - gap - size.y >= body.top() {
                at.y - gap - size.y
            } else {
                at.y + gap
            };
            let pill = Rect::from_min_size(pos2(x, y), size);
            painter.rect_filled(pill, 2.0, Color32::from_black_alpha(160));
            let text_at = draw_swatch(&painter, pill.min + pad, galley.size().y, swatch);
            painter.galley(text_at, galley, color);
            // 値の札は描いただけの絵なので、ウィジェットにせず、カーソルが乗ったかを自分で見る。
            // 札を hover のウィジェットにすると、その上ではスロットの本体が hover でなくなり、
            // ホイールやドラッグが効かなくなるため。
            if let Some(missing) = missing.as_ref().filter(|_| ui.rect_contains_pointer(pill)) {
                let id = ui.id().with(("mark-missing", mark.x, mark.y));
                egui::Tooltip::always_open(
                    ui.ctx().clone(),
                    ui.layer_id(),
                    id,
                    egui::PopupAnchor::Pointer,
                )
                .show(|ui| ui.label(missing));
            }
        }
    }

    /// `rect` の範囲にスロットを描き、操作を処理する。
    /// 比べるときの仕切りと左右の名前、差の最大。
    fn derive_overlay(&mut self, ui: &egui::Ui, body: Rect, lang: Language) {
        let (Some(d), Content::Image(shown)) = (&mut self.derive, &self.content) else {
            return;
        };
        let painter = ui.painter_at(body);
        let pill = |text: String, anchor: Pos2, align: egui::Align2| {
            let galley = painter.layout_no_wrap(
                text,
                egui::FontId::proportional(12.0),
                Color32::from_gray(230),
            );
            let pad = vec2(5.0, 2.0);
            let rect = align.anchor_size(anchor, galley.size() + 2.0 * pad);
            painter.rect_filled(rect, 3.0, Color32::from_black_alpha(150));
            painter.galley(rect.min + pad, galley, Color32::from_gray(230));
        };
        if let Some(reason) = d.not_applied(&shown.path) {
            pill(
                tr(lang, reason).to_owned(),
                body.right_bottom() + vec2(-6.0, -6.0),
                egui::Align2::RIGHT_BOTTOM,
            );
            return;
        }
        let Some(ready) = d.ready_for(shown) else {
            return;
        };
        match d.view {
            View::Compare => {
                let x = body.left() + body.width() * d.split;
                let handle = Rect::from_x_y_ranges(x - 4.0..=x + 4.0, body.y_range());
                let response = ui
                    .interact(handle, ui.id().with(("split", self.id)), Sense::drag())
                    .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                if response.dragged() {
                    let moved = d.split + response.drag_delta().x / body.width().max(1.0);
                    d.split = moved.clamp(0.0, 1.0);
                }
                let color = if response.hovered() || response.dragged() {
                    ui.visuals().selection.stroke.color
                } else {
                    Color32::from_gray(230)
                };
                // 白い画像の上でも黒い画像の上でも見えるよう、暗い縁取りの上に明るい線を引く。
                painter.vline(
                    x,
                    body.y_range(),
                    egui::Stroke::new(3.0, Color32::from_black_alpha(160)),
                );
                painter.vline(x, body.y_range(), egui::Stroke::new(1.0, color));
                let top = body.top() + 6.0;
                pill(
                    tr(lang, "Standard").to_owned(),
                    pos2(x - 6.0, top),
                    egui::Align2::RIGHT_TOP,
                );
                pill(
                    derive_name(d, lang),
                    pos2(x + 6.0, top),
                    egui::Align2::LEFT_TOP,
                );
            }
            View::Difference => {
                let text = if ready.max == 0 {
                    tr(lang, "No difference").to_owned()
                } else {
                    format!("{} {}", tr(lang, "Max"), ready.max)
                };
                pill(
                    text,
                    body.right_bottom() + vec2(-6.0, -6.0),
                    egui::Align2::RIGHT_BOTTOM,
                );
            }
            View::Derived => {}
        }
    }

    /// 下の帯: 保存したパイプラインのドロップダウン、編集、比較、差。
    fn derive_bar(
        &mut self,
        ui: &mut egui::Ui,
        bar: Rect,
        lang: Language,
        presets: &[crate::pipelines::Preset],
        out: &mut SlotFrame,
    ) {
        let inner = bar.shrink2(vec2(4.0, 1.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.spacing_mut().button_padding = vec2(6.0, 1.0);
        child.spacing_mut().interact_size.y = inner.height() - 2.0;
        let ui = &mut child;
        let selected = self
            .derive
            .as_ref()
            .map_or_else(|| tr(lang, "Standard").to_owned(), |d| derive_name(d, lang));
        let mut choice: Option<Option<&crate::pipelines::Preset>> = None;
        vw_ui::menu::combo(("derive", self.id))
            .selected_text(selected)
            .width((inner.width() * 0.35).clamp(70.0, 180.0))
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(self.derive.is_none(), tr(lang, "Standard"))
                    .clicked()
                {
                    choice = Some(None);
                }
                if presets.is_empty() {
                    ui.weak(tr(lang, "Save a pipeline with Edit to choose it here."));
                }
                for p in presets {
                    let current = self.derive.as_ref().is_some_and(|d| {
                        d.preset.as_deref() == Some(p.name.as_str()) && !d.modified
                    });
                    if ui.selectable_label(current, &p.name).clicked() {
                        choice = Some(Some(p));
                    }
                }
            });
        match choice {
            Some(None) => self.derive = None,
            Some(Some(p)) => {
                let view = self.derive.as_ref().map_or(View::Derived, |d| d.view);
                let split = self.derive.as_ref().map_or(0.5, |d| d.split);
                let mut d = Derive::new(Some(p.name.clone()), p.values.clone());
                d.view = view;
                d.split = split;
                self.derive = Some(d);
                if let Some(look) = p.look.clone() {
                    out.push(SlotCommand::ApplyLook(look));
                }
            }
            None => {}
        }
        if ui
            .button(tr(lang, "Edit"))
            .on_hover_text(tr(lang, "Open the pixel pipeline for this slot"))
            .clicked()
        {
            out.push(SlotCommand::OpenPipeline);
        }
        let on = self.derive.is_some();
        let view = self.derive.as_ref().map(|d| d.view);
        for (target, label, hover) in [
            (
                View::Compare,
                "Compare",
                "Left: standard, right: derived. Drag the line to move it.",
            ),
            (
                View::Difference,
                "Difference",
                "Brightness difference from the standard display: black (none) to red, yellow and white (largest).",
            ),
        ] {
            let response = ui
                .add_enabled(
                    on,
                    egui::Button::selectable(view == Some(target), tr(lang, label)),
                )
                .on_hover_text(tr(lang, hover));
            if response.clicked() {
                if let Some(d) = &mut self.derive {
                    d.view = if d.view == target {
                        View::Derived
                    } else {
                        target
                    };
                }
                out.push(SlotCommand::Activate);
            }
        }
        if self.derive.as_ref().is_some_and(Derive::busy) {
            ui.spinner();
        }
        if let Some((index, message)) = self.derive.as_ref().and_then(Derive::error) {
            ui.colored_label(ui.visuals().error_fg_color, icon::WARNING)
                .on_hover_text(format!("{} {}: {message}", tr(lang, "Node"), index + 1));
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, rect: Rect, active: bool, look: SlotLook) -> SlotFrame {
        let SlotLook {
            background,
            lang,
            empty_hint,
            label: image_label,
            tool,
            presets,
            texture_options,
            faces,
            colors,
            gauge,
            color_rules,
            similar,
            beam,
            spectrum_graph,
        } = look;
        self.colors = colors;
        let mut out = SlotFrame::default();
        let visuals = ui.visuals().clone();
        // グラフには画素がないので、十字・ROI・色導出は当てない。
        let graph = spectrum_graph && self.shown().is_some_and(|s| s.info.spectrum.is_some());
        let tool = match tool {
            Tool::Probe | Tool::Roi if graph => Tool::Hand,
            tool => tool,
        };
        // ゲージとビームは画像の色と大きさのためのもので、グラフに重ねると線が隠れる。
        let (gauge, beam) = (gauge.filter(|_| !graph), beam && !graph);
        let derivable = !graph && self.shown().is_some_and(is_derivable);
        if !derivable {
            self.derive = None;
        }
        let derive_event = match (&mut self.derive, &self.content) {
            (Some(d), Content::Image(shown)) => d.update(ui.ctx(), shown, texture_options),
            _ => None,
        };
        if let Some(event) = derive_event {
            if matches!(event, derive::Event::Refused { left: true, .. }) {
                self.derive = None;
            }
            out.push(SlotCommand::Derive(event));
        }
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
        let hovering = ui.rect_contains_pointer(rect);
        let bar_fits = derivable && rect.height() > HEADER + 3.0 * DERIVE_BAR;
        // 標準の表示のスロットでは、色導出の帯はマウスを乗せたときだけ画像の下端に重ねる。
        // 常に場所を取ると、並べたスロットのどれにも同じ行が並んで画像が狭くなるため。
        // 帯の ▼ で開いた一覧へマウスを動かすとスロットの外に出るので、一覧が開いている間は残す。
        let bar_height = if bar_fits && self.derive.is_some() {
            DERIVE_BAR
        } else {
            0.0
        };
        let floating = bar_fits
            && self.derive.is_none()
            && (hovering || (self.bar_floating && egui::Popup::is_any_open(ui.ctx())));
        self.bar_floating = floating;
        let body = Rect::from_min_max(
            pos2(rect.min.x, rect.min.y + HEADER),
            pos2(rect.max.x, rect.max.y - bar_height),
        );
        let bar = Rect::from_min_max(pos2(rect.min.x, body.max.y), rect.max);
        let painter = ui.painter_at(rect);

        painter.rect_filled(body, 0.0, background);
        let header_fill = if active {
            visuals.selection.bg_fill
        } else {
            visuals.faint_bg_color
        };
        painter.rect_filled(header, 0.0, header_fill);
        if bar_height > 0.0 {
            painter.rect_filled(bar, 0.0, visuals.faint_bg_color);
        }

        let response = ui.interact(
            body,
            ui.id().with(("slot", self.id)),
            Sense::click_and_drag(),
        );
        let pointer = Pointer::read(&response);
        if pointer.clicked || pointer.drag_started || pointer.secondary_clicked {
            out.push(SlotCommand::Activate);
        }
        let drop_target = self
            .folder()
            .filter(|f| !crate::archive::is_archive(f))
            .is_some();
        if drop_target {
            if response.dnd_hover_payload::<DraggedFile>().is_some() {
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(2.0, visuals.selection.stroke.color),
                    egui::StrokeKind::Inside,
                );
            }
            if let Some(file) = response.dnd_release_payload::<DraggedFile>() {
                out.push(SlotCommand::Drop(file.0.clone()));
            }
        }
        if pointer.double_clicked && tool.maximizes_on_double_click() {
            out.push(SlotCommand::ToggleMaximize);
        }
        if pointer.hovered {
            let (wheel, shifted) = ui.input(|i| split_wheel(&i.events, i.viewport_rect().height()));
            let step = wheel_steps(&mut self.wheel, wheel);
            if step != 0 {
                out.push(SlotCommand::Step(step));
            }
            let page_step = wheel_steps(&mut self.page_wheel, shifted);
            if page_step != 0 {
                out.push(SlotCommand::PageStep(page_step));
            }
        }

        self.last_body = Some(body.size());
        let base = self.base_scale(body);
        // `content` だけを借りて、パンや ROI の描きかけなどほかの欄を変えられるようにする。
        if let Content::Image(shown) = &self.content {
            let drag = tool.drag();
            match drag {
                Drag::Pan if pointer.dragged => self.pan += pointer.drag_delta,
                // サブフォルダのスロットの画像は、そのフォルダの中のものなので運ばない。
                Drag::CarryFile if self.folder().is_none() => {
                    response.dnd_set_drag_payload(DraggedFile(crate::frames::file_of(&shown.path)));
                    out.dragging |= pointer.dragged;
                }
                _ => {}
            }
            if pointer.clicked
                && let Some(factor) = tool.zoom_on_click(pointer.alt)
            {
                let cursor = pointer.pos.map(|p| p - body.center());
                (self.zoom, self.pan) = zoom_at(self.zoom, self.pan, factor, base, cursor);
            }
            if pointer.dragged_secondary || (drag == Drag::Zoom && pointer.dragged) {
                let cursor = pointer.press_origin.map(|p| p - body.center());
                let factor = drag_zoom_factor(pointer.drag_delta.y);
                (self.zoom, self.pan) = zoom_at(self.zoom, self.pan, factor, base, cursor);
            }
            if pointer.hovered {
                // タッチパッドのピンチと Ctrl+ホイール（egui がズームとして届ける）。
                let pinch = ui.input(|i| i.zoom_delta());
                // ピンチがなければ egui はちょうど 1.0 を返す。
                if pinch != 1.0 {
                    let cursor = pointer.hover.map(|p| p - body.center());
                    (self.zoom, self.pan) = zoom_at(self.zoom, self.pan, pinch, base, cursor);
                }
            }
            let placed = Placed::new(
                body,
                shown.info.width,
                shown.info.height,
                base * self.zoom,
                self.pan,
            );
            let (img_rect, scale) = (placed.rect, placed.scale);
            let [tw, th] = shown.thumb.size();
            out.wants_full = needs_full(
                placed.size().max_elem() * ui.ctx().pixels_per_point(),
                tw.max(th),
                shown.info.width.max(shown.info.height),
            );
            let ready = self
                .derive
                .as_ref()
                .and_then(|d| Some((d, d.ready_for(shown)?)));
            let whole = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
            match ready {
                _ if graph => {
                    if let Some(s) = &shown.info.spectrum {
                        crate::spectrum_view::paint(
                            &ui.painter_at(body),
                            body,
                            s,
                            lang,
                            &visuals,
                            pointer.hover,
                        );
                    }
                }
                Some((d, r)) if d.view == View::Compare => {
                    let split_x = body.left() + body.width() * d.split;
                    let left = body.with_max_x(split_x);
                    let right = body.with_min_x(split_x);
                    ui.painter_at(left).image(
                        shown.texture().id(),
                        img_rect,
                        whole,
                        Color32::WHITE,
                    );
                    ui.painter_at(right)
                        .image(r.derived.id(), img_rect, whole, Color32::WHITE);
                }
                Some((d, r)) => {
                    let texture = match d.view {
                        View::Difference => &r.difference,
                        _ => &r.derived,
                    };
                    ui.painter_at(body)
                        .image(texture.id(), img_rect, whole, Color32::WHITE);
                }
                None => {
                    let painter = ui.painter_at(body);
                    let deep = self
                        .deep
                        .as_ref()
                        .is_some_and(|d| d.paint(&painter, img_rect, whole, texture_options));
                    if !deep {
                        painter.image(shown.texture().id(), img_rect, whole, Color32::WHITE);
                    }
                }
            }
            // 色導出は元の画像から描くので、縮小画像で足りる大きさでも元の画像を持たせる。
            out.wants_full |= self.derive.is_some();

            let hits = Hits {
                mark: pointer.pos.and_then(|p| {
                    self.marks.iter().position(|m| {
                        m.path == shown.path && placed.mark_pos(m).distance(p) <= MARK_HIT
                    })
                }),
                roi: pointer.pos.and_then(|p| {
                    self.rois.iter().position(|r| {
                        r.path == shown.path && placed.region_rect(r.region).expand(2.0).contains(p)
                    })
                }),
                pixel: pointer.pos.and_then(|p| placed.pixel_at(p)),
            };
            let mut drawing = None;
            if drag == Drag::DrawRoi {
                if pointer.drag_started {
                    let kind = roi_kind(pointer.shift);
                    self.drawing = pointer.pos.map(|p| (kind, placed.pixel_clamped(p)));
                }
                if let (Some((kind, start)), Some(p)) = (self.drawing, pointer.pos) {
                    let region = Region::from_corners(start, placed.pixel_clamped(p));
                    if pointer.dragged {
                        drawing = Some((kind, region));
                    }
                    if pointer.drag_stopped {
                        self.drawing = None;
                        // 同じ画像の同じ種類はひとつだけなので、前のものと入れ替える。
                        let path = shown.path.clone();
                        self.rois.retain(|r| !(r.path == path && r.kind == kind));
                        self.rois.push(Roi { path, kind, region });
                        self.selected_roi = Some(self.rois.len() - 1);
                    }
                }
            } else {
                self.drawing = None;
            }
            if pointer.clicked {
                match tool.click(hits) {
                    Click::Nothing => {}
                    Click::ClearSelection => {
                        self.selected_mark = None;
                        self.selected_roi = None;
                    }
                    Click::SelectMark(i) => self.selected_mark = Some(i),
                    Click::AddMark(x, y) => {
                        self.marks.push(Mark {
                            path: shown.path.clone(),
                            x,
                            y,
                        });
                        self.selected_mark = Some(self.marks.len() - 1);
                    }
                    Click::SelectRoi(i) => self.selected_roi = i,
                }
            }
            if pointer.secondary_clicked {
                if let Some(i) = hits.mark {
                    self.selected_mark = Some(i);
                }
                if let Some(i) = hits.roi {
                    self.selected_roi = Some(i);
                }
                // 重なった枠は小さいほうを選ぶ。大きな顔の中に小さな顔が写り込むことがあるため。
                self.selected_face = pointer
                    .pos
                    .and_then(|p| {
                        faces
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| placed.fraction_rect(m.rect).contains(p))
                            .min_by(|a, b| {
                                (a.1.rect[2] * a.1.rect[3]).total_cmp(&(b.1.rect[2] * b.1.rect[3]))
                            })
                            .map(|(i, _)| i)
                    })
                    .map(|i| (shown.path.clone(), i));
            }
            let over_pixel = pointer.hover.is_some_and(|p| placed.pixel_at(p).is_some());
            if (pointer.hovered || pointer.dragged)
                && let Some(cursor) = tool.cursor(pointer.alt, over_pixel, pointer.dragged)
            {
                ui.ctx().set_cursor_icon(cursor);
            }
            draw_faces(ui, body, lang, faces, &placed, &self.colors);
            if beam {
                // 帯を重ねている間は、その上に出す。
                let clear = if floating {
                    Rect::from_min_max(body.min, pos2(body.max.x, body.max.y - DERIVE_BAR))
                } else {
                    body
                };
                draw_beam(ui, lang, clear, img_rect, scale, &shown.info.metadata);
            }
            let has_marks = self.marks.iter().any(|m| m.path == shown.path);
            let has_rois = self.rois.iter().any(|r| r.path == shown.path);
            // 値は元の画像から読むので、縮小画像で足りる大きさでも元の画像を持たせる。
            out.wants_full |= has_marks || has_rois || (tool == Tool::Probe && pointer.hovered);
            self.draw_rois(ui, body, lang, &placed, drawing);
            self.draw_marks(ui, body, lang, &placed);
            if tool == Tool::Probe {
                let hover = pointer.hover.and_then(|p| placed.pixel_at(p));
                if let Some((x, y)) = hover {
                    let value = self.mark_value(&Mark {
                        path: shown.path.clone(),
                        x,
                        y,
                    });
                    let text = match value {
                        Some(v) => format!("x {x}, y {y}   {}", crate::i18n::tr_text(lang, &v)),
                        None => format!("x {x}, y {y}"),
                    };
                    let galley = painter.layout_no_wrap(
                        text,
                        egui::FontId::proportional(13.0),
                        Color32::from_gray(230),
                    );
                    let pad = vec2(6.0, 2.0);
                    let pill = Rect::from_min_size(
                        body.left_bottom() + vec2(6.0, -6.0 - galley.size().y - 2.0 * pad.y),
                        galley.size() + 2.0 * pad,
                    );
                    painter.rect_filled(pill, 3.0, Color32::from_black_alpha(150));
                    painter.galley(pill.min + pad, galley, Color32::from_gray(230));
                }
            }
        } else if let (Content::Folder(folder), false) = (&self.content, self.loading) {
            let zip = crate::archive::is_archive(folder);
            let icon = if zip {
                egui_phosphor::regular::FILE_ZIP
            } else {
                egui_phosphor::regular::FOLDER
            };
            let size = (body.height() * 0.4).clamp(16.0, 96.0);
            painter.text(
                body.center(),
                egui::Align2::CENTER_CENTER,
                icon,
                egui::FontId::proportional(size),
                visuals.weak_text_color(),
            );
            // ZIP はダブルクリックで展開してその中へ移るので、中の画像を出す案内は出さない。
            if !zip && body.height() > 120.0 {
                painter.text(
                    body.center_bottom() - vec2(0.0, 8.0),
                    egui::Align2::CENTER_BOTTOM,
                    tr(lang, "Double-click to show the images inside"),
                    egui::FontId::proportional(12.0),
                    Color32::GRAY,
                );
            }
        } else {
            let msg = match (&self.content, self.loading) {
                (_, true) => tr(lang, "Loading...").to_owned(),
                // 理由もスロットに出す。ログ欄には失敗の数だけを出し、理由はログファイルにしか書かないため。
                (Content::Failed(e), false) => format!("{}\n{e}", tr(lang, "Failed to load")),
                _ => tr(lang, empty_hint).to_owned(),
            };
            let mut job = egui::text::LayoutJob::simple(
                msg,
                egui::FontId::proportional(14.0),
                Color32::GRAY,
                (body.width() - 16.0).max(1.0),
            );
            job.halign = egui::Align::Center;
            let galley = painter.fonts_mut(|f| f.layout_job(job));
            // 中央揃えの文は、置く位置が横の中心になる。
            let top = body.center().y - galley.size().y / 2.0;
            painter.galley(pos2(body.center().x, top), galley, Color32::GRAY);
        }

        let applied_look = match &self.content {
            Content::Image(shown) => shown.info.look.as_ref(),
            _ => None,
        };
        if let (Some(g), Some(applied)) = (&gauge, applied_look)
            && (applied.palette.is_some() || g.on_gray)
            && crate::colormap_ui::fits(body, g.side)
        {
            let inverted = self.shown().is_some_and(|s| {
                self.derive
                    .as_ref()
                    .is_some_and(|d| d.ready_for(s).is_some() && d.inverted(&s.path))
            });
            // 色導出で明るさを反転しているときは、ゲージの色もそれに合わせて逆に並べる。
            let flipped;
            let applied = if inverted {
                flipped = crate::colormap::flipped(applied);
                &flipped
            } else {
                applied
            };
            let id = ui.id().with(("gauge", self.id));
            let event = crate::colormap_ui::gauge(
                ui,
                id,
                body,
                g.side,
                lang,
                applied,
                color_rules,
                &mut self.gauge_drag,
            );
            match event {
                Some(crate::colormap_ui::GaugeEvent::Window(w)) => out.push(SlotCommand::Window(w)),
                Some(crate::colormap_ui::GaugeEvent::Pick(p)) => {
                    out.push(SlotCommand::ColorPick(p))
                }
                None => {}
            }
        }
        if bar_height > 0.0 {
            self.derive_overlay(ui, body, lang);
            self.derive_bar(ui, bar, lang, presets, &mut out);
        } else if floating {
            let bar = Rect::from_min_max(pos2(body.min.x, body.max.y - DERIVE_BAR), body.max);
            painter.rect_filled(bar, 0.0, visuals.faint_bg_color);
            self.derive_bar(ui, bar, lang, presets, &mut out);
        }
        // ヘッダ: ファイル名、表示倍率、閉じるボタン。
        // ヘッダ全体のクリック判定を閉じるボタンより先に登録する。egui では後から登録した
        // 判定が上に来るので、逆にすると閉じるボタンが押せなくなる。
        let name = self
            .path
            .as_ref()
            .or(match (&self.content, self.loading) {
                (Content::Folder(folder), false) => Some(folder),
                _ => None,
            })
            .and_then(|p| {
                crate::compose::title(p)
                    .or_else(|| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            })
            .unwrap_or_else(|| tr(lang, "(empty)").into());
        let name = match &self.sequence {
            Some(seq) if seq.kind == SequenceKind::Frames => seq
                .folder
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            Some(Sequence {
                title: Some(title), ..
            }) => format!("{title}/{name}"),
            Some(seq) => {
                let folder = seq
                    .folder
                    .file_name()
                    .map_or_else(|| seq.folder.to_string_lossy(), |n| n.to_string_lossy());
                format!("{folder}/{name}")
            }
            None => name,
        };
        if let Some(mut text) = self.sequence.as_ref().and_then(page_text) {
            // キューブの面なら、その面の周波数・速度・偏光も並べる。
            if let Some(plane) = self
                .shown()
                .and_then(|s| s.info.metadata.get(vw_fits::GROUP, vw_fits::PLANE))
            {
                text = format!("{text}  {plane}");
            }
            // ヘッダの名前の後ろに出すと、長い名前やフラグ・タグに隠れて読めなくなるので、画像の上に重ねる。
            let galley = painter.layout_no_wrap(
                text,
                egui::FontId::proportional(13.0),
                Color32::from_gray(230),
            );
            let pad = vec2(6.0, 2.0);
            let pill = Rect::from_min_size(
                pos2(
                    body.right() - 6.0 - galley.size().x - 2.0 * pad.x,
                    body.top() + 6.0,
                ),
                galley.size() + 2.0 * pad,
            );
            painter.rect_filled(pill, 3.0, Color32::from_black_alpha(150));
            painter.galley(pill.min + pad, galley, Color32::from_gray(230));
        }
        let label = name;
        let text_color = if active {
            visuals.strong_text_color()
        } else {
            visuals.weak_text_color()
        };
        // 画像の移動はヘッダのドラッグでも始められる。手のひらのツールでは画像の上のドラッグがパンになるため。
        let header_response = ui.interact(
            header,
            ui.id().with(("header", self.id)),
            Sense::click_and_drag(),
        );
        if header_response.clicked() {
            out.push(SlotCommand::Activate);
        }
        if let Some(shown) = self.shown().filter(|_| self.folder().is_none()) {
            header_response.dnd_set_drag_payload(DraggedFile(crate::frames::file_of(&shown.path)));
            out.dragging |= header_response.dragged();
        }
        // ボタンはマウスを乗せたスロットにだけ出す。並んだスロットのすべてに出すと、
        // 画像のヘッダがボタンで埋まって名前が読みにくくなるため。
        let close_rect = Rect::from_center_size(
            header.right_center() - vec2(HEADER / 2.0, 0.0),
            Vec2::splat(HEADER - 4.0),
        );
        let mut labels_right = header.right() - 4.0;
        if hovering {
            if header_button(ui, &painter, close_rect, ("close", self.id), "×", 14.0)
                .on_hover_text(tr(lang, "Close slot"))
                .clicked()
            {
                out.push(SlotCommand::Close);
            }
            labels_right = close_rect.left();
        }
        let next_rect = |right: f32| close_rect.translate(vec2(right - close_rect.right(), 0.0));
        if self.folder().is_some() && hovering {
            let open_rect = next_rect(labels_right);
            if header_button(
                ui,
                &painter,
                open_rect,
                ("open", self.id),
                egui_phosphor::regular::ARROW_SQUARE_IN,
                14.0,
            )
            .on_hover_text(tr(lang, "Open this folder"))
            .clicked()
            {
                out.push(SlotCommand::OpenFolder);
            }
            labels_right = open_rect.left();
        }

        if self.shown().is_some_and(Shown::developing) {
            let film_rect = next_rect(labels_right);
            let film = ui.interact(film_rect, ui.id().with(("film", self.id)), Sense::hover());
            painter.text(
                film_rect.center(),
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::FILM_STRIP,
                egui::FontId::proportional(14.0),
                visuals.weak_text_color(),
            );
            film.on_hover_text(tr(lang, "Developing RAW..."));
            labels_right = film_rect.left();
        }

        if let Some(preview) = self
            .shown()
            .filter(|s| s.other.is_some())
            .map(Shown::showing_raw_preview)
        {
            let badge_rect = Rect::from_min_max(
                pos2(labels_right - 36.0, close_rect.top()),
                pos2(labels_right, close_rect.bottom()),
            );
            let badge = ui.interact(
                badge_rect,
                ui.id().with(("raw_face", self.id)),
                Sense::click(),
            );
            let color = if badge.hovered() {
                visuals.strong_text_color()
            } else {
                visuals.weak_text_color()
            };
            painter.text(
                badge_rect.center(),
                egui::Align2::CENTER_CENTER,
                if preview { "JPEG" } else { "RAW" },
                egui::FontId::proportional(11.0),
                color,
            );
            let hint = if preview {
                "Showing the preview embedded by the camera. Click to show the developed image."
            } else {
                "Showing the image developed from the sensor data. Click to show the camera's embedded preview."
            };
            if badge.on_hover_text(tr(lang, hint)).clicked() {
                out.push(SlotCommand::SwapRawFace);
            }
            labels_right = badge_rect.left();
        }

        let calibration = self.shown().and_then(|s| {
            s.info
                .metadata
                .get(vw_fits::calibration::GROUP, vw_fits::calibration::STATE)
        });
        if let Some(state) = calibration.filter(|&s| s != vw_fits::calibration::NOT_APPLIED) {
            let applied = state == vw_fits::calibration::APPLIED;
            let mark_rect = next_rect(labels_right);
            let mark = ui.interact(
                mark_rect,
                ui.id().with(("calibrated", self.id)),
                Sense::hover(),
            );
            painter.text(
                mark_rect.center(),
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::APERTURE,
                egui::FontId::proportional(14.0),
                if applied {
                    visuals.selection.stroke.color
                } else {
                    visuals.weak_text_color()
                },
            );
            mark.on_hover_text(tr(
                lang,
                if applied {
                    "Dark and flat applied"
                } else {
                    "Calibration off for this image"
                },
            ));
            labels_right = mark_rect.left();
        }

        if let Some(mark) = similar.filter(|_| self.shown().is_some()) {
            let keeper = if mark.rank == Some(1) {
                egui_phosphor::regular::STAR
            } else {
                ""
            };
            let text = format!(
                "{}{}{keeper}",
                egui_phosphor::regular::LINK_SIMPLE,
                mark.number
            );
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), mark.color);
            let mark_rect = Rect::from_min_max(
                pos2(labels_right - galley.size().x - 8.0, header.min.y + 2.0),
                pos2(labels_right, header.max.y - 2.0),
            );
            let response = ui.interact(
                mark_rect,
                ui.id().with(("similar", self.id)),
                Sense::click(),
            );
            if response.hovered() {
                painter.rect_filled(mark_rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
            }
            painter.galley(mark_rect.center() - galley.size() / 2.0, galley, mark.color);
            let mut hint = tr1(
                lang,
                "Similar to {}. Click to align and compare.",
                &mark.others,
            );
            if let Some(rank) = mark.rank {
                hint.push('\n');
                hint.push_str(
                    &tr(
                        lang,
                        "Pick {rank} of {of} by sharpness, then size (the star marks the one to keep)",
                    )
                    .replace("{rank}", &rank.to_string())
                    .replace("{of}", &mark.of.to_string()),
                );
            }
            let response = response.on_hover_text(hint);
            if response.clicked() {
                out.push(SlotCommand::CompareSimilar);
                out.push(SlotCommand::Activate);
            }
            response.context_menu(|ui| {
                if ui.button(tr(lang, "Compare the group")).clicked() {
                    out.push(SlotCommand::CompareSimilar);
                    out.push(SlotCommand::Activate);
                    ui.close();
                }
                if ui
                    .button(tr(lang, "Keep this one, flag the rest bad"))
                    .clicked()
                {
                    out.push(SlotCommand::KeepInGroup);
                    out.push(SlotCommand::Activate);
                    ui.close();
                }
            });
            labels_right = mark_rect.left();
        }

        if let Some(d) = &self.derive {
            let mark_rect = next_rect(labels_right);
            let mark = ui.interact(
                mark_rect,
                ui.id().with(("derived", self.id)),
                Sense::hover(),
            );
            painter.text(
                mark_rect.center(),
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::SWATCHES,
                egui::FontId::proportional(14.0),
                visuals.selection.stroke.color,
            );
            mark.on_hover_text(format!("{}: {}", tr(lang, "Derived"), derive_name(d, lang)));
            labels_right = mark_rect.left();
        }

        // 倍率はファイル名の後ろではなく右側にボタンと並べる。名前の長さで位置が動くと
        // ボタンを探しにくく、長い名前ではボタンが名前に重なるため。
        if self.shown().is_some() {
            if hovering {
                let actual_rect = next_rect(labels_right);
                if header_button(ui, &painter, actual_rect, ("actual", self.id), "1:1", 12.0)
                    .on_hover_text(tr(lang, "Actual size (100%)"))
                    .clicked()
                {
                    self.actual_size();
                    out.push(SlotCommand::Activate);
                }
                let fit_rect = actual_rect.translate(vec2(-(HEADER - 2.0), 0.0));
                if header_button(
                    ui,
                    &painter,
                    fit_rect,
                    ("fit", self.id),
                    egui_phosphor::regular::FRAME_CORNERS,
                    12.0,
                )
                .on_hover_text(tr(lang, "Fit image to slot"))
                .clicked()
                {
                    self.reset_view();
                    out.push(SlotCommand::Activate);
                }
                labels_right = fit_rect.left();
            }
            let zoom = painter.text(
                pos2(labels_right - 2.0, header.center().y),
                egui::Align2::RIGHT_CENTER,
                format!("{:.0}%", base * self.zoom * 100.0),
                egui::FontId::proportional(12.0),
                visuals.weak_text_color(),
            );
            labels_right = zoom.left();
        }

        // フラグとタグはヘッダの右端に出し、フラグはヘッダの下端の色の線でも示す。
        // 縮小したマス目でも、どの画像にフラグが付いているか一目で分かるように。
        if !image_label.is_empty() {
            let job = labels::labeled_name(
                "",
                image_label,
                text_color,
                visuals.weak_text_color(),
                egui::FontId::proportional(13.0),
            );
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            let pos = pos2(
                labels_right - 6.0 - galley.size().x,
                header.center().y - galley.size().y / 2.0,
            );
            labels_right = pos.x;
            painter.galley(pos, galley, text_color);
        }
        // 名前は右側の倍率やボタンを置いてから、残りの幅に収まるよう末尾を省いて描く。
        // 先に描くと、狭いスロットで長い名前と倍率の文字が重なって読めなくなるため。
        let name_left = header.left() + 6.0;
        let mut job = egui::text::LayoutJob::simple_singleline(
            label,
            egui::FontId::proportional(13.0),
            text_color,
        );
        job.wrap =
            egui::text::TextWrapping::truncate_at_width((labels_right - 6.0 - name_left).max(0.0));
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(
            pos2(name_left, header.center().y - galley.size().y / 2.0),
            galley,
            text_color,
        );
        if let Some(flag) = image_label.flag {
            let strip = Rect::from_min_max(pos2(header.min.x, header.max.y - 2.0), header.max);
            painter.rect_filled(strip, 0.0, crate::theme::flag_color(flag));
        }

        // 枠は選んでいるスロットだけに描く。スロットの間は並べる側が 1px 空けて区切り線の色で
        // 見せるので、全部に枠を描くと区切りが 3px の太さになってしまうため。
        if active {
            painter.rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.0, visuals.selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        out.body = Some(response);
        out
    }
}

/// 顔の枠を描く。人物が分かった顔と分からない顔で色を分け（設定の色）、名札を添える。
fn draw_faces(
    ui: &egui::Ui,
    body: Rect,
    lang: Language,
    faces: &[FaceMark],
    placed: &Placed,
    colors: &Colors,
) {
    let painter = ui.painter_at(body);
    for (i, m) in faces.iter().enumerate() {
        let rect = placed.fraction_rect(m.rect);
        let color = if m.person.is_some() {
            colors.face_known
        } else {
            colors.face_unknown
        };
        crate::marks::rect_outline(&painter, rect, color, 1.5, false);
        let text = match &m.person {
            Some(name) => name.clone(),
            None => format!("{} {}", tr(lang, "Face"), i + 1),
        };
        crate::marks::name_tag(&painter, rect, body, &text, color);
    }
}

/// 色の見本の四角を置くために、文字の前に空ける幅。
fn swatch_room(swatch: Option<[u8; 3]>, line: f32) -> Vec2 {
    match swatch {
        Some(_) => vec2(line + 4.0, 0.0),
        None => Vec2::ZERO,
    }
}

/// `at` に文字の高さの色の見本を描き、文字を置く位置を返す。見本がなければ `at` のまま。
fn draw_swatch(painter: &egui::Painter, at: Pos2, line: f32, swatch: Option<[u8; 3]>) -> Pos2 {
    let Some([r, g, b]) = swatch else {
        return at;
    };
    let side = line - 2.0;
    let rect = Rect::from_min_size(at + vec2(0.0, 1.0), vec2(side, side));
    painter.rect_filled(rect, 1.0, Color32::from_rgb(r, g, b));
    painter.rect_stroke(
        rect,
        1.0,
        egui::Stroke::new(1.0, Color32::from_gray(200)),
        egui::StrokeKind::Inside,
    );
    at + vec2(line + 4.0, 0.0)
}

/// テストで使う、表示中の画像の作り方。
#[cfg(test)]
pub mod test_support {
    use super::*;

    fn info(width: u32, height: u32, color: ColorModel) -> ImageInfo {
        ImageInfo {
            width,
            height,
            color,
            bits_per_sample: 8,
            significant_bits: 8,
            metadata: Default::default(),
            develop_size: None,
            plugin: "PNG".into(),
            frames: 1,
            file_size: 0,
            read: Default::default(),
            decode: Default::default(),
            convert: Default::default(),
            look: None,
            compose: None,
            fingerprint: None,
            sharpness: None,
            gamut: None,
            frame_delays: None,
            spectrum: None,
        }
    }

    fn thumb() -> TextureHandle {
        let ctx = egui::Context::default();
        ctx.load_texture("t", ColorImage::example(), Default::default())
    }

    /// 埋め込みのプレビューを出している、現像できる RAW。
    pub fn raw_preview() -> Shown {
        let mut info = info(160, 120, ColorModel::Rgb);
        info.develop_size = Some((6000, 4000));
        info.plugin = "RAW".into();
        Shown {
            path: PathBuf::from("/a.ARW"),
            thumb: thumb(),
            info,
            full: None,
            developed: false,
            other: None,
        }
    }

    /// 普通の灰色の画像。
    pub fn gray_image() -> Shown {
        Shown {
            path: PathBuf::from("/g.png"),
            thumb: thumb(),
            info: info(160, 120, ColorModel::Gray),
            full: None,
            developed: false,
            other: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::raw_preview;

    #[test]
    fn a_failed_develop_stops_the_developing_mark_for_good() {
        let mut shown = raw_preview();
        shown.developed = true;
        assert!(shown.developing());
        assert!(shown.give_up_developing());
        assert!(!shown.developing());
        // もう現像を頼まない（develop_slot は develop_size のない画像を飛ばす）。
        assert_eq!(shown.info.develop_size, None);
        assert!(!shown.developed);
    }

    #[test]
    fn a_failed_preview_load_is_not_a_failed_develop() {
        let mut shown = raw_preview();
        assert!(!shown.give_up_developing());
        assert_eq!(shown.info.develop_size, Some((6000, 4000)));
    }

    /// 現像し終えて、埋め込みのプレビューを残した RAW。
    fn developed_with_preview() -> Shown {
        let preview = raw_preview();
        let mut shown = raw_preview();
        shown.developed = true;
        shown.info.develop_size = None;
        shown.info.width = 6000;
        shown.other = Some(Box::new(RawFace {
            thumb: preview.thumb,
            info: preview.info,
            full: None,
            developed: false,
        }));
        shown
    }

    #[test]
    fn swapping_shows_the_embedded_preview_and_back() {
        let mut shown = developed_with_preview();
        assert!(!shown.showing_raw_preview());
        assert!(shown.swap_raw_face());
        assert!(shown.showing_raw_preview());
        assert_eq!(shown.info.width, 160);
        assert_eq!(shown.info.develop_size, Some((6000, 4000)));
        assert!(shown.swap_raw_face());
        assert!(!shown.showing_raw_preview());
        assert_eq!(shown.info.width, 6000);
    }

    #[test]
    fn the_preview_shown_is_not_mistaken_for_a_develop_in_progress() {
        let mut shown = developed_with_preview();
        shown.swap_raw_face();
        assert!(!shown.developing());
    }

    #[test]
    fn a_raw_not_yet_developed_has_nothing_to_swap() {
        let mut shown = raw_preview();
        assert!(!shown.swap_raw_face());
        assert!(!shown.showing_raw_preview());
    }

    #[test]
    fn nearest_minify_uses_no_mipmaps_so_only_original_values_are_shown() {
        let o = texture_options(ScaleFilter::Nearest, ScaleFilter::Nearest);
        assert_eq!(o.magnification, TextureFilter::Nearest);
        assert_eq!(o.minification, TextureFilter::Nearest);
        assert_eq!(o.mipmap_mode, None);
    }

    #[test]
    fn linear_minify_uses_linear_mipmaps() {
        let o = texture_options(ScaleFilter::Linear, ScaleFilter::Linear);
        assert_eq!(o.magnification, TextureFilter::Linear);
        assert_eq!(o.minification, TextureFilter::Linear);
        assert_eq!(o.mipmap_mode, Some(TextureFilter::Linear));
    }

    #[test]
    fn folder_slot_stays_closed_until_its_images_are_loaded() {
        let mut slot = Slot::new(1);
        slot.set_subfolder(PathBuf::from("/photos/2026"));
        assert!(slot.is_closed_folder());
        slot.sequence = Some(Sequence::subtree(PathBuf::from("/photos/2026")));
        slot.loading = true;
        assert!(!slot.is_closed_folder());
    }

    #[test]
    fn page_of_a_sequence_counts_from_one() {
        let mut seq = Sequence::frames(PathBuf::from("/a.webp"), 12, None);
        seq.index = 2;
        assert_eq!(page_text(&seq).as_deref(), Some("3 / 12"));
    }

    #[test]
    fn single_image_sequence_shows_no_page() {
        let seq = Sequence::new(PathBuf::from("/photos"), PathBuf::from("/photos/a.png"));
        assert_eq!(page_text(&seq), None);
    }

    #[test]
    fn new_slot_is_empty_and_fitted() {
        let slot = Slot::new(7);
        assert_eq!(slot.id, 7);
        assert!(slot.path.is_none() && slot.shown().is_none() && !slot.loading);
        assert_eq!((slot.zoom, slot.pan), (1.0, Vec2::ZERO));
    }
}
