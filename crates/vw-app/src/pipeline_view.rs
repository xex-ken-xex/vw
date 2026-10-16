//! 画素の変換の図。格納値が表示の明るさになるまでの段をノードと線で並べた別ウインドウで、
//! 値を変えるとアクティブなスロットの表示に当てる（ファイルには書き戻さない）。
//!
//! 組み立てと操作はノードエディタ（ComfyUI）に、見た目は vw のパネルに合わせる。
//! ノードエディタのクレート（egui-snarl など）を使わないのは、初版はつなぎ替えがなく
//! 決まった 1 本の流れを描くだけで、見た目を vw のテーマに揃える手間の方が大きいため。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui::{
    self, emath::TSTransform, epaint::CubicBezierShape, pos2, vec2, Color32, ColorImage, Pos2,
    Rect, Sense, Stroke, UiBuilder, Vec2,
};
use vw_core::VwImage;
use vw_dicom::pipeline::{tags, Color, Gray, Origin, Pipeline, Tag};

use egui_phosphor::regular as icon;

use crate::i18n::{tr, tr_text};
use crate::pixel_pipeline::{self, Decoded, Histogram, Params, Trace, VoiFunction};
use crate::settings::Language;

/// ウインドウに渡す、アクティブなスロットの画像。
pub struct Target<'a> {
    /// 表示中の画像のパス（フレームならフレームのパス）。
    pub path: &'a Path,
    /// フル解像度の画像。まだ読み込んでいなければ `None`。
    pub image: Option<&'a Arc<VwImage>>,
    /// その画像に置いた十字の位置。
    pub marks: Vec<(u32, u32)>,
}

/// スロットの表示に当てる変更。
pub enum Change {
    /// 手で変えた値で描いた画像に差し替える。
    Show(PathBuf, ColorImage),
    /// ファイルの値での表示に戻す。
    Restore(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Node {
    Stored,
    Modality,
    Unit,
    Voi,
    Presentation,
    ColorSpace,
    Display,
}

/// 関係するノードをまとめる枠。
struct Group {
    title: &'static str,
    nodes: &'static [Node],
}

const GRAY_FLOW: [Node; 6] = [
    Node::Stored,
    Node::Modality,
    Node::Unit,
    Node::Voi,
    Node::Presentation,
    Node::Display,
];
const GRAY_GROUPS: [Group; 3] = [
    Group {
        title: "Modality",
        nodes: &[Node::Modality, Node::Unit],
    },
    Group {
        title: "VOI",
        nodes: &[Node::Voi],
    },
    Group {
        title: "Display",
        nodes: &[Node::Presentation, Node::Display],
    },
];
const COLOR_FLOW: [Node; 3] = [Node::Stored, Node::ColorSpace, Node::Display];

/// ノードの配置を形式ごとに分けて覚えるための鍵。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Gray,
    Color,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Gray => "gray",
            Kind::Color => "color",
        }
    }
}

impl Node {
    const ALL: [Node; 7] = [
        Node::Stored,
        Node::Modality,
        Node::Unit,
        Node::Voi,
        Node::Presentation,
        Node::ColorSpace,
        Node::Display,
    ];

    fn key(self) -> &'static str {
        match self {
            Node::Stored => "stored",
            Node::Modality => "modality",
            Node::Unit => "unit",
            Node::Voi => "voi",
            Node::Presentation => "presentation",
            Node::ColorSpace => "color_space",
            Node::Display => "display",
        }
    }
}

const NODE_WIDTH: f32 = 210.0;
const NODE_GAP: f32 = 40.0;

struct GrayState {
    info: Gray,
    decoded: Decoded,
    image: Option<Arc<VwImage>>,
    file: Params,
    params: Params,
    histogram: Option<Histogram>,
}

enum Loaded {
    Reading(Receiver<Result<Pipeline, String>>),
    Gray(Box<GrayState>),
    Color(Color),
    Failed(String),
}

