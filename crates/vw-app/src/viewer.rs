//! 画像フィールドの 1 枠（スロット）。ヘッダ、ズーム、パン（移動）を受け持つ。
//!
//! ズームとパンの計算は egui に依存しない関数（[`fit_scale`], [`zoom_at`]）に分けてあり、
//! テストではそちらを確かめる。

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, pos2, vec2, Color32, MouseWheelUnit, PointerButton, Pos2, Rect, Sense, TextureHandle,
    Vec2,
};
use vw_core::VwImage;

use crate::i18n::tr;
use crate::labels::{self, Label};
use crate::loader::ImageInfo;
use crate::settings::Language;

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

    /// 描画に使うテクスチャ。フル解像度があればそれ、なければ縮小画像。
    pub fn texture(&self) -> &TextureHandle {
        self.full.as_ref().map_or(&self.thumb, |f| &f.texture)
    }
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
    /// フォルダのスロットなら、そのフォルダ（または ZIP）。cd したフォルダの中のサブフォルダを
    /// エクスプローラのように並べるためのもので、中身を開くまではアイコンと名前だけを出す。
    pub folder: Option<PathBuf>,
    /// フォルダスロットなら、そのフォルダの画像の並び。普通のスロットは `None` で、
    /// 前後の画像はその都度同じフォルダから探す。
    pub sequence: Option<Sequence>,
    /// フォルダの読み込みで 1 枚 1 スロットに並べたスロット。フォルダの画像が増減したら、
    /// 中身を差し替えずにスロットを足したり閉じたりする。
    pub from_folder: bool,
    /// 最後に出した読み込み要求の番号。これと違う番号の結果は古いので捨てる。
    /// 前後の画像へ素早く送ったとき、遅れて届いた前の画像で上書きされるのを防ぐ。
    pub request: u64,
    pub loading: bool,
    pub shown: Option<Shown>,
    pub error: Option<String>,
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
    /// 直前に描いた画像部分の大きさ。キー操作で倍率を変えるときに使う。
    last_body: Option<Vec2>,
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
}

/// スロットのヘッダからドラッグしている画像のファイル（egui のドラッグ＆ドロップの中身）。
pub struct DraggedFile(pub PathBuf);

/// 1 フレーム分の操作結果。
#[derive(Default)]
pub struct SlotResponse {
    pub clicked: bool,
    pub close: bool,
    /// 右クリックメニューで「コピー」が選ばれた。
    pub copy: bool,
    /// 縮小画像では足りない大きさで表示している。
    pub wants_full: bool,
    /// 右クリックメニューで「現像」が選ばれた。
    pub develop: bool,
    /// ダブルクリックされた（最大化の切り替え）。
    pub toggle_maximize: bool,
    /// ホイールで送る枚数。負なら前へ。
    pub step: isize,
    /// フォルダのスロットの「このフォルダへ移動」が押された。
    pub open_folder: bool,
    /// フォルダのスロットに画像がドロップされた。
    pub dropped: Option<PathBuf>,
}

/// ヘッダ（ファイル名とズーム率の帯）の高さ。
const HEADER: f32 = 20.0;
/// ズーム倍率の下限（全体表示に対して）。
const MIN_ZOOM: f32 = 0.05;
/// 実寸に対する拡大率の上限。画素を 1 つずつ確認できれば十分なので 200 倍で止める。
const MAX_PIXEL_SCALE: f32 = 200.0;
/// 拡大縮小のドラッグ 1 論理ピクセル分のズーム量（指数の係数）。100 px で約 2.7 倍。
const DRAG_ZOOM_RATE: f32 = 0.01;
/// ホイールで 1 枚送るのに要る回転量（論理ピクセル）。egui がホイール 1 目盛りを
/// 40 論理ピクセルに換算するので、1 目盛りで 1 枚になる。
pub const WHEEL_STEP: f32 = 40.0;
/// キーで拡大縮小するときの 1 回分の倍率。
pub const KEY_ZOOM_FACTOR: f32 = 1.25;
/// パンに使うマウスボタン。
const PAN_BUTTON: PointerButton = PointerButton::Primary;
/// 上下のドラッグで拡大縮小するマウスボタン。
const ZOOM_BUTTON: PointerButton = PointerButton::Secondary;

