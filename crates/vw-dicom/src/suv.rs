//! PET の SUV（体重で割った標準化取り込み値、SUVbw）の換算。
//!
//! 画素の値（Units が BQML なら Bq/ml）に [`factor`] を掛けると SUVbw になる。
//! 減衰の補正は QIBA の SUV の手順に合わせる。

use dicom_core::Tag;
use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;

/// 換算に使う値。ファイルに無いものは `None`。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inputs {
    pub weight_kg: Option<f64>,
    pub dose_bq: Option<f64>,
    pub half_life_s: Option<f64>,
    /// Decay Correction（START / ADMIN / NONE）。
    pub decay_correction: Option<String>,
    /// 投与の時刻。Radiopharmaceutical Start DateTime、無ければ Start Time と検査の日付。
    pub injection: Option<When>,
    pub series: Option<When>,
    pub acquisition: Option<When>,
}

/// 時刻。日付が分からないものは、その日の 0 時からの秒だけを持つ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct When {
    pub seconds: f64,
    pub dated: bool,
}

/// 換算できないときの、足りない項目（メタデータ欄に出す英語の名前）。
pub type Missing = Vec<&'static str>;

/// Bq/ml に掛けると SUVbw（g/ml）になる値。
pub fn factor(i: &Inputs) -> Result<f64, Missing> {
    let mut missing = Missing::new();
    let positive = |v: Option<f64>| v.filter(|v| v.is_finite() && *v > 0.0);
    let weight = positive(i.weight_kg);
    let dose = positive(i.dose_bq);
    let half_life = positive(i.half_life_s);
    if weight.is_none() {
        missing.push("Patient weight");
    }
    if dose.is_none() {
        missing.push("Injected dose");
    }
    if half_life.is_none() {
        missing.push("Half-life");
    }
    let decay = elapsed_seconds(i);
    if let Err(m) = &decay {
        missing.push(m);
    }
    let (Some(weight), Some(dose), Some(half_life), Ok(elapsed)) = (weight, dose, half_life, decay)
    else {
        return Err(missing);
    };
    let dose_now = dose * (-elapsed * std::f64::consts::LN_2 / half_life).exp();
    Ok(weight * 1000.0 / dose_now)
}

/// 投与から、画素の値が補正されている時刻までの秒。
pub fn elapsed_seconds(i: &Inputs) -> Result<f64, &'static str> {
    let reference = match i.decay_correction.as_deref().map(str::trim) {
        // 投与の時刻に戻して補正済みなので、減衰させない。
        Some("ADMIN") => return Ok(0.0),
        // 補正されていない値なので、この画像を撮った時刻まで減衰させる。
        Some("NONE") => i.acquisition.or(i.series),
        // Decay Correction が無いファイルも START として扱う。PET のほとんどが START で、
        // 無いからといって SUV を出さないと、古い装置の画像で何も見えなくなるため。
        _ => match (i.series, i.acquisition) {
            // 装置によっては、後から作り直したシリーズの Series Time が撮った時刻より後になる。
            // QIBA の手順に合わせて、そのときは撮った時刻を使う。
            (Some(s), Some(a)) if s.dated == a.dated && s.seconds > a.seconds => Some(a),
            (s, a) => s.or(a),
        },
    };
    let (Some(start), Some(reference)) = (i.injection, reference) else {
        return Err("Injection time");
    };
    let mut elapsed = reference.seconds - start.seconds;
    if !(start.dated && reference.dated) && elapsed < 0.0 {
        // 日付が分からないときは、日をまたいだとみなす。
        elapsed += 86_400.0;
    }
    if elapsed < 0.0 {
        return Err("Injection time");
    }
    Ok(elapsed)
}

