//! 貼り合わせのウインドウ（ツール →「貼り合わせ...」）。選んだ画像か、フォルダの画像を全部、
//! 重なりで合わせて 1 枚にする。
//!
//! 左に画像の一覧（同じ所を写した画像は 1 行にまとめ、使う 1 枚を選ぶ）、真ん中にプレビュー、
//! 下につなぎ目の診断を出す。合わなかった画像は一覧からプレビューへドラッグして大まかに置くと、
//! そこから細かく合わせる。結果はスロットに出すか、場所を選んで保存する。
//!
//! 重い処理（読み込み、重なりの探索、つなぎ目の診断、プレビュー）は裏のスレッドで行う。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use egui_phosphor::regular as icon;
use vw_compare::Affine;
use vw_compare::mosaic::{self, Canvas, Grid, Layer, Link, Piece, Seam, Search, Verdict};
use vw_compare::plane::Rgb;
use vw_core::{Curve, DisplayParams, PixelData, Registry, VwImage};

use crate::i18n::{tr, tr1};
use crate::settings::{Language, StitchBlend, StitchSettings};
use crate::sort::SortOrder;
use crate::stitch::{self, Depth, How, Plan, Source};

/// 位置合わせに使う縮めた画像の長辺。細かく直す段の上限（2048）より小さくして、何十枚でも
/// メモリと時間を食いすぎないようにする。合わせた写し方は元の大きさに戻して使うので、
/// 1 画素より細かいずれまで求まる。
const ALIGN_SIDE: usize = 1024;
/// プレビューと一覧に使う小さな絵の長辺。
const THUMB_SIDE: usize = 512;
/// プレビューの画素の数の上限。
const PREVIEW_PIXELS: usize = 2_000_000;
/// 同時に読む枚数（[`crate::subframes_window`] と同じ理由）。
const WORKERS: usize = 4;
/// WCS で置いた組を、重なりとみなす面積の割合。
const WCS_OVERLAP: f64 = 0.02;

/// 何を貼り合わせるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// 一覧で選んだ画像。
    Chosen,
    /// 開いているフォルダの画像を全部（開くたびに自動で合わせる）。
    Folder,
}

/// ウインドウを描くのに要るもの。
pub struct Inputs<'a> {
    pub lang: Language,
    pub folder: Option<&'a Path>,
    pub registry: &'a Registry,
    pub order: SortOrder,
    pub settings: &'a StitchSettings,
    /// スロットに出ている画像。
    pub shown: &'a [PathBuf],
}

/// アプリへの頼みごと。
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// 貼り合わせた画像をスロットに出す。
    Show(PathBuf),
    /// 元の画像を開く。
    Open(PathBuf),
}

/// 1 枚の画質。写真は鮮鋭度、天体は星の太さで比べる。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Quality {
    pub sharpness: Option<f64>,
    /// 星の FWHM（画素）。小さいほどくっきり。
    pub fwhm: Option<f32>,
}

impl Quality {
    /// `self` のほうがよいか。比べられないときは `false`。
    pub fn better_than(&self, other: &Quality) -> bool {
        match (self.fwhm, other.fwhm) {
            (Some(a), Some(b)) => a < b,
            _ => match (self.sharpness, other.sharpness) {
                (Some(a), Some(b)) => a > b,
                (Some(_), None) => true,
                _ => false,
            },
        }
    }

    fn text(&self, lang: Language) -> String {
        match (self.fwhm, self.sharpness) {
            (Some(f), _) => format!("FWHM {f:.2}"),
            (None, Some(s)) => format!("{} {}", tr(lang, "Sharpness"), significant(s)),
            _ => "-".into(),
        }
    }
}

/// 1 枚の下ごしらえ。
pub struct Prepared {
    pub path: PathBuf,
    pub thumb: Rgb,
    pub quality: Quality,
    /// WCS のカードと、画面の画素（0 始まり）から FITS の画素（1 始まり）への写し方。
    pub wcs: Option<stitch::Wcs>,
    pub color: bool,
    pub depth: Depth,
    pub window: Option<(f32, f32)>,
    pub curve: Curve,
    pub fits: bool,
    /// 計測の値の明るさ（`Piece` と同じ大きさ）。写真は `None`。
    pub measured: Option<vw_compare::plane::Plane>,
}

/// 下ごしらえと重なりの探索の結果。
struct Found {
    images: Vec<Prepared>,
    pieces: Vec<Piece>,
    /// つなぎ目の明るさと鮮鋭度を測る絵。写真は `pieces` と同じ。計測の値（16bit・実数）は、
    /// 画像ごとに伸ばし方の違う表示ではなく、値そのもの（伸ばし方が違うと、同じ星空でも
    /// 明るさが何割も違って見えるため）。
    measured: Vec<Piece>,
    links: Vec<Link>,
    /// WCS で置いた組（`links` の番号）。
    wcs_links: HashSet<usize>,
}

impl Found {
    fn new(ready: Vec<(Prepared, Piece)>, links: Vec<Link>, wcs_links: HashSet<usize>) -> Self {
        let (mut images, pieces): (Vec<Prepared>, Vec<Piece>) = ready.into_iter().unzip();
        let measured = images
            .iter_mut()
            .zip(&pieces)
            .map(|(image, piece)| match image.measured.take() {
                Some(plane) => Piece {
                    plane,
                    full: piece.full,
                },
                None => piece.clone(),
            })
            .collect();
        Self {
            images,
            pieces,
            measured,
            links,
            wcs_links,
        }
    }
}

enum Message {
    Total(usize),
    Ready(usize, Box<(Prepared, Piece)>),
    Problem(String),
    Linking(usize),
    Done(Vec<Link>, HashSet<usize>),
}