/// ホイールの回転量 `delta`（論理ピクセル、上に回すと正）を `acc` に足し、
/// 1 段分たまるごとに送る枚数を返す。上に回すと前（負）、下に回すと次（正）。
///
/// タッチパッドは細かい量を何度も送ってくるので、1 回のイベントを 1 枚にはしない。
/// 回す向きが変わったら、それまでの端数は捨てる。逆向きに回し始めたのに、
/// 前の向きの端数を打ち消すまで反応しないのを避けるため。
/// ヘッダの右側に並べる小さなボタン。マウスが乗ると濃くする。
fn header_button(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    icon: &str,
) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(id), Sense::click());
    let visuals = ui.visuals();
    let color = if response.hovered() {
        visuals.strong_text_color()
    } else {
        visuals.weak_text_color()
    };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(12.0),
        color,
    );
    response
}

/// 複数枚の並びの「何枚目 / 全部で何枚」。1 枚しかなければ出さない。
pub fn page_text(seq: &Sequence) -> Option<String> {
    (seq.paths.len() > 1).then(|| format!("{} / {}", seq.index + 1, seq.paths.len()))
}

pub fn wheel_steps(acc: &mut f32, delta: f32) -> isize {
    if delta == 0.0 {
        return 0;
    }
    if acc.signum() != delta.signum() {
        *acc = 0.0;
    }
    *acc += delta;
    let steps = (*acc / WHEEL_STEP).trunc();
    *acc -= steps * WHEEL_STEP;
    -(steps as isize)
}

/// 画像を枠いっぱいに収める倍率。枠より小さい画像も拡大する。
///
/// 小さな画像を 100% のまま止めないのは、1x1 や最大化でスロットを広げても画像が
/// 広がらず、「枠に合わせる」が効いていないように見えるため。実寸で見たいときは
/// 100% 表示の操作がある。
pub fn fit_scale(view: Vec2, image: Vec2) -> f32 {
    if image.x <= 0.0 || image.y <= 0.0 {
        return 1.0;
    }
    (view.x / image.x).min(view.y / image.y)
}

/// 倍率を `factor` 倍し、`cursor`（画像中心からのずれではなく枠中心からの位置）の下にある
/// 点が動かないようにパンを補正した `(zoom, pan)` を返す。
pub fn zoom_at(zoom: f32, pan: Vec2, factor: f32, base: f32, cursor: Option<Vec2>) -> (f32, Vec2) {
    let new_zoom = (zoom * factor).clamp(MIN_ZOOM, MAX_PIXEL_SCALE / base.max(1e-6));
    let pan = match cursor {
        Some(c) => pan + (c - pan) * (1.0 - new_zoom / zoom),
        None => pan,
    };
    (new_zoom, pan)
}

/// 長辺 `shown_side`（物理ピクセル）で表示するのに、長辺 `thumb_side` の縮小画像では
/// 画素が足りないか。元画像（長辺 `image_side`）が縮小されていない小さな画像なら、
/// 縮小画像がそのまま元の画素なので要らない。
pub fn needs_full(shown_side: f32, thumb_side: usize, image_side: u32) -> bool {
    image_side as usize > thumb_side && shown_side > thumb_side as f32
}

/// このフレームのホイールの縦の回転量（論理ピクセル、上に回すと正）。
///
/// egui の `smooth_scroll_delta` を使わないのは、回転量を数フレームに分けて届けるので、
/// 1 目盛りで 1 枚送る判定がフレームの区切りでぶれるため。Ctrl 付きは egui が
/// ズームとして扱うので数えない。
fn wheel_delta(events: &[egui::Event], page_height: f32) -> f32 {
    events
        .iter()
        .filter_map(|e| match e {
            egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } if !modifiers.command => Some(match unit {
                MouseWheelUnit::Point => delta.y,
                MouseWheelUnit::Line => delta.y * WHEEL_STEP,
                MouseWheelUnit::Page => delta.y * page_height,
            }),
            _ => None,
        })
        .sum()
}

impl Slot {
    /// 中身をまだ開いていないフォルダのスロット。
    pub fn is_closed_folder(&self) -> bool {
        self.folder.is_some() && self.sequence.is_none() && self.path.is_none() && !self.loading
    }

