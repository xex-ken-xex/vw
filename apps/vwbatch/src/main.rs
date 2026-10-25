//! vw で保存した画素の変換（パイプライン）を、たくさんの画像にまとめてかけて書き出す。
//!
//! ビューアの exe に `--apply` を足さず別のアプリにしたのは、Windows のビューアは GUI の exe で、
//! コマンドプロンプトに進み具合やエラーを出せないため。
mod convert;
mod jobs;
mod write;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use clap::Parser;
use rayon::prelude::*;
use vw_core::Registry;
use vw_pipeline::preset;

use convert::Recipe;
use write::Format;

#[derive(Parser)]
#[command(
    version,
    about = "vw で保存した画素の変換（パイプライン）を、画像にまとめてかけて書き出す",
    after_help = "\
パイプラインは、vw の画素の変換の窓で「ファイルに書き出す…」をした .vwpipe か、
設定フォルダの pipelines.toml（--name で名前を選ぶ）。
入力のフォルダは中まで探し、出力先にも同じサブフォルダの並びで書く。
出力先に同じ名前のファイルがあれば、上書きせずに飛ばす。
例: vwbatch -p 仕上げ.vwpipe -o 書き出し -f png16 撮影/2026-10-25"
)]
struct Args {
    /// 入力の画像ファイルかフォルダ（いくつでも）
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// パイプラインのファイル（.vwpipe か pipelines.toml）
    #[arg(short, long, value_name = "FILE")]
    pipeline: PathBuf,
    /// pipelines.toml から使うパイプラインの名前
    #[arg(short, long)]
    name: Option<String>,
    /// 出力先のフォルダ
    #[arg(short, long, value_name = "DIR")]
    output: PathBuf,
    /// 書き出す形式
    #[arg(short, long, value_enum, default_value_t = Format::Png)]
    format: Format,
    /// JPEG の品質（1〜100）
    #[arg(short, long, default_value_t = 90, value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,
    /// カメラ RAW を現像せず、埋め込みのプレビューを使う
    #[arg(long)]
    no_develop: bool,
    /// 何枚も入ったファイル（アニメーションなど）は全部のフレームを書く（名前の後ろに -0001 などを付ける）
    #[arg(long)]
    all_frames: bool,
}

/// 組み込みのデコーダプラグイン。ビューアの `registry`（apps/vw/src/main.rs）と同じ並び。
/// FITS の表示と校正は、プラグインの既定（ビューアの設定は読まない）。
fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(vw_png::PngPlugin));
    registry.register(Arc::new(vw_jpeg::JpegPlugin));
    registry.register(Arc::new(vw_bmp::BmpPlugin));
    registry.register(Arc::new(vw_webp::WebpPlugin::default()));
    registry.register(Arc::new(vw_gif::GifPlugin::default()));
    registry.register(Arc::new(vw_tiff::TiffPlugin));
    registry.register(Arc::new(vw_fits::FitsPlugin::default()));
    registry.register(Arc::new(vw_raw::RawPlugin));
    registry.register(Arc::new(vw_dicom::DicomPlugin));
    registry
}

fn load_recipe(args: &Args) -> Result<Recipe, String> {
    let at = |e: String| format!("{}: {e}", args.pipeline.display());
    let text = std::fs::read_to_string(&args.pipeline).map_err(|e| at(e.to_string()))?;
    let is_single = args
        .pipeline
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(preset::FILE_EXTENSION));
    let (values, look) = if is_single {
        preset::from_file(&text).map_err(at)?
    } else {
        let (presets, problems) = preset::from_toml(&text).map_err(at)?;
        for p in problems {
            eprintln!("注意: {}", at(p));
        }
        let names = || {
            presets
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let found = match &args.name {
            Some(n) => presets.iter().find(|p| &p.name == n),
            None if presets.len() == 1 => presets.first(),
            None => return Err(at(format!("--name で選んでください: {}", names()))),
        };
        let p = found.ok_or_else(|| at(format!("その名前はありません。あるのは: {}", names())))?;
        (p.values.clone(), p.look.clone())
    };
    let look = look.unwrap_or_default();
    let palette = look.palette().map_err(at)?;
    Ok(Recipe {
        values,
        look,
        palette,
        develop: !args.no_develop,
        all_frames: args.all_frames,
    })
}

fn main() -> ExitCode {
    let args = Args::parse();
    let recipe = match load_recipe(&args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let registry = registry();
    let jobs = jobs::collect(&registry, &args.inputs, &args.output);
    for e in &jobs.errors {
        eprintln!("失敗: {e}");
    }
    let total = jobs.files.len();
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(jobs.errors.len());
    let skipped = AtomicUsize::new(0);
    let written = AtomicUsize::new(0);
    jobs.files.par_iter().for_each(|job| {
        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
        let pictures = match convert::convert(&registry, &job.source, &recipe) {
            Ok(p) => p,
            Err(e) => {
                failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("[{n}/{total}] 失敗: {}: {e}", job.source.display());
                return;
            }
        };
        for (frame, picture) in pictures {
            let out = job.output(&args.output, frame, args.format.extension());
            let result = write::encode(&picture, args.format, args.quality)
                .and_then(|bytes| write::write_new(&out, &bytes));
            match result {
                Ok(true) => {
                    written.fetch_add(1, Ordering::Relaxed);
                    println!("[{n}/{total}] {}", out.display());
                }
                Ok(false) => {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    eprintln!(
                        "[{n}/{total}] 飛ばした（同じ名前のファイルがある）: {}",
                        out.display()
                    );
                }
                Err(e) => {
                    failed.fetch_add(1, Ordering::Relaxed);
                    eprintln!("[{n}/{total}] 失敗: {}: {e}", job.source.display());
                }
            }
        }
    });
    let failed = failed.into_inner();
    eprintln!(
        "書いた {}、飛ばした {}、失敗 {}",
        written.into_inner(),
        skipped.into_inner(),
        failed
    );
    match failed {
        0 => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}
