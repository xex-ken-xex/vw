//! 画像は開けないがタグは見られる DICOM を作る。
//! `cargo run -p vw-dicom --example make_tag_only_samples -- <出力フォルダ>`
//!
//! - `01-long-source-ae.dcm`: (0002,0016) Source AE Title が 18 バイト（上限 16）
//! - `02-uid-with-letters.dcm`: (0002,0003) Media Storage SOP Instance UID に英字
//! - `03-wrong-group-length.dcm`: (0002,0000) グループ長が実際より短い
//! - `04-no-transfer-syntax.dcm`: (0002,0010) がなく、本体は暗黙 VR
//! - `05-syntax-mismatch.dcm`: 明示 VR と書いてあるのに、本体は暗黙 VR
//! - `06-implicit-meta.dcm`: メタ情報まで暗黙 VR で書いてある
//! - `07-dicomdir/DICOMDIR`: 患者・検査・シリーズ・画像の 4 段の記録を持つ DICOMDIR
//! - `08-text-report.dcm`: 構造化レポート（Basic Text SR）。所見の文と、測った長さを持つ
//! - `09-normal.dcm`: 比べる用の、どこも外れていない CT
//! - `10-broken-data-set.dcm`: 01 に加えて本体の頭の要素の長さも壊れている（タグも読めない）
//!
//! 名前・検査・シリーズはすべて架空のもの。

use std::path::{Path, PathBuf};

use dicom_core::value::DataSetSequence;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::tags;
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use vw_dicom::synth::{Synth, add_meta_element};

const EXPLICIT_LE: &str = "1.2.840.10008.1.2.1";
const IMPLICIT_LE: &str = "1.2.840.10008.1.2";
const CT_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.2";
const MEDIA_STORAGE_DIRECTORY: &str = "1.2.840.10008.1.3.10";
const BASIC_TEXT_SR: &str = "1.2.840.10008.5.1.4.1.1.88.11";

fn ct(number: i32, syntax: &'static str) -> Vec<u8> {
    let uid = format!("1.2.826.0.1.3680043.9.79{number:02}");
    let gradient: Vec<i16> = (0..64 * 64).map(|i| ((i % 64) * 30 - 900) as i16).collect();
    Synth::new(CT_IMAGE, 64, 64)
        .pixels_i16(&gradient)
        .transfer_syntax(syntax)
        .patient("Test^Taro")
        .study(&format!("{uid}.1"), "Tag-only samples", "20261112")
        .series(&format!("{uid}.2"), number, &format!("Sample {number:02}"))
        .instance(&format!("{uid}.3"), 1)
        .window(40.0, 400.0)
        .bytes()
}

/// メタ情報のグループ長の値の位置。
const GROUP_LENGTH_VALUE: usize = 128 + 4 + 8;

/// ファイルのメタ情報（グループ長の要素の後ろから本体の手前まで）と本体。
fn split(file: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let length = u32::from_le_bytes(
        file[GROUP_LENGTH_VALUE..GROUP_LENGTH_VALUE + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    let meta_start = GROUP_LENGTH_VALUE + 4;
    (
        file[meta_start..meta_start + length].to_vec(),
        file[meta_start + length..].to_vec(),
    )
}

/// プリアンブル・DICM・グループ長（`group_length`、`None` なら `meta` の長さ）・`meta`・`body`。
fn join(meta: &[u8], group_length: Option<u32>, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 128];
    out.extend(b"DICM");
    out.extend([0x02, 0x00, 0x00, 0x00, b'U', b'L', 4, 0]);
    out.extend(group_length.unwrap_or(meta.len() as u32).to_le_bytes());
    out.extend(meta);
    out.extend(body);
    out
}

/// 2 バイトの長さを持つ VR の要素の並びから、(0002,`element`) を除く。
fn without(meta: &[u8], element: u16) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < meta.len() {
        let e = u16::from_le_bytes([meta[pos + 2], meta[pos + 3]]);
        let vr = &meta[pos + 4..pos + 6];
        let (header, length) = if vr == b"OB" {
            (
                12,
                u32::from_le_bytes(meta[pos + 8..pos + 12].try_into().unwrap()) as usize,
            )
        } else {
            (
                8,
                u16::from_le_bytes([meta[pos + 6], meta[pos + 7]]) as usize,
            )
        };
        let end = pos + header + length;
        if e != element {
            out.extend(&meta[pos..end]);
        }
        pos = end;
    }
    out
}

fn explicit(element: u16, vr: &[u8; 2], value: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(0x0002u16.to_le_bytes());
    b.extend(element.to_le_bytes());
    b.extend(vr);
    b.extend((value.len() as u16).to_le_bytes());
    b.extend(value);
    b
}

fn implicit(element: u16, value: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(0x0002u16.to_le_bytes());
    b.extend(element.to_le_bytes());
    b.extend((value.len() as u32).to_le_bytes());
    b.extend(value);
    b
}