#[derive(Default)]
pub struct PipelineView {
    pub open: bool,
    /// 見出しのボタンでヒストグラム・ズームマップを開くよう頼まれた。
    pub open_histogram: bool,
    pub open_zoom_map: bool,
    path: Option<PathBuf>,
    loaded: Option<Loaded>,
    /// 手で変えた値で描いた画像を当てているスロットの画像。
    applied: Option<PathBuf>,
    rendering: Option<(PathBuf, Receiver<ColorImage>)>,
    /// 描き直しを頼んだあとに、また値が変わった。
    stale: bool,
    positions: HashMap<(Kind, Node), Pos2>,
    collapsed: HashSet<Node>,
    rects: HashMap<Node, Rect>,
    transform: Option<TSTransform>,
    /// キャンバスの倍率。
    scale: f32,
}

impl PipelineView {
    /// 動かしたノードの位置（状態ファイルに残す形）。
    pub fn node_positions(&self) -> BTreeMap<String, [f32; 2]> {
        self.positions
            .iter()
            .map(|((kind, node), p)| (format!("{}.{}", kind.key(), node.key()), [p.x, p.y]))
            .collect()
    }

    /// 前回動かしたノードの位置に戻す。知らない鍵は捨てる。
    pub fn restore_node_positions(&mut self, saved: &BTreeMap<String, [f32; 2]>) {
        for kind in [Kind::Gray, Kind::Color] {
            for node in Node::ALL {
                if let Some([x, y]) = saved.get(&format!("{}.{}", kind.key(), node.key())) {
                    self.positions.insert((kind, node), pos2(*x, *y));
                }
            }
        }
    }

    /// `path` の画像に手で変えた値を当てていれば、その値。
    pub fn edited_for(&self, path: &Path) -> Option<Params> {
        match &self.loaded {
            Some(Loaded::Gray(g)) if self.path.as_deref() == Some(path) && g.params != g.file => {
                Some(g.params)
            }
            _ => None,
        }
    }

    /// 手で変えた値で描いた画像を当てているスロットの画像。
    pub fn applied(&self) -> Option<&Path> {
        self.applied.as_deref()
    }

    /// 閉じる。手で変えた値は捨て、表示をファイルの値に戻す。
    pub fn close(&mut self) -> Vec<Change> {
        self.open = false;
        self.path = None;
        self.loaded = None;
        self.rendering = None;
        self.applied
            .take()
            .map(Change::Restore)
            .into_iter()
            .collect()
    }

