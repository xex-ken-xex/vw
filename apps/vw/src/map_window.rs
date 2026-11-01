//! 地図のウインドウ。画像の撮影地点（EXIF の GPS）を地図の上に赤い点で出す。
//!
//! 出す画像は「今開いているフォルダ」か「登録したデータセット全体」から選ぶ。近い点は
//! 数字入りの丸にまとめ、点をクリックすると縮小画像と名前を出し、そこから画像を開ける。
//! メインのウインドウから画像を地図へドラッグすると、その場所を撮影地点として置ける
//! （ラベルに持ち、EXIF より優先する）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use egui_phosphor::regular as icon;
use vw_core::Registry;
use vw_core::exif::Gps;

use crate::geo_map::{self, Cluster, View};
use crate::geonames::{Found as FoundName, Gazetteer};
use crate::i18n::{tr, tr1};
use crate::labels::{LabelStore, Position};
use crate::map_tiles::Tiles;
use crate::settings::{Language, MapSettings};
use crate::sort::SortOrder;

/// 点をまとめる升の大きさ（画素）。
const CLUSTER_CELL: f64 = 44.0;
const PIN: f32 = 6.0;
const PIN_RED: Color32 = Color32::from_rgb(234, 67, 53);

/// 地図に出す画像の範囲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Folder,
    Datasets,
}

/// 撮影地点のある画像 1 枚。
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub path: PathBuf,
    pub gps: Gps,
    /// 撮影日時（EXIF の書き方のまま）。
    pub taken: Option<String>,
    /// 地図で置いた位置か（EXIF の位置でなく）。
    pub placed: bool,
    world: [f64; 2],
    /// 格子のデータ（NetCDF）の範囲の外周（地図の座標）。写真は空。点はその真ん中に置く。
    outline: Vec<[f64; 2]>,
    /// 外周のうち線を描く点の数。極を回って閉じた分（[`ring`]）は、中か外かを決めるのにだけ使い、
    /// 描かない（地図の継ぎ目に縦の線が出るため）。
    drawn: usize,
}

impl Place {
    pub fn new(path: PathBuf, gps: Gps, taken: Option<String>) -> Self {
        let world = geo_map::project(gps.latitude, gps.longitude);
        Self {
            path,
            gps,
            taken,
            placed: false,
            world,
            outline: Vec::new(),
            drawn: 0,
        }
    }

    /// 外周 `(緯度, 経度)` を足す。日付変更線をまたぐ所は、隣の点から離れないよう x を何周か
    /// ずらして続ける（地図は横に繰り返すので、そのまま描ける）。
    fn with_outline(mut self, outline: &[(f64, f64)]) -> Self {
        self.outline = ring(outline);
        self.drawn = outline.len();
        self
    }
}

///
/// 極を囲む格子（北極の海氷など）の外周は、地球を 1 周して始まりと 1 周ずれた所で終わる。
/// そのまま閉じると地図を横切る線になるので、極の側の地図の端を回って閉じる。
fn ring(outline: &[(f64, f64)]) -> Vec<[f64; 2]> {
    let mut previous: Option<f64> = None;
    let mut points: Vec<[f64; 2]> = outline
        .iter()
        .map(|&(lat, lon)| {
            let [mut x, y] = geo_map::project(lat, lon);
            if let Some(p) = previous {
                x += (p - x).round();
            }
            previous = Some(x);
            [x, y]
        })
        .collect();
    if let (Some(&first), Some(&last)) = (points.first(), points.last())
        && (last[0] - first[0]).abs() > 0.5
    {
        let north = outline.iter().map(|p| p.0).sum::<f64>() > 0.0;
        let edge = if north { 0.0 } else { 1.0 };
        points.extend([[last[0], edge], [first[0], edge]]);
    }
    points
}

/// 読んだ画像 1 枚。地図で置いた位置は、出すときにラベルから重ねる（置いたり消したりする
/// たびに読み直さなくて済むように）。
#[derive(Debug, Clone, PartialEq)]
struct Scanned {
    path: PathBuf,
    gps: Option<Gps>,
    taken: Option<String>,
    /// 読んだときの位置が、地図で置いたものだったか（データセットのカタログ）。
    placed: bool,
    /// 格子のデータの範囲の外周 `(緯度, 経度)`。
    outline: Vec<(f64, f64)>,
}

/// どの範囲の画像を読んだか。フォルダを移ったら読み直す。
#[derive(Debug, Clone, PartialEq)]
enum Source {
    Folder(PathBuf),
    Datasets(Vec<String>),
}

enum Found {
    Total(usize),
    Image(Scanned),
    Step,
    Problem(String),
}

struct Scan {
    rx: Receiver<Found>,
    cancel: Arc<AtomicBool>,
    done: usize,
    total: usize,
}

impl Drop for Scan {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// 点をクリックして選んだもの。
#[derive(Debug, Clone, PartialEq)]
enum Selected {
    One(usize),
    /// 同じ場所で撮った（拡大しても分かれない）画像。
    Many(Vec<usize>),
}

/// アプリへの頼みごと。
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Open(PathBuf),
    /// 地名の表のダウンロードの画面を開く。
    ShowDownloads,
    /// 地図で置いた位置を消す。
    ClearPosition(PathBuf),
    /// 位置の変更を 1 つ戻す。
    Undo,
}

