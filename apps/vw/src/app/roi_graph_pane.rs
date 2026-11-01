//! ROI のグラフの欄。アクティブなスロットの画像に描いた ROI（対象・背景）の値を、帯のプロファイル・
//! ヒストグラム・フレームごとの平均のどれかの形で、計測線のグラフと同じレポートのテーマで描く。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;
use vw_report::canvas::{Op, Recorder};
use vw_report::{Block, Chart, Face, Sheet, Theme};

use crate::i18n::tr;
use crate::line_profile::Values;
use crate::roi::{Kind, Region};
use crate::roi_graph::{self, Area, Bins, Direction, FrameMean, Mode, Pixels};
use crate::settings::Language;

use super::VwApp;
use super::line_profile_pane::{paint_sheet, write_new};

/// 書き出す形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Export {
    Image,
    Report,
}

/// フレームを裏で読む同時の数。読むのは主にファイルの読み出しと展開で、多すぎると
/// 表示の読み込みの邪魔をする。
const WORKERS: usize = 3;

#[derive(Default)]
pub(super) struct RoiGraphPane {
    pub open: bool,
    mode: Mode,
    direction: Direction,
    picker: Option<(Box<vw_tree::picker::Picker>, Export)>,
    dir: Option<PathBuf>,
    frames: Option<FramesJob>,
    /// 前のフレームで数えた帯とヒストグラム。ROI の画素を毎フレーム数え直すと、大きな ROI で
    /// 画面が重くなるため。
    cache: Option<(CacheKey, Arc<Gathered>)>,
}

#[derive(PartialEq)]
struct CacheKey {
    path: PathBuf,
    image: usize,
    areas: Vec<Area>,
    mode: Mode,
    direction: Direction,
    bins: usize,
}

/// フレームごとの平均を裏で計る仕事。並びか ROI が変わったら作り直す。
struct FramesJob {
    paths: Vec<PathBuf>,
    areas: Vec<Area>,
    rx: Receiver<Found>,
    cancel: Arc<AtomicBool>,
    /// ROI ごと（`areas` と同じ順）の平均。届いた順なので、描く前に並べ替える。
    means: Vec<Vec<FrameMean>>,
    done: usize,
    problems: Vec<String>,
}

enum Found {
    Means(Vec<FrameMean>),
    Problem(String),
}

impl Drop for FramesJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn kind_name_en(kind: Kind) -> &'static str {
    kind.title()
}

fn kind_name_ja(kind: Kind) -> &'static str {
    tr(Language::Japanese, kind.title())
}

fn labels(lang: Language) -> roi_graph::Labels<'static> {
    roi_graph::Labels {
        kind_name: match lang {
            Language::Japanese => kind_name_ja,
            _ => kind_name_en,
        },
        position_x: tr(lang, "Position x"),
        position_y: tr(lang, "Position y"),
        pixel_value: tr(lang, "Pixel value"),
        measured_value: tr(lang, "Measured value"),
        mean_pixel: tr(lang, "Mean pixel value"),
        mean_measured: tr(lang, "Mean measured value"),
        count: tr(lang, "Pixels"),
        frame: tr(lang, "Frame"),
        roi: tr(lang, "ROI"),
        place: tr(lang, "Place (x, y  w×h)"),
        frames: tr(lang, "Frames"),
        kind: tr(lang, "Values"),
        min: tr(lang, "Min"),
        max: tr(lang, "Max"),
        mean: tr(lang, "Mean"),
        sd: tr(lang, "SD"),
    }
}

/// 欄に描くために集めたもの。
struct Gathered {
    path: PathBuf,
    areas: Vec<Area>,
    values: Values,
    color: vw_core::ColorModel,
    width: u32,
    height: u32,
    bits: u32,
    /// ROI ごとの中の全部の画素（表の統計とヒストグラムに使う。フレームの平均では空）。
    pixels: Vec<(Area, Pixels)>,
    body: Body,
}

