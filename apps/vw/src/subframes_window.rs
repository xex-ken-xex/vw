//! サブフレームの欄。今のフォルダの画像（一晩に撮った露光の 1 枚ずつ）を評価して表にし、
//! しきい値から外れた画像に印を付ける。星で位置を合わせた縮小画像を順に切り替えて
//! （blink）、雲・飛行機・人工衛星の跡や流れた星を目で確かめられるようにする。
//!
//! 評価は元の画素の値で行い、縮小画像は見るためだけに使う。縮小画像で星を測らないのは、
//! 星が 1〜2 画素につぶれて FWHM が測れなくなるため。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use eframe::egui::{self, vec2, Color32, Sense};
use egui_phosphor::regular as icon;
use vw_core::Registry;
use vw_pipeline::nodes::Float;
use vw_pipeline::stretch_curves::AutoStretch;
use vw_pipeline::subframes::{self, Evaluation, Limits, Map, Reason};

use crate::i18n::{tr, tr1};
use crate::settings::Language;
use crate::sort::SortOrder;

/// 縮小画像の長い辺の画素数。blink で画面いっぱいに出しても粗く見えず、
/// 数百枚を覚えておいてもメモリを食いすぎない大きさ。
const PREVIEW_SIDE: usize = 1024;
/// 同時に読む枚数。FITS の読み込みと星探しはどちらも 1 枚を 1 つのスレッドで行うので、
/// 数枚を並べて読むとほぼ枚数分だけ速くなる。それより多くしても、ディスクの読み込みで詰まる。
const WORKERS: usize = 4;

/// 見るための縮小画像（8bit のグレー）。
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub width: usize,
    pub height: usize,
    /// 元の画像の 1 画素がこの画像の何画素か（1 以下）。
    pub scale: f64,
    pub pixels: Vec<u8>,
}

/// 評価した 1 枚。
pub struct Row {
    pub path: PathBuf,
    /// フォルダの中の順番。並べ替えを戻すときと blink の順に使う。
    order: usize,
    pub evaluation: Evaluation,
    preview: Preview,
}

enum Found {
    Total(usize),
    Row(Box<Row>),
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

/// 表の並べ替えに使う列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Name,
    Fwhm,
    Eccentricity,
    Stars,
    Background,
    Noise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Table,
    Blink,
}

/// アプリへの頼みごと。
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Open(PathBuf),
    FlagBad(Vec<PathBuf>),
}

/// 欄を描くのに要るもの。
pub struct Inputs<'a> {
    pub lang: Language,
    /// 今開いているフォルダ。
    pub folder: Option<&'a Path>,
    pub registry: &'a Registry,
    pub order: SortOrder,
    pub limits: Limits,
    /// blink の 1 秒あたりの枚数。
    pub fps: f32,
}

/// 基準の画像と、ほかの画像を基準に合わせる写し方。
struct Alignment {
    /// 基準（`rows` の添字）。
    reference: usize,
    /// `rows` ごとの、基準の座標からその画像の座標への写し方。合わなかった画像は `None`。
    maps: Vec<Option<Map>>,
}

struct Blink {
    playing: bool,
    /// 出している画像（`rows` の添字）。
    frame: usize,
    /// 前に切り替えた時刻（egui の時計の秒）。
    last_step: f64,
    skip_rejected: bool,
    textures: HashMap<usize, egui::TextureHandle>,
}

pub struct SubframesWindow {
    pub open: bool,
    folder: Option<PathBuf>,
    scan: Option<Scan>,
    /// フォルダの順に並べた評価。
    rows: Vec<Row>,
    reasons: Vec<Vec<Reason>>,
    /// `reasons` を決めたときのしきい値。設定が変わったら決め直す。
    reasons_for: Option<(Limits, usize)>,
    sort: (Column, bool),
    tab: Tab,
    alignment: Option<Alignment>,
    blink: Blink,
    problems: Vec<String>,
}

