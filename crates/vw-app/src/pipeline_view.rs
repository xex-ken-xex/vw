//! 画素の変換の図。格納値が表示の明るさになるまでの段をノードと線で並べた別ウインドウで、
//! 値を変えるとアクティブなスロットの色導出にする（ファイルには書き戻さない）。
//! 描き直しはスロットの側（[`crate::derive`]）が受け持つ。
//!
//! ファイルの段（形式で決まる）の後ろに、自由なノード（式・レベル・反転・混合）を 1 本の
//! 鎖でつなぐ。鎖は標準の表示の 0〜1 の値に当てるので、どの形式の画像にも使える。
//!
//! 組み立てと操作はノードエディタ（ComfyUI）に、見た目は vw のパネルに合わせる。
//! ノードエディタのクレート（egui-snarl など）を使わないのは、線のつなぎ替えがなく
//! 決まった 1 本の流れを描くだけで、見た目を vw のテーマに揃える手間の方が大きいため。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui::{
    self, emath::TSTransform, epaint::CubicBezierShape, pos2, vec2, Color32, Pos2, Rect, Sense,
    Stroke, UiBuilder, Vec2,
};
use vw_core::VwImage;
use vw_dicom::pipeline::{tags, Color, Gray, Origin, Pipeline, Tag};

use egui_phosphor::regular as icon;

use crate::i18n::{tr, tr_text};
use crate::loader::ImageInfo;
use crate::nodes::{self, Chain};
use crate::pipelines::Values;
use crate::pixel_pipeline::{self, Decoded, Histogram, Params, Trace, VoiFunction};
use crate::settings::Language;

/// ウインドウに渡す、アクティブなスロットの画像。
pub struct Target<'a> {
    /// 表示中の画像のパス（フレームならフレームのパス）。
    pub path: &'a Path,
    /// フル解像度の画像。まだ読み込んでいなければ `None`。
    pub image: Option<&'a Arc<VwImage>>,
    pub info: &'a ImageInfo,
    /// その画像に置いた十字の位置。
    pub marks: Vec<(u32, u32)>,
    /// 定量評価の結果と判定の閾値。
    pub evaluation: Option<(
        Option<&'a crate::quality::Report>,
        &'a crate::settings::Quality,
    )>,
    /// スロットの色導出の値。`None` は標準（ファイルの値、ノードなし）。
    pub values: Option<&'a Values>,
    /// スロットで選んでいる保存したパイプラインの名前と、そのあと値を変えたか。
    pub preset: Option<(&'a str, bool)>,
    /// 保存したパイプラインの名前（同じ名前で保存しないように確かめる）。
    pub saved: Vec<&'a str>,
}

/// ウインドウでの操作。スロットと保存したパイプラインの一覧に当てる。
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// アクティブなスロットの色導出の値を変えた。
    Edit(Values),
    SaveAs(String, Values),
    Overwrite(String, Values),
    Delete(String),
    /// ウインドウを閉じた。保存していない値は捨てる。
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Node {
    Stored,
    Modality,
    Unit,
    Voi,
    Presentation,
    ColorSpace,
    /// 自由なノードの鎖の `i` 番目。
    Chain(usize),
    Display,
}

/// 関係するノードをまとめる枠。
struct Group {
    title: &'static str,
    nodes: Vec<Node>,
}

const GRAY_FILE: [Node; 5] = [
    Node::Stored,
    Node::Modality,
    Node::Unit,
    Node::Voi,
    Node::Presentation,
];
const COLOR_FILE: [Node; 2] = [Node::Stored, Node::ColorSpace];
const PLAIN_FILE: [Node; 1] = [Node::Stored];

/// ノードの配置を形式ごとに分けて覚えるための鍵。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Gray,
    Color,
    /// DICOM 以外。ファイルの段は 1 つにまとめて出す。
    Plain,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Gray => "gray",
            Kind::Color => "color",
            Kind::Plain => "plain",
        }
    }

    fn file_nodes(self) -> &'static [Node] {
        match self {
            Kind::Gray => &GRAY_FILE,
            Kind::Color => &COLOR_FILE,
            Kind::Plain => &PLAIN_FILE,
        }
    }

    fn groups(self, chain_len: usize) -> Vec<Group> {
        let mut groups = match self {
            Kind::Gray => vec![
                Group {
                    title: "Modality",
                    nodes: vec![Node::Modality, Node::Unit],
                },
                Group {
                    title: "VOI",
                    nodes: vec![Node::Voi],
                },
                Group {
                    title: "Presentation",
                    nodes: vec![Node::Presentation],
                },
            ],
            Kind::Color | Kind::Plain => Vec::new(),
        };
        if chain_len > 0 {
            groups.push(Group {
                title: "Nodes",
                nodes: (0..chain_len).map(Node::Chain).collect(),
            });
        }
        groups
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
            Node::Chain(_) => "chain",
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
    /// DICOM 以外。ファイルの段は変えられず、ノードの鎖だけを当てる。
    Plain,
    Failed(String),
}

