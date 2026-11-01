//! 比べた結果の表（CSV・タブ区切り）と、画像を中に入れた 1 ファイルの HTML。

use vw_report::html::Page;
use vw_report::{Chart, Color, LegendItem, Panel, Series, Swatch, Table, Theme};

use crate::align::{Alignment, Warped};
use crate::analysis::{Analysis, Rect};
use crate::plane::{Rgb, fit_long_side};
use crate::render::{self, Rgba};

/// HTML に入れる画像の長辺。
const HTML_LONG_SIDE: usize = 1600;

/// 画面の言語の文言。
#[derive(Debug, Clone, Copy)]
pub struct Labels {
    pub title: &'static str,
    pub created: &'static str,
    pub reference: &'static str,
    pub image: &'static str,
    pub measured: &'static str,
    pub matched: &'static str,
    pub not_matched: &'static str,
    pub grid: &'static str,
    pub weak: &'static str,
    pub aligned: &'static str,
    pub difference: &'static str,
    pub ssim_map: &'static str,
    pub histogram: &'static str,
    pub hotspots: &'static str,
    pub max: &'static str,
    pub no_overlap: &'static str,
    pub item: &'static str,
    pub value: &'static str,
    pub yes: &'static str,
    pub no: &'static str,
    pub rows: [&'static str; ROWS],
    /// ヒストグラムの横軸と縦軸。
    pub brightness: &'static str,
    pub pixels: &'static str,
    /// 違いの色の凡例（0 から最大まで）。
    pub difference_scale: &'static str,
    pub words: vw_report::Words,
}

/// 表の行の数（[`row_keys`] と同じ並び）。
pub const ROWS: usize = 27;

impl Labels {
    pub const ENGLISH: Labels = Labels {
        title: "Image comparison",
        created: "Created",
        reference: "Reference",
        image: "Image",
        measured: "Values are measured on the displayed colours (after the pixel pipeline, sRGB 8 bit), inside the rectangle where the images overlap.",
        matched: "Brightness of each image was matched to the reference before comparing (offsets, vignetting and histograms are before matching).",
        not_matched: "Brightness was not matched before comparing.",
        grid: "Analysed at",
        weak: "The alignment may be wrong (low correlation or small overlap).",
        aligned: "Aligned to the reference",
        difference: "Difference",
        ssim_map: "SSIM map (white = least similar)",
        histogram: "Brightness histogram",
        hotspots: "Largest differences (reference pixels)",
        max: "max",
        no_overlap: "The images do not overlap.",
        item: "Item",
        value: "Value",
        yes: "yes",
        no: "no",
        rows: [
            "Reference",
            "Image",
            "Correlation",
            "Overlap",
            "Shift x (px)",
            "Shift y (px)",
            "Rotation (°)",
            "Scale",
            "Shear (°)",
            "PSNR (dB)",
            "SSIM",
            "ΔE2000 mean",
            "ΔE2000 95%",
            "ΔE2000 max",
            "Offset R mean / median",
            "Offset G mean / median",
            "Offset B mean / median",
            "Sharpness (reference / image)",
            "Sharpness ratio",
            "Noise (reference / image)",
            "Noise ratio",
            "Vignetting (reference / image)",
            "Vignetting ratio",
            "Brightness matched",
            "Measured region (x, y, w, h)",
            "Largest difference",
            "Aligned",
        ],
        brightness: "Brightness (0–255)",
        pixels: "Pixels",
        difference_scale: "Difference: 0 (black) to max (white)",
        words: vw_report::Words::ENGLISH,
    };

