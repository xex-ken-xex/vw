//! 格納された値が表示の色になるまでに通る段と、それぞれに使うタグの値。
//!
//! 画素の変換の図に出すための情報で、デコードはしない（ヘッダだけを読む）。

pub use dicom_core::Tag;
pub use dicom_dictionary_std::tags;
use dicom_object::{DefaultDicomObject, InMemDicomObject};

use crate::{int, open, text};
use vw_core::DecodeError;

/// 値の出どころ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// データセットの最上位にあるタグ。
    File,
    /// Enhanced 形式の Functional Group（フレームごと、または全フレーム共通）。
    FunctionalGroup,
    /// タグがないので規格の既定値を使った。
    Default,
}

/// 1 つのタグの値と出どころ。
#[derive(Debug, Clone, PartialEq)]
pub struct Value<T> {
    pub tag: Tag,
    pub value: T,
    pub origin: Origin,
}

/// 格納値の段。
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub bits_allocated: u16,
    pub bits_stored: u16,
    pub high_bit: u16,
    pub signed: bool,
    pub samples_per_pixel: u16,
    pub photometric: String,
    pub planar_configuration: Option<u16>,
}

/// Modality LUT の段（格納値 → 物理量）。
#[derive(Debug, Clone, PartialEq)]
pub struct Modality {
    pub slope: Value<f64>,
    pub intercept: Value<f64>,
    pub rescale_type: Option<String>,
    /// Modality LUT Sequence があるか。あれば規格上はスロープより優先されるが、vw はまだ当てない。
    pub lut_sequence: bool,
    pub modality: Option<String>,
    pub units: Option<String>,
}

/// VOI LUT の段（物理量 → 表示の明るさ）。
#[derive(Debug, Clone, PartialEq)]
pub struct Voi {
    /// ファイルにある幅・中心の組（最初の組を使う）。ないときは `None`。
    pub center: Option<Value<f64>>,
    pub width: Option<Value<f64>>,
    /// 組が複数あるときの説明（Window Center & Width Explanation）。
    pub explanation: Option<String>,
    /// 中心・幅の組の数。
    pub windows: usize,
    pub function: Value<String>,
    /// VOI LUT Sequence があるか。vw はまだ当てない。
    pub lut_sequence: bool,
}

/// Presentation LUT の段（明るさ → 白黒の向き）。
#[derive(Debug, Clone, PartialEq)]
pub struct Presentation {
    pub shape: Option<String>,
    /// MONOCHROME1（値が大きいほど黒い）なので反転する。
    pub invert: bool,
}

/// モノクロ画像の段。
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub stored: Stored,
    pub modality: Modality,
    pub voi: Voi,
    pub presentation: Presentation,
    /// PET の Bq/ml の画像なら、SUVbw に換算するのに使う値。表示の道からの枝として出す。
    pub suv: Option<crate::suv::Inputs>,
}

/// カラー画像の段。
#[derive(Debug, Clone, PartialEq)]
pub struct Color {
    pub stored: Stored,
    /// 色空間の変換。YBR → RGB、パレットなど。
    pub conversion: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pipeline {
    Gray(Box<Gray>),
    Color(Color),
}

/// `data`（DICOM ファイル）の `frame` 番目のフレームが通る段を読む。
pub fn read(data: &[u8], frame: usize) -> Result<Pipeline, DecodeError> {
    let obj = open(data, true)?;
    let stored = stored(&obj);
    if stored.samples_per_pixel != 1 || stored.photometric == "PALETTE COLOR" {
        let conversion = match stored.photometric.as_str() {
            "RGB" => "none",
            "PALETTE COLOR" => "palette",
            p if p.starts_with("YBR") => "ybr_to_rgb",
            _ => "unknown",
        };
        return Ok(Pipeline::Color(Color { stored, conversion }));
    }
    let groups = Groups::of(&obj, frame);
    let invert = stored.photometric == "MONOCHROME1";
    let modality = modality(&obj, &groups);
    let suv = (modality.modality.as_deref() == Some("PT")
        && modality.units.as_deref() == Some("BQML"))
    .then(|| crate::suv::read(&obj));
    Ok(Pipeline::Gray(Box::new(Gray {
        suv,
        modality,
        voi: voi(&obj, &groups),
        presentation: Presentation {
            shape: text(&obj, tags::PRESENTATION_LUT_SHAPE),
            invert,
        },
        stored,
    })))
}

fn stored(obj: &DefaultDicomObject) -> Stored {
    let u = |tag| int(obj, tag).map(|v| v as u16);
    let bits_allocated = u(tags::BITS_ALLOCATED).unwrap_or(16);
    let bits_stored = u(tags::BITS_STORED).unwrap_or(bits_allocated);
    Stored {
        bits_allocated,
        bits_stored,
        high_bit: u(tags::HIGH_BIT).unwrap_or(bits_stored.saturating_sub(1)),
        signed: u(tags::PIXEL_REPRESENTATION) == Some(1),
        samples_per_pixel: u(tags::SAMPLES_PER_PIXEL).unwrap_or(1),
        photometric: text(obj, tags::PHOTOMETRIC_INTERPRETATION).unwrap_or_default(),
        planar_configuration: u(tags::PLANAR_CONFIGURATION),
    }
}

/// Enhanced 形式で、そのフレームに効く Functional Group。フレームごとの組を全フレーム共通の
/// 組より先に見る（規格ではどちらか一方にしか置けないが、両方を見ても害はない）。
struct Groups<'a> {
    items: Vec<&'a InMemDicomObject>,
}

