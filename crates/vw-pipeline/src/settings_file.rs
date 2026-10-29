//! vw の設定ファイル（settings.toml）のうち、vwbatch も読む部分。
//!
//! FITS の表示と校正は、ビューアの設定で絵が変わる。vwbatch がビューアと同じ絵を書き出すには
//! 同じ値を使う必要があるので、`[fits]` と `[fits.calibration]` をここで読む。
//! ビューアの `Settings`（apps/vw/src/settings.rs）をここへ移さないのは、あちらは画面の部品
//! （egui の色など）や設定画面の説明と一体で、vwbatch に要らない依存を持ち込むため。
//! 2 つの読み方がずれないことは、ビューア側のテストで確かめる。

use std::ffi::OsString;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use vw_fits::calibration;

/// 設定ファイルの場所。Windows は `%APPDATA%\vw`、それ以外は XDG の規約に従う。
/// `env` は環境変数を引く関数（テストで差し替えるため）。
pub fn config_path(windows: bool, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| env(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let dir = if windows {
        absolute("APPDATA")?
    } else {
        absolute("XDG_CONFIG_HOME").or_else(|| absolute("HOME").map(|h| h.join(".config")))?
    };
    Some(dir.join("vw").join("settings.toml"))
}

/// FITS の表示と校正の設定。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FitsSettings {
    pub options: vw_fits::Options,
    pub calibration: calibration::Options,
}

/// settings.toml の文から FITS の設定を読む。読めない項目はその項目だけ既定にし、
/// 何が読めなかったかを返す（ビューアと同じ扱い）。
pub fn read_fits(text: &str) -> (FitsSettings, Vec<String>) {
    let mut problems = Vec::new();
    let root = match text.parse::<toml::Table>() {
        Ok(t) => t,
        Err(e) => {
            problems.push(format!("cannot read the file: {}", e.message()));
            return (FitsSettings::default(), problems);
        }
    };
    let fits = table(&root, "fits", "fits", &mut problems);
    let cal = fits.and_then(|f| table(f, "calibration", "fits.calibration", &mut problems));
    let mut r = Section {
        table: fits,
        name: "fits",
        problems: &mut problems,
    };
    let options = read_options(&mut r);
    let mut r = Section {
        table: cal,
        name: "fits.calibration",
        problems: &mut problems,
    };
    let calibration = read_calibration(&mut r);
    (
        FitsSettings {
            options,
            calibration,
        },
        problems,
    )
}

fn table<'a>(
    parent: &'a toml::Table,
    key: &str,
    name: &str,
    problems: &mut Vec<String>,
) -> Option<&'a toml::Table> {
    match parent.get(key)? {
        toml::Value::Table(t) => Some(t),
        _ => {
            problems.push(format!("{name}: must be a table"));
            None
        }
    }
}

struct Section<'a> {
    table: Option<&'a toml::Table>,
    name: &'static str,
    problems: &'a mut Vec<String>,
}