    /// 1 フレーム分。ウインドウを描き、スロットに当てる変更を返す。
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        lang: Language,
        target: Option<Target<'_>>,
    ) -> Vec<Change> {
        if !self.open {
            return Vec::new();
        }
        let mut out = self.follow(ctx, target.as_ref());
        let title = crate::sub_window::title(lang, "Pixel pipeline", self.path.as_deref());
        let marks = target.as_ref().map(|t| t.marks.clone()).unwrap_or_default();
        let mut close = false;
        let mut edited = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("pixel_pipeline"),
            egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([1560.0, 560.0]),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(ctx.style().visuals.extreme_bg_color))
                    .show(ctx, |ui| edited = self.contents(ui, lang, &marks));
            },
        );
        if edited {
            self.request_render(ctx);
        }
        out.extend(self.poll_render(ctx));
        if close {
            out.extend(self.close());
        }
        out
    }

    /// アクティブなスロットの画像が変わったら、その画像の段を読み直す。
    fn follow(&mut self, ctx: &egui::Context, target: Option<&Target<'_>>) -> Vec<Change> {
        let mut out = Vec::new();
        let path = target.map(|t| t.path.to_path_buf());
        if path != self.path {
            out.extend(self.applied.take().map(Change::Restore));
            self.rendering = None;
            self.path = path.clone();
            self.loaded =
                path.map(|p| Loaded::Reading(pixel_pipeline::read_in_background(ctx, &p)));
        }
        if let Some(Loaded::Reading(rx)) = &self.loaded {
            if let Ok(result) = rx.try_recv() {
                self.loaded = Some(match result {
                    Ok(Pipeline::Gray(info)) => Loaded::Gray(Box::new(GrayState {
                        decoded: Decoded::of(&info),
                        info: *info,
                        image: None,
                        file: placeholder(),
                        params: placeholder(),
                        histogram: None,
                    })),
                    Ok(Pipeline::Color(c)) => Loaded::Color(c),
                    Err(e) => Loaded::Failed(e),
                });
            }
        }
        // フル解像度の画像が届いた、または捨てられて読み直されたら、ヒストグラムを作り直し、
        // 手で変えた値があれば当て直す。
        if let (Some(Loaded::Gray(g)), Some(image)) =
            (&mut self.loaded, target.and_then(|t| t.image))
        {
            let same = g.image.as_ref().is_some_and(|i| Arc::ptr_eq(i, image));
            if !same {
                let first = g.image.is_none();
                g.image = Some(image.clone());
                g.file = pixel_pipeline::file_params(&g.info, image, &g.decoded);
                if first {
                    g.params = g.file;
                }
                g.histogram = Some(pixel_pipeline::histogram(
                    image,
                    &g.decoded,
                    g.file.slope,
                    g.file.intercept,
                ));
                if g.params != g.file {
                    self.request_render(ctx);
                }
            }
        }
        out
    }

    fn request_render(&mut self, ctx: &egui::Context) {
        let (Some(path), Some(Loaded::Gray(g))) = (&self.path, &self.loaded) else {
            return;
        };
        let Some(image) = g.image.clone() else {
            return;
        };
        if self.rendering.is_some() {
            self.stale = true;
            return;
        }
        self.stale = false;
        let (decoded, params) = (g.decoded, g.params);
        let rx = crate::background::compute(ctx, move || {
            pixel_pipeline::render(&image, &decoded, &params)
        });
        self.rendering = Some((path.clone(), rx));
    }

    fn poll_render(&mut self, ctx: &egui::Context) -> Vec<Change> {
        let Some((path, rx)) = &self.rendering else {
            return Vec::new();
        };
        let Ok(image) = rx.try_recv() else {
            return Vec::new();
        };
        let path = path.clone();
        self.rendering = None;
        let at_file = matches!(&self.loaded, Some(Loaded::Gray(g)) if g.params == g.file);
        let change = if at_file {
            self.applied = None;
            Change::Restore(path)
        } else {
            self.applied = Some(path.clone());
            Change::Show(path, image)
        };
        if self.stale {
            // 描いている間に変わった値で、もう一度描く。
            self.request_render(ctx);
        }
        vec![change]
    }

    /// 図の中身。値を変えたら `true`。
    fn contents(&mut self, ui: &mut egui::Ui, lang: Language, marks: &[(u32, u32)]) -> bool {
        let mut edited = false;
        egui::Panel::top("pipeline_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let can_reset = matches!(&self.loaded, Some(Loaded::Gray(g)) if g.params != g.file);
                if ui
                    .add_enabled(can_reset, egui::Button::new(tr(lang, "Reset to file values")))
                    .clicked()
                {
                    if let Some(Loaded::Gray(g)) = &mut self.loaded {
                        g.params = g.file;
                        edited = true;
                    }
                }
                if ui.button(tr(lang, "Fit to window")).clicked() {
                    self.transform = None;
                }
                if ui.button(tr(lang, "Histogram")).clicked() {
                    self.open_histogram = true;
                }
                if ui.button(tr(lang, "Zoom map")).clicked() {
                    self.open_zoom_map = true;
                }
                ui.weak(tr(
                    lang,
                    "Changed values are only shown, never written to the file. Closing the window goes back to the file values.",
                ));
            });
        });
        let kind = match &self.loaded {
            Some(Loaded::Gray(_)) => Kind::Gray,
            Some(Loaded::Color(_)) => Kind::Color,
            Some(Loaded::Reading(_)) => {
                ui.centered_and_justified(|ui| ui.spinner());
                return edited;
            }
            Some(Loaded::Failed(e)) => {
                let e = e.clone();
                ui.centered_and_justified(|ui| ui.label(e));
                return edited;
            }
            None => {
                ui.centered_and_justified(|ui| ui.weak(tr(lang, "No image")));
                return edited;
            }
        };
        let flow: &[Node] = match kind {
            Kind::Gray => &GRAY_FLOW,
            Kind::Color => &COLOR_FLOW,
        };
        let groups: &[Group] = match kind {
            Kind::Gray => &GRAY_GROUPS,
            Kind::Color => &[],
        };
        edited |= self.canvas(ui, lang, kind, flow, groups, marks);
        edited
    }

    /// 無限キャンバス。背景のドラッグでパン、ホイールでカーソル位置を中心にズームする。
    ///
    /// ズームは egui の層の変換（`set_transform_layer`、egui の `Scene`）では行わない。
    /// 層の変換は描いた後の形を縮めるだけなので、縮小すると「-」や「.」のような細い字が
    /// 消え、-1024 が 1024 に見えてしまう。代わりに文字の大きさと余白を倍率に合わせた
    /// スタイルで描き、字をその大きさで描き直す。
    #[allow(clippy::too_many_arguments)]
    fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        kind: Kind,
        flow: &[Node],
        groups: &[Group],
        marks: &[(u32, u32)],
    ) -> bool {
        let (outer, background) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let content = flow
            .iter()
            .filter_map(|n| self.rects.get(n))
            .fold(Rect::NOTHING, |a, r| a.union(*r));
        let mut transform = match self.transform {
            Some(t) => t,
            None if content.is_positive() => fit(outer, content.expand(40.0)),
            None => TSTransform::from_translation(outer.min.to_vec2() + vec2(30.0, 60.0)),
        };
        if background.dragged() {
            transform.translation += background.drag_delta();
        }
        if let Some(pointer) = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| outer.contains(*p))
        {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
            if scroll != 0.0 {
                let factor = (scroll / 300.0).exp();
                let scale = quantize((transform.scaling * factor).clamp(0.4, 2.5));
                let at = transform.inverse() * pointer;
                transform.scaling = scale;
                transform.translation = pointer.to_vec2() - at.to_vec2() * scale;
            }
        }
        if content.is_positive() {
            self.transform = Some(transform);
        }
        let scale = transform.scaling;
        self.scale = scale;
        let painter = ui.painter_at(outer);
        draw_grid(&painter, outer, transform, ui.visuals());

        let mut canvas = ui.new_child(UiBuilder::new().max_rect(outer));
        canvas.set_clip_rect(outer);
        *canvas.style_mut() = scaled_style(ui.style(), scale);

        let visuals = canvas.visuals().clone();
        let hairline = visuals.widgets.noninteractive.bg_stroke;
        let wire = Stroke::new(1.5, visuals.weak_text_color().gamma_multiply(0.8));
        // 枠と線は前のフレームのノードの位置で描く（今のフレームのノードはまだ並べていないため）。
        // 先に描くので、ノードの後ろに見える。
        for group in groups {
            let rect = group
                .nodes
                .iter()
                .filter_map(|n| self.rects.get(n))
                .fold(Rect::NOTHING, |a, r| a.union(*r));
            if !rect.is_positive() {
                continue;
            }
            let rect =
                transform.mul_rect(rect.expand2(vec2(10.0, 10.0)).with_min_y(rect.min.y - 26.0));
            painter.rect_stroke(rect, 3.0, hairline, egui::StrokeKind::Inside);
            painter.text(
                rect.min + vec2(8.0, 5.0) * scale,
                egui::Align2::LEFT_TOP,
                tr(lang, group.title),
                egui::FontId::proportional(quantize(11.0 * scale)),
                visuals.weak_text_color(),
            );
        }
        let mut ports = Vec::new();
        for pair in flow.windows(2) {
            let (Some(a), Some(b)) = (self.rects.get(&pair[0]), self.rects.get(&pair[1])) else {
                continue;
            };
            let from = transform * port(*a, true);
            let to = transform * port(*b, false);
            let dx = ((to.x - from.x) * 0.5).max(30.0 * scale);
            painter.add(CubicBezierShape::from_points_stroke(
                [from, from + vec2(dx, 0.0), to - vec2(dx, 0.0), to],
                false,
                Color32::TRANSPARENT,
                wire,
            ));
            ports.extend([from, to]);
        }

        let mut edited = false;
        for (i, &node) in flow.iter().enumerate() {
            let default = pos2(i as f32 * (NODE_WIDTH + NODE_GAP), 0.0);
            let pos = *self.positions.get(&(kind, node)).unwrap_or(&default);
            let (rect, moved, body_edited) =
                self.node(&mut canvas, lang, node, transform * pos, marks);
            edited |= body_edited;
            self.rects.insert(node, transform.inverse().mul_rect(rect));
            if moved != Vec2::ZERO {
                self.positions.insert((kind, node), pos + moved / scale);
            }
        }
        // 丸はノードの枠の上に重ねる。
        for p in ports {
            canvas.painter().circle_filled(p, 4.0 * scale, wire.color);
        }
        edited
    }

    /// ノード 1 つ。`pos` は画面上の左上。返すのは描いた範囲（画面上）、見出しのドラッグで
    /// 動かした量（画面上）、値を変えたか。
    fn node(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        node: Node,
        pos: Pos2,
        marks: &[(u32, u32)],
    ) -> (Rect, Vec2, bool) {
        let visuals = ui.visuals().clone();
        let scale = self.scale;
        let mut edited = false;
        let mut moved = Vec2::ZERO;
        let collapsed = self.collapsed.contains(&node);
        let mut toggle = false;
        let width = NODE_WIDTH * scale;
        let area = Rect::from_min_size(pos, vec2(width, 4000.0 * scale));
        let margin = egui::Margin::symmetric((8.0 * scale) as i8, (6.0 * scale) as i8);
        let inner = ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
            egui::Frame::new()
                .fill(visuals.panel_fill)
                .stroke(visuals.widgets.noninteractive.bg_stroke)
                .corner_radius(3.0)
                .inner_margin(margin)
                .show(ui, |ui| {
                    ui.set_width(width - margin.sum().x);
                    let header = ui.horizontal(|ui| {
                        let arrow = if collapsed {
                            icon::CARET_RIGHT
                        } else {
                            icon::CARET_DOWN
                        };
                        if ui.small_button(arrow).clicked() {
                            toggle = true;
                        }
                        ui.label(tr(lang, node_title(node)));
                    });
                    let drag = ui.interact(
                        header.response.rect,
                        ui.id().with(("node_drag", node)),
                        Sense::drag(),
                    );
                    moved = drag.drag_delta();
                    if !collapsed {
                        ui.separator();
                        edited = self.body(ui, lang, node, marks);
                    }
                })
        });
        if toggle {
            if collapsed {
                self.collapsed.remove(&node);
            } else {
                self.collapsed.insert(node);
            }
        }
        (inner.response.rect, moved, edited)
    }

    fn body(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        node: Node,
        marks: &[(u32, u32)],
    ) -> bool {
        let traces = self.traces(marks);
        match &mut self.loaded {
            Some(Loaded::Gray(g)) => gray_body(ui, lang, node, g, &traces, self.scale),
            Some(Loaded::Color(c)) => {
                color_body(ui, lang, node, c);
                false
            }
            _ => false,
        }
    }

    /// 十字の位置の、各段の値。
    fn traces(&self, marks: &[(u32, u32)]) -> Vec<(u32, u32, Trace)> {
        let Some(Loaded::Gray(g)) = &self.loaded else {
            return Vec::new();
        };
        let Some(image) = &g.image else {
            return Vec::new();
        };
        marks
            .iter()
            .take(3)
            .filter_map(|&(x, y)| {
                Some((
                    x,
                    y,
                    pixel_pipeline::trace(image, &g.decoded, &g.params, x, y)?,
                ))
            })
            .collect()
    }
}