    pub fn new(id: u64) -> Self {
        Self {
            id,
            path: None,
            folder: None,
            sequence: None,
            from_folder: false,
            request: 0,
            loading: false,
            shown: None,
            error: None,
            full_request: None,
            copy_when_full: false,
            wants_full_now: false,
            zoom: 1.0,
            pan: Vec2::ZERO,
            wheel: 0.0,
            last_body: None,
        }
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

    /// 全体表示・中央に戻す。
    pub fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    fn base_scale(&self, body: Rect) -> f32 {
        let Some(s) = &self.shown else { return 1.0 };
        // テクスチャの大きさではなく元画像の大きさを使う。テクスチャは上限に合わせて
        // 縮小されていることがあり、それで計算すると表示倍率が実寸とずれるため。
        fit_scale(body.size(), vec2(s.info.width as f32, s.info.height as f32))
    }

    /// `rect` の範囲にスロットを描き、操作を処理する。
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        active: bool,
        look: SlotLook,
    ) -> SlotResponse {
        let SlotLook {
            background,
            lang,
            empty_hint,
            label: image_label,
        } = look;
        let mut out = SlotResponse::default();
        let visuals = ui.visuals().clone();
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
        let body = Rect::from_min_max(pos2(rect.min.x, rect.min.y + HEADER), rect.max);
        let painter = ui.painter_at(rect);

        painter.rect_filled(body, 0.0, background);
        painter.rect_filled(header, 0.0, visuals.faint_bg_color);

        let response = ui.interact(
            body,
            ui.id().with(("slot", self.id)),
            Sense::click_and_drag(),
        );
        if response.clicked() || response.drag_started() || response.secondary_clicked() {
            out.clicked = true;
        }
        let drop_target = self
            .folder
            .as_ref()
            .filter(|f| !crate::archive::is_zip(f))
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
            out.dropped = response
                .dnd_release_payload::<DraggedFile>()
                .map(|f| f.0.clone());
        }
        if self.shown.is_some() {
            response.context_menu(|ui| {
                if ui.button(tr(lang, "Fit image to slot")).clicked() {
                    self.reset_view();
                    ui.close();
                }
                if ui.button(tr(lang, "Copy")).clicked() {
                    out.copy = true;
                    ui.close();
                }
                let developable = self
                    .shown
                    .as_ref()
                    .is_some_and(|s| s.info.develop_size.is_some() && !s.developed);
                if developable
                    && ui
                        .button(tr(lang, "Develop RAW from sensor data"))
                        .clicked()
                {
                    out.develop = true;
                    ui.close();
                }
            });
        }
        out.toggle_maximize = response.double_clicked();
        if response.hovered() {
            let wheel = ui.input(|i| wheel_delta(&i.events, i.viewport_rect().height()));
            out.step = wheel_steps(&mut self.wheel, wheel);
        }