    pub const JAPANESE: Labels = Labels {
        title: "画像の比較",
        created: "作った日時",
        reference: "基準",
        image: "画像",
        measured: "値は表示の色（画素の変換を通した後の sRGB 8 bit）で、画像が重なった長方形の中で測った。",
        matched: "比べる前に、各画像の明るさを基準に合わせた（明るさのずれ・周辺の暗さ・ヒストグラムは合わせる前の値）。",
        not_matched: "比べる前に明るさを合わせていない。",
        grid: "解析した大きさ",
        weak: "位置合わせが外れているかもしれない（相関が低いか、重なりが小さい）。",
        aligned: "基準に合わせた画像",
        difference: "差",
        ssim_map: "SSIM の地図（白いほど似ていない）",
        histogram: "明るさのヒストグラム",
        hotspots: "違いの大きい場所（基準の画素）",
        max: "最大",
        no_overlap: "画像が重なっていない。",
        item: "項目",
        value: "値",
        yes: "はい",
        no: "いいえ",
        rows: [
            "基準",
            "画像",
            "相関",
            "重なり",
            "ずれ x（画素）",
            "ずれ y（画素）",
            "回転（度）",
            "倍率",
            "せん断（度）",
            "PSNR（dB）",
            "SSIM",
            "ΔE2000 平均",
            "ΔE2000 95%",
            "ΔE2000 最大",
            "明るさのずれ R 平均 / 中央値",
            "明るさのずれ G 平均 / 中央値",
            "明るさのずれ B 平均 / 中央値",
            "鮮鋭度（基準 / 画像）",
            "鮮鋭度の比",
            "ノイズ（基準 / 画像）",
            "ノイズの比",
            "周辺の暗さ（基準 / 画像）",
            "周辺の暗さの比",
            "明るさを合わせた",
            "測った範囲（x, y, 幅, 高さ）",
            "違いの最大",
            "合っている",
        ],
        brightness: "明るさ（0〜255）",
        pixels: "画素の数",
        difference_scale: "違い: 0（黒）から最大（白）",
        words: vw_report::Words::JAPANESE,
    };
}

/// CSV の列名（言語によらない）。
pub fn row_keys() -> [&'static str; ROWS] {
    [
        "reference",
        "image",
        "correlation",
        "overlap",
        "shift_x_px",
        "shift_y_px",
        "rotation_deg",
        "scale",
        "shear_deg",
        "psnr_db",
        "ssim",
        "delta_e2000_mean",
        "delta_e2000_p95",
        "delta_e2000_max",
        "offset_r_mean_median",
        "offset_g_mean_median",
        "offset_b_mean_median",
        "sharpness_reference_image",
        "sharpness_ratio",
        "noise_reference_image",
        "noise_ratio",
        "vignetting_reference_image",
        "vignetting_ratio",
        "brightness_matched",
        "region_x_y_w_h",
        "difference_max",
        "aligned",
    ]
}

/// 比べた 1 枚。
pub struct Entry<'a> {
    pub name: &'a str,
    /// 元の大きさ。
    pub size: [usize; 2],
    pub alignment: &'a Alignment,
    pub analysis: Option<&'a Analysis>,
    pub warped: Option<&'a Warped>,
}

/// 比べた全体。
pub struct Report<'a> {
    pub reference_name: &'a str,
    /// 基準の元の大きさ。
    pub reference_size: [usize; 2],
    /// 解析した格子の基準の画像。
    pub grid: &'a Rgb,
    pub entries: Vec<Entry<'a>>,
    /// 作った日時（呼ぶ側で書いたもの）。
    pub created: &'a str,
}