/// `content`（キャンバスの座標）が `outer` に収まる変換。等倍より大きくはしない。
fn fit(outer: Rect, content: Rect) -> TSTransform {
    let scale = quantize((outer.size() / content.size()).min_elem().clamp(0.4, 1.0));
    TSTransform::from_translation(outer.center().to_vec2() - scale * content.center().to_vec2())
        * TSTransform::from_scaling(scale)
}

/// 倍率を 1/16 刻みにする。字は大きさごとにフォントの表へ描かれるので、
/// 連続した倍率のまま描くと表が際限なく増える。
fn quantize(scale: f32) -> f32 {
    (scale * 16.0).round() / 16.0
}

/// `scale` 倍で描くためのスタイル。字の大きさ・余白・部品の大きさを倍率に合わせる。
fn scaled_style(style: &egui::Style, scale: f32) -> egui::Style {
    let mut style = style.clone();
    for font in style.text_styles.values_mut() {
        font.size = quantize(font.size * scale);
    }
    let s = &mut style.spacing;
    s.item_spacing *= scale;
    s.button_padding *= scale;
    s.interact_size *= scale;
    s.indent *= scale;
    s.icon_width *= scale;
    s.icon_width_inner *= scale;
    s.icon_spacing *= scale;
    s.combo_width *= scale;
    s.slider_width *= scale;
    style
}

