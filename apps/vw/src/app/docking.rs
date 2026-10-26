//! 本体の中のペイン（フォルダ・画像・メタデータ・ログ）と道具の欄（解析・ズームマップ・
//! 画素の変換・相関図・地図・データセット）の並べ替えと、別ウインドウへの出し入れ。
//! 並べ方そのものは vw-dock が持ち、ここは各ペインの中身と見出しを渡す。

use eframe::egui;
use vw_dock::{Axis, Dock, Labels, Node, Panes};

use crate::copy::Focus;
use crate::i18n::tr;
use crate::settings::Language;
use crate::tree;

use super::*;

pub(super) const TREE: &str = "tree";
pub(super) const IMAGES: &str = "images";
pub(super) const METADATA: &str = "metadata";
pub(super) const LOG: &str = "log";
pub(super) const PANES: [&str; 4] = [TREE, IMAGES, METADATA, LOG];

pub(super) const ANALYSIS: &str = "analysis";
pub(super) const ZOOM_MAP: &str = "zoom_map";
pub(super) const PIPELINE: &str = "pixel_pipeline";
pub(super) const CHARTS: &str = "relationship_charts";
pub(super) const MAP: &str = "map";
pub(super) const DATASETS: &str = "datasets";
pub(super) const SKY: &str = "sky_chart";
pub(super) const COMPARE: &str = "compare_images";
/// 出し入れする道具の欄（データセットのほかは、もとは別ウインドウだった）。開いているか
/// どうかは各道具の `open` が持ち、欄の出し入れをそれに合わせる（道具を開く操作があちこちに
/// あり、閉じるときの後始末も道具ごとに違うため）。
pub(super) const TOOLS: [&str; 8] = [
    ANALYSIS, ZOOM_MAP, PIPELINE, CHARTS, MAP, DATASETS, SKY, COMPARE,
];

/// 道具を初めて出すときの大きさ（前の別ウインドウの既定の大きさ）と、前の位置を残した鍵。
fn first_float(tool: &str) -> ([f32; 2], Option<&'static str>) {
    match tool {
        ANALYSIS => ([760.0, 480.0], Some("histogram")),
        ZOOM_MAP => ([640.0, 380.0], Some("zoom_map")),
        PIPELINE => ([1560.0, 560.0], None),
        CHARTS => ([1000.0, 680.0], None),
        DATASETS => ([1100.0, 700.0], None),
        SKY => ([900.0, 640.0], None),
        COMPARE => ([1280.0, 800.0], None),
        _ => ([900.0, 640.0], Some("map")),
    }
}

/// 保存した並びから始める。並びを保存する前の版から来たときは、道具を前の別ウインドウの
/// 位置に出す。
pub(super) fn restored_dock(saved: &crate::state::AppState, monitors_changed: bool) -> Dock {
    let all: Vec<&str> = PANES.iter().chain(TOOLS.iter()).copied().collect();
    let mut dock = match saved.dock.clone() {
        Some(layout) => Dock::restored(default_layout(), &all, layout),
        None => Dock::new(default_layout(), &all),
    };
    if monitors_changed {
        dock.bring_floats_home();
    }
    // 本体の左上から少しずらした所。本体の位置が分からなければ画面の左上寄り。
    let near_main = saved
        .window
        .as_ref()
        .filter(|_| !monitors_changed)
        .and_then(|w| {
            let [x, y] = w.position?;
            Some([x as f32 / w.scale + 80.0, y as f32 / w.scale + 80.0])
        })
        .unwrap_or([120.0, 120.0]);
    for tool in TOOLS {
        if saved.dock.as_ref().is_some_and(|l| l.mentions(tool)) {
            continue;
        }
        let ([w, h], key) = first_float(tool);
        let before = key
            .and_then(|k| saved.sub_windows.get(k))
            .and_then(|s| s.rect)
            .filter(|_| !monitors_changed);
        let rect = before.unwrap_or([near_main[0], near_main[1], w, h]);
        dock.show_next_in_float(tool, rect);
    }
    dock
}

/// 既定の並び。上にフォルダ・画像・メタデータ、下の端から端までログ（今までのパネルの並び）。
pub(super) fn default_layout() -> Node {
    let split = |axis, ratio, a: Node, b: Node| Node::Split {
        axis,
        ratio,
        first: Box::new(a),
        second: Box::new(b),
    };
    let pane = |id: &str| Node::Pane(id.to_owned());
    split(
        Axis::Column,
        0.8,
        split(
            Axis::Row,
            0.2,
            pane(TREE),
            split(Axis::Row, 0.75, pane(IMAGES), pane(METADATA)),
        ),
        pane(LOG),
    )
}

