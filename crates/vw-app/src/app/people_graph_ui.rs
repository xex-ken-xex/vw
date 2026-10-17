//! 人物の相関図のウインドウ：図を選び、人物を置いて動かし、線を引いて関係の名前を付ける。

use std::collections::{BTreeSet, HashMap};

use eframe::egui::{self, emath::TSTransform, pos2, vec2, Color32, Pos2, Rect, Sense, Stroke};
use egui_phosphor::regular as icon;

use super::VwApp;
use crate::i18n::{tr, tr1};
use crate::people::Person;
use crate::people_graph::{self, pair, Clip, Graph};
use crate::settings::Language;

const CARD: egui::Vec2 = vec2(88.0, 96.0);
const FACE: f32 = 64.0;

#[derive(Default)]
pub(super) struct GraphWindow {
    pub open: bool,
    /// 開いている図の名前。
    current: Option<String>,
    selected: BTreeSet<String>,
    /// 選んだ線（手で引いた線の 2 人）。
    edge: Option<(String, String)>,
    /// 「ここから線を引く」を選んだ人。次にクリックした人へ線を引く。
    connecting: Option<String>,
    /// 名前を変えている図と、入力中の名前。
    renaming: Option<String>,
    deleting: bool,
    add_filter: String,
    clip: Option<Clip>,
    /// 図ごとの表示の位置と倍率。
    views: HashMap<String, TSTransform>,
    /// 代表の顔のテクスチャ。キーは (名前, 代表の番号, 顔の数)。顔を足すと作り直す。
    faces: HashMap<(String, usize, usize), egui::TextureHandle>,
}

/// 1 フレームで頼まれた、図の外にも響く変更。
#[derive(Default)]
struct Outcome {
    /// 保存する。ドラッグ中は毎フレーム書かないよう、離したときだけ立てる。
    save: bool,
}

impl VwApp {
    pub(super) fn people_graph_ui(&mut self, ctx: &egui::Context) {
        if !self.graph_window.open {
            return;
        }
        self.refresh_face_counts();
        let lang = self.lang();
        let together = self.people_window.together.clone().unwrap_or_default();
        let mut close = false;
        let mut outcome = Outcome::default();
        let people = &mut self.people;
        let w = &mut self.graph_window;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("people_graph"),
            egui::ViewportBuilder::default()
                .with_title(tr(lang, "Relationship charts"))
                .with_inner_size([1000.0, 680.0]),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                egui::Panel::top("graph_toolbar").show(ctx, |ui| {
                    toolbar(ui, w, &people.list, &mut people.graphs, lang, &mut outcome);
                });
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(ctx.style().visuals.extreme_bg_color))
                    .show(ctx, |ui| {
                        let Some(g) = w
                            .current
                            .as_ref()
                            .and_then(|n| people.graphs.iter_mut().find(|g| &g.name == n))
                        else {
                            ui.centered_and_justified(|ui| {
                                ui.weak(tr(
                                    lang,
                                    "Make a chart with New chart, then add people to it.",
                                ));
                            });
                            return;
                        };
                        canvas(ui, w, g, &people.list, &together, lang, &mut outcome);
                    });
            },
        );
        if close {
            self.graph_window.open = false;
            self.graph_window.connecting = None;
        }
        if outcome.save {
            self.save_people();
        }
    }
}