impl Default for SubframesWindow {
    fn default() -> Self {
        Self {
            open: false,
            folder: None,
            scan: None,
            rows: Vec::new(),
            reasons: Vec::new(),
            reasons_for: None,
            sort: (Column::Name, true),
            tab: Tab::Table,
            alignment: None,
            blink: Blink {
                playing: false,
                frame: 0,
                last_step: 0.0,
                skip_rejected: true,
                textures: HashMap::new(),
            },
            problems: Vec::new(),
        }
    }
}

impl SubframesWindow {
    /// ログに出す、読めなかったもの。
    pub fn take_problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    /// 閉じている間はファイルを読みに行かず、覚えた縮小画像も手放す。
    pub fn forget_while_closed(&mut self) {
        if !self.open && self.folder.is_some() {
            *self = Self::default();
        }
    }

    /// 1 フレーム分、欄の中を描く。
    pub fn ui(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) -> Vec<Command> {
        let ctx = ui.ctx().clone();
        self.follow_folder(&ctx, inputs);
        self.receive_scan();
        self.update_reasons(inputs.limits);
        self.update_alignment();
        let mut commands = Vec::new();
        egui::Panel::top("subframes_bar").show(ui, |ui| self.bar(ui, inputs, &mut commands));
        let rest = ui.available_rect_before_wrap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| match self.tab {
            Tab::Table => self.table(ui, inputs.lang, &mut commands),
            Tab::Blink => self.blink(ui, inputs),
        });
        commands
    }

    fn follow_folder(&mut self, ctx: &egui::Context, inputs: &Inputs<'_>) {
        let folder = inputs.folder.map(Path::to_path_buf);
        if folder == self.folder {
            return;
        }
        let tab = self.tab;
        *self = Self {
            open: self.open,
            tab,
            sort: self.sort,
            ..Self::default()
        };
        self.scan = folder
            .as_ref()
            .map(|f| start_scan(f.clone(), inputs.registry.clone(), inputs.order, ctx));
        self.folder = folder;
    }

    fn receive_scan(&mut self) {
        let Some(scan) = &mut self.scan else {
            return;
        };
        loop {
            match scan.rx.try_recv() {
                Ok(Found::Total(n)) => scan.total = n,
                Ok(Found::Row(row)) => {
                    let at = self.rows.partition_point(|r| r.order < row.order);
                    self.rows.insert(at, *row);
                }
                Ok(Found::Step) => scan.done += 1,
                Ok(Found::Problem(e)) => self.problems.push(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        self.scan = None;
    }

    fn update_reasons(&mut self, limits: Limits) {
        let key = (limits, self.rows.len());
        if self.reasons_for == Some(key) {
            return;
        }
        let evaluations: Vec<Evaluation> = self.rows.iter().map(|r| r.evaluation.clone()).collect();
        self.reasons = subframes::reasons(&evaluations, limits);
        self.reasons_for = Some(key);
    }

    /// 読み終えたら、いちばん星の細い画像を基準にして位置を合わせる。読む途中で合わせないのは、
    /// 基準が後から来た画像に替わると、作った blink の画像を全部作り直すことになるため。
    fn update_alignment(&mut self) {
        if self.scan.is_some() || self.alignment.is_some() || self.rows.is_empty() {
            return;
        }
        let Some(reference) = reference_of(&self.rows) else {
            self.alignment = Some(Alignment {
                reference: 0,
                maps: vec![None; self.rows.len()],
            });
            return;
        };
        let stars = &self.rows[reference].evaluation.bright;
        let maps = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                if i == reference {
                    Some(IDENTITY)
                } else {
                    subframes::align(stars, &r.evaluation.bright)
                }
            })
            .collect();
        self.alignment = Some(Alignment { reference, maps });
        self.blink.frame = reference;
    }

    fn rejected(&self) -> Vec<PathBuf> {
        self.rows
            .iter()
            .zip(&self.reasons)
            .filter(|(_, r)| !r.is_empty())
            .map(|(row, _)| row.path.clone())
            .collect()
    }

    fn bar(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>, commands: &mut Vec<Command>) {
        let lang = inputs.lang;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Table, tr(lang, "Evaluation"));
            ui.selectable_value(&mut self.tab, Tab::Blink, tr(lang, "Blink"));
            ui.separator();
            let reload = ui
                .add_enabled(self.folder.is_some(), egui::Button::new(icon::ARROW_CLOCKWISE))
                .on_hover_text(tr(lang, "Evaluate again"));
            if reload.clicked() {
                self.folder = None;
            }
            let rejected = self.rejected();
            let flag = ui
                .add_enabled(
                    self.scan.is_none() && !rejected.is_empty(),
                    egui::Button::new(tr(lang, "Flag rejected as bad")),
                )
                .on_hover_text(tr(
                    lang,
                    "Put the bad flag on every image outside the limits, so that a filter or an export can leave them out.",
                ));
            if flag.clicked() {
                commands.push(Command::FlagBad(rejected.clone()));
            }
            match &self.scan {
                Some(scan) => {
                    ui.spinner();
                    ui.weak(format!(
                        "{} {} / {}",
                        tr(lang, "Evaluating"),
                        scan.done,
                        scan.total
                    ));
                }
                None if self.folder.is_none() => {
                    ui.weak(tr(lang, "Open a folder of subframes to evaluate them."));
                }
                None if self.rows.is_empty() => {
                    ui.weak(tr(lang, "No images to evaluate in this folder."));
                }
                None => {
                    ui.weak(format!(
                        "{}, {}",
                        tr1(lang, "{} images", &self.rows.len().to_string()),
                        tr1(lang, "{} rejected", &rejected.len().to_string()),
                    ));
                }
            }
        });
    }

    fn table(&mut self, ui: &mut egui::Ui, lang: Language, commands: &mut Vec<Command>) {
        let medians = Medians::of(&self.rows);
        let order = sorted(&self.rows, self.sort);
        let error = ui.visuals().error_fg_color;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("subframes_table")
                .striped(true)
                .spacing(vec2(16.0, 2.0))
                .show(ui, |ui| {
                    for (column, title) in [
                        (Column::Name, "File"),
                        (Column::Fwhm, "FWHM (px)"),
                        (Column::Eccentricity, "Eccentricity"),
                        (Column::Stars, "Stars"),
                        (Column::Background, "Background"),
                        (Column::Noise, "Noise"),
                    ] {
                        let mark = match self.sort {
                            (c, true) if c == column => " ⏶",
                            (c, false) if c == column => " ⏷",
                            _ => "",
                        };
                        let text = format!("{}{mark}", tr(lang, title));
                        if ui.add(egui::Button::new(text).frame(false)).clicked() {
                            self.sort = if self.sort.0 == column {
                                (column, !self.sort.1)
                            } else {
                                (column, true)
                            };
                        }
                    }
                    ui.label(tr(lang, "Rejected because"));
                    ui.end_row();

                    ui.weak(tr(lang, "Median"));
                    for v in [
                        medians.fwhm.map(|v| format!("{v:.2}")),
                        medians.eccentricity.map(|v| format!("{v:.2}")),
                        medians.stars.map(|v| format!("{v:.0}")),
                        medians.background.map(number),
                        medians.noise.map(number),
                    ] {
                        ui.weak(v.unwrap_or_default());
                    }
                    ui.label("");
                    ui.end_row();

                    for i in order {
                        let row = &self.rows[i];
                        let reasons = &self.reasons[i];
                        let tint = |text: String| {
                            let t = egui::RichText::new(text);
                            if reasons.is_empty() {
                                t
                            } else {
                                t.color(error)
                            }
                        };
                        let name = crate::compare::file_name(&row.path);
                        let selected = self.blink.frame == i;
                        let r = ui.add(egui::Button::selectable(selected, tint(name)));
                        if r.clicked() {
                            self.blink.frame = i;
                            commands.push(Command::Open(row.path.clone()));
                        }
                        let e = &row.evaluation;
                        ui.label(tint(e.fwhm.map_or("-".into(), |v| format!("{v:.2}"))));
                        ui.label(tint(
                            e.eccentricity.map_or("-".into(), |v| format!("{v:.2}")),
                        ));
                        ui.label(tint(e.stars.to_string()));
                        ui.label(tint(number(e.background)));
                        ui.label(tint(number(e.noise)));
                        let why: Vec<&str> = reasons.iter().map(|r| tr(lang, r.title())).collect();
                        ui.label(tint(why.join(", ")));
                        ui.end_row();
                    }
                });
        });
    }

    fn blink(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) {
        let lang = inputs.lang;
        let Some(alignment) = &self.alignment else {
            ui.centered_and_justified(|ui| {
                ui.weak(tr(lang, "Blink starts when all images are evaluated."));
            });
            return;
        };
        let shown = blink_frames(&alignment.maps, &self.reasons, self.blink.skip_rejected);
        let unaligned = alignment.maps.iter().filter(|m| m.is_none()).count();
        if !shown.contains(&self.blink.frame) {
            self.blink.frame = shown.first().copied().unwrap_or(alignment.reference);
        }
        let now = ui.input(|i| i.time);
        if self.blink.playing && !shown.is_empty() {
            let interval = 1.0 / f64::from(inputs.fps.max(0.1));
            if now - self.blink.last_step >= interval {
                self.blink.frame = step(&shown, self.blink.frame, true);
                self.blink.last_step = now;
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(interval));
        }
        let mut step_by = None;
        ui.horizontal(|ui| {
            if ui
                .button(icon::CARET_LEFT)
                .on_hover_text(tr(lang, "Previous"))
                .clicked()
            {
                step_by = Some(false);
            }
            let play = if self.blink.playing {
                icon::PAUSE
            } else {
                icon::PLAY
            };
            if ui.button(play).clicked() {
                self.blink.playing = !self.blink.playing;
                self.blink.last_step = now;
            }
            if ui
                .button(icon::CARET_RIGHT)
                .on_hover_text(tr(lang, "Next"))
                .clicked()
            {
                step_by = Some(true);
            }
            ui.checkbox(&mut self.blink.skip_rejected, tr(lang, "Skip rejected"));
            let position = shown.iter().position(|&i| i == self.blink.frame);
            ui.weak(format!(
                "{} / {}",
                position.map_or(0, |p| p + 1),
                shown.len()
            ));
            if unaligned > 0 {
                ui.weak(tr1(
                    lang,
                    "Not aligned, left out: {}",
                    &unaligned.to_string(),
                ));
            }
        });
        if let Some(forward) = step_by {
            self.blink.frame = step(&shown, self.blink.frame, forward);
            self.blink.playing = false;
        }
        let frame = self.blink.frame;
        let Some(Some(map)) = alignment.maps.get(frame) else {
            return;
        };
        let reference = &self.rows[alignment.reference].preview;
        let texture = self.blink.textures.entry(frame).or_insert_with(|| {
            let pixels = warp(reference, &self.rows[frame].preview, map);
            let image = egui::ColorImage::from_gray([reference.width, reference.height], &pixels);
            ui.ctx()
                .load_texture(format!("subframe-{frame}"), image, Default::default())
        });
        let area = ui.available_rect_before_wrap();
        let size = vec2(reference.width as f32, reference.height as f32);
        let fit = (area.width() / size.x).min(area.height() / size.y).max(0.0);
        let rect = egui::Rect::from_center_size(area.center(), size * fit);
        ui.allocate_rect(area, Sense::hover());
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let row = &self.rows[frame];
        let reasons = &self.reasons[frame];
        let mut caption = crate::compare::file_name(&row.path);
        if !reasons.is_empty() {
            let why: Vec<&str> = reasons.iter().map(|r| tr(lang, r.title())).collect();
            caption = format!("{caption}  ({})", why.join(", "));
        }
        let color = if reasons.is_empty() {
            Color32::WHITE
        } else {
            ui.visuals().error_fg_color
        };
        ui.painter().text(
            rect.left_top() + vec2(6.0, 4.0),
            egui::Align2::LEFT_TOP,
            caption,
            egui::FontId::proportional(14.0),
            color,
        );
    }
}