impl Loaded {
    fn kind(&self) -> Option<Kind> {
        match self {
            Loaded::Gray(_) => Some(Kind::Gray),
            Loaded::Color(_) => Some(Kind::Color),
            Loaded::Plain => Some(Kind::Plain),
            Loaded::Reading(_) | Loaded::Failed(_) => None,
        }
    }
}

#[derive(Default)]
pub struct PipelineView {
    pub open: bool,
    /// 見出しのボタンでヒストグラム・ズームマップを開くよう頼まれた。
    pub open_histogram: bool,
    pub open_zoom_map: bool,
    path: Option<PathBuf>,
    loaded: Option<Loaded>,
    /// 自由なノードの鎖。毎フレーム、スロットの色導出の値から写す。
    chain: Chain,
    /// ダブルクリックで開いた、ノードを探して足す小窓の位置（キャンバスの座標）と、絞り込みの文字。
    add_at: Option<Pos2>,
    add_filter: String,
    /// 「名前を付けて保存」で入力中の名前。
    save_name: String,
    /// 「名前を付けて保存」を開いた。次のフレームで名前の欄に入力できるようにする
    /// （欄は開いた次のフレームに作られるので、押したフレームでは選べない）。
    focus_name: bool,
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
            .filter(|((_, node), _)| !matches!(node, Node::Chain(_)))
            .map(|((kind, node), p)| (format!("{}.{}", kind.key(), node.key()), [p.x, p.y]))
            .collect()
    }

    /// 前回動かしたノードの位置に戻す。知らない鍵は捨てる。
    pub fn restore_node_positions(&mut self, saved: &BTreeMap<String, [f32; 2]>) {
        for kind in [Kind::Gray, Kind::Color, Kind::Plain] {
            for node in Node::ALL {
                if let Some([x, y]) = saved.get(&format!("{}.{}", kind.key(), node.key())) {
                    self.positions.insert((kind, node), pos2(*x, *y));
                }
            }
        }
    }

    /// 閉じる。保存していない値はスロットの側で捨てる。
    pub fn close(&mut self) -> Vec<Command> {
        self.open = false;
        self.path = None;
        self.loaded = None;
        self.chain.clear();
        self.add_at = None;
        vec![Command::Closed]
    }

    /// 1 フレーム分。ウインドウを描き、スロットと保存したパイプラインに当てる操作を返す。
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        lang: Language,
        target: Option<Target<'_>>,
    ) -> Vec<Command> {
        if !self.open {
            return Vec::new();
        }
        self.follow(ctx, target.as_ref());
        let mut out = Vec::new();
        let title = crate::sub_window::title(lang, "Pixel pipeline", self.path.as_deref());
        let marks = target.as_ref().map(|t| t.marks.clone()).unwrap_or_default();
        let evaluation = target.as_ref().and_then(|t| t.evaluation);
        let preset = target.as_ref().and_then(|t| t.preset);
        let saved = target.as_ref().map(|t| t.saved.clone()).unwrap_or_default();
        let info = target.as_ref().map(|t| t.info);
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
                if let Some((report, thresholds)) = evaluation {
                    egui::Panel::bottom("pipeline_evaluation").show(ctx, |ui| {
                        crate::quality_view::summary(ui, lang, report, thresholds);
                    });
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(ctx.style().visuals.extreme_bg_color))
                    .show(ctx, |ui| {
                        edited = self.contents(ui, lang, &marks, preset, &saved, info, &mut out);
                    });
            },
        );
        if edited {
            if let Some(values) = self.values() {
                out.insert(0, Command::Edit(values));
            }
        }
        if close {
            out.extend(self.close());
        }
        out
    }

    /// 今の値（スロットの色導出に渡す形）。ファイルの段の値は、ファイルと違うときだけ持つ。
    fn values(&self) -> Option<Values> {
        let params = match self.loaded.as_ref()? {
            Loaded::Gray(g) => (g.params != g.file).then_some(g.params),
            Loaded::Color(_) | Loaded::Plain => None,
            Loaded::Reading(_) | Loaded::Failed(_) => return None,
        };
        Some(Values {
            params,
            chain: self.chain.clone(),
        })
    }

    /// アクティブなスロットの画像が変わったら、その画像の段を読み直す。
    /// 値はスロットの色導出に合わせる。
    fn follow(&mut self, ctx: &egui::Context, target: Option<&Target<'_>>) {
        let path = target.map(|t| t.path.to_path_buf());
        if path != self.path {
            self.path = path.clone();
            self.loaded = path.map(|p| match target.map(|t| t.info.plugin.as_str()) {
                Some("DICOM") => Loaded::Reading(pixel_pipeline::read_in_background(ctx, &p)),
                _ => Loaded::Plain,
            });
        }
        self.chain = target
            .and_then(|t| t.values)
            .map(|v| v.chain.clone())
            .unwrap_or_default();
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
        // フル解像度の画像が届いた、または捨てられて読み直されたら、ヒストグラムを作り直す。
        if let (Some(Loaded::Gray(g)), Some(image)) =
            (&mut self.loaded, target.and_then(|t| t.image))
        {
            let same = g.image.as_ref().is_some_and(|i| Arc::ptr_eq(i, image));
            if !same {
                g.image = Some(image.clone());
                g.file = pixel_pipeline::file_params(&g.info, image, &g.decoded);
                g.histogram = Some(pixel_pipeline::histogram(
                    image,
                    &g.decoded,
                    g.file.slope,
                    g.file.intercept,
                ));
            }
        }
        if let (Some(Loaded::Gray(g)), Some(t)) = (&mut self.loaded, target) {
            if g.image.is_some() {
                g.params = t.values.and_then(|v| v.params).unwrap_or(g.file);
            }
        }
    }

    /// 名前を付けて保存・上書き保存・削除。
    fn save_buttons(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        values: &Values,
        preset: Option<(&str, bool)>,
        saved: &[&str],
        out: &mut Vec<Command>,
    ) {
        let response = ui.button(tr(lang, "Save as..."));
        if response.clicked() {
            self.focus_name = true;
        }
        egui::Popup::from_toggle_button_response(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.label(tr(lang, "Name"));
                let field = ui.text_edit_singleline(&mut self.save_name);
                if std::mem::take(&mut self.focus_name) {
                    field.request_focus();
                }
                let name = self.save_name.trim();
                let problem = if name.is_empty() {
                    Some(tr(lang, "Enter a name."))
                } else if saved.contains(&name) {
                    Some(tr(
                        lang,
                        "That name is already saved. Use Overwrite to change it.",
                    ))
                } else {
                    None
                };
                if let Some(p) = problem {
                    ui.weak(p);
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let save = ui.add_enabled(problem.is_none(), egui::Button::new(tr(lang, "Save")));
                if problem.is_none() && (save.clicked() || enter) {
                    out.push(Command::SaveAs(name.to_owned(), values.clone()));
                    self.save_name.clear();
                    ui.close();
                }
            });
        let name = preset.map(|(n, _)| n);
        let modified = preset.is_some_and(|(_, m)| m);
        let overwrite = ui
            .add_enabled(modified, egui::Button::new(tr(lang, "Overwrite")))
            .on_hover_text(match name {
                Some(n) => crate::i18n::tr1(lang, "Save the changed values as \"{}\"", n),
                None => tr(lang, "Choose a saved pipeline in the slot first.").to_owned(),
            });
        if let (true, Some(n)) = (overwrite.clicked(), name) {
            out.push(Command::Overwrite(n.to_owned(), values.clone()));
        }
        ui.add_enabled_ui(name.is_some(), |ui| {
            ui.menu_button(tr(lang, "Delete"), |ui| {
                if let Some(n) = name {
                    if ui
                        .button(crate::i18n::tr1(
                            lang,
                            "Delete the saved pipeline \"{}\"",
                            n,
                        ))
                        .clicked()
                    {
                        out.push(Command::Delete(n.to_owned()));
                        ui.close();
                    }
                }
            });
        });
    }

    /// 図の中身。値を変えたら `true`。保存や削除は `out` に足す。
    #[allow(clippy::too_many_arguments)]
    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        marks: &[(u32, u32)],
        preset: Option<(&str, bool)>,
        saved: &[&str],
        info: Option<&ImageInfo>,
        out: &mut Vec<Command>,
    ) -> bool {
        let mut edited = false;
        let values = self.values();
        egui::Panel::top("pipeline_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(values) = &values {
                    self.save_buttons(ui, lang, values, preset, saved, out);
                    ui.separator();
                }
                let can_reset = values.as_ref().is_some_and(|v| !v.is_standard());
                if ui
                    .add_enabled(can_reset, egui::Button::new(tr(lang, "Reset to file values")))
                    .clicked()
                {
                    if let Some(Loaded::Gray(g)) = &mut self.loaded {
                        g.params = g.file;
                    }
                    self.chain.clear();
                    edited = true;
                }
                if ui.button(tr(lang, "Fit to window")).clicked() {
                    self.transform = None;
                }
                if ui.button(tr(lang, "Analysis")).clicked() {
                    self.open_histogram = true;
                }
                if ui.button(tr(lang, "Zoom map")).clicked() {
                    self.open_zoom_map = true;
                }
                ui.weak(tr(
                    lang,
                    "Changed values are only shown, never written to the file. Closing the window drops values that are not saved.",
                ));
            });
        });
        let kind = match &self.loaded {
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
            Some(loaded) => loaded.kind().expect("loaded kinds are matched above"),
        };
        let flow: Vec<Node> = kind
            .file_nodes()
            .iter()
            .copied()
            .chain((0..self.chain.len()).map(Node::Chain))
            .chain([Node::Display])
            .collect();
        let groups = kind.groups(self.chain.len());
        let Some(info) = info else {
            return edited;
        };
        edited |= self.canvas(ui, lang, kind, &flow, &groups, marks, info);
        edited
    }

    /// 鎖の末尾（表示の前）にノードを足す。
    fn add_node(&mut self, kind: &str) {
        if let Some(node) = nodes::Node::new(kind) {
            self.chain.push(node);
        }
    }

    /// キャンバスの背景の右クリックメニューと、ダブルクリックで開く検索の小窓。足したら `true`。
    fn add_menus(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        background: &egui::Response,
    ) -> bool {
        let mut added = false;
        background.context_menu(|ui| {
            ui.menu_button(tr(lang, "Add node"), |ui| {
                for kind in nodes::Node::KINDS {
                    if ui.button(tr(lang, nodes::Node::title(kind))).clicked() {
                        self.add_node(kind);
                        added = true;
                        ui.close();
                    }
                }
            });
        });
        if background.double_clicked() {
            self.add_at = background.interact_pointer_pos();
            self.add_filter.clear();
        }
        let Some(at) = self.add_at else {
            return added;
        };
        let mut open = true;
        egui::Window::new(tr(lang, "Search nodes"))
            .id(ui.id().with("add_node_search"))
            .fixed_pos(at)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                let field = ui.text_edit_singleline(&mut self.add_filter);
                if self.add_filter.is_empty() && !field.has_focus() {
                    field.request_focus();
                }
                let filter = self.add_filter.trim().to_lowercase();
                let matching: Vec<&str> = nodes::Node::KINDS
                    .into_iter()
                    .filter(|k| {
                        let title = tr(lang, nodes::Node::title(k));
                        filter.is_empty()
                            || title.to_lowercase().contains(&filter)
                            || k.contains(&filter)
                    })
                    .collect();
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                for (i, kind) in matching.iter().enumerate() {
                    if ui.button(tr(lang, nodes::Node::title(kind))).clicked() || (enter && i == 0)
                    {
                        self.add_node(kind);
                        added = true;
                    }
                }
                if matching.is_empty() {
                    ui.weak(tr(lang, "No node matches."));
                }
            });
        if added || !open || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.add_at = None;
        }
        added
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
        info: &ImageInfo,
    ) -> bool {
        let (outer, background) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let mut edited = self.add_menus(ui, lang, &background);
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

        // 鎖のノードは並び順で置く（足す・外す・入れ替えるたびに番号が変わるので、覚えない）。
        // 表示のノードは覚えた位置を鎖の長さの分だけ右にずらす。
        let chain_shift = vec2(self.chain.len() as f32 * (NODE_WIDTH + NODE_GAP), 0.0);
        let mut chain_edit: Option<(usize, ChainEdit)> = None;
        for (i, &node) in flow.iter().enumerate() {
            let default = pos2(i as f32 * (NODE_WIDTH + NODE_GAP), 0.0);
            let shift = match node {
                Node::Display => chain_shift,
                _ => Vec2::ZERO,
            };
            let pos = match node {
                Node::Chain(_) => default,
                _ => self
                    .positions
                    .get(&(kind, node))
                    .map_or(default, |p| *p + shift),
            };
            let (rect, moved, body_edited, chain) =
                self.node(&mut canvas, lang, node, transform * pos, marks, info);
            edited |= body_edited;
            chain_edit = chain_edit.or(chain.map(|c| (i, c)));
            self.rects.insert(node, transform.inverse().mul_rect(rect));
            if moved != Vec2::ZERO && !matches!(node, Node::Chain(_)) {
                self.positions
                    .insert((kind, node), pos + moved / scale - shift);
            }
        }
        if let Some((_, c)) = chain_edit {
            if let Some(Node::Chain(i)) = chain_edit.map(|(i, _)| flow[i]) {
                match c {
                    ChainEdit::Remove => {
                        self.chain.remove(i);
                    }
                    ChainEdit::Earlier if i > 0 => self.chain.swap(i, i - 1),
                    ChainEdit::Later if i + 1 < self.chain.len() => self.chain.swap(i, i + 1),
                    _ => {}
                }
                edited = true;
            }
        }
        // 丸はノードの枠の上に重ねる。
        for p in ports {
            canvas.painter().circle_filled(p, 4.0 * scale, wire.color);
        }
        edited
    }

    /// ノード 1 つ。`pos` は画面上の左上。返すのは描いた範囲（画面上）、見出しのドラッグで
    /// 動かした量（画面上）、値を変えたか、鎖のノードの見出しのボタンで頼まれたこと。
    fn node(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        node: Node,
        pos: Pos2,
        marks: &[(u32, u32)],
        info: &ImageInfo,
    ) -> (Rect, Vec2, bool, Option<ChainEdit>) {
        let visuals = ui.visuals().clone();
        let scale = self.scale;
        let mut edited = false;
        let mut chain_edit = None;
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
                    // 見出しのドラッグ判定は、見出しの部品より先に登録する。egui は後から登録した
                    // 判定を上に置くので、逆にすると折りたたみや × のボタンが押せなくなる。
                    let row = ui.spacing().interact_size.y;
                    let (header_rect, drag) =
                        ui.allocate_exact_size(vec2(ui.available_width(), row), Sense::drag());
                    moved = drag.drag_delta();
                    let mut header_ui = ui.new_child(
                        UiBuilder::new()
                            .id_salt(("node_header", node))
                            .max_rect(header_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    {
                        let ui = &mut header_ui;
                        let arrow = if collapsed {
                            icon::CARET_RIGHT
                        } else {
                            icon::CARET_DOWN
                        };
                        if ui.small_button(arrow).clicked() {
                            toggle = true;
                        }
                        ui.label(self.node_title(lang, node));
                        if let Node::Chain(i) = node {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button(icon::X)
                                        .on_hover_text(tr(lang, "Remove node"))
                                        .clicked()
                                    {
                                        chain_edit = Some(ChainEdit::Remove);
                                    }
                                    if ui
                                        .add_enabled(
                                            i + 1 < self.chain.len(),
                                            egui::Button::new(icon::ARROW_RIGHT).small(),
                                        )
                                        .on_hover_text(tr(lang, "Move later"))
                                        .clicked()
                                    {
                                        chain_edit = Some(ChainEdit::Later);
                                    }
                                    if ui
                                        .add_enabled(
                                            i > 0,
                                            egui::Button::new(icon::ARROW_LEFT).small(),
                                        )
                                        .on_hover_text(tr(lang, "Move earlier"))
                                        .clicked()
                                    {
                                        chain_edit = Some(ChainEdit::Earlier);
                                    }
                                },
                            );
                        }
                    }
                    if !collapsed {
                        ui.separator();
                        edited = self.body(ui, lang, node, marks, info);
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
        (inner.response.rect, moved, edited, chain_edit)
    }

    fn node_title(&self, lang: Language, node: Node) -> String {
        match node {
            Node::Chain(i) => {
                let kind = self.chain.get(i).map_or("", nodes::Node::kind);
                format!("{}  {}", i + 1, tr(lang, nodes::Node::title(kind)))
            }
            Node::Display => match &self.loaded {
                Some(Loaded::Plain) | Some(Loaded::Color(_)) => tr(lang, "Display").to_owned(),
                _ => tr(lang, node_title(node)).to_owned(),
            },
            _ => tr(lang, node_title(node)).to_owned(),
        }
    }

    fn body(
        &mut self,
        ui: &mut egui::Ui,
        lang: Language,
        node: Node,
        marks: &[(u32, u32)],
        info: &ImageInfo,
    ) -> bool {
        if let Node::Chain(i) = node {
            return match self.chain.get_mut(i) {
                Some(n) => chain_body(ui, lang, n),
                None => false,
            };
        }
        let traces = self.traces(marks);
        match &mut self.loaded {
            Some(Loaded::Gray(g)) => gray_body(ui, lang, node, g, &traces, self.scale),
            Some(Loaded::Color(c)) => {
                color_body(ui, lang, node, c);
                false
            }
            Some(Loaded::Plain) => {
                plain_body(ui, lang, node, info);
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

/// 鎖のノードの見出しのボタンで頼まれたこと。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChainEdit {
    Remove,
    Earlier,
    Later,
}

fn node_title(node: Node) -> &'static str {
    match node {
        Node::Stored => "Stored values",
        Node::Modality => "Modality LUT",
        Node::Unit => "Unit",
        Node::Voi => "VOI LUT",
        Node::Presentation => "Presentation LUT",
        Node::ColorSpace => "Color space",
        Node::Chain(_) => "Node",
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
        Node::ColorSpace | Node::Chain(_) => {}
    }
    edited
}

/// DICOM 以外の形式。ファイルの段は変えられないので、画像の情報だけを出す。
fn plain_body(ui: &mut egui::Ui, lang: Language, node: Node, info: &ImageInfo) {
    match node {
        Node::Stored => {
            ui.label(&info.plugin);
            ui.label(format!(
                "{} × {}  {} bit  {}",
                info.width,
                info.height,
                info.significant_bits,
                color_name(lang, info.color)
            ));
            ui.weak(tr(lang, "This format has no editable file stages."));
        }
        Node::Display => {
            ui.label(if info.color.channels() >= 3 {
                tr(lang, "8-bit RGB")
            } else {
                tr(lang, "8-bit gray")
            });
            ui.weak(tr(
                lang,
                "Nodes work on the standard display values scaled to 0..1.",
            ));
        }
        _ => {}
    }
}

fn color_name(lang: Language, color: vw_core::ColorModel) -> &'static str {
    use vw_core::ColorModel;
    match color {
        ColorModel::Gray => tr(lang, "gray"),
        ColorModel::GrayAlpha => tr(lang, "gray + alpha"),
        ColorModel::Rgb => "RGB",
        ColorModel::Rgba => "RGBA",
    }
}

/// 自由なノードの中身。値を変えたら `true`。
fn chain_body(ui: &mut egui::Ui, lang: Language, node: &mut nodes::Node) -> bool {
    let mut edited = false;
    let unit = |ui: &mut egui::Ui, v: &mut f32, lo: f32, hi: f32, speed: f64| -> bool {
        ui.add(
            egui::DragValue::new(v)
                .range(lo..=hi)
                .speed(speed)
                .max_decimals(3),
        )
        .changed()
    };
    match node {
        nodes::Node::Expression { text } => {
            edited |= ui
                .add(
                    egui::TextEdit::multiline(text)
                        .code_editor()
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                )
                .changed();
            match nodes::check(node) {
                Some(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                None => {
                    ui.weak(tr(lang, "v = this channel, r g b a, x y w h (0..1)"));
                }
            }
            ui.weak(tr(
                lang,
                "One line for every channel, or lines like \"r: expr\".",
            ))
            .on_hover_text(expression_help(lang));
        }
        nodes::Node::Levels {
            black,
            white,
            gamma,
        } => {
            egui::Grid::new("levels").num_columns(2).show(ui, |ui| {
                ui.label(tr(lang, "Black point"));
                edited |= unit(ui, black, 0.0, 1.0, 0.002);
                ui.end_row();
                ui.label(tr(lang, "White point"));
                edited |= unit(ui, white, 0.0, 1.0, 0.002);
                ui.end_row();
                ui.label(tr(lang, "Gamma"));
                edited |= unit(ui, gamma, 0.05, 20.0, 0.01);
                ui.end_row();
            });
        }
        nodes::Node::Invert => {
            ui.weak("1 − v");
        }
        nodes::Node::Blur { radius } => {
            egui::Grid::new("blur").num_columns(2).show(ui, |ui| {
                ui.label(tr(lang, "Radius (px)"));
                edited |= unit(ui, radius, 0.0, 50.0, 0.05);
                ui.end_row();
            });
            ui.weak(tr(lang, "Gaussian; the radius is σ."));
        }
        nodes::Node::Noise { sigma, seed } => {
            egui::Grid::new("noise").num_columns(2).show(ui, |ui| {
                ui.label(tr(lang, "Sigma"));
                edited |= unit(ui, sigma, 0.0, 1.0, 0.001);
                ui.end_row();
                ui.label(tr(lang, "Seed"));
                edited |= ui.add(egui::DragValue::new(seed).speed(1.0)).changed();
                ui.end_row();
            });
            ui.weak(tr(lang, "Gaussian noise on the 0..1 values."));
        }
        nodes::Node::Median { size } => {
            ui.horizontal(|ui| {
                ui.label(tr(lang, "Window"));
                for n in [3u32, 5] {
                    edited |= ui.selectable_value(size, n, format!("{n} × {n}")).changed();
                }
            });
        }
        nodes::Node::Tone {
            brightness,
            contrast,
            curve,
        } => {
            egui::Grid::new("tone").num_columns(2).show(ui, |ui| {
                ui.label(tr(lang, "Brightness"));
                edited |= unit(ui, brightness, -1.0, 1.0, 0.002);
                ui.end_row();
                ui.label(tr(lang, "Contrast"));
                edited |= unit(ui, contrast, 0.0, 4.0, 0.005);
                ui.end_row();
                ui.label(tr(lang, "S-curve"));
                edited |= unit(ui, curve, -1.0, 1.0, 0.005);
                ui.end_row();
            });
        }
        nodes::Node::Mix { matrix } => {
            ui.weak(tr(lang, "Output = matrix × (r, g, b)"));
            egui::Grid::new("mix").num_columns(4).show(ui, |ui| {
                ui.label("");
                for c in ["r", "g", "b"] {
                    ui.weak(c);
                }
                ui.end_row();
                for (row, name) in matrix.iter_mut().zip(["r'", "g'", "b'"]) {
                    ui.weak(name);
                    for v in row {
                        edited |= unit(ui, v, -4.0, 4.0, 0.01);
                    }
                    ui.end_row();
                }
            });
        }
    }
    edited
}

fn expression_help(lang: Language) -> String {
    let functions: Vec<&str> = crate::expr::function_names().collect();
    format!(
        "{}\n{}\n{}: {}",
        tr(lang, "Variables: v (this channel), r, g, b, a, x, y (0..1), w, h (pixels). Constants: pi, e."),
        tr(lang, "Operators: + - * / % ^, comparisons, && || !, cond ? a : b. Lines starting with # are comments."),
        tr(lang, "Functions"),
        functions.join(" ")
    )
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
        view.positions
            .insert((Kind::Plain, Node::Stored), pos2(1.0, 2.0));
        let saved = view.node_positions();
        assert_eq!(saved["gray.voi"], [560.0, -20.0]);
        assert_eq!(saved["plain.stored"], [1.0, 2.0]);

        let mut next = PipelineView::default();
        next.restore_node_positions(&saved);
        assert_eq!(next.positions, view.positions);
    }

    #[test]
    fn chain_node_positions_are_not_remembered() {
        let mut view = PipelineView::default();
        view.positions
            .insert((Kind::Plain, Node::Chain(0)), pos2(1.0, 2.0));
        assert!(view.node_positions().is_empty());
    }

    #[test]
    fn values_keep_file_stage_values_only_when_they_differ_from_the_file() {
        let mut view = PipelineView {
            loaded: Some(Loaded::Plain),
            ..Default::default()
        };
        assert_eq!(view.values(), Some(Values::default()));
        view.add_node("invert");
        let values = view.values().unwrap();
        assert_eq!((values.params, values.chain.len()), (None, 1));
        view.loaded = Some(Loaded::Reading(std::sync::mpsc::channel().1));
        assert_eq!(view.values(), None);
    }

    #[test]
    fn closing_tells_the_slots_to_drop_unsaved_values() {
        let mut view = PipelineView {
            open: true,
            ..Default::default()
        };
        assert_eq!(view.close(), [Command::Closed]);
        assert!(!view.open);
    }
}