fn toolbar(
    ui: &mut egui::Ui,
    w: &mut GraphWindow,
    people: &[Person],
    graphs: &mut Vec<Graph>,
    lang: Language,
    outcome: &mut Outcome,
) {
    if w.current
        .as_ref()
        .is_none_or(|c| !graphs.iter().any(|g| &g.name == c))
    {
        w.current = graphs.first().map(|g| g.name.clone());
        w.selected.clear();
        w.edge = None;
    }
    ui.horizontal(|ui| {
        match &mut w.renaming {
            Some(text) => {
                let edit = ui.text_edit_singleline(text);
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let new = text.trim().to_owned();
                let clash =
                    graphs.iter().any(|g| g.name == new) && w.current.as_ref() != Some(&new);
                if (ui.button("OK").clicked() || enter) && !new.is_empty() && !clash {
                    if let Some(g) = graphs
                        .iter_mut()
                        .find(|g| Some(&g.name) == w.current.as_ref())
                    {
                        let old = std::mem::replace(&mut g.name, new.clone());
                        if let Some(view) = w.views.remove(&old) {
                            w.views.insert(new.clone(), view);
                        }
                        w.current = Some(new);
                        outcome.save = true;
                    }
                    w.renaming = None;
                }
                if clash {
                    ui.weak(tr(lang, "A chart with this name already exists."));
                }
                if ui.button(tr(lang, "Cancel")).clicked() {
                    w.renaming = None;
                }
            }
            None => {
                let shown = w.current.clone().unwrap_or_default();
                egui::ComboBox::from_id_salt("graph_pick")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for g in graphs.iter() {
                            if ui
                                .selectable_label(w.current.as_ref() == Some(&g.name), &g.name)
                                .clicked()
                            {
                                w.current = Some(g.name.clone());
                                w.selected.clear();
                                w.edge = None;
                                w.deleting = false;
                            }
                        }
                    });
                if ui.button(tr(lang, "New chart")).clicked() {
                    let name = people_graph::unique_name(graphs, tr(lang, "Chart"));
                    graphs.push(Graph::new(&name));
                    w.current = Some(name);
                    w.selected.clear();
                    w.edge = None;
                    outcome.save = true;
                }
                let has = w.current.is_some();
                if ui
                    .add_enabled(has, egui::Button::new(tr(lang, "Rename")))
                    .clicked()
                {
                    w.renaming = w.current.clone();
                }
                if w.deleting {
                    if ui.button(tr(lang, "Really delete")).clicked() {
                        graphs.retain(|g| Some(&g.name) != w.current.as_ref());
                        w.current = None;
                        w.deleting = false;
                        outcome.save = true;
                    }
                } else if ui
                    .add_enabled(has, egui::Button::new(tr(lang, "Delete chart")))
                    .clicked()
                {
                    w.deleting = true;
                }
            }
        }
        ui.separator();
        let Some(g) = graphs
            .iter_mut()
            .find(|g| Some(&g.name) == w.current.as_ref())
        else {
            return;
        };
        ui.menu_button(
            format!("{} {}", icon::USER_PLUS, tr(lang, "Add person")),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut w.add_filter)
                        .hint_text(tr(lang, "Search by name"))
                        .desired_width(160.0),
                );
                let query = w.add_filter.trim().to_lowercase();
                let mut left: Vec<&Person> = people
                    .iter()
                    .filter(|p| !g.has(&p.name) && p.name.to_lowercase().contains(&query))
                    .collect();
                left.sort_by(|a, b| a.name.cmp(&b.name));
                if left.is_empty() {
                    ui.weak(tr(lang, "Everyone is already in this chart."));
                }
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for p in left {
                            if ui.button(&p.name).clicked() {
                                let at = w
                                    .views
                                    .get(&g.name)
                                    .map(|t| t.inverse() * ui.ctx().content_rect().center())
                                    .unwrap_or(pos2(0.0, 0.0));
                                // 続けて足した人が重ならないよう、画面の中央から升目に並べる。
                                let n = g.members.len();
                                let (col, row) = ((n % 4) as f32, ((n / 4) % 3) as f32);
                                g.add(
                                    &p.name,
                                    [
                                        at.x + (col - 2.0) * (CARD.x + 40.0),
                                        at.y + (row - 1.0) * (CARD.y + 40.0),
                                    ],
                                );
                                w.selected = BTreeSet::from([p.name.clone()]);
                                outcome.save = true;
                            }
                        }
                    });
            },
        );
        let chosen: Vec<String> = w.selected.iter().cloned().collect();
        let copy = ui
            .add_enabled(!chosen.is_empty(), egui::Button::new(tr(lang, "Copy")))
            .on_hover_text(tr(
                lang,
                "Copy the selected people and the lines between them (Ctrl+C)",
            ));
        let paste = ui
            .add_enabled(w.clip.is_some(), egui::Button::new(tr(lang, "Paste")))
            .on_hover_text(tr(lang, "Paste into this chart (Ctrl+V)"));
        let remove = ui
            .add_enabled(
                !chosen.is_empty(),
                egui::Button::new(tr(lang, "Remove from chart")),
            )
            .on_hover_text(tr(
                lang,
                "Remove the selected people from this chart (Delete). They stay registered.",
            ));
        let typing = ui.ctx().egui_wants_keyboard_input();
        let (key_copy, key_paste, key_delete) = ui.input(|i| {
            (
                !typing && i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                !typing && i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                    || (!typing && i.modifiers.command && i.key_pressed(egui::Key::V)),
                !typing && i.key_pressed(egui::Key::Delete),
            )
        });
        if (copy.clicked() || key_copy) && !chosen.is_empty() {
            w.clip = Some(g.copy(&chosen));
            // 名前を OS のクリップボードにも入れる。egui は Ctrl+V をクリップボードに文字が
            // あるときだけ貼り付けとして届けるので、空のままだと Ctrl+V が効かない。
            ui.ctx().copy_text(chosen.join(", "));
        }
        if let Some(clip) = w.clip.as_ref().filter(|_| paste.clicked() || key_paste) {
            let at = w
                .views
                .get(&g.name)
                .map(|t| t.inverse() * (ui.ctx().content_rect().center() - CARD))
                .unwrap_or(pos2(0.0, 0.0));
            w.selected = g.paste(clip, [at.x, at.y]).into_iter().collect();
            w.edge = None;
            outcome.save = true;
        }
        if remove.clicked() || (key_delete && !chosen.is_empty()) {
            g.remove(&chosen);
            w.selected.clear();
            outcome.save = true;
        } else if key_delete {
            if let Some((a, b)) = w.edge.take() {
                g.remove_edge(&a, &b);
                outcome.save = true;
            }
        }
    });
    let Some(g) = graphs
        .iter()
        .position(|g| Some(&g.name) == w.current.as_ref())
    else {
        return;
    };
    let labels = people_graph::labels(graphs);
    let g = &mut graphs[g];
    ui.horizontal(|ui| {
        // 線を選んだときの入力欄と同じ高さにしておき、選ぶたびに図が上下に動かないようにする。
        ui.set_min_height(ui.spacing().interact_size.y + 4.0);
        edge_row(ui, w, g, &labels, lang, outcome);
    });
}

