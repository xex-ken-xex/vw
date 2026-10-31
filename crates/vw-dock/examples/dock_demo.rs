//! vw-dock の試し。vw と同じ並び（フォルダ・画像・メタデータ・ログ）の色付きの箱を、
//! ドラッグで並べ替えたり、別ウインドウへ引き出したり戻したりする。
//!
//! 動きの記録はログ欄と、exe の隣の `vw-dock-demo-<日時>.log` に出す。並びは終了時に
//! exe の隣の `vw-dock-demo-layout.json` に残し、次に起動したときに戻す。
//!
//! 実行: `cargo run -p vw-dock --example dock_demo`

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use serde::{Deserialize, Serialize};
use vw_dock::{Axis, Dock, Labels, Layout, Node, Panes};

const PANES: [&str; 9] = [
    "tree",
    "images",
    "metadata",
    "log",
    "histogram",
    "zoom_map",
    "pipeline",
    "people",
    "map",
];

fn title(pane: &str) -> &'static str {
    match pane {
        "tree" => "フォルダ",
        "images" => "画像",
        "metadata" => "メタデータ",
        "log" => "ログ",
        "histogram" => "ヒストグラム",
        "zoom_map" => "ズームマップ",
        "pipeline" => "画素の変換",
        "people" => "相関図",
        "map" => "地図",
        _ => "?",
    }
}

fn default_layout() -> Node {
    let split = |axis, ratio, a: Node, b: Node| Node::Split {
        axis,
        ratio,
        first: Box::new(a),
        second: Box::new(b),
    };
    let pane = |id: &str| Node::Pane(id.to_owned());
    split(
        Axis::Column,
        0.78,
        split(
            Axis::Row,
            0.18,
            pane("tree"),
            split(Axis::Row, 0.76, pane("images"), pane("metadata")),
        ),
        pane("log"),
    )
}

#[derive(Serialize, Deserialize)]
struct Saved {
    layout: Layout,
    /// 保存したときの本体のモニタの大きさ。変わっていたら別ウインドウを本体へ戻す。
    monitor: Option<[f32; 2]>,
}

struct Log {
    lines: Vec<String>,
    file: Option<File>,
    path: PathBuf,
    start: Instant,
}

impl Log {
    fn line(&mut self, text: &str) {
        let t = self.start.elapsed().as_secs_f64();
        let line = format!("[{t:9.3}] {text}");
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
        self.lines.push(line);
    }
}

struct Demo {
    dock: Dock,
    log: Log,
    layout_path: PathBuf,
    restored_monitor: Option<[f32; 2]>,
    checked_monitor: bool,
    follow_log: bool,
    ctx: egui::Context,
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// アイコン（メニューのチェック）と、OS にある日本語フォント。vw 本体と同じ候補から探す。
fn install_fonts(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        "C:\\Windows\\Fonts\\YuGothM.ttc",
        "C:\\Windows\\Fonts\\meiryo.ttc",
        "C:\\Windows\\Fonts\\msgothic.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/ipaexfont-gothic/ipaexg.ttf",
        "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
    ];
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    if let Some(bytes) = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert(
            "ja".into(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("ja".into());
        }
    }
    ctx.set_fonts(fonts);
}

impl Demo {
    fn new(cc: &eframe::CreationContext) -> Self {
        vw_ui::theme::apply(&cc.egui_ctx, egui::ThemePreference::System, true, 1.0);
        install_fonts(&cc.egui_ctx);
        let dir = exe_dir();
        let path = dir.join(format!("vw-dock-demo-{}.log", stamp()));
        let file = File::create(&path).ok();
        let mut log = Log {
            lines: Vec::new(),
            file,
            path,
            start: Instant::now(),
        };
        log.line(&format!(
            "vw-dock demo {} on {} {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
        log.line(&format!("log file: {}", log.path.display()));
        let layout_path = dir.join("vw-dock-demo-layout.json");
        let saved: Option<Saved> = std::fs::read_to_string(&layout_path).ok().and_then(|s| {
            match serde_json::from_str(&s) {
                Ok(v) => Some(v),
                Err(e) => {
                    log.line(&format!("layout file unreadable, using default: {e}"));
                    None
                }
            }
        });
        let (dock, restored_monitor) = match saved {
            Some(s) => {
                log.line(&format!("layout file: {}", layout_path.display()));
                (
                    Dock::restored(default_layout(), &PANES, s.layout),
                    s.monitor,
                )
            }
            None => {
                log.line("no saved layout: default layout");
                (Dock::new(default_layout(), &PANES), None)
            }
        };
        Self {
            dock,
            log,
            layout_path,
            restored_monitor,
            checked_monitor: false,
            follow_log: true,
            ctx: cc.egui_ctx.clone(),
        }
    }

    fn save(&mut self, ctx: &egui::Context) {
        let monitor = ctx
            .input_for(egui::ViewportId::ROOT, |i| i.viewport().monitor_size)
            .map(|v| [v.x, v.y]);
        let saved = Saved {
            layout: self.dock.layout(),
            monitor,
        };
        match serde_json::to_string_pretty(&saved) {
            Ok(text) => match std::fs::write(&self.layout_path, text) {
                Ok(()) => self
                    .log
                    .line(&format!("saved layout to {}", self.layout_path.display())),
                Err(e) => self.log.line(&format!("could not save layout: {e}")),
            },
            Err(e) => self.log.line(&format!("could not save layout: {e}")),
        }
    }
}

struct Content<'a> {
    log: &'a Log,
    follow_log: &'a mut bool,
}

/// 箱の色。ペインの見分けがつくよう、名前ごとに少しずつ色を変える。
fn tint(pane: &str) -> egui::Color32 {
    let i = PANES.iter().position(|p| *p == pane).unwrap_or(0) as f32;
    let hue = (i * 0.137).fract();
    egui::ecolor::Hsva::new(hue, 0.22, 0.26, 1.0).into()
}

impl Panes for Content<'_> {
    fn title(&self, pane: &str) -> String {
        title(pane).to_owned()
    }

