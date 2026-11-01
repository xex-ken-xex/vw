//! 4 つのテーマで同じ図を描いて PNG と HTML に書く（見た目の確認用）。
//! `cargo run -p vw-report --example themes -- <出力先> <本文の字体> [等幅の字体]`
use std::sync::Arc;
use vw_report::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(&args[1]);
    let font = |p: &str| (Arc::new(std::fs::read(p).expect("font")), 0);
    let fonts = Fonts::new(
        vec![font(&args[2])],
        args.get(3).map(|p| vec![font(p)]).unwrap_or_default(),
    );
    for theme in Theme::ALL {
        let series: Vec<Series> = (0..3)
            .map(|i| Series {
                name: format!("計測線 {}", i + 1),
                points: (0..240)
                    .map(|x| {
                        let x = f64::from(x);
                        let c = 90.0 + 30.0 * i as f64;
                        [
                            x,
                            20000.0
                                + 15000.0 * (-(x - c).powi(2) / 600.0).exp()
                                + 1500.0 * (x / (9.0 + i as f64)).sin(),
                        ]
                    })
                    .collect(),
                style: theme.line(i),
            })
            .collect();
        let sheet = Sheet {
            title: "計測線に沿った画素の値".into(),
            meta: vec![
                "sample-0042.tif".into(),
                "2048 × 1536".into(),
                "16bit グレー".into(),
                "2026-11-01 19:52".into(),
            ],
            blocks: vec![
                Block::Chart(Chart {
                    x_label: "始点からの距離 (px)".into(),
                    x_names: Vec::new(),
                    panels: vec![
                        Panel {
                            y_label: "画素の値".into(),
                            series: series.clone(),
                        },
                        Panel {
                            y_label: "計測の値 (HU)".into(),
                            series: series
                                .iter()
                                .map(|s| Series {
                                    points: s
                                        .points
                                        .iter()
                                        .map(|p| [p[0], p[1] - 32768.0])
                                        .collect(),
                                    ..s.clone()
                                })
                                .collect(),
                        },
                    ],
                }),
                Block::Caption(
                    "3 本の計測線の始点から終点までの値。値は表示用に変換する前の元の値。".into(),
                ),
                Block::Table(Table {
                    head: vec![
                        "線".into(),
                        "最小".into(),
                        "最大".into(),
                        "平均".into(),
                        "標準偏差".into(),
                    ],
                    rows: (1..=3)
                        .map(|i| {
                            vec![
                                format!("計測線 {i}"),
                                "18,512".into(),
                                "36,770".into(),
                                "23,104.5".into(),
                                "4,210.2".into(),
                            ]
                        })
                        .collect(),
                    numeric_from: 1,
                }),
            ],
        };
        let words = Words::JAPANESE;
        let img = sheet.image(theme, &words, &fonts);
        std::fs::write(out.join(format!("{}.png", theme.key())), png(&img)).unwrap();
        let mut page = html::Page::new(theme, words, &sheet.title, &sheet.meta);
        if let Block::Chart(c) = &sheet.blocks[0] {
            page.chart(c, "3 本の計測線の始点から終点までの値。");
        }
        if let Block::Table(t) = &sheet.blocks[2] {
            page.table(t, Some("線ごとの値"));
        }
        std::fs::write(out.join(format!("{}.html", theme.key())), page.finish()).unwrap();
    }
}