fn edge_row(
    ui: &mut egui::Ui,
    w: &mut GraphWindow,
    g: &mut Graph,
    labels: &[String],
    lang: Language,
    outcome: &mut Outcome,
) {
    match w.edge.clone() {
        Some((a, b)) if g.edge(&a, &b).is_some() => {
            ui.label(format!("{a} — {b}"));
            let mut label = g.edge(&a, &b).map(|e| e.label.clone()).unwrap_or_default();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut label)
                    .hint_text(tr(lang, "Relationship (parent, colleague...)"))
                    .desired_width(180.0),
            );
            if edit.changed() {
                g.set_label(&a, &b, &label);
            }
            if edit.lost_focus() {
                outcome.save = true;
            }
            if !labels.is_empty() {
                ui.menu_button(icon::CARET_DOWN, |ui| {
                    for l in labels {
                        if ui.button(l).clicked() {
                            g.set_label(&a, &b, l);
                            outcome.save = true;
                        }
                    }
                });
            }
            if ui.button(tr(lang, "Remove line")).clicked() {
                g.remove_edge(&a, &b);
                w.edge = None;
                outcome.save = true;
            }
        }
        _ => {
            w.edge = None;
            let hint = match &w.connecting {
                Some(from) => tr1(lang, "Click the person to join to {}. Esc cancels.", from),
                None => tr(
                    lang,
                    "Drag people to move them. Right-click a person to draw a line. Ctrl+click selects more. Thicker lines: more images together.",
                )
                .to_owned(),
            };
            ui.weak(hint);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn canvas(
    ui: &mut egui::Ui,
    w: &mut GraphWindow,
    g: &mut Graph,
    people: &[Person],
    together: &HashMap<(String, String), usize>,
    lang: Language,
    outcome: &mut Outcome,
) {
    let (outer, background) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let mut t = match w.views.get(&g.name) {
        Some(t) => *t,
        None => fit(outer, g),
    };
    if background.dragged() {
        t.translation += background.drag_delta();
    }
    if let Some(pointer) = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| outer.contains(*p))
    {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
        if scroll != 0.0 {
            let scale = (t.scaling * (scroll / 300.0).exp()).clamp(0.3, 2.5);
            let at = t.inverse() * pointer;
            t.scaling = scale;
            t.translation = pointer.to_vec2() - at.to_vec2() * scale;
        }
    }
    w.views.insert(g.name.clone(), t);
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        w.connecting = None;
    }
    let painter = ui.painter_at(outer);
    let visuals = ui.visuals().clone();
    let center = |name: &str| {
        g.members
            .iter()
            .find(|m| m.name == name)
            .map(|m| t * (pos2(m.pos[0], m.pos[1]) + CARD / 2.0))
    };
    let width =
        |count: usize| (1.0 + 2.2 * (1.0 + count as f32).ln()).min(8.0) * t.scaling.max(0.6);

    // 手で引いていない 2 人でも、同じ画像に写っていれば薄い線で出す。
    let line_color = visuals.weak_text_color();
    for (i, a) in g.members.iter().enumerate() {
        for b in &g.members[i + 1..] {
            let count = together.get(&pair(&a.name, &b.name)).copied().unwrap_or(0);
            if count == 0 || g.edge(&a.name, &b.name).is_some() {
                continue;
            }
            let (Some(p), Some(q)) = (center(&a.name), center(&b.name)) else {
                continue;
            };
            painter.add(egui::Shape::dashed_line(
                &[p, q],
                Stroke::new(width(count), line_color.gamma_multiply(0.45)),
                6.0,
                5.0,
            ));
        }
    }
    let hover = ui.input(|i| i.pointer.hover_pos());
    let mut hovered_edge = None;
    for e in &g.edges {
        let (Some(p), Some(q)) = (center(&e.a), center(&e.b)) else {
            continue;
        };
        let count = together.get(&pair(&e.a, &e.b)).copied().unwrap_or(0);
        let chosen = w.edge.as_ref().is_some_and(|(a, b)| e.joins(a, b));
        let near = hover.is_some_and(|h| distance_to_segment(h, p, q) < 6.0);
        if near {
            hovered_edge = Some((e.a.clone(), e.b.clone(), count));
        }
        let color = if chosen {
            visuals.selection.stroke.color
        } else if near {
            visuals.strong_text_color()
        } else {
            visuals.text_color()
        };
        painter.line_segment([p, q], Stroke::new(width(count).max(1.5), color));
        if !e.label.is_empty() {
            let mid = p + (q - p) / 2.0;
            let galley = painter.layout_no_wrap(
                e.label.clone(),
                egui::FontId::proportional(12.0 * t.scaling.clamp(0.8, 1.5)),
                color,
            );
            let rect = Rect::from_center_size(mid, galley.size() + vec2(8.0, 4.0));
            painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);
            painter.galley(rect.min + vec2(4.0, 2.0), galley, color);
        }
    }
    if let Some(from) = w.connecting.as_ref().and_then(|n| center(n)) {
        if let Some(h) = hover {
            painter.line_segment([from, h], Stroke::new(1.5, visuals.selection.stroke.color));
        }
    }

    let mut clicked_person = false;
    let mut drag_by = None;
    let mut drag_done = false;
    for i in 0..g.members.len() {
        let name = g.members[i].name.clone();
        let pos = pos2(g.members[i].pos[0], g.members[i].pos[1]);
        let rect = t.mul_rect(Rect::from_min_size(pos, CARD));
        let r = ui.interact(
            rect,
            ui.id().with(("graph_person", &name)),
            Sense::click_and_drag(),
        );
        let selected = w.selected.contains(&name);
        let person = people.iter().find(|p| p.name == name);
        let face = person.map(|p| face_texture(ui.ctx(), &mut w.faces, p));
        draw_card(
            &painter,
            rect,
            &name,
            face.as_ref(),
            selected,
            r.hovered(),
            t.scaling,
            &visuals,
        );
        if r.drag_started() && !selected {
            w.selected = BTreeSet::from([name.clone()]);
        }
        if r.dragged() {
            drag_by = Some(r.drag_delta() / t.scaling);
        }
        if r.drag_stopped() {
            drag_done = true;
        }
        if r.clicked() {
            clicked_person = true;
            w.edge = None;
            if let Some(from) = w.connecting.take() {
                if g.connect(&from, &name) {
                    w.edge = Some((from, name.clone()));
                    outcome.save = true;
                }
            } else if ui.input(|i| i.modifiers.command || i.modifiers.shift) {
                if !w.selected.remove(&name) {
                    w.selected.insert(name.clone());
                }
            } else {
                w.selected = BTreeSet::from([name.clone()]);
            }
        }
        if let Some(p) = person {
            let r = r.on_hover_text(tr1(lang, "{} face(s)", &p.faces.len().to_string()));
            r.context_menu(|ui| {
                if ui.button(tr(lang, "Draw a line from here")).clicked() {
                    w.connecting = Some(name.clone());
                    ui.close();
                }
                if ui.button(tr(lang, "Remove from chart")).clicked() {
                    g.remove(std::slice::from_ref(&name));
                    w.selected.remove(&name);
                    outcome.save = true;
                    ui.close();
                }
            });
        }
    }
    if let Some(d) = drag_by {
        for m in g
            .members
            .iter_mut()
            .filter(|m| w.selected.contains(&m.name))
        {
            m.pos[0] += d.x;
            m.pos[1] += d.y;
        }
    }
    outcome.save |= drag_done;

    if let Some((a, b, count)) = &hovered_edge {
        background
            .clone()
            .on_hover_text(tr1(lang, "{} image(s) together", &count.to_string()));
        if background.clicked() && !clicked_person {
            w.edge = Some((a.clone(), b.clone()));
            w.selected.clear();
        }
        let (a, b) = (a.clone(), b.clone());
        background.context_menu(|ui| {
            if ui.button(tr(lang, "Remove line")).clicked() {
                g.remove_edge(&a, &b);
                w.edge = None;
                outcome.save = true;
                ui.close();
            }
        });
    } else if background.clicked() && !clicked_person {
        w.selected.clear();
        w.edge = None;
        w.connecting = None;
    }
}

