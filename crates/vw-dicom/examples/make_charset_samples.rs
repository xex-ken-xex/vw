//! 日本語の名前の読み方を確かめる DICOM を作る。
//! `cargo run -p vw-dicom --example make_charset_samples -- <出力フォルダ>`
//!
//! - `jis-kanji.dcm`: `ISO 2022 IR 6\ISO 2022 IR 87`。患者名は「ローマ字=漢字=ひらがな」
//! - `jis-kana.dcm`: `ISO 2022 IR 13\ISO 2022 IR 87`。患者名は「半角カナ=漢字=ひらがな」
//! - `utf8.dcm`: `ISO_IR 192`（UTF-8）。比べる用
//! - `latin-kanji.dcm`: `ISO 2022 IR 100\ISO 2022 IR 87`。漢字の後を規格どおり `ESC ( B` で戻す
//! - `latin-kanji-pydicom.dcm`: 同じ組で、pydicom 3 が書くように漢字の後を `ESC - A` で戻す
//! - `cyrillic-kanji.dcm`: `ISO 2022 IR 144\ISO 2022 IR 87`
//! - `greek-kanji.dcm`: `ISO 2022 IR 126\ISO 2022 IR 87`
//! - `latin-cyrillic.dcm`: `ISO 2022 IR 100\ISO 2022 IR 144`。値の途中で `ESC - L` に切り替える
//! - `supplementary-kanji.dcm`: `ISO 2022 IR 6\ISO 2022 IR 87\ISO 2022 IR 159`。補助漢字の「鷗」
//!
//! 名前・検査・シリーズはすべて架空のもの。

use std::path::PathBuf;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use vw_dicom::synth::Synth;

/// JIS X 0208 の文字を ESC $ B … ESC ( B で囲んだ 7 ビットの文字列にする。
fn jis(text: &str) -> String {
    use encoding::{EncoderTrap, Encoding};
    let bytes = encoding::all::ISO_2022_JP
        .encode(text, EncoderTrap::Strict)
        .expect("JIS X 0208 にある文字だけを使う");
    let mut text = String::from_utf8(bytes).expect("ISO-2022-JP は 7 ビット");
    // encoding は漢字で終わると ASCII に戻すエスケープを書かない。DICOM は区切り（^ や =）の前で
    // 戻すことを求めるので、ここで足す。
    if text.rfind("\x1b$B") > text.rfind("\x1b(B") {
        text.push_str("\x1b(B");
    }
    text
}

/// 半角カナ（JIS X 0201 の G1）の 1 バイトの符号。`Synth` は書くときに最初の文字集合
/// （ASCII）しか使えないので、同じ長さの印で書いてから置き換える。
const KANA: &[u8] = &[0xB6, 0xB8, 0xB3, b'^', 0xB2, 0xC1, 0xDB, 0xB3]; // ｶｸｳ^ｲﾁﾛｳ
const KANA_MARK: &str = "########";

fn sample(number: i32, charset: &str, name: &str, study: &str, series: &str) -> Synth {
    let uid = format!("1.2.826.0.1.3680043.9.78{number:02}");
    Synth::ct(64, 64)
        .text(tags::SPECIFIC_CHARACTER_SET, VR::CS, charset)
        .patient(name)
        .study(&format!("{uid}.1"), study, "20261016")
        .series(&format!("{uid}.2"), number, series)
        .instance(&format!("{uid}.3"), 1)
}

/// 1 バイトの文字集合で書いたバイト列。
fn bytes_in(e: encoding::EncodingRef, text: &str) -> Vec<u8> {
    use encoding::EncoderTrap;
    e.encode(text, EncoderTrap::Strict)
        .expect("その文字集合にある文字だけを使う")
}

/// 患者名と検査名を、ASCII 以外のバイトも含めてそのまま書く。`Synth` は ASCII しか書けないので、
/// 同じ長さの印で書いてから置き換える。
fn raw_sample(number: i32, charset: &str, name: &[u8], study: &[u8], series: &str) -> Vec<u8> {
    let name_mark = "#".repeat(name.len());
    let study_mark = "%".repeat(study.len());
    let mut bytes = sample(number, charset, &name_mark, &study_mark, series).bytes();
    for (mark, raw) in [(name_mark, name), (study_mark, study)] {
        let at = bytes
            .windows(raw.len())
            .position(|w| w == mark.as_bytes())
            .expect("印");
        bytes[at..at + raw.len()].copy_from_slice(raw);
    }
    bytes
}

