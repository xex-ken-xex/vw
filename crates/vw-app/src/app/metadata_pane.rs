//! メタデータ欄。

use eframe::egui::{self, collapsing_header::CollapsingState, vec2};

use crate::copy::{self, Focus, MetaGroup, MetaSelection};
use crate::frames;
use crate::i18n::tr;
use crate::settings::HeaderClick;

use super::*;

impl VwApp {
    /// メタデータ欄に出すスロット。アクティブなスロットが読み込み中でまだ画像がないときは、
    /// 直前に出していたスロットのまま変えない。
    ///
    /// 読み込み中に「画像なし」を出すと、スロットを足して画像を開くたびに表が消えては現れ、
    /// フォルダの画像をまとめて開くときに点滅して見えるため。
    pub(super) fn metadata_slot(&mut self) -> Option<usize> {
        let slots: Vec<SlotView> = self
            .slots
            .iter()
            .map(|s| SlotView {
                id: s.id,
                shown: s.shown.is_some(),
                loading: s.loading,
            })
            .collect();
        let index = metadata_source(&slots, self.active, self.metadata_from);
        self.metadata_from = index.map(|i| slots[i].id);
        index
    }

    /// メタデータ欄に出す表。ファイル情報のグループの後に、プラグインのメタデータのグループが続く。
    /// 画像がなければ `None`。
    pub(super) fn metadata_groups(&mut self) -> Option<Vec<MetaGroup>> {
        let lang = self.lang();
        let index = self.metadata_slot()?;
        let shown = self.slots[index].shown.as_ref()?;
        let info = &shown.info;
        let row = |k: &'static str, v: String| (tr(lang, k).to_owned(), v);
        let frame = frames::split(&shown.path);
        let mut file_rows = vec![
            row("Path", frames::file_of(&shown.path).display().to_string()),
            row("Size", format!("{} bytes", info.file_size)),
            row("Plugin", info.plugin.clone()),
            row("Pixels", format!("{} x {}", info.width, info.height)),
            row("Color", color_name(info.color).into()),
            row(
                "Sample",
                format!(
                    "{} bit ({} significant)",
                    info.bits_per_sample, info.significant_bits
                ),
            ),
            row("Texture", {
                let [w, h] = shown.texture().size();
                // スライドショーの先読みは、フル解像度の描画用の画像を縮小画像の欄に入れて出す。
                let reduced = (w as u32) < info.width && (h as u32) < info.height;
                let kind = if shown.full.is_some() || !reduced {
                    ""
                } else {
                    " (thumbnail)"
                };
                format!("{w} x {h}{kind}")
            }),
        ];
        if let Some((_, i)) = frame {
            let total = self.slots[index]
                .sequence
                .as_ref()
                .map_or(0, |q| q.paths.len());
            file_rows.insert(1, row("Frame", format!("{} / {total}", i + 1)));
        }
        let mut groups = vec![(tr(lang, "File").to_owned(), file_rows)];
        groups.extend(
            group_metadata(&info.metadata.entries)
                .into_iter()
                .map(|(g, rows)| {
                    let rows = rows
                        .into_iter()
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .collect();
                    (g.to_owned(), rows)
                }),
        );
        Some(groups)
    }