/// 図の全員が収まる表示。誰もいなければ原点を左上の近くに置く。
fn fit(outer: Rect, g: &Graph) -> TSTransform {
    let content = g
        .members
        .iter()
        .map(|m| Rect::from_min_size(pos2(m.pos[0], m.pos[1]), CARD))
        .fold(Rect::NOTHING, |a, r| a.union(r));
    if !content.is_positive() {
        return TSTransform::from_translation(outer.min.to_vec2() + vec2(40.0, 40.0));
    }
    let content = content.expand(40.0);
    let scale = (outer.size() / content.size()).min_elem().clamp(0.3, 1.0);
    TSTransform::from_translation(outer.center().to_vec2() - scale * content.center().to_vec2())
        * TSTransform::from_scaling(scale)
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len = ab.length_sq();
    if len == 0.0 {
        return (p - a).length();
    }
    let u = ((p - a).dot(ab) / len).clamp(0.0, 1.0);
    (p - (a + ab * u)).length()
}

fn face_texture(
    ctx: &egui::Context,
    cache: &mut HashMap<(String, usize, usize), egui::TextureHandle>,
    person: &Person,
) -> egui::TextureHandle {
    cache
        .entry((person.name.clone(), person.cover, person.faces.len()))
        .or_insert_with(|| {
            let img = person
                .cover()
                .and_then(|s| crate::people::thumb_image(&s.thumb))
                .unwrap_or_else(|| egui::ColorImage::filled([1, 1], Color32::GRAY));
            ctx.load_texture(format!("graph-{}", person.name), img, Default::default())
        })
        .clone()
}