impl Report<'_> {
    /// 格子の画素 → 基準の画素の倍率。
    fn to_reference(&self) -> f64 {
        self.reference_size[0] as f64 / self.grid.width as f64
    }

    fn region_text(&self, r: Rect) -> String {
        let k = self.to_reference();
        let s = |v: usize| format!("{}", (v as f64 * k).round());
        format!("{}, {}, {}, {}", s(r.x), s(r.y), s(r.w), s(r.h))
    }

    /// 1 枚ぶんの値（[`row_keys`] の並び）。
    pub fn values(&self, e: &Entry<'_>) -> [String; ROWS] {
        let as_f64 = |s: [usize; 2]| [s[0] as f64, s[1] as f64];
        let p = e
            .alignment
            .transform
            .parts(as_f64(self.reference_size), as_f64(e.size));
        let f = |v: f64, digits: usize| {
            if v.is_infinite() {
                "inf".to_owned()
            } else if v.is_nan() {
                String::new()
            } else {
                format!("{v:.digits$}")
            }
        };
        let a = e.analysis;
        let opt = |v: Option<f64>, d: usize| v.map_or_else(String::new, |v| f(v, d));
        let pair = |p: Option<crate::analysis::Pair>, d: usize| {
            p.map_or_else(String::new, |p| {
                format!("{} / {}", f(p.reference, d), f(p.moving, d))
            })
        };
        let ratio =
            |p: Option<crate::analysis::Pair>| p.map_or_else(String::new, |p| f(p.ratio(), 3));
        let offset = |c: usize| {
            a.map_or_else(String::new, |a| {
                format!("{} / {}", f(a.offset[c].mean, 2), f(a.offset[c].median, 2))
            })
        };
        [
            self.reference_name.to_owned(),
            e.name.to_owned(),
            f(e.alignment.correlation, 4),
            f(e.alignment.overlap, 3),
            f(p.shift[0], 2),
            f(p.shift[1], 2),
            f(p.rotation, 3),
            f(p.scale, 5),
            f(p.shear, 3),
            opt(a.and_then(|a| a.psnr), 2),
            opt(a.and_then(|a| a.ssim), 4),
            opt(a.map(|a| a.delta_e.mean), 2),
            opt(a.map(|a| a.delta_e.p95), 2),
            opt(a.map(|a| a.delta_e.max), 2),
            offset(0),
            offset(1),
            offset(2),
            pair(a.and_then(|a| a.sharpness), 1),
            ratio(a.and_then(|a| a.sharpness)),
            pair(a.and_then(|a| a.noise), 3),
            ratio(a.and_then(|a| a.noise)),
            pair(a.and_then(|a| a.vignetting), 3),
            ratio(a.and_then(|a| a.vignetting)),
            a.map_or_else(String::new, |a| a.matched.to_string()),
            a.map_or_else(String::new, |a| self.region_text(a.region)),
            opt(a.map(|a| f64::from(a.difference_max)), 1),
            e.alignment.looks_right().to_string(),
        ]
    }

    /// 人が読む表の値。CSV と違い、真偽を言葉にする。
    pub fn shown(&self, labels: &Labels, e: &Entry<'_>) -> [String; ROWS] {
        self.values(e).map(|v| match v.as_str() {
            "true" => labels.yes.to_owned(),
            "false" => labels.no.to_owned(),
            _ => v,
        })
    }

    /// 1 行目が列名の CSV。
    pub fn csv(&self) -> String {
        let quote = |s: &str| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_owned()
            }
        };
        let mut out = row_keys().join(",");
        out.push('\n');
        for e in &self.entries {
            let row: Vec<String> = self.values(e).iter().map(|v| quote(v)).collect();
            out.push_str(&row.join(","));
            out.push('\n');
        }
        out
    }

    /// 表計算ソフトに貼れるタブ区切り。1 列目が項目、2 列目からが比べた画像。
    pub fn tsv(&self, labels: &Labels) -> String {
        let columns: Vec<[String; ROWS]> =
            self.entries.iter().map(|e| self.shown(labels, e)).collect();
        let mut out = String::new();
        for (i, label) in labels.rows.iter().enumerate() {
            out.push_str(label);
            for c in &columns {
                out.push('\t');
                out.push_str(&c[i].replace(['\t', '\n'], " "));
            }
            out.push('\n');
        }
        out
    }

    /// 画像を中に入れた 1 ファイルの HTML。見た目は `theme`。
    pub fn html(&self, labels: &Labels, theme: Theme) -> String {
        let mut page = Page::new(
            theme,
            labels.words,
            labels.title,
            &[
                format!("{}: {}", labels.reference, self.reference_name),
                format!("{} × {}", self.reference_size[0], self.reference_size[1]),
                format!(
                    "{}: {} × {}",
                    labels.grid, self.grid.width, self.grid.height
                ),
                format!("{}: {}", labels.created, self.created),
            ],
        );
        page.paragraph(labels.measured);
        let matched = self
            .entries
            .iter()
            .any(|e| e.analysis.is_some_and(|a| a.matched));
        page.paragraph(if matched {
            labels.matched
        } else {
            labels.not_matched
        });
        let (sw, sh, _) = fit_long_side(self.grid.width, self.grid.height, HTML_LONG_SIDE);
        let small = |img: &Rgba| shrink(img, sw, sh);
        let reference = render::masked(
            self.grid,
            &vec![true; self.grid.width * self.grid.height],
            [0; 3],
        );
        page.image(&small(&reference), &[], labels.reference);
        for e in &self.entries {
            page.heading(e.name);
            if !e.alignment.looks_right() {
                page.warning(labels.weak);
            }
            page.table(
                &Table {
                    head: vec![labels.item.to_owned(), labels.value.to_owned()],
                    rows: labels
                        .rows
                        .iter()
                        .zip(self.shown(labels, e))
                        .skip(2)
                        .map(|(l, v)| vec![(*l).to_owned(), v])
                        .collect(),
                    numeric_from: 1,
                },
                Some(e.name),
            );
            let (Some(a), Some(w)) = (e.analysis, e.warped) else {
                page.warning(labels.no_overlap);
                continue;
            };
            let aligned = render::masked(&w.image, &w.valid, [0; 3]);
            let mut diff = render::difference(a, self.grid.width, self.grid.height);
            outline(
                &mut diff,
                &a.hotspots.iter().map(|s| s.rect).collect::<Vec<_>>(),
            );
            let aligned = small(&aligned);
            let diff = small(&diff);
            let ssim = render::ssim(a);
            let mut row: Vec<(&Rgba, String)> = vec![
                (&aligned, labels.aligned.to_owned()),
                (
                    &diff,
                    format!(
                        "{} ({} {:.1})",
                        labels.difference, labels.max, a.difference_max
                    ),
                ),
            ];
            if let Some(map) = &ssim {
                row.push((map, labels.ssim_map.to_owned()));
            }
            page.image_row(&row);
            page.raw(&vw_report::legend_svg(
                theme,
                &[LegendItem {
                    label: labels.difference_scale.to_owned(),
                    swatch: Swatch::Ramp(heat_ramp()),
                }],
                720.0,
            ));
            page.subheading(labels.hotspots);
            page.ordered_list(
                &a.hotspots
                    .iter()
                    .map(|s| format!("{} ({:.1})", self.region_text(s.rect), s.mean))
                    .collect::<Vec<_>>(),
            );
            page.chart(
                &histogram_chart(&a.histograms, labels, e.name, theme),
                labels.histogram,
            );
        }
        page.finish()
    }
}

