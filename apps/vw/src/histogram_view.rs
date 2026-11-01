//! 解析ウインドウ。アクティブなスロットの画像に追従し、画像全体のヒストグラムと
//! 定量評価（[`crate::quality_view`]）をタブで切り替えて出す。十分に高いウインドウでは両方を上下に並べる。
//! ピクセルパイプラインで値を変えているときは、変更前（グレーの塗り）と変更後（線）を重ねる。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, pos2};
use vw_core::{ColorModel, DisplayParams, PixelData, VwImage, display};
use vw_dicom::pipeline::Pipeline;

use crate::histogram::{self, Channel, LEVELS, Levels, Physical, Stats};
use crate::i18n::tr;
use crate::pixel_pipeline::{self, Decoded, Params};
use crate::quality::Report;
use crate::quality_view::{self, Command};
use crate::roi::Region;
use crate::settings::{Language, Quality};

/// ウインドウに渡す、アクティブなスロットの画像。
pub struct Target<'a> {
    /// 表示中の画像のパス（フレームならフレームのパス）。
    pub path: &'a Path,
    /// フル解像度の画像。まだ読み込んでいなければ `None`。
    pub image: Option<&'a Arc<VwImage>>,
    pub dicom: bool,
    /// カメラ RAW。センサーの値（現像前）の軸を出せる。
    pub raw: bool,
    /// ピクセルパイプラインで手で変えている値（この画像に当てているときだけ）。
    pub edited: Option<Params>,
    /// 色導出で描いた画像（当てているときだけ）。明るさの軸の「変更後」に使う。
    pub derived: Option<&'a Arc<egui::ColorImage>>,
    /// スロットに描いた対象と背景の ROI。
    pub target_roi: Option<Region>,
    pub background_roi: Option<Region>,
}

/// 定量評価の結果。ウインドウの外（[`crate::quality::Evaluation`]）が持つ。
pub struct Evaluated<'a> {
    pub report: Option<&'a Report>,
    pub computing: bool,
    pub thresholds: &'a Quality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Histogram,
    Evaluation,
}

/// 両方を上下に並べるのに要るウインドウの高さ。
const BOTH_HEIGHT: f32 = 700.0;

/// 数える範囲。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Range {
    #[default]
    Whole,
    Target,
    Background,
}

impl Range {
    fn title(self) -> &'static str {
        match self {
            Range::Whole => "Whole image",
            Range::Target => "Target ROI",
            Range::Background => "Background ROI",
        }
    }

    fn region(self, t: &Target<'_>) -> Option<Region> {
        match self {
            Range::Whole => None,
            Range::Target => t.target_roi,
            Range::Background => t.background_roi,
        }
    }
}

/// 横軸。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Axis {
    /// 表示の明るさ（0..=255）。色の変え方の違いがそのまま出るので既定にする。
    #[default]
    Brightness,
    /// 物理量（HU など）。DICOM のグレースケールのみ。
    Physical,
    /// ファイルに入っている値そのもの。DICOM 以外の 16 bit・実数のグレースケール。
    Original,
    /// カメラ RAW の現像前のセンサーの値。白レベルから何 EV 下か。
    Sensor,
}

/// チャンネルの選び方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Shown {
    #[default]
    Luma,
    One(Channel),
    /// R・G・B を重ねる。
    Rgb,
}

/// DICOM のグレースケール画像の段の値。物理量の軸と窓の位置に使う。
struct GrayInfo {
    decoded: Decoded,
    info: Box<vw_dicom::pipeline::Gray>,
    unit: Option<String>,
}

enum Gray {
    Reading(Receiver<Result<Pipeline, String>>),
    Loaded(GrayInfo),
    /// カラーの DICOM、または読めなかった。物理量の軸は出さない。
    None,
}

/// 数え直すかどうかを決める、数えた条件。
#[derive(Debug, Clone, PartialEq)]
struct Key {
    path: PathBuf,
    image: usize,
    axis: Axis,
    edited: Option<Params>,
    derived: Option<usize>,
    region: Option<Region>,
}

