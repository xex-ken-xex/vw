//! タグの値が VR（値の型）の決まりに合っているかを調べ、合わないタグを [`Metadata::problems`] に残す。
//!
//! 値には患者の名前や生年月日が入りうるので、ログに出る文には値を入れない。タグ番号・名前・VR・
//! 長さと、どの決まりに外れたかだけを書く。dicom-rs のエラー文も値を含むことがあるため使わない。
//! ファイルの中の位置は書かない。dicom-rs は読んだ後のタグに位置を残さず、位置を得るには
//! ファイルを自分でもう一度読む必要があるため。

use dicom_core::VR;
use dicom_core::value::{PrimitiveValue, Value};
use dicom_object::InMemDicomObject;
use vw_core::Metadata;

use crate::charset::{self, Charset};

/// [`Metadata::problems`] のグループ。
const GROUP: &str = "DICOM";

/// 文字だけの VR の、1 つの値の長さの上限（PS3.5 6.2）。2 バイト文字が入りうる VR（LO・PN など）は
/// 文字の数え方が文字集合で変わるので調べない。DA と TM は、区切りのある古い書き方（ACR-NEMA）が
/// 上限より長くなるので、長さではなく形（[`is_date`]・[`is_time`]）で調べる。
fn max_len(vr: VR) -> Option<usize> {
    Some(match vr {
        VR::AE | VR::CS | VR::DS | VR::SH => 16,
        VR::AS => 4,
        VR::IS => 12,
        VR::UI => 64,
        VR::DT => 26,
        _ => return None,
    })
}

/// `obj` の全タグ（シーケンスの中も）を調べる。
pub fn push_problems(obj: &InMemDicomObject, charset: &Charset, depth: usize, m: &mut Metadata) {
    for e in obj.iter() {
        let tag = e.header().tag;
        match e.value() {
            Value::Sequence(seq) => {
                for item in seq.items() {
                    push_problems(item, &crate::charset_of(item, *charset), depth + 1, m);
                }
            }
            Value::PixelSequence(_) => {}
            Value::Primitive(p) => {
                let vr = e.vr();
                let Some(error) = problem(vr, p, charset) else {
                    continue;
                };
                let length = e
                    .header()
                    .len
                    .get()
                    .map_or("undefined length".to_owned(), |n| format!("{n} bytes"));
                let place = format!("{} {vr:?}, {length}", crate::listed_key(tag, depth));
                m.problem(GROUP, place, error);
            }
        }
    }
}

/// 値が決まりに外れていれば、その理由（値は入れない）。
fn problem(vr: VR, p: &PrimitiveValue, charset: &Charset) -> Option<String> {
    let parts: Vec<String> = match p {
        PrimitiveValue::Strs(parts) => parts.iter().map(|s| s.to_string()).collect(),
        PrimitiveValue::Str(s) => vec![s.clone()],
        _ => return None,
    };
    let count = parts.len();
    let which = |i: usize| {
        if count == 1 {
            "the value".to_owned()
        } else {
            format!("value {} of {count}", i + 1)
        }
    };
    let text = charset::value_text(p, charset);
    if text.contains('\u{FFFD}') {
        let n = text.matches('\u{FFFD}').count();
        return Some(format!(
            "{n} character(s) cannot be read in the file's character set"
        ));
    }
    for (i, part) in parts.iter().enumerate() {
        let v = part.trim_matches([' ', '\0']);
        if v.is_empty() {
            continue;
        }
        if let Some(max) = max_len(vr)
            && v.len() > max
        {
            return Some(format!(
                "{} is {} characters, longer than {vr:?} allows ({max})",
                which(i),
                v.len()
            ));
        }
        let ok = match vr {
            VR::IS => v.parse::<i64>().is_ok(),
            VR::DS => v.parse::<f64>().is_ok_and(f64::is_finite),
            VR::DA => is_date(v),
            VR::TM => is_time(v),
            VR::UI => v.bytes().all(|b| b.is_ascii_digit() || b == b'.'),
            _ => true,
        };
        if !ok {
            let rule = match vr {
                VR::IS => "an integer",
                VR::DS => "a decimal number",
                VR::DA => "a date (YYYYMMDD)",
                VR::TM => "a time (HHMMSS.FFFFFF)",
                _ => "a UID (digits and dots)",
            };
            return Some(format!("{} is not {rule}", which(i)));
        }
    }
    None
}

fn is_date(v: &str) -> bool {
    // 古い規格（ACR-NEMA）の YYYY.MM.DD も読める機器が多いので、外れとはしない。
    let digits: String = v.chars().filter(|&c| c != '.').collect();
    if digits.len() != 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let month: u32 = digits[4..6].parse().unwrap_or(0);
    let day: u32 = digits[6..8].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

fn is_time(v: &str) -> bool {
    // 古い規格の HH:MM:SS も同じ理由で認める。
    let v: String = v.chars().filter(|&c| c != ':').collect();
    let (whole, fraction) = v.split_once('.').unwrap_or((&v, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || !digits(fraction) || ![2, 4, 6].contains(&whole.len()) {
        return false;
    }
    let field = |at: usize| whole.get(at..at + 2).map_or(0, |s| s.parse().unwrap_or(99));
    field(0) < 24 && field(2) < 60 && field(4) < 61
}

#[cfg(test)]
mod tests {
    use dicom_dictionary_std::tags;

    use super::*;
    use crate::synth::Synth;
    use vw_core::DecoderPlugin;

    fn problems(synth: Synth) -> Vec<String> {
        crate::DicomPlugin
            .read_metadata(&synth.bytes())
            .unwrap()
            .problems
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn a_sound_file_has_no_problems() {
        let synth = Synth::ct(2, 2)
            .patient("Doe^Jane")
            .study("1.2.3", "Head", "20260101")
            .series("1.2.3.4", 1, "Axial")
            .instance("1.2.3.4.5", 7)
            .window(40.0, 400.0);
        assert_eq!(problems(synth), Vec::<String>::new());
    }

    #[test]
    fn a_date_that_does_not_exist_names_the_tag_without_its_value() {
        let synth = Synth::ct(2, 2).text(tags::PATIENT_BIRTH_DATE, VR::DA, "19991340");
        let found = problems(synth);
        assert_eq!(
            found,
            ["DICOM (0010,0030) PatientBirthDate DA, 8 bytes: the value is not a date (YYYYMMDD)"]
        );
        assert!(!found[0].contains("1999"));
    }

    #[test]
    fn a_number_that_is_not_one_says_which_of_the_values() {
        let synth = Synth::ct(2, 2).text(tags::PIXEL_SPACING, VR::DS, "0.5\\half");
        assert_eq!(
            problems(synth),
            ["DICOM (0028,0030) PixelSpacing DS, 8 bytes: value 2 of 2 is not a decimal number"]
        );
    }

    #[test]
    fn a_value_longer_than_its_vr_allows_is_reported() {
        let synth = Synth::ct(2, 2).text(tags::MODALITY, VR::CS, "ABCDEFGHIJKLMNOPQ");
        assert_eq!(
            problems(synth),
            [
                "DICOM (0008,0060) Modality CS, 18 bytes: the value is 17 characters, longer than CS allows (16)"
            ]
        );
    }

    #[test]
    fn dates_and_times_in_the_old_style_are_accepted() {
        assert!(is_date("1999.12.31"));
        assert!(is_time("12:30:59"));
        assert!(is_time("123059.123456"));
        assert!(!is_time("1260"));
    }
}
