//! 人物の相関図：作品ごとの図に、人物（ノード）と関係（線）を置く。`people.json` の `graphs` に持つ。
//!
//! 線の太さに使う「同じ画像に写った回数」は保存しない。顔の枠から毎回数えるので、
//! 顔を探し直したり人物を消したりしても食い違わない。

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::faces::FaceMark;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub name: String,
    #[serde(default)]
    pub members: Vec<Member>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

/// 図の上の人物と、その位置（図の座標）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub pos: [f32; 2],
}

/// 手で引いた線。向きは持たない（`a` と `b` を入れ替えても同じ線）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub a: String,
    pub b: String,
    /// 関係の名前（親子、同僚など）。空なら名前のない線。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

impl Edge {
    pub fn joins(&self, a: &str, b: &str) -> bool {
        (self.a == a && self.b == b) || (self.a == b && self.b == a)
    }

    fn touches(&self, name: &str) -> bool {
        self.a == name || self.b == name
    }
}

/// コピーした人物と、その人物どうしの線。位置は左上の人物からの相対。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Clip {
    pub members: Vec<Member>,
    pub edges: Vec<Edge>,
}

impl Graph {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            members: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn has(&self, name: &str) -> bool {
        self.members.iter().any(|m| m.name == name)
    }

    /// 人物を置く。もう図にいれば何もしない。
    pub fn add(&mut self, name: &str, pos: [f32; 2]) {
        if !self.has(name) {
            self.members.push(Member {
                name: name.to_owned(),
                pos,
            });
        }
    }

    /// 線を引く。同じ 2 人の線がもうあるとき、自分どうし、図にいない人とは引かない。
    pub fn connect(&mut self, a: &str, b: &str) -> bool {
        if a == b || !self.has(a) || !self.has(b) || self.edge(a, b).is_some() {
            return false;
        }
        self.edges.push(Edge {
            a: a.to_owned(),
            b: b.to_owned(),
            label: String::new(),
        });
        true
    }

    pub fn edge(&self, a: &str, b: &str) -> Option<&Edge> {
        self.edges.iter().find(|e| e.joins(a, b))
    }

    pub fn set_label(&mut self, a: &str, b: &str, label: &str) {
        if let Some(e) = self.edges.iter_mut().find(|e| e.joins(a, b)) {
            e.label = label.trim().to_owned();
        }
    }

    pub fn remove_edge(&mut self, a: &str, b: &str) {
        self.edges.retain(|e| !e.joins(a, b));
    }

    /// 人物を図から外す。その人の線も消える（人物の登録はそのまま）。
    pub fn remove(&mut self, names: &[String]) {
        self.members.retain(|m| !names.contains(&m.name));
        self.edges.retain(|e| !names.iter().any(|n| e.touches(n)));
    }

    /// 選んだ人物と、その人たちのあいだの線を写す。
    pub fn copy(&self, names: &[String]) -> Clip {
        let members: Vec<&Member> = self
            .members
            .iter()
            .filter(|m| names.contains(&m.name))
            .collect();
        let left = members
            .iter()
            .map(|m| m.pos[0])
            .fold(f32::INFINITY, f32::min);
        let top = members
            .iter()
            .map(|m| m.pos[1])
            .fold(f32::INFINITY, f32::min);
        Clip {
            members: members
                .iter()
                .map(|m| Member {
                    name: m.name.clone(),
                    pos: [m.pos[0] - left, m.pos[1] - top],
                })
                .collect(),
            edges: self
                .edges
                .iter()
                .filter(|e| names.contains(&e.a) && names.contains(&e.b))
                .cloned()
                .collect(),
        }
    }

    /// 写したものを `at` を左上にして貼る。図にもういる人は動かさず、ない線だけ足す。
    /// 貼った人物の名前を返す（選んだ状態にするため）。
    pub fn paste(&mut self, clip: &Clip, at: [f32; 2]) -> Vec<String> {
        for m in &clip.members {
            self.add(&m.name, [at[0] + m.pos[0], at[1] + m.pos[1]]);
        }
        for e in &clip.edges {
            if self.connect(&e.a, &e.b) {
                self.set_label(&e.a, &e.b, &e.label);
            }
        }
        clip.members.iter().map(|m| m.name.clone()).collect()
    }
}

/// 人物の名前が変わったら、すべての図の名前も変える。
pub fn rename_person(graphs: &mut [Graph], from: &str, to: &str) {
    for g in graphs {
        for m in &mut g.members {
            if m.name == from {
                m.name = to.to_owned();
            }
        }
        for e in &mut g.edges {
            for end in [&mut e.a, &mut e.b] {
                if end == from {
                    *end = to.to_owned();
                }
            }
        }
    }
}

/// 人物が消えたら、すべての図から外す。
pub fn remove_person(graphs: &mut [Graph], name: &str) {
    for g in graphs {
        g.remove(&[name.to_owned()]);
    }
}

/// `base` と重ならない図の名前（「図 2」のように番号を足す）。
pub fn unique_name(graphs: &[Graph], base: &str) -> String {
    let taken = |n: &str| graphs.iter().any(|g| g.name == n);
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|i| format!("{base} {i}"))
        .find(|n| !taken(n))
        .expect("an unused number exists")
}