enum Counted {
    Levels {
        before: Levels,
        after: Option<Levels>,
    },
    Physical {
        histogram: Physical,
        windows: Option<Windows>,
        /// 手で値を変えている（変えたのが窓や関数だけで山が同じでも）。
        changed: bool,
        unit: Option<String>,
    },
    Sensor(Result<vw_raw::SensorHistogram, String>),
}

#[derive(Default)]
pub struct HistogramView {
    pub open: bool,
    tab: Tab,
    axis: Axis,
    channel: Shown,
    log: bool,
    range: Range,
    path: Option<PathBuf>,
    raw: bool,
    gray: Option<Gray>,
    counted: Option<(Key, Counted)>,
    counting: Option<(Key, Receiver<Counted>)>,
}

impl HistogramView {
    pub fn close(&mut self) {
        self.open = false;
        self.path = None;
        self.gray = None;
        self.counted = None;
        self.counting = None;
    }

    /// 欄の見出し。「解析 - ファイル名」。
    pub fn title(&self, lang: Language) -> String {
        crate::sub_window::title(lang, "Analysis", self.path.as_deref())
    }

    /// 1 フレーム分、欄の中を描く。評価の表から頼まれたことを返す。
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        target: Option<Target<'_>>,
        evaluated: Evaluated<'_>,
    ) -> Option<Command> {
        let ctx = ui.ctx().clone();
        self.follow(&ctx, target.as_ref());
        if let Some(t) = &target {
            self.count(&ctx, t);
        }
        let rois = target.as_ref().map_or((false, false), |t| {
            (t.target_roi.is_some(), t.background_roi.is_some())
        });
        let has_image = target.as_ref().is_some_and(|t| t.image.is_some());
        if target.is_none() {
            ui.centered_and_justified(|ui| ui.weak(tr(lang, "Select an image in a slot.")));
            None
        } else if !has_image {
            ui.centered_and_justified(|ui| ui.spinner());
            None
        } else {
            self.tabs(
                ui,
                lang,
                rois,
                &evaluated,
                target.as_ref().and_then(|t| t.image),
            )
        }
    }

    /// タブの行と、選んだタブの中身。ウインドウが高ければタブを出さずに両方を並べる。
    fn tabs(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        rois: (bool, bool),
        evaluated: &Evaluated<'_>,
        image: Option<&Arc<VwImage>>,
    ) -> Option<Command> {
        let evaluation = |ui: &mut egui::Ui| {
            quality_view::table(
                ui,
                lang,
                evaluated.report,
                evaluated.thresholds,
                evaluated.computing,
            )
        };
        if ui.available_height() >= BOTH_HEIGHT {
            let mut command = None;
            egui::Panel::bottom("analysis_evaluation")
                .resizable(false)
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.strong(tr(lang, "Evaluation"));
                    command = evaluation(ui);
                });
            ui.strong(tr(lang, "Histogram"));
            self.contents(ui, lang, rois, image);
            return command;
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Histogram, tr(lang, "Histogram"));
            ui.selectable_value(&mut self.tab, Tab::Evaluation, tr(lang, "Evaluation"));
        });
        ui.separator();
        match self.tab {
            Tab::Histogram => {
                self.contents(ui, lang, rois, image);
                None
            }
            Tab::Evaluation => evaluation(ui),
        }
    }

    fn follow(&mut self, ctx: &egui::Context, target: Option<&Target<'_>>) {
        let path = target.map(|t| t.path.to_path_buf());
        self.raw = target.is_some_and(|t| t.raw);
        if path != self.path {
            self.path = path;
            self.gray = target.map(|t| {
                if t.dicom {
                    Gray::Reading(pixel_pipeline::read_in_background(ctx, t.path))
                } else {
                    Gray::None
                }
            });
        }
        if let Some(Gray::Reading(rx)) = &self.gray
            && let Ok(result) = rx.try_recv()
        {
            self.gray = Some(match result {
                Ok(Pipeline::Gray(info)) => Gray::Loaded(GrayInfo {
                    decoded: Decoded::of(&info),
                    unit: pixel_pipeline::unit_of(&info),
                    info,
                }),
                _ => Gray::None,
            });
        }
        let available = self.axis_beyond_brightness(target.and_then(|t| t.image));
        if available != Some(self.axis) {
            self.axis = Axis::Brightness;
        }
    }

    /// 明るさのほかに選べる横軸。画像をまだ読んでいなければ、DICOM の段の値だけで決める。
    fn axis_beyond_brightness(&self, image: Option<&Arc<VwImage>>) -> Option<Axis> {
        if matches!(self.gray, Some(Gray::Loaded(_))) {
            Some(Axis::Physical)
        } else if self.raw {
            Some(Axis::Sensor)
        } else if image.is_some_and(|i| has_original_values(i)) {
            Some(Axis::Original)
        } else {
            None
        }
    }

    /// 条件が変わっていたら裏で数え直す。数えている間は前の結果を出しておく。
    fn count(&mut self, ctx: &egui::Context, t: &Target<'_>) {
        if let Some((key, rx)) = &self.counting
            && let Ok(counted) = rx.try_recv()
        {
            self.counted = Some((key.clone(), counted));
            self.counting = None;
        }
        let Some(image) = t.image else { return };
        let gray = match &self.gray {
            Some(Gray::Loaded(g)) => Some(g),
            _ => None,
        };
        // 選んだ範囲の ROI が消えていたら画像全体に戻る。
        if self.range.region(t).is_none() {
            self.range = Range::Whole;
        }
        // センサーの値はファイルから数えるので、表示の画像や ROI が変わっても数え直さない。
        // ROI は向きを直した後の画素で引くので、センサーの並びには当てられない。
        let sensor = self.axis == Axis::Sensor;
        let key = Key {
            path: t.path.to_path_buf(),
            image: if sensor {
                0
            } else {
                Arc::as_ptr(image) as usize
            },
            axis: self.axis,
            edited: t.edited.filter(|_| gray.is_some() && !sensor),
            derived: t
                .derived
                .filter(|_| !sensor)
                .map(|d| Arc::as_ptr(d) as usize),
            region: self.range.region(t).filter(|_| !sensor),
        };
        let done = self.counted.as_ref().is_some_and(|(k, _)| *k == key);
        if done || self.counting.is_some() {
            return;
        }
        let image = image.clone();
        let gray = gray.map(|g| {
            let file = pixel_pipeline::file_params(&g.info, &image, &g.decoded);
            (g.decoded, file, g.unit.clone())
        });
        let (axis, edited, region) = (key.axis, key.edited, key.region);
        let derived = t.derived.cloned();
        let path = t.path.to_path_buf();
        let rx = crate::background::compute(ctx, move || match (axis, gray) {
            (Axis::Sensor, _) => Counted::Sensor(
                std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| {
                        vw_raw::sensor_histogram(&bytes, histogram::BINS).map_err(|e| e.to_string())
                    }),
            ),
            (Axis::Physical, Some((decoded, file, unit))) => Counted::Physical {
                histogram: histogram::physical(
                    &image,
                    &decoded,
                    (file.slope, file.intercept),
                    edited.map(|p| (p.slope, p.intercept)),
                    region,
                ),
                windows: Some(Windows {
                    before: (file.center, file.width),
                    after: edited.filter(|e| *e != file).map(|e| (e.center, e.width)),
                }),
                changed: edited.is_some_and(|e| e != file),
                unit,
            },
            (Axis::Original, _) => Counted::Physical {
                histogram: histogram::physical(&image, &AS_STORED, (1.0, 0.0), None, region),
                windows: image.window.map(|(black, white)| Windows {
                    before: (f64::from(black + white) / 2.0, f64::from(white - black)),
                    after: None,
                }),
                changed: false,
                unit: None,
            },
            _ => {
                // 表示用の既定（一辺 8192 まで縮める）を使わないのは、ROI が元の大きさの画素で
                // 来るため。縮めた絵を元の幅で引くと、一辺 8192 を超える画像で行がずれていた。
                let params = DisplayParams {
                    max_side: u32::MAX,
                    ..DisplayParams::default()
                };
                let shown = display::to_display(&image, &params);
                Counted::Levels {
                    before: histogram::levels_of_rgba(&shown.rgba, shown.width, region),
                    after: derived
                        .map(|d| histogram::levels_of_rgba(d.as_raw(), d.size[0] as u32, region)),
                }
            }
        });
        self.counting = Some((key, rx));
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        rois: (bool, bool),
        image: Option<&Arc<VwImage>>,
    ) {
        let beyond = self.axis_beyond_brightness(image);
        let color =
            matches!(&self.counted, Some((_, Counted::Levels { before, .. })) if before.color);
        ui.horizontal(|ui| {
            ui.label(tr(lang, "Horizontal axis"));
            vw_ui::menu::combo("histogram_axis")
                .selected_text(axis_title(lang, self.axis))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.axis,
                        Axis::Brightness,
                        axis_title(lang, Axis::Brightness),
                    );
                    if let Some(axis) = beyond {
                        ui.selectable_value(&mut self.axis, axis, axis_title(lang, axis));
                    }
                });
            if self.axis == Axis::Brightness && color {
                ui.label(tr(lang, "Channel"));
                vw_ui::menu::combo("histogram_channel")
                    .selected_text(shown_title(lang, self.channel))
                    .show_ui(ui, |ui| {
                        for choice in [
                            Shown::Luma,
                            Shown::One(Channel::Red),
                            Shown::One(Channel::Green),
                            Shown::One(Channel::Blue),
                            Shown::Rgb,
                        ] {
                            ui.selectable_value(
                                &mut self.channel,
                                choice,
                                shown_title(lang, choice),
                            );
                        }
                    });
            }
            ui.checkbox(&mut self.log, tr(lang, "Logarithmic vertical axis"));
            if (rois.0 || rois.1) && self.axis != Axis::Sensor {
                ui.label(tr(lang, "Range"));
                vw_ui::menu::combo("histogram_range")
                    .selected_text(tr(lang, self.range.title()))
                    .show_ui(ui, |ui| {
                        let choices = [
                            (Range::Whole, true),
                            (Range::Target, rois.0),
                            (Range::Background, rois.1),
                        ];
                        for (choice, available) in choices {
                            if available {
                                ui.selectable_value(
                                    &mut self.range,
                                    choice,
                                    tr(lang, choice.title()),
                                );
                            }
                        }
                    });
            }
            if self.counting.is_some() {
                ui.spinner();
            }
        });
        let Some((_, counted)) = &self.counted else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        if let Counted::Sensor(Err(e)) = counted {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return;
        }
        let channel = if color { self.channel } else { Shown::Luma };
        let graph = Graph::of(counted, channel, ui.visuals());
        let changed = graph.series.iter().any(|s| s.after.is_some())
            || matches!(counted, Counted::Physical { changed: true, .. });
        let sensor = match counted {
            Counted::Sensor(Ok(h)) => Some(h),
            _ => None,
        };
        if sensor.is_none() {
            legend(ui, lang, changed);
        }
        // センサーの軸は下に白飛びの表が付くので、その分グラフを低くする。
        let below = if sensor.is_some() { 190.0 } else { 100.0 };
        let height = (ui.available_height() - below).max(80.0);
        let hover = draw(ui, &graph, self.log, height);
        stats_table(ui, lang, &graph, changed);
        if let Some(h) = sensor {
            exposure_table(ui, lang, h);
        }
        let text = match hover {
            Some(i) => graph.hover_text(lang, i),
            None => tr(lang, "Point at the graph to see the counts.").to_owned(),
        };
        ui.weak(text);
    }
}