fn uid(text: &str) -> Vec<u8> {
    let mut v = text.as_bytes().to_vec();
    if v.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn put(obj: &mut InMemDicomObject, tag: Tag, vr: VR, value: &str) {
    obj.put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
}

fn sequence(obj: &mut InMemDicomObject, tag: Tag, items: Vec<InMemDicomObject>) {
    obj.put(DataElement::new(tag, VR::SQ, DataSetSequence::from(items)));
}

fn write_object(obj: InMemDicomObject, sop_class: &str, instance: &str, path: &Path) {
    let file = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(EXPLICIT_LE)
                .media_storage_sop_class_uid(sop_class)
                .media_storage_sop_instance_uid(instance),
        )
        .unwrap();
    file.write_to_file(path).unwrap();
}

fn record(kind: &str, fields: &[(Tag, VR, &str)]) -> InMemDicomObject {
    let mut r = InMemDicomObject::new_empty();
    put(&mut r, tags::DIRECTORY_RECORD_TYPE, VR::CS, kind);
    for &(tag, vr, value) in fields {
        put(&mut r, tag, vr, value);
    }
    r
}

fn dicomdir(path: &Path) {
    let mut d = InMemDicomObject::new_empty();
    put(&mut d, tags::FILE_SET_ID, VR::CS, "VWSAMPLE");
    let records = vec![
        record(
            "PATIENT",
            &[
                (tags::PATIENT_NAME, VR::PN, "Test^Hanako"),
                (tags::PATIENT_ID, VR::LO, "VW-0007"),
            ],
        ),
        record(
            "STUDY",
            &[
                (tags::STUDY_DATE, VR::DA, "20261112"),
                (tags::STUDY_DESCRIPTION, VR::LO, "Directory sample"),
                (
                    tags::STUDY_INSTANCE_UID,
                    VR::UI,
                    "1.2.826.0.1.3680043.9.7907.1",
                ),
            ],
        ),
        record(
            "SERIES",
            &[
                (tags::MODALITY, VR::CS, "CT"),
                (tags::SERIES_NUMBER, VR::IS, "1"),
                (
                    tags::SERIES_INSTANCE_UID,
                    VR::UI,
                    "1.2.826.0.1.3680043.9.7907.2",
                ),
            ],
        ),
        record(
            "IMAGE",
            &[
                (tags::REFERENCED_FILE_ID, VR::CS, "IMAGES\\IM0001"),
                (tags::REFERENCED_SOP_CLASS_UID_IN_FILE, VR::UI, CT_IMAGE),
                (
                    tags::REFERENCED_SOP_INSTANCE_UID_IN_FILE,
                    VR::UI,
                    "1.2.826.0.1.3680043.9.7907.3",
                ),
                (tags::INSTANCE_NUMBER, VR::IS, "1"),
            ],
        ),
    ];
    sequence(&mut d, tags::DIRECTORY_RECORD_SEQUENCE, records);
    write_object(
        d,
        MEDIA_STORAGE_DIRECTORY,
        "1.2.826.0.1.3680043.9.7907.9",
        path,
    );
}

fn code(value: &str, scheme: &str, meaning: &str) -> InMemDicomObject {
    let mut c = InMemDicomObject::new_empty();
    put(&mut c, tags::CODE_VALUE, VR::SH, value);
    put(&mut c, tags::CODING_SCHEME_DESIGNATOR, VR::SH, scheme);
    put(&mut c, tags::CODE_MEANING, VR::LO, meaning);
    c
}

