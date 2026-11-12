//! テストと画面確認用に、小さな DICOM ファイルを組み立てる。
//!
//! 実際の検査画像は個人情報を含むのでリポジトリに置けない。テストに要る属性だけを持つ
//! ファイルをその場で作る。

use std::path::Path;

use dicom_core::value::{DataSetSequence, PixelFragmentSequence};
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::tags;
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};

/// CT Image Storage。
pub const CT_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.2";
/// X-Ray Angiographic Image Storage（シネのマルチフレーム）。
pub const XA_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.12.1";
/// Positron Emission Tomography Image Storage。
pub const PET_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.128";
/// Secondary Capture Image Storage（カラーの画面写しなど）。
pub const SC_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.7";

const EXPLICIT_VR_LE: &str = "1.2.840.10008.1.2.1";
pub const JPEG_LS_LOSSLESS: &str = "1.2.840.10008.1.2.4.80";
pub const JPEG_LS_NEAR_LOSSLESS: &str = "1.2.840.10008.1.2.4.81";
pub const JPEG_2000_LOSSLESS: &str = "1.2.840.10008.1.2.4.90";
pub const JPEG_2000: &str = "1.2.840.10008.1.2.4.91";

/// 組み立て中の DICOM ファイル。
pub struct Synth {
    sop_class: &'static str,
    transfer_syntax: &'static str,
    obj: InMemDicomObject,
}