struct Job {
    rx: Receiver<Message>,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
    linking: Option<usize>,
    ready: Vec<Option<(Prepared, Piece)>>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// 並べた結果（どれを使い、どこに置き、つなぎ目がどうか）。
pub struct Arranged {
    /// 画像ごとの置き場所と、置くのにたどった組。
    pub tree: Vec<Option<(Affine, Option<usize>)>>,
    pub groups: Vec<Vec<usize>>,
    pub used: Vec<bool>,
    pub seams: Vec<Seam>,
    pub gains: Vec<f64>,
    preview: Option<(egui::ColorImage, Grid)>,
}

/// 並べ直す条件。変わったら並べ直す。
#[derive(Debug, Clone, PartialEq)]
struct Arrangement {
    links: usize,
    choices: Vec<(usize, usize)>,
    blend: StitchBlend,
    match_brightness: bool,
    same_area_percent: f32,
}

/// 一覧からプレビューへドラッグする画像。
#[derive(Debug, Clone, Copy)]
struct Dragged(usize);

pub struct StitchWindow {
    pub open: bool,
    target: Target,
    folder: Option<PathBuf>,
    /// フォルダの画像。
    listed: Vec<PathBuf>,
    /// 一覧で選んだ画像（`Target::Chosen`）。
    chosen: HashSet<PathBuf>,
    job: Option<Job>,
    found: Option<Arc<Found>>,
    /// 手で置いて足した組と、手で置いた画像。
    extra_links: Vec<Link>,
    by_hand: HashSet<usize>,
    /// 同じ所のまとまり（先頭の画像の番号）ごとに使う画像。
    choices: HashMap<usize, usize>,
    arranging: Option<(Arrangement, Receiver<Arranged>)>,
    arranged: Option<(Arrangement, Arranged)>,
    texture: Option<egui::TextureHandle>,
    picker: Option<Box<vw_tree::picker::Picker>>,
    saving: Option<Receiver<Result<String, String>>>,
    /// 星図に出す、最後にスロットに出した貼り合わせの枠。
    sky: Vec<crate::sky_window::Item>,
    problems: Vec<String>,
    notes: Vec<String>,
}

impl Default for StitchWindow {
    fn default() -> Self {
        Self {
            open: false,
            target: Target::Chosen,
            folder: None,
            listed: Vec::new(),
            chosen: HashSet::new(),
            job: None,
            found: None,
            extra_links: Vec::new(),
            by_hand: HashSet::new(),
            choices: HashMap::new(),
            arranging: None,
            arranged: None,
            texture: None,
            picker: None,
            saving: None,
            sky: Vec::new(),
            problems: Vec::new(),
            notes: Vec::new(),
        }
    }
}

impl StitchWindow {
    /// ログに赤で出すもの。
    pub fn take_problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    /// ログに出すもの。
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// 星図に出す枠。
    pub fn sky_items(&self) -> &[crate::sky_window::Item] {
        &self.sky
    }

    /// 閉じている間は読み込みを止め、覚えた絵を手放す。星図の枠は残す。
    pub fn forget_while_closed(&mut self) {
        if !self.open && (self.found.is_some() || self.job.is_some()) {
            let sky = std::mem::take(&mut self.sky);
            let target = self.target;
            *self = Self {
                sky,
                target,
                ..Self::default()
            };
        }
    }

