//! レポートの図を画像にするときの字体と、図の言葉。
//!
//! 字体は画面と同じく OS のものを読む（[`crate::fonts`] と同じ理由で埋め込まない）。読むのは
//! 図を初めて画像にするときだけで、起動は待たせない。欧文の字体には egui の組み込みのものを
//! 足し、OS に日本語の字体が無い環境でも数字と英語は描けるようにする。

use std::path::Path;
use std::sync::{Arc, OnceLock};

use eframe::egui::FontDefinitions;
use vw_report::{Fonts, Theme, Words};

use crate::settings::Language;

/// 明朝。論文のテーマの本文に使う。無ければゴシックで描く。
const SERIF: &[&str] = &[
    "C:\\Windows\\Fonts\\yumin.ttf",
    "C:\\Windows\\Fonts\\msmincho.ttc",
    "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSerifCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSerifCJK-Regular.ttc",
    "/usr/share/fonts/opentype/ipaexfont-mincho/ipaexm.ttf",
    "/usr/share/fonts/opentype/ipafont-mincho/ipam.ttf",
];

const MONO: &[&str] = &[
    "C:\\Windows\\Fonts\\consola.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
];

type Font = (Arc<Vec<u8>>, u32);

fn first(paths: &[&str]) -> Option<Font> {
    paths
        .iter()
        .find_map(|p| std::fs::read(Path::new(p)).ok())
        .map(|b| (Arc::new(b), 0))
}

fn builtin(name: &str) -> Option<Font> {
    let defs = FontDefinitions::default();
    let data = defs.font_data.get(name)?;
    Some((Arc::new(data.font.to_vec()), data.index))
}

/// テーマに合う字体の並び。
pub fn fonts(theme: Theme) -> &'static Fonts {
    static SANS: OnceLock<Fonts> = OnceLock::new();
    static SERIF_FONTS: OnceLock<Fonts> = OnceLock::new();
    let build = |serif: bool| {
        let mut body: Vec<Font> = Vec::new();
        if serif {
            body.extend(first(SERIF));
        }
        body.extend(first(crate::fonts::CANDIDATES));
        body.extend(builtin("Ubuntu-Light"));
        let mut mono: Vec<Font> = first(MONO).into_iter().collect();
        mono.extend(builtin("Hack"));
        Fonts::new(body, mono)
    };
    if theme == Theme::Paper {
        SERIF_FONTS.get_or_init(|| build(true))
    } else {
        SANS.get_or_init(|| build(false))
    }
}

pub fn words(lang: Language) -> Words {
    match lang {
        Language::English => Words::ENGLISH,
        Language::Japanese => Words::JAPANESE,
    }
}

/// 書き出した日時（地方時）。
pub fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_latin_font_is_always_there() {
        assert!(builtin("Ubuntu-Light").is_some());
        assert!(builtin("Hack").is_some());
        assert!(!fonts(Theme::Lab).is_empty());
    }
}