fn placeholder() -> Params {
    Params {
        slope: 1.0,
        intercept: 0.0,
        center: 0.0,
        width: 1.0,
        function: VoiFunction::Linear,
        invert: false,
    }
}

fn node_title(node: Node) -> &'static str {
    match node {
        Node::Stored => "Stored values",
        Node::Modality => "Modality LUT",
        Node::Unit => "Unit",
        Node::Voi => "VOI LUT",
        Node::Presentation => "Presentation LUT",
        Node::ColorSpace => "Color space",
        Node::Display => "Display",
    }
}

/// ノードの入力（左端）か出力（右端）の丸の位置。見出しの高さに置く。
fn port(rect: Rect, output: bool) -> Pos2 {
    let x = if output { rect.max.x } else { rect.min.x };
    pos2(x, rect.min.y + 15.0)
}

/// 目立たない細い点のグリッド。
fn draw_grid(painter: &egui::Painter, outer: Rect, t: TSTransform, visuals: &egui::Visuals) {
    let step = 40.0 * t.scaling;
    if step < 12.0 {
        return;
    }
    let color = visuals.weak_text_color().gamma_multiply(0.25);
    let start = t.translation.to_pos2();
    let first = |o: f32, s: f32| o + ((s - o) / step).floor() * step;
    let mut y = first(start.y, outer.min.y);
    while y < outer.max.y {
        let mut x = first(start.x, outer.min.x);
        while x < outer.max.x {
            painter.circle_filled(pos2(x, y), 0.8, color);
            x += step;
        }
        y += step;
    }
}

