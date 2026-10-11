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
use std::sync::Arc;

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, FontTweak};
use eframe::epaint::text::{FontInsert, FontPriority, Fonts, InsertFontFamily, TextOptions};

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
        // 0 番の字形を使う。.ttc の先頭が日本語版になっているものだけを候補にしてある。
        let data = FontData::from_owned(bytes);
        let tweak = FontTweak {
            y_offset_factor: baseline_shift(&data, 'あ'),
            ..Default::default()
        };
        ctx.add_font(FontInsert {
            name: path.to_owned(),
            data: data.tweak(tweak),
            families,
        });
        ctx.request_repaint();
    });
}

/// 代替フォント `data` の文字（`sample` で測る）を本文フォントの文字と同じ基準線に
/// 載せるために、下へずらす量（文字の大きさに対する割合）。
///
/// egui は代替フォントの文字を、基準線ではなく行の高さの中央でそろえる。日本語フォントは
/// 本文フォントと上下の余白の比が違うので、そのままだと日本語だけ上か下にずれる。
/// 等幅フォント（Hack）向けに別に測らないのは、Hack と本文フォントの基準線の差が
/// 文字の大きさの 1% ほどで、見て分からないため（1 つのフォントにずらす量は 1 つしか持てない）。
/// ずらす量を固定値にしないのは、行の高さの比がフォントごとに違い（游ゴシック・メイリオ・
/// Noto Sans CJK など）、どの候補が見つかるかが環境で変わるため。egui 自身に並べさせて
/// 測るのは、egui の配置の計算（丸めを含む）を手で写すと、egui の更新で食い違うため。
fn baseline_shift(data: &FontData, sample: char) -> f32 {
    const SIZE: f32 = 100.0;
    let mut definitions = FontDefinitions::default();
    let body = definitions.families[&FontFamily::Proportional][0].clone();
    definitions
        .font_data
        .insert("fallback".into(), Arc::new(data.clone()));
    definitions
        .families
        .insert(FontFamily::Proportional, vec![body, "fallback".into()]);
    let mut fonts = Fonts::new(TextOptions::default(), definitions);
    let galley = fonts.with_pixels_per_point(1.0).layout_no_wrap(
        format!("x{sample}"),
        FontId::proportional(SIZE),
        Color32::WHITE,
    );
    let [latin, fallback] = galley.rows[0].glyphs[..] else {
        return 0.0;
    };
    (latin.pos.y - fallback.pos.y) / SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` を `definitions` で並べた 1 行目の文字。
    fn glyphs(definitions: FontDefinitions, text: &str) -> Vec<egui::epaint::text::Glyph> {
        let mut fonts = Fonts::new(TextOptions::default(), definitions);
        let galley = fonts.with_pixels_per_point(1.0).layout_no_wrap(
            text.into(),
            FontId::proportional(100.0),
            Color32::WHITE,
        );
        galley.rows[0].glyphs.clone()
    }

    #[test]
    fn fallback_glyph_sits_on_the_body_font_baseline() {
        // テスト環境に日本語フォントがあるとは限らないので、組み込みの絵文字フォントを
        // 本文フォント（Ubuntu）にない文字の代替フォントにして確かめる。
        // 絵文字フォントは Ubuntu と行の高さが違い、日本語フォントと同じくずれが出る。
        let sample = '😀';
        let emoji = FontDefinitions::default().font_data["NotoEmoji-Regular"]
            .as_ref()
            .clone()
            .tweak(FontTweak::default());
        let shift = baseline_shift(&emoji, sample);
        assert_ne!(shift, 0.0);

        // 絵文字フォントだけで並べたときの、基準線から字形の上端までの距離。
        let mut alone = FontDefinitions::default();
        alone
            .families
            .insert(FontFamily::Proportional, vec!["NotoEmoji-Regular".into()]);
        alone
            .font_data
            .insert("NotoEmoji-Regular".into(), Arc::new(emoji.clone()));
        let expected = glyphs(alone, &sample.to_string())[0].uv_rect.offset.y;

        let mut fallback = FontDefinitions::default();
        fallback.font_data.insert(
            "fallback".into(),
            Arc::new(emoji.tweak(FontTweak {
                y_offset_factor: shift,
                ..Default::default()
            })),
        );
        fallback.families.insert(
            FontFamily::Proportional,
            vec!["Ubuntu-Light".into(), "fallback".into()],
        );
        let [latin, glyph] = glyphs(fallback, &format!("x{sample}"))[..] else {
            panic!("expected two glyphs");
        };
        let top_from_latin_baseline = glyph.pos.y + glyph.uv_rect.offset.y - latin.pos.y;
        assert!(
            (top_from_latin_baseline - expected).abs() <= 1.0,
            "{top_from_latin_baseline} vs {expected}"
        );
    }

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
