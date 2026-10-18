//! ドッキング。ペインを左右・上下に分けて並べ、境目をドラッグして大きさを変え、
//! 見出しをドラッグして Visual Studio のようなガイドで別の場所へ入れ直し、
//! 本体の外の別ウインドウへ引き出したり戻したりする。
//!
//! ドラッグを egui のウィジェットの「ドラッグ中」に任せず、押したウインドウ（ビューポート）の
//! 生のマウスの状態から自分で追うのは、ドラッグを始めたペインが途中で木から抜けて
//! 別ウインドウへ移り、元のウィジェットが消えてもドラッグを続けるため。また、マウスを
//! 押したままの出来事は押したウインドウにしか届かない（Windows のキャプチャ、X11 の暗黙の
//! つかみ）ので、ほかのウインドウの上にいるかどうかは画面の座標どうしで比べる。

pub mod guides;
pub mod tree;

use std::collections::BTreeMap;

use egui::{
    vec2, CursorIcon, Id, LayerId, Order, Pos2, Rect, Sense, Ui, UiBuilder, Vec2, ViewportBuilder,
    ViewportCommand, ViewportId,
};
use serde::{Deserialize, Serialize};

pub use tree::{Axis, Node, Side, Target};

/// ペインの中身。呼ぶ側（アプリ）が持つ。
pub trait Panes {
    /// 見出しと別ウインドウの題名に出す名前。
    fn title(&self, pane: &str) -> String;
    /// 見出しの下の中身。
    fn ui(&mut self, ui: &mut Ui, pane: &str);
    /// 見出しを出すか。出さないペイン（vw の画像）は見出しをドラッグできないので、
    /// [`Dock::pane_menu`] をアプリのメニューに入れて動かす。
    fn has_header(&self, _pane: &str) -> bool {
        true
    }
    /// 見出しの行の右に置くボタン。
    fn header_buttons(&mut self, _ui: &mut Ui, _pane: &str) {}
    /// 見出しの行の中身。左から右へ並べる行で呼ぶ。既定は名前と、右に詰めた [`Panes::header_buttons`]。
    /// 行のどこを押してもドラッグになるが、行に置いたボタンはボタンとして押せる。
    fn header(&mut self, ui: &mut Ui, pane: &str) {
        vw_ui::panel::header(ui, &self.title(pane));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            self.header_buttons(ui, pane);
        });
    }
}

/// 見出しの行の左右の余白。egui のパネルの枠（`Frame::side_top_panel`）の余白と同じにして、
/// 見出しの字と中身の字の頭をそろえる。
pub const HEADER_MARGIN: f32 = 8.0;

/// 見出しの高さ（ポイント）。vw の今の見出しと同じ。
pub const HEADER: f32 = 20.0;

/// ウインドウ。本体か、引き出した別ウインドウ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Window {
    Main,
    Float(u64),
}

/// 隠したペインを出すときの行き先。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Back {
    Main(Target),
    /// 別ウインドウ。画面の `[左, 上, 幅, 高さ]`（中身の範囲、ポイント）。
    Float([f32; 4]),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloatSaved {
    pub root: Node,
    pub rect: [f32; 4],
}

/// 起動をまたいで残す並び。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub main: Option<Node>,
    pub floats: Vec<FloatSaved>,
    pub hidden: BTreeMap<String, Back>,
    /// 本体で最後にいた場所。Ctrl＋ダブルクリックと「本体に戻す」で使う。
    pub docked: BTreeMap<String, Target>,
    /// 別ウインドウで最後にいた位置と大きさ。
    pub floated: BTreeMap<String, [f32; 4]>,
}

impl Layout {
    /// 並び（本体・別ウインドウ・隠したもの）のどこかに `pane` があるか。
    pub fn mentions(&self, pane: &str) -> bool {
        self.main.as_ref().is_some_and(|n| n.contains(pane))
            || self.floats.iter().any(|f| f.root.contains(pane))
            || self.hidden.contains_key(pane)
    }
}

struct Float {
    id: u64,
    root: Option<Node>,
    /// 開くときに渡す位置と大きさ。動かした後の値は渡し直さない（egui は渡した値が
    /// 前のフレームと変わるとウインドウを動かし、利用者が動かしている最中と取り合いになるため）。
    initial: [f32; 4],
    /// 今の中身の範囲（画面の座標）。
    inner: Option<Rect>,
    /// 外枠の左上から中身の左上までのずれ（OS の題名の帯の分）。
    frame: Vec2,
    close: bool,
}

impl Float {
    fn rect(&self) -> [f32; 4] {
        match self.inner {
            Some(r) => [r.left(), r.top(), r.width(), r.height()],
            None => self.initial,
        }
    }

    fn viewport(&self) -> ViewportId {
        ViewportId::from_hash_of(("vw-dock-float", self.id))
    }
}

/// 前のフレームで並べた結果。ほかのウインドウからのドラッグの当たりを調べるのに使う。
#[derive(Clone)]
struct Geometry {
    /// 中身の範囲（画面の座標）。
    inner: Rect,
    /// ペインを並べた範囲（ウインドウの中の座標）。
    area: Rect,
    panes: Vec<tree::Placed>,
}

