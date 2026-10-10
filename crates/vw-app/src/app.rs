use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2, Color32, Key, Rect, TextureFilter, TextureOptions};
use vw_core::{ColorModel, Registry};

use crate::loader::{LoadResult, Loader};
use crate::log::{Level, Log};
use crate::tree::{self, FolderTree, TreeAction};
use crate::viewer::{Shown, Slot};

/// Which of the four fields are visible (View menu).
struct Fields {
    tree: bool,
    metadata: bool,
    images: bool,
    log: bool,
}

pub struct VwApp {
    loader: Loader,
    tree: FolderTree,
    slots: Vec<Slot>,
    active: usize,
    next_id: u64,
    fields: Fields,
    log: Log,
    started: Instant,
    first_frame: bool,
}

impl VwApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        registry: Registry,
        initial: Vec<PathBuf>,
        started: Instant,
    ) -> Self {
        crate::fonts::load_cjk_in_background(&cc.egui_ctx);
        let mut log = Log::new(started);
        log.info(format!(
            "Plugins: {}",
            registry
                .plugins()
                .iter()
                .map(|p| p.name())
                .collect::<Vec<_>>()
                .join(", ")
        ));

        // A folder argument becomes the first tree root; file arguments open in slots.
        let (dirs, files): (Vec<PathBuf>, Vec<PathBuf>) =
            initial.into_iter().partition(|p| p.is_dir());
        let root = dirs.into_iter().next().or_else(|| {
            files
                .first()
                .and_then(|f| f.parent().map(Path::to_path_buf))
        });

        let mut app = Self {
            loader: Loader::new(registry, cc.egui_ctx.clone()),
            tree: FolderTree::new(root),
            slots: vec![Slot::new(0)],
            active: 0,
            next_id: 1,
            fields: Fields {
                tree: true,
                metadata: true,
                images: true,
                log: true,
            },
            log,
            started,
            first_frame: true,
        };
        for (i, f) in files.into_iter().enumerate() {
            if i > 0 {
                app.add_slot();
            }
            app.open_in_active(f);
        }
        app
    }

    fn add_slot(&mut self) {
        self.slots.push(Slot::new(self.next_id));
        self.next_id += 1;
        self.active = self.slots.len() - 1;
    }

    fn close_slot(&mut self, index: usize) {
        self.slots.remove(index);
        if self.slots.is_empty() {
            self.add_slot();
        }
        self.active = self.active.min(self.slots.len() - 1);
    }

    fn open_in_active(&mut self, path: PathBuf) {
        let request = self.next_id;
        self.next_id += 1;
        let slot = &mut self.slots[self.active];
        slot.path = Some(path.clone());
        slot.request = request;
        slot.loading = true;
        slot.error = None;
        self.loader.request(slot.id, request, path);
    }

    /// Previous/next supported file in the active slot's folder.
    fn step_active(&mut self, delta: isize) {
        let Some(path) = self.slots[self.active].path.clone() else {
            return;
        };
        let files = tree::sibling_images(&path, self.loader.registry());
        let Some(pos) = files.iter().position(|p| p == &path) else {
            return;
        };
        let next = (pos as isize + delta).rem_euclid(files.len() as isize) as usize;
        if next != pos {
            self.open_in_active(files[next].clone());
        }
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(LoadResult {
            slot,
            request,
            path,
            result,
        }) = self.loader.rx.try_recv()
        {
            let Some(s) = self
                .slots
                .iter_mut()
                .find(|s| s.id == slot && s.request == request)
            else {
                continue;
            };
            s.loading = false;
            match result {
                Ok(loaded) => {
                    let t = Instant::now();
                    let texture = ctx.load_texture(
                        path.to_string_lossy(),
                        loaded.display,
                        TextureOptions {
                            magnification: TextureFilter::Nearest,
                            minification: TextureFilter::Linear,
                            mipmap_mode: Some(TextureFilter::Linear),
                            ..TextureOptions::LINEAR
                        },
                    );
                    let i = &loaded.info;
                    self.log.info(format!(
                        "{}: {}x{} {} | read {} / decode {} / convert {} / texture {}",
                        path.display(),
                        i.image.width,
                        i.image.height,
                        i.plugin,
                        ms(i.read),
                        ms(i.decode),
                        ms(i.convert),
                        ms(t.elapsed()),
                    ));
                    if texture.size()[0] < i.image.width as usize {
                        self.log.warn(format!(
                            "{}: shown downsampled to {}x{} (texture size limit)",
                            path.display(),
                            texture.size()[0],
                            texture.size()[1]
                        ));
                    }
                    s.shown = Some(Shown {
                        texture,
                        info: loaded.info,
                    });
                    s.error = None;
                    s.reset_view();
                }
                Err(e) => {
                    self.log.error(format!("{}: {e}", path.display()));
                    s.shown = None;
                    s.error = Some(e);
                }
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New slot").clicked() {
                    self.add_slot();
                }
                if ui.button("Close all slots").clicked() {
                    self.slots.clear();
                    self.add_slot();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.fields.tree, "Folder tree");
                ui.checkbox(&mut self.fields.metadata, "Metadata");
                ui.checkbox(&mut self.fields.images, "Images");
                ui.checkbox(&mut self.fields.log, "Log");
            });
        });
    }

    fn metadata_ui(&self, ui: &mut egui::Ui) {
        let slot = &self.slots[self.active];
        let Some(shown) = &slot.shown else {
            ui.weak("No image");
            return;
        };
        let info = &shown.info;
        let img = &info.image;
        let mut groups: Vec<(&str, Vec<(String, String)>)> = vec![(
            "File",
            vec![
                (
                    "Path".into(),
                    slot.path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                ),
                ("Size".into(), format!("{} bytes", info.file_size)),
                ("Plugin".into(), info.plugin.clone()),
                ("Pixels".into(), format!("{} x {}", img.width, img.height)),
                ("Color".into(), color_name(img.color).into()),
                (
                    "Sample".into(),
                    format!(
                        "{} bit ({} significant)",
                        img.data.bits_per_sample(),
                        img.significant_bits
                    ),
                ),
                (
                    "Texture".into(),
                    format!("{} x {}", shown.texture.size()[0], shown.texture.size()[1]),
                ),
            ],
        )];
        for (group, key, value) in &img.metadata.entries {
            match groups.iter_mut().find(|(g, _)| g == group) {
                Some((_, rows)) => rows.push((key.clone(), value.clone())),
                None => groups.push((group.as_str(), vec![(key.clone(), value.clone())])),
            }
        }
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (group, rows) in groups {
                    egui::CollapsingHeader::new(group)
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::Grid::new(group)
                                .num_columns(2)
                                .striped(true)
                                .show(ui, |ui| {
                                    for (k, v) in rows {
                                        ui.label(k);
                                        ui.add(egui::Label::new(v).wrap());
                                        ui.end_row();
                                    }
                                });
                        });
                }
            });
    }

    fn log_ui(&self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (t, level, msg) in &self.log.lines {
                    let color = match level {
                        Level::Info => ui.visuals().text_color(),
                        Level::Warn => ui.visuals().warn_fg_color,
                        Level::Error => ui.visuals().error_fg_color,
                    };
                    ui.label(
                        egui::RichText::new(format!("[{t:8.3}] {msg}"))
                            .monospace()
                            .color(color),
                    );
                }
            });
    }

    fn images_ui(&mut self, ui: &mut egui::Ui) {
        let area = ui.available_rect_before_wrap();
        let n = self.slots.len();
        let cols = (n as f32).sqrt().ceil() as usize;
        let rows = n.div_ceil(cols);
        let cell = vec2(area.width() / cols as f32, area.height() / rows as f32);
        let mut close = None;
        for i in 0..n {
            let (c, r) = (i % cols, i / cols);
            let rect = Rect::from_min_size(
                pos2(
                    area.min.x + c as f32 * cell.x,
                    area.min.y + r as f32 * cell.y,
                ),
                cell,
            );
            let resp = self.slots[i].ui(ui, rect.shrink(1.0), i == self.active);
            if resp.clicked {
                self.active = i;
            }
            if resp.close {
                close = Some(i);
            }
        }
        if let Some(i) = close {
            self.close_slot(i);
        }
        ui.allocate_rect(area, egui::Sense::hover());
    }
}

