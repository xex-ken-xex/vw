//! レポートの HTML（画像も中に入れた 1 ファイル）。見た目はテーマの CSS で揃える。
//!
//! 色はテーマで決めた固定の色にし、開く側の明暗の設定（`prefers-color-scheme`）には合わせない。
//! 書き出したものは「選んだテーマの結果」で、開く環境で見た目が変わると、貼った資料と揃わないため。

use std::fmt::Write as _;

use crate::canvas::esc;
use crate::sheet::{Chart, LegendItem, Table, caption};
use crate::theme::{Face, Theme};
use crate::{Rgba, Words, data_url};

/// 図の SVG の幅（論理の単位）。本文の幅に合わせて縮む。
const FIGURE_WIDTH: f32 = 720.0;

pub struct Page {
    theme: Theme,
    words: Words,
    body: String,
    title: String,
    figures: usize,
    tables: usize,
}

impl Page {
    /// 題と、元の情報（ファイル名・大きさ・日時など）の行。
    pub fn new(theme: Theme, words: Words, title: &str, meta: &[String]) -> Self {
        let mut body = format!("<header><h1>{}</h1>", esc(title));
        if !meta.is_empty() {
            let _ = write!(
                body,
                "<p class=\"meta\">{}</p>",
                esc(&meta.join(words.separator))
            );
        }
        body.push_str("</header>\n");
        Self {
            theme,
            words,
            body,
            title: title.to_owned(),
            figures: 0,
            tables: 0,
        }
    }

    pub fn heading(&mut self, text: &str) {
        let _ = writeln!(self.body, "<h2>{}</h2>", esc(text));
    }

    pub fn subheading(&mut self, text: &str) {
        let _ = writeln!(self.body, "<h3>{}</h3>", esc(text));
    }

    pub fn paragraph(&mut self, text: &str) {
        let _ = writeln!(self.body, "<p>{}</p>", esc(text));
    }

    pub fn warning(&mut self, text: &str) {
        let _ = writeln!(self.body, "<p class=\"warn\">{}</p>", esc(text));
    }

    /// 項目と値の組（作った日時など）。
    pub fn facts(&mut self, rows: &[(String, String)]) {
        self.body.push_str("<dl class=\"facts\">");
        for (k, v) in rows {
            let _ = write!(self.body, "<dt>{}</dt><dd>{}</dd>", esc(k), esc(v));
        }
        self.body.push_str("</dl>\n");
    }

    fn next_caption(&mut self, text: &str) -> String {
        self.figures += 1;
        caption(self.theme, &self.words, self.figures, text)
    }

    pub fn chart(&mut self, chart: &Chart, text: &str) {
        let svg = chart.svg(self.theme, FIGURE_WIDTH, text);
        let caption = self.next_caption(text);
        let _ = writeln!(
            self.body,
            "<figure class=\"chart\">{svg}<figcaption>{}</figcaption></figure>",
            esc(&caption)
        );
    }

    pub fn image(&mut self, image: &Rgba, legend: &[LegendItem], text: &str) {
        let caption = self.next_caption(text);
        let legend = if legend.is_empty() {
            String::new()
        } else {
            crate::legend_svg(self.theme, legend, FIGURE_WIDTH)
        };
        let _ = writeln!(
            self.body,
            "<figure><img src=\"{}\" alt=\"{}\">{legend}<figcaption>{}</figcaption></figure>",
            data_url(image),
            esc(text),
            esc(&caption)
        );
    }

    /// 横に並べる図（比べた画像など）。
    pub fn image_row(&mut self, images: &[(&Rgba, String)]) {
        self.body.push_str("<div class=\"row\">");
        for (image, text) in images {
            let caption = self.next_caption(text);
            let _ = write!(
                self.body,
                "<figure><img src=\"{}\" alt=\"{}\"><figcaption>{}</figcaption></figure>",
                data_url(image),
                esc(text),
                esc(&caption)
            );
        }
        self.body.push_str("</div>\n");
    }