    /// スロットの画像について、ファイル情報とプラグインのメタデータを表で出す。
    /// 見出しと行はクリックで選んでコピーでき、値の文字列はドラッグで範囲選択できる。
    pub(super) fn metadata_ui(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let Some(groups) = self.metadata_groups() else {
            ui.weak(tr(lang, "No image"));
            return;
        };
        let header_click = self.settings.settings.behavior.header_click;
        let mut picked: Option<MetaSelection> = None;
        let mut copy: Option<MetaSelection> = None;
        let mut text_drag = false;
        let selected = self.meta_selected.clone();
        let highlight = ui.visuals().selection.bg_fill;
        let widest_key = groups
            .iter()
            .flat_map(|(_, rows)| rows)
            .map(|(k, _)| {
                egui::WidgetText::from(k.as_str())
                    .into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        egui::TextStyle::Body,
                    )
                    .size()
                    .x
            })
            .fold(0.0, f32::max);
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (group, rows) in &groups {
                    let id = ui.make_persistent_id(("metadata", group));
                    let state = CollapsingState::load_with_default_open(ui.ctx(), id, true);
                    let group_sel = MetaSelection::Group(group.clone());
                    let is_selected = selected.as_ref() == Some(&group_sel);
                    let (_, head, _) = state
                        .show_header(ui, |ui| {
                            // 欄より長い見出しで欄が押し広げられ、その幅のまま戻らなくなるため省略する。
                            ui.add(egui::Button::selectable(is_selected, group.as_str()).truncate())
                        })
                        .body(|ui| {
                            let avail = ui.available_width();
                            let gap = ui.spacing().item_spacing.x;
                            let gap_y = ui.spacing().item_spacing.y;
                            let key_width = key_column_width(widest_key, avail - gap);
                            let value_width = (avail - gap - key_width).max(0.0);
                            let row_of = |i: usize| MetaSelection::Row(group.clone(), i);
                            let selected_row = match &selected {
                                Some(MetaSelection::Row(g, i)) if g == group => Some(*i),
                                _ => None,
                            };
                            for (i, (k, v)) in rows.iter().enumerate() {
                                // 縞と選択の色は行の高さが決まってから塗るので、先に場所だけ取る。
                                let background = ui.painter().add(egui::Shape::Noop);
                                let row = ui.horizontal_top(|ui| {
                                    let key = fixed_width(ui, key_width, |ui| {
                                        ui.add(
                                            egui::Label::new(k)
                                                .truncate()
                                                .selectable(false)
                                                .sense(egui::Sense::click()),
                                        )
                                    });
                                    let value = fixed_width(ui, value_width, |ui| {
                                        ui.add(egui::Label::new(v).wrap())
                                    });
                                    (key, value)
                                });
                                let fill = if selected_row == Some(i) {
                                    Some(highlight)
                                } else {
                                    (i % 2 == 1).then_some(ui.visuals().faint_bg_color)
                                };
                                if let Some(fill) = fill {
                                    let rect = row.response.rect.expand2(vec2(0.0, gap_y / 2.0));
                                    ui.painter().set(
                                        background,
                                        egui::epaint::RectShape::filled(rect, 0.0, fill),
                                    );
                                }
                                let (key, value) = row.inner;
                                if key.clicked() || value.clicked() {
                                    picked = Some(row_of(i));
                                }
                                if value.drag_started() {
                                    text_drag = true;
                                }
                                for r in [&key, &value] {
                                    r.context_menu(|ui| {
                                        if ui.button(tr(lang, "Copy")).clicked() {
                                            copy = Some(row_of(i));
                                            ui.close();
                                        }
                                    });
                                }
                            }
                        });
                    let label = head.inner;
                    if label.clicked() {
                        picked = Some(group_sel.clone());
                        // ▶ を押したときと同じく、保存された開閉状態を反転して次のフレームから効かせる。
                        if header_click == HeaderClick::SelectAndToggle {
                            let mut state =
                                CollapsingState::load_with_default_open(ui.ctx(), id, true);
                            state.toggle(ui);
                            state.store(ui.ctx());
                        }
                    }
                    label.context_menu(|ui| {
                        if ui.button(tr(lang, "Copy")).clicked() {
                            copy = Some(group_sel.clone());
                            ui.close();
                        }
                    });
                }
            });
        let ctx = ui.ctx().clone();
        if text_drag {
            self.set_focus(&ctx, Focus::MetadataText);
        }
        if let Some(p) = picked {
            self.meta_selected = Some(p);
            self.set_focus(&ctx, Focus::Metadata);
        }
        if let Some(c) = copy {
            if let Some(text) = copy::metadata_text(&groups, &c) {
                let what = selection_name(&c);
                self.copy_text(&ctx, text, &format!("metadata {what}"));
            }
        }
    }
}
