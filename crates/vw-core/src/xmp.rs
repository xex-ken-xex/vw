//! XMP（画像に埋め込まれた XML のメタデータ）から、決まった項目だけを読む。
//!
//! 今読むのは IPTC のデジタルソースタイプ（写真か、生成 AI で作ったものか、など）だけ。
//! XML を組み立てて読む仕組み（XML のクレート）は使わず、項目の名前を文字列で探す。項目 1 つの
//! ために依存を足して起動を重くしないためで、XMP の書き方（属性と要素の 2 通り）は両方読む。

use crate::Metadata;

/// IPTC NewsCodes のデジタルソースタイプのコードと、IPTC の付けた英語の名前。
///
/// 名前は IPTC NewsCodes（<https://cv.iptc.org/newscodes/digitalsourcetype/>、
/// Copyright IPTC、CC BY 4.0）による。廃止されたコードも、古いファイルに残っているので読む。
const DIGITAL_SOURCE_TYPES: &[(&str, &str)] = &[
    ("digitalCapture", "Digital capture sampled from real life"),
    (
        "computationalCapture",
        "Multi-frame computational capture sampled from real life",
    ),
    ("negativeFilm", "Digitised from a transparent negative"),
    ("positiveFilm", "Digitised from a transparent positive"),
    ("print", "Digitised from a non-transparent medium"),
    (
        "minorHumanEdits",
        "Original media with minor human edits (retired code)",
    ),
    ("humanEdits", "Human-edited media"),
    (
        "compositeWithTrainedAlgorithmicMedia",
        "Edited using Generative AI",
    ),
    ("algorithmicallyEnhanced", "Algorithmically-altered media"),
    ("softwareImage", "Created by software (retired code)"),
    ("digitalArt", "Digital art (retired code)"),
    ("digitalCreation", "Digital creation"),
    ("dataDrivenMedia", "Data-driven media"),
    ("trainedAlgorithmicMedia", "Created using Generative AI"),
    ("algorithmicMedia", "Pure algorithmic media"),
    ("screenCapture", "Screen capture"),
    ("virtualRecording", "Virtual event recording"),
    ("composite", "Composite of elements"),
    ("compositeCapture", "Composite of captured elements"),
    (
        "compositeSynthetic",
        "Composite including generative AI elements",
    ),
];

/// 生成 AI がかかわったことを表すコード。
const GENERATIVE_AI: &[&str] = &[
    "trainedAlgorithmicMedia",
    "compositeWithTrainedAlgorithmicMedia",
    "compositeSynthetic",
];

/// XMP の中身を読み、分かった項目を `m` の "XMP" に足す。
pub fn push_xmp(m: &mut Metadata, xmp: &[u8]) {
    let text = String::from_utf8_lossy(xmp);
    if let Some(code) = digital_source_type(&text) {
        let value = match DIGITAL_SOURCE_TYPES.iter().find(|(c, _)| *c == code) {
            Some((_, name)) => format!("{name} ({code})"),
            None => code.clone(),
        };
        m.push("XMP", "Digital source type", value);
        if GENERATIVE_AI.contains(&code.as_str()) {
            m.push("XMP", "Generative AI", "yes");
        }
    }
}

/// `Iptc4xmpExt:DigitalSourceType` の値の、URI の最後の部分（コード）。
fn digital_source_type(xmp: &str) -> Option<String> {
    let at = xmp.find("DigitalSourceType")? + "DigitalSourceType".len();
    let rest = &xmp[at..];
    // 属性（`="…"`）、要素（`>…<`）、rdf:resource（`rdf:resource="…"`）のどれでも、名前の後の
    // 最初の値を読む。
    let start = rest.find(['"', '\'', '>'])? + 1;
    let value = &rest[start..];
    let end = value.find(['"', '\'', '<'])?;
    let value = value[..end].trim();
    let code = value.rsplit('/').next()?.trim();
    (!code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric())).then(|| code.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(xmp: &str) -> Vec<(String, String)> {
        let mut m = Metadata::default();
        push_xmp(&mut m, xmp.as_bytes());
        m.entries.into_iter().map(|(_, k, v)| (k, v)).collect()
    }

    #[test]
    fn reads_the_digital_source_type_written_as_an_attribute() {
        let xmp = r#"<rdf:Description xmlns:Iptc4xmpExt="http://iptc.org/std/Iptc4xmpExt/2008-02-29/"
            Iptc4xmpExt:DigitalSourceType="http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia"/>"#;
        assert_eq!(
            read(xmp),
            [
                (
                    "Digital source type".to_owned(),
                    "Created using Generative AI (trainedAlgorithmicMedia)".to_owned()
                ),
                ("Generative AI".to_owned(), "yes".to_owned())
            ]
        );
    }

    #[test]
    fn reads_the_digital_source_type_written_as_an_element() {
        let xmp = "<Iptc4xmpExt:DigitalSourceType>\n  http://cv.iptc.org/newscodes/digitalsourcetype/digitalCapture\n</Iptc4xmpExt:DigitalSourceType>";
        assert_eq!(
            read(xmp),
            [(
                "Digital source type".to_owned(),
                "Digital capture sampled from real life (digitalCapture)".to_owned()
            )]
        );
    }

    #[test]
    fn reads_an_rdf_resource() {
        let xmp = r#"<Iptc4xmpExt:DigitalSourceType rdf:resource="http://cv.iptc.org/newscodes/digitalsourcetype/compositeSynthetic"/>"#;
        assert_eq!(read(xmp).len(), 2);
    }

    #[test]
    fn an_unknown_code_is_shown_as_it_is() {
        let xmp = r#"Iptc4xmpExt:DigitalSourceType="http://cv.iptc.org/newscodes/digitalsourcetype/somethingNew""#;
        assert_eq!(
            read(xmp),
            [("Digital source type".to_owned(), "somethingNew".to_owned())]
        );
    }

    #[test]
    fn xmp_without_the_field_adds_nothing() {
        assert!(read("<x:xmpmeta><dc:title>sea</dc:title></x:xmpmeta>").is_empty());
    }
}