/// 地図を描くのに要るもの。
pub struct Inputs<'a> {
    pub lang: Language,
    pub settings: &'a MapSettings,
    /// 今開いているフォルダ。
    pub folder: Option<&'a Path>,
    pub roots: &'a [String],
    pub registry: &'a Registry,
    pub order: SortOrder,
    /// 取った地図の絵を残すフォルダ。
    pub cache: Option<&'a Path>,
    /// 地名の表。まだダウンロードしていなければ `None`。
    pub places: Option<&'a Gazetteer>,
    /// 地図で置いた位置。
    pub labels: &'a LabelStore,
    /// 格子のデータ（NetCDF）の範囲も描く（設定「座標を出す」）。
    pub extents: bool,
}

pub struct MapWindow {
    pub open: bool,
    scope: Scope,
    view: Option<View>,
    tiles: Option<Tiles>,
    /// 地図に出すもの。`scanned` にラベルの位置を重ねて作る。
    places: Vec<Place>,
    scanned: Vec<Scanned>,
    /// 読んだ範囲の外の画像で、この地図に置いたもの。
    dropped: Vec<PathBuf>,
    /// `places` を作り直す。
    stale: bool,
    source: Option<Source>,
    scan: Option<Scan>,
    /// 読み終えたら、点が全部入るように合わせる。
    fit_when_done: bool,
    selected: Option<Selected>,
    thumb: crate::popup_thumb::PopupThumb,
    problems: Vec<String>,
    /// 地名の検索欄と、その候補。
    query: String,
    names: Vec<FoundName>,
    /// 地図を描いたウインドウ（本体か、欄を出した別ウインドウ）。
    drawn_in: Option<egui::ViewportId>,
    /// 前に描いた地図の範囲（このウインドウの中の座標）。ドロップの場所を地図の座標にするのに使う。
    last_rect: Option<Rect>,
    /// ドラッグ中の画像を放す所（このウインドウの中の座標）と、そこの緯度・経度。
    pub drop_hover: Option<(Pos2, Position)>,
    /// 右クリックした点の画像。
    context: Option<PathBuf>,
    /// 次に描くとき、この範囲（地図の座標）が入るように合わせる。
    fit_to: Option<Vec<[f64; 2]>>,
}

impl Default for MapWindow {
    fn default() -> Self {
        Self {
            open: false,
            scope: Scope::Folder,
            view: None,
            tiles: None,
            places: Vec::new(),
            scanned: Vec::new(),
            dropped: Vec::new(),
            stale: false,
            source: None,
            scan: None,
            fit_when_done: true,
            selected: None,
            thumb: Default::default(),
            problems: Vec::new(),
            query: String::new(),
            names: Vec::new(),
            drawn_in: None,
            last_rect: None,
            drop_hover: None,
            context: None,
            fit_to: None,
        }
    }
}

impl MapWindow {
    /// ログに出す、読めなかったもの。
    pub fn take_problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    /// 閉じている間は地図の絵を取りに行かない。
    pub fn forget_while_closed(&mut self) {
        if !self.open {
            self.tiles = None;
            self.scan = None;
        }
    }