/// 違いの色（[`render::heat`]）の帯。
pub fn heat_ramp() -> Vec<Color> {
    (0..12)
        .map(|i| {
            let [r, g, b] = render::heat(i as f32 / 11.0);
            Color([r, g, b, 255])
        })
        .collect()
}

/// 基準と比べる画像の明るさのヒストグラム。
fn histogram_chart(h: &[Vec<u32>; 2], labels: &Labels, image: &str, theme: Theme) -> Chart {
    let series = |i: usize, name: &str| Series {
        name: name.to_owned(),
        points: h[i]
            .iter()
            .enumerate()
            .map(|(x, &c)| [x as f64, f64::from(c)])
            .collect(),
        style: theme.line(i),
    };
    Chart {
        x_label: labels.brightness.to_owned(),
        x_names: Vec::new(),
        panels: vec![Panel {
            y_label: labels.pixels.to_owned(),
            series: vec![series(0, labels.reference), series(1, image)],
        }],
    }
}

fn shrink(img: &Rgba, w: usize, h: usize) -> Rgba {
    if (img.width, img.height) == (w, h) {
        return img.clone();
    }
    let rgb = Rgb {
        width: img.width,
        height: img.height,
        data: img
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
            .collect(),
    };
    Rgba {
        width: w,
        height: h,
        data: rgb.resized(w, h).to_rgba8(),
    }
}