        self.last_body = Some(body.size());
        let base = self.base_scale(body);
        if let Some(shown) = &self.shown {
            if response.dragged_by(PAN_BUTTON) {
                self.pan += response.drag_delta();
            }
            if response.dragged_by(ZOOM_BUTTON) {
                let factor = (-response.drag_delta().y * DRAG_ZOOM_RATE).exp();
                let anchor = ui.input(|i| i.pointer.press_origin());
                let cursor = anchor.map(|p| p - body.center());
                (self.zoom, self.pan) = zoom_at(self.zoom, self.pan, factor, base, cursor);
            }
            if response.hovered() {
                // タッチパッドのピンチと Ctrl+ホイール（egui がズームとして届ける）。
                let (pinch, pointer) = ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
                if pinch != 1.0 {
                    let cursor = pointer.map(|p| p - body.center());
                    (self.zoom, self.pan) = zoom_at(self.zoom, self.pan, pinch, base, cursor);
                }
            }
            let scale = base * self.zoom;
            let size = vec2(shown.info.width as f32, shown.info.height as f32) * scale;
            let img_rect = Rect::from_center_size(body.center() + self.pan, size);
            let [tw, th] = shown.thumb.size();
            out.wants_full = needs_full(
                size.max_elem() * ui.ctx().pixels_per_point(),
                tw.max(th),
                shown.info.width.max(shown.info.height),
            );
            ui.painter_at(body).image(
                shown.texture().id(),
                img_rect,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else if let Some(folder) = self.folder.as_ref().filter(|_| self.is_closed_folder()) {
            let zip = crate::archive::is_zip(folder);
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
            let msg = if self.loading {
                "Loading..."
            } else if self.error.is_some() {
                "Failed to load (see log)"
            } else {
                empty_hint
            };
            let msg = tr(lang, msg);
            painter.text(
                body.center(),
                egui::Align2::CENTER_CENTER,
                msg,
                egui::FontId::proportional(14.0),
                Color32::GRAY,
            );
        }

        // ヘッダ: ファイル名、表示倍率、閉じるボタン。
        // ヘッダ全体のクリック判定を閉じるボタンより先に登録する。egui では後から登録した
        // 判定が上に来るので、逆にすると閉じるボタンが押せなくなる。
        let name = self
            .path
            .as_ref()
            .or(self.folder.as_ref().filter(|_| self.is_closed_folder()))
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
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
        if let Some(text) = self.sequence.as_ref().and_then(page_text) {
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
        // 画像の移動はヘッダのドラッグで始める。画像の上のドラッグはパンに使っているため。
        let header_response = ui.interact(
            header,
            ui.id().with(("header", self.id)),
            Sense::click_and_drag(),
        );
        if header_response.clicked() {
            out.clicked = true;
        }
        if let Some(shown) = self.shown.as_ref().filter(|_| self.folder.is_none()) {
            header_response.dnd_set_drag_payload(DraggedFile(crate::frames::file_of(&shown.path)));
        }
        let close_rect = Rect::from_center_size(
            header.right_center() - vec2(HEADER / 2.0, 0.0),
            Vec2::splat(HEADER - 4.0),
        );
        let close = ui.interact(close_rect, ui.id().with(("close", self.id)), Sense::click());
        let close_color = if close.hovered() {
            visuals.strong_text_color()
        } else {
            visuals.weak_text_color()
        };
        painter.text(
            close_rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(14.0),
            close_color,
        );
        out.close = close.on_hover_text(tr(lang, "Close slot")).clicked();
        // フォルダへ移る印は、マウスが乗っているスロットにだけ出す。並んだスロットのすべてに
        // 出すと、画像のヘッダが印で埋まって名前が読みにくくなるため。
        let mut labels_right = close_rect.left();
        if self.folder.is_some() && ui.rect_contains_pointer(rect) {
            let open_rect = close_rect.translate(vec2(-(HEADER - 2.0), 0.0));
            let open = ui.interact(open_rect, ui.id().with(("open", self.id)), Sense::click());
            let open_color = if open.hovered() {
                visuals.strong_text_color()
            } else {
                visuals.weak_text_color()
            };
            painter.text(
                open_rect.center(),
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::ARROW_SQUARE_IN,
                egui::FontId::proportional(14.0),
                open_color,
            );
            out.open_folder = open.on_hover_text(tr(lang, "Open this folder")).clicked();
            labels_right = open_rect.left();
        }

        if self.shown.as_ref().is_some_and(Shown::developing) {
            let film_rect = close_rect.translate(vec2(labels_right - close_rect.right(), 0.0));
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

        // 倍率はファイル名の後ろではなく右側にボタンと並べる。名前の長さで位置が動くと
        // ボタンを探しにくく、長い名前ではボタンが名前に重なるため。
        if self.shown.is_some() {
            let zoom = format!("{:.0}%", base * self.zoom * 100.0);
            let actual_rect = close_rect.translate(vec2(labels_right - close_rect.right(), 0.0));
            if header_button(ui, &painter, actual_rect, ("actual", self.id), "1:1")
                .on_hover_text(tr(lang, "Actual size (100%)"))
                .clicked()
            {
                self.actual_size();
                out.clicked = true;
            }
            let fit_rect = actual_rect.translate(vec2(-(HEADER - 2.0), 0.0));
            if header_button(
                ui,
                &painter,
                fit_rect,
                ("fit", self.id),
                egui_phosphor::regular::FRAME_CORNERS,
            )
            .on_hover_text(tr(lang, "Fit image to slot"))
            .clicked()
            {
                self.reset_view();
                out.clicked = true;
            }
            let zoom = painter.text(
                fit_rect.left_center() - vec2(2.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                zoom,
                egui::FontId::proportional(13.0),
                text_color,
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

        let border = if active {
            visuals.selection.stroke.color
        } else {
            visuals.widgets.noninteractive.bg_stroke.color
        };
        painter.rect_stroke(
            rect.shrink(0.5),
            0.0,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_image_fits_the_limiting_side() {
        assert_eq!(fit_scale(vec2(100.0, 100.0), vec2(400.0, 200.0)), 0.25);
        assert_eq!(fit_scale(vec2(100.0, 100.0), vec2(200.0, 400.0)), 0.25);
    }

    #[test]
    fn small_image_is_enlarged_to_fill_the_slot() {
        assert_eq!(fit_scale(vec2(1000.0, 500.0), vec2(10.0, 10.0)), 50.0);
    }

    #[test]
    fn empty_image_does_not_divide_by_zero() {
        assert_eq!(fit_scale(vec2(100.0, 100.0), vec2(0.0, 0.0)), 1.0);
    }

    #[test]
    fn zoom_keeps_point_under_cursor_fixed() {
        let (zoom, pan, base) = (1.0, vec2(10.0, -5.0), 0.5);
        let cursor = vec2(40.0, 30.0);
        // カーソル位置にある画像上の点（画像中心からの距離を倍率で割った値）
        let image_point = (cursor - pan) / (base * zoom);
        let (z2, p2) = zoom_at(zoom, pan, 2.0, base, Some(cursor));
        assert_eq!(z2, 2.0);
        let after = p2 + image_point * (base * z2);
        assert!((after - cursor).length() < 1e-4, "{after:?}");
    }

    #[test]
    fn zoom_without_cursor_keeps_pan() {
        let (z, p) = zoom_at(1.0, vec2(3.0, 4.0), 1.5, 1.0, None);
        assert_eq!((z, p), (1.5, vec2(3.0, 4.0)));
    }

    #[test]
    fn zoom_is_clamped() {
        assert_eq!(zoom_at(1.0, Vec2::ZERO, 1e-6, 1.0, None).0, MIN_ZOOM);
        // 全体表示が 50% なら、実寸 200 倍は全体表示の 400 倍。
        assert_eq!(zoom_at(1.0, Vec2::ZERO, 1e6, 0.5, None).0, 400.0);
    }

    #[test]
    fn full_resolution_is_needed_only_when_shown_larger_than_the_thumbnail() {
        assert!(!needs_full(400.0, 512, 4000));
        assert!(!needs_full(512.0, 512, 4000));
        assert!(needs_full(513.0, 512, 4000));
    }

    #[test]
    fn images_smaller_than_the_thumbnail_never_need_full_resolution() {
        // 元が 300 x 200 なら縮小画像は元の画素そのもの。拡大表示しても読み直さない。
        assert!(!needs_full(3000.0, 300, 300));
    }

    #[test]
    fn image_just_over_the_thumbnail_size_needs_full_resolution() {
        // 513 px は半分の 257 px に縮小されるので、それより大きく出すなら元が要る。
        assert!(needs_full(300.0, 257, 513));
    }

    #[test]
    fn one_wheel_notch_steps_one_image() {
        let mut acc = 0.0;
        assert_eq!(wheel_steps(&mut acc, -WHEEL_STEP), 1);
        assert_eq!(wheel_steps(&mut acc, WHEEL_STEP), -1);
    }

    #[test]
    fn small_touchpad_deltas_add_up_to_one_step() {
        let mut acc = 0.0;
        assert_eq!(wheel_steps(&mut acc, -15.0), 0);
        assert_eq!(wheel_steps(&mut acc, -15.0), 0);
        assert_eq!(wheel_steps(&mut acc, -15.0), 1);
        assert_eq!(wheel_steps(&mut acc, -15.0), 0);
    }

    #[test]
    fn fast_spin_steps_several_images_at_once() {
        let mut acc = 0.0;
        assert_eq!(wheel_steps(&mut acc, -3.0 * WHEEL_STEP), 3);
    }

    #[test]
    fn reversing_the_wheel_drops_the_leftover() {
        let mut acc = 0.0;
        assert_eq!(wheel_steps(&mut acc, -30.0), 0);
        assert_eq!(wheel_steps(&mut acc, WHEEL_STEP), -1);
    }

    #[test]
    fn wheel_with_ctrl_is_not_counted_as_image_steps() {
        let wheel = |ctrl: bool| egui::Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            modifiers: if ctrl {
                egui::Modifiers::COMMAND
            } else {
                egui::Modifiers::NONE
            },
        };
        assert_eq!(wheel_delta(&[wheel(false)], 500.0), -WHEEL_STEP);
        assert_eq!(wheel_delta(&[wheel(true)], 500.0), 0.0);
    }

    #[test]
    fn folder_slot_stays_closed_until_its_images_are_loaded() {
        let mut slot = Slot::new(1);
        slot.folder = Some(PathBuf::from("/photos/2026"));
        assert!(slot.is_closed_folder());
        slot.sequence = Some(Sequence::subtree(PathBuf::from("/photos/2026")));
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
        assert!(slot.path.is_none() && slot.shown.is_none() && !slot.loading);
        assert_eq!((slot.zoom, slot.pan), (1.0, Vec2::ZERO));
    }
}