    /// 1 フレーム分、欄の中を描く。
    pub fn ui(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) -> Vec<Command> {
        let ctx = ui.ctx().clone();
        self.follow_source(&ctx, inputs);
        self.receive_scan();
        if self.stale {
            self.rebuild(inputs.labels);
        }
        self.drawn_in = Some(ctx.viewport_id());
        let mut out = Vec::new();
        // 検索欄に打っている間は、文字の取り消しに使う。
        if crate::sub_window::owns_keys(ui)
            && !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z))
        {
            out.push(Command::Undo);
        }
        egui::Panel::top("map_bar").show(ui, |ui| out.extend(self.bar(ui, inputs)));
        // 地図は渡された範囲いっぱいに描くので、上の帯を除いた範囲だけを渡す。
        let rest = ui.available_rect_before_wrap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
            out.extend(self.map(ui, inputs));
        });
        out
    }

    /// スロットのデータの範囲 `(緯度, 経度)` が入るように地図を合わせる。
    pub fn show_extent(&mut self, outline: &[(f64, f64)]) {
        let points = ring(outline);
        self.fit_to = (!points.is_empty()).then_some(points);
    }

    /// 地図で置いた位置が変わった。読んだ範囲の外の画像なら、それも出すようにする。
    pub fn position_changed(&mut self, path: &Path) {
        if !self.scanned.iter().any(|s| s.path == path) && !self.dropped.iter().any(|p| p == path) {
            self.dropped.push(path.to_path_buf());
        }
        self.stale = true;
    }

    /// メインのウインドウの中の座標 `global`（画面全体の座標）が地図の上なら、
    /// このウインドウの中の座標と、そこの緯度・経度を返す。
    ///
    /// ドラッグはメインのウインドウが始めたもので、放したときのマウスの出来事もメインの
    /// ウインドウにしか届かないので、地図の側では受けられない。画面の座標どうしで比べる。
    /// 倍率の違う画面にまたがると少しずれるが、ポイントを画素に直す倍率がウインドウごとに
    /// 違い、両方を合わせて直す手段が egui にないため、そのままにしている。
    pub fn drop_point(&self, ctx: &egui::Context, global: Pos2) -> Option<(Pos2, Position)> {
        if !self.open {
            return None;
        }
        let inner = ctx.input_for(self.drawn_in?, |i| i.viewport().inner_rect)?;
        let rect = self.last_rect?;
        let view = self.view?;
        let local = global - inner.min.to_vec2();
        if !rect.contains(local) {
            return None;
        }
        let (latitude, longitude) = geo_map::unproject(view.to_world(rect, local));
        Some((
            local,
            Position {
                latitude,
                longitude,
            },
        ))
    }

    /// 読んだ画像に、地図で置いた位置を重ねて、出す点を作り直す。選んでいた点は選んだままにする。
    fn rebuild(&mut self, labels: &LabelStore) {
        self.stale = false;
        let chosen: Vec<PathBuf> = match &self.selected {
            Some(Selected::One(i)) => self
                .places
                .get(*i)
                .map(|p| p.path.clone())
                .into_iter()
                .collect(),
            Some(Selected::Many(v)) => v
                .iter()
                .filter_map(|&i| self.places.get(i))
                .map(|p| p.path.clone())
                .collect(),
            None => Vec::new(),
        };
        let extra = self.dropped.iter().map(|path| Scanned {
            path: path.clone(),
            gps: None,
            taken: None,
            placed: false,
            outline: Vec::new(),
        });
        self.places = self
            .scanned
            .iter()
            .cloned()
            .chain(extra)
            .filter_map(|s| {
                let placed = labels.get(&s.path).position;
                let gps = match placed {
                    Some(p) => Gps::new(p.latitude, p.longitude, None),
                    // 前に置いた位置をもう消していれば、カタログの古い位置は使わない。
                    None if s.placed => None,
                    None => s.gps,
                }?;
                let mut place = Place::new(s.path, gps, s.taken).with_outline(&s.outline);
                place.placed = placed.is_some();
                Some(place)
            })
            .collect();
        let index = |path: &PathBuf| self.places.iter().position(|p| &p.path == path);
        let found: Vec<usize> = chosen.iter().filter_map(index).collect();
        self.selected = match (&self.selected, found.as_slice()) {
            (_, []) => None,
            (Some(Selected::One(_)), [one, ..]) => Some(Selected::One(*one)),
            _ => Some(Selected::Many(found)),
        };
    }

    /// 範囲や開いているフォルダが変わったら読み直す。
    fn follow_source(&mut self, ctx: &egui::Context, inputs: &Inputs<'_>) {
        let source = match self.scope {
            Scope::Folder => inputs.folder.map(|f| Source::Folder(f.to_path_buf())),
            Scope::Datasets => Some(Source::Datasets(inputs.roots.to_vec())),
        };
        if source == self.source {
            return;
        }
        self.source = source.clone();
        self.places.clear();
        self.scanned.clear();
        self.dropped.clear();
        self.selected = None;
        self.fit_when_done = true;
        self.scan = source.map(|s| {
            start_scan(
                s,
                inputs.registry.clone(),
                inputs.order,
                inputs.extents,
                ctx,
            )
        });
    }

    fn receive_scan(&mut self) {
        let Some(scan) = &mut self.scan else {
            return;
        };
        let mut finished = false;
        loop {
            match scan.rx.try_recv() {
                Ok(Found::Total(n)) => scan.total = n,
                Ok(Found::Image(i)) => {
                    self.scanned.push(i);
                    self.stale = true;
                }
                Ok(Found::Step) => scan.done += 1,
                Ok(Found::Problem(e)) => self.problems.push(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.scan = None;
        }
    }

    fn bar(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) -> Vec<Command> {
        let lang = inputs.lang;
        let mut out = Vec::new();
        ui.horizontal(|ui| {
            out.extend(self.search(ui, inputs));
            ui.separator();
            let before = self.scope;
            ui.selectable_value(&mut self.scope, Scope::Folder, tr(lang, "This folder"));
            ui.selectable_value(&mut self.scope, Scope::Datasets, tr(lang, "All datasets"));
            if self.scope != before {
                self.source = None;
            }
            ui.separator();
            let fit = ui
                .add_enabled(
                    !self.places.is_empty(),
                    egui::Button::new(icon::CORNERS_OUT),
                )
                .on_hover_text(tr(lang, "Show all places"));
            if fit.clicked() {
                self.fit_when_done = true;
            }
            match &self.scan {
                Some(scan) => {
                    ui.spinner();
                    ui.weak(format!(
                        "{} {} / {}",
                        tr(lang, "Reading positions"),
                        scan.done,
                        scan.total
                    ));
                }
                None => {
                    let text = match (&self.source, self.places.len()) {
                        (None, _) => {
                            tr(lang, "Open a folder to see where its photos were taken.").to_owned()
                        }
                        (Some(_), 0) => tr(lang, "No photos with a position.").to_owned(),
                        (Some(_), n) => tr1(lang, "{} photos with a position", &n.to_string()),
                    };
                    ui.weak(text);
                }
            }
        });
        out
    }

    /// 地名の検索欄。打つたびに候補を出し、選んだ所へ地図を動かす。
    fn search(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) -> Option<Command> {
        let lang = inputs.lang;
        let Some(places) = inputs.places else {
            let r = ui
                .button(format!(
                    "{} {}",
                    icon::MAGNIFYING_GLASS,
                    tr(lang, "Place names...")
                ))
                .on_hover_text(tr(
                    lang,
                    "Download the place names (GeoNames) to search the map by name.",
                ));
            return r.clicked().then_some(Command::ShowDownloads);
        };
        let edit = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text(format!(
                    "{} {}",
                    icon::MAGNIFYING_GLASS,
                    tr(lang, "Search places")
                ))
                .desired_width(200.0),
        );
        if edit.changed() {
            self.names = places.search(&self.query, lang, 12);
        }
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let mut chosen = enter.then(|| self.names.first().cloned()).flatten();
        if !self.names.is_empty() && !self.query.is_empty() {
            egui::Area::new(ui.id().with("map_names"))
                .fixed_pos(edit.rect.left_bottom() + vec2(0.0, 2.0))
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(edit.rect.width());
                        for f in &self.names {
                            if ui.selectable_label(false, &f.label).clicked() {
                                chosen = Some(f.clone());
                            }
                        }
                    });
                });
        }
        if let Some(f) = chosen {
            let view = self.view.get_or_insert_with(View::default);
            view.center = geo_map::project(f.latitude, f.longitude);
            view.zoom = f.zoom;
            self.names.clear();
            self.fit_when_done = false;
        }
        None
    }

    fn map(&mut self, ui: &egui::Ui, inputs: &Inputs<'_>) -> Vec<Command> {
        let rect = ui.max_rect();
        self.last_rect = Some(rect);
        let response = ui.interact(rect, ui.id().with("map"), Sense::click_and_drag());
        let view = self.view.get_or_insert_with(View::default);
        if self.fit_when_done && self.scan.is_none() {
            let points: Vec<[f64; 2]> = self.places.iter().map(|p| p.world).collect();
            if !points.is_empty() {
                view.fit(rect, &points);
            }
            self.fit_when_done = false;
        }
        if let Some(points) = self.fit_to.take() {
            view.fit(rect, &points);
            // 読み終えたときに全部の点へ合わせ直すと、頼まれた範囲から離れてしまう。
            self.fit_when_done = false;
        }
        navigate(ui, &response, rect, view);
        let view = *view;

        let template = &inputs.settings.tile_url;
        if self.tiles.as_ref().is_none_or(|t| t.template() != template) {
            self.tiles = Some(Tiles::new(template, inputs.cache, ui.ctx()));
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        if let Some(tiles) = &mut self.tiles {
            draw_tiles(&painter, ui.visuals(), tiles, &view, rect);
        }

        let hover = response.hover_pos();
        let area = draw_outlines(&painter, &view, rect, &self.places, &self.selected, hover);
        let points: Vec<[f64; 2]> = self.places.iter().map(|p| p.world).collect();
        let clusters = geo_map::cluster(&points, view.zoom, CLUSTER_CELL);
        let hit = draw_pins(
            &painter,
            &view,
            rect,
            &clusters,
            &self.selected,
            response.hover_pos(),
        );
        if let Some(i) = hit {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            let c = &clusters[i];
            let tip = match c.members.as_slice() {
                [one] => file_name(&self.places[*one].path),
                many => tr1(inputs.lang, "{} photos", &many.len().to_string()),
            };
            response.clone().on_hover_text_at_pointer(tip);
        } else if let Some(i) = area {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            response
                .clone()
                .on_hover_text_at_pointer(file_name(&self.places[i].path));
        }
        if response.clicked() {
            match (hit, area) {
                (None, Some(i)) => self.selected = Some(Selected::One(i)),
                _ => self.click(hit.map(|i| &clusters[i]), rect),
            }
        }
        if response.double_clicked() && hit.is_none() {
            self.selected = None;
        }
        if let Some((at, _)) = self.drop_hover {
            draw_drop_marker(&painter, at, tr(inputs.lang, "Place here"));
        }
        let mut out = Vec::new();
        if response.secondary_clicked() {
            self.context = hit
                .and_then(|i| match clusters[i].members.as_slice() {
                    [one] => self.places.get(*one),
                    _ => None,
                })
                .filter(|p| p.placed)
                .map(|p| p.path.clone());
        }
        if let Some(path) = self.context.clone() {
            response.context_menu(|ui| {
                if ui.button(tr(inputs.lang, "Remove the position")).clicked() {
                    out.push(Command::ClearPosition(path));
                    ui.close();
                }
            });
        }
        self.attribution(ui, inputs, rect);
        if let Some(c) = self.popup(ui, inputs, rect) {
            out.push(c);
        }
        out
    }

    fn click(&mut self, cluster: Option<&Cluster>, rect: Rect) {
        let Some(c) = cluster else {
            self.selected = None;
            return;
        };
        let Some(view) = &mut self.view else {
            return;
        };
        match c.members.as_slice() {
            [one] => self.selected = Some(Selected::One(*one)),
            many => {
                let points: Vec<[f64; 2]> = many.iter().map(|&i| self.places[i].world).collect();
                // 一番拡大しても分かれない（同じ所で撮った）なら、拡大せずに一覧にする。
                let apart = geo_map::cluster(&points, geo_map::MAX_ZOOM, CLUSTER_CELL).len() > 1;
                let mut zoomed = *view;
                zoomed.fit(rect, &points);
                if apart && zoomed.zoom > view.zoom + 0.5 {
                    *view = zoomed;
                    self.selected = None;
                } else {
                    self.selected = Some(Selected::Many(many.to_vec()));
                }
            }
        }
    }

    /// 選んだ点の上に、縮小画像・名前・撮影日時と「開く」を出す。
    fn popup(&mut self, ui: &egui::Ui, inputs: &Inputs<'_>, rect: Rect) -> Option<Command> {
        let lang = inputs.lang;
        let selected = self.selected.clone()?;
        let view = self.view?;
        let first = match &selected {
            Selected::One(i) => *i,
            Selected::Many(v) => *v.first()?,
        };
        let anchor = view
            .copies(rect, self.places.get(first)?.world, 0.0)
            .into_iter()
            .min_by(|a, b| {
                let d = |p: &Pos2| (p.x - rect.center().x).abs();
                d(a).total_cmp(&d(b))
            })?;
        let mut command = None;
        let mut choose = None;
        egui::Area::new(ui.id().with("map_popup"))
            .fixed_pos(anchor + vec2(12.0, -12.0))
            .order(egui::Order::Foreground)
            .constrain_to(rect)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| match &selected {
                    Selected::One(i) => {
                        let place = self.places[*i].clone();
                        command = self.place_card(ui, inputs, &place);
                    }
                    Selected::Many(members) => {
                        ui.label(tr1(lang, "{} photos", &members.len().to_string()));
                        egui::ScrollArea::vertical()
                            .max_height(240.0)
                            .show(ui, |ui| {
                                for &i in members {
                                    if ui
                                        .selectable_label(false, file_name(&self.places[i].path))
                                        .clicked()
                                    {
                                        choose = Some(i);
                                    }
                                }
                            });
                    }
                });
            });
        if let Some(i) = choose {
            self.selected = Some(Selected::One(i));
        }
        command
    }

    /// 1 枚の情報と、「開く」（地図で置いた位置なら「位置を消す」も）。
    fn place_card(
        &mut self,
        ui: &mut egui::Ui,
        inputs: &Inputs<'_>,
        place: &Place,
    ) -> Option<Command> {
        let lang = inputs.lang;
        self.thumb
            .show(ui, lang, inputs.registry, &place.path, 180.0);
        ui.strong(file_name(&place.path));
        if let Some(t) = &place.taken {
            ui.weak(t);
        }
        if let Some(a) = inputs
            .places
            .and_then(|g| g.address(place.gps.latitude, place.gps.longitude))
        {
            ui.label(a.text(lang));
        }
        ui.weak(format!(
            "{:.5}, {:.5}",
            place.gps.latitude, place.gps.longitude
        ));
        if place.placed {
            ui.weak(tr(lang, "Placed on the map"));
        }
        ui.horizontal(|ui| {
            let open = ui.button(tr(lang, "Open")).clicked();
            let clear = place.placed && ui.button(tr(lang, "Remove the position")).clicked();
            if open {
                Some(Command::Open(place.path.clone()))
            } else if clear {
                Some(Command::ClearPosition(place.path.clone()))
            } else {
                None
            }
        })
        .inner
    }

    /// 地図の絵の出典。提供元の条件で、いつも見える所に出す。
    fn attribution(&self, ui: &egui::Ui, inputs: &Inputs<'_>, rect: Rect) {
        let s = inputs.settings;
        egui::Area::new(ui.id().with("map_credit"))
            .fixed_pos(rect.right_bottom())
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .order(egui::Order::Middle)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(ui.visuals().panel_fill.gamma_multiply(0.85))
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if let Some(problem) =
                                self.tiles.as_ref().and_then(|t| t.problem.as_ref())
                            {
                                ui.colored_label(ui.visuals().warn_fg_color, icon::WARNING)
                                    .on_hover_text(format!(
                                        "{}\n{problem}",
                                        tr(inputs.lang, "Cannot download the map pictures.")
                                    ));
                            }
                            ui.hyperlink_to(
                                egui::RichText::new(&s.attribution).small(),
                                &s.attribution_url,
                            );
                            if inputs.places.is_some() {
                                ui.hyperlink_to(
                                    egui::RichText::new("GeoNames (CC BY 4.0)").small(),
                                    crate::geonames::CREDIT_URL,
                                )
                                .on_hover_text(crate::geonames::CREDIT);
                            }
                        });
                    });
            });
    }
}