fn origin_text(lang: Language, origin: Origin) -> &'static str {
    match origin {
        Origin::File => tr(lang, "from the file"),
        Origin::FunctionalGroup => tr(lang, "per frame"),
        Origin::Default => tr(lang, "default (no tag)"),
    }
}

/// 手で変えられる数値の欄。ファイルの値と違えばアクセント色の印を付ける。
fn number_field(
    ui: &mut egui::Ui,
    lang: Language,
    (name, tag): (&str, Tag),
    value: &mut f64,
    file: f64,
    origin: Origin,
    speed: f64,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(name);
        ui.weak(format!("({:04X},{:04X})", tag.group(), tag.element()));
    });
    let mut changed = false;
    ui.horizontal(|ui| {
        changed = ui
            .add(egui::DragValue::new(value).speed(speed).max_decimals(4))
            .changed();
        // 「変更あり」は値がファイルの値と完全に一致しないこと。入力欄に戻せば同じ値になる。
        if *value != file {
            ui.colored_label(ui.visuals().selection.stroke.color, tr(lang, "changed"));
            if ui
                .small_button("↺")
                .on_hover_text(tr(lang, "Back to the file value"))
                .clicked()
            {
                *value = file;
                changed = true;
            }
        } else {
            ui.weak(origin_text(lang, origin));
        }
    });
    changed
}