impl Synth {
    /// `columns` x `rows` の、画素がまだない画像。
    pub fn new(sop_class: &'static str, columns: u16, rows: u16) -> Self {
        let mut s = Self {
            sop_class,
            transfer_syntax: EXPLICIT_VR_LE,
            obj: InMemDicomObject::new_empty(),
        };
        let modality = match sop_class {
            CT_IMAGE => "CT",
            XA_IMAGE => "XA",
            PET_IMAGE => "PT",
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

    /// 16bit のカラー画像。`data` は R G B の順に並べたサンプル、`stored` は Bits Stored。
    pub fn rgb16(columns: u16, rows: u16, data: &[u16], stored: u16) -> Self {
        let mut s = Self::new(SC_IMAGE, columns, rows);
        s.put_str(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "RGB");
        s.put_u16(tags::SAMPLES_PER_PIXEL, 3);
        s.put_u16(tags::PLANAR_CONFIGURATION, 0);
        s.pixels_u16(data, stored)
    }

    /// 非圧縮の YBR_FULL_422 の 8bit の画像。`data` は横の 2 画素ごとに Y1 Y2 Cb Cr。
    pub fn ybr_full_422(columns: u16, rows: u16, data: &[u8]) -> Self {
        let mut s = Self::new(SC_IMAGE, columns, rows);
        s.put_str(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "YBR_FULL_422");
        s.put_u16(tags::SAMPLES_PER_PIXEL, 3);
        s.put_u16(tags::PLANAR_CONFIGURATION, 0);
        s.bits(8, 8, false);
        s.pixel_data(VR::OB, data.to_vec())
    }

    /// PALETTE COLOR の 8bit の画像。色の表は [`Synth::lut`] で足す。
    pub fn palette(columns: u16, rows: u16, indices: &[u8]) -> Self {
        let mut s = Self::new(SC_IMAGE, columns, rows);
        s.put_str(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "PALETTE COLOR");
        s.bits(8, 8, false);
        s.pixel_data(VR::OB, indices.to_vec())
    }

    /// Palette Color LUT を 1 色分足す。`channel` は 0 = Red、1 = Green、2 = Blue。
    /// `entries` は記述子の項目数（65536 は 0 と書く）、`data` は OW の語（8bit の表なら
    /// 2 項目を 1 語に詰めたもの）。`segmented` なら Segmented の方の属性に書く。
    pub fn lut(
        mut self,
        channel: usize,
        entries: u32,
        first: u16,
        bits: u16,
        data: &[u16],
        segmented: bool,
    ) -> Self {
        let (descriptor, plain, segmented_tag) = [
            (
                tags::RED_PALETTE_COLOR_LOOKUP_TABLE_DESCRIPTOR,
                tags::RED_PALETTE_COLOR_LOOKUP_TABLE_DATA,
                tags::SEGMENTED_RED_PALETTE_COLOR_LOOKUP_TABLE_DATA,
            ),
            (
                tags::GREEN_PALETTE_COLOR_LOOKUP_TABLE_DESCRIPTOR,
                tags::GREEN_PALETTE_COLOR_LOOKUP_TABLE_DATA,
                tags::SEGMENTED_GREEN_PALETTE_COLOR_LOOKUP_TABLE_DATA,
            ),
            (
                tags::BLUE_PALETTE_COLOR_LOOKUP_TABLE_DESCRIPTOR,
                tags::BLUE_PALETTE_COLOR_LOOKUP_TABLE_DATA,
                tags::SEGMENTED_BLUE_PALETTE_COLOR_LOOKUP_TABLE_DATA,
            ),
        ][channel];
        let entries = if entries >= 65_536 { 0 } else { entries as u16 };
        self.obj.put(DataElement::new(
            descriptor,
            VR::US,
            PrimitiveValue::from([entries, first, bits]),
        ));
        self.obj.put(DataElement::new(
            if segmented { segmented_tag } else { plain },
            VR::OW,
            PrimitiveValue::U16(data.into()),
        ));
        self
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

    /// 画素データを、圧縮済みのフレームを並べたものに置き換える。Bits Allocated などの属性は
    /// 先に `pixels_u16` などで決めておく。
    pub fn encapsulated(mut self, transfer_syntax: &'static str, frames: Vec<Vec<u8>>) -> Self {
        self.transfer_syntax = transfer_syntax;
        if frames.len() > 1 {
            self.put_str(tags::NUMBER_OF_FRAMES, VR::IS, &frames.len().to_string());
        }
        self.obj.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PixelFragmentSequence::new_fragments(frames),
        ));
        self
    }

    /// [`Self::encapsulated`] の、各フレームを 2 つの断片に分け、Basic Offset Table を書く版。
    pub fn encapsulated_in_halves(
        mut self,
        transfer_syntax: &'static str,
        frames: Vec<Vec<u8>>,
    ) -> Self {
        self.transfer_syntax = transfer_syntax;
        self.put_str(tags::NUMBER_OF_FRAMES, VR::IS, &frames.len().to_string());
        let mut offsets = Vec::new();
        let mut fragments = Vec::new();
        let mut at = 0u32;
        for frame in frames {
            offsets.push(at);
            // 断片は偶数の長さにする。
            let half = (frame.len() / 2) & !1;
            let (a, b) = frame.split_at(half);
            for part in [a, b] {
                let mut part = part.to_vec();
                if part.len() % 2 == 1 {
                    part.push(0);
                }
                at += 8 + part.len() as u32;
                fragments.push(part);
            }
        }
        self.obj.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PixelFragmentSequence::new(offsets, fragments),
        ));
        self
    }

    /// ファイルの転送構文を変える（非圧縮どうし）。
    pub fn transfer_syntax(mut self, uid: &'static str) -> Self {
        self.transfer_syntax = uid;
        self
    }

    /// 圧縮したカラーは YBR_RCT（可逆）や YBR_ICT（非可逆）と書かれる。
    pub fn photometric(mut self, value: &str) -> Self {
        self.put_str(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, value);
        self
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

    /// VOI LUT Function（LINEAR / LINEAR_EXACT / SIGMOID）。
    pub fn voi_function(self, value: &str) -> Self {
        self.text(tags::VOILUT_FUNCTION, VR::CS, value)
    }

    /// 任意の文字の属性。
    pub fn text(mut self, tag: Tag, vr: VR, value: &str) -> Self {
        self.put_str(tag, vr, value);
        self
    }

    /// 符号付き 16bit のフレームを並べたマルチフレーム。
    pub fn frames_i16(self, frames: &[&[i16]]) -> Self {
        let n = frames.len().to_string();
        self.pixels_i16(&frames.concat())
            .text(tags::NUMBER_OF_FRAMES, VR::IS, &n)
    }

    /// Enhanced 形式の、フレームごとの Rescale とウィンドウ。組は (slope, intercept, center, width)。
    pub fn per_frame_values(mut self, frames: &[(f64, f64, f64, f64)]) -> Self {
        let ds =
            |tag: Tag, v: f64| DataElement::new(tag, VR::DS, PrimitiveValue::from(v.to_string()));
        let items = frames
            .iter()
            .map(|&(slope, intercept, center, width)| {
                let transform = InMemDicomObject::from_element_iter([
                    ds(tags::RESCALE_SLOPE, slope),
                    ds(tags::RESCALE_INTERCEPT, intercept),
                    DataElement::new(tags::RESCALE_TYPE, VR::LO, PrimitiveValue::from("HU")),
                ]);
                let voi = InMemDicomObject::from_element_iter([
                    ds(tags::WINDOW_CENTER, center),
                    ds(tags::WINDOW_WIDTH, width),
                ]);
                InMemDicomObject::from_element_iter([
                    DataElement::new(
                        tags::PIXEL_VALUE_TRANSFORMATION_SEQUENCE,
                        VR::SQ,
                        DataSetSequence::from(vec![transform]),
                    ),
                    DataElement::new(
                        tags::FRAME_VOILUT_SEQUENCE,
                        VR::SQ,
                        DataSetSequence::from(vec![voi]),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        self.obj.put(DataElement::new(
            tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(items),
        ));
        self
    }

    /// PET の投与の情報（Radiopharmaceutical Information Sequence の 1 項目）。
    /// 値が `None` のものは書かない（足りないファイルを作るため）。
    pub fn radiopharmaceutical(
        mut self,
        dose_bq: Option<f64>,
        half_life_s: Option<f64>,
        start_time: Option<&str>,
    ) -> Self {
        let mut item = InMemDicomObject::new_empty();
        let mut put = |tag: Tag, vr: VR, value: String| {
            item.put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
        };
        if let Some(dose) = dose_bq {
            put(tags::RADIONUCLIDE_TOTAL_DOSE, VR::DS, dose.to_string());
        }
        if let Some(half_life) = half_life_s {
            put(tags::RADIONUCLIDE_HALF_LIFE, VR::DS, half_life.to_string());
        }
        if let Some(time) = start_time {
            put(
                tags::RADIOPHARMACEUTICAL_START_TIME,
                VR::TM,
                time.to_owned(),
            );
        }
        self.obj.put(DataElement::new(
            tags::RADIOPHARMACEUTICAL_INFORMATION_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![item]),
        ));
        self
    }

    /// 中身が空の項目を 1 つ持つシーケンス。あるかどうかだけを見る属性（Modality LUT Sequence など）用。
    pub fn empty_sequence(mut self, tag: Tag) -> Self {
        self.obj.put(DataElement::new(
            tag,
            VR::SQ,
            DataSetSequence::from(vec![InMemDicomObject::new_empty()]),
        ));
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
            .transfer_syntax(self.transfer_syntax)
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
/// `file`（[`Synth::bytes`] の形）のメタ情報の終わりに、2 バイトの長さを持つ VR の要素
/// (0002,`element`) を足し、グループ長を合わせる。dicom-rs は規格に外れた値を書けないので、
/// 外れたメタ情報を持つファイルはこれで作る。値の長さは偶数に揃えない（揃えるかは呼ぶ側が決める）。
pub fn add_meta_element(file: &[u8], element: u16, vr: &[u8; 2], value: &[u8]) -> Vec<u8> {
    const GROUP_LENGTH_VALUE: usize = 128 + 4 + 8;
    let length = u32::from_le_bytes(
        file[GROUP_LENGTH_VALUE..GROUP_LENGTH_VALUE + 4]
            .try_into()
            .expect("4 bytes"),
    );
    let end = GROUP_LENGTH_VALUE + 4 + length as usize;
    let mut element_bytes = Vec::new();
    element_bytes.extend(0x0002u16.to_le_bytes());
    element_bytes.extend(element.to_le_bytes());
    element_bytes.extend(vr);
    element_bytes.extend((value.len() as u16).to_le_bytes());
    element_bytes.extend(value);
    let mut out = file[..end].to_vec();
    let new_length = length + element_bytes.len() as u32;
    out[GROUP_LENGTH_VALUE..GROUP_LENGTH_VALUE + 4].copy_from_slice(&new_length.to_le_bytes());
    out.extend(element_bytes);
    out.extend(&file[end..]);
    out
}