/// 格納値をそのまま値とみなす。DICOM 以外の画像は傾きや切片を持たない。
const AS_STORED: Decoded = Decoded {
    slope: 1.0,
    intercept: 0.0,
    negated: false,
};

/// 表示の明るさ（8 bit）に縮める前の値を、軸にする意味がある画像か。
/// カラーは 1 本の山にまとめられないので、グレーだけにする。
fn has_original_values(image: &VwImage) -> bool {
    image.color == ColorModel::Gray && !matches!(image.data, PixelData::U8(_))
}

fn axis_title(lang: Language, axis: Axis) -> &'static str {
    match axis {
        Axis::Brightness => tr(lang, "Display brightness"),
        Axis::Physical => tr(lang, "Physical value"),
        Axis::Original => tr(lang, "Original value"),
        Axis::Sensor => tr(lang, "RAW sensor value (stops below white)"),
    }
}

fn shown_title(lang: Language, s: Shown) -> &'static str {
    match s {
        Shown::Luma => tr(lang, Channel::Luma.title()),
        Shown::One(c) => tr(lang, c.title()),
        Shown::Rgb => tr(lang, "RGB overlaid"),
    }
}

/// グラフに描く 1 本分（1 チャンネル）。
struct Series {
    before: Vec<u32>,
    after: Option<Vec<u32>>,
    color: Color32,
    before_stats: Stats,
    after_stats: Option<Stats>,
    name: &'static str,
}

