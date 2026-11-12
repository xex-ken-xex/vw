//! メタ情報（グループ 0002）が規格に外れていて dicom-rs が開けないファイルの、タグだけを読む。
//!
//! dicom-rs はメタ情報を自分の決まりで厳しく調べ（AE の 16 バイト超え、UID の文字、グループ長の
//! 食い違いなど）、1 つでも外れるとファイルごと読むのをやめる。その決まりを緩める口はないので、
//! メタ情報はここで読み、本体（グループ 0002 より後ろ）だけを dicom-rs に読ませる。
//!
//! 画像を読むときには使わない（ken の判断 2026-11-12: 画像の読み込みは止まってよく、タグを見たい）。
//! メタ情報を書き直したファイルを dicom-rs に渡し直す案は採らない。書き直すとき、長すぎる値を
//! 切るか捨てるしかなく、ファイルにある値をそのまま見せられないため。

use std::io::Cursor;

use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry};
use dicom_core::{Tag, VR};
use dicom_dictionary_std::uids;
use dicom_encoding::TransferSyntaxIndex;
use dicom_object::meta::FileMetaTableBuilder;
use dicom_object::{DefaultDicomObject, InMemDicomObject};
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use vw_core::{DecodeError, Metadata};

use crate::structure;

const PREAMBLE: usize = 128;
const MAGIC: &[u8; 4] = b"DICM";

/// メタ情報の 1 つの要素。値はファイルのバイトのまま持つ。
pub(crate) struct MetaElement {
    pub tag: Tag,
    pub vr: VR,
    /// ファイルに書かれた長さ（値がファイルの終わりで切れていても、書かれた数）。
    pub length: u32,
    pub value: Vec<u8>,
    /// 明示 VR で書かれていたか（規格ではメタ情報は必ず明示 VR）。
    pub explicit: bool,
}

/// 緩く読んだファイル。
pub(crate) struct Opened {
    pub obj: DefaultDicomObject,
    pub meta: Vec<MetaElement>,
    /// 規格に外れていた箇所 `(場所, 理由)`。値は入れない（患者の情報が入りうるため）。
    pub problems: Vec<(String, String)>,
}

/// `data` はファイルの頭から。`header_only` なら一番外の画素データの手前で読むのをやめる。
pub(crate) fn open(data: &[u8], header_only: bool) -> Result<Opened, DecodeError> {
    if data.get(PREAMBLE..PREAMBLE + 4) != Some(MAGIC) {
        return Err(DecodeError::Invalid("no DICM after the preamble".into()));
    }
    let (meta, start) = read_meta(data)?;
    let mut problems = meta_problems(&meta, start);
    let syntax = choose_syntax(&meta, data, start, &mut problems);
    let pixel = structure::check_data_set(data, start, syntax.uid(), header_only)?;
    let end = match (header_only, pixel) {
        (true, Some(at)) => at.header_at as usize,
        _ => data.len(),
    };
    let body = data.get(start..end).unwrap_or_default();
    let obj = InMemDicomObject::read_dataset_with_ts(Cursor::new(body), syntax)
        .map_err(|e| DecodeError::Invalid(crate::error_chain(&e)))?;
    let table = meta_table(&meta, syntax.uid())?;
    Ok(Opened {
        obj: obj.with_exact_meta(table),
        meta,
        problems,
    })
}