fn main() -> std::io::Result<()> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "charset-samples".into()),
    );
    std::fs::create_dir_all(&out)?;
    let kanji = jis("架空^一郎");
    let hiragana = jis("かくう^いちろう");
    sample(
        1,
        "ISO 2022 IR 6\\ISO 2022 IR 87",
        &format!("Kakuu^Ichiro={kanji}={hiragana}"),
        &jis("胸部ＣＴ"),
        &jis("肺野条件"),
    )
    .write(&out.join("jis-kanji.dcm"))?;
    let mut kana = sample(
        2,
        "ISO 2022 IR 13\\ISO 2022 IR 87",
        &format!("{KANA_MARK}={kanji}={hiragana}"),
        &jis("頭部単純"),
        &jis("横断像"),
    )
    .bytes();
    let at = kana
        .windows(KANA.len())
        .position(|w| w == KANA_MARK.as_bytes())
        .expect("患者名の印");
    kana[at..at + KANA.len()].copy_from_slice(KANA);
    std::fs::write(out.join("jis-kana.dcm"), kana)?;
    sample(
        3,
        "ISO_IR 192",
        "Kakuu^Ichiro=架空^一郎=かくう^いちろう",
        "腹部造影",
        "動脈相",
    )
    .write(&out.join("utf8.dcm"))?;
    use encoding::all::{ISO_8859_1, ISO_8859_5, ISO_8859_7};
    let latin_name = bytes_in(ISO_8859_1, "Kakuu^Jörg");
    let head = jis("頭部");
    let latin_kanji = |back: &str| {
        let kanji = head.trim_end_matches("\x1b(B");
        [
            b"Kopf-CT ".as_slice(),
            kanji.as_bytes(),
            back.as_bytes(),
            &bytes_in(ISO_8859_1, " Übersicht"),
        ]
        .concat()
    };
    let samples = [
        (
            "latin-kanji.dcm",
            "ISO 2022 IR 100\\ISO 2022 IR 87",
            latin_name.clone(),
            latin_kanji("\x1b(B"),
        ),
        (
            "latin-kanji-pydicom.dcm",
            "ISO 2022 IR 100\\ISO 2022 IR 87",
            [b"\x1b-A".as_slice(), &latin_name].concat(),
            [b"\x1b-A".as_slice(), &latin_kanji("\x1b-A")].concat(),
        ),
        (
            "cyrillic-kanji.dcm",
            "ISO 2022 IR 144\\ISO 2022 IR 87",
            bytes_in(ISO_8859_5, "Какуу^Иван"),
            [bytes_in(ISO_8859_5, "КТ головы "), jis("頭部").into_bytes()].concat(),
        ),
        (
            "greek-kanji.dcm",
            "ISO 2022 IR 126\\ISO 2022 IR 87",
            bytes_in(ISO_8859_7, "Κακού^Νίκος"),
            [
                bytes_in(ISO_8859_7, "Αξονική κεφαλής "),
                jis("頭部").into_bytes(),
            ]
            .concat(),
        ),
        (
            "latin-cyrillic.dcm",
            "ISO 2022 IR 100\\ISO 2022 IR 144",
            latin_name.clone(),
            [
                bytes_in(ISO_8859_1, "Kopf Übersicht "),
                b"\x1b-L".to_vec(),
                bytes_in(ISO_8859_5, "КТ головы"),
                b"\x1b-A".to_vec(),
            ]
            .concat(),
        ),
        (
            "supplementary-kanji.dcm",
            "ISO 2022 IR 6\\ISO 2022 IR 87\\ISO 2022 IR 159",
            // 「鷗」は JIS X 0212 にだけあり、0x6C 0x3F。
            [
                b"Kakuu^Ouko=".as_slice(),
                jis("架空").as_bytes(),
                b"^\x1b$(Dl?\x1b(B".as_slice(),
                jis("子").as_bytes(),
            ]
            .concat(),
            jis("頭部単純").into_bytes(),
        ),
    ];
    for (i, (file, charset, name, study)) in samples.into_iter().enumerate() {
        let bytes = raw_sample(i as i32 + 4, charset, &name, &study, "Axial");
        std::fs::write(out.join(file), bytes)?;
    }
    println!("wrote samples to {}", out.display());
    Ok(())
}
