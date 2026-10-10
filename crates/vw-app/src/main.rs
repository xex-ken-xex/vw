//! vw: Linux / Windows 向けの高速な画像ビューワ。
//!
//! 使い方: `vw [フォルダ または 画像ファイル ...]`

// Windows のリリースビルドでは GUI サブシステムにして、起動時に黒いコンソール窓を出さない。
// デバッグビルドで残すのは、panic メッセージなどを端末で読めるようにするため。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autoload;
mod copy;
mod export;
mod fonts;
mod i18n;
mod keymap;
mod labels;
mod layout;
mod loader;
mod log;
mod memory;
mod settings;
mod settings_ui;
mod slideshow;
mod sort;
mod state;
mod theme;
mod toml_reader;
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
    registry
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
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "vw",
        options,
        Box::new(move |cc| Ok(Box::new(app::VwApp::new(cc, registry(), initial, started)))),
    )
}