impl<'a> Groups<'a> {
    fn of(obj: &'a DefaultDicomObject, frame: usize) -> Self {
        let mut items = Vec::new();
        let item = |tag: Tag, index: usize| {
            obj.get(tag)
                .and_then(|e| e.items())
                .and_then(|items| items.get(index))
        };
        items.extend(item(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE, frame));
        items.extend(item(tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE, 0));
        Self { items }
    }

    /// `macro_tag` の最初の項目の中の `tag`。
    fn get(&self, macro_tag: Tag, tag: Tag) -> Option<&'a dicom_object::mem::InMemElement> {
        self.items
            .iter()
            .find_map(|group| group.get(macro_tag)?.items()?.first()?.get(tag))
    }

    fn float(&self, macro_tag: Tag, tag: Tag) -> Option<f64> {
        self.get(macro_tag, tag)?.to_float64().ok()
    }

    fn text(&self, macro_tag: Tag, tag: Tag) -> Option<String> {
        let s = self.get(macro_tag, tag)?.to_str().ok()?;
        let s = s.trim_matches([' ', '\0']);
        (!s.is_empty()).then(|| s.to_owned())
    }
}

/// タグの値を、Functional Group、最上位、既定値の順に探す。
fn lookup(
    obj: &DefaultDicomObject,
    groups: &Groups<'_>,
    macro_tag: Tag,
    tag: Tag,
    default: f64,
) -> Value<f64> {
    if let Some(value) = groups.float(macro_tag, tag) {
        return Value {
            tag,
            value,
            origin: Origin::FunctionalGroup,
        };
    }
    match obj.get(tag).and_then(|e| e.to_float64().ok()) {
        Some(value) => Value {
            tag,
            value,
            origin: Origin::File,
        },
        None => Value {
            tag,
            value: default,
            origin: Origin::Default,
        },
    }
}

fn modality(obj: &DefaultDicomObject, groups: &Groups<'_>) -> Modality {
    let m = tags::PIXEL_VALUE_TRANSFORMATION_SEQUENCE;
    Modality {
        slope: lookup(obj, groups, m, tags::RESCALE_SLOPE, 1.0),
        intercept: lookup(obj, groups, m, tags::RESCALE_INTERCEPT, 0.0),
        rescale_type: groups
            .text(m, tags::RESCALE_TYPE)
            .or_else(|| text(obj, tags::RESCALE_TYPE)),
        lut_sequence: obj.get(tags::MODALITY_LUT_SEQUENCE).is_some(),
        modality: text(obj, tags::MODALITY),
        units: text(obj, tags::UNITS),
    }
}