fn gray_body(
    ui: &mut egui::Ui,
    lang: Language,
    node: Node,
    g: &mut GrayState,
    traces: &[(u32, u32, Trace)],
    scale: f32,
) -> bool {
    let info = &g.info;
    let mut edited = false;
    let file = g.file;
    let p = &mut g.params;
    match node {
        Node::Stored => {
            let s = &info.stored;
            ui.label(format!(
                "{} {} / {} {} / {} {}",
                tr(lang, "Allocated"),
                s.bits_allocated,
                tr(lang, "stored"),
                s.bits_stored,
                tr(lang, "high bit"),
                s.high_bit
            ));
            ui.label(if s.signed {
                tr(lang, "Signed integers")
            } else {
                tr(lang, "Unsigned integers")
            });
            ui.weak(format!("Photometric {}", s.photometric));
            for (x, y, t) in traces {
                ui.monospace(format!("({x}, {y})  {}", number(t.stored)));
            }
        }
        Node::Modality => {
            let m = &info.modality;
            if m.lut_sequence {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    tr(lang, "The file has a Modality LUT Sequence, which vw does not apply yet. The slope and intercept below are used instead."),
                );
            }
            edited |= number_field(
                ui,
                lang,
                ("Rescale Slope", m.slope.tag),
                &mut p.slope,
                file.slope,
                m.slope.origin,
                0.001,
            );
            edited |= number_field(
                ui,
                lang,
                ("Rescale Intercept", m.intercept.tag),
                &mut p.intercept,
                file.intercept,
                m.intercept.origin,
                1.0,
            );
            if let Some(t) = &m.rescale_type {
                ui.weak(format!("Rescale Type {t}"));
            }
            ui.weak(tr(lang, "stored × slope + intercept"));
        }
        Node::Unit => {
            let unit = pixel_pipeline::unit_of(info);
            ui.label(unit.unwrap_or_else(|| tr(lang, "No unit").to_owned()));
            if let Some(m) = &info.modality.modality {
                ui.weak(format!("Modality {m}"));
            }
            for (x, y, t) in traces {
                ui.monospace(format!("({x}, {y})  {}", number(t.modality)));
            }
        }
        Node::Voi => {
            let v = &info.voi;
            if v.lut_sequence {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    tr(lang, "The file has a VOI LUT Sequence, which vw does not apply yet. The window below is used instead."),
                );
            }
            let (c_origin, w_origin) = match (&v.center, &v.width) {
                (Some(c), Some(w)) => (c.origin, w.origin),
                _ => (Origin::Default, Origin::Default),
            };
            let (ctag, wtag) = (tags::WINDOW_CENTER, tags::WINDOW_WIDTH);
            edited |= number_field(
                ui,
                lang,
                ("Window Center", ctag),
                &mut p.center,
                file.center,
                c_origin,
                1.0,
            );
            edited |= number_field(
                ui,
                lang,
                ("Window Width", wtag),
                &mut p.width,
                file.width,
                w_origin,
                1.0,
            );
            if v.center.is_none() {
                ui.weak(tr(
                    lang,
                    "No window in the file: the full range of values is used.",
                ));
            }
            if v.windows > 1 {
                ui.weak(
                    tr_text(lang, "The file has several windows; the first is used.")
                        + &v.explanation
                            .as_ref()
                            .map(|e| format!(" ({e})"))
                            .unwrap_or_default(),
                );
            }
            ui.horizontal(|ui| {
                ui.label("VOI LUT Function");
                egui::ComboBox::from_id_salt("voi_function")
                    .selected_text(p.function.dicom_name())
                    .show_ui(ui, |ui| {
                        for f in VoiFunction::ALL {
                            edited |= ui
                                .selectable_value(&mut p.function, f, f.dicom_name())
                                .changed();
                        }
                    });
            });
            ui.weak(origin_text(lang, v.function.origin));
            edited |= voi_graph(ui, p, g.histogram.as_ref(), scale);
            ui.weak(tr(
                lang,
                "Drag the graph: sideways moves the center, up and down changes the width.",
            ));
        }
        Node::Presentation => {
            if let Some(shape) = &info.presentation.shape {
                ui.weak(format!("Presentation LUT Shape {shape}"));
            }
            ui.weak(format!("Photometric {}", info.stored.photometric));
            ui.horizontal(|ui| {
                edited |= ui
                    .checkbox(&mut p.invert, tr(lang, "Invert (MONOCHROME1)"))
                    .changed();
                if p.invert != file.invert {
                    ui.colored_label(ui.visuals().selection.stroke.color, tr(lang, "changed"));
                }
            });
        }
        Node::Display => {
            ui.label(tr(lang, "8-bit gray"));
            for (x, y, t) in traces {
                ui.monospace(format!("({x}, {y})  {}", t.display));
            }
            if traces.is_empty() {
                ui.weak(tr(
                    lang,
                    "Put a probe cross on the image to see its values at each step.",
                ));
            }
        }
        Node::ColorSpace => {}
    }
    edited
}