struct Drag {
    pane: String,
    /// マウスを押したウインドウ。押したままの出来事はここにしか届かない。
    source: ViewportId,
    /// 押した点の、ペインの左上からのずれ。引き出したウインドウをこのずれで追わせる。
    grab: Vec2,
    /// 引き出した別ウインドウ。
    float: Option<u64>,
    /// 引き出す前にいた場所。Esc で取り消すと戻す。
    origin: Option<Back>,
    /// 最後に分かったカーソルの位置（画面の座標）。放したフレームでは位置が取れない
    /// ことがある（X11）ので、前の値を使う。
    screen: Pos2,
    under: Option<Window>,
    hover: Option<(Window, guides::Button)>,
    ctrl: bool,
    moves: u64,
    /// ほかのウインドウの上にいる間、引き出したウインドウを見出しだけの大きさに縮めている。
    /// 縮める前の大きさ。
    shrunk: Option<Vec2>,
    /// 引き出したウインドウをカーソルに追わせるか。押したウインドウそのものを動かすドラッグ
    /// （別ウインドウのただ 1 つのペインの見出し）では追わせず、放したときに 1 度だけ動かす。
    /// 押したウインドウのマウスの座標はそのウインドウの左上からの位置で、動かしている最中は
    /// 座標を測ったときの位置と egui が知っている位置がずれ、追わせると行き過ぎを繰り返して
    /// 画面の外へ飛んでいくため（X11 で確かめた）。そのウインドウは OS の題名の帯でも動かせる。
    follows: bool,
    title: String,
}

struct Resize {
    window: Window,
    divider: tree::Divider,
}

pub struct Dock {
    main: Option<Node>,
    floats: Vec<Float>,
    hidden: BTreeMap<String, Back>,
    docked: BTreeMap<String, Target>,
    floated: BTreeMap<String, [f32; 4]>,
    default: Option<Node>,
    drag: Option<Drag>,
    resize: Option<Resize>,
    geometry: BTreeMap<Window, Geometry>,
    next_id: u64,
    log: Vec<LogLine>,
    /// 前に記録した各ウインドウの位置。変わったときだけ記録する。
    logged_rects: BTreeMap<Window, Rect>,
    logged_ppp: BTreeMap<Window, (f32, Option<Vec2>)>,
}

/// ほかのウインドウの上で縮めた、引き出したウインドウの大きさ。
const CHIP: Vec2 = vec2(160.0, HEADER);
/// 縮めたウインドウのカーソルからのずれ。カーソルの先とガイドを隠さないよう右下へ。
const CHIP_OFFSET: Vec2 = vec2(14.0, 18.0);

/// 別ウインドウの最初の大きさの下限。
const MIN_FLOAT: Vec2 = vec2(220.0, 140.0);

impl Dock {
    /// `default` の並びで始める。`all` のうち並びにないペインは隠しておく。
    pub fn new(default: Node, all: &[&str]) -> Self {
        let mut dock = Self {
            main: Some(default.clone()),
            floats: Vec::new(),
            hidden: BTreeMap::new(),
            docked: BTreeMap::new(),
            floated: BTreeMap::new(),
            default: Some(default),
            drag: None,
            resize: None,
            geometry: BTreeMap::new(),
            next_id: 1,
            log: Vec::new(),
            logged_rects: BTreeMap::new(),
            logged_ppp: BTreeMap::new(),
        };
        dock.adopt(all);
        dock
    }

    /// 保存した並びから始める。並びにない・二重にあるペインは直し、知らないペインは捨てる。
    pub fn restored(default: Node, all: &[&str], saved: Layout) -> Self {
        let mut dock = Self::new(default, all);
        dock.main = saved.main;
        dock.floats = saved
            .floats
            .into_iter()
            .map(|f| {
                let id = dock.next_id;
                dock.next_id += 1;
                Float {
                    id,
                    root: Some(f.root),
                    initial: f.rect,
                    inner: None,
                    frame: Vec2::ZERO,
                    close: false,
                }
            })
            .collect();
        dock.hidden = saved.hidden;
        dock.docked = saved.docked;
        dock.floated = saved.floated;
        dock.adopt(all);
        dock.event(format!(
            "restored layout: main {:?}, {} floating window(s), hidden {:?}",
            dock.main.as_ref().map(Node::panes),
            dock.floats.len(),
            dock.hidden.keys().collect::<Vec<_>>()
        ));
        dock
    }

    /// 並びを `all` にそろえる。
    fn adopt(&mut self, all: &[&str]) {
        let mut seen = Vec::<String>::new();
        let drop_from = |root: &mut Option<Node>, seen: &mut Vec<String>| {
            for p in root.as_ref().map(Node::panes).unwrap_or_default() {
                if !all.contains(&p.as_str()) || seen.contains(&p) {
                    tree::remove(root, &p);
                } else {
                    seen.push(p);
                }
            }
        };
        drop_from(&mut self.main, &mut seen);
        for f in &mut self.floats {
            drop_from(&mut f.root, &mut seen);
        }
        self.floats.retain(|f| f.root.is_some());
        self.hidden
            .retain(|p, _| all.contains(&p.as_str()) && !seen.contains(p));
        for p in all {
            if !seen.iter().any(|s| s == p) && !self.hidden.contains_key(*p) {
                self.hidden
                    .insert((*p).to_owned(), Back::Main(Target::edge(Side::Right)));
            }
        }
    }

