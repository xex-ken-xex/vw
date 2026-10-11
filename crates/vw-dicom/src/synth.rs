//! テストと画面確認用に、小さな DICOM ファイルを組み立てる。
//!
//! 実際の検査画像は個人情報を含むのでリポジトリに置けない。テストに要る属性だけを持つ
//! ファイルをその場で作る。

use std::path::Path;

use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::tags;
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};

/// CT Image Storage。
pub const CT_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.2";
/// X-Ray Angiographic Image Storage（シネのマルチフレーム）。
pub const XA_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.12.1";
/// Secondary Capture Image Storage（カラーの画面写しなど）。
pub const SC_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.7";

const EXPLICIT_VR_LE: &str = "1.2.840.10008.1.2.1";

/// 組み立て中の DICOM ファイル。
pub struct Synth {
    sop_class: &'static str,
    obj: InMemDicomObject,
}

impl Synth {
    /// `columns` x `rows` の、画素がまだない画像。
    pub fn new(sop_class: &'static str, columns: u16, rows: u16) -> Self {
        let mut s = Self {
            sop_class,
            obj: InMemDicomObject::new_empty(),
        };
        let modality = match sop_class {
            CT_IMAGE => "CT",
            XA_IMAGE => "XA",
            _ => "OT",
        };
        s.put_str(tags::SOP_CLASS_UID, VR::UI, sop_class);
        s.put_str(tags::SOP_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.9.1");
        s.put_str(tags::STUDY_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.9.2");
        s.put_str(tags::SERIES_INSTANCE_UID, VR::UI, "1.2.826.0.1.3680043.9.3");
        s.put_str(tags::MODALITY, VR::CS, modality);
        s.put_str(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2");
        s.put_u16(tags::ROWS, rows);
        s.put_u16(tags::COLUMNS, columns);
        s.put_u16(tags::SAMPLES_PER_PIXEL, 1);
        s
    }

    /// 画素がすべて 0 の、符号付き 16bit の CT 画像。
    pub fn ct(columns: u16, rows: u16) -> Self {
        let n = columns as usize * rows as usize;
        Self::new(CT_IMAGE, columns, rows).pixels_i16(&vec![0; n])
    }

    /// 8bit のカラー画像。`planar` なら R の面、G の面、B の面の順に並んだ `data`。
    pub fn rgb(columns: u16, rows: u16, data: &[u8], planar: bool) -> Self {
        let mut s = Self::new(SC_IMAGE, columns, rows);
        s.put_str(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "RGB");
        s.put_u16(tags::SAMPLES_PER_PIXEL, 3);
        s.put_u16(tags::PLANAR_CONFIGURATION, planar as u16);
        s.bits(8, 8, false);
        s.pixel_data(VR::OB, data.to_vec())
    }

    pub fn pixels_i16(mut self, values: &[i16]) -> Self {
        self.bits(16, 16, true);
        let bytes = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.pixel_data(VR::OW, bytes)
    }

    /// 符号なし 16bit の画素。`stored` は Bits Stored。
    pub fn pixels_u16(mut self, values: &[u16], stored: u16) -> Self {
        self.bits(16, stored, false);
        let bytes = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.pixel_data(VR::OW, bytes)
    }

    /// 8bit のフレームを並べたマルチフレーム。
    pub fn frames_u8(mut self, frames: &[&[u8]]) -> Self {
        self.bits(8, 8, false);
        self.put_str(tags::NUMBER_OF_FRAMES, VR::IS, &frames.len().to_string());
        self.pixel_data(VR::OB, frames.concat())
    }

    pub fn signed(mut self) -> Self {
        self.put_u16(tags::PIXEL_REPRESENTATION, 1);
        self
    }

    pub fn monochrome1(mut self) -> Self {
        self.put_str(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME1");
        self
    }

    pub fn rescale(mut self, slope: f64, intercept: f64) -> Self {
        self.put_str(tags::RESCALE_SLOPE, VR::DS, &slope.to_string());
        self.put_str(tags::RESCALE_INTERCEPT, VR::DS, &intercept.to_string());
        self
    }

    pub fn window(mut self, center: f64, width: f64) -> Self {
        self.put_str(tags::WINDOW_CENTER, VR::DS, &center.to_string());
        self.put_str(tags::WINDOW_WIDTH, VR::DS, &width.to_string());
        self
    }

    pub fn patient(mut self, name: &str) -> Self {
        self.put_str(tags::PATIENT_NAME, VR::PN, name);
        self
    }

    pub fn study(mut self, uid: &str, description: &str, date: &str) -> Self {
        self.put_str(tags::STUDY_INSTANCE_UID, VR::UI, uid);
        self.put_str(tags::STUDY_DESCRIPTION, VR::LO, description);
        self.put_str(tags::STUDY_DATE, VR::DA, date);
        self
    }

    pub fn series(mut self, uid: &str, number: i32, description: &str) -> Self {
        self.put_str(tags::SERIES_INSTANCE_UID, VR::UI, uid);
        self.put_str(tags::SERIES_NUMBER, VR::IS, &number.to_string());
        self.put_str(tags::SERIES_DESCRIPTION, VR::LO, description);
        self
    }

    pub fn instance(mut self, uid: &str, number: i32) -> Self {
        self.put_str(tags::SOP_INSTANCE_UID, VR::UI, uid);
        self.put_str(tags::INSTANCE_NUMBER, VR::IS, &number.to_string());
        self
    }

    /// シネのフレーム間隔（ミリ秒）。
    pub fn frame_time(mut self, ms: f64) -> Self {
        self.put_str(tags::FRAME_TIME, VR::DS, &ms.to_string());
        self
    }

    /// プリアンブルと DICM の印から始まるファイルの中身。
    pub fn bytes(&self) -> Vec<u8> {
        let uid = self
            .obj
            .get(tags::SOP_INSTANCE_UID)
            .and_then(|e| e.to_str().ok())
            .map(|s| s.into_owned())
            .unwrap_or_default();
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax(EXPLICIT_VR_LE)
            .media_storage_sop_class_uid(self.sop_class)
            .media_storage_sop_instance_uid(uid);
        let file = self
            .obj
            .clone()
            .with_meta(meta)
            .expect("synthetic meta table is complete");
        let mut out = Vec::new();
        file.write_all(&mut out)
            .expect("writing to memory cannot fail");
        out
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.bytes())
    }

    fn bits(&mut self, allocated: u16, stored: u16, signed: bool) {
        self.put_u16(tags::BITS_ALLOCATED, allocated);
        self.put_u16(tags::BITS_STORED, stored);
        self.put_u16(tags::HIGH_BIT, stored - 1);
        self.put_u16(tags::PIXEL_REPRESENTATION, signed as u16);
    }

    fn pixel_data(mut self, vr: VR, bytes: Vec<u8>) -> Self {
        self.obj.put(DataElement::new(
            tags::PIXEL_DATA,
            vr,
            PrimitiveValue::from(bytes),
        ));
        self
    }

    fn put_str(&mut self, tag: Tag, vr: VR, value: &str) {
        self.obj
            .put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
    }

    fn put_u16(&mut self, tag: Tag, value: u16) {
        self.obj
            .put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
    }
}