    /// 1 フレーム分、ウインドウの中を描く。
    pub fn ui(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) -> Vec<Command> {
        let ctx = ui.ctx().clone();
        self.follow_folder(inputs);
        self.receive_job();
        self.receive_arranged(&ctx);
        self.receive_saved();
        self.arrange_if_needed(&ctx, inputs.settings);
        let mut commands = Vec::new();
        egui::Panel::top("stitch_bar").show(ui, |ui| self.bar(ui, inputs, &mut commands));
        egui::Panel::left("stitch_list")
            .resizable(true)
            .default_size(320.0)
            .show(ui, |ui| self.list(ui, inputs, &mut commands));
        egui::Panel::bottom("stitch_seams")
            .resizable(true)
            .default_size(170.0)
            .show(ui, |ui| self.seam_table(ui, inputs.lang));
        let rest = ui.available_rect_before_wrap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
            self.preview(ui, inputs.lang);
        });
        self.save_picker(ui, inputs);
        commands
    }

    fn follow_folder(&mut self, inputs: &Inputs<'_>) {
        let folder = inputs.folder.map(Path::to_path_buf);
        if folder == self.folder {
            return;
        }
        self.listed = folder
            .as_ref()
            .map(|f| crate::tree::list_folder(f, inputs.registry, inputs.order).images)
            .unwrap_or_default();
        self.folder = folder;
        if self.target == Target::Folder {
            self.start(inputs, self.listed.clone());
        }
    }

    /// 選んだ画像（一覧の順）。
    fn targets(&self) -> Vec<PathBuf> {
        match self.target {
            Target::Folder => self.listed.clone(),
            Target::Chosen => {
                let mut out: Vec<PathBuf> = self
                    .listed
                    .iter()
                    .filter(|p| self.chosen.contains(*p))
                    .cloned()
                    .collect();
                // ほかのフォルダから選んだ画像は後ろに。
                let mut rest: Vec<PathBuf> = self
                    .chosen
                    .iter()
                    .filter(|p| !self.listed.contains(p))
                    .cloned()
                    .collect();
                rest.sort();
                out.extend(rest);
                out
            }
        }
    }

    fn start(&mut self, inputs: &Inputs<'_>, paths: Vec<PathBuf>) {
        self.found = None;
        self.extra_links.clear();
        self.by_hand.clear();
        self.choices.clear();
        self.arranging = None;
        self.arranged = None;
        self.texture = None;
        self.job = (paths.len() >= 2).then(|| start_job(paths, inputs.registry.clone()));
    }

    fn receive_job(&mut self) {
        let Some(job) = &mut self.job else {
            return;
        };
        loop {
            match job.rx.try_recv() {
                Ok(Message::Total(n)) => {
                    job.total = n;
                    job.ready = (0..n).map(|_| None).collect();
                }
                Ok(Message::Ready(i, p)) => {
                    if let Some(slot) = job.ready.get_mut(i) {
                        *slot = Some(*p);
                    }
                }
                Ok(Message::Problem(e)) => self.problems.push(e),
                Ok(Message::Linking(steps)) => job.linking = Some(steps),
                Ok(Message::Done(links, wcs_links)) => {
                    let ready = std::mem::take(&mut job.ready)
                        .into_iter()
                        .flatten()
                        .collect();
                    self.found = Some(Arc::new(Found::new(ready, links, wcs_links)));
                    self.job = None;
                    return;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.job = None;
                    return;
                }
            }
        }
    }

    fn all_links(&self) -> Vec<Link> {
        let mut links = self
            .found
            .as_ref()
            .map(|f| f.links.clone())
            .unwrap_or_default();
        links.extend(self.extra_links.iter().copied());
        links
    }

    fn wanted(&self, settings: &StitchSettings) -> Arrangement {
        let mut choices: Vec<(usize, usize)> = self.choices.iter().map(|(k, v)| (*k, *v)).collect();
        choices.sort_unstable();
        Arrangement {
            links: self.all_links().len(),
            choices,
            blend: settings.blend,
            match_brightness: settings.match_brightness,
            same_area_percent: settings.same_area_percent,
        }
    }

    fn arrange_if_needed(&mut self, ctx: &egui::Context, settings: &StitchSettings) {
        let Some(found) = self.found.clone() else {
            return;
        };
        let wanted = self.wanted(settings);
        let current = self
            .arranging
            .as_ref()
            .map(|(a, _)| a)
            .or(self.arranged.as_ref().map(|(a, _)| a));
        if current == Some(&wanted) {
            return;
        }
        let (tx, rx) = channel();
        let links = self.all_links();
        let choices = self.choices.clone();
        let settings = settings.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let arranged = arrange(&found, &links, &choices, &settings);
            let _ = tx.send(arranged);
            ctx.request_repaint();
        });
        self.arranging = Some((wanted, rx));
    }

    fn receive_arranged(&mut self, ctx: &egui::Context) {
        let Some((_, rx)) = &self.arranging else {
            return;
        };
        match rx.try_recv() {
            Ok(mut arranged) => {
                self.texture = arranged.preview.as_ref().map(|(img, _)| {
                    ctx.load_texture("stitch-preview", img.clone(), Default::default())
                });
                if let Some((img, _)) = &mut arranged.preview {
                    // テクスチャに送ったので、画素は手放す（格子は枠を描くのに残す）。
                    img.pixels = Vec::new();
                }
                // 初めて並べたときは、同じ所のまとまりごとに画質のいちばんよい画像を選んでおく。
                if let Some(found) = &self.found {
                    for group in &arranged.groups {
                        self.choices
                            .entry(group[0])
                            .or_insert_with(|| best_of(&found.images, group));
                    }
                }
                let (wanted, _) = self.arranging.take().expect("arranging");
                self.arranged = Some((wanted, arranged));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.arranging = None,
        }
    }

    fn receive_saved(&mut self) {
        let Some(rx) = &self.saving else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(note)) => {
                self.notes.push(note);
                self.saving = None;
            }
            Ok(Err(e)) => {
                self.problems.push(e);
                self.saving = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.saving = None,
        }
    }

    fn bar(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>, commands: &mut Vec<Command>) {
        let lang = inputs.lang;
        ui.horizontal_wrapped(|ui| {
            let before = self.target;
            ui.selectable_value(&mut self.target, Target::Chosen, tr(lang, "Chosen images"));
            ui.selectable_value(
                &mut self.target,
                Target::Folder,
                tr(lang, "All images in the folder"),
            );
            if self.target != before && self.target == Target::Folder {
                self.start(inputs, self.listed.clone());
            }
            ui.separator();
            if self.target == Target::Chosen {
                if ui
                    .button(tr(lang, "Choose the images in the slots"))
                    .on_hover_text(tr(lang, "Choose every image that a slot shows now."))
                    .clicked()
                {
                    self.chosen.extend(
                        inputs
                            .shown
                            .iter()
                            .filter(|p| {
                                !stitch::is_stitched(p) && crate::compose::split(p).is_none()
                            })
                            .cloned(),
                    );
                }
                if ui.button(tr(lang, "Clear the choice")).clicked() {
                    self.chosen.clear();
                }
            }
            let targets = self.targets();
            let align = ui
                .add_enabled(
                    targets.len() >= 2 && self.job.is_none(),
                    egui::Button::new(format!("{} {}", icon::PUZZLE_PIECE, tr(lang, "Align"))),
                )
                .on_hover_text(tr(lang, "Find the overlaps and place the images."))
                .on_disabled_hover_text(tr(lang, "Choose two or more images."));
            if align.clicked() {
                self.start(inputs, targets);
            }
            ui.separator();
            let ready = self.plan(inputs.settings).is_some();
            if ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Show in a slot")))
                .clicked()
                && let Some(plan) = self.plan(inputs.settings)
                && let Some(path) = stitch::remember(plan.clone())
            {
                self.sky = stitch::sky_items(&plan, &path);
                commands.push(Command::Show(path));
            }
            if ui
                .add_enabled(
                    ready && self.saving.is_none(),
                    egui::Button::new(tr(lang, "Save as...")),
                )
                .clicked()
            {
                self.open_picker(lang);
            }
            self.status(ui, lang);
        });
    }

    fn status(&self, ui: &mut egui::Ui, lang: Language) {
        if let Some(job) = &self.job {
            ui.spinner();
            let done = job.progress.load(Ordering::Relaxed);
            let text = match job.linking {
                Some(steps) => format!("{} {done} / {steps}", tr(lang, "Finding overlaps")),
                None => format!("{} {done} / {}", tr(lang, "Reading"), job.total),
            };
            ui.weak(text);
        } else if self.arranging.is_some() {
            ui.spinner();
            ui.weak(tr(lang, "Placing"));
        } else if self.saving.is_some() {
            ui.spinner();
            ui.weak(tr(lang, "Saving"));
        } else if let (Some(found), Some((_, a))) = (&self.found, &self.arranged) {
            let placed = (0..found.images.len())
                .filter(|&i| a.used[i] && a.tree[i].is_some())
                .count();
            let used = a.used.iter().filter(|u| **u).count();
            ui.weak(format!(
                "{} / {}",
                tr1(lang, "{} placed", &placed.to_string()),
                tr1(lang, "{} images", &used.to_string())
            ));
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>, commands: &mut Vec<Command>) {
        let lang = inputs.lang;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| match (&self.found, &self.arranged) {
                (Some(found), Some((_, arranged))) => {
                    let found = found.clone();
                    let view = arranged_view(arranged, &found.wcs_links, &self.by_hand);
                    self.arranged_list(ui, lang, &found, view, commands);
                }
                _ => self.choice_list(ui, inputs),
            });
    }

    /// 合わせる前の一覧。選ぶときは印を付ける。
    fn choice_list(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) {
        let lang = inputs.lang;
        if self.listed.is_empty() {
            ui.weak(tr(lang, "Open a folder of images to stitch."));
            return;
        }
        if self.target == Target::Chosen {
            ui.weak(tr(lang, "Tick the images to stitch, then press Align."));
        }
        let extra: Vec<PathBuf> = self
            .chosen
            .iter()
            .filter(|p| !self.listed.contains(p))
            .cloned()
            .collect();
        for path in self.listed.iter().chain(&extra) {
            let name = crate::compare::file_name(path);
            match self.target {
                Target::Folder => {
                    ui.label(name);
                }
                Target::Chosen => {
                    let mut on = self.chosen.contains(path);
                    if ui.checkbox(&mut on, name).changed() {
                        if on {
                            self.chosen.insert(path.clone());
                        } else {
                            self.chosen.remove(path);
                        }
                    }
                }
            }
        }
    }

    /// 合わせたあとの一覧。同じ所の画像は 1 行にまとめ、合わなかった画像はドラッグできる。
    fn arranged_list(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        found: &Found,
        view: ArrangedView,
        commands: &mut Vec<Command>,
    ) {
        let error = ui.visuals().error_fg_color;
        let mut grouped: HashSet<usize> = HashSet::new();
        for g in &view.groups {
            grouped.extend(g.iter().copied());
        }
        let mut unplaced = Vec::new();
        egui::Grid::new("stitch_images")
            .striped(true)
            .num_columns(3)
            .spacing(vec2(10.0, 4.0))
            .show(ui, |ui| {
                for i in 0..found.images.len() {
                    if grouped.contains(&i) && !view.groups.iter().any(|g| g[0] == i) {
                        continue;
                    }
                    let group = view.groups.iter().find(|g| g[0] == i);
                    let shown = match group {
                        Some(g) => self.choices.get(&g[0]).copied().unwrap_or(g[0]),
                        None => i,
                    };
                    if view.how[shown].is_none() {
                        unplaced.push(shown);
                        continue;
                    }
                    let image = &found.images[shown];
                    match group {
                        Some(g) => {
                            let mut pick = shown;
                            egui::ComboBox::from_id_salt(("stitch_group", g[0]))
                                .selected_text(crate::compare::file_name(&image.path))
                                .show_ui(ui, |ui| {
                                    for &m in g {
                                        let text = format!(
                                            "{}  {}",
                                            crate::compare::file_name(&found.images[m].path),
                                            found.images[m].quality.text(lang)
                                        );
                                        ui.selectable_value(&mut pick, m, text);
                                    }
                                })
                                .response
                                .on_hover_text(tr1(
                                    lang,
                                    "{} images of the same area: choose the one to use",
                                    &g.len().to_string(),
                                ));
                            if pick != shown {
                                self.choices.insert(g[0], pick);
                            }
                        }
                        None => {
                            if ui
                                .add(
                                    egui::Button::new(crate::compare::file_name(&image.path))
                                        .frame(false),
                                )
                                .on_hover_text(tr(lang, "Open this image"))
                                .clicked()
                            {
                                commands.push(Command::Open(image.path.clone()));
                            }
                        }
                    }
                    ui.weak(image.quality.text(lang));
                    ui.weak(view.how[shown].map_or("", |h| tr(lang, h.title())));
                    ui.end_row();
                }
            });
        if unplaced.is_empty() {
            return;
        }
        ui.separator();
        ui.colored_label(error, tr(lang, "Not placed"));
        ui.weak(tr(
            lang,
            "Drag an image onto the preview where it roughly belongs. It is matched from there, or left where you put it.",
        ));
        for i in unplaced {
            let id = ui.id().with(("stitch_drag", i));
            ui.dnd_drag_source(id, Dragged(i), |ui| {
                ui.label(format!(
                    "{} {}",
                    icon::DOTS_SIX_VERTICAL,
                    crate::compare::file_name(&found.images[i].path)
                ));
            });
        }
    }

    fn seam_table(&self, ui: &mut egui::Ui, lang: Language) {
        let (Some(found), Some((_, arranged))) = (&self.found, &self.arranged) else {
            ui.weak(tr(
                lang,
                "The seams are checked after the images are placed.",
            ));
            return;
        };
        let mut seams = arranged.seams.clone();
        seams.sort_by_key(|s| std::cmp::Reverse(s.verdict()));
        if seams.is_empty() {
            ui.weak(tr(lang, "No overlapping images."));
            return;
        }
        let name = |i: usize| crate::compare::file_name(&found.images[i].path);
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("stitch_seams_table")
                .striped(true)
                .spacing(vec2(14.0, 2.0))
                .show(ui, |ui| {
                    for title in [
                        "Seam",
                        "Overlap",
                        "Correlation",
                        "Offset (px)",
                        "Brightness difference",
                        "Sharpness ratio",
                        "Verdict",
                    ] {
                        ui.strong(tr(lang, title));
                    }
                    ui.end_row();
                    for s in &seams {
                        ui.label(format!("{} – {}", name(s.a), name(s.b)));
                        ui.label(format!("{:.0} %", s.shared * 100.0));
                        ui.label(format!("{:.3}", s.correlation));
                        ui.label(s.mismatch.map_or("-".into(), |m| format!("{m:.2}")));
                        ui.label(format!("{:.1} %", s.brightness * 100.0));
                        ui.label(format!("{:.2}", s.sharpness));
                        let (text, color) = verdict_look(s.verdict(), ui.visuals());
                        ui.colored_label(color, tr(lang, text));
                        ui.end_row();
                    }
                });
        });
    }

    fn preview(&mut self, ui: &mut egui::Ui, lang: Language) {
        let area = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(area, Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, ui.visuals().extreme_bg_color);
        let (Some(found), Some((_, arranged)), Some(texture)) =
            (self.found.clone(), &self.arranged, &self.texture)
        else {
            let text = if self.job.is_some() || self.arranging.is_some() {
                tr(lang, "Working...")
            } else {
                tr(lang, "Choose images and press Align.")
            };
            painter.text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(14.0),
                ui.visuals().weak_text_color(),
            );
            return;
        };
        let Some((_, grid)) = &arranged.preview else {
            return;
        };
        let grid = *grid;
        let size = vec2(grid.width as f32, grid.height as f32);
        let fit = (area.width() / size.x).min(area.height() / size.y).max(0.0);
        let rect = Rect::from_center_size(area.center(), size * fit);
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let Some(to_preview) = grid.to_mosaic.inverse() else {
            return;
        };
        let to_screen = |p: [f64; 2]| -> Pos2 {
            let [x, y] = to_preview.apply(p);
            pos2(
                rect.min.x + (x as f32 + 0.5) * fit,
                rect.min.y + (y as f32 + 0.5) * fit,
            )
        };
        let hover = response.hover_pos();
        let accent = ui.visuals().selection.stroke.color;
        let mut tip: Option<String> = None;
        for (i, image) in found.images.iter().enumerate() {
            let (Some((placed, via)), true) = (arranged.tree[i], arranged.used[i]) else {
                continue;
            };
            let outline: Vec<Pos2> = mosaic::corners(found.pieces[i].full)
                .iter()
                .map(|c| to_screen(placed.apply(*c)))
                .collect();
            let inside = hover.is_some_and(|p| inside_polygon(p, &outline));
            let stroke = Stroke::new(if inside { 2.0 } else { 1.0 }, accent);
            let mut closed = outline.clone();
            closed.push(outline[0]);
            painter.add(egui::Shape::line(closed, stroke));
            let name = crate::compare::file_name(&image.path);
            painter.text(
                outline[0] + vec2(4.0, 2.0),
                egui::Align2::LEFT_TOP,
                &name,
                egui::FontId::proportional(11.0),
                accent,
            );
            if inside {
                let how = how_of(i, via, &found.wcs_links, &self.by_hand);
                let partners: Vec<String> = arranged
                    .seams
                    .iter()
                    .filter(|s| s.a == i || s.b == i)
                    .map(|s| {
                        let other = if s.a == i { s.b } else { s.a };
                        format!(
                            "{}  r {:.2}",
                            crate::compare::file_name(&found.images[other].path),
                            s.correlation
                        )
                    })
                    .collect();
                let mut text = format!("{name}\n{}", tr(lang, how.title()));
                if !partners.is_empty() {
                    text.push_str(&format!(
                        "\n{}\n{}",
                        tr(lang, "Overlaps"),
                        partners.join("\n")
                    ));
                }
                tip = Some(text);
            }
        }
        if let Some(tip) = tip {
            response.clone().on_hover_text_at_pointer(tip);
        }
        // 一覧から落とした画像を、落とした所から合わせる。
        if let (Some(dropped), Some(at)) = (
            response.dnd_release_payload::<Dragged>(),
            ui.ctx().pointer_interact_pos(),
        ) {
            let x = f64::from((at.x - rect.min.x) / fit - 0.5);
            let y = f64::from((at.y - rect.min.y) / fit - 0.5);
            let center = grid.to_mosaic.apply([x, y]);
            self.place_by_hand(&found, dropped.0, center);
        }
    }

    /// 画像 `i` を、中心が貼り合わせの座標 `center` に来るように置き、重なる画像と合わせる。
    /// 合わなければ置いた所のままにする。
    fn place_by_hand(&mut self, found: &Found, i: usize, center: [f64; 2]) {
        let Some((_, arranged)) = &self.arranged else {
            return;
        };
        let full = found.pieces[i].full;
        let dropped = Affine {
            c: center[0] - (full[0] as f64 - 1.0) / 2.0,
            f: center[1] - (full[1] as f64 - 1.0) / 2.0,
            ..Affine::IDENTITY
        };
        let mut best: Option<Link> = None;
        let mut nearest: Option<(f64, usize, Affine)> = None;
        for (j, p) in arranged.tree.iter().enumerate() {
            let (Some((pj, _)), true) = (p, arranged.used[j]) else {
                continue;
            };
            if j == i {
                continue;
            }
            let Some(to_j) = pj.inverse() else {
                continue;
            };
            let start = dropped.then(&to_j);
            let share = mosaic::shared(full, &dropped, found.pieces[j].full, pj);
            if nearest.is_none_or(|(s, ..)| share > s) {
                nearest = Some((share, j, start));
            }
            if share < mosaic::MIN_OVERLAP {
                continue;
            }
            if let Some(l) = mosaic::link_from(&found.pieces, i, j, start)
                && best.is_none_or(|b| l.correlation > b.correlation)
            {
                best = Some(l);
            }
        }
        let link = match (best, nearest) {
            (Some(l), _) => l,
            // 合わなかったら、いちばん重なる画像との組を、確かさ 0 で足す（ほかの組より後にたどる）。
            (None, Some((_, j, start))) => Link {
                a: i,
                b: j,
                map: start,
                correlation: 0.0,
                overlap: 0.0,
            },
            (None, None) => return,
        };
        let index = found.links.len() + self.extra_links.len();
        self.extra_links.push(link);
        self.by_hand.insert(index);
    }

    /// 今の並べ方から、スロットに出す・保存する置き方。2 枚以上置けていなければ `None`。
    fn plan(&self, settings: &StitchSettings) -> Option<Plan> {
        let found = self.found.as_ref()?;
        let (_, arranged) = self.arranged.as_ref()?;
        let links = self.all_links();
        plan_of(found, &links, arranged, &self.by_hand, settings)
    }

    fn open_picker(&mut self, lang: Language) {
        let Some(found) = &self.found else {
            return;
        };
        let fits = found.images.iter().all(|i| i.fits);
        let first = found.images.first().map(|i| i.path.clone());
        let name = format!(
            "{}{}",
            chrono::Local::now().format("stitched-%Y%m%d-%H%M%S."),
            if fits { "fits" } else { "png" }
        );
        let picker = vw_tree::picker::Picker::new(
            tr(lang, "Save the stitched image"),
            vw_tree::picker::Mode::SaveFile {
                extensions: ["png", "tif", "tiff", "fits", "fit"]
                    .map(String::from)
                    .to_vec(),
                name,
            },
            first.as_deref().and_then(Path::parent),
            crate::settings_ui::picker_labels(lang),
        );
        self.picker = Some(Box::new(picker));
    }

    fn save_picker(&mut self, ui: &egui::Ui, inputs: &Inputs<'_>) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        match picker.show(ui.ctx()) {
            vw_tree::picker::Outcome::Open => {}
            vw_tree::picker::Outcome::Cancelled => self.picker = None,
            vw_tree::picker::Outcome::Chosen(path) => {
                self.picker = None;
                let Some(plan) = self.plan(inputs.settings) else {
                    return;
                };
                let (tx, rx) = channel();
                let registry = inputs.registry.clone();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let _ = tx.send(save(&plan, &registry, &path));
                    ctx.request_repaint();
                });
                self.saving = Some(rx);
            }
        }
    }
}

