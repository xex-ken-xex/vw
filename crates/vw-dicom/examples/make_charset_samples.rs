//! 日本語の名前の読み方を確かめる DICOM を作る。
//! `cargo run -p vw-dicom --example make_charset_samples -- <出力フォルダ>`
//!
//! - `jis-kanji.dcm`: `ISO 2022 IR 6\ISO 2022 IR 87`。患者名は「ローマ字=漢字=ひらがな」
//! - `jis-kana.dcm`: `ISO 2022 IR 13\ISO 2022 IR 87`。患者名は「半角カナ=漢字=ひらがな」
//! - `utf8.dcm`: `ISO_IR 192`（UTF-8）。比べる用
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
    println!("wrote samples to {}", out.display());
    Ok(())
}