/// 違いの大きい場所の枠を水色で描く。
fn outline(img: &mut Rgba, rects: &[Rect]) {
    let mut put = |x: usize, y: usize| {
        if x < img.width && y < img.height {
            let i = (y * img.width + x) * 4;
            img.data[i..i + 4].copy_from_slice(&[0, 220, 255, 255]);
        }
    };
    for r in rects {
        for t in 0..2 {
            for x in r.x..r.x + r.w {
                put(x, r.y + t);
                put(x, (r.y + r.h).saturating_sub(1 + t));
            }
            for y in r.y..r.y + r.h {
                put(r.x + t, y);
                put((r.x + r.w).saturating_sub(1 + t), y);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::affine::Affine;
    use crate::analysis::{Options, analyse};
    use crate::test_util::color_texture;

    fn sample() -> (Rgb, Alignment, Warped, Analysis) {
        let grid = color_texture(64, 48);
        let mut moved = grid.clone();
        moved.data[10] = [255.0, 0.0, 0.0];
        let warped = Warped {
            image: moved,
            valid: vec![true; 64 * 48],
        };
        let alignment = Alignment {
            transform: Affine::IDENTITY,
            correlation: 0.99,
            overlap: 1.0,
        };
        let a = analyse(&grid, &warped, Options::default()).unwrap();
        (grid, alignment, warped, a)
    }

    #[test]
    fn the_csv_has_one_row_per_image_under_the_column_names() {
        let (grid, alignment, warped, a) = sample();
        let report = Report {
            reference_name: "a, raw.NEF",
            reference_size: [128, 96],
            grid: &grid,
            entries: vec![Entry {
                name: "b.JPG",
                size: [128, 96],
                alignment: &alignment,
                analysis: Some(&a),
                warped: Some(&warped),
            }],
            created: "2026-10-26 12:00",
        };
        let csv = report.csv();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("reference,image,correlation"));
        assert!(lines[1].starts_with("\"a, raw.NEF\",b.JPG,0.9900"));
        // 測った範囲は基準の画素（格子の 2 倍）で書く。
        assert!(lines[1].contains("\"0, 0, 128, 96\""), "{}", lines[1]);
        let tsv = report.tsv(&Labels::JAPANESE);
        assert!(tsv.lines().any(|l| l.starts_with("相関\t0.9900")));
        let html = report.html(&Labels::ENGLISH, Theme::Lab);
        assert!(html.contains("<h2>b.JPG</h2>"));
        assert!(html.matches("data:image/png;base64,").count() >= 4);
    }

    #[test]
    fn a_weak_alignment_is_flagged_in_the_html() {
        let (grid, mut alignment, warped, a) = sample();
        alignment.correlation = 0.1;
        let report = Report {
            reference_name: "a",
            reference_size: [64, 48],
            grid: &grid,
            entries: vec![Entry {
                name: "<b>",
                size: [64, 48],
                alignment: &alignment,
                analysis: Some(&a),
                warped: Some(&warped),
            }],
            created: "",
        };
        let html = report.html(&Labels::ENGLISH, Theme::Lab);
        assert!(html.contains(Labels::ENGLISH.weak));
        assert!(html.contains("<h2>&lt;b&gt;</h2>"));
    }
}