/// どの図にも使われている関係の名前（重なりなし、名前順）。入力の候補にする。
pub fn labels(graphs: &[Graph]) -> Vec<String> {
    graphs
        .iter()
        .flat_map(|g| &g.edges)
        .filter(|e| !e.label.is_empty())
        .map(|e| e.label.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// 2 人の組の鍵。名前の順にそろえる。
pub fn pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

/// 2 人が同じ画像に写った枚数。1 枚に同じ人が 2 回写っていても 1 回。
pub fn together<'a>(
    images: impl IntoIterator<Item = &'a [FaceMark]>,
) -> HashMap<(String, String), usize> {
    let mut counts = HashMap::new();
    for marks in images {
        let names: BTreeSet<&str> = marks.iter().filter_map(|m| m.person.as_deref()).collect();
        let names: Vec<&str> = names.into_iter().collect();
        for (i, a) in names.iter().enumerate() {
            for b in &names[i + 1..] {
                *counts.entry(pair(a, b)).or_insert(0) += 1;
            }
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(names: &[&str]) -> Graph {
        let mut g = Graph::new("G");
        for (i, n) in names.iter().enumerate() {
            g.add(n, [i as f32 * 100.0, 50.0]);
        }
        g
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_person_is_placed_once() {
        let mut g = graph(&["A"]);
        g.add("A", [9.0, 9.0]);
        assert_eq!(g.members.len(), 1);
        assert_eq!(g.members[0].pos, [0.0, 50.0]);
    }

    #[test]
    fn a_line_joins_two_different_members_once_in_either_direction() {
        let mut g = graph(&["A", "B"]);
        assert!(g.connect("A", "B"));
        assert!(!g.connect("B", "A"), "the same pair");
        assert!(!g.connect("A", "A"), "the same person");
        assert!(!g.connect("A", "Z"), "not in this chart");
        g.set_label("B", "A", " 親子 ");
        assert_eq!(g.edge("A", "B").unwrap().label, "親子");
        g.remove_edge("B", "A");
        assert!(g.edges.is_empty());
    }

    #[test]
    fn removing_a_member_removes_its_lines() {
        let mut g = graph(&["A", "B", "C"]);
        g.connect("A", "B");
        g.connect("B", "C");
        g.connect("A", "C");
        g.remove(&names(&["B"]));
        assert_eq!(
            names(&["A", "C"]),
            g.members.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
        );
        assert_eq!(g.edges.len(), 1);
        assert!(g.edge("A", "C").is_some());
    }

    #[test]
    fn copy_and_paste_carry_the_lines_between_the_chosen_people() {
        let mut from = graph(&["A", "B", "C"]);
        from.connect("A", "B");
        from.set_label("A", "B", "同僚");
        from.connect("B", "C");
        let clip = from.copy(&names(&["A", "B"]));
        assert_eq!(clip.members[0].pos, [0.0, 0.0], "relative to the top left");
        assert_eq!(clip.members[1].pos, [100.0, 0.0]);
        assert_eq!(clip.edges.len(), 1, "B-C is left behind");

        let mut to = graph(&["B"]);
        let pasted = to.paste(&clip, [500.0, 500.0]);
        assert_eq!(pasted, names(&["A", "B"]));
        assert_eq!(to.members[0].pos, [0.0, 50.0], "B already here stays put");
        assert_eq!(to.members[1].name, "A");
        assert_eq!(to.members[1].pos, [500.0, 500.0]);
        assert_eq!(to.edge("B", "A").unwrap().label, "同僚");
        to.paste(&clip, [0.0, 0.0]);
        assert_eq!(
            (to.members.len(), to.edges.len()),
            (2, 1),
            "pasting again adds nothing"
        );
    }

    #[test]
    fn renaming_or_deleting_a_person_follows_into_every_chart() {
        let mut gs = vec![graph(&["A", "B"]), graph(&["A"])];
        gs[0].connect("A", "B");
        rename_person(&mut gs, "A", "C");
        assert!(gs[0].edge("C", "B").is_some());
        assert!(gs[1].has("C"));
        remove_person(&mut gs, "C");
        assert!(gs[0].edges.is_empty());
        assert!(gs[1].members.is_empty());
    }

    #[test]
    fn new_chart_names_do_not_clash() {
        let gs = vec![Graph::new("Chart"), Graph::new("Chart 2")];
        assert_eq!(unique_name(&gs, "Chart"), "Chart 3");
        assert_eq!(unique_name(&gs, "Family"), "Family");
    }

    #[test]
    fn relationship_names_are_offered_once_each() {
        let mut a = graph(&["A", "B", "C"]);
        a.connect("A", "B");
        a.set_label("A", "B", "友人");
        a.connect("B", "C");
        let mut b = graph(&["A", "B"]);
        b.connect("A", "B");
        b.set_label("A", "B", "友人");
        assert_eq!(labels(&[a, b]), ["友人"]);
    }

    fn mark(person: Option<&str>) -> FaceMark {
        FaceMark {
            rect: [0.0; 4],
            score: 1.0,
            person: person.map(str::to_owned),
            similarity: 1.0,
            unlinked: false,
            hidden: false,
        }
    }

    #[test]
    fn people_in_the_same_image_count_once_per_image() {
        let one = [
            mark(Some("B")),
            mark(Some("A")),
            mark(Some("A")),
            mark(None),
        ];
        let two = [mark(Some("A")), mark(Some("B")), mark(Some("C"))];
        let three = [mark(Some("C"))];
        let counts = together([&one[..], &two[..], &three[..]]);
        assert_eq!(counts[&pair("B", "A")], 2);
        assert_eq!(counts[&pair("A", "C")], 1);
        assert_eq!(counts[&pair("C", "B")], 1);
        assert_eq!(counts.len(), 3);
    }
}