/// 並べた結果のうち、一覧を描くのに使うもの（借用を切るために写す）。
struct ArrangedView {
    groups: Vec<Vec<usize>>,
    /// 画像ごとのどう置いたか（置けていなければ `None`）。
    how: Vec<Option<How>>,
}

fn arranged_view(a: &Arranged, wcs: &HashSet<usize>, by_hand: &HashSet<usize>) -> ArrangedView {
    ArrangedView {
        groups: a.groups.clone(),
        how: a
            .tree
            .iter()
            .enumerate()
            .map(|(i, p)| p.map(|(_, via)| how_of(i, via, wcs, by_hand)))
            .collect(),
    }
}

/// 有効数字 3 桁（鮮鋭度は画像によって桁が大きく違うため、小数点以下の桁を決めない）。
fn significant(v: f64) -> String {
    if v <= 0.0 || !v.is_finite() {
        return format!("{v}");
    }
    let digits = (2 - v.log10().floor() as i32).max(0) as usize;
    format!("{v:.digits$}")
}

fn how_of(i: usize, via: Option<usize>, wcs: &HashSet<usize>, by_hand: &HashSet<usize>) -> How {
    let _ = i;
    match via {
        None => How::Reference,
        Some(l) if wcs.contains(&l) => How::Wcs,
        Some(l) if by_hand.contains(&l) => How::ByHand,
        Some(_) => How::Matched,
    }
}

