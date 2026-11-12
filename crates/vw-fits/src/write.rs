//! FITS に書く（貼り合わせた画像の保存など）。

use vw_core::{ColorModel, PixelData, VwImage};

use crate::header::BLOCK;

/// `img` を、値を 32bit の実数にした FITS にする。色は 3 つの面（NAXIS3 = 3）に分け、
/// アルファが 0 の画素は NaN（写っていない所）にする。行は画像の上から順に書き、
/// `ROWORDER = 'TOP-DOWN'` を付ける（vw も多くの撮影ソフトも、この向きで読む）。
///
/// `cards` は (名前, 値) で、値は数ならそのまま、ほかは文字として引用符で囲む。`history` は
/// 1 行ずつ HISTORY に書く。整数で書かないのは、何枚かを混ぜた値は整数に収まらず、写っていない
/// 所を NaN で表せるのが実数だけのため。
pub fn encode(
    img: &VwImage,
    cards: &[(String, String)],
    history: &[String],
) -> Result<Vec<u8>, String> {
    encode_rows(img, cards, history, Some("TOP-DOWN"))
}

/// [`encode`] の、`ROWORDER` を `row_order` にする（`None` は書かない）版。`img` の行は
/// ファイルに並べる順のまま書く。元のファイルの行の並びをそのまま引き継ぐ画像（キューブから
/// 作った図）に使う。`ROWORDER` のないファイルから作った図に `TOP-DOWN` と書くと、設定で
/// 上下を返して見ている人には、元の画像と上下が逆に出るため。
pub(crate) fn encode_rows(
    img: &VwImage,
    cards: &[(String, String)],
    history: &[String],
    row_order: Option<&str>,
) -> Result<Vec<u8>, String> {
    let (w, h) = (img.width as usize, img.height as usize);
    let (color, alpha) = match img.color {
        ColorModel::Gray => (1, false),
        ColorModel::GrayAlpha => (1, true),
        ColorModel::Rgb => (3, false),
        ColorModel::Rgba => (3, true),
    };
    let stride = color + usize::from(alpha);
    let values: Vec<f32> = match &img.data {
        PixelData::U8(v) => v.iter().map(|&x| f32::from(x)).collect(),
        PixelData::U16(v) => v.iter().map(|&x| f32::from(x)).collect(),
        PixelData::F32(v) => v.clone(),
    };
    if values.len() != w * h * stride {
        return Err("the image data does not match its size".into());
    }
    let mut header = vec![
        card_logical("SIMPLE", true),
        card_number("BITPIX", "-32"),
        card_number("NAXIS", if color == 3 { "3" } else { "2" }),
        card_number("NAXIS1", &w.to_string()),
        card_number("NAXIS2", &h.to_string()),
    ];
    if color == 3 {
        header.push(card_number("NAXIS3", "3"));
    }
    if let Some(order) = row_order {
        header.push(card_string("ROWORDER", order));
    }
    for (key, value) in cards {
        let reserved = [
            "SIMPLE", "BITPIX", "NAXIS", "NAXIS1", "NAXIS2", "NAXIS3", "ROWORDER", "END",
        ];
        if reserved.contains(&key.as_str()) {
            continue;
        }
        header.push(if crate::sky::number(value).is_some() {
            card_number(key, value)
        } else {
            card_string(key, value)
        });
    }
    for line in history {
        for part in line.chars().collect::<Vec<_>>().chunks(72) {
            header.push(pad(&format!("HISTORY {}", part.iter().collect::<String>())));
        }
    }
    header.push(pad("END"));
    let mut out: Vec<u8> = header.concat().into_bytes();
    out.resize(out.len().div_ceil(BLOCK) * BLOCK, b' ');
    for c in 0..color {
        for i in 0..w * h {
            let px = &values[i * stride..(i + 1) * stride];
            let v = if alpha && px[color] == 0.0 {
                f32::NAN
            } else {
                px[c]
            };
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
    out.resize(out.len().div_ceil(BLOCK) * BLOCK, 0);
    Ok(out)
}

/// 80 文字に足りない分を空白で埋める（長い分は切る）。
fn pad(s: &str) -> String {
    let mut s: String = s.chars().filter(char::is_ascii).take(80).collect();
    while s.len() < 80 {
        s.push(' ');
    }
    s
}

fn card_number(key: &str, value: &str) -> String {
    pad(&format!("{key:<8}= {value:>20}"))
}

fn card_logical(key: &str, value: bool) -> String {
    pad(&format!("{key:<8}= {:>20}", if value { "T" } else { "F" }))
}

fn card_string(key: &str, value: &str) -> String {
    let quoted = format!("'{:<8}'", value.replace('\'', "''"));
    pad(&format!("{key:<8}= {quoted}"))
}
