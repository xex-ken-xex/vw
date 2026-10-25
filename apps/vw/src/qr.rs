//! 画像の中の QR コードを読む。

use eframe::egui::ColorImage;

/// 読めた QR コード 1 つ。
#[derive(Debug, Clone, PartialEq)]
pub struct Code {
    pub text: String,
    /// 四隅の画素の位置（左上から時計回り）。
    pub corners: [[i32; 2]; 4],
}

/// 見つけたものの、壊れていて読めなかった QR コード。
#[derive(Debug, Clone, PartialEq)]
pub struct Unreadable {
    pub corners: [[i32; 2]; 4],
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub codes: Vec<Code>,
    pub unreadable: Vec<Unreadable>,
}

/// `image` の中の QR コードを探して読む。見つけた順（上から）に並ぶ。
pub fn read(image: &ColorImage) -> Found {
    let [w, h] = image.size;
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        let p = image.pixels[y * w + x];
        luma(p.r(), p.g(), p.b())
    });
    let mut found = Found::default();
    for grid in prepared.detect_grids() {
        let corners = grid.bounds.map(|p| [p.x, p.y]);
        let mut bytes = Vec::new();
        match grid.decode_to(&mut bytes) {
            Ok(_) => found.codes.push(Code {
                text: text_of(&bytes),
                corners,
            }),
            Err(e) => found.unreadable.push(Unreadable {
                corners,
                reason: e.to_string(),
            }),
        }
    }
    found
}

fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((r as u32 * 54 + g as u32 * 183 + b as u32 * 19) >> 8) as u8
}

/// 中身の文字。QR コードの 8 ビットのモードは文字コードを決めていないので、UTF-8 として読めなければ
/// Shift_JIS として読む（日本で作られたものに多い）。どちらでもなければ 16 進で出す。
fn text_of(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_owned();
    }
    let (s, _, bad) = encoding_rs::SHIFT_JIS.decode(bytes);
    if !bad {
        return s.into_owned();
    }
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;

    /// `data` の QR コードを、1 モジュール `scale` 画素で、周りに 4 モジュールの余白を付けて描く。
    fn qr_image(data: &[u8], scale: usize) -> ColorImage {
        let code = qrcode::QrCode::new(data).unwrap();
        let n = code.width();
        let colors = code.to_colors();
        let side = (n + 8) * scale;
        let mut img = ColorImage::new([side, side], vec![Color32::WHITE; side * side]);
        for (i, c) in colors.iter().enumerate() {
            if *c == qrcode::Color::Dark {
                let (mx, my) = (i % n + 4, i / n + 4);
                for y in my * scale..(my + 1) * scale {
                    for x in mx * scale..(mx + 1) * scale {
                        img.pixels[y * side + x] = Color32::BLACK;
                    }
                }
            }
        }
        img
    }

    #[test]
    fn reads_the_text_of_a_qr_code() {
        let found = read(&qr_image(b"https://example.com/vw", 4));
        let texts: Vec<_> = found.codes.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["https://example.com/vw"]);
        assert!(found.unreadable.is_empty());
    }

    #[test]
    fn reads_japanese_written_in_utf8() {
        let found = read(&qr_image("画像ビューア".as_bytes(), 4));
        assert_eq!(found.codes[0].text, "画像ビューア");
    }

    #[test]
    fn reads_japanese_written_in_shift_jis() {
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("検査番号");
        let found = read(&qr_image(&sjis, 4));
        assert_eq!(found.codes[0].text, "検査番号");
    }

    #[test]
    fn binary_contents_are_shown_in_hex() {
        assert_eq!(text_of(&[0xff, 0x00, 0x80]), "ff 00 80");
    }

    #[test]
    fn an_image_without_a_qr_code_finds_nothing() {
        let img = ColorImage::new([64, 64], vec![Color32::GRAY; 64 * 64]);
        assert_eq!(read(&img), Found::default());
    }

    #[test]
    fn corners_are_where_the_code_is_drawn() {
        let found = read(&qr_image(b"vw", 4));
        let xs = found.codes[0].corners.map(|[x, _]| x);
        // 余白の 4 モジュール（16 画素）の内側に枠がある。
        assert!(xs.iter().all(|&x| (14..=120).contains(&x)), "{xs:?}");
    }
}