fn voi(obj: &DefaultDicomObject, groups: &Groups<'_>) -> Voi {
    let m = tags::FRAME_VOILUT_SEQUENCE;
    // 中心と幅は複数の組を持てる（\ 区切り）。最初の組を使うのはデコーダと同じ。
    let first = |tag: Tag| -> Option<Value<f64>> {
        if let Some(value) = groups.float(m, tag) {
            return Some(Value {
                tag,
                value,
                origin: Origin::FunctionalGroup,
            });
        }
        let values = obj.get(tag)?.to_multi_float64().ok()?;
        Some(Value {
            tag,
            value: *values.first()?,
            origin: Origin::File,
        })
    };
    let windows = obj
        .get(tags::WINDOW_WIDTH)
        .and_then(|e| e.to_multi_float64().ok())
        .map_or(0, |v| v.len());
    let function = match groups
        .text(m, tags::VOILUT_FUNCTION)
        .map(|v| (v, Origin::FunctionalGroup))
        .or_else(|| text(obj, tags::VOILUT_FUNCTION).map(|v| (v, Origin::File)))
    {
        Some((value, origin)) => Value {
            tag: tags::VOILUT_FUNCTION,
            value,
            origin,
        },
        None => Value {
            tag: tags::VOILUT_FUNCTION,
            value: "LINEAR".into(),
            origin: Origin::Default,
        },
    };
    let width = first(tags::WINDOW_WIDTH).filter(|w| w.value > 0.0);
    Voi {
        center: width.as_ref().and(first(tags::WINDOW_CENTER)),
        width,
        explanation: text(obj, tags::WINDOW_CENTER_WIDTH_EXPLANATION),
        windows: windows.max(usize::from(groups.float(m, tags::WINDOW_WIDTH).is_some())),
        function,
        lut_sequence: obj.get(tags::VOILUT_SEQUENCE).is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Synth;

    fn gray(bytes: &[u8]) -> Gray {
        match read(bytes, 0).unwrap() {
            Pipeline::Gray(g) => *g,
            other => panic!("not gray: {other:?}"),
        }
    }

    #[test]
    fn pet_in_bq_per_ml_carries_what_suv_needs() {
        use dicom_core::VR;
        let bytes = Synth::new(crate::synth::PET_IMAGE, 1, 1)
            .pixels_u16(&[1], 16)
            .text(tags::UNITS, VR::CS, "BQML")
            .text(tags::PATIENT_WEIGHT, VR::DS, "70")
            .bytes();
        assert_eq!(gray(&bytes).suv.and_then(|s| s.weight_kg), Some(70.0));
        assert_eq!(gray(&Synth::ct(1, 1).bytes()).suv, None);
    }

    #[test]
    fn ct_rescale_and_window_come_from_the_file() {
        let bytes = Synth::ct(2, 2)
            .rescale(1.0, -1024.0)
            .window(40.0, 400.0)
            .bytes();
        let g = gray(&bytes);
        assert_eq!(g.modality.slope.value, 1.0);
        assert_eq!(g.modality.intercept.value, -1024.0);
        assert_eq!(g.modality.intercept.origin, Origin::File);
        assert_eq!(g.voi.center.as_ref().unwrap().value, 40.0);
        assert_eq!(g.voi.width.as_ref().unwrap().value, 400.0);
        assert_eq!(g.voi.function.origin, Origin::Default);
        assert!(!g.presentation.invert);
    }

    #[test]
    fn missing_rescale_uses_the_identity_as_default() {
        let bytes = Synth::ct(2, 2).bytes();
        let g = gray(&bytes);
        assert_eq!(g.modality.slope.origin, Origin::Default);
        assert_eq!(g.modality.intercept.value, 0.0);
        assert_eq!(g.voi.width, None);
        assert_eq!(g.voi.center, None);
    }

    #[test]
    fn monochrome1_is_inverted_at_presentation() {
        let bytes = Synth::ct(2, 2).monochrome1().bytes();
        assert!(gray(&bytes).presentation.invert);
    }

    #[test]
    fn per_frame_values_win_over_the_top_level_ones() {
        let bytes = Synth::ct(1, 1)
            .rescale(1.0, -1024.0)
            .window(40.0, 400.0)
            .per_frame_values(&[(1.0, -1000.0, 50.0, 350.0), (2.0, -2048.0, 500.0, 2000.0)])
            .bytes();
        let Pipeline::Gray(g) = read(&bytes, 1).unwrap() else {
            panic!("not gray")
        };
        assert_eq!(g.modality.slope.value, 2.0);
        assert_eq!(g.modality.intercept.origin, Origin::FunctionalGroup);
        assert_eq!(g.modality.rescale_type.as_deref(), Some("HU"));
        assert_eq!(g.voi.center.as_ref().unwrap().value, 500.0);
        assert_eq!(g.voi.width.unwrap().origin, Origin::FunctionalGroup);
    }

    #[test]
    fn lut_sequences_and_the_voi_function_are_reported() {
        let bytes = Synth::ct(1, 1)
            .text(tags::VOILUT_FUNCTION, dicom_core::VR::CS, "SIGMOID")
            .empty_sequence(tags::MODALITY_LUT_SEQUENCE)
            .empty_sequence(tags::VOILUT_SEQUENCE)
            .bytes();
        let g = gray(&bytes);
        assert_eq!(g.voi.function.value, "SIGMOID");
        assert_eq!(g.voi.function.origin, Origin::File);
        assert!(g.modality.lut_sequence);
        assert!(g.voi.lut_sequence);
    }

    #[test]
    fn rgb_is_a_color_pipeline() {
        let bytes = Synth::rgb(1, 1, &[1, 2, 3], false).bytes();
        let Pipeline::Color(c) = read(&bytes, 0).unwrap() else {
            panic!("not color")
        };
        assert_eq!(c.conversion, "none");
        assert_eq!(c.stored.samples_per_pixel, 3);
    }
}