/// 物理量の軸の窓（中心, 幅）。
#[derive(Clone, Copy)]
struct Windows {
    before: (f64, f64),
    after: Option<(f64, f64)>,
}

/// 描くものをまとめたもの。横軸は区間の番号で、値への換算は `value_at`。
struct Graph {
    series: Vec<Series>,
    lo: f64,
    hi: f64,
    /// 物理量の軸の、変更前と変更後の窓（中心, 幅）。
    windows: Option<Windows>,
    unit: Option<String>,
    physical: bool,
    /// 塗りの色（変更前）。チャンネルを重ねるときは各チャンネルの色を薄くして使う。
    single: bool,
    /// ピクセルパイプラインの変更前と変更後を比べられる軸。統計の行に「変更前」と付ける。
    compared: bool,
}

impl Graph {
    fn of(counted: &Counted, shown: Shown, visuals: &egui::Visuals) -> Self {
        let accent = visuals.selection.stroke.color;
        match counted {
            Counted::Levels { before, after } => {
                let channels: Vec<Channel> = match shown {
                    Shown::Luma => vec![Channel::Luma],
                    Shown::One(c) => vec![c],
                    Shown::Rgb => vec![Channel::Red, Channel::Green, Channel::Blue],
                };
                let single = channels.len() == 1;
                let series = channels
                    .into_iter()
                    .map(|c| {
                        let b = before.channel(c).to_vec();
                        let a = after.as_ref().map(|a| a.channel(c).to_vec());
                        Series {
                            before_stats: histogram::level_stats(&b),
                            after_stats: a.as_deref().map(histogram::level_stats),
                            before: b,
                            after: a,
                            color: if single { accent } else { channel_color(c) },
                            name: c.title(),
                        }
                    })
                    .collect();
                Self {
                    series,
                    lo: 0.0,
                    hi: (LEVELS - 1) as f64,
                    windows: None,
                    unit: None,
                    physical: false,
                    single,
                    compared: true,
                }
            }
            Counted::Sensor(h) => {
                let channels = h.as_ref().map_or(&[][..], |h| &h.channels[..]);
                let single = channels.len() == 1;
                Self {
                    series: channels
                        .iter()
                        .map(|c| Series {
                            before_stats: histogram::bin_stats(&c.counts, vw_raw::DARKEST_EV, 0.0),
                            after_stats: None,
                            before: c.counts.clone(),
                            after: None,
                            color: if single {
                                accent
                            } else {
                                sensor_color(c.color)
                            },
                            name: sensor_title(c.color),
                        })
                        .collect(),
                    lo: vw_raw::DARKEST_EV,
                    hi: 0.0,
                    windows: None,
                    unit: Some("EV".to_owned()),
                    physical: true,
                    single,
                    compared: false,
                }
            }
            Counted::Physical {
                histogram: h,
                windows,
                unit,
                ..
            } => Self {
                series: vec![Series {
                    before: h.before.clone(),
                    after: h.after.clone(),
                    color: accent,
                    before_stats: h.before_stats,
                    after_stats: h.after_stats,
                    name: Channel::Luma.title(),
                }],
                lo: h.lo,
                hi: h.hi,
                windows: *windows,
                unit: unit.clone(),
                physical: true,
                single: true,
                compared: true,
            },
        }
    }

