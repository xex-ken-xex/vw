//! 日本語表示用のフォント読み込み。
//!
//! egui の組み込みフォントには日本語の字形がないので、OS にある日本語フォントを
//! 代替フォントとして足す。フォントを実行ファイルに埋め込まないのは、日本語フォントは
//! 数 MB〜数十 MB あり、実行ファイルが大きくなって起動も遅くなるため。
//! 読み込みはバックグラウンドで行い、最初の画面表示を待たせない。

use std::path::Path;

use eframe::egui::{self, FontData, FontFamily};
use eframe::epaint::text::{FontInsert, FontPriority, InsertFontFamily};

/// 探すフォントファイル。先に見つかったものを使う。
const CANDIDATES: &[&str] = &[
    // Windows
    "C:\\Windows\\Fonts\\YuGothM.ttc",
    "C:\\Windows\\Fonts\\meiryo.ttc",
    "C:\\Windows\\Fonts\\msgothic.ttc",
    // Linux
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/ipaexfont-gothic/ipaexg.ttf",
    "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
    "/usr/share/fonts/truetype/vlgothic/VL-Gothic-Regular.ttf",
    "/usr/share/fonts/truetype/takao-gothic/TakaoPGothic.ttf",
];

/// 見つかった日本語フォントを、優先度最低の代替フォントとして追加する。
/// 見つからなければ何もしない（日本語は □ で表示される）。
pub fn load_cjk_in_background(ctx: &egui::Context) {
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let Some((path, bytes)) = CANDIDATES
            .iter()
            .find_map(|p| std::fs::read(Path::new(p)).ok().map(|b| (*p, b)))
        else {
            return;
        };
        let families = [FontFamily::Proportional, FontFamily::Monospace]
            .into_iter()
            .map(|family| InsertFontFamily {
                family,
                priority: FontPriority::Lowest,
            })
            .collect();
        ctx.add_font(FontInsert {
            name: path.to_owned(),
            data: FontData::from_owned(bytes),
            families,
        });
        ctx.request_repaint();
    });
}
