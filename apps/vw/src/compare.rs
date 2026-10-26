//! 似た画像を位置合わせして比べる欄。スロットに見えている画像（画素の変換を通した色）を
//! 受け取り、裏で位置合わせと差の分析をして、並べる・切り替え・スワイプ・差で見せる。
//! 拡大とパンは基準の画像の座標で持ち、どの見せ方でも連動する。
//!
//! 位置合わせと分析は [`vw_compare`]。ここは受け渡しと描画だけを持つ。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use eframe::egui::{
    self, pos2, vec2, Color32, ColorImage, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2,
};
use vw_compare::report::{Labels, Report};
use vw_compare::{render, Affine, Analysis, Compared, Options, Prepared};

use crate::i18n::{tr, tr1};
use crate::settings::Language;

/// 比べる 1 枚（スロットに見えている、元の解像度の画像）。
pub struct Input {
    pub path: PathBuf,
    pub image: ColorImage,
}

/// 欄の外（本体）にしてもらうこと。
pub enum Command {
    CopyImage(ColorImage, String),
    CopyText(String, String),
    Info(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    SideBySide,
    Blink,
    Swipe,
    Difference,
}

impl Mode {
    const ALL: [Mode; 4] = [Mode::SideBySide, Mode::Blink, Mode::Swipe, Mode::Difference];

    fn title(self) -> &'static str {
        match self {
            Mode::SideBySide => "Side by side",
            Mode::Blink => "Blink",
            Mode::Swipe => "Swipe",
            Mode::Difference => "Difference",
        }
    }
}

/// 差を重ねるときの不透明度。
const OVERLAY_ALPHA: f32 = 0.5;
/// 違いの大きい場所の枠の色。
const HOTSPOT: Color32 = Color32::from_rgb(0, 220, 255);
/// 値の欄の幅。
const PANEL_WIDTH: f32 = 300.0;

#[derive(Default)]
pub struct CompareWindow {
    pub open: bool,
    session: Option<Session>,
    picker: Option<Box<vw_tree::picker::Picker>>,
    /// 前にレポートを書いたフォルダ。
    report_dir: Option<PathBuf>,
}

struct Entry {
    path: PathBuf,
    name: String,
    size: [usize; 2],
    texture: TextureHandle,
    /// 基準（0 番）は `None` のまま。
    result: Option<Arc<Compared>>,
    heat: Option<TextureHandle>,
}

/// 分析し直した結果（`entries` の添字ごと）。
type Reanalysed = Vec<(usize, Option<Analysis>)>;

enum Progress {
    Reference(Arc<Prepared>),
    Compared(usize, Box<Compared>),
}

struct Session {
    entries: Vec<Entry>,
    reference: Option<Arc<Prepared>>,
    job: Option<Receiver<Progress>>,
    /// 明るさの合わせ方を変えたときの分析し直し。
    reanalysis: Option<Receiver<Reanalysed>>,
    mode: Mode,
    overlay: bool,
    show_hotspots: bool,
    options: Options,
    /// 切り替え・スワイプ・差で基準と比べる画像（1 番から）。
    target: usize,
    /// 枠に収まる倍率に掛ける倍率。
    zoom: f32,
    /// 枠の中心に来る点の、基準の中心からのずれ（基準の画素）。
    pan: Vec2,
    /// スワイプの境目（左からの割合）。
    swipe: f32,
    dragging_swipe: bool,
    /// 切り替えで基準を見せている。
    blink_reference: bool,
    /// 位置合わせの間に出す、終わった枚数。
    done: usize,
}

impl CompareWindow {
    pub fn title(&self, lang: Language) -> String {
        let name = tr(lang, "Compare images");
        match self.session.as_ref().and_then(|s| s.entries.first()) {
            Some(r) => format!("{name} - {}", r.name),
            None => name.to_owned(),
        }
    }