pub(super) fn labels(lang: Language) -> &'static Labels {
    match lang {
        Language::English => &Labels::ENGLISH,
        Language::Japanese => &Labels::JAPANESE,
    }
}

/// ペインの名前（i18n の鍵）。
pub(super) fn pane_name(pane: &str) -> &'static str {
    match pane {
        TREE => "Folders",
        IMAGES => "Images",
        METADATA => "Metadata",
        ANALYSIS => "Analysis",
        ZOOM_MAP => "Zoom map",
        PIPELINE => "Pixel pipeline",
        CHARTS => "Relationship charts",
        MAP => "Map",
        SKY => "Sky chart",
        COMPARE => "Compare images",
        DATASETS => "Datasets",
        _ => "Log",
    }
}

struct AppPanes<'a> {
    app: &'a mut VwApp,
    ctx: egui::Context,
}

/// 中身の余白。今までのパネルの枠（`Frame::side_top_panel`）と同じ左右の余白で、
/// 上は見出しの行が受け持つので空けない。
fn content_frame() -> egui::Frame {
    egui::Frame::new().inner_margin(egui::Margin {
        left: 8,
        right: 8,
        top: 0,
        bottom: 2,
    })
}

impl Panes for AppPanes<'_> {
    fn title(&self, pane: &str) -> String {
        let lang = self.app.lang();
        match pane {
            ANALYSIS => self.app.histogram.title(lang),
            PIPELINE => self.app.pipeline.title(lang),
            COMPARE => self.app.compare.title(lang),
            ZOOM_MAP => {
                crate::sub_window::title(lang, "Zoom map", self.app.active_image().as_deref())
            }
            _ => tr(lang, pane_name(pane)).to_owned(),
        }
    }

    fn has_header(&self, pane: &str) -> bool {
        pane != IMAGES
    }

    fn header(&mut self, ui: &mut egui::Ui, pane: &str) {
        let lang = self.app.lang();
        match pane {
            TREE => {
                // 絞り込み中は、ファイルが隠れていることに気づけるよう見出しに条件を出す。
                if self.app.filter.is_all() {
                    vw_ui::panel::header(ui, tr(lang, "Folders"));
                } else {
                    let text = format!(
                        "{}  ({}: {})",
                        tr(lang, "Folders"),
                        tr(lang, "Filter"),
                        filter_name(&self.app.filter, lang)
                    );
                    vw_ui::panel::header(ui, &text);
                }
            }
            METADATA => self.app.metadata_header_ui(ui),
            LOG => {
                vw_ui::panel::header(ui, tr(lang, "Log"));
                let sending = self.app.send_job.as_ref().and_then(|j| j.progress());
                let mut cancel = false;
                if let Some((done, total)) = sending {
                    ui.weak(format!(
                        "{} {} / {}",
                        tr(lang, "Sending"),
                        thousands(done),
                        thousands(total)
                    ));
                    cancel = ui.small_button(tr(lang, "Cancel sending")).clicked();
                }
                let action = self.app.log_header(ui);
                if cancel {
                    self.app.cancel_send();
                }
                if let Some(action) = action {
                    self.app.log_action(&self.ctx, action);
                }
            }
            _ => {
                let title = self.title(pane);
                vw_ui::panel::header(ui, &title);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, pane: &str) {
        if TOOLS.contains(&pane) && ui.ctx().viewport_id() == egui::ViewportId::ROOT {
            self.app.tool_hovered |= ui.rect_contains_pointer(ui.max_rect());
        }
        match pane {
            IMAGES => self.app.images_ui(ui),
            TREE => {
                content_frame().show(ui, |ui| self.app.tree_pane(ui));
            }
            METADATA => {
                content_frame().show(ui, |ui| self.app.metadata_ui(ui));
            }
            ANALYSIS => {
                content_frame().show(ui, |ui| self.app.histogram_pane(ui));
            }
            ZOOM_MAP => {
                content_frame().show(ui, |ui| self.app.zoom_map_pane(ui));
            }
            PIPELINE => self.app.pipeline_pane(ui),
            CHARTS => self.app.people_graph_pane(ui),
            MAP => self.app.map_pane(ui),
            SKY => self.app.sky_pane(ui),
            COMPARE => self.app.compare_pane(ui),
            DATASETS => self.app.datasets_pane(ui),
            _ => {
                content_frame().show(ui, |ui| self.app.log_ui(ui));
            }
        }
    }
}

impl VwApp {
    /// 本体の中のペインと別ウインドウを出す。
    pub(super) fn docked_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut dock) = self.dock.take() else {
            return;
        };
        let labels = labels(self.lang());
        let ctx = ui.ctx().clone();
        self.tool_hovered = false;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            dock.show(
                ui,
                &mut AppPanes {
                    app: self,
                    ctx: ctx.clone(),
                },
                labels,
            );
        });
        self.log_dock(&mut dock);
        self.dock = Some(dock);
        self.close_hidden_tools();
    }

    /// 本体の中を設定画面などに使っている間も、別ウインドウに出したペインは出しておく。
    pub(super) fn floating_panes_ui(&mut self, ctx: &egui::Context) {
        let Some(mut dock) = self.dock.take() else {
            return;
        };
        let labels = labels(self.lang());
        self.tool_hovered = false;
        dock.show_floats(
            ctx,
            &mut AppPanes {
                app: self,
                ctx: ctx.clone(),
            },
            labels,
        );
        self.log_dock(&mut dock);
        self.dock = Some(dock);
        self.close_hidden_tools();
    }

    /// 前回出したまま終えた道具を開き直す。開いているかは道具の `open` が持つので、合わせないと
    /// 最初のフレームで隠され、ドッキングした場所に出ない。
    pub(super) fn reopen_tools(&mut self) {
        let tools = self.dock.as_ref().map(tools_to_reopen).unwrap_or_default();
        for tool in tools {
            *self.tool_open(tool) = true;
        }
    }

    fn tool_open(&mut self, tool: &str) -> &mut bool {
        match tool {
            ANALYSIS => &mut self.histogram.open,
            ZOOM_MAP => &mut self.zoom_map.open,
            PIPELINE => &mut self.pipeline.open,
            CHARTS => &mut self.graph_window.open,
            DATASETS => &mut self.show_datasets,
            SKY => &mut self.sky.open,
            COMPARE => &mut self.compare.open,
            _ => &mut self.map.open,
        }
    }

    /// 道具の開閉を欄の出し入れに映す。描く前に毎フレーム呼ぶ。
    pub(super) fn sync_tool_panes(&mut self) {
        if std::mem::take(&mut self.pipeline.open_histogram) {
            self.histogram.open = true;
        }
        if std::mem::take(&mut self.pipeline.open_zoom_map) {
            self.zoom_map.open = true;
        }
        for tool in TOOLS {
            let open = *self.tool_open(tool);
            if let Some(dock) = self.dock.as_mut() {
                dock.set_shown(tool, open);
            }
        }
    }

    /// 欄の側で閉じた道具（別ウインドウの × や、見出しの右クリックの「隠す」）の後始末をする。
    fn close_hidden_tools(&mut self) {
        for tool in TOOLS {
            if !*self.tool_open(tool) || self.pane_shown(tool) {
                continue;
            }
            match tool {
                ANALYSIS => self.histogram.close(),
                PIPELINE => {
                    let commands = self.pipeline.close();
                    self.apply_pipeline(commands);
                }
                CHARTS => self.people_graph_closed(),
                _ => *self.tool_open(tool) = false,
            }
        }
    }

    fn log_dock(&mut self, dock: &mut Dock) {
        for line in dock.take_log() {
            let text = format!("Panes: {}", line.text);
            if line.summary {
                self.log.info(text);
            } else {
                self.log.detail(crate::log::Level::Info, text);
            }
        }
    }

    pub(super) fn pane_shown(&self, pane: &str) -> bool {
        self.dock.as_ref().is_some_and(|d| d.is_shown(pane))
    }

    pub(super) fn toggle_pane(&mut self, pane: &str) {
        if let Some(dock) = self.dock.as_mut() {
            let shown = dock.is_shown(pane);
            dock.set_shown(pane, !shown);
        }
    }

    /// 表示メニューの「ペインの配置」。見出しのない画像のペインはここから動かす。
    pub(super) fn panes_menu(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let labels = labels(lang);
        let Some(dock) = self.dock.as_mut() else {
            return;
        };
        for pane in PANES.into_iter().chain(TOOLS) {
            if !dock.is_shown(pane) {
                continue;
            }
            vw_ui::menu::submenu(ui, tr(lang, pane_name(pane)), |ui| {
                dock.pane_menu(ui, pane, labels);
            });
        }
        ui.separator();
        if ui.add(vw_ui::menu::item(false, labels.reset, "")).clicked() {
            dock.reset();
        }
    }

    /// フォルダツリーの欄の中身。
    pub(super) fn tree_pane(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        // フレームを出しているときは、そのファイルのシリーズを選択として見せる。
        let active_image = self.active_image().map(|p| frames::file_of(&p));
        let destinations = self.destination_names();
        let apps = self.send_apps().to_vec();
        let ai_ready = self.ai_ready();
        let click_expands = self.settings.settings.tree.click_expands;
        let drawn = egui::ScrollArea::both()
            .auto_shrink(false)
            .show(ui, |ui| {
                self.tree.ui(
                    ui,
                    tree::Context {
                        registry: self.loader.registry(),
                        series: &self.series,
                        current: self.current.as_deref(),
                        active_image: active_image.as_deref(),
                        lang,
                        destinations: &destinations,
                        apps: &apps,
                        ai_ready,
                        click_expands,
                    },
                )
            })
            .inner;
        let ctx = ui.ctx().clone();
        for (dir, files) in drawn.scans {
            let registry = self.loader.registry().clone();
            let wake = ctx.clone();
            self.series
                .start(dir, files, registry, move || wake.request_repaint());
        }
        let Some(action) = drawn.action else {
            return;
        };
        match action {
            TreeAction::Cd(p) => self.open_folder(&ctx, p),
            TreeAction::Dataset(request) => self.open_dataset_dialog(&ctx, request),
            TreeAction::Send { choice, targets } => self.send_to(choice, targets),
            TreeAction::Mount(zip) => self.mount(&ctx, &zip),
            TreeAction::Unmount(dir) => self.unmount(&ctx, &dir),
            TreeAction::Series { folder, files } => self.show_series(&ctx, folder, &files),
            TreeAction::Drop { file, dir } => {
                self.drop_file(&ctx, file, dir);
                return;
            }
            TreeAction::OpenInFileManager(p) => {
                self.open_in_file_manager(&p);
                return;
            }
            TreeAction::AiTagFolder(p) => {
                self.ai_tag_folder(&ctx, &p);
                return;
            }
            TreeAction::FindFaces(p) => {
                self.find_faces_folder(&ctx, &p);
                return;
            }
            TreeAction::RemoveAiTags(p) => {
                self.ask_remove_ai_tags(&p);
                return;
            }
            TreeAction::Copy(p) => {
                let what = format!("path {}", p.display());
                self.copy_text(&ctx, p.display().to_string(), &what);
                return;
            }
        }
        self.set_focus(&ctx, Focus::Tree);
    }
}