/// ファイルから換算に使う値を読む。
pub fn read(obj: &InMemDicomObject) -> Inputs {
    let radiopharmaceutical = obj
        .get(tags::RADIOPHARMACEUTICAL_INFORMATION_SEQUENCE)
        .and_then(|e| e.items())
        .and_then(|items| items.first());
    let in_item = |tag: Tag| radiopharmaceutical.and_then(|item| text(item, tag));
    let float_in_item = |tag: Tag| in_item(tag).and_then(|s| s.parse::<f64>().ok());
    let series_date = text(obj, tags::SERIES_DATE).or_else(|| text(obj, tags::STUDY_DATE));
    let injection = in_item(tags::RADIOPHARMACEUTICAL_START_DATE_TIME)
        .and_then(|dt| date_time(&dt))
        .or_else(|| {
            let time = in_item(tags::RADIOPHARMACEUTICAL_START_TIME)?;
            at(series_date.as_deref(), &time)
        });
    Inputs {
        weight_kg: text(obj, tags::PATIENT_WEIGHT).and_then(|s| s.parse().ok()),
        dose_bq: float_in_item(tags::RADIONUCLIDE_TOTAL_DOSE),
        half_life_s: float_in_item(tags::RADIONUCLIDE_HALF_LIFE),
        decay_correction: text(obj, tags::DECAY_CORRECTION),
        injection,
        series: text(obj, tags::SERIES_TIME).and_then(|t| at(series_date.as_deref(), &t)),
        acquisition: text(obj, tags::ACQUISITION_TIME).and_then(|t| {
            let date = text(obj, tags::ACQUISITION_DATE).or_else(|| series_date.clone());
            at(date.as_deref(), &t)
        }),
    }
}

fn text(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    let s = obj.get(tag)?.to_str().ok()?;
    let s = s.trim_matches([' ', '\0']);
    (!s.is_empty()).then(|| s.to_owned())
}

/// 日付（DA、無くてもよい）と時刻（TM）。
fn at(date: Option<&str>, time: &str) -> Option<When> {
    let seconds = time_of_day(time)?;
    Some(match date.and_then(day_number) {
        Some(day) => When {
            seconds: day as f64 * 86_400.0 + seconds,
            dated: true,
        },
        None => When {
            seconds,
            dated: false,
        },
    })
}

/// DT（`YYYYMMDDHHMMSS.FFFFFF&ZZXX`）。時差の部分は、比べる相手の時刻にも無いので読まない。
fn date_time(dt: &str) -> Option<When> {
    let dt = dt.split(['+', '-']).next()?;
    if dt.len() < 8 {
        return None;
    }
    let (date, time) = dt.split_at(8);
    at(Some(date), if time.is_empty() { "000000" } else { time })
}

/// TM（`HHMMSS.FFFFFF`。古い書き方の `HH:MM:SS` も）を、その日の 0 時からの秒にする。
fn time_of_day(tm: &str) -> Option<f64> {
    let tm: String = tm.chars().filter(|c| *c != ':').collect();
    let (whole, fraction) = tm.split_once('.').unwrap_or((&tm, ""));
    if whole.is_empty() || whole.len() % 2 != 0 || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let part = |i: usize| {
        whole
            .get(i..i + 2)
            .map_or(Some(0), |s| s.parse::<u32>().ok())
    };
    let (h, m, s) = (part(0)?, part(2)?, part(4)?);
    let fraction = if fraction.is_empty() {
        0.0
    } else {
        format!("0.{fraction}").parse::<f64>().ok()?
    };
    Some(f64::from(h * 3600 + m * 60 + s) + fraction)
}

/// DA（`YYYYMMDD`）を、ある日からの日数にする。差だけを使うので、起点はどこでもよい。
fn day_number(da: &str) -> Option<i64> {
    let da: String = da.chars().filter(|c| *c != '.').collect();
    if da.len() != 8 {
        return None;
    }
    let y: i64 = da[0..4].parse().ok()?;
    let m: i64 = da[4..6].parse().ok()?;
    let d: i64 = da[6..8].parse().ok()?;
    // 3 月始まりの暦にして閏日を年の最後に回す（Howard Hinnant の days_from_civil）。
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe)
}

#[cfg(test)]
mod tests {
    use super::*;

