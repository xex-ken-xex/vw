//! フォントの準備。アイコンフォントと、日本語表示用のフォント。
//!
//! egui の組み込みフォントには日本語の字形がないので、OS にある日本語フォントを
//! 代替フォントとして足す。フォントを実行ファイルに埋め込まないのは、日本語フォントは
//! 数 MB〜数十 MB あり、実行ファイルが大きくなって起動も遅くなるため。
//! 読み込みはバックグラウンドで行い、最初の画面表示を待たせない。
//!
//! 候補は日本語向けのフォントだけにする。漢字は日本語と中国語で同じ文字コードを使うが
//! 字形が異なる（「直」「骨」「写」など）ので、中国語のフォント（Noto Sans CJK SC/TC、
//! Microsoft YaHei、WenQuanYi など）を拾うと日本語が中国風の字形で表示されてしまう。
//! fontconfig に「sans の日本語」を問い合わせる方法も採らない。環境によっては
//! 中国語フォントが返り、しかも問い合わせのコストが起動時にかかるため。

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
    // Noto Sans CJK の .ttc は複数言語の字形を束ねたもので、先頭（0 番）が日本語版（JP）。
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-sans-cjk-fonts/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJKjp-Regular.otf",
    "/usr/share/fonts/noto-cjk/NotoSansCJKjp-Regular.otf",
    "/usr/share/fonts/opentype/ipaexfont-gothic/ipaexg.ttf",
    "/usr/share/fonts/opentype/ipafont-gothic/ipagp.ttf",
    "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
    "/usr/share/fonts/truetype/vlgothic/VL-Gothic-Regular.ttf",
    "/usr/share/fonts/truetype/takao-gothic/TakaoPGothic.ttf",
];

/// ツール列などで使うアイコンフォント（Phosphor）を組み込む。
/// 普通の文字列の中にアイコンを混ぜて書けるよう、本文フォントの次の代替フォントにする。
pub fn install_icons(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}

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
            // 0 番の字形を使う。.ttc の先頭が日本語版になっているものだけを候補にしてある。
            data: FontData::from_owned(bytes),
            families,
        });
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_japanese_fonts_only() {
        let chinese = [
            "SC",
            "TC",
            "HK",
            "CN",
            "TW",
            "YaHei",
            "JhengHei",
            "SimSun",
            "SimHei",
            "MingLiU",
            "wqy",
            "WenQuanYi",
            "Droid",
        ];
        for path in CANDIDATES {
            let name = Path::new(path).file_name().unwrap().to_string_lossy();
            for c in chinese {
                assert!(!name.contains(c), "{path} looks like a Chinese font");
            }
        }
    }
}