/// メタ情報の要素を、グループが 0002 でなくなるまで読む。グループ長（0002,0000）は信じない。
/// 違っていても、要素そのものを辿れば本体の始まりが分かるため。返すのは要素と、本体の始まり。
fn read_meta(data: &[u8]) -> Result<(Vec<MetaElement>, usize), DecodeError> {
    let mut pos = PREAMBLE + 4;
    let mut out = Vec::new();
    while let Some(head) = data.get(pos..pos + 8) {
        let tag = Tag(
            u16::from_le_bytes([head[0], head[1]]),
            u16::from_le_bytes([head[2], head[3]]),
        );
        if tag.group() != 0x0002 {
            break;
        }
        let explicit = explicit_vr(&head[4..6]);
        let (vr, length, header) = match explicit {
            Some(vr) if long_length(vr) => {
                let Some(b) = data.get(pos + 8..pos + 12) else {
                    break;
                };
                (vr, u32::from_le_bytes([b[0], b[1], b[2], b[3]]), 12)
            }
            Some(vr) => (vr, u32::from(u16::from_le_bytes([head[6], head[7]])), 8),
            // メタ情報を暗黙 VR で書いてしまったファイル。
            None => (
                dictionary_vr(tag),
                u32::from_le_bytes([head[4], head[5], head[6], head[7]]),
                8,
            ),
        };
        if length == u32::MAX {
            return Err(DecodeError::Invalid(format!(
                "({:04X},{:04X}) in the file meta information has no length",
                tag.group(),
                tag.element()
            )));
        }
        let value_at = pos + header;
        let value_end = value_at.saturating_add(length as usize);
        let Some(value) = data.get(value_at..value_end) else {
            return Err(DecodeError::Invalid(format!(
                "({:04X},{:04X}) in the file meta information is longer than the file",
                tag.group(),
                tag.element()
            )));
        };
        out.push(MetaElement {
            tag,
            vr,
            length,
            value: value.to_vec(),
            explicit: explicit.is_some(),
        });
        pos = value_end;
    }
    Ok((out, pos))
}

fn explicit_vr(code: &[u8]) -> Option<VR> {
    VR::from_binary([code[0], code[1]])
}

fn long_length(vr: VR) -> bool {
    matches!(
        vr,
        VR::OB
            | VR::OD
            | VR::OF
            | VR::OL
            | VR::OV
            | VR::OW
            | VR::SQ
            | VR::SV
            | VR::UC
            | VR::UN
            | VR::UR
            | VR::UT
            | VR::UV
    )
}

fn dictionary_vr(tag: Tag) -> VR {
    dicom_dictionary_std::StandardDataDictionary
        .by_tag(tag)
        .map_or(VR::UN, |e| e.vr().relaxed())
}

/// 上限の長さ（PS3.5 6.2）を持つ、メタ情報の文字の値。
fn max_len(tag: Tag) -> Option<usize> {
    match dictionary_vr(tag) {
        VR::AE | VR::SH => Some(16),
        VR::UI => Some(64),
        _ => None,
    }
}

fn meta_problems(meta: &[MetaElement], start: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut push = |e: &MetaElement, why: String| {
        out.push((
            format!(
                "{} {:?}, {} bytes",
                crate::listed_key(e.tag, 0),
                e.vr,
                e.length
            ),
            why,
        ));
    };
    for e in meta {
        if let Some(max) = max_len(e.tag)
            && e.value.len() > max
        {
            push(e, format!("longer than {max} bytes"));
        }
        if dictionary_vr(e.tag) == VR::UI
            && !e
                .value
                .iter()
                .all(|&b| b == b'.' || b.is_ascii_digit() || b == 0 || b == b' ')
        {
            push(e, "has characters other than digits and '.'".into());
        }
        if e.tag == Tag(0x0002, 0x0001) && e.value.len() != 2 {
            push(e, "must be 2 bytes".into());
        }
    }
    match meta.first() {
        Some(first) if first.tag == Tag(0x0002, 0x0000) => {
            let written = (first.value.len() == 4).then(|| {
                u32::from_le_bytes([
                    first.value[0],
                    first.value[1],
                    first.value[2],
                    first.value[3],
                ])
            });
            // グループ長の要素は 12 バイト（タグ 4、VR 2、長さ 2、値 4）。
            let actual = (start - (PREAMBLE + 4)).saturating_sub(12) as u32;
            if written != Some(actual) {
                let says = written.map_or("an unreadable length".to_owned(), |n| format!("{n}"));
                push(
                    first,
                    format!("says {says}, but the elements after it take {actual} bytes"),
                );
            }
        }
        _ => out.push((
            "(0002,0000) FileMetaInformationGroupLength".into(),
            "missing or not first".into(),
        )),
    }
    if meta.iter().any(|e| !e.explicit) {
        out.push((
            "File meta information".into(),
            "written in implicit VR (it must be explicit VR little endian)".into(),
        ));
    }
    out
}