    /// `images` を比べ始める。最初の 1 枚が基準。
    pub fn start(&mut self, ctx: &egui::Context, images: Vec<Input>, match_brightness: bool) {
        let max_side = ctx.input(|i| i.max_texture_side).min(8192);
        let mut entries = Vec::new();
        let mut sources = Vec::new();
        for (i, input) in images.into_iter().enumerate() {
            let [w, h] = input.image.size;
            let shown = fit_texture(&input.image, max_side);
            let texture =
                ctx.load_texture(format!("compare-{i}"), shown, egui::TextureOptions::LINEAR);
            entries.push(Entry {
                name: file_name(&input.path),
                path: input.path,
                size: [w, h],
                texture,
                result: None,
                heat: None,
            });
            sources.push(input.image);
        }
        let options = Options { match_brightness };
        let (tx, rx) = channel();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let mut sources = sources.into_iter();
            let Some(first) = sources.next() else {
                return;
            };
            let reference = Arc::new(prepare(&first));
            drop(first);
            if tx.send(Progress::Reference(reference.clone())).is_err() {
                return;
            }
            wake.request_repaint();
            for (i, img) in sources.enumerate() {
                let moving = prepare(&img);
                drop(img);
                let c = vw_compare::compare(&reference, &moving, options);
                if tx.send(Progress::Compared(i + 1, Box::new(c))).is_err() {
                    return;
                }
                wake.request_repaint();
            }
        });
        self.session = Some(Session {
            entries,
            reference: None,
            job: Some(rx),
            reanalysis: None,
            mode: Mode::SideBySide,
            overlay: false,
            show_hotspots: true,
            options,
            target: 1,
            zoom: 1.0,
            pan: Vec2::ZERO,
            swipe: 0.5,
            dragging_swipe: false,
            blink_reference: true,
            done: 0,
        });
        self.open = true;
    }

    /// 閉じたら、画像と結果を手放す（何百 MB にもなるため）。
    pub fn forget_while_closed(&mut self) {
        if !self.open {
            self.session = None;
            self.picker = None;
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, lang: Language) -> Vec<Command> {
        let mut out = Vec::new();
        if let Some(s) = &mut self.session {
            s.receive(ui.ctx());
        }
        let Some(session) = &mut self.session else {
            ui.centered_and_justified(|ui| {
                ui.weak(tr(
                    lang,
                    "Right-click an image in a slot and choose Align and compare...",
                ))
            });
            return out;
        };
        let mut write_report = false;
        session.toolbar(ui, lang, &mut out, &mut write_report);
        session.status(ui, lang);
        let rest = ui.available_rect_before_wrap();
        let panel_w = PANEL_WIDTH.min(rest.width() * 0.4);
        let view = rest.with_max_x(rest.max.x - panel_w - 6.0);
        let panel = rest.with_min_x(rest.max.x - panel_w);
        session.view(ui, lang, view);
        ui.scope_builder(egui::UiBuilder::new().max_rect(panel), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("compare_values")
                .auto_shrink(false)
                .show(ui, |ui| session.values(ui, lang));
        });
        ui.allocate_rect(rest, Sense::hover());
        if write_report {
            let start = self.report_dir.clone().or_else(|| {
                session
                    .entries
                    .first()
                    .and_then(|e| e.path.parent())
                    .map(Path::to_path_buf)
            });
            let name = chrono::Local::now()
                .format("compare-%Y%m%d-%H%M%S.html")
                .to_string();
            let picker = vw_tree::picker::Picker::new(
                tr(lang, "Write the comparison report"),
                vw_tree::picker::Mode::SaveFile {
                    extensions: vec!["html".to_owned()],
                    name,
                },
                start.as_deref(),
                crate::settings_ui::picker_labels(lang),
            );
            self.picker = Some(Box::new(picker));
        }
        if let Some(picker) = &mut self.picker {
            match picker.show(ui.ctx()) {
                vw_tree::picker::Outcome::Open => {}
                vw_tree::picker::Outcome::Cancelled => self.picker = None,
                vw_tree::picker::Outcome::Chosen(path) => {
                    self.picker = None;
                    self.report_dir = path.parent().map(Path::to_path_buf);
                    if let Some(s) = &self.session {
                        out.push(s.write_report(lang, &path));
                    }
                }
            }
        }
        out
    }
}

/// スロットの画像を、位置合わせと分析の大きさにする。
fn prepare(img: &ColorImage) -> Prepared {
    // as_raw はアルファを掛けた値だが、スロットの画像は不透明なので掛けていない値と同じ。
    Prepared::new(img.size[0], img.size[1], img.as_raw())
}