    /// 数値の表。`text` があれば表の番号を付けた説明を上に出す。
    pub fn table(&mut self, table: &Table, text: Option<&str>) {
        self.body.push_str("<div class=\"tbl\"><table>");
        if let Some(text) = text {
            self.tables += 1;
            let label = if self.theme.numbers_figures() {
                format!(
                    "{} {}{}{text}",
                    self.words.table, self.tables, self.words.number_gap
                )
            } else {
                text.to_owned()
            };
            let _ = write!(self.body, "<caption>{}</caption>", esc(&label));
        }
        let class = |c: usize| {
            if c >= table.numeric_from && c > 0 {
                " class=\"num\""
            } else {
                ""
            }
        };
        if !table.head.is_empty() {
            self.body.push_str("<thead><tr>");
            for (c, h) in table.head.iter().enumerate() {
                let _ = write!(self.body, "<th{}>{}</th>", class(c), esc(h));
            }
            self.body.push_str("</tr></thead>");
        }
        self.body.push_str("<tbody>");
        for row in &table.rows {
            self.body.push_str("<tr>");
            for (c, v) in row.iter().enumerate() {
                let _ = write!(self.body, "<td{}>{}</td>", class(c), esc(v));
            }
            self.body.push_str("</tr>");
        }
        self.body.push_str("</tbody></table></div>\n");
    }

    pub fn ordered_list(&mut self, items: &[String]) {
        self.body.push_str("<ol>");
        for i in items {
            let _ = write!(self.body, "<li>{}</li>", esc(i));
        }
        self.body.push_str("</ol>\n");
    }

    /// 自分で組み立てた SVG などをそのまま入れる。
    pub fn raw(&mut self, html: &str) {
        self.body.push_str(html);
        self.body.push('\n');
    }

    pub fn finish(self) -> String {
        format!(
            "<!doctype html>\n<html lang=\"{}\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta name=\"generator\" content=\"vw\"><title>{}</title><style>{}</style></head>\
<body class=\"theme-{}\"><main>\n{}</main></body></html>\n",
            self.words.lang,
            esc(&self.title),
            css(self.theme),
            self.theme.key(),
            self.body
        )
    }
}

