//! vw: Linux / Windows 向けの高速な画像ビューワ。
//!
//! 使い方: `vw [フォルダ または 画像ファイル ...]`

// Windows のリリースビルドでは GUI サブシステムにして、起動時に黒いコンソール窓を出さない。
// デバッグビルドで残すのは、panic メッセージなどを端末で読めるようにするため。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod archive;
mod autoload;
mod catalog;
mod copy;
mod dataset_extract;
mod dataset_search;
mod dataset_ui;
mod datasets_screen;
mod dicom_send;
mod discard;
mod export;
mod fonts;
mod frames;
mod fullscreen;
mod i18n;
mod keymap;
mod labels;
mod launch;
mod layout;
mod loader;
mod log;
mod memory;
mod probe;
mod series;
mod settings;
mod settings_ui;
mod slideshow;
mod sort;
mod state;
mod theme;
mod toml_reader;
mod transfer;
mod tree;
mod viewer;
mod watch;

#[cfg(test)]
mod test_util;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use vw_core::Registry;

/// 組み込みのデコーダプラグインを登録する。新しい形式はここに 1 行足す。
fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(vw_png::PngPlugin));
    registry.register(Arc::new(vw_jpeg::JpegPlugin));
    registry.register(Arc::new(vw_bmp::BmpPlugin));
    registry.register(Arc::new(vw_webp::WebpPlugin::default()));
    registry.register(Arc::new(vw_gif::GifPlugin::default()));
    registry.register(Arc::new(vw_tiff::TiffPlugin));
    registry.register(Arc::new(vw_raw::RawPlugin));
    registry.register(Arc::new(vw_dicom::DicomPlugin));
    registry
}

/// タイトルバーとタスクバーのアイコン。指定しないと eframe の既定（egui のロゴ）になる。
fn window_icon() -> eframe::egui::IconData {
    const SIDE: u32 = 64;
    eframe::egui::IconData {
        rgba: include_bytes!("../../../packaging/icons/vw-64.rgba").to_vec(),
        width: SIDE,
        height: SIDE,
    }
}

fn main() -> eframe::Result {
    let started = Instant::now();
    let initial: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        // 相対パスのままだとツリーのルート名が空になり、前後移動の基準も曖昧になるので絶対パスにする。
        // canonicalize にしないのは、Windows で \\?\ 付きの扱いにくいパスになるため。
        .map(|p| std::path::absolute(&p).unwrap_or(p))
        .collect();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("vw")
            .with_inner_size([1280.0, 800.0])
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "vw",
        options,
        Box::new(move |cc| Ok(Box::new(app::VwApp::new(cc, registry(), initial, started)))),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_icon_is_a_full_64px_square() {
        let icon = super::window_icon();
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }
}
