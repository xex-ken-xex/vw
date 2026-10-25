//! ライトに合うマスターを選ぶ。

use super::frame::{Frame, Kind};
use super::{DarkScaling, FlatMatch, Options};

/// 選んだマスターと、合わせた条件の説明。
#[derive(Debug, Clone, PartialEq)]
pub struct Choice<'a> {
    pub frame: &'a Frame,
    pub note: String,
}

/// 条件を 1 つずつ当てて候補を絞る。どこかで 0 枚になったら、その条件を理由にする。
struct Narrowing<'a> {
    kind: Kind,
    left: Vec<&'a Frame>,
}

impl<'a> Narrowing<'a> {
    fn new(kind: Kind, frames: &'a [Frame]) -> Result<Self, String> {
        let all: Vec<&Frame> = frames.iter().filter(|f| f.kind == kind).collect();
        if all.is_empty() {
            return Err(format!("no {} found", kind.name()));
        }
        let left: Vec<&Frame> = all.iter().copied().filter(|f| f.master).collect();
        if left.is_empty() {
            return Err(format!(
                "only single {} frames ({}), no master",
                kind.name(),
                all.len()
            ));
        }
        Ok(Self { kind, left })
    }

    fn keep(mut self, what: &str, test: impl Fn(&Frame) -> bool) -> Result<Self, String> {
        self.left.retain(|f| test(f));
        if self.left.is_empty() {
            Err(format!("no master {} {what}", self.kind.name()))
        } else {
            Ok(self)
        }
    }

    /// `rank` が小さいものを選ぶ。
    fn best(self, rank: impl Fn(&Frame) -> f64) -> &'a Frame {
        self.left
            .into_iter()
            .min_by(|a, b| rank(a).total_cmp(&rank(b)))
            .expect("narrowing never leaves an empty list")
    }
}

fn same_shape(light: &Frame) -> impl Fn(&Frame) -> bool + '_ {
    move |f| f.width == light.width && f.height == light.height && f.binning == light.binning
}

fn shape_text(light: &Frame) -> String {
    format!(
        "of {} x {} (bin {}x{})",
        light.width, light.height, light.binning.0, light.binning.1
    )
}

/// 両方に書いてあるときだけ比べる。
fn same_if_both(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() < 1e-6,
        _ => true,
    }
}

fn gain_offset<'a>(light: &'a Frame, o: &'a Options) -> impl Fn(&Frame) -> bool + 'a {
    let check = o.match_gain_offset;
    move |f| !check || (same_if_both(f.gain, light.gain) && same_if_both(f.offset, light.offset))
}

fn gain_offset_text(light: &Frame) -> String {
    let mut parts = Vec::new();
    if let Some(g) = light.gain {
        parts.push(format!("GAIN {}", number(g)));
    }
    if let Some(o) = light.offset {
        parts.push(format!("OFFSET {}", number(o)));
    }
    format!("with {}", parts.join(" "))
}

/// 小数点以下が 0 なら整数で書く。
pub fn number(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

fn time_gap(a: &Frame, b: &Frame) -> f64 {
    match (a.time, b.time) {
        (Some(x), Some(y)) => (x - y).abs(),
        _ => f64::MAX / 4.0,
    }
}

/// ライトから引くダーク。
pub fn dark<'a>(light: &Frame, frames: &'a [Frame], o: &Options) -> Result<Choice<'a>, String> {
    let mut n = Narrowing::new(Kind::Dark, frames)?
        .keep(&shape_text(light), same_shape(light))?
        .keep(&gain_offset_text(light), gain_offset(light, o))?;
    let scaling = o.dark_scaling == DarkScaling::Exposure;
    if !scaling {
        let Some(exposure) = light.exposure else {
            return Err("the light has no EXPTIME to match a dark".into());
        };
        let tol = o.exposure_tolerance_secs;
        n = n.keep(
            &format!("of {} s (±{} s)", number(exposure), number(tol)),
            |f| {
                f.exposure
                    .is_some_and(|e| (e - exposure).abs() <= tol + 1e-9)
            },
        )?;
    } else {
        n = n.keep("with EXPTIME", |f| f.exposure.is_some())?;
    }
    if let Some(t) = light.temperature {
        let tol = o.temperature_tolerance;
        n = n.keep(
            &format!("within ±{} °C of {} °C", number(tol), number(t)),
            |f| f.temperature.is_none_or(|d| (d - t).abs() <= tol + 1e-9),
        )?;
    }
    let best = n.best(|f| {
        let exposure = match (f.exposure, light.exposure) {
            (Some(a), Some(b)) => (a - b).abs(),
            _ => 0.0,
        };
        let temp = match (f.temperature, light.temperature) {
            (Some(a), Some(b)) => (a - b).abs(),
            _ => 0.0,
        };
        // 露出の差、温度の差、日時の差の順に効く重み。
        exposure * 1e12 + temp * 1e8 + time_gap(f, light).min(1e7)
    });
    Ok(Choice {
        frame: best,
        note: conditions(best),
    })
}