fn verdict_look(v: Verdict, visuals: &egui::Visuals) -> (&'static str, Color32) {
    match v {
        Verdict::Clean => ("Clean", Color32::from_rgb(80, 180, 100)),
        Verdict::Visible => ("Seam may show", visuals.warn_fg_color),
        Verdict::Misaligned => ("Misaligned", visuals.error_fg_color),
    }
}

fn inside_polygon(p: Pos2, poly: &[Pos2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

/// まとまり `group` のうち、画質のいちばんよい画像。
pub fn best_of(images: &[Prepared], group: &[usize]) -> usize {
    let mut best = group[0];
    for &i in &group[1..] {
        if images[i].quality.better_than(&images[best].quality) {
            best = i;
        }
    }
    best
}

/// 元の画像の型から、貼り合わせた画像の型を決める。実数が混じるか、8bit と 16bit が混じれば実数
/// （値の目盛りが違うものを同じ整数の入れ物に入れると、どちらかの値が変わってしまうため）。
pub fn depth_of(depths: &[Depth]) -> Depth {
    if depths.iter().all(|d| *d == Depth::U8) {
        return Depth::U8;
    }
    let bits: Option<Vec<u32>> = depths
        .iter()
        .map(|d| match d {
            Depth::U16(b) => Some(*b),
            _ => None,
        })
        .collect();
    match bits {
        Some(b) => Depth::U16(b.into_iter().max().unwrap_or(16)),
        None => Depth::F32,
    }
}

fn plan_of(
    found: &Found,
    links: &[Link],
    arranged: &Arranged,
    by_hand: &HashSet<usize>,
    settings: &StitchSettings,
) -> Option<Plan> {
    let mut sources = Vec::new();
    let mut depths = Vec::new();
    let mut color = false;
    let mut reference: Option<&Prepared> = None;
    for (i, image) in found.images.iter().enumerate() {
        let (Some((placed, via)), true) = (arranged.tree[i], arranged.used[i]) else {
            continue;
        };
        let how = how_of(i, via, &found.wcs_links, by_hand);
        if how == How::Reference {
            reference = Some(image);
        }
        let correlation = match how {
            How::Matched | How::ByHand => via.and_then(|l| links.get(l)).map(|l| l.correlation),
            _ => None,
        };
        depths.push(image.depth);
        color |= image.color;
        sources.push(Source {
            path: image.path.clone(),
            size: found.pieces[i].full,
            placed,
            how,
            correlation,
            gain: arranged.gains[i],
            wcs: image.wcs.clone(),
        });
    }
    if sources.len() < 2 {
        return None;
    }
    let reference = reference.or(found.images.first())?;
    Some(Plan {
        sources,
        blend: settings.blend.blend(),
        matched_brightness: arranged.gains.iter().any(|g| (g - 1.0).abs() > 1e-9),
        max_pixels: settings.max_pixels(),
        color,
        depth: depth_of(&depths),
        window: reference.window,
        curve: reference.curve,
    })
}

/// 並べる。使う画像を決め、置き、つなぎ目を調べ、明るさの比とプレビューを作る。
fn arrange(
    found: &Found,
    links: &[Link],
    choices: &HashMap<usize, usize>,
    settings: &StitchSettings,
) -> Arranged {
    let n = found.images.len();
    let tree = mosaic::tree(n, links);
    let placed: Vec<Option<Affine>> = tree.iter().map(|p| p.map(|(t, _)| t)).collect();
    let sizes: Vec<[usize; 2]> = found.pieces.iter().map(|p| p.full).collect();
    let groups = mosaic::same_area(
        &sizes,
        &placed,
        f64::from(settings.same_area_percent) / 100.0,
    );
    let mut used = vec![true; n];
    for g in &groups {
        let pick = choices
            .get(&g[0])
            .copied()
            .filter(|c| g.contains(c))
            .unwrap_or_else(|| best_of(&found.images, g));
        for &m in g {
            used[m] = m == pick;
        }
    }
    let photos = found.images.iter().all(|i| i.depth == Depth::U8);
    let gains = if settings.match_brightness && photos {
        mosaic::gains(&found.pieces, &placed, &used)
    } else {
        vec![1.0; n]
    };
    let seams = mosaic::seams(&found.measured, &placed, links, &used, &gains);
    let preview = preview_image(found, &placed, &used, &gains, settings.blend.blend());
    Arranged {
        tree,
        groups,
        used,
        seams,
        gains,
        preview,
    }
}

fn preview_image(
    found: &Found,
    placed: &[Option<Affine>],
    used: &[bool],
    gains: &[f64],
    blend: mosaic::Blend,
) -> Option<(egui::ColorImage, Grid)> {
    let sizes: Vec<[usize; 2]> = found.pieces.iter().map(|p| p.full).collect();
    let shown: Vec<Option<Affine>> = placed
        .iter()
        .zip(used)
        .map(|(p, u)| p.filter(|_| *u))
        .collect();
    let grid = Grid::covering(mosaic::bounds(&sizes, &shown)?, PREVIEW_PIXELS)?;
    let mut canvas = Canvas::new(grid, 3, blend);
    for (i, image) in found.images.iter().enumerate() {
        let Some(p) = shown[i] else {
            continue;
        };
        let t = &image.thumb;
        let k = sizes[i][0] as f64 / t.width as f64;
        let from_thumb = Affine {
            a: k,
            b: 0.0,
            c: 0.5 * k - 0.5,
            d: 0.0,
            e: k,
            f: 0.5 * k - 0.5,
        };
        let data: Vec<f32> = t.data.iter().flatten().copied().collect();
        canvas.add(&Layer {
            width: t.width,
            height: t.height,
            channels: 3,
            data: &data,
            placed: from_thumb.then(&p),
            gain: gains[i] as f32,
        });
    }
    let values = canvas.finish();
    let pixels: Vec<Color32> = values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|px| {
            if px[0].is_finite() {
                let [r, g, b] = px.map(|v| v.round().clamp(0.0, 255.0) as u8);
                Color32::from_rgb(r, g, b)
            } else {
                Color32::TRANSPARENT
            }
        })
        .collect();
    Some((
        egui::ColorImage::new([grid.width, grid.height], pixels),
        grid,
    ))
}

/// 重なりを探すのに要る、1 枚ぶんのもの（番号、縮めた絵、WCS、FITS か）。
type Linkable = (usize, Piece, Option<stitch::Wcs>, bool);

fn start_job(paths: Vec<PathBuf>, registry: Registry) -> Job {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(AtomicUsize::new(0));
    let (stop, counter) = (cancel.clone(), progress.clone());
    std::thread::spawn(move || {
        let _ = tx.send(Message::Total(paths.len()));
        let next = AtomicUsize::new(0);
        let ready: std::sync::Mutex<Vec<Option<Linkable>>> =
            std::sync::Mutex::new((0..paths.len()).map(|_| None).collect());
        std::thread::scope(|s| {
            for _ in 0..WORKERS {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= paths.len() || stop.load(Ordering::Relaxed) {
                            return;
                        }
                        match prepare(&paths[i], &registry) {
                            Ok((prepared, piece)) => {
                                let fits = prepared.fits;
                                let wcs = prepared.wcs.clone();
                                if let Ok(mut r) = ready.lock() {
                                    r[i] = Some((i, piece.clone(), wcs, fits));
                                }
                                let _ = tx.send(Message::Ready(i, Box::new((prepared, piece))));
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Message::Problem(format!("{}: {e}", paths[i].display())));
                            }
                        }
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
        });
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let ready: Vec<_> = ready
            .into_inner()
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect();
        let pieces: Vec<Piece> = ready.iter().map(|r| r.1.clone()).collect();
        counter.store(0, Ordering::Relaxed);
        let _ = tx.send(Message::Linking(mosaic::link_steps(pieces.len())));
        let search = Search {
            quarter_turns: !ready.is_empty() && ready.iter().all(|r| r.3),
        };
        let mut links = mosaic::link_all(&pieces, search, &|| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let wcs: Vec<Option<stitch::Wcs>> = ready.iter().map(|r| r.2.clone()).collect();
        let wcs_links = add_wcs_links(&pieces, &wcs, &mut links);
        let _ = tx.send(Message::Done(links, wcs_links));
    });
    Job {
        rx,
        cancel,
        progress,
        total: 0,
        linking: None,
        ready: Vec::new(),
    }
}

/// どちらにも WCS のある組を、絵を合わせた組の代わりに WCS で置く（星の少ない所でも確か）。
/// 足した組の番号を返す。
fn add_wcs_links(
    pieces: &[Piece],
    wcs: &[Option<stitch::Wcs>],
    links: &mut Vec<Link>,
) -> HashSet<usize> {
    let mut out = HashSet::new();
    for a in 0..pieces.len() {
        for b in a + 1..pieces.len() {
            let (Some((ca, fa)), Some((cb, fb))) = (&wcs[a], &wcs[b]) else {
                continue;
            };
            let Some(m) = vw_fits::pixel_map(&stitch::lookup(ca), &stitch::lookup(cb)) else {
                continue;
            };
            let fits_map = Affine {
                a: m[0][0],
                b: m[0][1],
                c: m[0][2],
                d: m[1][0],
                e: m[1][1],
                f: m[1][2],
            };
            let Some(back) = fb.inverse() else {
                continue;
            };
            let map = fa.then(&fits_map).then(&back);
            let Some(b_in_a) = map.inverse() else {
                continue;
            };
            let share = mosaic::shared(pieces[a].full, &Affine::IDENTITY, pieces[b].full, &b_in_a);
            if share < WCS_OVERLAP {
                continue;
            }
            links.retain(|l| !(l.a == a && l.b == b));
            links.push(Link {
                a,
                b,
                map,
                correlation: 1.0,
                overlap: share,
            });
        }
    }
    // retain で番号がずれるので、最後にまとめて数える。
    for (i, l) in links.iter().enumerate() {
        if l.correlation == 1.0 && wcs[l.a].is_some() && wcs[l.b].is_some() {
            out.insert(i);
        }
    }
    out
}

/// WCS を読むのに要るカード。
const WCS_KEYS: [&str; 22] = [
    "NAXIS1", "NAXIS2", "ZNAXIS1", "ZNAXIS2", "CTYPE1", "CTYPE2", "CRPIX1", "CRPIX2", "CRVAL1",
    "CRVAL2", "CD1_1", "CD1_2", "CD2_1", "CD2_2", "CDELT1", "CDELT2", "CROTA2", "PC1_1", "PC1_2",
    "PC2_1", "PC2_2", "LONPOLE",
];

fn prepare(path: &Path, registry: &Registry) -> Result<(Prepared, Piece), String> {
    let (file, frame) = crate::frames::split(path).unwrap_or((path.to_path_buf(), 0));
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    let (image, plugin) = registry
        .decode_frame_bytes(&file, &bytes, frame)
        .map_err(|e| e.to_string())?;
    drop(bytes);
    let fits = plugin == "FITS";
    let (w, h) = (image.width as usize, image.height as usize);
    let display = vw_core::display::to_display(
        &image,
        &DisplayParams {
            window: None,
            max_side: image.width.max(image.height),
        },
    );
    let rgb = Rgb::from_rgba8(
        display.width as usize,
        display.height as usize,
        &display.rgba,
    );
    drop(display);
    let fit = |side: usize| {
        let (fw, fh, _) = vw_compare::plane::fit_long_side(w, h, side);
        if (fw, fh) == (rgb.width, rgb.height) {
            rgb.clone()
        } else {
            rgb.resized(fw, fh)
        }
    };
    let align = fit(ALIGN_SIDE);
    let thumb = fit(THUMB_SIDE);
    drop(rgb);
    let sharpness = vw_compare::pairs::sharpness(align.width, align.height, &align.to_rgba8());
    let linear = (fits || !matches!(image.data, PixelData::U8(_)))
        .then(|| vw_pipeline::pixel::linear_of_display(&image).0);
    let fwhm = linear
        .as_ref()
        .filter(|_| fits)
        .and_then(|l| vw_pipeline::subframes::evaluate(l).fwhm);
    let measured = linear.map(|l| {
        let values = (0..l.width * l.height)
            .map(|i| match l.channels {
                3 => vw_compare::plane::luma([l.data[i * 3], l.data[i * 3 + 1], l.data[i * 3 + 2]]),
                _ => l.data[i],
            })
            .collect();
        vw_compare::plane::Plane::new(l.width, l.height, values).resized(align.width, align.height)
    });
    let wcs = wcs_of(&image);
    let depth = match &image.data {
        PixelData::U8(_) => Depth::U8,
        PixelData::U16(_) => Depth::U16(image.significant_bits),
        PixelData::F32(_) => Depth::F32,
    };
    let prepared = Prepared {
        path: path.to_path_buf(),
        thumb,
        quality: Quality { sharpness, fwhm },
        wcs,
        color: matches!(
            image.color,
            vw_core::ColorModel::Rgb | vw_core::ColorModel::Rgba
        ),
        depth,
        window: image.window,
        curve: image.curve,
        fits,
        measured,
    };
    let piece = Piece {
        plane: align.luma(),
        full: [w, h],
    };
    Ok((prepared, piece))
}

/// FITS の WCS のカードと、画面の画素（0 始まり）から FITS の画素（1 始まり、y は下から上。
/// 行を返して出した画像は逆）への写し方。
fn wcs_of(image: &VwImage) -> Option<stitch::Wcs> {
    let lookup = crate::compose::lookup(&image.metadata);
    let cards: Vec<(String, String)> = WCS_KEYS
        .iter()
        .chain(&["RADESYS", "EQUINOX"])
        .filter_map(|k| Some(((*k).to_owned(), lookup(k)?)))
        .collect();
    vw_fits::sky(&stitch::lookup(&cards)).filter(|s| s.certain)?;
    let to_fits = if crate::compose::flipped(&image.metadata) {
        Affine {
            a: 1.0,
            b: 0.0,
            c: 1.0,
            d: 0.0,
            e: -1.0,
            f: f64::from(image.height),
        }
    } else {
        Affine {
            c: 1.0,
            f: 1.0,
            ..Affine::IDENTITY
        }
    };
    Some((cards, to_fits))
}

/// 貼り合わせて `path` に保存する。形式は拡張子で決める。同じ名前のファイルがあれば書かない。
fn save(plan: &Plan, registry: &Registry, path: &Path) -> Result<String, String> {
    let (image, _) = stitch::render(plan, registry)?;
    let grid = plan.grid().ok_or("nothing to stitch")?;
    let lines: Vec<String> =
        std::iter::once(format!("Stitched by vw from {} images", plan.sources.len()))
            .chain(plan.sources.iter().map(|s| {
                format!(
                    "{}: {}",
                    crate::compare::file_name(&s.path),
                    stitch::placement_text(s, &grid)
                )
            }))
            .collect();
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let bytes = match extension.as_str() {
        "fits" | "fit" => {
            let cards = plan.wcs(&grid).map(|(c, _)| c).unwrap_or_default();
            vw_fits::encode(&image, &cards, &lines)?
        }
        "tif" | "tiff" => vw_tiff::encode(&image, &lines.join("\n"))?,
        _ => vw_png::encode(&image, &lines.join("\n"))?,
    };
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path);
    let mut file = match file {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(format!(
                "{}: already exists, not overwritten",
                path.display()
            ));
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    std::io::Write::write_all(&mut file, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!(
        "Stitching: saved {} ({} x {})",
        path.display(),
        image.width,
        image.height
    ))
}

#[cfg(test)]
mod tests;
