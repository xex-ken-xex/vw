//! Background decoding: file -> VwImage (plugin) -> RGBA8 display image, off the UI thread.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage};
use vw_core::{display, DisplayParams, Registry, VwImage};

pub struct Loaded {
    pub info: ImageInfo,
    pub display: ColorImage,
}

/// Everything about a loaded image except the display buffer, which moves into a texture.
pub struct ImageInfo {
    pub image: Arc<VwImage>,
    pub plugin: String,
    pub file_size: u64,
    pub read: Duration,
    pub decode: Duration,
    pub convert: Duration,
}

pub struct LoadResult {
    pub slot: u64,
    pub request: u64,
    pub path: PathBuf,
    pub result: Result<Loaded, String>,
}

pub struct Loader {
    registry: Registry,
    tx: Sender<LoadResult>,
    pub rx: Receiver<LoadResult>,
    ctx: egui::Context,
}

impl Loader {
    pub fn new(registry: Registry, ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            registry,
            tx,
            rx,
            ctx,
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn request(&self, slot: u64, request: u64, path: PathBuf) {
        let (registry, tx, ctx) = (self.registry.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = load(&registry, &path);
            let _ = tx.send(LoadResult {
                slot,
                request,
                path,
                result,
            });
            ctx.request_repaint();
        });
    }
}

fn load(registry: &Registry, path: &std::path::Path) -> Result<Loaded, String> {
    let t0 = Instant::now();
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let t1 = Instant::now();
    let (image, plugin) = registry
        .decode_bytes(path, &bytes)
        .map_err(|e| e.to_string())?;
    let plugin = plugin.to_owned();
    let t2 = Instant::now();
    let d = display::to_display(&image, &DisplayParams::default());
    let display =
        ColorImage::from_rgba_unmultiplied([d.width as usize, d.height as usize], &d.rgba);
    let t3 = Instant::now();
    Ok(Loaded {
        info: ImageInfo {
            image: Arc::new(image),
            plugin,
            file_size: bytes.len() as u64,
            read: t1 - t0,
            decode: t2 - t1,
            convert: t3 - t2,
        },
        display,
    })
}