fn color_body(ui: &mut egui::Ui, lang: Language, node: Node, c: &Color) {
    let s = &c.stored;
    match node {
        Node::Stored => {
            ui.label(format!("{} × {} bit", s.samples_per_pixel, s.bits_stored));
            ui.weak(format!("Photometric {}", s.photometric));
            if let Some(p) = s.planar_configuration {
                ui.weak(format!("Planar Configuration {p}"));
            }
        }
        Node::ColorSpace => {
            ui.label(match c.conversion {
                "none" => tr(lang, "Already RGB"),
                "palette" => tr(lang, "Palette lookup"),
                "ybr_to_rgb" => tr(lang, "YCbCr to RGB"),
                _ => tr(lang, "Unknown"),
            });
        }
        Node::Display => {
            ui.label(tr(lang, "8-bit RGB"));
        }
        _ => {}
    }
}

/// VOI の曲線。後ろに物理量のヒストグラムを出す。ドラッグで中心と幅を変える。
fn voi_graph(ui: &mut egui::Ui, p: &mut Params, histogram: Option<&Histogram>, scale: f32) -> bool {
    let size = vec2(ui.available_width(), 110.0 * scale);
    let (rect, response) = ui.allocate_exact_size(size, Sense::drag());
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    let (lo, hi) = {
        let (a, b) = histogram.map_or((p.center - p.width, p.center + p.width), |h| (h.lo, h.hi));
        let (a, b) = (
            a.min(p.center - p.width / 2.0),
            b.max(p.center + p.width / 2.0),
        );
        if b > a {
            (a, b)
        } else {
            (a - 1.0, a + 1.0)
        }
    };
    let x_of = |v: f64| rect.min.x + ((v - lo) / (hi - lo)) as f32 * rect.width();
    if let Some(h) = histogram {
        let peak = h.bins.iter().copied().max().unwrap_or(1).max(1) as f32;
        let color = visuals.weak_text_color().gamma_multiply(0.5);
        for (i, &n) in h.bins.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let v = h.lo + (h.hi - h.lo) * i as f64 / (h.bins.len() - 1).max(1) as f64;
            let x = x_of(v);
            // 少ない値も見えるよう、高さは対数にする。
            let height = ((n as f32).ln_1p() / peak.ln_1p()) * rect.height();
            painter.line_segment(
                [pos2(x, rect.max.y), pos2(x, rect.max.y - height)],
                Stroke::new(1.0, color),
            );
        }
    }
    let accent = visuals.selection.stroke.color;
    let points: Vec<Pos2> = (0..=128)
        .map(|i| {
            let v = lo + (hi - lo) * f64::from(i) / 128.0;
            let y = p.function.apply(v, p.center, p.width);
            let y = if p.invert { 1.0 - y } else { y };
            pos2(x_of(v), rect.max.y - y as f32 * rect.height())
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.5, accent)));
    let cx = x_of(p.center);
    painter.line_segment(
        [pos2(cx, rect.min.y), pos2(cx, rect.max.y)],
        Stroke::new(1.0, accent.gamma_multiply(0.4)),
    );
    if response.dragged() {
        let d = response.drag_delta();
        let per_px = (hi - lo) / f64::from(rect.width());
        p.center += f64::from(d.x) * per_px;
        p.width = (p.width + f64::from(d.y) * per_px).max(1.0);
        return d != Vec2::ZERO;
    }
    false
}

fn number(v: f64) -> String {
    if (v - v.round()).abs() < 0.005 {
        format!("{}", v.round())
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_nodes_come_back_at_the_same_place_per_kind() {
        let mut view = PipelineView::default();
        view.positions
            .insert((Kind::Gray, Node::Voi), pos2(560.0, -20.0));
        view.positions
            .insert((Kind::Color, Node::Display), pos2(10.0, 5.0));
        let saved = view.node_positions();
        assert_eq!(saved["gray.voi"], [560.0, -20.0]);

        let mut next = PipelineView::default();
        next.restore_node_positions(&saved);
        assert_eq!(next.positions, view.positions);
    }

    #[test]
    fn closing_restores_the_image_that_had_changed_values() {
        let mut view = PipelineView {
            open: true,
            applied: Some(PathBuf::from("a.dcm")),
            ..Default::default()
        };
        let changes = view.close();
        assert!(!view.open);
        assert!(matches!(changes.as_slice(), [Change::Restore(p)] if p == Path::new("a.dcm")));
    }
}