    pub fn layout(&self) -> Layout {
        Layout {
            main: self.main.clone(),
            floats: self
                .floats
                .iter()
                .filter_map(|f| {
                    Some(FloatSaved {
                        root: f.root.clone()?,
                        rect: f.rect(),
                    })
                })
                .collect(),
            hidden: self.hidden.clone(),
            docked: self.docked.clone(),
            floated: self.floated.clone(),
        }
    }

    /// たまった記録を取り出す。
    pub fn take_log(&mut self) -> Vec<LogLine> {
        std::mem::take(&mut self.log)
    }

    /// ドラッグの途中の位置のような、調べるときに使う細かい記録。
    fn note(&mut self, text: String) {
        self.log.push(LogLine {
            text,
            summary: false,
        });
    }

    /// 並びが変わった結果の記録。
    fn event(&mut self, text: String) {
        self.log.push(LogLine {
            text,
            summary: true,
        });
    }

    pub fn is_shown(&self, pane: &str) -> bool {
        !self.hidden.contains_key(pane)
    }

    /// ペインがいるウインドウ。隠していれば `None`。
    pub fn window_of(&self, pane: &str) -> Option<Window> {
        if self.main.as_ref().is_some_and(|n| n.contains(pane)) {
            return Some(Window::Main);
        }
        self.floats
            .iter()
            .find(|f| f.root.as_ref().is_some_and(|n| n.contains(pane)))
            .map(|f| Window::Float(f.id))
    }

    pub fn set_shown(&mut self, pane: &str, shown: bool) {
        if shown == self.is_shown(pane) {
            return;
        }
        if shown {
            let back = self.hidden.remove(pane).expect("hidden pane has a place");
            self.event(format!("show {pane}: {back:?}"));
            self.put(pane, back);
        } else {
            let back = self.take(pane);
            self.event(format!("hide {pane}: will come back to {back:?}"));
            self.hidden.insert(pane.to_owned(), back);
        }
    }

    /// 隠しているペインを、次に出すときは別ウインドウ（`rect` は画面の `[左, 上, 幅, 高さ]`）で
    /// 出すようにする。もとは別ウインドウだった道具を、初めて出すときに前と同じ所へ出すのに使う。
    pub fn show_next_in_float(&mut self, pane: &str, rect: [f32; 4]) {
        if let Some(back) = self.hidden.get_mut(pane) {
            *back = Back::Float(rect);
        }
    }

    /// ペインを並びから抜き、元にいた場所を返す。
    fn take(&mut self, pane: &str) -> Back {
        if let Some(t) = tree::remove(&mut self.main, pane) {
            self.docked.insert(pane.to_owned(), t.clone());
            return Back::Main(t);
        }
        for f in &mut self.floats {
            if tree::remove(&mut f.root, pane).is_some() {
                let rect = f.rect();
                if f.root.is_none() {
                    f.close = true;
                }
                self.floated.insert(pane.to_owned(), rect);
                return Back::Float(rect);
            }
        }
        Back::Main(Target::edge(Side::Right))
    }

    fn put(&mut self, pane: &str, back: Back) {
        match back {
            Back::Main(t) => tree::insert(&mut self.main, pane, &t),
            Back::Float(rect) => {
                self.new_float(pane, rect);
            }
        }
    }

