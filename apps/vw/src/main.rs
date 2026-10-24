//! vw: Linux / Windows 向けの高速な画像ビューワ。
//!
//! 使い方: `vw [フォルダ または 画像ファイル ...]`

// Windows のリリースビルドでは GUI サブシステムにして、起動時に黒いコンソール窓を出さない。
// デバッグビルドで残すのは、panic メッセージなどを端末で読めるようにするため。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about;
mod ai_models;
mod ai_tags;
mod app;
mod archive;
mod autoload;
mod background;
mod catalog;
mod color_text;
mod copy;
mod dataset_extract;
mod dataset_search;
mod dataset_ui;
mod datasets_screen;
mod derive;
mod dicom_send;
mod discard;
mod export;
mod expr;
mod face_groups;
mod face_store;
mod faces;
mod fonts;
mod format_stages;
mod frames;
mod fullscreen;
mod geo_map;
mod geonames;
mod histogram;
mod histogram_view;
mod i18n;
mod keymap;
mod labels;
mod layout;
mod loader;
use vw_log::log;
mod map_tiles;
mod map_window;
mod marks;
mod memory;
mod meta_view;
mod nodes;
mod owned_windows;
mod people;
mod people_graph;
mod pipeline_view;
mod pipelines;
mod pixel_pipeline;
mod popup_thumb;
mod probe;
mod quality;
mod quality_view;
mod quantity;
mod roi;
mod series;
mod settings;
mod settings_ui;
mod sky_chart;
mod sky_window;
mod slideshow;
mod sort;
mod state;
use vw_log::status;
mod sub_window;
mod tagger;
mod theme;
mod toml_reader;
mod transfer;
mod tree;
mod viewer;
mod watch;
mod window_placement;
mod zoom_map;

#[cfg(test)]
mod test_util;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use vw_core::Registry;

/// 組み込みのデコーダプラグインを登録する。新しい形式はここに 1 行足す。
/// `fits` は FITS のプラグインと共有する、設定画面で変える表示の決め方。
fn registry(fits: &vw_fits::SharedOptions) -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(vw_png::PngPlugin));
    registry.register(Arc::new(vw_jpeg::JpegPlugin));
    registry.register(Arc::new(vw_bmp::BmpPlugin));
    registry.register(Arc::new(vw_webp::WebpPlugin::default()));
    registry.register(Arc::new(vw_gif::GifPlugin::default()));
    registry.register(Arc::new(vw_tiff::TiffPlugin));
    registry.register(Arc::new(vw_fits::FitsPlugin::new(fits.clone())));
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
    vw_renderer::run_native(
        "vw",
        options,
        Box::new(move |cc| {
            let fits = vw_fits::SharedOptions::default();
            Ok(Box::new(app::VwApp::new(
                cc,
                registry(&fits),
                fits,
                initial,
                started,
            )))
        }),
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