    fn bins(&self) -> usize {
        self.series.first().map_or(1, |s| s.before.len()).max(1)
    }

    fn value_at(&self, i: usize) -> f64 {
        if self.physical {
            self.lo + (self.hi - self.lo) * (i as f64 + 0.5) / self.bins() as f64
        } else {
            i as f64
        }
    }

    fn format(&self, v: f64) -> String {
        // 0 のすぐ下の値が「-0.0」と出ないように。
        let v = if v.abs() < 0.05 { 0.0 } else { v };
        let n = if self.physical {
            format!("{v:.1}")
        } else {
            format!("{v:.0}")
        };
        match &self.unit {
            Some(u) => format!("{n} {u}"),
            None => n,
        }
    }

    fn hover_text(&self, lang: Language, i: usize) -> String {
        let mut parts = vec![format!(
            "{}: {}",
            tr(lang, "Value"),
            self.format(self.value_at(i))
        )];
        for s in &self.series {
            let prefix = if self.single {
                String::new()
            } else {
                format!("{} ", tr(lang, s.name))
            };
            parts.push(match &s.after {
                Some(a) => format!(
                    "{prefix}{} {} / {} {}",
                    tr(lang, "before"),
                    s.before[i],
                    tr(lang, "after"),
                    a[i]
                ),
                None => format!("{prefix}{}", s.before[i]),
            });
        }
        parts.push(tr(lang, "pixels").to_owned());
        parts.join("   ")
    }
}