    fn new_float(&mut self, pane: &str, rect: [f32; 4]) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.floats.push(Float {
            id,
            root: Some(Node::Pane(pane.to_owned())),
            initial: rect,
            inner: None,
            frame: Vec2::ZERO,
            close: false,
        });
        self.note(format!("new window #{id} for {pane} at {rect:?}"));
        id
    }

    /// ペインを別ウインドウへ出す。前に別ウインドウでいた位置があればそこへ。
    pub fn float(&mut self, pane: &str) {
        let size = self.geometry.iter().find_map(|(w, g)| {
            g.panes
                .iter()
                .find(|p| p.pane == pane)
                .map(|p| (g.inner.min + p.rect.min.to_vec2(), p.rect.size(), *w))
        });
        let rect = self.floated.get(pane).copied().or_else(|| {
            size.map(|(min, size, _)| {
                let size = size.max(MIN_FLOAT);
                [min.x + 40.0, min.y + 40.0, size.x, size.y]
            })
        });
        let rect = rect.unwrap_or([200.0, 200.0, 360.0, 280.0]);
        let back = self.take(pane);
        self.hidden.remove(pane);
        self.event(format!("float {pane} (was {back:?})"));
        self.new_float(pane, rect);
    }

    /// ペインを本体の、最後にいた場所へ戻す。
    pub fn dock(&mut self, pane: &str) {
        let target = self
            .docked
            .get(pane)
            .cloned()
            .unwrap_or_else(|| Target::edge(Side::Right));
        self.take(pane);
        self.hidden.remove(pane);
        self.event(format!("dock {pane} back to {target:?}"));
        tree::insert(&mut self.main, pane, &target);
    }

    /// 既定の並びに戻す。別ウインドウは閉じる。
    pub fn reset(&mut self) {
        let all: Vec<String> = self.all_panes();
        self.main = self.default.clone();
        for f in &mut self.floats {
            f.close = true;
            f.root = None;
        }
        self.hidden.clear();
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        self.adopt(&refs);
        self.event("reset layout to default".to_owned());
    }

    /// 別ウインドウの中身をすべて本体へ戻す。モニタの構成が変わり、前の位置が
    /// 見えない所かもしれないときに使う。
    pub fn bring_floats_home(&mut self) {
        let panes: Vec<String> = self
            .floats
            .iter()
            .flat_map(|f| f.root.as_ref().map(Node::panes).unwrap_or_default())
            .collect();
        for p in panes {
            self.dock(&p);
        }
        for back in self.hidden.values_mut() {
            if matches!(back, Back::Float(_)) {
                *back = Back::Main(Target::edge(Side::Right));
            }
        }
        self.floated.clear();
        self.event("monitor layout changed: brought floating panes into the main window".into());
    }

    fn all_panes(&self) -> Vec<String> {
        let mut all: Vec<String> = self.main.as_ref().map(Node::panes).unwrap_or_default();
        for f in &self.floats {
            all.extend(f.root.as_ref().map(Node::panes).unwrap_or_default());
        }
        all.extend(self.hidden.keys().cloned());
        all
    }

    /// ペインの右クリックの項目。見出しのないペインのために、アプリのメニューにも入れられる。
    pub fn pane_menu(&mut self, ui: &mut Ui, pane: &str, labels: &Labels) {
        let window = self.window_of(pane);
        let alone_in_float = match window {
            Some(Window::Float(id)) => self
                .floats
                .iter()
                .find(|f| f.id == id)
                .and_then(|f| f.root.as_ref())
                .is_some_and(|n| matches!(n, Node::Pane(_))),
            _ => false,
        };
        if !alone_in_float && ui.add(vw_ui::menu::item(false, labels.float, "")).clicked() {
            self.float(pane);
        }
        if matches!(window, Some(Window::Float(_)))
            && ui
                .add(vw_ui::menu::item(false, labels.dock, labels.dock_key))
                .clicked()
        {
            self.dock(pane);
        }
        ui.separator();
        if ui.add(vw_ui::menu::item(false, labels.hide, "")).clicked() {
            self.set_shown(pane, false);
        }
        ui.separator();
        if ui.add(vw_ui::menu::item(false, labels.reset, "")).clicked() {
            self.reset();
        }
    }

    /// 本体の `ui` の残りの範囲にペインを並べ、別ウインドウも出す。毎フレーム呼ぶ。
    pub fn show(&mut self, ui: &mut Ui, panes: &mut impl Panes, labels: &Labels) {
        let ctx = ui.ctx().clone();
        self.log_window_changes(&ctx);
        self.track_drag(&ctx);
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, Sense::hover());
        self.window_ui(ui, Window::Main, area, panes, labels);
        self.floats_ui(&ctx, panes, labels);
    }

    /// 別ウインドウだけを出す。本体の中を設定画面などに使っていて [`Dock::show`] を
    /// 呼ばないフレームでも、別ウインドウは開いたままにするため。
    pub fn show_floats(&mut self, ctx: &egui::Context, panes: &mut impl Panes, labels: &Labels) {
        self.log_window_changes(ctx);
        // 本体にはペインを出していないので、ガイドで本体に入れられないようにする。
        self.geometry.remove(&Window::Main);
        self.track_drag(ctx);
        self.floats_ui(ctx, panes, labels);
    }

    fn floats_ui(&mut self, ctx: &egui::Context, panes: &mut impl Panes, labels: &Labels) {
        self.floats.retain(|f| !f.close);
        let ids: Vec<u64> = self.floats.iter().map(|f| f.id).collect();
        for id in ids {
            self.float_viewport(ctx, id, panes, labels);
        }
        let closed: Vec<u64> = self
            .floats
            .iter()
            .filter(|f| f.close)
            .map(|f| f.id)
            .collect();
        for id in closed {
            self.close_float(id);
        }
        if self.drag.is_some() {
            ctx.request_repaint();
            for f in &self.floats {
                ctx.request_repaint_of(f.viewport());
            }
        }
    }

    fn close_float(&mut self, id: u64) {
        let Some(i) = self.floats.iter().position(|f| f.id == id) else {
            return;
        };
        let f = self.floats.remove(i);
        let rect = f.rect();
        for p in f.root.as_ref().map(Node::panes).unwrap_or_default() {
            self.event(format!("window #{id} closed: hide {p}"));
            self.floated.insert(p.clone(), rect);
            self.hidden.insert(p, Back::Float(rect));
        }
    }

    fn float_viewport(
        &mut self,
        ctx: &egui::Context,
        id: u64,
        panes: &mut impl Panes,
        labels: &Labels,
    ) {
        let Some(f) = self.floats.iter().find(|f| f.id == id) else {
            return;
        };
        let viewport = f.viewport();
        let [x, y, w, h] = f.initial;
        let title = f
            .root
            .as_ref()
            .map(|n| {
                n.panes()
                    .iter()
                    .map(|p| panes.title(p))
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .unwrap_or_default();
        let builder = ViewportBuilder::default()
            .with_title(format!("{title} - vw"))
            .with_position([x, y])
            .with_inner_size([w, h]);
        ctx.show_viewport_immediate(viewport, builder, |ctx, _class| {
            let (inner, outer, close) = ctx.input(|i| {
                (
                    i.viewport().inner_rect,
                    i.viewport().outer_rect,
                    i.viewport().close_requested(),
                )
            });
            if let Some(f) = self.floats.iter_mut().find(|f| f.id == id) {
                if let Some(inner) = inner {
                    f.inner = Some(inner);
                    if let Some(outer) = outer {
                        f.frame = inner.min - outer.min;
                    }
                }
                if close {
                    f.close = true;
                }
            }
            egui::CentralPanel::no_frame().show(ctx, |ui| {
                let area = ui.max_rect();
                self.window_ui(ui, Window::Float(id), area, panes, labels);
            });
        });
    }

    fn root_mut(&mut self, window: Window) -> Option<&mut Option<Node>> {
        match window {
            Window::Main => Some(&mut self.main),
            Window::Float(id) => self
                .floats
                .iter_mut()
                .find(|f| f.id == id)
                .map(|f| &mut f.root),
        }
    }

    fn root(&self, window: Window) -> Option<&Node> {
        match window {
            Window::Main => self.main.as_ref(),
            Window::Float(id) => self.floats.iter().find(|f| f.id == id)?.root.as_ref(),
        }
    }

    /// 1 つのウインドウの中を描く。
    fn window_ui(
        &mut self,
        ui: &mut Ui,
        window: Window,
        area: Rect,
        panes: &mut impl Panes,
        labels: &Labels,
    ) {
        let visuals = ui.visuals().clone();
        ui.painter().rect_filled(area, 0.0, visuals.panel_fill);
        let arranged = self
            .root(window)
            .map(|n| tree::arrange(n, area))
            .unwrap_or_default();
        let inner = ui
            .ctx()
            .input(|i| i.viewport().inner_rect)
            .unwrap_or(Rect::from_min_size(Pos2::ZERO, area.size()));
        self.geometry.insert(
            window,
            Geometry {
                inner,
                area,
                panes: arranged.panes.clone(),
            },
        );
        let line = visuals.widgets.noninteractive.bg_stroke.color;
        for d in &arranged.dividers {
            ui.painter().rect_filled(d.rect, 0.0, line);
        }
        for placed in &arranged.panes {
            self.pane_ui(ui, window, placed, panes, labels);
        }
        self.dividers_ui(ui, window, &arranged.dividers);
        self.guides_ui(ui, window);
    }

    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        window: Window,
        placed: &tree::Placed,
        panes: &mut impl Panes,
        labels: &Labels,
    ) {
        let pane = &placed.pane;
        let mut content = placed.rect;
        if panes.has_header(pane) {
            let header = Rect::from_min_size(content.min, vec2(content.width(), HEADER));
            content.min.y += HEADER;
            let id = Id::new(("vw-dock-header", pane));
            let response = ui.interact(header, id, Sense::click_and_drag());
            let mut row = ui.new_child(
                UiBuilder::new()
                    .max_rect(header.shrink2(vec2(HEADER_MARGIN, 0.0)))
                    .id_salt(("vw-dock-header-row", pane))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            row.set_clip_rect(header.intersect(ui.clip_rect()));
            panes.header(&mut row, pane);
            if response.drag_started_by(egui::PointerButton::Primary) {
                self.begin_drag(ui.ctx(), window, placed, &panes.title(pane));
            }
            if response.double_clicked() && ui.input(|i| i.modifiers.ctrl) {
                self.note(format!("ctrl+double-click on {pane} in {window:?}"));
                match window {
                    Window::Main => self.float(pane),
                    Window::Float(_) => self.dock(pane),
                }
            }
            let pane_owned = pane.clone();
            vw_ui::menu::context(&response, |ui| {
                self.pane_menu(ui, &pane_owned, labels);
            });
        }
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(content)
                .id_salt(("vw-dock-pane", pane)),
        );
        child.set_clip_rect(content.intersect(ui.clip_rect()));
        panes.ui(&mut child, pane);
    }

    fn dividers_ui(&mut self, ui: &mut Ui, window: Window, dividers: &[tree::Divider]) {
        for d in dividers {
            let zone = d.rect.expand(3.0);
            let id = Id::new(("vw-dock-divider", window, &d.path));
            let response = ui.interact(zone, id, Sense::drag());
            let icon = match d.axis {
                Axis::Row => CursorIcon::ResizeHorizontal,
                Axis::Column => CursorIcon::ResizeVertical,
            };
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(icon);
            }
            if response.drag_started() {
                self.resize = Some(Resize {
                    window,
                    divider: d.clone(),
                });
                self.note(format!("resize start in {window:?} at {:?}", d.path));
            }
            if response.dragged() {
                if let (Some(pointer), Some(r)) = (response.interact_pointer_pos(), &self.resize) {
                    if r.window == window && r.divider.path == d.path {
                        let ratio = tree::ratio_at(&r.divider, pointer);
                        if let Some(Some(Node::Split { ratio: old, .. })) = self
                            .root_mut(window)
                            .map(|root| root.as_mut().and_then(|n| n.at_mut(&d.path)))
                        {
                            *old = ratio;
                        }
                    }
                }
            }
            if response.drag_stopped() {
                let ratio = match self
                    .root_mut(window)
                    .and_then(|r| r.as_mut())
                    .and_then(|n| n.at_mut(&d.path))
                {
                    Some(Node::Split { ratio, .. }) => *ratio,
                    _ => f32::NAN,
                };
                self.note(format!(
                    "resize end in {window:?} at {:?}: ratio {ratio:.3}",
                    d.path
                ));
                self.resize = None;
            }
        }
    }

    fn begin_drag(
        &mut self,
        ctx: &egui::Context,
        window: Window,
        placed: &tree::Placed,
        title: &str,
    ) {
        let Some(local) = ctx.input(|i| i.pointer.press_origin()) else {
            return;
        };
        let inner = ctx.input(|i| i.viewport().inner_rect).unwrap_or(Rect::ZERO);
        let screen = inner.min + local.to_vec2();
        let grab = local - placed.rect.min;
        let source = ctx.viewport_id();
        self.note(format!(
            "drag start: {} in {window:?}, press at local {local:?} = screen {screen:?}, grab {grab:?}, pane rect {:?}",
            placed.pane, placed.rect
        ));
        let alone = matches!(self.root(window), Some(Node::Pane(_)));
        let (float, origin) = match window {
            Window::Float(id) if alone => {
                self.note(format!(
                    "  only pane of window #{id}: move the window itself"
                ));
                (Some(id), None)
            }
            _ => {
                let size = placed.rect.size().max(MIN_FLOAT);
                let min = screen - grab;
                let back = self.take(&placed.pane);
                let id = self.new_float(&placed.pane, [min.x, min.y, size.x, size.y]);
                self.note(format!(
                    "  pulled out of {window:?} into window #{id}; origin {back:?}"
                ));
                (Some(id), Some(back))
            }
        };
        let follows = origin.is_some();
        self.drag = Some(Drag {
            pane: placed.pane.clone(),
            source,
            grab,
            float,
            origin,
            screen,
            under: None,
            hover: None,
            ctrl: false,
            moves: 0,
            shrunk: None,
            follows,
            title: title.to_owned(),
        });
    }

    /// ドラッグを追う。押したウインドウのマウスの状態から画面の座標を出し、引き出した
    /// ウインドウを動かし、カーソルの下のウインドウとガイドを調べ、放したら入れる。
    fn track_drag(&mut self, ctx: &egui::Context) {
        let Some(drag) = &self.drag else {
            return;
        };
        let source = drag.source;
        let (local, down, ctrl, esc, inner) = ctx.input_for(source, |i| {
            (
                i.pointer.latest_pos(),
                i.pointer.primary_down(),
                i.modifiers.ctrl,
                i.key_pressed(egui::Key::Escape),
                i.viewport().inner_rect,
            )
        });
        let drag = self.drag.as_mut().expect("checked above");
        if let (Some(local), Some(inner)) = (local, inner) {
            drag.screen = inner.min + local.to_vec2();
        }
        if ctrl != drag.ctrl {
            drag.ctrl = ctrl;
            let line = format!(
                "  ctrl {}: guides {}",
                if ctrl { "down" } else { "up" },
                if ctrl { "off" } else { "on" }
            );
            self.note(line);
        }
        if esc {
            self.cancel_drag();
            return;
        }
        let drag = self.drag.as_mut().expect("checked above");
        let screen = drag.screen;
        drag.moves += 1;
        let moves = drag.moves;
        let float = drag.float;
        if moves.is_multiple_of(30) {
            let line = format!("  moving: screen {screen:?} (source {source:?}, local {local:?}, source inner {inner:?})");
            self.note(line);
        }
        let under = self.window_under(screen, float);
        self.follow(ctx, under.is_some());
        let hover = if ctrl {
            None
        } else {
            under.and_then(|w| {
                let g = self.geometry.get(&w)?;
                let local = screen - g.inner.min.to_vec2();
                let buttons = guides::buttons(g.area, &g.panes, local);
                guides::hit(&buttons, local).cloned().map(|b| (w, b))
            })
        };
        let drag = self.drag.as_mut().expect("checked above");
        let changed_under = drag.under != under;
        let changed_hover = drag.hover.as_ref().map(|(w, b)| (*w, b.target.clone()))
            != hover.as_ref().map(|(w, b)| (*w, b.target.clone()));
        drag.under = under;
        drag.hover = hover.clone();
        if changed_under {
            self.note(format!("  over {under:?} at screen {screen:?}"));
        }
        if changed_hover {
            match &hover {
                Some((w, b)) => self.note(format!("  guide {:?} of {w:?}: {:?}", b.side, b.target)),
                None => self.note("  off the guides".to_owned()),
            }
        }
        if !down {
            self.finish_drag(ctx);
        }
    }

    /// 引き出したウインドウをカーソルに追わせる。
    ///
    /// ほかのウインドウの上では見出しだけの大きさに縮め、カーソルの右下に置く。
    /// そのままの大きさでカーソルに付けると、カーソルのまわりに出すガイドをウインドウが
    /// 隠してしまうため。ガイドを別ウインドウのさらに上に描くには、常に最前面の透明な
    /// ウインドウが要り、透明なウインドウは OS とドライバによって映らないことがあるので採らない。
    fn follow(&mut self, ctx: &egui::Context, over_window: bool) {
        let Some(drag) = self.drag.as_mut().filter(|d| d.follows) else {
            return;
        };
        let Some(f) = drag
            .float
            .and_then(|id| self.floats.iter().find(|f| f.id == id))
        else {
            return;
        };
        let viewport = f.viewport();
        let mut line = None;
        if over_window && drag.shrunk.is_none() {
            let full = f
                .inner
                .map(|r| r.size())
                .unwrap_or(vec2(f.initial[2], f.initial[3]));
            drag.shrunk = Some(full);
            ctx.send_viewport_cmd_to(viewport, ViewportCommand::InnerSize(CHIP));
            line = Some(format!(
                "  window #{} shrunk to a chip {CHIP:?} (was {full:?})",
                f.id
            ));
        } else if !over_window {
            if let Some(full) = drag.shrunk.take() {
                ctx.send_viewport_cmd_to(viewport, ViewportCommand::InnerSize(full));
                line = Some(format!("  window #{} back to {full:?}", f.id));
            }
        }
        let to = if drag.shrunk.is_some() {
            drag.screen + CHIP_OFFSET - f.frame
        } else {
            drag.screen - drag.grab - f.frame
        };
        ctx.send_viewport_cmd_to(viewport, ViewportCommand::OuterPosition(to));
        if let Some(line) = line {
            self.note(line);
        }
    }

    /// 画面の点 `screen` の下のウインドウ。引き出して動かしているウインドウ `moving` は除く。
    /// 別ウインドウどうしの重なりの順は分からないので、別ウインドウを本体より先に見る。
    fn window_under(&self, screen: Pos2, moving: Option<u64>) -> Option<Window> {
        self.floats
            .iter()
            .filter(|f| Some(f.id) != moving)
            .map(|f| Window::Float(f.id))
            .chain(std::iter::once(Window::Main))
            .find(|w| {
                self.geometry
                    .get(w)
                    .is_some_and(|g| g.inner.contains(screen))
            })
    }

    fn finish_drag(&mut self, ctx: &egui::Context) {
        let unshrunk = self.drag.as_ref().and_then(|d| d.shrunk);
        if self.drag.as_ref().is_some_and(|d| d.hover.is_none()) {
            self.follow(ctx, false);
        }
        let Some(drag) = self.drag.take() else {
            return;
        };
        let Some((window, button)) = drag.hover else {
            if let (false, Some(f)) = (
                drag.follows,
                drag.float
                    .and_then(|id| self.floats.iter().find(|f| f.id == id)),
            ) {
                let to = drag.screen - drag.grab - f.frame;
                ctx.send_viewport_cmd_to(f.viewport(), ViewportCommand::OuterPosition(to));
                self.note(format!("  move window #{} to {to:?}", f.id));
            }
            if let Some(id) = drag.float {
                // 動かしたり大きさを戻したりした結果は次のフレームまで egui に届かないので、
                // 置いた位置と大きさをここで決めて覚える。
                if let Some(f) = self.floats.iter_mut().find(|f| f.id == id) {
                    let size = unshrunk
                        .or(f.inner.map(|r| r.size()))
                        .unwrap_or(vec2(f.initial[2], f.initial[3]));
                    let inner = Rect::from_min_size(drag.screen - drag.grab, size);
                    f.inner = Some(inner);
                    let rect = f.rect();
                    self.floated.insert(drag.pane.clone(), rect);
                }
                self.event(format!(
                    "drag end: {} stays in window #{id} at screen {:?} ({} frames)",
                    drag.pane, drag.screen, drag.moves
                ));
            }
            return;
        };
        if let Some(id) = drag.float {
            if let Some(f) = self.floats.iter_mut().find(|f| f.id == id) {
                tree::remove(&mut f.root, &drag.pane);
                if f.root.is_none() {
                    f.close = true;
                }
            }
        }
        if let Some(root) = self.root_mut(window) {
            tree::insert(root, &drag.pane, &button.target);
        }
        if window == Window::Main {
            self.docked.insert(drag.pane.clone(), button.target.clone());
        }
        self.event(format!(
            "drag end: {} docked into {window:?} by guide {:?} ({:?}) ({} frames)",
            drag.pane, button.side, button.target, drag.moves
        ));
    }

    fn cancel_drag(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        match drag.origin {
            Some(back) => {
                if let Some(id) = drag.float {
                    if let Some(f) = self.floats.iter_mut().find(|f| f.id == id) {
                        f.close = true;
                        f.root = None;
                    }
                }
                self.event(format!(
                    "drag cancelled (Esc): {} back to {back:?}",
                    drag.pane
                ));
                self.put(&drag.pane, back);
            }
            None => self.event(format!(
                "drag cancelled (Esc): {} stays where it is",
                drag.pane
            )),
        }
    }

    /// カーソルがこのウインドウの上にあればガイドを描く。
    fn guides_ui(&mut self, ui: &mut Ui, window: Window) {
        let Some(drag) = &self.drag else {
            return;
        };
        if drag.under != Some(window) || drag.ctrl {
            return;
        }
        let Some(g) = self.geometry.get(&window) else {
            return;
        };
        let local = drag.screen - g.inner.min.to_vec2();
        let buttons = guides::buttons(g.area, &g.panes, local);
        let hot = guides::hit(&buttons, local);
        let painter = ui
            .ctx()
            .layer_painter(LayerId::new(Order::Foreground, Id::new("vw-dock-guides")));
        let look = guides::Look::from_visuals(ui.visuals());
        guides::paint(&painter, &buttons, hot, &look);
        if !drag.follows {
            guides::paint_chip(
                &painter,
                local + CHIP_OFFSET,
                &drag.title,
                ui.visuals(),
                &look,
            );
        }
    }

    /// ウインドウの位置・大きさ・倍率・モニタの大きさが変わったら記録する。
    fn log_window_changes(&mut self, ctx: &egui::Context) {
        let mut lines = Vec::new();
        let mut watch = |w: Window, viewport: ViewportId, this: &mut Self| {
            let (inner, ppp, monitor) = ctx.input_for(viewport, |i| {
                (
                    i.viewport().inner_rect,
                    i.pixels_per_point,
                    i.viewport().monitor_size,
                )
            });
            if let Some(inner) = inner {
                if this.logged_rects.get(&w) != Some(&inner) {
                    this.logged_rects.insert(w, inner);
                    lines.push(format!(
                        "{w:?} inner rect {:?} size {:?}",
                        inner.min,
                        inner.size()
                    ));
                }
            }
            if this.logged_ppp.get(&w) != Some(&(ppp, monitor)) {
                this.logged_ppp.insert(w, (ppp, monitor));
                lines.push(format!("{w:?} pixels per point {ppp}, monitor {monitor:?}"));
            }
        };
        watch(Window::Main, ViewportId::ROOT, self);
        let floats: Vec<(u64, ViewportId)> =
            self.floats.iter().map(|f| (f.id, f.viewport())).collect();
        for (id, vp) in floats {
            watch(Window::Float(id), vp, self);
        }
        for line in lines {
            self.note(line);
        }
    }
}