pub fn file_name(path: &Path) -> String {
    if let Some(t) = crate::compose::title(path) {
        return t;
    }
    let file = crate::frames::file_of(path);
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    match crate::frames::split(path) {
        Some((_, frame)) => format!("{name} #{}", frame + 1),
        None => name,
    }
}

/// テクスチャに載る大きさ（長辺 `max_side` 以下）にする。整数分の 1 の平均で縮める。
fn fit_texture(img: &ColorImage, max_side: usize) -> ColorImage {
    let [w, h] = img.size;
    let factor = w.max(h).div_ceil(max_side.max(1));
    if factor <= 1 {
        return img.clone();
    }
    let (nw, nh) = (w / factor, h / factor);
    let mut pixels = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0u32; 4];
            for dy in 0..factor {
                for dx in 0..factor {
                    let p = img.pixels[(y * factor + dy) * w + x * factor + dx].to_array();
                    for c in 0..4 {
                        sum[c] += u32::from(p[c]);
                    }
                }
            }
            let n = (factor * factor) as u32;
            pixels.push(Color32::from_rgba_premultiplied(
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                (sum[3] / n) as u8,
            ));
        }
    }
    ColorImage::new([nw, nh], pixels)
}

fn heat_texture(ctx: &egui::Context, i: usize, a: &Analysis, grid: [usize; 2]) -> TextureHandle {
    let img = render::difference(a, grid[0], grid[1]);
    ctx.load_texture(
        format!("compare-heat-{i}"),
        ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.data),
        egui::TextureOptions::NEAREST,
    )
}

fn color_image(img: &render::Rgba) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.data)
}

impl Session {
    fn receive(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.job {
            loop {
                match rx.try_recv() {
                    Ok(Progress::Reference(r)) => self.reference = Some(r),
                    Ok(Progress::Compared(i, c)) => {
                        self.done += 1;
                        let grid = self.grid_size();
                        if let Some(e) = self.entries.get_mut(i) {
                            e.heat = c.analysis.as_ref().map(|a| heat_texture(ctx, i, a, grid));
                            e.result = Some(Arc::from(c));
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.job = None;
                        break;
                    }
                }
            }
        }
        if let Some(rx) = &self.reanalysis {
            if let Ok(results) = rx.try_recv() {
                self.reanalysis = None;
                let grid = self.grid_size();
                for (i, a) in results {
                    let Some(e) = self.entries.get_mut(i) else {
                        continue;
                    };
                    let Some(old) = &e.result else {
                        continue;
                    };
                    e.heat = a.as_ref().map(|a| heat_texture(ctx, i, a, grid));
                    e.result = Some(Arc::new(Compared {
                        alignment: old.alignment,
                        warped: old.warped.clone(),
                        analysis: a,
                    }));
                }
            }
        }
    }

    fn grid_size(&self) -> [usize; 2] {
        self.reference
            .as_ref()
            .map_or([1, 1], |r| [r.image.width, r.image.height])
    }

    fn busy(&self) -> bool {
        self.job.is_some() || self.reanalysis.is_some()
    }

    /// 明るさの合わせ方を変えたので、写した画像はそのままで分析だけをし直す。
    fn reanalyse(&mut self, ctx: &egui::Context) {
        let Some(reference) = self.reference.clone() else {
            return;
        };
        let results: Vec<(usize, Arc<Compared>)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Some((i, e.result.clone()?)))
            .collect();
        let options = self.options;
        self.reanalysis = Some(crate::background::compute(ctx, move || {
            results
                .into_iter()
                .map(|(i, c)| (i, vw_compare::analyse(&reference.image, &c.warped, options)))
                .collect()
        }));
    }

