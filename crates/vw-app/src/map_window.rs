//! 地図のウインドウ。画像の撮影地点（EXIF の GPS）を地図の上に赤い点で出す。
//!
//! 出す画像は「今開いているフォルダ」か「登録したデータセット全体」から選ぶ。近い点は
//! 数字入りの丸にまとめ、点をクリックすると縮小画像と名前を出し、そこから画像を開ける。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Sense, Stroke, TextureHandle};
use egui_phosphor::regular as icon;
use vw_core::exif::Gps;
use vw_core::Registry;

use crate::geo_map::{self, Cluster, View};
use crate::i18n::{tr, tr1};
use crate::map_tiles::Tiles;
use crate::settings::{Language, MapSettings};
use crate::sort::SortOrder;
use crate::sub_window::SubWindow;

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
    world: [f64; 2],
}

impl Place {
    pub fn new(path: PathBuf, gps: Gps, taken: Option<String>) -> Self {
        let world = geo_map::project(gps.latitude, gps.longitude);
        Self {
            path,
            gps,
            taken,
            world,
        }
    }
}

/// どの範囲の画像を読んだか。フォルダを移ったら読み直す。
#[derive(Debug, Clone, PartialEq)]
enum Source {
    Folder(PathBuf),
    Datasets(Vec<String>),
}

