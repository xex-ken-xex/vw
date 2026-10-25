//! 同梱のヘルプページ。ブラウザで開く。
//!
//! ページは実行ファイルに埋め込み、開くときに設定のフォルダの `help/` へ書き出す。
//! 配布物に HTML を別に入れないのは、実行ファイルだけを置き換えて版を上げたときに、
//! 古い版のページが残って説明と動きが食い違わないようにするため。

use std::path::{Path, PathBuf};

use crate::settings::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// 画質の評価（PSNR・SSIM・SNR・CNR・鮮鋭度）。
    Quality,
}

impl Page {
    fn file_name(self, lang: Language) -> &'static str {
        match (self, lang) {
            (Page::Quality, Language::English) => "quality.en.html",
            (Page::Quality, Language::Japanese) => "quality.ja.html",
        }
    }

    fn html(self, lang: Language) -> &'static str {
        match (self, lang) {
            (Page::Quality, Language::English) => include_str!("../help/quality.en.html"),
            (Page::Quality, Language::Japanese) => include_str!("../help/quality.ja.html"),
        }
    }
}

/// `page` を `dir` に書き出し、そのパスを返す。中身が同じなら書き直さない。
pub fn write_page(dir: &Path, page: Page, lang: Language) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(page.file_name(lang));
    let html = page.html(lang);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(html) {
        std::fs::write(&path, html)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn writes_the_page_in_the_chosen_language() {
        let tmp = TempDir::new();
        let ja = write_page(tmp.path(), Page::Quality, Language::Japanese).unwrap();
        let en = write_page(tmp.path(), Page::Quality, Language::English).unwrap();
        assert!(std::fs::read_to_string(ja).unwrap().contains("画質の評価"));
        assert!(std::fs::read_to_string(en)
            .unwrap()
            .contains("Image quality metrics"));
    }

    #[test]
    fn replaces_a_page_left_by_another_version() {
        let tmp = TempDir::new();
        let path = tmp.path().join("quality.en.html");
        std::fs::write(&path, "old").unwrap();
        write_page(tmp.path(), Page::Quality, Language::English).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            Page::Quality.html(Language::English)
        );
    }

    #[test]
    fn both_languages_explain_every_metric_and_its_default_thresholds() {
        for lang in [Language::English, Language::Japanese] {
            let html = Page::Quality.html(lang);
            for m in vw_quality::Metric::ALL {
                assert!(
                    html.contains(&format!("<code>quality.{}</code>", m.key())),
                    "{lang:?} lacks {}",
                    m.key()
                );
                let t = m.default_thresholds();
                for bound in [t.good, t.caution] {
                    let text = format!("{bound}");
                    assert!(html.contains(&text), "{lang:?} lacks {} {text}", m.key());
                }
            }
        }
    }
}