fn channel_color(c: Channel) -> Color32 {
    match c {
        Channel::Red => Color32::from_rgb(230, 90, 90),
        Channel::Green => Color32::from_rgb(90, 200, 110),
        Channel::Blue => Color32::from_rgb(100, 140, 240),
        Channel::Luma => Color32::GRAY,
    }
}

fn sensor_color(c: vw_raw::SensorColor) -> Color32 {
    match c {
        vw_raw::SensorColor::Red => channel_color(Channel::Red),
        vw_raw::SensorColor::Green => channel_color(Channel::Green),
        vw_raw::SensorColor::Blue => channel_color(Channel::Blue),
        vw_raw::SensorColor::Other => Color32::GRAY,
    }
}

fn sensor_title(c: vw_raw::SensorColor) -> &'static str {
    match c {
        vw_raw::SensorColor::Red => Channel::Red.title(),
        vw_raw::SensorColor::Green => Channel::Green.title(),
        vw_raw::SensorColor::Blue => Channel::Blue.title(),
        vw_raw::SensorColor::Other => "Sensor",
    }
}

/// センサーの色ごとの、白飛び・黒つぶれの割合と白レベルまでの余裕。
fn exposure_table(ui: &mut egui::Ui, lang: Language, h: &vw_raw::SensorHistogram) {
    ui.add_space(4.0);
    egui::Grid::new("sensor_exposure")
        .num_columns(4)
        .spacing([18.0, 2.0])
        .show(ui, |ui| {
            ui.label("");
            for title in ["Clipped (white level)", "At black level", "Headroom"] {
                ui.weak(tr(lang, title));
            }
            ui.end_row();
            for c in &h.channels {
                let percent = |n: u64| 100.0 * n as f64 / c.total.max(1) as f64;
                ui.label(tr(lang, sensor_title(c.color)));
                let clipped = format!("{:.3} %", percent(c.clipped));
                if c.clipped > 0 {
                    ui.colored_label(ui.visuals().error_fg_color, clipped);
                } else {
                    ui.label(clipped);
                }
                ui.label(format!("{:.3} %", percent(c.at_black)));
                let headroom = c.headroom_ev();
                ui.label(if headroom.is_finite() {
                    format!("{headroom:.2} EV")
                } else {
                    "-".to_owned()
                });
                ui.end_row();
            }
        });
}