    fn transform(&self, i: usize) -> Affine {
        let e = &self.entries[i];
        if let Some(r) = &e.result {
            return r.alignment.transform;
        }
        // 合わせ終わるまでは、中心を合わせて面積の比で倍率を揃えておく。
        let r = self.entries[0].size;
        let (rw, rh) = (r[0] as f64, r[1] as f64);
        let (mw, mh) = (e.size[0] as f64, e.size[1] as f64);
        Affine::similarity(
            ((mw * mh) / (rw * rh)).sqrt(),
            0.0,
            [(rw - 1.0) / 2.0, (rh - 1.0) / 2.0],
            [(mw - 1.0) / 2.0, (mh - 1.0) / 2.0],
        )
    }

    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        out: &mut Vec<Command>,
        write_report: &mut bool,
    ) {
        ui.horizontal_wrapped(|ui| {
            for m in Mode::ALL {
                ui.selectable_value(&mut self.mode, m, tr(lang, m.title()));
            }
            ui.separator();
            if self.entries.len() > 2 && self.mode != Mode::SideBySide {
                let shown = self.entries[self.target].name.clone();
                vw_ui::menu::combo("compare_target")
                    .selected_text(shown)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for i in 1..self.entries.len() {
                            let name = self.entries[i].name.clone();
                            ui.selectable_value(&mut self.target, i, name);
                        }
                    });
            }
            if self.mode == Mode::Blink
                && ui
                    .button(tr(lang, "Switch (Space)"))
                    .clicked()
            {
                self.blink_reference = !self.blink_reference;
            }
            if self.mode != Mode::Difference {
                ui.checkbox(&mut self.overlay, tr(lang, "Overlay the difference"));
            }
            ui.checkbox(&mut self.show_hotspots, tr(lang, "Largest differences"));
            let mut matched = self.options.match_brightness;
            let changed = ui
                .add_enabled(
                    !self.busy(),
                    egui::Checkbox::new(&mut matched, tr(lang, "Match brightness")),
                )
                .on_hover_text(tr(
                    lang,
                    "Match the brightness of each image to the reference before measuring differences (tone curves of RAW and JPEG differ).",
                ))
                .changed();
            if changed {
                self.options.match_brightness = matched;
                self.reanalyse(ui.ctx());
            }
            ui.separator();
            if ui.button(tr(lang, "Fit")).clicked() {
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            }
            let ready = !self.busy() && self.reference.is_some();
            if ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Copy view")))
                .on_hover_text(tr(lang, "Copy the whole picture of the current view as an image."))
                .clicked()
            {
                if let Some(img) = self.view_image() {
                    let what = format!("comparison view ({}x{})", img.size[0], img.size[1]);
                    out.push(Command::CopyImage(img, what));
                }
            }
            if ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Copy values")))
                .on_hover_text(tr(lang, "Copy the values as a table to paste into a spreadsheet."))
                .clicked()
            {
                if let Some(r) = self.report() {
                    out.push(Command::CopyText(
                        r.tsv(labels(lang)),
                        "comparison values".to_owned(),
                    ));
                }
            }
            if ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Write report...")))
                .on_hover_text(tr(lang, "Write an HTML report with the images, and a CSV of the values beside it."))
                .clicked()
            {
                *write_report = true;
            }
        });
    }

    fn status(&self, ui: &mut egui::Ui, lang: Language) {
        ui.horizontal(|ui| {
            if self.job.is_some() {
                ui.spinner();
                ui.label(tr1(
                    lang,
                    "Aligning and comparing {}",
                    &format!("{} / {}", self.done, self.entries.len() - 1),
                ));
            } else if self.reanalysis.is_some() {
                ui.spinner();
                ui.label(tr(lang, "Measuring the differences again"));
            }
            let weak: Vec<&str> = self
                .entries
                .iter()
                .filter(|e| {
                    e.result
                        .as_ref()
                        .is_some_and(|r| !r.alignment.looks_right())
                })
                .map(|e| e.name.as_str())
                .collect();
            if !weak.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    tr1(lang, "May be misaligned: {}", &weak.join(", ")),
                );
            }
        });
    }

    /// 基準の画素 → 画面の写し方（`tile` の中、今の拡大とパン）。
    fn to_screen(&self, tile: Rect) -> impl Fn([f64; 2]) -> Pos2 + Copy {
        let [w, h] = self.entries[0].size;
        let fit = crate::viewer::fit_scale(tile.size(), vec2(w as f32, h as f32));
        let scale = f64::from(fit * self.zoom);
        let centre = tile.center();
        let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
        let pan = self.pan;
        move |[x, y]: [f64; 2]| {
            pos2(
                centre.x + ((x - cx - f64::from(pan.x)) * scale) as f32,
                centre.y + ((y - cy - f64::from(pan.y)) * scale) as f32,
            )
        }
    }

    fn scale(&self, tile: Rect) -> f32 {
        let [w, h] = self.entries[0].size;
        crate::viewer::fit_scale(tile.size(), vec2(w as f32, h as f32)) * self.zoom
    }

    /// 枠の分け方。並べるときは全部、ほかは 1 枠。
    fn tiles(&self, area: Rect) -> Vec<(Rect, usize)> {
        if self.mode != Mode::SideBySide {
            return vec![(area, self.target)];
        }
        let n = self.entries.len();
        let cols = if n <= 3 {
            n
        } else {
            (n as f64).sqrt().ceil() as usize
        };
        let rows = n.div_ceil(cols);
        let (w, h) = (area.width() / cols as f32, area.height() / rows as f32);
        (0..n)
            .map(|i| {
                let (c, r) = (i % cols, i / cols);
                let min = area.min + vec2(c as f32 * w, r as f32 * h);
                (Rect::from_min_size(min, vec2(w, h)).shrink(1.0), i)
            })
            .collect()
    }

    fn view(&mut self, ui: &mut egui::Ui, lang: Language, area: Rect) {
        let id = ui.id().with("compare_view");
        let response = ui.interact(area, id, Sense::click_and_drag());
        let tiles = self.tiles(area);
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, ui.visuals().extreme_bg_color);
        for &(tile, i) in &tiles {
            self.draw_tile(ui, lang, tile, i);
        }
        // 操作は、ポインタの下の枠の大きさで解く（どの枠でも同じ点が同じ所に来る）。
        let hovered_tile = response
            .hover_pos()
            .or(response.interact_pointer_pos())
            .and_then(|p| tiles.iter().find(|(t, _)| t.contains(p)).map(|(t, _)| *t))
            .unwrap_or(area);
        if self.mode == Mode::Swipe {
            let x = hovered_tile.min.x + self.swipe * hovered_tile.width();
            if response.drag_started() {
                self.dragging_swipe = response
                    .interact_pointer_pos()
                    .is_some_and(|p| (p.x - x).abs() < 8.0);
            }
            if self.dragging_swipe {
                if let Some(p) = response.interact_pointer_pos() {
                    self.swipe =
                        ((p.x - hovered_tile.min.x) / hovered_tile.width()).clamp(0.0, 1.0);
                }
            }
            if response.drag_stopped() {
                self.dragging_swipe = false;
            }
            if response.hover_pos().is_some_and(|p| (p.x - x).abs() < 8.0) || self.dragging_swipe {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
        }
        if response.dragged() && !self.dragging_swipe {
            let s = self.scale(hovered_tile).max(1e-6);
            self.pan -= response.drag_delta() / s;
        }
        if response.double_clicked() {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
        if response.hovered() {
            let scroll = ui
                .ctx()
                .input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 200.0);
            if scroll != 0.0 {
                let factor = (scroll / 200.0).exp().clamp(0.5, 2.0);
                let before = self.scale(hovered_tile);
                let new_zoom = (self.zoom * factor).clamp(0.05, 400.0);
                if let Some(p) = response.hover_pos() {
                    let c = p - hovered_tile.center();
                    let after = before * new_zoom / self.zoom;
                    self.pan += c / before - c / after;
                }
                self.zoom = new_zoom;
            }
        }
        if self.mode == Mode::Blink
            && crate::sub_window::owns_keys(ui)
            && ui.ctx().input(|i| i.key_pressed(egui::Key::Space))
        {
            self.blink_reference = !self.blink_reference;
        }
    }

    fn draw_tile(&self, ui: &egui::Ui, lang: Language, tile: Rect, i: usize) {
        let painter = ui.painter_at(tile);
        let screen = self.to_screen(tile);
        let target = i.max(1).min(self.entries.len() - 1);
        match self.mode {
            Mode::SideBySide => {
                self.draw_image(&painter, i, screen);
                if self.overlay && i > 0 {
                    self.draw_heat(&painter, i, screen, OVERLAY_ALPHA);
                }
                if i > 0 {
                    self.draw_hotspots(&painter, i, screen);
                }
                self.caption(&painter, lang, tile, i);
            }
            Mode::Blink => {
                let shown = if self.blink_reference { 0 } else { target };
                self.draw_image(&painter, shown, screen);
                if self.overlay {
                    self.draw_heat(&painter, target, screen, OVERLAY_ALPHA);
                }
                self.draw_hotspots(&painter, target, screen);
                self.caption(&painter, lang, tile, shown);
            }
            Mode::Swipe => {
                let x = tile.min.x + self.swipe * tile.width();
                let left = painter.with_clip_rect(tile.with_max_x(x));
                let right = painter.with_clip_rect(tile.with_min_x(x));
                self.draw_image(&left, 0, screen);
                self.draw_image(&right, target, screen);
                if self.overlay {
                    self.draw_heat(&painter, target, screen, OVERLAY_ALPHA);
                }
                self.draw_hotspots(&painter, target, screen);
                painter.line_segment(
                    [pos2(x, tile.min.y), pos2(x, tile.max.y)],
                    Stroke::new(2.0, Color32::WHITE),
                );
                self.caption(&painter, lang, tile.with_max_x(x), 0);
                self.caption(&painter, lang, tile.with_min_x(x), target);
            }
            Mode::Difference => {
                if self.entries[target].heat.is_some() {
                    self.draw_heat(&painter, target, screen, 1.0);
                } else {
                    painter.text(
                        tile.center(),
                        egui::Align2::CENTER_CENTER,
                        tr(lang, "Measuring the differences"),
                        egui::FontId::proportional(14.0),
                        ui.visuals().weak_text_color(),
                    );
                }
                self.draw_hotspots(&painter, target, screen);
                let max = self.entries[target]
                    .result
                    .as_ref()
                    .and_then(|r| r.analysis.as_ref())
                    .map(|a| a.difference_max);
                let text = match max {
                    Some(m) => format!(
                        "{}  ({} {m:.1})",
                        self.entries[target].name,
                        tr(lang, "largest")
                    ),
                    None => self.entries[target].name.clone(),
                };
                caption_text(&painter, tile, &text, false);
            }
        }
    }

    fn draw_image(&self, painter: &egui::Painter, i: usize, screen: impl Fn([f64; 2]) -> Pos2) {
        let e = &self.entries[i];
        let [w, h] = e.size.map(|v| v as f64);
        let corners = [
            [-0.5, -0.5],
            [w - 0.5, -0.5],
            [w - 0.5, h - 0.5],
            [-0.5, h - 0.5],
        ];
        let uvs = [
            pos2(0.0, 0.0),
            pos2(1.0, 0.0),
            pos2(1.0, 1.0),
            pos2(0.0, 1.0),
        ];
        let to_reference = match i {
            0 => Affine::IDENTITY,
            _ => match self.transform(i).inverse() {
                Some(t) => t,
                None => return,
            },
        };
        let mut mesh = egui::Mesh::with_texture(e.texture.id());
        for (c, uv) in corners.iter().zip(uvs) {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: screen(to_reference.apply(*c)),
                uv,
                color: Color32::WHITE,
            });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }

    fn draw_heat(
        &self,
        painter: &egui::Painter,
        i: usize,
        screen: impl Fn([f64; 2]) -> Pos2,
        alpha: f32,
    ) {
        let Some(heat) = &self.entries[i].heat else {
            return;
        };
        let [w, h] = self.entries[0].size.map(|v| v as f64);
        let rect = Rect::from_min_max(screen([-0.5, -0.5]), screen([w - 0.5, h - 0.5]));
        painter.image(
            heat.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::from_white_alpha((alpha * 255.0).round() as u8),
        );
    }

    fn draw_hotspots(&self, painter: &egui::Painter, i: usize, screen: impl Fn([f64; 2]) -> Pos2) {
        if !self.show_hotspots {
            return;
        }
        let Some(a) = self.analysis(i) else {
            return;
        };
        let k = self.entries[0].size[0] as f64 / self.grid_size()[0] as f64;
        for (n, s) in a.hotspots.iter().enumerate() {
            let r = s.rect;
            let min = screen([r.x as f64 * k - 0.5, r.y as f64 * k - 0.5]);
            let max = screen([(r.x + r.w) as f64 * k - 0.5, (r.y + r.h) as f64 * k - 0.5]);
            let rect = Rect::from_min_max(min, max);
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.5, HOTSPOT),
                egui::StrokeKind::Outside,
            );
            painter.text(
                rect.left_top() + vec2(2.0, 1.0),
                egui::Align2::LEFT_TOP,
                format!("{}", n + 1),
                egui::FontId::proportional(12.0),
                HOTSPOT,
            );
        }
    }

    fn caption(&self, painter: &egui::Painter, lang: Language, tile: Rect, i: usize) {
        let e = &self.entries[i];
        let weak = e
            .result
            .as_ref()
            .is_some_and(|r| !r.alignment.looks_right());
        let text = match i {
            0 => format!("{}  ({})", e.name, tr(lang, "reference")),
            _ => e.name.clone(),
        };
        caption_text(painter, tile, &text, weak);
    }

    fn analysis(&self, i: usize) -> Option<&Analysis> {
        self.entries.get(i)?.result.as_ref()?.analysis.as_ref()
    }

    fn report(&self) -> Option<Report<'_>> {
        let reference = self.reference.as_ref()?;
        let entries = self.entries[1..]
            .iter()
            .filter_map(|e| {
                let r = e.result.as_ref()?;
                Some(vw_compare::report::Entry {
                    name: &e.name,
                    size: e.size,
                    alignment: &r.alignment,
                    analysis: r.analysis.as_ref(),
                    warped: Some(&r.warped),
                })
            })
            .collect();
        Some(Report {
            reference_name: &self.entries[0].name,
            reference_size: self.entries[0].size,
            grid: &reference.image,
            entries,
            created: "",
        })
    }

    fn write_report(&self, lang: Language, path: &Path) -> Command {
        let Some(mut report) = self.report() else {
            return Command::Error("Compare: nothing to write yet".to_owned());
        };
        let created = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        report.created = &created;
        let html = report.html(labels(lang));
        let csv_path = path.with_extension("csv");
        let write = |p: &Path, text: &str| -> Result<(), String> {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(p)
                .map_err(|e| format!("{}: {e}", p.display()))?;
            f.write_all(text.as_bytes())
                .map_err(|e| format!("{}: {e}", p.display()))
        };
        match write(path, &html).and_then(|_| write(&csv_path, &report.csv())) {
            Ok(()) => Command::Info(format!(
                "Compare: wrote {} and {}",
                path.display(),
                csv_path.display()
            )),
            Err(e) => Command::Error(format!("Compare: {e}")),
        }
    }

    /// 今の見せ方の全体を、解析した大きさの 1 枚にする。
    fn view_image(&self) -> Option<ColorImage> {
        let reference = self.reference.as_ref()?;
        let grid = &reference.image;
        let base = render::masked(grid, &vec![true; grid.width * grid.height], [0; 3]);
        let aligned = |i: usize| -> Option<render::Rgba> {
            let r = self.entries[i].result.as_ref()?;
            Some(render::masked(&r.warped.image, &r.warped.valid, [0; 3]))
        };
        let with_overlay = |img: render::Rgba, i: usize| -> render::Rgba {
            match (self.overlay, self.analysis(i)) {
                (true, Some(a)) => render::overlay(
                    &img,
                    &render::difference(a, grid.width, grid.height),
                    OVERLAY_ALPHA,
                ),
                _ => img,
            }
        };
        let t = self.target;
        let img = match self.mode {
            Mode::SideBySide => {
                let mut all = vec![base];
                for i in 1..self.entries.len() {
                    all.push(with_overlay(aligned(i)?, i));
                }
                let refs: Vec<&render::Rgba> = all.iter().collect();
                render::side_by_side(&refs, 8, [0; 3])
            }
            Mode::Blink => match self.blink_reference {
                true => with_overlay(base, t),
                false => with_overlay(aligned(t)?, t),
            },
            Mode::Swipe => with_overlay(render::swipe(&base, &aligned(t)?, self.swipe), t),
            Mode::Difference => render::difference(self.analysis(t)?, grid.width, grid.height),
        };
        Some(color_image(&img))
    }

    /// 値の欄。基準と比べる画像の組ごとの表と、明るさのヒストグラム。
    fn values(&self, ui: &mut egui::Ui, lang: Language) {
        let Some(report) = self.report() else {
            ui.weak(tr(lang, "Aligning the images..."));
            return;
        };
        let labels = labels(lang);
        let shown: Vec<usize> = match self.mode {
            Mode::SideBySide => (1..self.entries.len()).collect(),
            _ => vec![self.target],
        };
        for i in shown {
            let e = &self.entries[i];
            let Some(r) = &e.result else {
                ui.weak(format!("{}: {}", e.name, tr(lang, "aligning...")));
                continue;
            };
            let entry = vw_compare::report::Entry {
                name: &e.name,
                size: e.size,
                alignment: &r.alignment,
                analysis: r.analysis.as_ref(),
                warped: None,
            };
            ui.strong(&e.name);
            if !r.alignment.looks_right() {
                ui.colored_label(ui.visuals().warn_fg_color, labels.weak);
            }
            egui::Grid::new(("compare_values", i))
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (label, value) in
                        labels.rows.iter().zip(report.shown(labels, &entry)).skip(2)
                    {
                        ui.label(*label);
                        ui.label(value);
                        ui.end_row();
                    }
                });
            if let Some(a) = &r.analysis {
                ui.add_space(4.0);
                ui.label(labels.histogram);
                histogram(ui, &a.histograms);
                if !a.hotspots.is_empty() {
                    ui.label(labels.hotspots);
                    let k = self.entries[0].size[0] as f64 / report.grid.width as f64;
                    for (n, s) in a.hotspots.iter().enumerate() {
                        let r = s.rect;
                        let v = |x: usize| (x as f64 * k).round();
                        ui.weak(format!(
                            "{}. {}, {}, {}, {}  ({:.1})",
                            n + 1,
                            v(r.x),
                            v(r.y),
                            v(r.w),
                            v(r.h),
                            s.mean
                        ));
                    }
                }
            }
            ui.separator();
        }
    }
}

