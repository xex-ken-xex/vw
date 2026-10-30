//! データセットのファイルを、撮影の出来事（旅行・イベント）・観測した夜・メタデータの値でまとめる。

use std::collections::{BTreeMap, HashMap};

use chrono::{Duration, NaiveDate, NaiveDateTime};

use crate::catalog::{Catalog, FileEntry};

/// まとめ方。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum GroupBy {
    #[default]
    None,
    /// 撮影時刻の空きと、撮影地点の離れ具合で区切る。
    Events,
    /// 昼の 12 時で日を区切る（日付をまたいだ一晩の観測を 1 つにする）。
    Nights,
    /// メタデータの値ごと。種類が空ならどの種類のキーでもよい。
    Key { kind: String, key: String },
}

impl GroupBy {
    /// よく使う値のまとめ方（題, 種類, キー）。
    pub const PRESETS: [(&'static str, &'static str, &'static str); 3] = [
        ("Target object", "fits", "OBJECT"),
        ("Optical filter", "fits", "FILTER"),
        ("Camera model", "exif", "Model"),
    ];

    /// 設定と保存した検索に書く形。`events`・`nights`・`fits:OBJECT`。
    pub fn to_setting(&self) -> String {
        match self {
            GroupBy::None => String::new(),
            GroupBy::Events => "events".into(),
            GroupBy::Nights => "nights".into(),
            GroupBy::Key { kind, key } => format!("{kind}:{key}"),
        }
    }

    pub fn from_setting(s: &str) -> Self {
        match s.trim() {
            "" => GroupBy::None,
            "events" => GroupBy::Events,
            "nights" => GroupBy::Nights,
            other => {
                let (kind, key) = crate::settings::SearchGroup::split_key(other);
                GroupBy::Key {
                    kind: kind.to_lowercase(),
                    key: key.to_owned(),
                }
            }
        }
    }
}

/// 出来事を区切る決まり。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// この時間より長く撮っていなければ、次の出来事にする。
    pub gap_hours: f64,
    /// 前の地点からこれより離れて撮ったら、次の出来事にする。0 なら地点では区切らない。
    pub distance_km: f64,
}

/// まとめた 1 組。
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// 値でまとめたときの値、夜でまとめたときの夜の日付。出来事は空（[`Group::span`] で表す）。
    /// 値や日時のないファイルの組も空。
    pub title: String,
    /// カタログの `files` の添字。
    pub files: Vec<usize>,
    /// 最初と最後の撮影日時（撮影地の時刻）。
    pub span: Option<(NaiveDateTime, NaiveDateTime)>,
    /// いちばん多く撮った町（なければ地方）。
    pub place: Option<String>,
    /// FITS の露出時間の合計（秒）。積算時間。
    pub exposure: Option<f64>,
}

impl Group {
    /// 値も日時もないファイルの組か。
    pub fn is_rest(&self) -> bool {
        self.title.is_empty() && self.span.is_none()
    }
}

/// `files`（カタログの `files` の添字）を `by` でまとめる。値や日時のないファイルは最後の組に入れる。
pub fn group(catalog: &Catalog, files: &[usize], by: &GroupBy, rules: Rules) -> Vec<Group> {
    let file = |i: usize| &catalog.files[i];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut titles: Vec<String> = Vec::new();
    let mut rest = Vec::new();
    match by {
        GroupBy::None => return Vec::new(),
        GroupBy::Events => {
            let mut timed: Vec<(NaiveDateTime, usize)> = Vec::new();
            for &i in files {
                match local_time(catalog, file(i)) {
                    Some(t) => timed.push((t, i)),
                    None => rest.push(i),
                }
            }
            timed.sort();
            let gap = Duration::seconds((rules.gap_hours * 3600.0) as i64);
            let mut last: Option<NaiveDateTime> = None;
            let mut here: Option<(f64, f64)> = None;
            for (t, i) in timed {
                let position = position(catalog, file(i));
                let moved = match (here, position) {
                    (Some(a), Some(b)) if rules.distance_km > 0.0 => {
                        crate::geonames::distance_km(a.0, a.1, b.0, b.1) > rules.distance_km
                    }
                    _ => false,
                };
                if last.is_none_or(|l| t - l > gap) || moved {
                    groups.push(Vec::new());
                    titles.push(String::new());
                    here = None;
                }
                groups.last_mut().unwrap().push(i);
                last = Some(t);
                if position.is_some() {
                    here = position;
                }
            }
        }
        GroupBy::Nights => {
            let mut nights: BTreeMap<NaiveDate, Vec<usize>> = BTreeMap::new();
            for &i in files {
                match local_time(catalog, file(i)) {
                    Some(t) => nights
                        .entry((t - Duration::hours(12)).date())
                        .or_default()
                        .push(i),
                    None => rest.push(i),
                }
            }
            for (night, files) in nights {
                titles.push(night.format("%Y-%m-%d").to_string());
                groups.push(files);
            }
        }
        GroupBy::Key { kind, key } => {
            let mut values: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            for &i in files {
                match value(catalog, file(i), kind, key).map(str::trim) {
                    Some(v) if !v.is_empty() => values.entry(v.to_owned()).or_default().push(i),
                    _ => rest.push(i),
                }
            }
            for (v, files) in values {
                titles.push(v);
                groups.push(files);
            }
        }
    }
    if !rest.is_empty() {
        groups.push(rest);
        titles.push(String::new());
    }
    groups
        .into_iter()
        .zip(titles)
        .map(|(files, title)| summarize(catalog, title, files))
        .collect()
}