#[allow(clippy::too_many_arguments)]
fn draw_card(
    painter: &egui::Painter,
    rect: Rect,
    name: &str,
    face: Option<&egui::TextureHandle>,
    selected: bool,
    hovered: bool,
    scale: f32,
    visuals: &egui::Visuals,
) {
    let stroke = if selected {
        Stroke::new(2.0, visuals.selection.stroke.color)
    } else if hovered {
        visuals.widgets.hovered.bg_stroke
    } else {
        visuals.widgets.noninteractive.bg_stroke
    };
    painter.rect(
        rect,
        4.0,
        visuals.panel_fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    let side = FACE * scale;
    let face_rect = Rect::from_min_size(
        pos2(rect.center().x - side / 2.0, rect.min.y + 6.0 * scale),
        vec2(side, side),
    );
    match face {
        Some(tex) => {
            painter.image(
                tex.id(),
                face_rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            painter.rect_filled(face_rect, 2.0, visuals.faint_bg_color);
        }
    }
    let font = egui::FontId::proportional((12.0 * scale).clamp(8.0, 20.0));
    let galley = painter.layout(
        name.to_owned(),
        font,
        visuals.text_color(),
        rect.width() - 6.0 * scale,
    );
    let at = pos2(
        rect.center().x - galley.size().x / 2.0,
        face_rect.max.y + 4.0 * scale,
    );
    painter
        .with_clip_rect(rect)
        .galley(at, galley, visuals.text_color());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_to_a_line_is_measured_to_the_nearest_point_on_it() {
        let (a, b) = (pos2(0.0, 0.0), pos2(10.0, 0.0));
        assert_eq!(distance_to_segment(pos2(5.0, 3.0), a, b), 3.0);
        assert_eq!(
            distance_to_segment(pos2(-4.0, 3.0), a, b),
            5.0,
            "beyond the end"
        );
        assert_eq!(distance_to_segment(pos2(1.0, 1.0), a, a), 2f32.sqrt());
    }
}
