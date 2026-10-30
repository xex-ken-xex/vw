//! メニューバーと左端のツール列。

use eframe::egui;
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
        vw_ui::menu::bar(ui, |ui| {
            vw_ui::menu::bar_menu(ui, tr(lang, "File"), |ui| {
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
            vw_ui::menu::bar_menu(ui, tr(lang, "View"), |ui| {
                let toggles = [
                    (Action::ToggleFolderTree, self.pane_shown(docking::TREE)),
                    (Action::ToggleMetadata, self.pane_shown(docking::METADATA)),
                    (Action::ToggleImages, self.pane_shown(docking::IMAGES)),
                    (Action::ToggleLog, self.pane_shown(docking::LOG)),
                ];
                for (action, shown) in toggles {
                    if self.menu_item(ui, action, Some(shown)) {
                        chosen = Some(action);
                    }
                }
                vw_ui::menu::submenu(ui, tr(lang, "Window layout"), |ui| self.panes_menu(ui));
                ui.separator();
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
                if self.settings().map.enabled {
                    let map = self.map.open;
                    if self.menu_item(ui, Action::ToggleMap, Some(map)) {
                        chosen = Some(Action::ToggleMap);
                    }
                }
                if self.settings().sky.enabled {
                    let sky = self.sky.open;
                    if self.menu_item(ui, Action::ToggleSkyChart, Some(sky)) {
                        chosen = Some(Action::ToggleSkyChart);
                    }
                }
                ui.separator();
                let maximized = self.maximized.is_some();
                if self.menu_item(ui, Action::ToggleMaximize, Some(maximized)) {
                    chosen = Some(Action::ToggleMaximize);
                }
                if let Some(Some(action)) =
                    vw_ui::menu::submenu(ui, tr(lang, "Layout"), |ui| self.layout_menu(ui))
                {
                    chosen = Some(action);
                }
                ui.separator();
                if self.menu_item(ui, Action::FullscreenSlideshow, None) {
                    chosen = Some(Action::FullscreenSlideshow);
                }
                if self.menu_item(ui, Action::ToggleDatasets, Some(self.show_datasets)) {
                    chosen = Some(Action::ToggleDatasets);
                }
            });
            vw_ui::menu::bar_menu(ui, tr(lang, "Labels"), |ui| {
                if let Some(action) = self.labels_menu(ui) {
                    chosen = Some(action);
                }
            });
            vw_ui::menu::bar_menu(ui, tr(lang, "AI tags"), |ui| {
                if let Some(action) = self.ai_menu(ui) {
                    chosen = Some(action);
                }
            });
            vw_ui::menu::bar_menu(ui, tr(lang, "Image"), |ui| {
                let groups: [&[Action]; 8] = [
                    &[Action::PreviousImage, Action::NextImage],
                    &[Action::PreviousSlot, Action::NextSlot],
                    &[Action::PreviousPage, Action::NextPage],
                    &[
                        Action::FitImage,
                        Action::ActualSize,
                        Action::ZoomIn,
                        Action::ZoomOut,
                    ],
                    &[
                        Action::DevelopRaw,
                        Action::SwapRawPreview,
                        Action::ToggleCalibration,
                    ],
                    &[Action::NextColorMap, Action::ToggleColorGauge],
                    &[
                        Action::ToggleClippingWarning,
                        Action::ToggleFocusPeaking,
                        Action::ToggleColorVision,
                        Action::ToggleGamutWarning,
                    ],
                    &[Action::ReadQrCodes],
                ];
                for (i, group) in groups.into_iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    for &action in group {
                        let checked =
                            (action == Action::ToggleColorGauge).then_some(self.show_gauge);
                        if self.menu_item(ui, action, checked) {
                            chosen = Some(action);
                        }
                    }
                    if group.contains(&Action::ToggleColorGauge) {
                        self.color_map_menu(ui);
                    }
                }
            });
            vw_ui::menu::bar_menu(ui, tr(lang, "Settings"), |ui| {
                if self.menu_item(ui, Action::ToggleSettings, Some(self.show_settings)) {
                    chosen = Some(Action::ToggleSettings);
                }
                ui.separator();
                for action in [Action::OpenSettingsFolder, Action::OpenModelsFolder] {
                    if self.menu_item(ui, action, None) {
                        chosen = Some(action);
                    }
                }
            });
            vw_ui::menu::bar_menu(ui, tr(lang, "Help"), |ui| {
                if self.menu_item(ui, Action::OpenQualityHelp, None) {
                    chosen = Some(Action::OpenQualityHelp);
                }
                ui.separator();
                for action in [Action::ShowSystemInfo, Action::ShowGradationTest] {
                    if self.menu_item(ui, action, None) {
                        chosen = Some(action);
                    }
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
        for action in [Action::FlagGood, Action::FlagBad, Action::KeepInGroup] {
            if self.menu_item(ui, action, None) {
                chosen = Some(action);
            }
        }
        if self.settings().map.enabled
            && self.menu_item_if(
                ui,
                Action::UndoPosition,
                !self.position_undo.is_empty(),
                "Nothing to undo",
            )
        {
            chosen = Some(Action::UndoPosition);
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
            vw_ui::menu::submenu(ui, tr(lang, "Show only tag"), |ui| {
                for tag in tags {
                    let filter = Filter::Tag(tag.clone());
                    if ui
                        .add(vw_ui::menu::item(self.filter == filter, tag, ""))
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
        chosen
    }

    /// AI tags メニュー。モデルが揃うまでは、タグ付けと顔探しを押せなくする。
    fn ai_menu(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let ready = self.ai_ready();
        let mut chosen = None;
        for action in [Action::AiTagFolder, Action::FindFacesFolder] {
            if self.menu_item_if(ui, action, ready, crate::ai_models::NOT_READY) {
                chosen = Some(action);
            }
        }
        for action in [Action::TidyAiTags, Action::RemoveAiTagsFolder] {
            if self.menu_item(ui, action, None) {
                chosen = Some(action);
            }
        }
        ui.separator();
        for action in [Action::ShowPeople, Action::ShowPeopleGraph] {
            if self.menu_item(ui, action, None) {
                chosen = Some(action);
            }
        }
        if self.menu_item(ui, Action::ToggleFaceFrames, Some(self.show_faces)) {
            chosen = Some(Action::ToggleFaceFrames);
        }
        ui.separator();
        if self.menu_item(ui, Action::ShowAiModels, None) {
            chosen = Some(Action::ShowAiModels);
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
            let button =
                vw_ui::menu::item(current, title, self.shortcut(action).unwrap_or_default());
            if ui.add(button).clicked() {
                chosen = Some(action);
            }
        }
        chosen
    }

    /// メニューの 1 項目。右側に操作の最初のキーを出し、`checked` が `Some(true)` なら左に印を付ける。
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
        let shortcut = self.shortcut(action).unwrap_or_default();
        ui.add(vw_ui::menu::item(checked == Some(true), title, shortcut))
            .clicked()
    }

    /// 押せないときは灰色にし、マウスを乗せると `why`（英語の原文）を出す項目。
    pub(super) fn menu_item_if(
        &self,
        ui: &mut egui::Ui,
        action: Action,
        enabled: bool,
        why: &'static str,
    ) -> bool {
        let title = tr(self.lang(), menu_title(action));
        let button = vw_ui::menu::item(false, title, self.shortcut(action).unwrap_or_default());
        ui.add_enabled(enabled, button)
            .on_disabled_hover_text(tr(self.lang(), why))
            .clicked()
    }

    /// 「画像」メニューの、アクティブなスロットの色の表を選ぶ小メニュー。
    fn color_map_menu(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let index = self.active;
        let applied = self.slots[index]
            .shown
            .as_ref()
            .and_then(|s| s.info.look.as_ref());
        let enabled = applied.is_some();
        let mut pick = None;
        ui.add_enabled_ui(enabled, |ui| {
            vw_ui::menu::submenu(ui, tr(lang, "Color map"), |ui| {
                if let Some(a) = applied {
                    pick = crate::colormap_ui::pick_menu(ui, lang, a, &self.color_rules);
                }
            });
        })
        .response
        .on_disabled_hover_text(tr(lang, "Only for gray images"));
        if let Some(p) = pick {
            self.set_color_pick(index, p);
        }
    }

    /// 左端の、アイコンが縦に並ぶ固定幅の列。
    pub(super) fn tool_column(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let big = |s: &str| egui::RichText::new(s).size(17.0);
        ui.vertical_centered(|ui| {
            ui.add_space(4.0);
            for tool in Tool::ALL {
                let mut tip = match tool.action() {
                    Some(action) => self.with_key(tr(lang, tool.title()), action),
                    None => tr(lang, tool.title()).to_owned(),
                };
                if let Some(hint) = tool.hint() {
                    tip = format!("{tip}\n{}", tr(lang, hint));
                }
                let available = tool.available();
                let r = tool_button(ui, self.tool == tool, available, big(tool.icon()));
                let r = if available {
                    r.on_hover_text(tip)
                } else {
                    r.on_hover_text(format!(
                        "{}\n{}",
                        tr(lang, tool.title()),
                        tr(lang, "Not available yet")
                    ))
                };
                if r.clicked() {
                    self.select_tool(tool);
                }
            }
            let tip = self.with_key(tr(lang, "Tagging mode"), Action::ToggleTagging);
            if tool_button(ui, self.tagging, true, big(icon::TAG))
                .on_hover_text(tip)
                .clicked()
            {
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
            if tool_button(ui, !self.filter.is_all(), true, text)
                .on_hover_text(tip)
                .clicked()
            {
                let next = self.filter.next(&self.settings().labels.tags);
                self.set_filter(ui.ctx(), next);
            }
            let tip = self.with_key(tr(lang, "Color scale gauge"), Action::ToggleColorGauge);
            if tool_button(ui, self.show_gauge, true, big(icon::GRADIENT))
                .on_hover_text(tip)
                .clicked()
            {
                self.show_gauge ^= true;
            }
            ui.separator();
            let tip = self.with_key(tr(lang, "New slot"), Action::NewSlot);
            if tool_button(ui, false, true, big(icon::PLUS_SQUARE))
                .on_hover_text(tip)
                .clicked()
            {
                self.add_slot();
            }
        });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            let tip = self.with_key(tr(lang, "Settings"), Action::ToggleSettings);
            if tool_button(ui, self.show_settings, true, big(icon::GEAR_SIX))
                .on_hover_text(tip)
                .clicked()
            {
                self.toggle_settings();
            }
        });
    }
}

fn tool_button(
    ui: &mut egui::Ui,
    selected: bool,
    enabled: bool,
    icon: egui::RichText,
) -> egui::Response {
    vw_ui::tool::button(ui, selected, enabled, icon, TOOL_BUTTON)
}