/// ドラッグで移動、ホイールとピンチでカーソルの所を中心に拡大縮小、ダブルクリックで 1 段拡大。
fn navigate(ui: &egui::Ui, response: &egui::Response, rect: Rect, view: &mut View) {
    if response.dragged_by(egui::PointerButton::Primary) {
        view.pan(rect, response.drag_delta());
    }
    if let Some(at) = response.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let dz = f64::from(scroll) / 200.0 + f64::from(pinch).log2();
        if dz != 0.0 {
            view.zoom_at(rect, at, dz);
        }
        if response.double_clicked() {
            view.zoom_at(rect, at, 1.0);
        }
    }
    // 画面の大きさが変わったときも、南北の外が見えないように合わせ直す。
    view.pan(rect, egui::Vec2::ZERO);
}

/// タイルを並べる。まだ取れていないタイルは、持っている広い範囲のタイルを引き伸ばして
/// 代わりに描き、それもなければ方眼にする（つながらなくても、点の位置の目安になるように）。
fn draw_tiles(
    painter: &egui::Painter,
    visuals: &egui::Visuals,
    tiles: &mut Tiles,
    view: &View,
    rect: Rect,
) {
    tiles.receive(painter.ctx());
    let visible = view.tiles(rect);
    let ids: Vec<_> = visible.iter().map(|(id, _)| *id).collect();
    tiles.keep_only(&ids);
    let full = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    let grid = Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
    for (id, at) in visible {
        if let Some(texture) = tiles.get(id) {
            painter.image(texture.id(), at, full, Color32::WHITE);
            continue;
        }
        let mut uv = full;
        let mut parent = id;
        let mut drawn = false;
        for _ in 0..6 {
            let Some((p, part)) = parent.parent() else {
                break;
            };
            // 子の範囲を、親のタイルの中の範囲に重ねていく。
            uv = Rect::from_min_size(
                part.min + uv.min.to_vec2() * part.width(),
                uv.size() * part.width(),
            );
            parent = p;
            if let Some(texture) = tiles.cached(parent) {
                painter.image(texture.id(), at, uv, Color32::WHITE);
                drawn = true;
                break;
            }
        }
        if !drawn {
            painter.rect_stroke(at, 0.0, grid, egui::StrokeKind::Inside);
        }
    }
    if tiles.loading() {
        painter
            .ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// ドラッグ中の画像を放す所。置かれる点と同じ赤の、中の空いた丸と十字。
fn draw_drop_marker(painter: &egui::Painter, at: Pos2, label: &str) {
    let stroke = Stroke::new(2.0, PIN_RED);
    painter.circle_stroke(at, PIN + 4.0, stroke);
    let arm = PIN + 10.0;
    painter.line_segment([at - vec2(arm, 0.0), at + vec2(arm, 0.0)], stroke);
    painter.line_segment([at - vec2(0.0, arm), at + vec2(0.0, arm)], stroke);
    let text = painter.layout_no_wrap(
        format!("{} {label}", icon::MAP_PIN),
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
    let corner = at + vec2(arm + 4.0, -text.size().y / 2.0);
    let back = Rect::from_min_size(corner, text.size()).expand(4.0);
    painter.rect_filled(back, 4.0, PIN_RED);
    painter.galley(corner, text, Color32::WHITE);
}

/// 赤い点と、まとめた丸を描く。カーソルの下にあるものの番号を返す。
/// 格子のデータの範囲を枠で描き、カーソルの下にある範囲（いちばん小さいもの）を返す。
/// 小さいものを選ぶのは、全球のデータの中に地域のデータが重なっていても、地域のほうを選べるように。
fn draw_outlines(
    painter: &egui::Painter,
    view: &View,
    rect: Rect,
    places: &[Place],
    selected: &Option<Selected>,
    hover: Option<Pos2>,
) -> Option<usize> {
    let width = view.scale() as f32;
    let at = hover.map(|h| view.to_world(rect, h));
    let mut hit: Option<(usize, f64)> = None;
    for (i, p) in places
        .iter()
        .enumerate()
        .filter(|(_, p)| p.outline.len() > 2)
    {
        let chosen = matches!(selected, Some(Selected::One(s)) if *s == i);
        let stroke = Stroke::new(if chosen { 2.5 } else { 1.5 }, PIN_RED);
        let ring: Vec<Pos2> = p.outline.iter().map(|&w| view.to_screen(rect, w)).collect();
        let bounds = Rect::from_points(&ring);
        let first = ((rect.min.x - bounds.max.x) / width).floor() as i32;
        let last = ((rect.max.x - bounds.min.x) / width).ceil() as i32;
        for k in first..=last {
            let shift = vec2(k as f32 * width, 0.0);
            if !bounds.translate(shift).intersects(rect) {
                continue;
            }
            let mut line: Vec<Pos2> = ring.iter().take(p.drawn).map(|&q| q + shift).collect();
            if p.drawn == ring.len() {
                line.push(line[0]);
            }
            painter.add(egui::Shape::line(line, stroke));
        }
        let area = polygon_area(&p.outline);
        if let Some(w) = at
            && (-2..=2).any(|k| inside(&p.outline, [w[0] + f64::from(k), w[1]]))
            && hit.is_none_or(|(_, a)| area < a)
        {
            hit = Some((i, area));
        }
    }
    hit.map(|(i, _)| i)
}

/// 点 `p` が多角形 `ring` の中か（偶奇の規則）。
fn inside(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut odd = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            odd = !odd;
        }
        j = i;
    }
    odd
}

fn polygon_area(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        .abs()
        / 2.0
}

fn draw_pins(
    painter: &egui::Painter,
    view: &View,
    rect: Rect,
    clusters: &[Cluster],
    selected: &Option<Selected>,
    hover: Option<Pos2>,
) -> Option<usize> {
    let outline = Stroke::new(1.5, Color32::WHITE);
    let mut hit = None;
    for (i, c) in clusters.iter().enumerate() {
        let count = c.members.len();
        let radius = if count == 1 {
            PIN
        } else {
            9.0 + (count as f32).log10() * 4.0
        };
        let chosen = match selected {
            Some(Selected::One(s)) => c.members.contains(s),
            Some(Selected::Many(v)) => v.iter().any(|s| c.members.contains(s)),
            None => false,
        };
        for at in view.copies(rect, c.world, radius) {
            let r = if chosen { radius + 2.0 } else { radius };
            painter.circle(at, r, PIN_RED, outline);
            if count > 1 {
                painter.text(
                    at,
                    egui::Align2::CENTER_CENTER,
                    count.to_string(),
                    egui::FontId::proportional(11.0),
                    Color32::WHITE,
                );
            }
            if hover.is_some_and(|h| h.distance(at) <= r + 3.0) {
                hit = Some(i);
            }
        }
    }
    hit
}

fn file_name(path: &Path) -> String {
    let file = crate::frames::file_of(path);
    file.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn start_scan(
    source: Source,
    registry: Registry,
    order: SortOrder,
    extents: bool,
    ctx: &egui::Context,
) -> Scan {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let send = |f: Found| {
            let _ = tx.send(f);
            ctx.request_repaint();
        };
        match source {
            Source::Folder(dir) => {
                let images = crate::tree::list_folder(&dir, &registry, order).images;
                send(Found::Total(images.len()));
                for path in images {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let (mut gps, taken) = match vw_core::exif::gps_of_file(&path) {
                        Some((gps, taken)) => (Some(gps), taken),
                        None => (None, None),
                    };
                    let mut outline = Vec::new();
                    if gps.is_none()
                        && extents
                        && let Some(e) = netcdf_extent(&path)
                    {
                        gps = grid_center(e.center);
                        outline = e.outline;
                    }
                    send(Found::Image(Scanned {
                        path,
                        gps,
                        taken,
                        placed: false,
                        outline,
                    }));
                    send(Found::Step);
                }
            }
            Source::Datasets(roots) => {
                let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
                let index = crate::dataset_search::Index::discover(&roots);
                for p in index.problems {
                    send(Found::Problem(p));
                }
                send(Found::Total(index.entries.len()));
                for entry in &index.entries {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    for place in catalog_places(&entry.dir, &entry.catalog) {
                        send(Found::Image(Scanned {
                            path: place.path,
                            gps: Some(place.gps),
                            taken: place.taken,
                            placed: place.placed,
                            outline: Vec::new(),
                        }));
                    }
                    send(Found::Step);
                }
            }
        }
    });
    Scan {
        rx,
        cancel,
        done: 0,
        total: 0,
    }
}