/// 並びに出ている道具。データセットは開き直さない。開くたびにカタログを探し直すので、
/// ネットワークのドライブでは起動が遅くなるため。画像の比較も、比べた画像を残していないので開き直さない。
fn tools_to_reopen(dock: &Dock) -> Vec<&'static str> {
    TOOLS
        .into_iter()
        .filter(|t| *t != DATASETS && *t != COMPARE && dock.is_shown(t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    #[test]
    fn tools_start_hidden_and_main_panes_shown() {
        let dock = restored_dock(&AppState::default(), false);
        assert!(TOOLS.iter().all(|t| !dock.is_shown(t)));
        assert!(PANES.iter().all(|p| dock.is_shown(p)));
    }

    #[test]
    fn a_tool_first_opens_where_its_old_window_was() {
        let mut saved = AppState::default();
        saved.sub_windows.insert(
            "map".into(),
            crate::sub_window::Saved {
                rect: Some([5.0, 6.0, 700.0, 500.0]),
                on_top: false,
            },
        );
        let mut dock = restored_dock(&saved, false);
        dock.set_shown(MAP, true);
        assert_eq!(dock.layout().floats[0].rect, [5.0, 6.0, 700.0, 500.0]);
    }

    #[test]
    fn a_tool_already_in_the_saved_layout_keeps_its_place() {
        let mut first = restored_dock(&AppState::default(), false);
        first.set_shown(MAP, true);
        first.dock(MAP);
        let saved = AppState {
            dock: Some(first.layout()),
            ..AppState::default()
        };
        let dock = restored_dock(&saved, false);
        assert_eq!(dock.window_of(MAP), Some(vw_dock::Window::Main));
    }

    #[test]
    fn tools_left_shown_at_exit_open_again_except_datasets() {
        let mut first = restored_dock(&AppState::default(), false);
        for tool in [PIPELINE, ANALYSIS, DATASETS] {
            first.set_shown(tool, true);
            first.dock(tool);
        }
        first.set_shown(ZOOM_MAP, true);
        let saved = AppState {
            dock: Some(first.layout()),
            ..AppState::default()
        };
        let dock = restored_dock(&saved, false);
        assert_eq!(tools_to_reopen(&dock), [ANALYSIS, ZOOM_MAP, PIPELINE]);
    }
}
