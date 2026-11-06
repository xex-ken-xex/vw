//! カラーの形（Photometric Interpretation）ごとの DICOM を作る。
//! `cargo run -p vw-dicom --example make_color_samples -- <出力フォルダ>`
//!
//! どれも 256 x 256 で、左から右へ色相が回り、上から下へ暗くなる帯の中央に白い円がある。
//! `reference-rgb8.dcm` と見比べて、同じ絵に見えれば読めている。PALETTE COLOR は番号で色を
//! 引くので、上下の暗さはない。
//!
//! - `reference-rgb8.dcm`: 8bit の RGB（見比べの元）
//! - `palette-8bit.dcm`: PALETTE COLOR、8bit の表（2 項目を 1 語に詰める）
//! - `palette-16bit.dcm`: PALETTE COLOR、16bit の表
//! - `palette-segmented.dcm`: PALETTE COLOR、Segmented の 16bit の表（直線の段と間接の段）。
//!   色相が 2 周する（左半分で 1 周、右半分でもう 1 周）
//! - `palette-first-64.dcm`: PALETTE COLOR、表の先頭が 64（64 より小さい番号は先頭の色）
//! - `ybr-full-422.dcm`: 非圧縮の YBR_FULL_422
//! - `ybr-full-422-3frames.dcm`: 非圧縮の YBR_FULL_422 の 3 フレーム（赤み・緑み・青み）
//! - `rgb16.dcm`: 16bit の RGB（Bits Stored 16）
//! - `rgb12.dcm`: 16bit の RGB（Bits Stored 12）
//! - `ybr-full16.dcm`: 16bit の YBR_FULL
//! - `palette-no-table.dcm`: PALETTE COLOR なのに色の表がない壊れたファイル（読めずにログに出る）

use std::path::PathBuf;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use vw_dicom::synth::Synth;

const SIDE: usize = 256;

/// 色相 `h`（0..1）の鮮やかな色。
fn hue(h: f32) -> [f32; 3] {
    let k = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        1.0 - k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [k(5.0), k(3.0), k(1.0)]
}

/// 0..1 の RGB の絵。
fn picture() -> Vec<[f32; 3]> {
    let c = SIDE as f32 / 2.0;
    (0..SIDE * SIDE)
        .map(|i| {
            let (x, y) = ((i % SIDE) as f32, (i / SIDE) as f32);
            if ((x - c).powi(2) + (y - c).powi(2)).sqrt() < 30.0 {
                [1.0; 3]
            } else {
                let shade = 1.0 - 0.8 * y / SIDE as f32;
                hue(x / SIDE as f32).map(|v| v * shade)
            }
        })
        .collect()
}

fn to_ybr([r, g, b]: [f32; 3], max: f32) -> [f32; 3] {
    let half = (max + 1.0) / 2.0;
    [
        0.299 * r + 0.587 * g + 0.114 * b,
        half - 0.168736 * r - 0.331264 * g + 0.5 * b,
        half + 0.5 * r - 0.418688 * g - 0.081312 * b,
    ]
    .map(|v| v.round().clamp(0.0, max))
}

fn rgb8(p: &[[f32; 3]]) -> Vec<u8> {
    p.iter()
        .flat_map(|c| c.map(|v| (v * 255.0).round() as u8))
        .collect()
}

/// 横の 2 画素ごとに Y1 Y2 Cb Cr（Cb・Cr は 2 画素の平均）。
fn ybr_422(p: &[[f32; 3]]) -> Vec<u8> {
    p.chunks(2)
        .flat_map(|pair| {
            let [a, b] = [pair[0], pair[1]].map(|c| to_ybr(c.map(|v| v * 255.0), 255.0));
            [
                a[0],
                b[0],
                ((a[1] + b[1]) / 2.0).round(),
                ((a[2] + b[2]) / 2.0).round(),
            ]
            .map(|v| v as u8)
        })
        .collect()
}

/// 色相の帯の番号（0..254）と、円の 255。番号では明るさを持てないので、上下の暗さは付けない。
fn indices() -> Vec<u8> {
    picture()
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if *c == [1.0; 3] {
                255
            } else {
                ((i % SIDE) * 254 / (SIDE - 1)) as u8
            }
        })
        .collect()
}

/// 番号 0..254 は色相を一周、255 は白。値は 0..1。
fn rainbow_lut() -> [Vec<f32>; 3] {
    let colors: Vec<[f32; 3]> = (0..256)
        .map(|i| {
            if i == 255 {
                [1.0; 3]
            } else {
                hue(i as f32 / 255.0)
            }
        })
        .collect();
    [0, 1, 2].map(|c| colors.iter().map(|v| v[c]).collect())
}

