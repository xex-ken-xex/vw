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
mod colormap;
mod colormap_ui;
mod compare;
mod compose;
mod copy;
mod dataset_extract;
mod dataset_groups;
mod dataset_search;
mod dataset_ui;
mod datasets_screen;
mod deep;
mod derive;
mod dicom_send;
mod discard;
mod displays;
mod export;
mod face_groups;
mod face_store;
mod faces;
mod fonts;
mod format_stages;
mod frames;
mod fullscreen;
mod geo_map;
mod geonames;
mod help;
mod histogram;
mod histogram_view;
mod i18n;
mod keymap;
mod labels;
mod layout;
mod line_profile;
mod loader;
mod loading;
mod roi_graph;
use vw_log::log;
mod map_tiles;
mod map_window;
mod marks;
mod memory;
mod meta_view;
use vw_pipeline::{expr, nodes};
mod owned_windows;
mod people;
mod people_graph;
mod pipeline_view;
mod pipelines;
mod pixel_pipeline;
mod popup_thumb;
mod probe;
mod qr;
mod quality;
mod quality_view;
mod quantity;
mod report_fonts;
mod roi;
mod send_app;
mod series;
mod settings;
mod settings_ui;
mod sky_chart;
mod sky_window;
mod slideshow;
mod sort;
mod spectrum_view;
mod state;
mod strip_location;
mod subframes_window;
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
/// `fits` と `calibration` は FITS のプラグインと共有する、設定画面で変える表示と校正の決め方。
/// `netcdf` は NetCDF のプラグインと共有する表示の決め方と、ファイルごとに選んだ変数。
fn registry(
    fits: &vw_fits::SharedOptions,
    calibration: &vw_fits::Calibration,
    netcdf: &(vw_netcdf::SharedOptions, vw_netcdf::Choices),
) -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(vw_png::PngPlugin));
    registry.register(Arc::new(vw_jpeg::JpegPlugin));
    registry.register(Arc::new(vw_bmp::BmpPlugin));
    registry.register(Arc::new(vw_webp::WebpPlugin::default()));
    registry.register(Arc::new(vw_gif::GifPlugin::default()));
    registry.register(Arc::new(vw_tiff::TiffPlugin));
    registry.register(Arc::new(vw_fits::FitsPlugin::new(
        fits.clone(),
        calibration.clone(),
    )));
    registry.register(Arc::new(vw_netcdf::NetcdfPlugin::new(
        netcdf.0.clone(),
        netcdf.1.clone(),
    )));
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
            .with_title(about::NAME)
            .with_inner_size([1280.0, 800.0])
            .with_icon(window_icon()),
        ..Default::default()
    };
    let depth = settings::startup_depth(settings::SettingsStore::default_path().as_deref());
    vw_renderer::run_native_with_depth(
        "vw",
        options,
        depth,
        Box::new(move |cc| {
            let fits = vw_fits::SharedOptions::default();
            let calibration = vw_fits::Calibration::default();
            let netcdf = Default::default();
            Ok(Box::new(app::VwApp::new(
                cc,
                registry(&fits, &calibration, &netcdf),
                fits,
                calibration,
                netcdf,
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