fn legend(ui: &mut egui::Ui, lang: Language, changed: bool) {
    ui.horizontal(|ui| {
        let before = ui.visuals().weak_text_color();
        let accent = ui.visuals().selection.stroke.color;
        swatch(ui, before, true);
        ui.label(tr(lang, "Before (file values)"));
        if changed {
            swatch(ui, accent, false);
            ui.label(tr(lang, "After (pixel pipeline)"));
        } else {
            ui.weak(tr(lang, "No change in the pixel pipeline"));
        }
    });
}

fn swatch(ui: &mut egui::Ui, color: Color32, fill: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 10.0), Sense::hover());
    if fill {
        ui.painter()
            .rect_filled(rect, 1.0, color.gamma_multiply(0.6));
    } else {
        ui.painter().line_segment(
            [rect.left_center(), rect.right_center()],
            Stroke::new(1.5, color),
        );
    }
}

/// グラフを描き、カーソルが乗っている区間の番号を返す。
fn draw(ui: &mut egui::Ui, g: &Graph, log: bool, height: f32) -> Option<usize> {
    let label_h = 16.0;
    let (outer, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
    let rect = outer.with_max_y(outer.max.y - label_h);
    let visuals = ui.visuals();
    let painter = ui.painter_at(outer);
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    let bins = g.bins();
    let peak = g
        .series
        .iter()
        .flat_map(|s| s.before.iter().chain(s.after.iter().flatten()))
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let height_of = |n: u32| {
        let r = if log {
            (n as f32).ln_1p() / peak.ln_1p()
        } else {
            n as f32 / peak
        };
        r * rect.height()
    };
    let bin_w = rect.width() / bins as f32;
    let x_of_bin = |i: usize| rect.min.x + (i as f32 + 0.5) * bin_w;
    let weak = visuals.weak_text_color();
    for s in &g.series {
        let fill = if g.single {
            weak.gamma_multiply(0.6)
        } else {
            s.color.gamma_multiply(0.3)
        };
        for (i, &n) in s.before.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let x = rect.min.x + i as f32 * bin_w;
            let r = Rect::from_min_max(
                pos2(x, rect.max.y - height_of(n)),
                pos2((x + bin_w).max(x + 1.0), rect.max.y),
            );
            painter.rect_filled(r, 0.0, fill);
        }
    }
    for s in &g.series {
        if let Some(after) = &s.after {
            let points: Vec<Pos2> = after
                .iter()
                .enumerate()
                .map(|(i, &n)| pos2(x_of_bin(i), rect.max.y - height_of(n)))
                .collect();
            painter.add(egui::Shape::line(points, Stroke::new(1.5, s.color)));
        }
    }
    if let Some(Windows { before, after }) = g.windows {
        let x_of_value =
            |v: f64| rect.min.x + ((v - g.lo) / (g.hi - g.lo).max(1e-9)) as f32 * rect.width();
        let accent = visuals.selection.stroke.color;
        let edges = |(c, w): (f64, f64)| [c - w / 2.0, c + w / 2.0];
        for v in edges(before) {
            let x = x_of_value(v);
            painter.extend(egui::Shape::dashed_line(
                &[pos2(x, rect.min.y), pos2(x, rect.max.y)],
                Stroke::new(1.0, weak),
                4.0,
                3.0,
            ));
        }
        if let Some(after) = after {
            for v in edges(after) {
                let x = x_of_value(v);
                painter.line_segment(
                    [pos2(x, rect.min.y), pos2(x, rect.max.y)],
                    Stroke::new(1.5, accent),
                );
            }
        }
    }
    let font = egui::FontId::proportional(11.0);
    let y = rect.max.y + 2.0;
    painter.text(
        pos2(rect.min.x, y),
        egui::Align2::LEFT_TOP,
        g.format(g.lo),
        font.clone(),
        weak,
    );
    painter.text(
        pos2(rect.max.x, y),
        egui::Align2::RIGHT_TOP,
        g.format(g.hi),
        font,
        weak,
    );
    let pointer = response.hover_pos().filter(|p| rect.contains(*p))?;
    let i = (((pointer.x - rect.min.x) / bin_w) as usize).min(bins - 1);
    let x = x_of_bin(i);
    painter.line_segment(
        [pos2(x, rect.min.y), pos2(x, rect.max.y)],
        Stroke::new(1.0, visuals.strong_text_color().gamma_multiply(0.5)),
    );
    Some(i)
}