/// 記録の 1 行。
#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    pub text: String,
    /// 並びが変わった結果か。偽なら、ドラッグ中の座標のような細かい記録で、アプリは
    /// ログ欄には出さずログファイルにだけ残すなどに使い分ける。
    pub summary: bool,
}

/// メニューの文字。アプリの言語で渡す。
pub struct Labels {
    pub float: &'static str,
    pub dock: &'static str,
    pub dock_key: &'static str,
    pub hide: &'static str,
    pub reset: &'static str,
}

impl Labels {
    pub const ENGLISH: Labels = Labels {
        float: "Open in new window",
        dock: "Dock in main window",
        dock_key: "Ctrl+Double-click",
        hide: "Hide",
        reset: "Reset layout",
    };

    pub const JAPANESE: Labels = Labels {
        float: "別ウインドウで開く",
        dock: "本体に戻す",
        dock_key: "Ctrl+ダブルクリック",
        hide: "隠す",
        reset: "レイアウトを元に戻す",
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dock() -> Dock {
        Dock::new(Node::Pane("images".into()), &["images", "map"])
    }

    #[test]
    fn a_hidden_pane_set_to_float_comes_back_in_its_own_window() {
        let mut d = dock();
        d.show_next_in_float("map", [10.0, 20.0, 300.0, 200.0]);
        d.set_shown("map", true);
        assert!(matches!(d.window_of("map"), Some(Window::Float(_))));
        assert_eq!(d.layout().floats[0].rect, [10.0, 20.0, 300.0, 200.0]);
    }

    #[test]
    fn a_shown_pane_is_not_moved_by_show_next_in_float() {
        let mut d = dock();
        d.show_next_in_float("images", [10.0, 20.0, 300.0, 200.0]);
        assert_eq!(d.window_of("images"), Some(Window::Main));
    }

    #[test]
    fn a_layout_mentions_panes_in_every_place() {
        let mut d = dock();
        assert!(d.layout().mentions("images"));
        assert!(d.layout().mentions("map"));
        assert!(!d.layout().mentions("histogram"));
        d.set_shown("map", true);
        d.float("map");
        assert!(d.layout().mentions("map"));
    }
}