const IDENTITY: Map = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

/// 位置合わせの基準。FWHM のいちばん小さい（星のいちばん細い）画像。
fn reference_of(rows: &[Row]) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| r.evaluation.bright.len() >= 4)
        .filter_map(|(i, r)| Some((i, r.evaluation.fwhm?)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// blink で順に出す画像（`rows` の添字）。合わなかった画像は出さない。
fn blink_frames(maps: &[Option<Map>], reasons: &[Vec<Reason>], skip_rejected: bool) -> Vec<usize> {
    maps.iter()
        .enumerate()
        .filter(|(i, m)| m.is_some() && !(skip_rejected && !reasons[*i].is_empty()))
        .map(|(i, _)| i)
        .collect()
}

/// `frames` の中で `now` の次（`forward` でなければ前）。端では反対の端へ回る。
fn step(frames: &[usize], now: usize, forward: bool) -> usize {
    let Some(at) = frames.iter().position(|&i| i == now) else {
        return frames.first().copied().unwrap_or(now);
    };
    let n = frames.len();
    frames[if forward {
        (at + 1) % n
    } else {
        (at + n - 1) % n
    }]
}

struct Medians {
    fwhm: Option<f32>,
    eccentricity: Option<f32>,
    stars: Option<f32>,
    background: Option<f32>,
    noise: Option<f32>,
}

impl Medians {
    fn of(rows: &[Row]) -> Self {
        let median = |f: &dyn Fn(&Evaluation) -> Option<f32>| {
            let mut v: Vec<f32> = rows.iter().filter_map(|r| f(&r.evaluation)).collect();
            if v.is_empty() {
                return None;
            }
            let mid = v.len() / 2;
            Some(*v.select_nth_unstable_by(mid, f32::total_cmp).1)
        };
        Self {
            fwhm: median(&|e| e.fwhm),
            eccentricity: median(&|e| e.eccentricity),
            stars: median(&|e| Some(e.stars as f32)),
            background: median(&|e| Some(e.background)),
            noise: median(&|e| Some(e.noise)),
        }
    }
}

/// 表に出す順（`rows` の添字）。値のない画像は、昇順でも降順でも最後に置く。
fn sorted(rows: &[Row], (column, ascending): (Column, bool)) -> Vec<usize> {
    let key = |r: &Row| -> Option<f32> {
        let e = &r.evaluation;
        match column {
            Column::Name => Some(r.order as f32),
            Column::Fwhm => e.fwhm,
            Column::Eccentricity => e.eccentricity,
            Column::Stars => Some(e.stars as f32),
            Column::Background => Some(e.background),
            Column::Noise => Some(e.noise),
        }
    };
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| match (key(&rows[a]), key(&rows[b])) {
        (Some(x), Some(y)) if ascending => x.total_cmp(&y),
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    order
}

/// 背景と雑音の値。FITS の ADU（数千〜数万）も、0〜1 にそろえた値も読めるように桁を変える。
fn number(v: f32) -> String {
    if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else if v.abs() >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.4}")
    }
}

/// 画像を縮めてグレーにし、「自動で明るく見せる」と同じ曲線で見やすくする。
pub fn preview_of(img: &Float, side: usize) -> Preview {
    let factor = img.width.max(img.height).div_ceil(side.max(1)).max(1);
    let (w, h) = ((img.width / factor).max(1), (img.height / factor).max(1));
    let n = img.channels.max(1);
    let mut data = vec![0.0f32; w * h];
    for (y, out_row) in data.chunks_exact_mut(w).enumerate() {
        for (x, out) in out_row.iter_mut().enumerate() {
            let mut sum = 0.0;
            let mut count = 0;
            for sy in y * factor..((y + 1) * factor).min(img.height) {
                for sx in x * factor..((x + 1) * factor).min(img.width) {
                    let at = (sy * img.width + sx) * n;
                    let v: f32 = img.data[at..at + n].iter().sum::<f32>() / n as f32;
                    if v.is_finite() {
                        sum += v;
                        count += 1;
                    }
                }
            }
            *out = if count > 0 { sum / count as f32 } else { 0.0 };
        }
    }
    // 黒と白は最小と 99.9% の値。最大にしないのは、ホットピクセルや飽和した星 1 つで
    // 空が真っ黒につぶれるため。
    let mut sorted = data.clone();
    let lo = sorted.iter().copied().fold(f32::INFINITY, f32::min);
    let top = (sorted.len() - 1) * 999 / 1000;
    let hi = *sorted.select_nth_unstable_by(top, f32::total_cmp).1;
    let range = if hi > lo { hi - lo } else { 1.0 };
    for v in &mut data {
        *v = ((*v - lo) / range).clamp(0.0, 1.0);
    }
    AutoStretch {
        target: 0.25,
        shadows: 2.8,
        linked: true,
    }
    .apply(&mut data, 1);
    Preview {
        width: w,
        height: h,
        scale: 1.0 / factor as f64,
        pixels: data
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect(),
    }
}

/// `other` を `reference` の向きと位置に合わせた画像（大きさは `reference` と同じ）。
/// `map` は元の解像度での、基準の座標から `other` の座標への写し方。はみ出た所は黒。
pub fn warp(reference: &Preview, other: &Preview, map: &Map) -> Vec<u8> {
    let mut out = vec![0u8; reference.width * reference.height];
    for (v, out_row) in out.chunks_exact_mut(reference.width).enumerate() {
        for (u, out) in out_row.iter_mut().enumerate() {
            let full = [
                (u as f64 + 0.5) / reference.scale - 0.5,
                (v as f64 + 0.5) / reference.scale - 0.5,
            ];
            let [x, y] = subframes::apply(map, full);
            let px = (x + 0.5) * other.scale - 0.5;
            let py = (y + 0.5) * other.scale - 0.5;
            *out = bilinear(other, px, py).unwrap_or(0);
        }
    }
    out
}

fn bilinear(p: &Preview, x: f64, y: f64) -> Option<u8> {
    if x < -0.5 || y < -0.5 || x > p.width as f64 - 0.5 || y > p.height as f64 - 0.5 {
        return None;
    }
    let x = x.clamp(0.0, (p.width - 1) as f64);
    let y = y.clamp(0.0, (p.height - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(p.width - 1), (y0 + 1).min(p.height - 1));
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let at = |x: usize, y: usize| f64::from(p.pixels[y * p.width + x]);
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    Some((top * (1.0 - fy) + bottom * fy).round() as u8)
}

fn start_scan(dir: PathBuf, registry: Registry, order: SortOrder, ctx: &egui::Context) -> Scan {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let images = crate::tree::list_folder(&dir, &registry, order).images;
        send(&tx, &ctx, Found::Total(images.len()));
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..WORKERS {
                let tx = tx.clone();
                s.spawn(|| {
                    let tx = tx;
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= images.len() || stop.load(Ordering::Relaxed) {
                            return;
                        }
                        let found = match evaluate_file(&images[i], &registry) {
                            Ok((evaluation, preview)) => Found::Row(Box::new(Row {
                                path: images[i].clone(),
                                order: i,
                                evaluation,
                                preview,
                            })),
                            Err(e) => Found::Problem(format!("{}: {e}", images[i].display())),
                        };
                        send(&tx, &ctx, found);
                        send(&tx, &ctx, Found::Step);
                    }
                });
            }
        });
    });
    Scan {
        rx,
        cancel,
        done: 0,
        total: 0,
    }
}

fn send(tx: &Sender<Found>, ctx: &egui::Context, found: Found) {
    let _ = tx.send(found);
    ctx.request_repaint();
}

fn evaluate_file(path: &Path, registry: &Registry) -> Result<(Evaluation, Preview), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let (image, _) = registry
        .decode_bytes(path, &bytes)
        .map_err(|e| e.to_string())?;
    drop(bytes);
    let (linear, _) = vw_pipeline::pixel::linear_of_display(&image);
    drop(image);
    Ok((
        subframes::evaluate(&linear),
        preview_of(&linear, PREVIEW_SIDE),
    ))
}

#[cfg(test)]
mod tests;