fn summarize(catalog: &Catalog, title: String, files: Vec<usize>) -> Group {
    let mut times = files
        .iter()
        .filter_map(|&i| local_time(catalog, &catalog.files[i]));
    let span = times
        .next()
        .map(|first| times.fold((first, first), |(a, b), t| (a.min(t), b.max(t))));
    let mut towns: HashMap<&str, usize> = HashMap::new();
    let mut regions: HashMap<&str, usize> = HashMap::new();
    let mut exposure = None;
    for &i in &files {
        let f = &catalog.files[i];
        if let Some(t) = value(catalog, f, "location", "Town") {
            *towns.entry(t).or_default() += 1;
        } else if let Some(r) = value(catalog, f, "location", "Region") {
            *regions.entry(r).or_default() += 1;
        }
        let seconds = ["EXPTIME", "EXPOSURE"]
            .iter()
            .find_map(|k| value(catalog, f, "fits", k))
            .and_then(|v| v.trim().parse::<f64>().ok());
        if let Some(s) = seconds {
            *exposure.get_or_insert(0.0) += s;
        }
    }
    let most = |counts: HashMap<&str, usize>| {
        counts
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))
            .map(|(name, _)| name.to_owned())
    };
    Group {
        title,
        files,
        span,
        place: most(towns).or_else(|| most(regions)),
        exposure,
    }
}

/// `2024-05-01 10:20 – 18:05`、日をまたげば `2024-05-01 – 05-03`、年をまたげば両方に年を書く。
pub fn span_text((first, last): (NaiveDateTime, NaiveDateTime)) -> String {
    let start = first.format("%Y-%m-%d").to_string();
    if first.date() == last.date() {
        if first.time() == last.time() {
            format!("{} {}", start, first.format("%H:%M"))
        } else {
            format!(
                "{} {} – {}",
                start,
                first.format("%H:%M"),
                last.format("%H:%M")
            )
        }
    } else if first.date().format("%Y").to_string() == last.date().format("%Y").to_string() {
        format!("{start} – {}", last.format("%m-%d"))
    } else {
        format!("{start} – {}", last.format("%Y-%m-%d"))
    }
}

/// 積算時間を `2 h 05 m`・`12 m 30 s`・`45 s` の形にする。単位（` h`・` m`・` s`）は画面の言葉で渡す。
pub fn exposure_text(seconds: f64, [hours, minutes, secs]: [&str; 3]) -> String {
    let s = seconds.round() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}{hours} {m:02}{minutes}")
    } else if m > 0 {
        format!("{m}{minutes} {s:02}{secs}")
    } else {
        format!("{s}{secs}")
    }
}

/// ファイルの、`kind`（空ならどれでも）の `key`（大文字小文字を問わない）の値。
fn value<'a>(catalog: &'a Catalog, file: &FileEntry, kind: &str, key: &str) -> Option<&'a str> {
    file.meta
        .iter()
        .filter_map(|&i| catalog.meta.get(i as usize))
        .find(|(k, name, _)| (kind.is_empty() || k == kind) && name.eq_ignore_ascii_case(key))
        .map(|(_, _, v)| v.as_str())
}

/// 撮影した場所の時刻。
///
/// EXIF の撮影日時は撮影地の時刻なのでそのまま使う。FITS の DATE-OBS は世界時で、
/// そのまま昼の 12 時で区切ると、日本では夜の 21 時で一晩が 2 つに割れる。
/// そこで撮影地の時刻（DATE-LOC）を先に使い、なければ観測地の経度（SITELONG）で地方時に直す。
fn local_time(catalog: &Catalog, file: &FileEntry) -> Option<NaiveDateTime> {
    let get = |kind, key| value(catalog, file, kind, key).and_then(parse_time);
    if let Some(t) = get("exif", "DateTimeOriginal").or_else(|| get("fits", "DATE-LOC")) {
        return Some(t);
    }
    let utc = get("fits", "DATE-OBS")?;
    let longitude = value(catalog, file, "fits", "SITELONG").and_then(parse_degrees);
    Some(match longitude {
        Some(east) => utc + Duration::seconds((east * 240.0).round() as i64),
        None => utc,
    })
}

fn position(catalog: &Catalog, file: &FileEntry) -> Option<(f64, f64)> {
    let number = |key| {
        value(catalog, file, "location", key)?
            .trim()
            .parse::<f64>()
            .ok()
    };
    Some((number("Latitude")?, number("Longitude")?))
}

/// `2024-05-01 10:20:30`（EXIF）・`2024-01-01T21:00:00.123`（FITS）・`2024-01-01`。
fn parse_time(text: &str) -> Option<NaiveDateTime> {
    let text = text.trim();
    [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y:%m:%d %H:%M:%S",
    ]
    .iter()
    .find_map(|f| NaiveDateTime::parse_from_str(text, f).ok())
    .or_else(|| {
        NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
    })
}

/// `139.74`・`+139 44 24`・`139:44:24`（度・分・秒）を度にする。
fn parse_degrees(text: &str) -> Option<f64> {
    let text = text.trim();
    if let Ok(d) = text.parse::<f64>() {
        return Some(d);
    }
    let parts: Vec<f64> = text
        .split([' ', ':'])
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let (&d, rest) = parts.split_first()?;
    let minutes = rest.first().copied().unwrap_or(0.0) + rest.get(1).copied().unwrap_or(0.0) / 60.0;
    let sign = if text.starts_with('-') { -1.0 } else { 1.0 };
    Some(d + sign * minutes / 60.0)
}

#[cfg(test)]
mod tests;
