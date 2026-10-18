//! ペインの並び。1 つのウインドウの中を左右か上下に 2 つずつ分けていく木。
//!
//! 3 つ以上を一度に並べる節を持たず、いつも 2 つに分けるのは、入れる・抜く・境目を動かすが
//! どれも「その節の 2 つの子」だけの話で済み、入れ子を何段重ねても同じ手順で扱えるため。

use egui::{pos2, Pos2, Rect};
use serde::{Deserialize, Serialize};

/// 分ける向き。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// 左右に並べる。
    Row,
    /// 上下に積む。
    Column,
}

/// 入れる側。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];

    fn axis(self) -> Axis {
        match self {
            Side::Left | Side::Right => Axis::Row,
            Side::Top | Side::Bottom => Axis::Column,
        }
    }

    /// 新しいペインが分けた節の先（左か上）に入るか。
    fn goes_first(self) -> bool {
        matches!(self, Side::Left | Side::Top)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Pane(String),
    Split {
        axis: Axis,
        /// 先の子（左か上）の取り分。0〜1。
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// どこに入れるか。`anchor` のペインをすべて含むいちばん小さい節を `side` の側で分けて入れる。
/// `anchor` が空ならウインドウ全体（木の根）を分ける。
///
/// 入れる先をペインの名前の組で持つのは、隠したペインを後で戻すときに、隣にあった
/// 節が組み替わっていても、そこにあったペインが残っていれば同じあたりに戻せるため。
/// 木の中の道順で持つと、ほかのペインを動かしただけで別の場所を指してしまう。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub anchor: Vec<String>,
    pub side: Side,
    /// 入れたペインの取り分。0〜1。
    pub share: f32,
}

impl Target {
    pub fn edge(side: Side) -> Self {
        Self {
            anchor: Vec::new(),
            side,
            share: DEFAULT_SHARE,
        }
    }

    pub fn beside(pane: &str, side: Side) -> Self {
        Self {
            anchor: vec![pane.to_owned()],
            side,
            share: 0.5,
        }
    }
}

/// ウインドウの端に入れたときの取り分。端の 1 列は細い道具の欄になることが多いため半分より狭く。
const DEFAULT_SHARE: f32 = 0.25;

/// 分けた片側の、これより狭くはしない幅（ポイント）。
pub const MIN_SIZE: f32 = 48.0;

/// 境目の線の太さ（ポイント）。
pub const GAP: f32 = 1.0;

impl Node {
    pub fn panes(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<String>) {
        match self {
            Node::Pane(id) => out.push(id.clone()),
            Node::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    pub fn contains(&self, pane: &str) -> bool {
        match self {
            Node::Pane(id) => id == pane,
            Node::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    fn contains_all(&self, panes: &[String]) -> bool {
        panes.iter().all(|p| self.contains(p))
    }

    /// `path`（先なら false、後なら true の並び）でたどった節。
    pub fn at_mut(&mut self, path: &[bool]) -> Option<&mut Node> {
        let Some((&step, rest)) = path.split_first() else {
            return Some(self);
        };
        match self {
            Node::Pane(_) => None,
            Node::Split { first, second, .. } => {
                if step {
                    second.at_mut(rest)
                } else {
                    first.at_mut(rest)
                }
            }
        }
    }

    /// 根から `anchor` をすべて含むいちばん深い節までの道順。
    fn path_to_covering(&self, anchor: &[String]) -> Vec<bool> {
        let mut path = Vec::new();
        let mut node = self;
        while let Node::Split { first, second, .. } = node {
            if first.contains_all(anchor) {
                path.push(false);
                node = first;
            } else if second.contains_all(anchor) {
                path.push(true);
                node = second;
            } else {
                break;
            }
        }
        path
    }
}

/// `pane` を `target` に入れる。木が空ならそのペインだけの木にする。
/// `anchor` のペインが 1 つも残っていなければウインドウ全体の `side` の側に入れる。
pub fn insert(root: &mut Option<Node>, pane: &str, target: &Target) {
    let new = Node::Pane(pane.to_owned());
    let Some(tree) = root.as_mut() else {
        *root = Some(new);
        return;
    };
    let anchor: Vec<String> = target
        .anchor
        .iter()
        .filter(|p| tree.contains(p))
        .cloned()
        .collect();
    let path = if anchor.is_empty() {
        Vec::new()
    } else {
        tree.path_to_covering(&anchor)
    };
    let node = tree.at_mut(&path).expect("path comes from this tree");
    let old = std::mem::replace(node, Node::Pane(String::new()));
    let share = target.share.clamp(0.05, 0.95);
    let (first, second, ratio) = if target.side.goes_first() {
        (new, old, share)
    } else {
        (old, new, 1.0 - share)
    };
    *node = Node::Split {
        axis: target.side.axis(),
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    };
}

/// `pane` を木から抜き、残った片割れを親の場所へ詰める。抜いたペインが元にいた場所を、
/// そのまま [`insert`] に渡せば戻る形で返す。木になければ `None`。
pub fn remove(root: &mut Option<Node>, pane: &str) -> Option<Target> {
    let tree = root.as_mut()?;
    if let Node::Pane(id) = tree {
        if id != pane {
            return None;
        }
        *root = None;
        return Some(Target::edge(Side::Left));
    }
    remove_in(tree, pane)
}

fn remove_in(node: &mut Node, pane: &str) -> Option<Target> {
    let Node::Split {
        axis,
        ratio,
        first,
        second,
    } = node
    else {
        return None;
    };
    let first_is = matches!(&**first, Node::Pane(id) if id == pane);
    let second_is = matches!(&**second, Node::Pane(id) if id == pane);
    if first_is || second_is {
        let (kept, side, share) = if first_is {
            let side = if *axis == Axis::Row {
                Side::Left
            } else {
                Side::Top
            };
            (
                std::mem::replace(&mut **second, Node::Pane(String::new())),
                side,
                *ratio,
            )
        } else {
            let side = if *axis == Axis::Row {
                Side::Right
            } else {
                Side::Bottom
            };
            (
                std::mem::replace(&mut **first, Node::Pane(String::new())),
                side,
                1.0 - *ratio,
            )
        };
        let anchor = kept.panes();
        *node = kept;
        return Some(Target {
            anchor,
            side,
            share,
        });
    }
    if first.contains(pane) {
        remove_in(first, pane)
    } else if second.contains(pane) {
        remove_in(second, pane)
    } else {
        None
    }
}

/// 並べた結果の 1 枚。
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub pane: String,
    pub rect: Rect,
}

/// 境目。`path` は分けた節への道順で、[`Node::at_mut`] に渡して比を変える。
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    pub path: Vec<bool>,
    pub axis: Axis,
    /// 境目の線の範囲。
    pub rect: Rect,
    /// 分けた節全体の範囲。比を求めるのに使う。
    pub parent: Rect,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arranged {
    pub panes: Vec<Placed>,
    pub dividers: Vec<Divider>,
}

/// `rect` の中に木を並べる。
pub fn arrange(node: &Node, rect: Rect) -> Arranged {
    let mut out = Arranged::default();
    arrange_in(node, rect, &mut Vec::new(), &mut out);
    out
}

fn arrange_in(node: &Node, rect: Rect, path: &mut Vec<bool>, out: &mut Arranged) {
    match node {
        Node::Pane(id) => out.panes.push(Placed {
            pane: id.clone(),
            rect,
        }),
        Node::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (a, line, b) = split_rect(rect, *axis, *ratio);
            out.dividers.push(Divider {
                path: path.clone(),
                axis: *axis,
                rect: line,
                parent: rect,
            });
            path.push(false);
            arrange_in(first, a, path, out);
            path.pop();
            path.push(true);
            arrange_in(second, b, path, out);
            path.pop();
        }
    }
}

fn split_rect(rect: Rect, axis: Axis, ratio: f32) -> (Rect, Rect, Rect) {
    match axis {
        Axis::Row => {
            let x = (rect.left() + (rect.width() - GAP) * ratio).round();
            (
                Rect::from_min_max(rect.min, pos2(x, rect.bottom())),
                Rect::from_min_max(pos2(x, rect.top()), pos2(x + GAP, rect.bottom())),
                Rect::from_min_max(pos2(x + GAP, rect.top()), rect.max),
            )
        }
        Axis::Column => {
            let y = (rect.top() + (rect.height() - GAP) * ratio).round();
            (
                Rect::from_min_max(rect.min, pos2(rect.right(), y)),
                Rect::from_min_max(pos2(rect.left(), y), pos2(rect.right(), y + GAP)),
                Rect::from_min_max(pos2(rect.left(), y + GAP), rect.max),
            )
        }
    }
}

/// 境目を `pointer` の位置へ動かしたときの比。両側とも [`MIN_SIZE`] より狭くしない。
pub fn ratio_at(divider: &Divider, pointer: Pos2) -> f32 {
    let (start, length, at) = match divider.axis {
        Axis::Row => (divider.parent.left(), divider.parent.width(), pointer.x),
        Axis::Column => (divider.parent.top(), divider.parent.height(), pointer.y),
    };
    let usable = (length - GAP).max(1.0);
    let min = (MIN_SIZE / usable).min(0.5);
    ((at - start) / usable).clamp(min, 1.0 - min)
}

/// ペインを `anchor_rect` の `side` の側に `share` の取り分で入れたときに、そのペインが占める範囲。
/// ドラッグ中に落ちる場所を見せるのに使う。
pub fn preview(anchor_rect: Rect, side: Side, share: f32) -> Rect {
    let w = anchor_rect.width() * share;
    let h = anchor_rect.height() * share;
    let r = anchor_rect;
    match side {
        Side::Left => Rect::from_min_max(r.min, pos2(r.left() + w, r.bottom())),
        Side::Right => Rect::from_min_max(pos2(r.right() - w, r.top()), r.max),
        Side::Top => Rect::from_min_max(r.min, pos2(r.right(), r.top() + h)),
        Side::Bottom => Rect::from_min_max(pos2(r.left(), r.bottom() - h), r.max),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str) -> Node {
        Node::Pane(id.to_owned())
    }

    fn split(axis: Axis, ratio: f32, a: Node, b: Node) -> Node {
        Node::Split {
            axis,
            ratio,
            first: Box::new(a),
            second: Box::new(b),
        }
    }

    /// vw の既定に近い並び: 上に「ツリー | 画像 | メタデータ」、下にログ。
    fn vw_like() -> Option<Node> {
        Some(split(
            Axis::Column,
            0.8,
            split(
                Axis::Row,
                0.2,
                pane("tree"),
                split(Axis::Row, 0.75, pane("images"), pane("metadata")),
            ),
            pane("log"),
        ))
    }

    #[test]
    fn inserting_into_an_empty_window_fills_it() {
        let mut root = None;
        insert(&mut root, "log", &Target::edge(Side::Bottom));
        assert_eq!(root, Some(pane("log")));
    }

    #[test]
    fn inserting_below_a_pane_nests_a_split_inside_its_column() {
        let mut root = vw_like();
        insert(
            &mut root,
            "histogram",
            &Target::beside("metadata", Side::Bottom),
        );
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 500.0));
        let placed = arrange(root.as_ref().unwrap(), rect).panes;
        let metadata = placed.iter().find(|p| p.pane == "metadata").unwrap().rect;
        let histogram = placed.iter().find(|p| p.pane == "histogram").unwrap().rect;
        assert_eq!(metadata.left(), histogram.left());
        assert_eq!(metadata.right(), histogram.right());
        assert!(histogram.top() > metadata.bottom());
    }

    #[test]
    fn inserting_at_a_window_edge_spans_the_whole_side() {
        let mut root = vw_like();
        insert(&mut root, "map", &Target::edge(Side::Right));
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 500.0));
        let placed = arrange(root.as_ref().unwrap(), rect).panes;
        let map = placed.iter().find(|p| p.pane == "map").unwrap().rect;
        assert_eq!(map.top(), 0.0);
        assert_eq!(map.bottom(), 500.0);
        assert_eq!(map.right(), 1000.0);
    }

    #[test]
    fn removing_a_pane_gives_its_space_to_the_sibling() {
        let mut root = vw_like();
        remove(&mut root, "metadata");
        assert_eq!(
            root,
            Some(split(
                Axis::Column,
                0.8,
                split(Axis::Row, 0.2, pane("tree"), pane("images")),
                pane("log"),
            ))
        );
    }

    #[test]
    fn a_removed_pane_goes_back_to_the_same_place() {
        for id in ["tree", "images", "metadata", "log"] {
            let before = vw_like();
            let mut root = before.clone();
            let place = remove(&mut root, id).unwrap();
            insert(&mut root, id, &place);
            assert_eq!(root, before, "{id}");
        }
    }

    #[test]
    fn a_hidden_pane_returns_beside_its_old_neighbours_after_they_moved() {
        let mut root = vw_like();
        let place = remove(&mut root, "log").unwrap();
        remove(&mut root, "metadata");
        insert(&mut root, "metadata", &Target::edge(Side::Left));
        insert(&mut root, "log", &place);
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 500.0));
        let placed = arrange(root.as_ref().unwrap(), rect).panes;
        let log = placed.iter().find(|p| p.pane == "log").unwrap().rect;
        assert_eq!(log.bottom(), 500.0);
        assert_eq!(log.width(), 1000.0);
    }

    #[test]
    fn removing_the_last_pane_empties_the_window() {
        let mut root = Some(pane("map"));
        assert!(remove(&mut root, "map").is_some());
        assert_eq!(root, None);
    }

    #[test]
    fn removing_a_missing_pane_changes_nothing() {
        let mut root = vw_like();
        assert_eq!(remove(&mut root, "map"), None);
        assert_eq!(root, vw_like());
    }

    #[test]
    fn panes_and_dividers_tile_the_window_without_overlap() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(801.0, 401.0));
        let a = arrange(vw_like().as_ref().unwrap(), rect);
        assert_eq!(a.panes.len(), 4);
        assert_eq!(a.dividers.len(), 3);
        let area: f32 = a
            .panes
            .iter()
            .map(|p| p.rect.area())
            .chain(a.dividers.iter().map(|d| d.rect.area()))
            .sum();
        assert!(
            (area - rect.area()).abs() < 1.0,
            "{area} vs {}",
            rect.area()
        );
    }

    #[test]
    fn dragging_a_divider_never_squeezes_a_side_below_the_minimum() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 500.0));
        let a = arrange(vw_like().as_ref().unwrap(), rect);
        let d = &a.dividers[0];
        let r = ratio_at(d, pos2(0.0, -100.0));
        assert!(r * 500.0 >= MIN_SIZE - 1.0);
        let r = ratio_at(d, pos2(0.0, 900.0));
        assert!((1.0 - r) * 500.0 >= MIN_SIZE - 1.0);
    }

    #[test]
    fn preview_covers_the_side_the_pane_will_take() {
        let r = Rect::from_min_size(Pos2::ZERO, egui::vec2(200.0, 100.0));
        assert_eq!(
            preview(r, Side::Right, 0.5),
            Rect::from_min_max(pos2(100.0, 0.0), pos2(200.0, 100.0))
        );
        assert_eq!(
            preview(r, Side::Top, 0.25),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 25.0))
        );
    }
}