fn labels(lang: Language) -> &'static Labels {
    match lang {
        Language::English => &Labels::ENGLISH,
        Language::Japanese => &Labels::JAPANESE,
    }
}

fn caption_text(painter: &egui::Painter, tile: Rect, text: &str, warn: bool) {
    let font = egui::FontId::proportional(12.0);
    let color = if warn {
        Color32::from_rgb(255, 170, 60)
    } else {
        Color32::WHITE
    };
    let text = if warn {
        format!("⚠ {text}")
    } else {
        text.to_owned()
    };
    let galley = painter.layout_no_wrap(text, font, color);
    let rect = Rect::from_min_size(tile.min + vec2(4.0, 4.0), galley.size() + vec2(8.0, 4.0));
    painter.rect_filled(rect, 3.0, Color32::from_black_alpha(160));
    painter.galley(rect.min + vec2(4.0, 2.0), galley, color);
}

/// 基準（灰色）と比べる画像（橙）の明るさのヒストグラム。
fn histogram(ui: &mut egui::Ui, h: &[Vec<u32>; 2]) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width().min(280.0), 90.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect,
        0.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let top = h.iter().flatten().copied().max().unwrap_or(1).max(1) as f32;
    for (hist, color) in h
        .iter()
        .zip([Color32::GRAY, Color32::from_rgb(224, 128, 32)])
    {
        let points: Vec<Pos2> = hist
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                pos2(
                    rect.min.x + i as f32 / 255.0 * rect.width(),
                    rect.max.y - c as f32 / top * rect.height(),
                )
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(1.0, color)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_images_are_shrunk_to_fit_a_texture() {
        let img = ColorImage::new([100, 40], vec![Color32::from_gray(80); 4000]);
        let small = fit_texture(&img, 30);
        assert_eq!(small.size, [25, 10]);
        assert_eq!(small.pixels[0], Color32::from_gray(80));
        assert_eq!(fit_texture(&img, 100).size, [100, 40]);
    }

    #[test]
    fn the_side_by_side_view_has_one_tile_per_image() {
        let mut w = CompareWindow::default();
        let ctx = egui::Context::default();
        let input = |n: &str| Input {
            path: PathBuf::from(n),
            image: ColorImage::new([8, 8], vec![Color32::GRAY; 64]),
        };
        w.start(
            &ctx,
            vec![
                input("a.png"),
                input("b.png"),
                input("c.png"),
                input("d.png"),
            ],
            true,
        );
        let s = w.session.as_ref().unwrap();
        let area = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0));
        let tiles = s.tiles(area);
        assert_eq!(tiles.len(), 4);
        assert!(tiles[3].0.min.y > 100.0, "2 x 2: {tiles:?}");
        assert_eq!(w.title(Language::English), "Compare images - a.png");
        w.open = false;
        w.forget_while_closed();
        assert!(w.session.is_none());
    }
}