fn pack8(values: &[f32]) -> Vec<u16> {
    values
        .chunks(2)
        .map(|p| u16::from_le_bytes([(p[0] * 255.0) as u8, (p[1] * 255.0) as u8]))
        .collect()
}

fn words16(values: &[f32]) -> Vec<u16> {
    values
        .iter()
        .map(|v| (v * 65535.0).round() as u16)
        .collect()
}

fn palette(name: &str, mut add: impl FnMut(Synth, usize) -> Synth) -> (String, Synth) {
    let mut s = Synth::palette(SIDE as u16, SIDE as u16, &indices());
    for channel in 0..3 {
        s = add(s, channel);
    }
    (name.to_owned(), s)
}

/// 色相を 2 周する表を Segmented で書く。1 周目は 6 つの角を直線の段でつなぎ、2 周目は
/// 間接の段で 1 周目の直線の段を使い回し、最後に白の 1 項目を離散の段で足す。
fn segmented(channel: usize) -> Vec<u16> {
    let corner = |i: usize| (hue(i as f32 / 6.0)[channel] * 65535.0).round() as u16;
    let mut words = vec![0, 1, corner(0)];
    let first_linear = words.len();
    // 1 + 127 項目で 1 周。
    let mut filled = 0;
    for i in 1..=6 {
        let end = i * 127 / 6;
        words.extend([1, (end - filled) as u16, corner(i % 6)]);
        filled = end;
    }
    // オフセットはデータの先頭からのバイト数を 32bit で、下位の語を先に。
    let offset = (first_linear * 2) as u32;
    words.extend([2, 6, offset as u16, (offset >> 16) as u16]);
    words.extend([0, 1, 65535]);
    words
}

fn main() -> std::io::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&dir)?;
    let pic = picture();
    let side = SIDE as u16;
    let lut = rainbow_lut();
    let mut files: Vec<(String, Synth)> = vec![
        (
            "reference-rgb8".into(),
            Synth::rgb(side, side, &rgb8(&pic), false),
        ),
        palette("palette-8bit", |s, c| {
            s.lut(c, 256, 0, 8, &pack8(&lut[c]), false)
        }),
        palette("palette-16bit", |s, c| {
            s.lut(c, 256, 0, 16, &words16(&lut[c]), false)
        }),
        palette("palette-segmented", |s, c| {
            s.lut(c, 256, 0, 16, &segmented(c), true)
        }),
        palette("palette-first-64", |s, c| {
            // 番号 64..255 だけを表に持つ。64 より小さい番号（左の 1/4）は番号 64 の色になる。
            s.lut(c, 192, 64, 16, &words16(&lut[c][64..]), false)
        }),
        (
            "ybr-full-422".into(),
            Synth::ybr_full_422(side, side, &ybr_422(&pic)),
        ),
    ];
    let tint = |k: usize| -> Vec<[f32; 3]> {
        pic.iter()
            .map(|c| {
                let mut c = *c;
                c[k] = (c[k] + 0.5).min(1.0);
                c
            })
            .collect()
    };
    let frames: Vec<u8> = (0..3).flat_map(|k| ybr_422(&tint(k))).collect();
    files.push((
        "ybr-full-422-3frames".into(),
        Synth::ybr_full_422(side, side, &frames).text(tags::NUMBER_OF_FRAMES, VR::IS, "3"),
    ));
    for (name, bits) in [("rgb16", 16u32), ("rgb12", 12)] {
        let max = ((1u32 << bits) - 1) as f32;
        let data: Vec<u16> = pic
            .iter()
            .flat_map(|c| c.map(|v| (v * max).round() as u16))
            .collect();
        files.push((name.into(), Synth::rgb16(side, side, &data, bits as u16)));
    }
    let ybr16: Vec<u16> = pic
        .iter()
        .flat_map(|c| to_ybr(c.map(|v| v * 65535.0), 65535.0).map(|v| v as u16))
        .collect();
    files.push((
        "ybr-full16".into(),
        Synth::rgb16(side, side, &ybr16, 16).photometric("YBR_FULL"),
    ));
    files.push((
        "palette-no-table".into(),
        Synth::palette(side, side, &indices()),
    ));
    for (name, synth) in files {
        let path = dir.join(format!("{name}.dcm"));
        synth.write(&path)?;
        println!("{}", path.display());
    }
    Ok(())
}