/// テーマの CSS。
pub fn css(t: Theme) -> String {
    let mut s = format!(
        "body{{margin:0;background:{bg};color:{ink};font-family:{body};line-height:1.6}}\
main{{max-width:960px;margin:0 auto;padding:28px 24px 48px}}\
h1{{font-size:1.45rem;margin:0 0 4px;color:{head};font-weight:{h1w}}}\
h2{{font-size:1.15rem;margin:28px 0 8px;color:{head};font-weight:{h2w}}}\
h3{{font-size:1rem;margin:18px 0 6px;color:{head}}}\
.meta{{margin:0;color:{muted};font-size:.82rem{meta_font}}}\
p{{margin:8px 0}}.warn{{color:{warn}}}\
figure{{margin:16px 0}}figure svg,figure img{{max-width:100%;height:auto;display:block}}\
figure img{{border:1px solid {rule}}}\
figcaption{{color:{muted};font-size:.85rem;margin-top:6px}}\
.row{{display:flex;flex-wrap:wrap;gap:16px}}.row figure{{flex:1 1 280px;min-width:0}}\
.tbl{{overflow-x:auto;margin:12px 0}}\
table{{border-collapse:collapse;font-size:.85rem;min-width:50%}}\
caption{{text-align:left;color:{muted};font-size:.85rem;padding-bottom:4px}}\
th,td{{padding:3px 10px;text-align:left;vertical-align:top}}\
.num{{text-align:right;font-variant-numeric:tabular-nums{num_font}}}\
dl.facts{{display:grid;grid-template-columns:max-content 1fr;gap:2px 16px;font-size:.85rem;margin:8px 0}}\
dl.facts dt{{color:{muted}}}dl.facts dd{{margin:0}}\
ol{{padding-left:1.4em}}\
@media print{{main{{max-width:none;padding:0}}}}",
        bg = t.background().css(),
        ink = t.ink().css(),
        head = t.heading().css(),
        muted = t.muted().css(),
        rule = t.rule().css(),
        body = t.font_family(Face::Body),
        h1w = if t == Theme::Dark { 500 } else { 600 },
        h2w = if t == Theme::Lab { 700 } else { 600 },
        warn = if t == Theme::Dark {
            "#ff8a80"
        } else if t == Theme::Mono {
            "#000;font-weight:700"
        } else {
            "#b0342b"
        },
        meta_font = if t == Theme::Lab {
            format!(";font-family:{}", t.font_family(Face::Mono))
        } else {
            String::new()
        },
        num_font = if t.mono_numbers() {
            format!(";font-family:{}", t.font_family(Face::Mono))
        } else {
            String::new()
        },
    );
    s.push_str(&match t {
        Theme::Paper => format!(
            "table{{border-top:1.5px solid {ink};border-bottom:1.5px solid {ink}}}\
thead th{{border-bottom:.75px solid {ink};font-weight:600}}",
            ink = t.ink().css()
        ),
        Theme::Lab => format!(
            "th{{background:{};color:{};font-weight:500}}td{{border-bottom:1px solid {}}}",
            t.table_head().map(|c| c.css()).unwrap_or_default(),
            t.heading().css(),
            t.rule().css()
        ),
        Theme::Dark => format!(
            "th{{color:{};font-weight:500;border-bottom:1px solid {}}}td{{border-bottom:1px solid #2c2f33}}",
            t.muted().css(),
            t.rule().css()
        ),
        Theme::Mono => {
            "th{font-weight:700;border-bottom:1px solid #000}td{border-bottom:.5px dotted #888}"
                .to_owned()
        }
    });
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Panel, Series};

    fn table() -> Table {
        Table {
            head: vec!["Line".into(), "Mean".into()],
            rows: vec![vec!["1 <a>".into(), "12.5".into()]],
            numeric_from: 1,
        }
    }

    #[test]
    fn a_page_has_title_meta_and_the_theme_style() {
        let p = Page::new(
            Theme::Paper,
            Words::JAPANESE,
            "計測線",
            &["a.png".into(), "64 × 64".into()],
        );
        let h = p.finish();
        assert!(h.contains("<html lang=\"ja\">"));
        assert!(h.contains("<h1>計測線</h1>"));
        assert!(h.contains("a.png ・ 64 × 64"));
        assert!(h.contains("Yu Mincho"));
        assert!(h.contains("body class=\"theme-paper\""));
    }

    #[test]
    fn the_paper_theme_numbers_figures_and_tables() {
        let mut p = Page::new(Theme::Paper, Words::ENGLISH, "t", &[]);
        let chart = Chart {
            x_label: "x".into(),
            x_names: Vec::new(),
            panels: vec![Panel {
                y_label: "y".into(),
                series: vec![Series {
                    name: "a".into(),
                    points: vec![[0.0, 0.0], [1.0, 2.0]],
                    style: Theme::Paper.line(0),
                }],
            }],
        };
        p.chart(&chart, "Values along the line.");
        p.table(&table(), Some("Statistics."));
        let h = p.finish();
        assert!(h.contains("<figcaption>Figure 1. Values along the line.</figcaption>"));
        assert!(h.contains("<caption>Table 1. Statistics.</caption>"));
        assert!(h.contains("<svg"));
    }

    #[test]
    fn other_themes_leave_captions_unnumbered() {
        let mut p = Page::new(Theme::Lab, Words::ENGLISH, "t", &[]);
        p.table(&table(), Some("Statistics."));
        assert!(p.finish().contains("<caption>Statistics.</caption>"));
    }

    #[test]
    fn table_cells_are_escaped_and_numbers_aligned_right() {
        let mut p = Page::new(Theme::Mono, Words::ENGLISH, "t", &[]);
        p.table(&table(), None);
        let h = p.finish();
        assert!(h.contains("<td>1 &lt;a&gt;</td><td class=\"num\">12.5</td>"));
    }

    #[test]
    fn every_theme_has_fixed_colors_without_following_the_os() {
        for t in Theme::ALL {
            let c = css(t);
            assert!(!c.contains("prefers-color-scheme"), "{t:?}");
            assert!(c.contains(&t.background().css()), "{t:?}");
        }
    }
}