fn text_report(path: &Path) {
    let uid = "1.2.826.0.1.3680043.9.7908";
    let mut sr = InMemDicomObject::new_empty();
    put(&mut sr, tags::SOP_CLASS_UID, VR::UI, BASIC_TEXT_SR);
    put(&mut sr, tags::SOP_INSTANCE_UID, VR::UI, &format!("{uid}.3"));
    put(&mut sr, tags::MODALITY, VR::CS, "SR");
    put(&mut sr, tags::PATIENT_NAME, VR::PN, "Test^Taro");
    put(&mut sr, tags::PATIENT_ID, VR::LO, "VW-0008");
    put(&mut sr, tags::STUDY_DATE, VR::DA, "20261112");
    put(&mut sr, tags::STUDY_DESCRIPTION, VR::LO, "Report sample");
    put(
        &mut sr,
        tags::STUDY_INSTANCE_UID,
        VR::UI,
        &format!("{uid}.1"),
    );
    put(
        &mut sr,
        tags::SERIES_INSTANCE_UID,
        VR::UI,
        &format!("{uid}.2"),
    );
    put(&mut sr, tags::SERIES_NUMBER, VR::IS, "8");
    put(&mut sr, tags::INSTANCE_NUMBER, VR::IS, "1");
    put(&mut sr, tags::VALUE_TYPE, VR::CS, "CONTAINER");
    put(&mut sr, tags::CONTINUITY_OF_CONTENT, VR::CS, "SEPARATE");
    put(&mut sr, tags::COMPLETION_FLAG, VR::CS, "COMPLETE");
    put(&mut sr, tags::VERIFICATION_FLAG, VR::CS, "UNVERIFIED");
    sequence(
        &mut sr,
        tags::CONCEPT_NAME_CODE_SEQUENCE,
        vec![code("121060", "DCM", "History")],
    );
    let mut finding = InMemDicomObject::new_empty();
    put(&mut finding, tags::RELATIONSHIP_TYPE, VR::CS, "CONTAINS");
    put(&mut finding, tags::VALUE_TYPE, VR::CS, "TEXT");
    sequence(
        &mut finding,
        tags::CONCEPT_NAME_CODE_SEQUENCE,
        vec![code("121071", "DCM", "Finding")],
    );
    put(
        &mut finding,
        tags::TEXT_VALUE,
        VR::UT,
        "No abnormality. This text is a made-up sample.",
    );
    let mut measured = InMemDicomObject::new_empty();
    put(&mut measured, tags::RELATIONSHIP_TYPE, VR::CS, "CONTAINS");
    put(&mut measured, tags::VALUE_TYPE, VR::CS, "NUM");
    sequence(
        &mut measured,
        tags::CONCEPT_NAME_CODE_SEQUENCE,
        vec![code("410668003", "SCT", "Length")],
    );
    let mut value = InMemDicomObject::new_empty();
    put(&mut value, tags::NUMERIC_VALUE, VR::DS, "12.5");
    sequence(
        &mut value,
        tags::MEASUREMENT_UNITS_CODE_SEQUENCE,
        vec![code("mm", "UCUM", "millimeter")],
    );
    sequence(&mut measured, tags::MEASURED_VALUE_SEQUENCE, vec![value]);
    sequence(&mut sr, tags::CONTENT_SEQUENCE, vec![finding, measured]);
    write_object(sr, BASIC_TEXT_SR, &format!("{uid}.3"), path);
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: make_tag_only_samples <out dir>"),
    );
    std::fs::create_dir_all(dir.join("07-dicomdir")).unwrap();
    let write = |name: &str, bytes: Vec<u8>| std::fs::write(dir.join(name), bytes).unwrap();

    write(
        "01-long-source-ae.dcm",
        add_meta_element(&ct(1, EXPLICIT_LE), 0x0016, b"AE", b"WORKSTATION-123456"),
    );

    let (meta, body) = split(&ct(2, EXPLICIT_LE));
    let mut meta = without(&meta, 0x0003);
    meta.extend(explicit(
        0x0003,
        b"UI",
        &uid("1.2.826.0.1.3680043.9.7902.3.ABC"),
    ));
    write("02-uid-with-letters.dcm", join(&meta, None, &body));

    let (meta, body) = split(&ct(3, EXPLICIT_LE));
    write("03-wrong-group-length.dcm", join(&meta, Some(30), &body));

    let (meta, body) = split(&ct(4, IMPLICIT_LE));
    write(
        "04-no-transfer-syntax.dcm",
        join(&without(&meta, 0x0010), None, &body),
    );

    let (meta, body) = split(&ct(5, IMPLICIT_LE));
    let mut meta = without(&meta, 0x0010);
    meta.extend(explicit(0x0010, b"UI", &uid(EXPLICIT_LE)));
    write("05-syntax-mismatch.dcm", join(&meta, None, &body));

    let (_, body) = split(&ct(6, IMPLICIT_LE));
    let meta = [
        implicit(0x0002, &uid(CT_IMAGE)),
        implicit(0x0003, &uid("1.2.826.0.1.3680043.9.7906.3")),
        implicit(0x0010, &uid(IMPLICIT_LE)),
    ]
    .concat();
    let mut out = vec![0u8; 128];
    out.extend(b"DICM");
    out.extend(implicit(0x0000, &(meta.len() as u32).to_le_bytes()));
    out.extend(meta);
    out.extend(body);
    write("06-implicit-meta.dcm", out);

    dicomdir(&dir.join("07-dicomdir").join("DICOMDIR"));
    text_report(&dir.join("08-text-report.dcm"));
    write("09-normal.dcm", ct(9, EXPLICIT_LE));

    let broken = add_meta_element(&ct(10, EXPLICIT_LE), 0x0016, b"AE", b"WORKSTATION-123456");
    // 本体の頭に、長さがファイルよりずっと長い要素を差し込む。
    let (meta, body) = split(&broken);
    let mut bad = vec![0x08, 0x00, 0x05, 0x00, b'U', b'T', 0, 0];
    bad.extend(0xF000_0000u32.to_le_bytes());
    let broken = join(&meta, None, &[bad, body].concat());
    write("10-broken-data-set.dcm", broken);

    println!("wrote samples to {}", dir.display());
}