impl eframe::App for VwApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.first_frame {
            self.first_frame = false;
            self.log.info(format!(
                "Startup to first frame: {}",
                ms(self.started.elapsed())
            ));
        }
        self.receive(&ctx);

        if !ctx.egui_wants_keyboard_input() {
            let (prev, next) = ctx.input(|i| {
                (
                    i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::PageUp),
                    i.key_pressed(Key::ArrowRight)
                        || i.key_pressed(Key::PageDown)
                        || i.key_pressed(Key::Space),
                )
            });
            if prev {
                self.step_active(-1);
            } else if next {
                self.step_active(1);
            }
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));

        if self.fields.log {
            egui::Panel::bottom("log")
                .resizable(true)
                .default_size(140.0)
                .show(ui, |ui| {
                    ui.strong("Log");
                    self.log_ui(ui);
                });
        }
        if self.fields.tree {
            egui::Panel::left("tree")
                .default_size(260.0)
                .show(ui, |ui| {
                    ui.strong("Folders");
                    let selected = self.slots[self.active].path.clone();
                    let action = egui::ScrollArea::both()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            self.tree
                                .ui(ui, self.loader.registry(), selected.as_deref())
                        })
                        .inner;
                    match action {
                        Some(TreeAction::Open(p)) => self.open_in_active(p),
                        Some(TreeAction::Add(p)) => {
                            self.add_slot();
                            self.open_in_active(p);
                        }
                        None => {}
                    }
                });
        }
        if self.fields.metadata {
            egui::Panel::right("metadata")
                .default_size(300.0)
                .min_size(220.0)
                .show(ui, |ui| {
                    ui.strong("Metadata");
                    self.metadata_ui(ui);
                });
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if self.fields.images {
                self.images_ui(ui);
            } else {
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, Color32::from_gray(24));
            }
        });
    }
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

fn color_name(c: ColorModel) -> &'static str {
    match c {
        ColorModel::Gray => "Gray",
        ColorModel::GrayAlpha => "Gray + alpha",
        ColorModel::Rgb => "RGB",
        ColorModel::Rgba => "RGBA",
    }
}
