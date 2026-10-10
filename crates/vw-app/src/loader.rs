//! 画像の読み込み。ファイル → VwImage（プラグイン）→ RGBA8 の変換を UI スレッドの外で行う。
//!
//! UI スレッドで読み込むと、大きな画像の間ウィンドウが固まるため。

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage};
use vw_core::{display, DisplayParams, Registry, VwImage};

/// 読み込みが成功したときの結果。
pub struct Loaded {
    pub info: ImageInfo,
    pub display: ColorImage,
}

/// 読み込んだ画像の情報。描画用のバッファはテクスチャに移すので別に持つ。
/// 各段階の所要時間はログに出して速度を確認するのに使う。
pub struct ImageInfo {
    pub image: Arc<VwImage>,
    pub plugin: String,
    pub file_size: u64,
    pub read: Duration,
    pub decode: Duration,
    pub convert: Duration,
}

/// 読み込みスレッドから UI スレッドへ送る結果。どのスロットのどの要求かを添える。
pub struct LoadResult {
    pub slot: u64,
    pub request: u64,
    pub path: PathBuf,
    pub result: Result<Loaded, String>,
}

/// 読み込み要求を受け付け、結果をチャネルで返す。
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

    /// `path` の読み込みを別スレッドで始める。終わったら再描画を要求して結果を受け取らせる。
    ///
    /// 要求ごとにスレッドを立てる。スレッドプールにしないのは、同時に読む枚数が
    /// スロットの数程度と少なく、立ち上げの費用（数十マイクロ秒）が読み込みに比べて無視できるため。
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

/// 1 枚を読み込む。ファイルは一度に全部読む。メモリマップにしないのは、
/// 読み込み中にファイルが書き換えられると未定義動作になりうるため。
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{png_registry, write_png, TempDir};

    #[test]
    fn loads_png_into_image_and_display_buffer() {
        let dir = TempDir::new();
        let path = dir.path().join("red.png");
        write_png(&path, 2, 1, &[255, 0, 0, 255, 0, 0]);
        let loaded = load(&png_registry(), &path).unwrap();
        assert_eq!(loaded.info.plugin, "PNG");
        assert_eq!((loaded.info.image.width, loaded.info.image.height), (2, 1));
        assert_eq!(
            loaded.info.file_size,
            std::fs::metadata(&path).unwrap().len()
        );
        assert_eq!(loaded.display.size, [2, 1]);
        assert_eq!(loaded.display.pixels[0], egui::Color32::RED);
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = TempDir::new();
        assert!(load(&png_registry(), &dir.path().join("none.png")).is_err());
    }

    #[test]
    fn unsupported_file_is_an_error() {
        let dir = TempDir::new();
        let path = dir.touch("note.txt");
        let Err(err) = load(&png_registry(), &path) else {
            panic!("a text file must not load");
        };
        assert!(err.contains("no plugin"), "{err}");
    }
}
