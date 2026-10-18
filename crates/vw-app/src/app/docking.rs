//! 本体の中のペイン（フォルダ・画像・メタデータ・ログ）の並べ替えと、別ウインドウへの出し入れ。
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
        tr(self.app.lang(), pane_name(pane)).to_owned()
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
            _ => {
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
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, pane: &str) {
        match pane {
            IMAGES => self.app.images_ui(ui),
            TREE => {
                content_frame().show(ui, |ui| self.app.tree_pane(ui));
            }
            METADATA => {
                content_frame().show(ui, |ui| self.app.metadata_ui(ui));
            }
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
    }

    /// 本体の中を設定画面などに使っている間も、別ウインドウに出したペインは出しておく。
    pub(super) fn floating_panes_ui(&mut self, ctx: &egui::Context) {
        let Some(mut dock) = self.dock.take() else {
            return;
        };
        let labels = labels(self.lang());
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
        for pane in PANES {
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