/// 本体を読む転送構文。書かれていない・知らないもの、書かれたものと本体の書き方（暗黙 VR か
/// 明示 VR か）が食い違うものは、本体の頭の要素を見て決め、そのことを `problems` に残す。
fn choose_syntax(
    meta: &[MetaElement],
    data: &[u8],
    start: usize,
    problems: &mut Vec<(String, String)>,
) -> &'static dicom_encoding::TransferSyntax {
    let looks_explicit = data
        .get(start + 4..start + 6)
        .is_some_and(|code| explicit_vr(code).is_some());
    let guessed = if looks_explicit {
        uids::EXPLICIT_VR_LITTLE_ENDIAN
    } else {
        uids::IMPLICIT_VR_LITTLE_ENDIAN
    };
    let get = |uid: &str| TransferSyntaxRegistry.get(uid).expect("built-in syntax");
    let written = meta
        .iter()
        .find(|e| e.tag == Tag(0x0002, 0x0010))
        .map(|e| text_of(&e.value));
    let place = "(0002,0010) TransferSyntaxUID".to_owned();
    let Some(written) = written else {
        problems.push((place, format!("missing; read as {}", name_of(guessed))));
        return get(guessed);
    };
    let Some(ts) = TransferSyntaxRegistry.get(&written) else {
        problems.push((place, format!("unknown; read as {}", name_of(guessed))));
        return get(guessed);
    };
    let native =
        ts.uid() == uids::EXPLICIT_VR_LITTLE_ENDIAN || ts.uid() == uids::IMPLICIT_VR_LITTLE_ENDIAN;
    if native && data.len() > start && ts.uid() != guessed {
        problems.push((
            place,
            format!(
                "the data set is not written as it says; read as {}",
                name_of(guessed)
            ),
        ));
        return get(guessed);
    }
    ts
}

fn name_of(uid: &str) -> &'static str {
    if uid == uids::EXPLICIT_VR_LITTLE_ENDIAN {
        "Explicit VR Little Endian"
    } else {
        "Implicit VR Little Endian"
    }
}

fn text_of(value: &[u8]) -> String {
    String::from_utf8_lossy(value)
        .trim_matches(['\0', ' '])
        .to_owned()
}

/// dicom-rs の表。値は表示に使わないので（表示は [`MetaElement`] から）、作れる形に整えて入れる。
fn meta_table(
    meta: &[MetaElement],
    syntax: &str,
) -> Result<dicom_object::FileMetaTable, DecodeError> {
    let get = |element: u16| {
        meta.iter()
            .find(|e| e.tag == Tag(0x0002, element))
            .map(|e| text_of(&e.value))
    };
    let mut b = FileMetaTableBuilder::new().transfer_syntax(syntax);
    if let Some(v) = get(0x0002) {
        b = b.media_storage_sop_class_uid(v);
    }
    if let Some(v) = get(0x0003) {
        b = b.media_storage_sop_instance_uid(v);
    }
    if let Some(v) = get(0x0012) {
        b = b.implementation_class_uid(v);
    }
    if let Some(v) = get(0x0013) {
        b = b.implementation_version_name(v);
    }
    b.build()
        .map_err(|e| DecodeError::Invalid(crate::error_chain(&e)))
}

impl Opened {
    /// メタデータ欄の「DICOM tags」に出す、メタ情報の全要素。値はファイルにあるまま（切らない）。
    pub(crate) fn meta_rows(&self) -> Vec<(Tag, String)> {
        self.meta
            .iter()
            .map(|e| {
                let value = match e.vr {
                    VR::UL if e.value.len() == 4 => {
                        u32::from_le_bytes([e.value[0], e.value[1], e.value[2], e.value[3]])
                            .to_string()
                    }
                    VR::OB | VR::OW | VR::UN => format!("({:?}, {} bytes)", e.vr, e.value.len()),
                    _ => text_of(&e.value),
                };
                (e.tag, value)
            })
            .collect()
    }