/// 範囲の真ん中に置く点。緯度・経度がちょうど 0° の所は、写真の EXIF では「位置なし」の印なので
/// [`Gps::new`] が受け付けない。全球の格子の真ん中は 0° になりやすいので、見えないほどずらす。
fn grid_center((lat, lon): (f64, f64)) -> Option<Gps> {
    Gps::new(lat, lon, None).or_else(|| Gps::new(lat, lon + 1e-6, None))
}

/// NetCDF のファイルの範囲。先頭のバイトで見分け、それ以外のファイルは中身を読まない。
fn netcdf_extent(path: &Path) -> Option<vw_netcdf::Extent> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 8];
    file.read_exact(&mut head).ok()?;
    let size = file.metadata().ok()?.len();
    if !vw_netcdf::is_netcdf(&head) || size > vw_core::limits::max_image_bytes() as u64 {
        return None;
    }
    vw_netcdf::extent(&std::fs::read(path).ok()?)
}

/// 検索カタログに入っている撮影地点。ZIP / 7z の中の画像は、そこから直接開けないので出さない。
pub fn catalog_places(dir: &Path, catalog: &crate::catalog::Catalog) -> Vec<Place> {
    let location = vw_core::exif::LOCATION.to_lowercase();
    catalog
        .files
        .iter()
        .filter(|f| !f.path.contains(crate::catalog::IN_ARCHIVE))
        .filter_map(|f| {
            let value = |kind: &str, key: &str| {
                f.meta.iter().find_map(|&i| {
                    let (k, name, v) = catalog.meta.get(i as usize)?;
                    (k == kind && name == key).then(|| v.clone())
                })
            };
            let number = |key| value(&location, key)?.parse::<f64>().ok();
            let gps = Gps::new(number("Latitude")?, number("Longitude")?, None)?;
            let taken = value("exif", "DateTimeOriginal");
            let mut place = Place::new(dir.join(&f.path), gps, taken);
            place.placed = value(&location, crate::labels::POSITION_FROM).as_deref()
                == Some(crate::labels::FROM_MAP);
            Some(place)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, FileEntry};

    fn entry(path: &str, meta: Vec<u32>) -> FileEntry {
        FileEntry {
            path: path.into(),
            size: 0,
            modified: String::new(),
            meta,
        }
    }

    fn scanned(path: &Path, gps: Option<Gps>, placed: bool) -> Scanned {
        Scanned {
            path: path.to_path_buf(),
            gps,
            taken: None,
            placed,
            outline: Vec::new(),
        }
    }

    const SAPPORO: Position = Position {
        latitude: 43.0621,
        longitude: 141.3544,
    };

    #[test]
    fn position_placed_on_the_map_wins_over_exif() {
        let dir = crate::test_util::TempDir::new();
        let img = dir.touch("a.jpg");
        let labels = LabelStore::new(None);
        labels.set_position(&img, Some(SAPPORO)).unwrap();
        let mut map = MapWindow {
            scanned: vec![scanned(&img, Gps::new(35.0, 139.0, None), false)],
            ..Default::default()
        };
        map.rebuild(&labels);
        assert_eq!(map.places.len(), 1);
        assert!(map.places[0].placed);
        assert!((map.places[0].gps.latitude - SAPPORO.latitude).abs() < 1e-9);
    }

    #[test]
    fn image_without_exif_appears_once_placed() {
        let dir = crate::test_util::TempDir::new();
        let img = dir.touch("a.png");
        let labels = LabelStore::new(None);
        let mut map = MapWindow {
            scanned: vec![scanned(&img, None, false)],
            ..Default::default()
        };
        map.rebuild(&labels);
        assert!(map.places.is_empty());
        labels.set_position(&img, Some(SAPPORO)).unwrap();
        map.position_changed(&img);
        map.rebuild(&labels);
        assert_eq!(map.places.len(), 1);
    }

    #[test]
    fn image_outside_the_scanned_folder_is_shown_after_dropping() {
        let dir = crate::test_util::TempDir::new();
        let img = dir.touch("elsewhere.png");
        let labels = LabelStore::new(None);
        labels.set_position(&img, Some(SAPPORO)).unwrap();
        let mut map = MapWindow::default();
        map.position_changed(&img);
        map.rebuild(&labels);
        assert_eq!(map.places.len(), 1);
    }

    #[test]
    fn removed_position_falls_back_to_exif_but_not_to_an_old_catalog_one() {
        let dir = crate::test_util::TempDir::new();
        let exif = dir.touch("exif.jpg");
        let cataloged = dir.touch("cataloged.png");
        let labels = LabelStore::new(None);
        let mut map = MapWindow {
            scanned: vec![
                scanned(&exif, Gps::new(35.0, 139.0, None), false),
                scanned(&cataloged, Gps::new(43.0, 141.0, None), true),
            ],
            ..Default::default()
        };
        map.rebuild(&labels);
        assert_eq!(map.places.len(), 1);
        assert_eq!(map.places[0].path, exif);
        assert!(!map.places[0].placed);
    }

    #[test]
    fn selection_follows_the_image_when_places_are_rebuilt() {
        let dir = crate::test_util::TempDir::new();
        let a = dir.touch("a.jpg");
        let b = dir.touch("b.jpg");
        let labels = LabelStore::new(None);
        let mut map = MapWindow {
            scanned: vec![
                scanned(&a, None, false),
                scanned(&b, Gps::new(35.0, 139.0, None), false),
            ],
            ..Default::default()
        };
        map.rebuild(&labels);
        map.selected = Some(Selected::One(0));
        labels.set_position(&a, Some(SAPPORO)).unwrap();
        map.rebuild(&labels);
        assert_eq!(map.selected, Some(Selected::One(1)));
        assert_eq!(map.places[1].path, b);
    }

    #[test]
    fn catalog_positions_become_places() {
        let t = |k: &str, key: &str, v: &str| (k.to_owned(), key.to_owned(), v.to_owned());
        let catalog = Catalog {
            meta: vec![
                t("location", "Latitude", "43.062100"),
                t("location", "Longitude", "141.354400"),
                t("exif", "DateTimeOriginal", "2026-01-02 03:04:05"),
            ],
            files: vec![
                entry("a/sapporo.jpg", vec![0, 1, 2]),
                entry("no-gps.jpg", vec![2]),
                entry("photos.zip!/inside.jpg", vec![0, 1]),
            ],
            ..Default::default()
        };
        let places = catalog_places(Path::new("/d"), &catalog);
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].path, Path::new("/d/a/sapporo.jpg"));
        assert_eq!(places[0].taken.as_deref(), Some("2026-01-02 03:04:05"));
        assert!((places[0].gps.longitude - 141.3544).abs() < 1e-9);
    }

    #[test]
    fn outlines_across_the_date_line_stay_in_one_piece() {
        let r = ring(&[
            (10.0, 170.0),
            (10.0, -170.0),
            (-10.0, -170.0),
            (-10.0, 170.0),
        ]);
        assert!(r[1][0] > 1.0, "x continues past the edge: {r:?}");
        assert!((r[1][0] - r[0][0] - 20.0 / 360.0).abs() < 1e-9);
    }

    #[test]
    fn outlines_around_a_pole_are_closed_over_the_pole() {
        let around: Vec<(f64, f64)> = (0..8)
            .map(|i| (60.0, -180.0 + 45.0 * f64::from(i)))
            .collect();
        let r = ring(&around);
        assert_eq!(r.len(), 10);
        assert_eq!(r[8][1], 0.0);
        assert_eq!(r[9], [r[0][0], 0.0]);
        assert!(inside(&r, [0.5, 0.05]), "the pole is inside");
        assert!(!inside(&r, [0.5, 0.5]), "the equator is outside");
    }

    #[test]
    fn a_point_inside_an_outline_is_found() {
        let square = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(inside(&square, [0.5, 0.5]));
        assert!(!inside(&square, [1.5, 0.5]));
        assert!((polygon_area(&square) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn grids_centered_on_zero_degrees_still_get_a_point() {
        let g = grid_center((0.0, 0.0)).expect("point");
        assert!(g.longitude.abs() < 1e-5);
        assert!(grid_center((35.0, 139.0)).is_some());
    }

    #[test]
    fn netcdf_files_in_the_folder_become_outlined_places() {
        let dir = crate::test_util::TempDir::new();
        let file = dir.path().join("geo.nc");
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../crates/vw-netcdf/testdata/geo_cases.nc"
            ),
            &file,
        )
        .unwrap();
        let e = netcdf_extent(&file).expect("extent");
        let mut s = scanned(&file, grid_center(e.center), false);
        s.outline = e.outline;
        let mut w = MapWindow::default();
        w.scanned.push(s);
        w.rebuild(&LabelStore::new(None));
        assert_eq!(w.places.len(), 1);
        assert_eq!(w.places[0].outline.len(), 64);
        assert_eq!(netcdf_extent(&dir.path().join("missing.nc")), None);
    }
}