enum Found {
    Total(usize),
    Place(Place),
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

enum Thumb {
    Loading(PathBuf, Receiver<Option<egui::ColorImage>>),
    Ready(PathBuf, TextureHandle),
    Failed(PathBuf),
}

impl Thumb {
    fn path(&self) -> &Path {
        match self {
            Thumb::Loading(p, _) | Thumb::Ready(p, _) | Thumb::Failed(p) => p,
        }
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
}

pub struct MapWindow {
    pub open: bool,
    pub window: SubWindow,
    scope: Scope,
    view: Option<View>,
    tiles: Option<Tiles>,
    places: Vec<Place>,
    source: Option<Source>,
    scan: Option<Scan>,
    /// 読み終えたら、点が全部入るように合わせる。
    fit_when_done: bool,
    selected: Option<Selected>,
    thumb: Option<Thumb>,
    problems: Vec<String>,
}

impl Default for MapWindow {
    fn default() -> Self {
        Self {
            open: false,
            window: SubWindow::default(),
            scope: Scope::Folder,
            view: None,
            tiles: None,
            places: Vec::new(),
            source: None,
            scan: None,
            fit_when_done: true,
            selected: None,
            thumb: None,
            problems: Vec::new(),
        }
    }
}

impl MapWindow {
    /// ログに出す、読めなかったもの。
    pub fn take_problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    pub fn ui(&mut self, ctx: &egui::Context, inputs: &Inputs<'_>) -> Vec<Command> {
        if !self.open {
            // 閉じている間は地図の絵を取りに行かない。
            self.tiles = None;
            self.scan = None;
            return Vec::new();
        }
        self.follow_source(ctx, inputs);
        self.receive_scan();
        let lang = inputs.lang;
        let mut out = Vec::new();
        let mut close = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("map"),
            self.window
                .builder(tr(lang, "Map").to_owned(), [900.0, 640.0]),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                self.window.track(ctx);
                egui::Panel::top("map_bar").show(ctx, |ui| self.bar(ui, inputs));
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| out = self.map(ui, inputs));
                self.window.pin_overlay(ctx, lang);
            },
        );
        if close {
            self.open = false;
        }
        out
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
        self.selected = None;
        self.fit_when_done = true;
        self.scan = source.map(|s| start_scan(s, inputs.registry.clone(), inputs.order, ctx));
    }

    fn receive_scan(&mut self) {
        let Some(scan) = &mut self.scan else {
            return;
        };
        let mut finished = false;
        loop {
            match scan.rx.try_recv() {
                Ok(Found::Total(n)) => scan.total = n,
                Ok(Found::Place(p)) => self.places.push(p),
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

    fn bar(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) {
        let lang = inputs.lang;
        ui.horizontal(|ui| {
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
    }

    fn map(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) -> Vec<Command> {
        let rect = ui.max_rect();
        let response = ui.interact(rect, ui.id().with("map"), Sense::click_and_drag());
        let view = self.view.get_or_insert_with(View::default);
        if self.fit_when_done && self.scan.is_none() {
            let points: Vec<[f64; 2]> = self.places.iter().map(|p| p.world).collect();
            if !points.is_empty() {
                view.fit(rect, &points);
            }
            self.fit_when_done = false;
        }
        navigate(ui, &response, rect, view);
        let view = *view;

        let template = &inputs.settings.tile_url;
        if self.tiles.as_ref().is_none_or(|t| t.template() != template) {
            self.tiles = Some(Tiles::new(
                template,
                inputs.cache,
                inputs.registry.clone(),
                ui.ctx(),
            ));
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        if let Some(tiles) = &mut self.tiles {
            draw_tiles(&painter, ui.visuals(), tiles, &view, rect);
        }

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
        }
        if response.clicked() {
            self.click(hit.map(|i| &clusters[i]), rect);
        }
        if response.double_clicked() && hit.is_none() {
            self.selected = None;
        }
        self.attribution(ui, inputs, rect);
        let mut out = Vec::new();
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
    fn popup(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>, rect: Rect) -> Option<Command> {
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
                        if self.place_card(ui, inputs, &place) {
                            command = Some(Command::Open(place.path.clone()));
                        }
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

    /// 1 枚の情報。「開く」を押したら true。
    fn place_card(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>, place: &Place) -> bool {
        let lang = inputs.lang;
        self.load_thumb(ui.ctx(), inputs.registry, &place.path);
        let side = 180.0;
        match &self.thumb {
            Some(Thumb::Ready(_, texture)) => {
                let size = texture.size_vec2();
                let fit = (side / size.x).min(side / size.y).min(1.0);
                ui.image((texture.id(), size * fit));
            }
            Some(Thumb::Failed(_)) => {
                ui.weak(tr(lang, "Failed to load"));
            }
            _ => {
                ui.add_sized([side, side * 0.66], egui::Spinner::new());
            }
        }
        ui.strong(file_name(&place.path));
        if let Some(t) = &place.taken {
            ui.weak(t);
        }
        ui.weak(format!(
            "{:.5}, {:.5}",
            place.gps.latitude, place.gps.longitude
        ));
        ui.button(tr(lang, "Open")).clicked()
    }

    fn load_thumb(&mut self, ctx: &egui::Context, registry: &Registry, path: &Path) {
        if self.thumb.as_ref().is_none_or(|t| t.path() != path) {
            let (tx, rx) = channel();
            let (registry, file, ctx2) = (registry.clone(), path.to_path_buf(), ctx.clone());
            std::thread::spawn(move || {
                let thumb = crate::loader::load_fit(&registry, &file, true, false, None)
                    .ok()
                    .and_then(|l| l.thumb);
                let _ = tx.send(thumb);
                ctx2.request_repaint();
            });
            self.thumb = Some(Thumb::Loading(path.to_path_buf(), rx));
        }
        if let Some(Thumb::Loading(p, rx)) = &self.thumb {
            if let Ok(image) = rx.try_recv() {
                let p = p.clone();
                self.thumb = Some(match image {
                    Some(image) => Thumb::Ready(
                        p,
                        ctx.load_texture("map_thumb", image, egui::TextureOptions::LINEAR),
                    ),
                    None => Thumb::Failed(p),
                });
            }
        }
    }

    /// 地図の絵の出典。提供元の条件で、いつも見える所に出す。
    fn attribution(&self, ui: &mut egui::Ui, inputs: &Inputs<'_>, rect: Rect) {
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

/// 赤い点と、まとめた丸を描く。カーソルの下にあるものの番号を返す。
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

fn start_scan(source: Source, registry: Registry, order: SortOrder, ctx: &egui::Context) -> Scan {
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
                    if let Some((gps, taken)) = vw_core::exif::gps_of_file(&path) {
                        send(Found::Place(Place::new(path, gps, taken)));
                    }
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
                        send(Found::Place(place));
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
            Some(Place::new(dir.join(&f.path), gps, taken))
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
}