fn stats_table(ui: &mut egui::Ui, lang: Language, g: &Graph, changed: bool) {
    egui::Grid::new("histogram_stats")
        .num_columns(5)
        .spacing([18.0, 2.0])
        .show(ui, |ui| {
            ui.label("");
            for h in ["Minimum", "Maximum", "Mean", "Standard deviation"] {
                ui.weak(tr(lang, h));
            }
            ui.end_row();
            for s in &g.series {
                let name = |when: &'static str| {
                    if !g.compared {
                        tr(lang, s.name).to_owned()
                    } else if g.single {
                        tr(lang, when).to_owned()
                    } else {
                        format!("{} {}", tr(lang, s.name), tr(lang, when))
                    }
                };
                let row = |ui: &mut egui::Ui, label: String, st: &Stats| {
                    ui.label(label);
                    for v in [st.min, st.max] {
                        ui.label(g.format(v));
                    }
                    ui.label(format!("{:.2}", st.mean));
                    ui.label(format!("{:.2}", st.std_dev));
                    ui.end_row();
                };
                row(ui, name("Before"), &s.before_stats);
                if let (true, Some(a)) = (changed, &s.after_stats) {
                    row(ui, name("After"), a);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(color: ColorModel, data: PixelData) -> VwImage {
        VwImage::new(2, 1, color, data).unwrap()
    }

    #[test]
    fn deep_gray_images_offer_their_original_values() {
        assert!(has_original_values(&image(
            ColorModel::Gray,
            PixelData::U16(vec![0, 40000])
        )));
        assert!(has_original_values(&image(
            ColorModel::Gray,
            PixelData::F32(vec![-1.5, 2.5])
        )));
    }

    #[test]
    fn eight_bit_and_color_images_keep_only_the_brightness_axis() {
        assert!(!has_original_values(&image(
            ColorModel::Gray,
            PixelData::U8(vec![0, 255])
        )));
        assert!(!has_original_values(&image(
            ColorModel::Rgb,
            PixelData::U16(vec![0; 6])
        )));
    }

    #[test]
    fn original_values_are_counted_as_stored() {
        let picture = image(ColorModel::Gray, PixelData::U16(vec![1000, 50000]));
        let h = histogram::physical(&picture, &AS_STORED, (1.0, 0.0), None, None);
        assert_eq!((h.lo, h.hi), (1000.0, 50000.0));
        assert_eq!(h.before_stats.mean, 25500.0);
    }
}