/// ライトを割るフラット。
pub fn flat<'a>(light: &Frame, frames: &'a [Frame], o: &Options) -> Result<Choice<'a>, String> {
    let filter = light.filter.as_deref().map(str::to_lowercase);
    let filter_text = match &light.filter {
        Some(f) => format!("for filter {f}"),
        None => "without a filter".into(),
    };
    let mut n = Narrowing::new(Kind::Flat, frames)?
        .keep(&shape_text(light), same_shape(light))?
        .keep(&filter_text, |f| {
            f.filter.as_deref().map(str::to_lowercase) == filter
        })?;
    if o.flat_match == FlatMatch::FilterAndNight {
        let night = light.night();
        n = n.keep("from the same night", |f| {
            night.is_some() && f.night() == night
        })?;
    }
    let best = n.best(|f| time_gap(f, light));
    Ok(Choice {
        frame: best,
        note: conditions(best),
    })
}

/// フラットから引くダークフラット（フラットと同じ露出）。
pub fn dark_flat<'a>(flat: &Frame, frames: &'a [Frame], o: &Options) -> Result<Choice<'a>, String> {
    let Some(exposure) = flat.exposure else {
        return Err("the flat has no EXPTIME to match a dark flat".into());
    };
    let tol = o.exposure_tolerance_secs;
    let best = Narrowing::new(Kind::DarkFlat, frames)?
        .keep(&shape_text(flat), same_shape(flat))?
        .keep(&gain_offset_text(flat), gain_offset(flat, o))?
        .keep(&format!("of {} s", number(exposure)), |f| {
            f.exposure
                .is_some_and(|e| (e - exposure).abs() <= tol + 1e-9)
        })?
        .best(|f| time_gap(f, flat));
    Ok(Choice {
        frame: best,
        note: conditions(best),
    })
}

/// バイアス。
pub fn bias<'a>(of: &Frame, frames: &'a [Frame], o: &Options) -> Result<Choice<'a>, String> {
    let best = Narrowing::new(Kind::Bias, frames)?
        .keep(&shape_text(of), same_shape(of))?
        .keep(&gain_offset_text(of), gain_offset(of, o))?
        .best(|f| time_gap(f, of));
    Ok(Choice {
        frame: best,
        note: conditions(best),
    })
}

/// メタデータに出す、そのマスターの条件。
fn conditions(f: &Frame) -> String {
    let mut parts = Vec::new();
    if let Some(e) = f.exposure {
        parts.push(format!("{} s", number(e)));
    }
    if let Some(t) = f.temperature {
        parts.push(format!("{} °C", number(t)));
    }
    if let Some(g) = f.gain {
        parts.push(format!("GAIN {}", number(g)));
    }
    if let Some(o) = f.offset {
        parts.push(format!("OFFSET {}", number(o)));
    }
    if let Some(filter) = &f.filter {
        parts.push(format!("filter {filter}"));
    }
    parts.join(", ")
}