enum Body {
    Band(Vec<roi_graph::Band>),
    Histogram(Bins),
    Frames {
        series: Vec<(Kind, Vec<FrameMean>)>,
        names: Vec<String>,
        /// 読み終えた数と全部の数。
        progress: (usize, usize),
        problems: Vec<String>,
    },
}

impl Gathered {
    fn meta(&self, lang: Language) -> Vec<String> {
        let file = crate::frames::file_of(&self.path);
        let mut name = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if let Some((_, frame)) = crate::frames::split(&self.path) {
            name = format!("{name}  {} {}", tr(lang, "Frame"), frame + 1);
        }
        let color = match self.color {
            vw_core::ColorModel::Gray | vw_core::ColorModel::GrayAlpha => tr(lang, "gray"),
            vw_core::ColorModel::Rgb | vw_core::ColorModel::Rgba => tr(lang, "color"),
        };
        let mut meta = vec![
            name,
            format!("{} × {}", self.width, self.height),
            format!("{}bit {color}", self.bits),
        ];
        if let Body::Frames { progress, .. } = &self.body {
            meta.push(format!("{} {}", progress.1, tr(lang, "frames")));
        }
        meta.push(crate::report_fonts::now());
        meta
    }

    fn title(&self, lang: Language) -> &'static str {
        match self.body {
            Body::Band(_) => tr(lang, "Band profile of the ROI"),
            Body::Histogram(_) => tr(lang, "Histogram of the ROI"),
            Body::Frames { .. } => tr(lang, "Mean of the ROI per frame"),
        }
    }

    fn caption(&self, lang: Language) -> String {
        let mut text = match &self.body {
            Body::Band(b) => match b.first().map(|b| b.direction) {
                Some(Direction::Vertical) => tr(
                    lang,
                    "Values from the top of each ROI to its bottom; each point is the mean of one row of the ROI.",
                ),
                _ => tr(
                    lang,
                    "Values from the left of each ROI to its right; each point is the mean of one column of the ROI.",
                ),
            },
            Body::Histogram(_) => tr(
                lang,
                "Number of pixels of each value in each ROI. All ROIs share the same bins.",
            ),
            Body::Frames { .. } => tr(
                lang,
                "Mean of each ROI in every frame of the sequence, at the same place in every frame.",
            ),
        }
        .to_owned();
        text.push(' ');
        text.push_str(tr(
            lang,
            "The values are those of the file, before the conversion for display (black and white, color map).",
        ));
        if self.values.two_kinds() && !matches!(self.body, Body::Histogram(_)) {
            text.push(' ');
            text.push_str(tr(
                lang,
                "The upper graph shows the pixel values stored in the file, the lower graph the measured values converted with the formula in the file.",
            ));
        }
        text
    }

    fn charts(&self, lang: Language, theme: Theme) -> Vec<Chart> {
        let l = labels(lang);
        match &self.body {
            Body::Band(b) => vec![roi_graph::band_chart(
                b,
                self.color,
                &self.values,
                &l,
                theme,
            )],
            Body::Histogram(bins) => {
                let areas = &self.pixels;
                let mut charts = vec![roi_graph::histogram_chart(
                    areas,
                    bins,
                    self.color,
                    &self.values,
                    &l,
                    theme,
                    false,
                )];
                if self.values.two_kinds() && areas.iter().any(|(_, p)| p.measured.is_some()) {
                    charts.push(roi_graph::histogram_chart(
                        areas,
                        &roi_graph::measured_bins(bins, &self.values),
                        self.color,
                        &self.values,
                        &l,
                        theme,
                        true,
                    ));
                }
                charts
            }
            Body::Frames { series, .. } => vec![roi_graph::frames_chart(
                series,
                self.color,
                &self.values,
                &l,
                theme,
            )],
        }
    }

    fn table(&self, lang: Language) -> vw_report::Table {
        let l = labels(lang);
        match &self.body {
            Body::Band(_) | Body::Histogram(_) => {
                let rows: Vec<_> = self
                    .pixels
                    .iter()
                    .map(|(a, px)| {
                        (
                            a.kind,
                            roi_graph::place(a.region),
                            px.pixel.clone(),
                            px.measured.clone(),
                        )
                    })
                    .collect();
                roi_graph::table(&rows, self.color, &self.values, &l, l.place)
            }
            Body::Frames { series, .. } => {
                let channels = crate::line_profile::channel_names(self.color).len();
                let rows: Vec<_> = series
                    .iter()
                    .map(|(kind, means)| {
                        let pixel = (0..channels)
                            .map(|c| {
                                means
                                    .iter()
                                    .map(|m| m.pixel.get(c).copied().unwrap_or(f64::NAN))
                                    .collect()
                            })
                            .collect();
                        let measured = means.iter().map(|m| m.measured).collect();
                        (*kind, means.len().to_string(), pixel, measured)
                    })
                    .collect();
                roi_graph::table(&rows, self.color, &self.values, &l, l.frames)
            }
        }
    }

    fn table_caption(&self, lang: Language) -> &'static str {
        match self.body {
            Body::Frames { .. } => tr(lang, "Statistics of the means of all frames"),
            _ => tr(lang, "Place and statistics of the pixels in each ROI"),
        }
    }

    fn sheet(&self, lang: Language, theme: Theme) -> Sheet {
        let mut blocks: Vec<Block> = self
            .charts(lang, theme)
            .into_iter()
            .map(Block::Chart)
            .collect();
        blocks.push(Block::Caption(self.caption(lang)));
        blocks.push(Block::Table(self.table(lang)));
        Sheet {
            title: self.title(lang).to_owned(),
            meta: self.meta(lang),
            blocks,
        }
    }

    fn rows(&self, separator: char) -> String {
        match &self.body {
            Body::Band(b) => roi_graph::band_rows(b, self.color, &self.values, separator),
            Body::Histogram(bins) => {
                roi_graph::histogram_rows(&self.pixels, bins, self.color, &self.values, separator)
            }
            Body::Frames { series, names, .. } => {
                roi_graph::frames_rows(series, names, self.color, &self.values, separator)
            }
        }
    }

    fn file_tag(&self) -> &'static str {
        match self.body {
            Body::Band(_) => "roi-band",
            Body::Histogram(_) => "roi-histogram",
            Body::Frames { .. } => "roi-frames",
        }
    }
}