    const F18_HALF_LIFE: f64 = 6586.2;

    fn when(h: u32, m: u32) -> When {
        at(Some("20261018"), &format!("{h:02}{m:02}00")).unwrap()
    }

    fn inputs() -> Inputs {
        Inputs {
            weight_kg: Some(70.0),
            dose_bq: Some(370e6),
            half_life_s: Some(F18_HALF_LIFE),
            decay_correction: Some("START".into()),
            injection: Some(when(9, 0)),
            series: Some(when(10, 0)),
            acquisition: Some(when(10, 5)),
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= b.abs() * 1e-9
    }

    #[test]
    fn suv_divides_by_the_dose_decayed_to_the_series_time() {
        let decayed = 370e6 * 0.5f64.powf(3600.0 / F18_HALF_LIFE);
        assert!(close(factor(&inputs()).unwrap(), 70_000.0 / decayed));
    }

    #[test]
    fn admin_decay_correction_uses_the_injected_dose_as_is() {
        let i = Inputs {
            decay_correction: Some("ADMIN".into()),
            ..inputs()
        };
        assert!(close(factor(&i).unwrap(), 70_000.0 / 370e6));
    }

    #[test]
    fn uncorrected_images_decay_to_their_acquisition_time() {
        let i = Inputs {
            decay_correction: Some("NONE".into()),
            ..inputs()
        };
        let decayed = 370e6 * 0.5f64.powf(3900.0 / F18_HALF_LIFE);
        assert!(close(factor(&i).unwrap(), 70_000.0 / decayed));
    }

    #[test]
    fn a_series_time_after_the_acquisition_falls_back_to_the_acquisition_time() {
        let i = Inputs {
            series: Some(when(11, 0)),
            ..inputs()
        };
        let decayed = 370e6 * 0.5f64.powf(3900.0 / F18_HALF_LIFE);
        assert!(close(factor(&i).unwrap(), 70_000.0 / decayed));
    }

    #[test]
    fn missing_items_are_all_named() {
        let i = Inputs {
            weight_kg: None,
            injection: None,
            ..inputs()
        };
        assert_eq!(factor(&i), Err(vec!["Patient weight", "Injection time"]));
        assert_eq!(
            factor(&Inputs::default()),
            Err(vec![
                "Patient weight",
                "Injected dose",
                "Half-life",
                "Injection time"
            ])
        );
    }

    #[test]
    fn a_zero_weight_counts_as_missing() {
        let i = Inputs {
            weight_kg: Some(0.0),
            ..inputs()
        };
        assert_eq!(factor(&i), Err(vec!["Patient weight"]));
    }

    #[test]
    fn times_without_dates_cross_midnight() {
        let i = Inputs {
            injection: at(None, "233000"),
            series: at(None, "003000"),
            acquisition: None,
            ..inputs()
        };
        let decayed = 370e6 * 0.5f64.powf(3600.0 / F18_HALF_LIFE);
        assert!(close(factor(&i).unwrap(), 70_000.0 / decayed));
    }

    #[test]
    fn dated_times_count_the_days_between() {
        let injection = date_time("20261231233000").unwrap();
        let series = at(Some("20270101"), "003000").unwrap();
        assert!(close(series.seconds - injection.seconds, 3600.0));
    }

    #[test]
    fn reads_dicom_times_with_fractions_and_colons() {
        assert_eq!(
            time_of_day("093015.5"),
            Some(9.0 * 3600.0 + 30.0 * 60.0 + 15.5)
        );
        assert_eq!(
            time_of_day("09:30:15"),
            Some(9.0 * 3600.0 + 30.0 * 60.0 + 15.0)
        );
        assert_eq!(time_of_day("0930"), Some(9.0 * 3600.0 + 30.0 * 60.0));
        assert_eq!(time_of_day("9x"), None);
    }

    #[test]
    fn date_times_ignore_the_time_zone() {
        assert_eq!(
            date_time("20261018090000+0900"),
            at(Some("20261018"), "090000")
        );
    }
}
