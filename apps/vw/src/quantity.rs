//! 画素の値が何の量か（CT の HU、PET の Bq/ml と SUVbw など）と、その書き方。
//! 十字の印と ROI の値に単位を付けるのに使う。

use vw_core::Metadata;

use crate::histogram;
use crate::i18n::tr_text;
use crate::settings::Language;

/// 画素の値（Rescale を当てた値）の量。
#[derive(Debug, Clone, PartialEq)]
pub enum Quantity {
    /// 単位の分からない生の値。
    Raw,
    Unit(&'static str),
    /// ファイルに書いてある単位（FITS の `BUNIT`。電波の Jy/beam や K など）。
    Named(String),
    /// PET の Bq/ml。SUVbw に換算できれば掛ける値、できなければ足りない項目。
    Pet(Result<f64, String>),
}

impl Quantity {
    pub fn of(meta: &Metadata) -> Self {
        let dicom = |key: &str| meta.get("DICOM", key).map(str::trim);
        match dicom("Modality") {
            Some("CT") => Quantity::Unit("HU"),
            Some("PT") => match dicom("Units") {
                Some("BQML") => Quantity::Pet(
                    match dicom(vw_dicom::SUV_FACTOR).and_then(|f| f.parse::<f64>().ok()) {
                        Some(f) => Ok(f),
                        None => Err(dicom(vw_dicom::SUV_NEEDS).unwrap_or_default().to_owned()),
                    },
                ),
                // 装置が SUV にして書いたもの。
                Some("GML") => Quantity::Unit("g/ml"),
                Some("CNTS") => Quantity::Unit("counts"),
                _ => Quantity::Raw,
            },
            _ => fits_unit(meta)
                .or_else(|| netcdf_unit(meta))
                .map_or(Quantity::Raw, Quantity::Named),
        }
    }

    /// 1 つの値を単位つきで書く。PET は SUVbw を先に、Bq/ml を後ろに並べる。
    pub fn text(&self, v: f64) -> String {
        match self {
            Quantity::Raw => histogram::number(v),
            Quantity::Unit(unit) => format!("{} {unit}", histogram::number(v)),
            Quantity::Named(unit) => format!("{} {unit}", histogram::number(v)),
            Quantity::Pet(Ok(f)) => format!("{}  {}", suv(v * f), bq(v)),
            Quantity::Pet(Err(_)) => bq(v),
        }
    }

    /// ROI の平均の横に足す、最大の SUV（SUVmax）。
    pub fn suv_max(&self, max: f64) -> Option<String> {
        match self {
            Quantity::Pet(Ok(f)) => Some(format!("SUVmax {:.2}", max * f)),
            _ => None,
        }
    }

    /// SUVbw を出せないときの、足りない項目。
    pub fn missing(&self) -> Option<&str> {
        match self {
            Quantity::Pet(Err(m)) => Some(m),
            _ => None,
        }
    }
}

/// FITS の `BUNIT`。画像の HDU のもの（後ろの見出し）を先に見る。
pub fn fits_unit(meta: &Metadata) -> Option<String> {
    meta.entries
        .iter()
        .rev()
        .find(|(g, k, _)| g.starts_with(vw_fits::GROUP) && k == "BUNIT")
        .map(|(_, _, v)| v.trim().trim_matches('\'').trim().to_owned())
        .filter(|u| !u.is_empty())
}

/// NetCDF の変数の `units`。
pub fn netcdf_unit(meta: &Metadata) -> Option<String> {
    meta.get(vw_netcdf::GROUP, vw_netcdf::UNITS)
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
}

/// 足りない項目（`SUVbw needs` の、カンマで区切った英語の名前）を画面の言葉にする。
pub fn missing_text(lang: Language, missing: &str) -> String {
    missing
        .split(", ")
        .map(|m| tr_text(lang, m))
        .collect::<Vec<_>>()
        .join(", ")
}

/// SUV は 1 前後の値なので、整数でも小数 2 桁まで出す（`histogram::number` は整数なら丸める）。
fn suv(v: f64) -> String {
    format!("{v:.2} SUVbw")
}

/// Bq/ml は数千から数万の値で、小数は読むのに邪魔なだけなので丸める。
fn bq(v: f64) -> String {
    format!("{v:.0} Bq/ml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(entries: &[(&str, &str)]) -> Metadata {
        let mut m = Metadata::default();
        for (k, v) in entries {
            m.push("DICOM", *k, *v);
        }
        m
    }

    #[test]
    fn fits_values_carry_the_unit_written_in_bunit() {
        let mut m = Metadata::default();
        m.push(vw_fits::GROUP, "BUNIT", "Jy/beam");
        assert_eq!(Quantity::of(&m).text(0.5), "0.50 Jy/beam");
        assert_eq!(Quantity::of(&Metadata::default()).text(0.5), "0.50");
    }

    #[test]
    fn netcdf_values_carry_the_unit_of_the_variable() {
        let mut m = Metadata::default();
        m.push(vw_netcdf::GROUP, vw_netcdf::UNITS, "K");
        assert_eq!(Quantity::of(&m).text(273.5), "273.50 K");
    }

    #[test]
    fn ct_values_are_hu() {
        let q = Quantity::of(&meta(&[("Modality", "CT")]));
        assert_eq!(q.text(-35.0), "-35 HU");
    }

    #[test]
    fn pet_values_show_suv_and_bq_per_ml() {
        let q = Quantity::of(&meta(&[
            ("Modality", "PT"),
            ("Units", "BQML"),
            (vw_dicom::SUV_FACTOR, "0.0002"),
        ]));
        assert_eq!(q.text(12_400.4), "2.48 SUVbw  12400 Bq/ml");
        assert_eq!(q.suv_max(20_000.0).as_deref(), Some("SUVmax 4.00"));
        assert_eq!(q.missing(), None);
    }

    #[test]
    fn pet_without_what_suv_needs_shows_bq_per_ml_and_what_is_missing() {
        let q = Quantity::of(&meta(&[
            ("Modality", "PT"),
            ("Units", "BQML"),
            (vw_dicom::SUV_NEEDS, "Patient weight"),
        ]));
        assert_eq!(q.text(12_400.0), "12400 Bq/ml");
        assert_eq!(q.suv_max(1.0), None);
        assert_eq!(q.missing(), Some("Patient weight"));
    }

    #[test]
    fn missing_items_are_shown_in_the_chosen_language() {
        assert_eq!(
            missing_text(Language::Japanese, "Patient weight, Injection time"),
            "体重, 投与の時刻"
        );
        assert_eq!(
            missing_text(Language::English, "Patient weight"),
            "Patient weight"
        );
    }

    #[test]
    fn pet_already_in_suv_is_shown_in_g_per_ml() {
        let q = Quantity::of(&meta(&[("Modality", "PT"), ("Units", "GML")]));
        assert_eq!(q.text(2.5), "2.50 g/ml");
    }

    #[test]
    fn other_values_have_no_unit() {
        assert_eq!(
            Quantity::of(&meta(&[("Modality", "MR")])).text(812.0),
            "812"
        );
        assert_eq!(Quantity::of(&meta(&[])).text(0.126), "0.13");
    }
}