const NO_ROI: &str =
    "Choose the ROI tool (R) and drag on an image to draw a target or background ROI.";

impl VwApp {
    /// アクティブなスロットの ROI と、並び（フレームやフォルダの画像）。
    fn roi_sources(&self) -> Option<(Vec<Area>, Vec<PathBuf>)> {
        let slot = self.slots.get(self.active)?;
        let shown = slot.shown()?;
        let sequence: Vec<PathBuf> = match &slot.sequence {
            Some(s) if s.paths.len() > 1 => s.paths.clone(),
            _ if shown.info.frames > 1 => {
                crate::frames::frame_paths(&crate::frames::file_of(&shown.path), shown.info.frames)
            }
            _ => vec![shown.path.clone()],
        };
        // 表示中の画像の ROI。フレームごとの平均では、並びのほかの画像に描いた ROI も使う
        // （フレームを送ると ROI が見えなくなっても、グラフが消えないように）。
        let mut areas = Vec::new();
        for kind in [Kind::Target, Kind::Background] {
            let on_shown = slot
                .rois
                .iter()
                .find(|r| r.path == shown.path && r.kind == kind);
            let on_sequence = || {
                slot.rois
                    .iter()
                    .find(|r| r.kind == kind && sequence.contains(&r.path))
            };
            let found = if self.roi_graph.mode == Mode::Frames {
                on_shown.or_else(on_sequence)
            } else {
                on_shown
            };
            if let Some(r) = found {
                areas.push(Area {
                    kind,
                    region: r.region,
                });
            }
        }
        Some((areas, sequence))
    }

