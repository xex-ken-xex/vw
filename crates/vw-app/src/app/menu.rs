//! メニューバーと左端のツール列。

use eframe::egui::{self, vec2};
use egui_phosphor::regular as icon;

use crate::export::Filter;
use crate::i18n::tr;
use crate::keymap::Action;
use crate::labels::Flag;
use crate::settings_ui::layout_title;

use super::*;

impl VwApp {
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let ctx = ui.ctx().clone();
        let mut chosen = None;
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(tr(lang, "File"), |ui| {
                for action in [Action::NewSlot, Action::CloseSlot, Action::CloseAllSlots] {
                    if self.menu_item(ui, action, None) {
                        chosen = Some(action);
                    }
                }
                ui.separator();
                if self.menu_item(ui, Action::ParentFolder, None) {
                    chosen = Some(Action::ParentFolder);
                }
                if self.send_job.as_ref().is_some_and(|j| j.sending) {
                    ui.separator();
                    if self.menu_item(ui, Action::CancelSending, None) {
                        chosen = Some(Action::CancelSending);
                    }
                }
                ui.separator();
                if self.menu_item(ui, Action::Quit, None) {
                    chosen = Some(Action::Quit);
                }
            });
            ui.menu_button(tr(lang, "View"), |ui| {
                let toggles = [
                    (Action::ToggleFolderTree, self.fields.tree),
                    (Action::ToggleMetadata, self.fields.metadata),
                    (Action::ToggleImages, self.fields.images),
                    (Action::ToggleLog, self.fields.log),
                ];
                for (action, shown) in toggles {
                    if self.menu_item(ui, action, Some(shown)) {
                        chosen = Some(action);
                    }
                }
                let pipeline = self.pipeline.open;
                if self.menu_item(ui, Action::TogglePixelPipeline, Some(pipeline)) {
                    chosen = Some(Action::TogglePixelPipeline);
                }
                let histogram = self.histogram.open;
                if self.menu_item(ui, Action::ToggleHistogram, Some(histogram)) {
                    chosen = Some(Action::ToggleHistogram);
                }
                let zoom_map = self.zoom_map.open;
                if self.menu_item(ui, Action::ToggleZoomMap, Some(zoom_map)) {
                    chosen = Some(Action::ToggleZoomMap);
                }
                ui.separator();
                let maximized = self.maximized.is_some();
                if self.menu_item(ui, Action::ToggleMaximize, Some(maximized)) {
                    chosen = Some(Action::ToggleMaximize);
                }
                ui.menu_button(tr(lang, "Layout"), |ui| {
                    if let Some(action) = self.layout_menu(ui) {
                        chosen = Some(action);
                    }
                });
                ui.separator();
                if self.menu_item(ui, Action::FullscreenSlideshow, None) {
                    chosen = Some(Action::FullscreenSlideshow);
                }
                if self.menu_item(ui, Action::ToggleDatasets, Some(self.show_datasets)) {
                    chosen = Some(Action::ToggleDatasets);
                }
                if self.menu_item(ui, Action::ToggleSettings, Some(self.show_settings)) {
                    chosen = Some(Action::ToggleSettings);
                }
            });
            ui.menu_button(tr(lang, "Labels"), |ui| {
                if let Some(action) = self.labels_menu(ui) {
                    chosen = Some(action);
                }
            });
            ui.menu_button(tr(lang, "Image"), |ui| {
                let groups: [&[Action]; 5] = [
                    &[Action::PreviousImage, Action::NextImage],
                    &[Action::PreviousSlot, Action::NextSlot],
                    &[Action::PreviousPage, Action::NextPage],
                    &[
                        Action::FitImage,
                        Action::ActualSize,
                        Action::ZoomIn,
                        Action::ZoomOut,
                    ],
                    &[Action::DevelopRaw],
                ];
                for (i, group) in groups.into_iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    for &action in group {
                        if self.menu_item(ui, action, None) {
                            chosen = Some(action);
                        }
                    }
                }
            });
            ui.menu_button(tr(lang, "Help"), |ui| {
                if self.menu_item(ui, Action::ShowAiModels, None) {
                    chosen = Some(Action::ShowAiModels);
                }
                ui.separator();
                if self.menu_item(ui, Action::ShowAbout, None) {
                    chosen = Some(Action::ShowAbout);
                }
            });
        });
        if let Some(action) = chosen {
            self.run(&ctx, action);
        }
    }

    /// Labels メニュー。タグでの絞り込みはタグの数だけ並ぶので、キーを割り当てる操作にはせず
    /// ここで直接切り替える。
    pub(super) fn labels_menu(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let lang = self.lang();
        let mut chosen = None;
        if self.menu_item(ui, Action::ToggleTagging, Some(self.tagging)) {
            chosen = Some(Action::ToggleTagging);
        }
        for action in [Action::FlagGood, Action::FlagBad] {
            if self.menu_item(ui, action, None) {
                chosen = Some(action);
            }
        }
        ui.separator();
        let filters = [
            (Action::FilterAll, Filter::All),
            (Action::FilterGood, Filter::Flag(Some(Flag::Good))),
            (Action::FilterBad, Filter::Flag(Some(Flag::Bad))),
            (Action::FilterUnflagged, Filter::Flag(None)),
        ];
        for (action, filter) in filters {
            if self.menu_item(ui, action, Some(self.filter == filter)) {
                chosen = Some(action);
            }
        }
        let tags = self.settings().labels.tags.clone();
        if !tags.is_empty() {
            ui.menu_button(tr(lang, "Show only tag"), |ui| {
                for tag in tags {
                    let filter = Filter::Tag(tag.clone());
                    if ui
                        .add(egui::Button::selectable(self.filter == filter, &tag))
                        .clicked()
                    {
                        self.set_filter(ui.ctx(), filter);
                        ui.close();
                    }
                }
            });
        }
        ui.separator();
        if self.menu_item(ui, Action::ExportImages, None) {
            chosen = Some(Action::ExportImages);
        }
        if self.menu_item(ui, Action::TrashBadImages, None) {
            chosen = Some(Action::TrashBadImages);
        }
        ui.separator();
        for action in [
            Action::AiTagFolder,
            Action::FindFacesFolder,
            Action::ShowPeople,
        ] {
            if self.menu_item(ui, action, None) {
                chosen = Some(action);
            }
        }
        if self.menu_item(ui, Action::ToggleFaceFrames, Some(self.show_faces)) {
            chosen = Some(Action::ToggleFaceFrames);
        }
        chosen
    }

    /// View > Layout の項目。今のレイアウトに印を付ける。
    pub(super) fn layout_menu(&self, ui: &mut egui::Ui) -> Option<Action> {
        let lang = self.lang();
        let viewer = &self.settings().viewer;
        let mut chosen = None;
        let presets = [
            Action::LayoutAuto,
            Action::Layout1x1,
            Action::Layout1x2,
            Action::Layout2x1,
            Action::Layout2x2,
            Action::Layout3x3,
            Action::Layout4x4,
        ];
        let items = presets
            .into_iter()
            .filter_map(|a| Some((a, a.layout()?, layout_title(a.layout()?, lang))))
            .chain(std::iter::once((
                Action::LayoutCustom,
                viewer.custom_layout,
                format!(
                    "{} ({})",
                    tr(lang, "Custom"),
                    layout_title(viewer.custom_layout, lang)
                ),
            )));
        let mut marked = false;
        for (action, layout, title) in items {
            // カスタムが既定の項目と同じマス目のときに、2 つに印が付かないようにする。
            let current = !marked && viewer.layout == layout;
            marked |= current;
            let mark = if current { icon::CHECK } else { "  " };
            let button = egui::Button::new(format!("{mark} {title}"))
                .shortcut_text(self.shortcut(action).unwrap_or_default());
            if ui.add(button).clicked() {
                chosen = Some(action);
            }
        }
        chosen
    }

    /// メニューの 1 項目。右側に操作の最初のキーを出す。`checked` があれば左にチェック欄を付ける。
    ///
    /// 表示切り替えに egui のチェックボックスを使わないのは、チェックボックスには
    /// ショートカットの表示欄がなく、他の項目とキーの列がそろわないため。
    pub(super) fn menu_item(
        &self,
        ui: &mut egui::Ui,
        action: Action,
        checked: Option<bool>,
    ) -> bool {
        let title = tr(self.lang(), menu_title(action));
        let text = match checked {
            Some(true) => format!("{} {title}", icon::CHECK_SQUARE),
            Some(false) => format!("{} {title}", icon::SQUARE),
            None => title.to_owned(),
        };
        let button =
            egui::Button::new(text).shortcut_text(self.shortcut(action).unwrap_or_default());
        ui.add(button).clicked()
    }

    /// 左端の、アイコンが縦に並ぶ固定幅の列。
    pub(super) fn tool_column(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let big = |s: &str| egui::RichText::new(s).size(17.0);
        let size = vec2(TOOL_BUTTON, TOOL_BUTTON);
        ui.vertical_centered(|ui| {
            ui.add_space(4.0);
            for tool in Tool::ALL {
                let button =
                    egui::Button::selectable(self.tool == tool, big(tool.icon())).min_size(size);
                let mut tip = match tool.action() {
                    Some(action) => self.with_key(tr(lang, tool.title()), action),
                    None => tr(lang, tool.title()).to_owned(),
                };
                if let Some(hint) = tool.hint() {
                    tip = format!("{tip}\n{}", tr(lang, hint));
                }
                let r = ui
                    .add_enabled(tool.available(), button)
                    .on_hover_text(tip)
                    .on_disabled_hover_text(format!(
                        "{}\n{}",
                        tr(lang, tool.title()),
                        tr(lang, "Not available yet")
                    ));
                if r.clicked() {
                    self.tool = tool;
                }
            }
            let tip = self.with_key(tr(lang, "Tagging mode"), Action::ToggleTagging);
            let button = egui::Button::selectable(self.tagging, big(icon::TAG)).min_size(size);
            if ui.add(button).on_hover_text(tip).clicked() {
                self.tagging ^= true;
            }
            let mut text = big(icon::FUNNEL);
            if let Some(color) = self.filter_color() {
                text = text.color(color);
            }
            let tip = format!(
                "{}: {}\n{}",
                tr(lang, "Filter"),
                filter_name(&self.filter, lang),
                tr(lang, "Click to change")
            );
            let button = egui::Button::selectable(!self.filter.is_all(), text).min_size(size);
            if ui.add(button).on_hover_text(tip).clicked() {
                let next = self.filter.next(&self.settings().labels.tags);
                self.set_filter(ui.ctx(), next);
            }
            ui.separator();
            let tip = self.with_key(tr(lang, "New slot"), Action::NewSlot);
            if ui
                .add(egui::Button::new(big(icon::PLUS_SQUARE)).min_size(size))
                .on_hover_text(tip)
                .clicked()
            {
                self.add_slot();
            }
        });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            let tip = self.with_key(tr(lang, "Settings"), Action::ToggleSettings);
            let button =
                egui::Button::selectable(self.show_settings, big(icon::GEAR_SIX)).min_size(size);
            if ui.add(button).on_hover_text(tip).clicked() {
                self.toggle_settings();
            }
            let tip = self.with_key(tr(lang, "Datasets"), Action::ToggleDatasets);
            let button =
                egui::Button::selectable(self.show_datasets, big(icon::DATABASE)).min_size(size);
            if ui.add(button).on_hover_text(tip).clicked() {
                let ctx = ui.ctx().clone();
                self.toggle_datasets(&ctx);
            }
        });
    }
}
