//! 星図のウインドウ。天体の画像（FITS）が空のどこを写したかを、星図の上に枠で出す。
//!
//! 地図のウインドウの天球版。ヘッダの WCS から分かる範囲は実線、架台の向きと焦点距離から
//! 見積もった範囲は点線、向きしか分からない画像は十字で出す。枠をクリックすると縮小画像と
//! 名前を出し、そこから画像を開ける。アクティブなスロットの画像の枠は太く描く。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Sense, Stroke};
use egui_phosphor::regular as icon;
use vw_core::Registry;
use vw_fits::Sky;

use crate::i18n::{tr, tr1};
use crate::marks;
use crate::settings::{Language, SkySettings};
use crate::sky_chart::{self, Background, Radec, View};
use crate::sort::SortOrder;

/// 星図に出す画像の範囲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Folder,
    Datasets,
}

/// 写った範囲の分かる画像 1 枚。
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub path: PathBuf,
    pub sky: Sky,
    /// `OBJECT`（何を撮ったか）。
    pub object: Option<String>,
    /// `DATE-OBS`（撮った日時）。
    pub taken: Option<String>,
}

impl Item {
    /// ヘッダの値を引く `lookup` から作る。写った範囲が分からなければ `None`。
    pub fn from_header(path: PathBuf, lookup: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let text = |key: &str| {
            lookup(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        Some(Self {
            sky: vw_fits::sky(lookup)?,
            object: text("OBJECT"),
            taken: text("DATE-OBS"),
            path,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Source {
    Folder(PathBuf),
    Datasets(Vec<String>),
}

enum Found {
    Total(usize),
    Item(Item),
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

/// 枠をクリックして選んだもの。
#[derive(Debug, Clone, PartialEq)]
enum Selected {
    One(usize),
    /// 重なった枠（同じ対象を続けて撮った画像など）。
    Many(Vec<usize>),
}

/// アプリへの頼みごと。
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Open(PathBuf),
}

/// 星図を描くのに要るもの。
pub struct Inputs<'a> {
    pub lang: Language,
    pub settings: &'a SkySettings,
    /// 今開いているフォルダ。
    pub folder: Option<&'a Path>,
    pub roots: &'a [String],
    pub registry: &'a Registry,
    pub order: SortOrder,
    /// アクティブなスロットの画像。
    pub active: Option<&'a Path>,
}

pub struct SkyWindow {
    pub open: bool,
    scope: Scope,
    view: Option<View>,
    /// 背景の星と星座。初めて開いたときに読む。
    background: Option<Background>,
    items: Vec<Item>,
    source: Option<Source>,
    scan: Option<Scan>,
    /// 読み終えたら、枠が全部入るように合わせる。
    fit_when_done: bool,
    selected: Option<Selected>,
    thumb: crate::popup_thumb::PopupThumb,
    problems: Vec<String>,
}

impl Default for SkyWindow {
    fn default() -> Self {
        Self {
            open: false,
            scope: Scope::Folder,
            view: None,
            background: None,
            items: Vec::new(),
            source: None,
            scan: None,
            fit_when_done: true,
            selected: None,
            thumb: Default::default(),
            problems: Vec::new(),
        }
    }
}

/// 星図の地の色。昼の空の色にしないのは、白地では暗い星や薄い線が見えないため。
const SKY_BACKGROUND: Color32 = Color32::from_rgb(10, 14, 28);
const GRID: Color32 = Color32::from_rgb(40, 52, 80);
const BORDER: Color32 = Color32::from_rgb(70, 70, 40);
const FIGURE: Color32 = Color32::from_rgb(70, 110, 160);
const NAME: Color32 = Color32::from_rgb(120, 140, 170);
/// 十字の腕の長さ（画素）。クリックの当たりもこの範囲。
const CROSS_ARM: f32 = 8.0;
/// 名札を出す枠の大きさの下限（画素）。小さな枠に全部出すと、名札で星図が埋まるため。
const TAGGED_SIDE: f32 = 48.0;

impl SkyWindow {
    /// ログに出す、読めなかったもの。
    pub fn take_problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    /// 閉じている間はファイルを読みに行かない。
    pub fn forget_while_closed(&mut self) {
        if !self.open {
            self.scan = None;
            self.source = None;
        }
    }

    /// 1 フレーム分、欄の中を描く。
    pub fn ui(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) -> Vec<Command> {
        let ctx = ui.ctx().clone();
        self.follow_source(&ctx, inputs);
        self.receive_scan();
        egui::Panel::top("sky_bar").show(ui, |ui| self.bar(ui, inputs));
        let rest = ui.available_rect_before_wrap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
            self.chart(ui, inputs)
        })
        .inner
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
        self.items.clear();
        self.selected = None;
        self.fit_when_done = true;
        self.scan = source.map(|s| start_scan(s, inputs.registry.clone(), inputs.order, ctx));
    }

    fn receive_scan(&mut self) {
        let Some(scan) = &mut self.scan else {
            return;
        };
        loop {
            match scan.rx.try_recv() {
                Ok(Found::Total(n)) => scan.total = n,
                Ok(Found::Item(i)) => self.items.push(i),
                Ok(Found::Step) => scan.done += 1,
                Ok(Found::Problem(e)) => self.problems.push(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        self.scan = None;
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
                .add_enabled(!self.items.is_empty(), egui::Button::new(icon::CORNERS_OUT))
                .on_hover_text(tr(lang, "Show all frames"));
            if fit.clicked() {
                self.fit_when_done = true;
            }
            match &self.scan {
                Some(scan) => {
                    ui.spinner();
                    ui.weak(format!(
                        "{} {} / {}",
                        tr(lang, "Reading headers"),
                        scan.done,
                        scan.total
                    ));
                }
                None => {
                    let text = match (&self.source, self.items.len()) {
                        (None, _) => tr(
                            lang,
                            "Open a folder to see where its astronomy images point.",
                        )
                        .to_owned(),
                        (Some(_), 0) => tr(lang, "No images with a sky position.").to_owned(),
                        (Some(_), n) => tr1(lang, "{} images with a sky position", &n.to_string()),
                    };
                    ui.weak(text);
                }
            }
        });
    }

    fn chart(&mut self, ui: &egui::Ui, inputs: &Inputs<'_>) -> Vec<Command> {
        let rect = ui.max_rect();
        let response = ui.interact(rect, ui.id().with("sky"), Sense::click_and_drag());
        let view = self.view.get_or_insert_with(View::default);
        if self.fit_when_done && self.scan.is_none() {
            let points: Vec<Radec> = self
                .items
                .iter()
                .flat_map(|i| std::iter::once(i.sky.center).chain(i.sky.outline.iter().copied()))
                .collect();
            if !points.is_empty() {
                view.fit(rect, &points);
            }
            self.fit_when_done = false;
        }
        navigate(ui, &response, rect, view);
        let view = *view;

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, SKY_BACKGROUND);
        let background = self.background.get_or_insert_with(Background::bundled);
        draw_background(&painter, background, &view, rect, inputs.lang);

        let active = inputs.active.map(crate::frames::file_of);
        let shapes: Vec<Shape> = self
            .items
            .iter()
            .map(|item| Shape::of(item, &view, rect))
            .collect();
        let hover = response.hover_pos();
        let hits: Vec<usize> = hover.map(|p| hits_at(&shapes, p)).unwrap_or_default();
        for (i, (item, shape)) in self.items.iter().zip(&shapes).enumerate() {
            let chosen = match &self.selected {
                Some(Selected::One(s)) => *s == i,
                Some(Selected::Many(v)) => v.contains(&i),
                None => false,
            };
            let emphasized = chosen || active.as_deref() == Some(item.path.as_path());
            let color = if item.sky.certain {
                inputs.settings.certain()
            } else {
                inputs.settings.approximate()
            };
            draw_shape(
                &painter,
                shape,
                color,
                item,
                emphasized,
                hits.first() == Some(&i),
                rect,
            );
        }
        if let Some(&first) = hits.first() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            let tip = match hits.as_slice() {
                [_] => file_name(&self.items[first].path),
                many => tr1(inputs.lang, "{} images", &many.len().to_string()),
            };
            response.clone().on_hover_text_at_pointer(tip);
        }
        if response.clicked() {
            self.selected = match hits.as_slice() {
                [] => None,
                [one] => Some(Selected::One(*one)),
                many => Some(Selected::Many(many.to_vec())),
            };
        }
        if let Some(at) = hover {
            draw_cursor_position(&painter, rect, view.to_sky(rect, at));
        }
        self.popup(ui, inputs, rect, &shapes).into_iter().collect()
    }

    /// 選んだ枠の横に、縮小画像・名前・撮影日時と「開く」を出す。
    fn popup(
        &mut self,
        ui: &egui::Ui,
        inputs: &Inputs<'_>,
        rect: Rect,
        shapes: &[Shape],
    ) -> Option<Command> {
        let lang = inputs.lang;
        let selected = self.selected.clone()?;
        let first = match &selected {
            Selected::One(i) => *i,
            Selected::Many(v) => *v.first()?,
        };
        let anchor = shapes.get(first)?.anchor()?;
        let mut command = None;
        let mut choose = None;
        egui::Area::new(ui.id().with("sky_popup"))
            .fixed_pos(anchor + vec2(12.0, -12.0))
            .order(egui::Order::Foreground)
            .constrain_to(rect)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| match &selected {
                    Selected::One(i) => {
                        let item = self.items[*i].clone();
                        command = self.card(ui, inputs, &item);
                    }
                    Selected::Many(members) => {
                        ui.label(tr1(lang, "{} images", &members.len().to_string()));
                        egui::ScrollArea::vertical()
                            .max_height(240.0)
                            .show(ui, |ui| {
                                for &i in members {
                                    if ui
                                        .selectable_label(false, file_name(&self.items[i].path))
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

    fn card(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>, item: &Item) -> Option<Command> {
        let lang = inputs.lang;
        self.thumb
            .show(ui, lang, inputs.registry, &item.path, 180.0);
        ui.strong(file_name(&item.path));
        if let Some(object) = &item.object {
            ui.label(object);
        }
        if let Some(taken) = &item.taken {
            ui.weak(taken);
        }
        ui.weak(sky_chart::radec_text(item.sky.center));
        let how = match (item.sky.certain, item.sky.outline.is_empty()) {
            (true, _) => "Range from the WCS in the header",
            (false, false) => "Range estimated from the mount position and focal length",
            (false, true) => "Only the mount position is known",
        };
        ui.weak(tr(lang, how));
        ui.button(tr(lang, "Open"))
            .clicked()
            .then(|| Command::Open(item.path.clone()))
    }
}

/// 1 枚の枠を画面に置いたもの。
enum Shape {
    /// 縁の点（全部が画面の表側にある）。
    Outline(Vec<Pos2>),
    /// 範囲が分からないか、縁の一部が裏側に回ったもの。中心だけ。
    Center(Pos2),
    /// 中心も裏側。
    Hidden,
}

impl Shape {
    fn of(item: &Item, view: &View, rect: Rect) -> Self {
        let outline: Option<Vec<Pos2>> = item
            .sky
            .outline
            .iter()
            .map(|p| view.to_screen(rect, *p))
            .collect();
        match (outline, view.to_screen(rect, item.sky.center)) {
            (Some(points), _) if points.len() >= 3 => Shape::Outline(points),
            (_, Some(center)) => Shape::Center(center),
            _ => Shape::Hidden,
        }
    }

    fn anchor(&self) -> Option<Pos2> {
        match self {
            Shape::Outline(points) => Some(bounds(points).right_top()),
            Shape::Center(at) => Some(*at),
            Shape::Hidden => None,
        }
    }

    fn contains(&self, p: Pos2) -> bool {
        match self {
            Shape::Outline(points) => {
                // 引いて見て点ほどに縮んだ枠も、十字と同じ広さでクリックできるようにする。
                let b = bounds(points);
                let tiny = b.width().max(b.height()) < 2.0 * CROSS_ARM;
                inside(points, p) || (tiny && b.center().distance(p) <= CROSS_ARM)
            }
            Shape::Center(at) => at.distance(p) <= CROSS_ARM + 2.0,
            Shape::Hidden => false,
        }
    }

    /// 重なったときに小さいほうを先にするための大きさ。
    fn area(&self) -> f32 {
        match self {
            Shape::Outline(points) => {
                let b = bounds(points);
                b.width() * b.height()
            }
            _ => 0.0,
        }
    }
}

/// `p` にかかる枠の番号。小さい枠から順に（大きな写野の中の小さな写野を選べるように）。
fn hits_at(shapes: &[Shape], p: Pos2) -> Vec<usize> {
    let mut hits: Vec<usize> = (0..shapes.len())
        .filter(|&i| shapes[i].contains(p))
        .collect();
    hits.sort_by(|&a, &b| shapes[a].area().total_cmp(&shapes[b].area()));
    hits
}

fn bounds(points: &[Pos2]) -> Rect {
    Rect::from_points(points)
}

/// 点が多角形の中か（偶奇規則）。
fn inside(points: &[Pos2], p: Pos2) -> bool {
    let mut odd = false;
    let n = points.len();
    for i in 0..n {
        let (a, b) = (points[i], points[(i + n - 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            odd = !odd;
        }
    }
    odd
}

fn draw_shape(
    painter: &egui::Painter,
    shape: &Shape,
    color: Color32,
    item: &Item,
    emphasized: bool,
    hovered: bool,
    rect: Rect,
) {
    let width = if emphasized {
        3.0
    } else if hovered {
        2.0
    } else {
        1.5
    };
    match shape {
        Shape::Outline(points) => {
            marks::outline(painter, points, true, color, width, !item.sky.certain);
            let b = bounds(points);
            if emphasized || hovered || b.width().max(b.height()) >= TAGGED_SIDE {
                marks::name_tag(painter, b, rect, &file_name(&item.path), color);
            }
        }
        Shape::Center(at) => {
            let arm = if emphasized {
                CROSS_ARM + 3.0
            } else {
                CROSS_ARM
            };
            marks::cross(painter, *at, color, arm);
            if emphasized || hovered {
                let around = Rect::from_center_size(*at, vec2(2.0 * arm, 2.0 * arm));
                marks::name_tag(painter, around, rect, &file_name(&item.path), color);
            }
        }
        Shape::Hidden => {}
    }
}

/// ドラッグで移動、ホイールとピンチでカーソルの所を中心に拡大縮小、ダブルクリックで 2 倍。
fn navigate(ui: &egui::Ui, response: &egui::Response, rect: Rect, view: &mut View) {
    if response.dragged_by(egui::PointerButton::Primary) {
        view.pan(rect, response.drag_delta());
    }
    if let Some(at) = response.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = (f64::from(scroll) / 200.0).exp2() * f64::from(pinch);
        if factor != 1.0 {
            view.zoom_at(rect, at, factor);
        }
        if response.double_clicked() {
            view.zoom_at(rect, at, 2.0);
        }
    }
    view.settle(rect);
}

/// 赤経・赤緯の目盛り、星座の境界と線、星、星座の名前。
fn draw_background(
    painter: &egui::Painter,
    b: &Background,
    view: &View,
    rect: Rect,
    lang: Language,
) {
    let field = view.field_degrees(rect);
    draw_grid(painter, view, rect, field);
    let border = Stroke::new(1.0, BORDER);
    for line in &b.borders {
        let points = densify(line, 1.0);
        for run in screen_runs(&points, view, rect) {
            painter.extend(egui::Shape::dashed_line(&run, border, 3.0, 3.0));
        }
    }
    let figure = Stroke::new(1.0, FIGURE);
    for line in &b.lines {
        for run in screen_runs(&densify(line, 2.0), view, rect) {
            painter.add(egui::Shape::line(run, figure));
        }
    }
    // 広く見ているときは暗い星を省く（点が多すぎて星座の形が分からなくなるため）。
    let faintest = if field > 90.0 { 5.0 } else { 7.0 };
    for star in b.stars.iter().take_while(|s| s.magnitude <= faintest) {
        let Some(at) = view.to_screen(rect, star.at) else {
            continue;
        };
        if !rect.expand(4.0).contains(at) {
            continue;
        }
        let [r, g, bl] = sky_chart::star_color(star.bv);
        painter.circle_filled(
            at,
            sky_chart::star_radius(star.magnitude),
            Color32::from_rgb(r, g, bl),
        );
    }
    if field < 150.0 {
        let font = egui::FontId::proportional(12.0);
        for name in &b.names {
            let Some(at) = view.to_screen(rect, name.at) else {
                continue;
            };
            if rect.contains(at) {
                let text = match lang {
                    Language::Japanese if !name.japanese.is_empty() => &name.japanese,
                    _ => &name.latin,
                };
                painter.text(at, egui::Align2::CENTER_CENTER, text, font.clone(), NAME);
            }
        }
    }
}

/// 赤経（経線）と赤緯（緯線）の目盛り。見えている範囲だけを描く。
fn draw_grid(painter: &egui::Painter, view: &View, rect: Rect, field: f64) {
    let step = sky_chart::grid_step(field);
    let stroke = Stroke::new(1.0, GRID);
    let radius = [
        rect.left_top(),
        rect.right_top(),
        rect.left_bottom(),
        rect.right_bottom(),
    ]
    .into_iter()
    .map(|c| sky_chart::angle(view.center, view.to_sky(rect, c)))
    .fold(0.0, f64::max)
    .min(180.0);
    let dec_lo = (view.center[1] - radius).max(-90.0);
    let dec_hi = (view.center[1] + radius).min(90.0);
    let near_pole = dec_lo <= -89.0 || dec_hi >= 89.0 || radius > 60.0;
    let (ra_lo, ra_hi) = if near_pole {
        (0.0, 360.0)
    } else {
        let widest = dec_lo.abs().max(dec_hi.abs()).to_radians().cos();
        let half = (radius / widest).min(180.0);
        (view.center[0] - half, view.center[0] + half)
    };
    // 極の近くでは経線が集まって塗りつぶしたようになるので、間隔を広げる。
    let ra_step = if near_pole { step.max(15.0) } else { step };
    const SAMPLES: usize = 48;
    let mut ra = (ra_lo / ra_step).floor() * ra_step;
    while ra <= ra_hi {
        let line: Vec<Radec> = (0..=SAMPLES)
            .map(|k| [ra, dec_lo + (dec_hi - dec_lo) * k as f64 / SAMPLES as f64])
            .collect();
        for run in screen_runs(&line, view, rect) {
            painter.add(egui::Shape::line(run, stroke));
        }
        ra += ra_step;
    }
    let mut dec = (dec_lo / step).ceil() * step;
    while dec <= dec_hi {
        if dec.abs() < 90.0 {
            let line: Vec<Radec> = (0..=SAMPLES)
                .map(|k| [ra_lo + (ra_hi - ra_lo) * k as f64 / SAMPLES as f64, dec])
                .collect();
            for run in screen_runs(&line, view, rect) {
                painter.add(egui::Shape::line(run, stroke));
            }
        }
        dec += step;
    }
}

/// 折れ線の点の間を、`max_degrees` より離れないように埋める。星座の境界は赤緯の線に沿って
/// 長く伸びるものがあり、端だけを結ぶと平射図法で曲がるはずの所がまっすぐになるため。
fn densify(line: &[Radec], max_degrees: f64) -> Vec<Radec> {
    let mut out = Vec::with_capacity(line.len());
    for pair in line.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let mut dra = b[0] - a[0];
        if dra > 180.0 {
            dra -= 360.0;
        } else if dra < -180.0 {
            dra += 360.0;
        }
        let steps = ((dra.abs().max((b[1] - a[1]).abs()) / max_degrees).ceil() as usize).max(1);
        for k in 0..steps {
            let t = k as f64 / steps as f64;
            out.push([a[0] + dra * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    out.extend(line.last().copied());
    out
}

/// 折れ線を画面の点にして、裏側に回った所や、画面の外で大きく飛ぶ所で切る。
fn screen_runs(line: &[Radec], view: &View, rect: Rect) -> Vec<Vec<Pos2>> {
    let far = rect.width().max(rect.height()) * 3.0;
    let mut runs = Vec::new();
    let mut run: Vec<Pos2> = Vec::new();
    for p in line {
        match view.to_screen(rect, *p) {
            Some(at) if run.last().is_none_or(|last| last.distance(at) < far) => run.push(at),
            other => {
                if run.len() >= 2 {
                    runs.push(std::mem::take(&mut run));
                }
                run.clear();
                run.extend(other);
            }
        }
    }
    if run.len() >= 2 {
        runs.push(run);
    }
    runs.retain(|r| bounds(r).intersects(rect));
    runs
}

/// 左下に、カーソルの所の赤経・赤緯。
fn draw_cursor_position(painter: &egui::Painter, rect: Rect, at: Radec) {
    let text = sky_chart::radec_text(at);
    let galley = painter.layout_no_wrap(text, egui::FontId::monospace(12.0), NAME);
    let corner = pos2(rect.left() + 6.0, rect.bottom() - galley.size().y - 4.0);
    painter.galley(corner, galley, NAME);
}

fn file_name(path: &Path) -> String {
    let file = crate::frames::file_of(path);
    file.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// FITS の拡張子か。星図はヘッダしか読まないので、ほかの形式は開かずに飛ばす。
fn is_fits_path(path: &Path) -> bool {
    let name = path.to_string_lossy().to_lowercase();
    ["fits", "fit", "fts", "fz", "fits.gz", "fit.gz", "fts.gz"]
        .iter()
        .any(|ext| name.ends_with(&format!(".{ext}")))
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
                let images: Vec<PathBuf> = crate::tree::list_folder(&dir, &registry, order)
                    .images
                    .into_iter()
                    .filter(|p| is_fits_path(p))
                    .collect();
                send(Found::Total(images.len()));
                for path in images {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    match vw_fits::header_of_file(&path) {
                        Ok(Some(header)) => {
                            let item =
                                Item::from_header(path, &|k| header.get(k).map(str::to_owned));
                            if let Some(item) = item {
                                send(Found::Item(item));
                            }
                        }
                        Ok(None) => {}
                        Err(e) => send(Found::Problem(format!("{}: {e}", path.display()))),
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
                    for item in catalog_items(&entry.dir, &entry.catalog) {
                        send(Found::Item(item));
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

/// 検索カタログに入っている FITS のヘッダから、写った範囲の分かる画像。ZIP / 7z の中の画像は、
/// そこから直接開けないので出さない。
pub fn catalog_items(dir: &Path, catalog: &crate::catalog::Catalog) -> Vec<Item> {
    let primary = vw_fits::GROUP.to_lowercase();
    let image_hdu = format!("{primary} hdu ");
    catalog
        .files
        .iter()
        .filter(|f| !f.path.contains(crate::catalog::IN_ARCHIVE))
        .filter_map(|f| {
            let triples: Vec<&(String, String, String)> = f
                .meta
                .iter()
                .filter_map(|&i| catalog.meta.get(i as usize))
                .collect();
            // 画像の HDU のカードを先に、Primary を後に引く（ファイルから読むときと同じ）。
            let lookup = |key: &str| {
                let find = |kind: &dyn Fn(&str) -> bool| {
                    triples
                        .iter()
                        .find(|(k, name, _)| kind(k) && name == key)
                        .map(|(_, _, v)| v.clone())
                };
                find(&|k| k.starts_with(&image_hdu)).or_else(|| find(&|k| *k == primary))
            };
            Item::from_header(dir.join(&f.path), &lookup)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, FileEntry};

    fn triple(kind: &str, key: &str, value: &str) -> (String, String, String) {
        (kind.into(), key.into(), value.into())
    }

    fn entry(path: &str, meta: Vec<u32>) -> FileEntry {
        FileEntry {
            path: path.into(),
            size: 0,
            modified: String::new(),
            meta,
        }
    }

    #[test]
    fn catalog_headers_become_items_with_image_hdu_cards_first() {
        let catalog = Catalog {
            meta: vec![
                triple("fits", "NAXIS1", "0"),
                triple("fits", "RA", "10.0"),
                triple("fits", "DEC", "20.0"),
                triple("fits", "OBJECT", "M31"),
                triple("fits hdu 1", "NAXIS1", "100"),
                triple("fits hdu 1", "NAXIS2", "100"),
                triple("exif", "RA", "99"),
            ],
            files: vec![
                entry("a.fits", vec![0, 1, 2, 3, 4, 5]),
                entry("no-position.fits", vec![4, 5]),
                entry("in.zip!/b.fits", vec![1, 2, 4, 5]),
            ],
            ..Default::default()
        };
        let items = catalog_items(Path::new("/data"), &catalog);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, Path::new("/data/a.fits"));
        assert_eq!(items[0].sky.center, [10.0, 20.0]);
        assert_eq!(items[0].object.as_deref(), Some("M31"));
    }

    #[test]
    fn only_fits_extensions_are_read() {
        for name in ["a.fits", "b.FIT", "c.fts", "d.fz", "e.fits.gz"] {
            assert!(is_fits_path(Path::new(name)), "{name}");
        }
        for name in ["a.png", "b.gz", "c.fitsx"] {
            assert!(!is_fits_path(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn clicks_inside_an_outline_hit_it_and_smaller_frames_come_first() {
        let square = |half: f32| {
            Shape::Outline(vec![
                pos2(-half, -half),
                pos2(half, -half),
                pos2(half, half),
                pos2(-half, half),
            ])
        };
        let shapes = [
            square(100.0),
            square(10.0),
            Shape::Center(pos2(50.0, 50.0)),
            Shape::Hidden,
        ];
        assert_eq!(hits_at(&shapes, pos2(0.0, 0.0)), vec![1, 0]);
        assert_eq!(hits_at(&shapes, pos2(52.0, 49.0)), vec![2, 0]);
        assert_eq!(hits_at(&shapes, pos2(500.0, 0.0)), Vec::<usize>::new());
    }

    #[test]
    fn long_border_segments_are_filled_in() {
        let line = densify(&[[350.0, 10.0], [10.0, 10.0]], 2.0);
        assert_eq!(line.len(), 11);
        assert!(line.iter().all(|p| p[1] == 10.0));
        assert_eq!(line[5], [360.0, 10.0]);
    }
}