    fn gather_roi_graph(
        &mut self,
        lang: Language,
        ctx: &egui::Context,
    ) -> Result<Arc<Gathered>, String> {
        let hint = |s: &'static str| tr(lang, s).to_owned();
        let (areas, sequence) = self.roi_sources().ok_or_else(|| hint(NO_ROI))?;
        if areas.is_empty() {
            return Err(hint(NO_ROI));
        }
        let shown = self.slots[self.active]
            .shown()
            .ok_or_else(|| hint(NO_ROI))?;
        let full = shown
            .full
            .as_ref()
            .ok_or_else(|| hint("Reading the image..."))?;
        let meta = shown.info.metadata.clone();
        let img = full.image.clone();
        let (path, width, height, bits) = (
            shown.path.clone(),
            shown.info.width,
            shown.info.height,
            shown.info.bits_per_sample,
        );
        let values = Values::of(&meta);
        let mode = self.roi_graph.mode;
        let key = CacheKey {
            path: path.clone(),
            image: Arc::as_ptr(&img) as usize,
            areas: areas.clone(),
            mode,
            direction: self.roi_graph.direction,
            bins: self.settings.settings.measure.roi_bins,
        };
        if mode != Mode::Frames
            && let Some((k, g)) = &self.roi_graph.cache
            && *k == key
        {
            return Ok(g.clone());
        }
        let pixels: Vec<(Area, Pixels)> = if mode == Mode::Frames {
            Vec::new()
        } else {
            areas
                .iter()
                .map(|&a| (a, roi_graph::pixels(&img, &meta, a.region, &values)))
                .collect()
        };
        let body = match mode {
            Mode::Band => Body::Band(
                areas
                    .iter()
                    .map(|&a| roi_graph::band(&img, &meta, a, self.roi_graph.direction, &values))
                    .collect(),
            ),
            Mode::Histogram => {
                let sets: Vec<&[f64]> = pixels
                    .iter()
                    .flat_map(|(_, p)| p.pixel.iter().map(Vec::as_slice))
                    .collect();
                Body::Histogram(roi_graph::common_bins(
                    &sets,
                    self.settings.settings.measure.roi_bins,
                ))
            }
            Mode::Frames => {
                if sequence.len() < 2 {
                    return Err(hint(
                        "This slot shows one image. Open a file with frames, a DICOM series or a folder slot to see the mean per frame.",
                    ));
                }
                self.ensure_frames_job(&sequence, &areas, ctx);
                let job = self.roi_graph.frames.as_ref().expect("job was just made");
                let series = areas
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let mut means = job.means[i].clone();
                        means.sort_by_key(|m| m.index);
                        (a.kind, means)
                    })
                    .collect();
                let names = sequence
                    .iter()
                    .map(|p| {
                        let file = crate::frames::file_of(p);
                        let name = file.file_name().unwrap_or_default().to_string_lossy();
                        match crate::frames::split(p) {
                            Some((_, f)) => format!("{name} #{}", f + 1),
                            None => name.into_owned(),
                        }
                    })
                    .collect();
                Body::Frames {
                    series,
                    names,
                    progress: (job.done, sequence.len()),
                    problems: job.problems.clone(),
                }
            }
        };
        let g = Arc::new(Gathered {
            path,
            areas,
            values,
            color: img.color,
            width,
            height,
            bits,
            pixels,
            body,
        });
        if mode != Mode::Frames {
            self.roi_graph.cache = Some((key, g.clone()));
        }
        Ok(g)
    }

    /// 並びと ROI に合ったフレームの仕事がなければ始める。
    fn ensure_frames_job(&mut self, paths: &[PathBuf], areas: &[Area], ctx: &egui::Context) {
        if let Some(job) = &mut self.roi_graph.frames
            && job.paths == paths
            && job.areas == areas
        {
            {
                while let Ok(found) = job.rx.try_recv() {
                    match found {
                        Found::Means(means) => {
                            for (i, m) in means.into_iter().enumerate() {
                                job.means[i].push(m);
                            }
                            job.done += 1;
                        }
                        Found::Problem(p) => {
                            job.problems.push(p);
                            job.done += 1;
                        }
                    }
                }
                return;
            }
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let registry = self.loader.registry().clone();
        let ctx = ctx.clone();
        let regions: Vec<Region> = areas.iter().map(|a| a.region).collect();
        let jobs: Vec<(usize, PathBuf)> = paths.iter().cloned().enumerate().collect();
        std::thread::spawn(move || {
            let chunk = jobs.len().div_ceil(WORKERS).max(1);
            std::thread::scope(|s| {
                for part in jobs.chunks(chunk) {
                    let tx = tx.clone();
                    let (registry, regions, stop, ctx) = (&registry, &regions, &stop, &ctx);
                    s.spawn(move || {
                        // 同じファイルのフレームが続くので、ファイルの中身は 1 回だけ読む。
                        let mut cached: Option<(PathBuf, Vec<u8>)> = None;
                        for (index, path) in part {
                            if stop.load(Ordering::Relaxed) {
                                return;
                            }
                            let found =
                                match frame_means(registry, path, *index, regions, &mut cached) {
                                    Ok(means) => Found::Means(means),
                                    Err(e) => Found::Problem(format!("{}: {e}", path.display())),
                                };
                            if tx.send(found).is_err() {
                                return;
                            }
                            ctx.request_repaint();
                        }
                    });
                }
            });
        });
        self.roi_graph.frames = Some(FramesJob {
            paths: paths.to_vec(),
            areas: areas.to_vec(),
            rx,
            cancel,
            means: vec![Vec::new(); areas.len()],
            done: 0,
            problems: Vec::new(),
        });
    }

    pub(super) fn roi_graph_pane(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let theme = self.settings.settings.report.theme.theme();
        let mut mode = self.roi_graph.mode;
        let mut direction = self.roi_graph.direction;
        ui.horizontal_wrapped(|ui| {
            for m in Mode::ALL {
                ui.selectable_value(&mut mode, m, tr(lang, m.title()));
            }
            if mode == Mode::Band {
                ui.separator();
                ui.selectable_value(
                    &mut direction,
                    Direction::Horizontal,
                    tr(lang, "Left to right"),
                );
                ui.selectable_value(
                    &mut direction,
                    Direction::Vertical,
                    tr(lang, "Top to bottom"),
                );
            }
        });
        self.roi_graph.mode = mode;
        self.roi_graph.direction = direction;
        if mode != Mode::Frames {
            // 裏で読んでいるフレームは、ほかの形に切り替えたら止める。
            self.roi_graph.frames = None;
        }
        let gathered = self.gather_roi_graph(lang, ui.ctx());
        let ready = gathered.is_ok();
        let mut copy_graph = false;
        let mut copy_values = false;
        let mut export = None;
        ui.horizontal_wrapped(|ui| {
            copy_graph = ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Copy graph")))
                .on_hover_text(tr(
                    lang,
                    "Copy the graph with its title, legend and table as an image.",
                ))
                .clicked();
            copy_values = ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Copy values")))
                .on_hover_text(tr(
                    lang,
                    "Copy the value of every point as a table to paste into a spreadsheet.",
                ))
                .clicked();
            if ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Write image...")))
                .on_hover_text(tr(lang, "Write the graph as a PNG image."))
                .clicked()
            {
                export = Some(Export::Image);
            }
            if ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Write report...")))
                .on_hover_text(tr(
                    lang,
                    "Write an HTML report with the image, the ROIs, the graph and the values, and a CSV beside it.",
                ))
                .clicked()
            {
                export = Some(Export::Report);
            }
        });
        let g = match gathered {
            Ok(g) => g,
            Err(hint) => {
                ui.separator();
                ui.centered_and_justified(|ui| ui.weak(hint));
                return;
            }
        };
        if let Body::Frames {
            progress: (done, total),
            problems,
            ..
        } = &g.body
        {
            if done < total {
                ui.add(
                    egui::ProgressBar::new(*done as f32 / (*total).max(1) as f32)
                        .text(format!("{} {done} / {total}", tr(lang, "Reading frames"))),
                );
            }
            if !problems.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!(
                        "{}: {}",
                        tr(lang, "Frames that could not be read"),
                        problems.len()
                    ),
                )
                .on_hover_text(problems.join("\n"));
            }
        }
        ui.separator();
        let sheet = g.sheet(lang, theme);
        egui::ScrollArea::vertical()
            .id_salt("roi_graph")
            .auto_shrink(false)
            .show(ui, |ui| paint_sheet(ui, &sheet, theme, lang));
        let ctx = ui.ctx().clone();
        if copy_graph {
            let img = self.sheet_image(&sheet, lang, theme);
            let what = format!("ROI graph ({}x{})", img.size[0], img.size[1]);
            ctx.copy_image(img);
            self.copied(&ctx, &what);
        }
        if copy_values {
            self.copy_text(&ctx, g.rows('\t'), "ROI graph values");
        }
        if let Some(kind) = export {
            self.start_roi_export(&g, lang, kind);
        }
        self.roi_export_picker(&ctx, lang, theme);
    }

    fn start_roi_export(&mut self, g: &Gathered, lang: Language, kind: Export) {
        let stem = crate::frames::file_of(&g.path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let (ext, title) = match kind {
            Export::Image => ("png", "Write the graph image"),
            Export::Report => ("html", "Write the ROI graph report"),
        };
        let start = self.roi_graph.dir.clone().or_else(|| {
            crate::frames::file_of(&g.path)
                .parent()
                .map(Path::to_path_buf)
        });
        let picker = vw_tree::picker::Picker::new(
            tr(lang, title),
            vw_tree::picker::Mode::SaveFile {
                extensions: vec![ext.to_owned()],
                name: format!("{stem}-{}-{stamp}.{ext}", g.file_tag()),
            },
            start.as_deref(),
            crate::settings_ui::picker_labels(lang),
        );
        self.roi_graph.picker = Some((Box::new(picker), kind));
    }

    fn roi_export_picker(&mut self, ctx: &egui::Context, lang: Language, theme: Theme) {
        let Some((picker, kind)) = self.roi_graph.picker.as_mut() else {
            return;
        };
        let kind = *kind;
        match picker.show(ctx) {
            vw_tree::picker::Outcome::Open => {}
            vw_tree::picker::Outcome::Cancelled => self.roi_graph.picker = None,
            vw_tree::picker::Outcome::Chosen(path) => {
                self.roi_graph.picker = None;
                self.roi_graph.dir = path.parent().map(Path::to_path_buf);
                let result = match self.gather_roi_graph(lang, ctx) {
                    Ok(g) => match kind {
                        Export::Image => {
                            let words = crate::report_fonts::words(lang);
                            let img = g.sheet(lang, theme).image(
                                theme,
                                &words,
                                crate::report_fonts::fonts(theme),
                            );
                            write_new(&path, &vw_report::png(&img))
                                .map(|()| path.display().to_string())
                        }
                        Export::Report => self.write_roi_report(&g, lang, theme, &path),
                    },
                    Err(e) => Err(e),
                };
                match result {
                    Ok(written) => self.log.info(format!("ROI graph: wrote {written}")),
                    Err(e) => self.log.error(format!("ROI graph: {e}")),
                }
            }
        }
    }

    fn write_roi_report(
        &self,
        g: &Gathered,
        lang: Language,
        theme: Theme,
        path: &Path,
    ) -> Result<String, String> {
        let words = crate::report_fonts::words(lang);
        let mut page = vw_report::html::Page::new(theme, words, g.title(lang), &g.meta(lang));
        if let Some(picture) = self.picture_with_rois(g, theme, lang) {
            page.image(
                &picture,
                &[],
                tr(
                    lang,
                    "The ROIs on the image. The names match the graph and the table.",
                ),
            );
        }
        let charts = g.charts(lang, theme);
        let last = charts.len().saturating_sub(1);
        for (i, chart) in charts.iter().enumerate() {
            // 説明文は最後のグラフにだけ付ける（ヒストグラムの 2 つのグラフで同じ文を繰り返さない）。
            let caption = if i == last {
                g.caption(lang)
            } else {
                String::new()
            };
            page.chart(chart, &caption);
        }
        page.table(&g.table(lang), Some(g.table_caption(lang)));
        let csv_path = path.with_extension("csv");
        page.paragraph(&format!(
            "{} {}",
            tr(lang, "The values are in the CSV file:"),
            csv_path.file_name().unwrap_or_default().to_string_lossy()
        ));
        write_new(path, page.finish().as_bytes())?;
        write_new(&csv_path, g.rows(',').as_bytes())?;
        Ok(format!("{} and {}", path.display(), csv_path.display()))
    }

    /// スロットに見えている画像に ROI の枠と名前を描いた図。
    fn picture_with_rois(
        &self,
        g: &Gathered,
        theme: Theme,
        lang: Language,
    ) -> Option<vw_report::Rgba> {
        let (_, image) = self.slot_copy_image(self.active)?;
        let image = vw_report::Rgba {
            width: image.size[0],
            height: image.size[1],
            data: image.as_raw().to_vec(),
        };
        let fonts = crate::report_fonts::fonts(theme);
        let width = vw_report::SHEET_WIDTH;
        // 表示用の絵は元の画像より小さいことがあるので、元の画像の大きさで位置を合わせる。
        let k = width / g.width.max(1) as f32;
        let height = g.height as f32 * k;
        let mut r = Recorder::new(fonts);
        r.ops.push(Op::Image {
            rect: [0.0, 0.0, width, height],
            image,
        });
        for a in &g.areas {
            let [cr, cg, cb, _] = Theme::Lab.line(roi_index(a.kind)).color.0;
            let color = vw_report::Color([cr, cg, cb, 255]);
            let rect = [
                a.region.x as f32 * k,
                a.region.y as f32 * k,
                a.region.w as f32 * k,
                a.region.h as f32 * k,
            ];
            r.rect(rect, None, Some((vw_report::Color([0, 0, 0, 150]), 4.0)));
            r.rect(rect, None, Some((color, 2.0)));
            let label = tr(lang, a.kind.title());
            let w = r.width(label, 12.0, Face::Body) + 8.0;
            let x = rect[0].min(width - w).max(0.0);
            let y = (rect[1] - 17.0).max(0.0);
            r.rect([x, y, w, 16.0], Some(color), None);
            r.text(
                label,
                [x + 4.0, y + 12.0],
                12.0,
                vw_report::Color([255, 255, 255, 255]),
            );
        }
        Some(vw_report::raster::render(
            &r.ops,
            width,
            height,
            vw_report::IMAGE_SCALE,
            fonts,
        ))
    }
}

/// グラフの色の番号（[`roi_graph`] と同じく、対象が 1 本目、背景が 2 本目）。
fn roi_index(kind: Kind) -> usize {
    match kind {
        Kind::Target => 0,
        Kind::Background => 1,
    }
}

/// 1 枚（フレーム）を読み、ROI ごとの平均を出す。
fn frame_means(
    registry: &vw_core::Registry,
    path: &Path,
    index: usize,
    regions: &[Region],
    cached: &mut Option<(PathBuf, Vec<u8>)>,
) -> Result<Vec<FrameMean>, String> {
    let (file, frame) = crate::frames::split(path).unwrap_or((path.to_path_buf(), 0));
    if cached.as_ref().is_none_or(|(f, _)| *f != file) {
        let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
        *cached = Some((file.clone(), bytes));
    }
    let bytes = &cached.as_ref().expect("read just above").1;
    let (image, _) = registry
        .decode_frame_bytes(&file, bytes, frame)
        .map_err(|e| e.to_string())?;
    Ok(regions
        .iter()
        .map(|&r| roi_graph::frame_mean(&image, &image.metadata, r, index))
        .collect())
}