    pub(crate) fn push_problems(&self, m: &mut Metadata) {
        for (place, why) in &self.problems {
            m.problem("DICOM", place.clone(), why.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use vw_core::DecoderPlugin;

    use crate::{DicomPlugin, TAG_LISTING};

    const EXPLICIT_LE: &str = "1.2.840.10008.1.2.1";
    const IMPLICIT_LE: &str = "1.2.840.10008.1.2";

    fn even(value: &[u8], pad: u8) -> Vec<u8> {
        let mut v = value.to_vec();
        if v.len() % 2 == 1 {
            v.push(pad);
        }
        v
    }

    fn explicit(group: u16, element: u16, vr: &[u8; 2], value: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(group.to_le_bytes());
        b.extend(element.to_le_bytes());
        b.extend(vr);
        if vr == b"OB" {
            b.extend([0, 0]);
            b.extend((value.len() as u32).to_le_bytes());
        } else {
            b.extend((value.len() as u16).to_le_bytes());
        }
        b.extend(value);
        b
    }

    fn implicit(group: u16, element: u16, value: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(group.to_le_bytes());
        b.extend(element.to_le_bytes());
        b.extend((value.len() as u32).to_le_bytes());
        b.extend(value);
        b
    }

    /// メタ情報の要素（グループ長を除く）の並びと、書くグループ長（`None` なら正しい値）。
    fn file(meta: &[Vec<u8>], group_length: Option<u32>, body: &[Vec<u8>]) -> Vec<u8> {
        let elements: Vec<u8> = meta.concat();
        let length = group_length.unwrap_or(elements.len() as u32);
        let mut f = vec![0u8; 128];
        f.extend(b"DICM");
        f.extend(explicit(0x0002, 0x0000, b"UL", &length.to_le_bytes()));
        f.extend(elements);
        f.extend(body.concat());
        f
    }

    fn meta_with(syntax: &str, source_ae: &[u8]) -> Vec<Vec<u8>> {
        vec![
            explicit(0x0002, 0x0001, b"OB", &[0, 1]),
            explicit(
                0x0002,
                0x0002,
                b"UI",
                &even(b"1.2.840.10008.5.1.4.1.1.7", 0),
            ),
            explicit(0x0002, 0x0003, b"UI", &even(b"1.2.3.4", 0)),
            explicit(0x0002, 0x0010, b"UI", &even(syntax.as_bytes(), 0)),
            explicit(0x0002, 0x0012, b"UI", &even(b"1.2.3", 0)),
            explicit(0x0002, 0x0016, b"AE", &even(source_ae, b' ')),
        ]
    }

    fn explicit_body() -> Vec<Vec<u8>> {
        vec![
            explicit(0x0008, 0x0060, b"CS", b"OT"),
            explicit(0x0010, 0x0010, b"PN", &even(b"Yamada^Taro", b' ')),
        ]
    }

    fn implicit_body() -> Vec<Vec<u8>> {
        vec![
            implicit(0x0008, 0x0060, b"OT"),
            implicit(0x0010, 0x0010, &even(b"Yamada^Taro", b' ')),
        ]
    }

    fn listed(m: &vw_core::Metadata, key: &str) -> Option<String> {
        m.get(TAG_LISTING, key).map(str::to_owned)
    }

    fn problems(m: &vw_core::Metadata) -> String {
        m.problems
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    const LONG_AE: &[u8] = b"WORKSTATION-123456";

    #[test]
    fn tags_are_read_when_the_source_ae_title_is_longer_than_16_bytes() {
        let f = file(&meta_with(EXPLICIT_LE, LONG_AE), None, &explicit_body());
        let m = DicomPlugin.read_metadata(&f).unwrap();
        assert_eq!(m.get("Patient", "Name"), Some("Yamada^Taro"));
        assert_eq!(m.get("DICOM", "Modality"), Some("OT"));
        assert_eq!(
            listed(&m, "(0002,0016) SourceApplicationEntityTitle").as_deref(),
            Some("WORKSTATION-123456"),
            "the value is shown whole, not cut at 16 bytes"
        );
    }

    #[test]
    fn the_image_still_fails_with_the_same_error_when_the_meta_is_broken() {
        let f = file(&meta_with(EXPLICIT_LE, LONG_AE), None, &explicit_body());
        let e = DicomPlugin.decode(&f).unwrap_err().to_string();
        assert!(e.contains("Unexpected length 18"), "{e}");
    }

    #[test]
    fn a_broken_meta_is_reported_by_tag_without_its_value() {
        let f = file(&meta_with(EXPLICIT_LE, LONG_AE), None, &explicit_body());
        let all = problems(&DicomPlugin.read_metadata(&f).unwrap());
        assert!(all.contains("(0002,0016)"), "{all}");
        assert!(all.contains("longer than 16 bytes"), "{all}");
        assert!(!all.contains("WORKSTATION"), "{all}");
    }

    #[test]
    fn a_wrong_group_length_does_not_swallow_the_first_tags_of_the_data_set() {
        let f = file(&meta_with(EXPLICIT_LE, b"VW"), Some(20), &explicit_body());
        let m = DicomPlugin.read_metadata(&f).unwrap();
        assert_eq!(m.get("DICOM", "Modality"), Some("OT"));
        assert!(problems(&m).contains("says 20"), "{}", problems(&m));
    }

    #[test]
    fn a_uid_with_letters_still_lets_the_tags_be_read() {
        let mut meta = meta_with(EXPLICIT_LE, b"VW");
        meta[2] = explicit(0x0002, 0x0003, b"UI", &even(b"1.2.ABC", 0));
        let m = DicomPlugin
            .read_metadata(&file(&meta, None, &explicit_body()))
            .unwrap();
        assert_eq!(m.get("Patient", "Name"), Some("Yamada^Taro"));
        assert!(
            problems(&m).contains("other than digits"),
            "{}",
            problems(&m)
        );
    }

    #[test]
    fn a_missing_transfer_syntax_is_guessed_from_the_data_set() {
        let mut meta = meta_with(EXPLICIT_LE, b"VW");
        meta.remove(3);
        let m = DicomPlugin
            .read_metadata(&file(&meta, None, &implicit_body()))
            .unwrap();
        assert_eq!(m.get("DICOM", "Modality"), Some("OT"));
        assert_eq!(
            m.get("DICOM", "Transfer syntax"),
            Some("Implicit VR Little Endian")
        );
        assert!(problems(&m).contains("missing"), "{}", problems(&m));
    }

    #[test]
    fn a_data_set_written_unlike_its_transfer_syntax_is_read_as_it_is_written() {
        let f = file(&meta_with(EXPLICIT_LE, LONG_AE), None, &implicit_body());
        let m = DicomPlugin.read_metadata(&f).unwrap();
        assert_eq!(m.get("Patient", "Name"), Some("Yamada^Taro"));
        assert!(
            problems(&m).contains("not written as it says"),
            "{}",
            problems(&m)
        );
    }

    #[test]
    fn a_meta_written_in_implicit_vr_is_read() {
        let meta: Vec<Vec<u8>> = vec![
            implicit(0x0002, 0x0002, &even(b"1.2.840.10008.5.1.4.1.1.7", 0)),
            implicit(0x0002, 0x0003, &even(b"1.2.3.4", 0)),
            implicit(0x0002, 0x0010, &even(IMPLICIT_LE.as_bytes(), 0)),
        ];
        let m = DicomPlugin
            .read_metadata(&file(&meta, None, &implicit_body()))
            .unwrap();
        assert_eq!(m.get("DICOM", "Modality"), Some("OT"));
        assert!(problems(&m).contains("implicit VR"), "{}", problems(&m));
    }

    #[test]
    fn a_catalog_reads_all_tags_of_a_file_with_a_broken_meta() {
        let f = file(&meta_with(EXPLICIT_LE, LONG_AE), None, &explicit_body());
        let m = DicomPlugin.read_all_metadata(&f).unwrap();
        assert_eq!(m.get("DICOM", "0010,0010"), Some("Yamada^Taro"));
    }

    #[test]
    fn a_file_without_pixel_data_shows_its_tags_but_no_image() {
        let f = file(&meta_with(EXPLICIT_LE, b"VW"), None, &explicit_body());
        assert!(DicomPlugin.decode(&f).is_err());
        let m = DicomPlugin.read_metadata(&f).unwrap();
        assert_eq!(m.get("Patient", "Name"), Some("Yamada^Taro"));
        assert!(m.problems.is_empty(), "{}", problems(&m));
    }

    #[test]
    fn a_file_whose_data_set_is_broken_too_keeps_the_strict_error() {
        let mut body = explicit_body();
        body.push({
            let mut b = Vec::new();
            b.extend(0x0010u16.to_le_bytes());
            b.extend(0x0020u16.to_le_bytes());
            b.extend(b"OB\0\0");
            b.extend(u32::MAX.to_le_bytes());
            b
        });
        let f = file(&meta_with(EXPLICIT_LE, LONG_AE), None, &body);
        assert!(DicomPlugin.read_metadata(&f).is_err());
    }
}