    fn has_header(&self, pane: &str) -> bool {
        pane != "images"
    }

    fn has_header_controls(&self, pane: &str) -> bool {
        pane == "log"
    }

    fn header_buttons(&mut self, ui: &mut egui::Ui, pane: &str) {
        if pane == "log" {
            ui.checkbox(self.follow_log, "追う");
            if ui.small_button("すべてコピー").clicked() {
                ui.ctx().copy_text(self.log.lines.join("\n"));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, pane: &str) {
        let rect = ui.max_rect();
        match pane {
            "log" => {
                egui::ScrollArea::both()
                    .auto_shrink(false)
                    .stick_to_bottom(*self.follow_log)
                    .show(ui, |ui| {
                        for line in &self.log.lines {
                            ui.add(
                                egui::Label::new(egui::RichText::new(line).monospace().size(11.0))
                                    .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                    });
            }
            "images" => {
                let gap = 1.0;
                let half = (rect.size() - egui::vec2(gap, gap)) / 2.0;
                for (i, name) in [
                    "coffee.png",
                    "astronaut_small.png",
                    "astronaut_mirror.jpg",
                    "astronaut.png",
                ]
                .iter()
                .enumerate()
                {
                    let x = (i % 2) as f32 * (half.x + gap);
                    let y = (i / 2) as f32 * (half.y + gap);
                    let r = egui::Rect::from_min_size(rect.min + egui::vec2(x, y), half);
                    ui.painter().rect_filled(r, 0.0, tint(name));
                    ui.painter().text(
                        r.min + egui::vec2(4.0, 2.0),
                        egui::Align2::LEFT_TOP,
                        *name,
                        egui::FontId::proportional(11.0),
                        ui.visuals().text_color(),
                    );
                }
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "画像（見出しなし）: メニューの「画像」から動かす",
                    egui::FontId::proportional(12.0),
                    ui.visuals().strong_text_color(),
                );
            }
            _ => {
                let r = rect.shrink(6.0);
                ui.painter()
                    .rect_filled(r, 0.0, tint(pane).gamma_multiply(0.6));
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    format!(
                        "{}\n{:.0} x {:.0}",
                        title(pane),
                        rect.width(),
                        rect.height()
                    ),
                    egui::FontId::proportional(12.0),
                    ui.visuals().strong_text_color(),
                );
            }
        }
    }
}

impl eframe::App for Demo {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.checked_monitor {
            let now = ctx.input(|i| i.viewport().monitor_size).map(|v| [v.x, v.y]);
            if now.is_some() {
                self.checked_monitor = true;
                if self.restored_monitor.is_some() && self.restored_monitor != now {
                    self.log.line(&format!(
                        "monitor size changed {:?} -> {now:?}",
                        self.restored_monitor
                    ));
                    self.dock.bring_floats_home();
                }
            }
        }
        let labels = &Labels::JAPANESE;
        egui::Panel::top("menu").show(ui, |ui| {
            vw_ui::menu::bar(ui, |ui| {
                vw_ui::menu::button(ui, "表示", |ui| {
                    for pane in PANES {
                        let shown = self.dock.is_shown(pane);
                        if ui.add(vw_ui::menu::item(shown, title(pane), "")).clicked() {
                            self.dock.set_shown(pane, !shown);
                        }
                    }
                    ui.separator();
                    if ui.add(vw_ui::menu::item(false, labels.reset, "")).clicked() {
                        self.dock.reset();
                    }
                    if ui
                        .add(vw_ui::menu::item(
                            false,
                            "別ウインドウをすべて本体に戻す",
                            "",
                        ))
                        .clicked()
                    {
                        self.dock.bring_floats_home();
                    }
                    if ui
                        .add(vw_ui::menu::item(false, "今の並びを保存", ""))
                        .clicked()
                    {
                        self.save(&ctx);
                    }
                });
                vw_ui::menu::button(ui, "画像", |ui| {
                    self.dock.pane_menu(ui, "images", labels);
                });
            });
        });
        egui::Panel::left("tools")
            .exact_size(26.0)
            .resizable(false)
            .show(ui, |ui| {
                ui.add_space(4.0);
                for _ in 0..5 {
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    ui.painter().rect_stroke(
                        r,
                        0.0,
                        ui.visuals().widgets.inactive.fg_stroke,
                        egui::StrokeKind::Inside,
                    );
                    ui.add_space(6.0);
                }
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let mut content = Content {
                log: &self.log,
                follow_log: &mut self.follow_log,
            };
            self.dock.show(ui, &mut content, labels);
        });
        for line in self.dock.take_log() {
            self.log.line(&line.text);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let ctx = self.ctx.clone();
        self.save(&ctx);
        self.log.line("exit");
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("vw-dock デモ")
            .with_inner_size([1200.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        "vw-dock デモ",
        options,
        Box::new(|cc| Ok(Box::new(Demo::new(cc)))),
    )
}