impl Section<'_> {
    fn get<T: DeserializeOwned>(&mut self, key: &str) -> Option<T> {
        let value = self.table?.get(key)?;
        match value.clone().try_into() {
            Ok(v) => Some(v),
            Err(e) => {
                self.problem(key, e.to_string());
                None
            }
        }
    }

    fn get_valid<T: DeserializeOwned>(
        &mut self,
        key: &str,
        valid: impl Fn(&T) -> bool,
        why: &str,
    ) -> Option<T> {
        let v = self.get(key)?;
        if valid(&v) {
            Some(v)
        } else {
            self.problem(key, why.to_owned());
            None
        }
    }

    fn problem(&mut self, key: &str, why: String) {
        self.problems.push(format!("{}.{key}: {why}", self.name));
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Stretch {
    MinMax,
    Percentile,
    Zscale,
    Symmetric,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Curve {
    Linear,
    Sqrt,
    Log,
    Asinh,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Apply {
    Auto,
    Off,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum DarkScaling {
    None,
    Exposure,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FlatMatch {
    Filter,
    FilterAndNight,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FlatNormalize {
    Median,
    Mean,
}

const PERCENT_RANGE: std::ops::RangeInclusive<f32> = 0.0..=100.0;
const TOLERANCE_RANGE: std::ops::RangeInclusive<f64> = 0.0..=3600.0;

fn read_options(r: &mut Section<'_>) -> vw_fits::Options {
    let d = vw_fits::Options::default();
    let percent = |v: &f32| PERCENT_RANGE.contains(v);
    let mut low_percent = r
        .get_valid("low_percent", percent, "must be 0 to 100")
        .unwrap_or(d.low_percent);
    let mut high_percent = r
        .get_valid("high_percent", percent, "must be 0 to 100")
        .unwrap_or(d.high_percent);
    if low_percent >= high_percent {
        r.problem(
            "high_percent",
            "must be larger than low_percent (both reset)".to_owned(),
        );
        (low_percent, high_percent) = (d.low_percent, d.high_percent);
    }
    let stretch = match r.get("stretch") {
        None => d.stretch,
        Some(Stretch::MinMax) => vw_fits::Stretch::MinMax,
        Some(Stretch::Percentile) => vw_fits::Stretch::Percentile,
        Some(Stretch::Zscale) => vw_fits::Stretch::ZScale,
        Some(Stretch::Symmetric) => vw_fits::Stretch::Symmetric,
    };
    let curve = match r.get("curve") {
        None => d.curve,
        Some(Curve::Linear) => vw_core::Curve::Linear,
        Some(Curve::Sqrt) => vw_core::Curve::Sqrt,
        Some(Curve::Log) => vw_core::Curve::Log,
        Some(Curve::Asinh) => vw_core::Curve::Asinh,
    };
    let missing = r
        .get_valid(
            "missing_color",
            |v: &String| hex_color(v).is_some(),
            "must be #RRGGBB",
        )
        .map_or(d.missing, |v| hex_color(&v).map(visible_on_palettes));
    vw_fits::Options {
        stretch,
        low_percent,
        high_percent,
        curve,
        flip_rows: r.get("flip_rows").unwrap_or(d.flip_rows),
        demosaic: r.get("demosaic").unwrap_or(d.demosaic),
        same_window_for_planes: r
            .get("same_window_for_planes")
            .unwrap_or(d.same_window_for_planes),
        missing,
    }
}

/// 灰色（赤・緑・青が同じ）だと色の表を引いたときに表の色に変わってしまうので、
/// 見分けのつかない 1 段だけ青をずらす。
fn visible_on_palettes([r, g, mut b]: [u8; 3]) -> [u8; 3] {
    if r == g && g == b {
        b = if b == 255 { 254 } else { b + 1 };
    }
    [r, g, b]
}

/// `#rrggbb` を読む。
fn hex_color(text: &str) -> Option<[u8; 3]> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// `synthetic_bias` の値。空なら `None`、数でなければ `Err`。
fn bias_value(text: &str) -> Result<Option<f64>, ()> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(Some)
        .ok_or(())
}

fn read_calibration(r: &mut Section<'_>) -> calibration::Options {
    use calibration as c;
    let d = c::Options::default();
    let tolerance = |v: &f64| TOLERANCE_RANGE.contains(v);
    c::Options {
        apply: r.get("apply").map_or(d.apply, |a: Apply| a == Apply::Auto),
        search_nearby: r.get("search_nearby").unwrap_or(d.search_nearby),
        library: r
            .get::<Vec<String>>("library")
            .map_or(d.library.clone(), |list| {
                list.iter()
                    .filter(|p| !p.trim().is_empty())
                    .map(PathBuf::from)
                    .collect()
            }),
        use_dark: r.get("use_dark").unwrap_or(d.use_dark),
        use_flat: r.get("use_flat").unwrap_or(d.use_flat),
        use_bias: r.get("use_bias").unwrap_or(d.use_bias),
        exposure_tolerance_secs: r
            .get_valid("exposure_tolerance_secs", tolerance, "must be 0 to 3600")
            .unwrap_or(d.exposure_tolerance_secs),
        temperature_tolerance: r
            .get_valid("temperature_tolerance", tolerance, "must be 0 to 3600")
            .unwrap_or(d.temperature_tolerance),
        match_gain_offset: r.get("match_gain_offset").unwrap_or(d.match_gain_offset),
        dark_scaling: match r.get("dark_scaling") {
            None => d.dark_scaling,
            Some(DarkScaling::None) => c::DarkScaling::None,
            Some(DarkScaling::Exposure) => c::DarkScaling::Exposure,
        },
        flat_match: match r.get("flat_match") {
            None => d.flat_match,
            Some(FlatMatch::Filter) => c::FlatMatch::Filter,
            Some(FlatMatch::FilterAndNight) => c::FlatMatch::FilterAndNight,
        },
        flat_normalize: match r.get("flat_normalize") {
            None => d.flat_normalize,
            Some(FlatNormalize::Median) => c::FlatNormalize::Median,
            Some(FlatNormalize::Mean) => c::FlatNormalize::Mean,
        },
        equalize_cfa: r.get("equalize_cfa").unwrap_or(d.equalize_cfa),
        synthetic_bias: r
            .get_valid(
                "synthetic_bias",
                |v: &String| bias_value(v).is_ok(),
                "must be empty or a number",
            )
            .map_or(d.synthetic_bias, |v| bias_value(&v).ok().flatten()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_fits_table_gives_the_defaults() {
        let (s, problems) = read_fits("[viewer]\nlayout = \"1x1\"\n");
        assert_eq!(s, FitsSettings::default());
        assert!(problems.is_empty());
    }

    #[test]
    fn written_values_replace_the_defaults() {
        let (s, problems) = read_fits(
            "[fits]\nstretch = \"zscale\"\ncurve = \"asinh\"\nmissing_color = \"#808080\"\n\
             [fits.calibration]\napply = \"off\"\nsynthetic_bias = \"100\"\nlibrary = [\"/m\", \" \"]\n",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(s.options.stretch, vw_fits::Stretch::ZScale);
        assert_eq!(s.options.curve, vw_core::Curve::Asinh);
        assert_eq!(s.options.missing, Some([128, 128, 129]));
        assert!(!s.calibration.apply);
        assert_eq!(s.calibration.synthetic_bias, Some(100.0));
        assert_eq!(s.calibration.library, [PathBuf::from("/m")]);
    }

    #[test]
    fn a_bad_value_resets_only_that_item_and_is_reported() {
        let (s, problems) = read_fits(
            "[fits]\nstretch = \"wide\"\ncurve = \"log\"\nlow_percent = 90\nhigh_percent = 10\n",
        );
        let d = vw_fits::Options::default();
        assert_eq!(s.options.stretch, d.stretch);
        assert_eq!(s.options.curve, vw_core::Curve::Log);
        assert_eq!(
            (s.options.low_percent, s.options.high_percent),
            (d.low_percent, d.high_percent)
        );
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn an_unreadable_file_gives_the_defaults_and_says_so() {
        let (s, problems) = read_fits("[fits\n");
        assert_eq!(s, FitsSettings::default());
        assert_eq!(problems.len(), 1);
    }
}
