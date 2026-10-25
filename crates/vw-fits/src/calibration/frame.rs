//! 1 枚の FITS が何の画像で、どんな条件で撮ったか。ヘッダだけから読む。

use std::path::{Path, PathBuf};

/// 画像の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Light,
    Dark,
    Flat,
    Bias,
    DarkFlat,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Light => "light",
            Kind::Dark => "dark",
            Kind::Flat => "flat",
            Kind::Bias => "bias",
            Kind::DarkFlat => "dark flat",
        }
    }

    /// `IMAGETYP` の値や、フォルダの名前から。撮影ソフトごとの書き方
    /// （`Dark Frame`、`Flat Field`、IRAF の `zero`、PixInsight の `Master Dark`）を同じに読む。
    fn from_word(word: &str) -> Option<Kind> {
        let w = normalized(word);
        let w = w.strip_prefix("master").unwrap_or(&w);
        if w.starts_with("darkflat") || w.starts_with("flatdark") {
            Some(Kind::DarkFlat)
        } else if w.starts_with("dark") {
            Some(Kind::Dark)
        } else if w.starts_with("flat") {
            Some(Kind::Flat)
        } else if w.starts_with("bias") || w.starts_with("offset") || w == "zero" {
            Some(Kind::Bias)
        } else if w.starts_with("light") || w.starts_with("object") || w.starts_with("science") {
            Some(Kind::Light)
        } else {
            None
        }
    }
}

/// 大文字小文字と、区切りの `_` `-` 空白を無視して比べるための形。
fn normalized(word: &str) -> String {
    word.chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

/// 校正用の画像を入れておくフォルダの名前か（種類別のフォルダと、`calibration` `masters`）。
pub fn is_calibration_folder(name: &str) -> bool {
    let w = normalized(name);
    matches!(Kind::from_word(name), Some(k) if k != Kind::Light)
        || w.starts_with("calib")
        || w == "master"
        || w == "masters"
}

/// 種類のフォルダとして見る、ファイルから数えた上の階層の数。
const FOLDER_DEPTH: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub path: PathBuf,
    pub kind: Kind,
    /// 何枚も重ねて作ったマスターか。
    pub master: bool,
    pub width: usize,
    pub height: usize,
    pub binning: (i64, i64),
    /// 露出（秒）。
    pub exposure: Option<f64>,
    /// センサーの温度（℃）。
    pub temperature: Option<f64>,
    pub gain: Option<f64>,
    pub offset: Option<f64>,
    pub filter: Option<String>,
    /// 撮った時刻（`DATE-OBS`）。比べるだけなので、ある日を起点にした秒。
    pub time: Option<f64>,
}

impl Frame {
    /// `get` はカードの名前から値を返す（画像の HDU を先に、Primary を後に引く）。
    /// 画像の大きさが分からなければ `None`。
    pub fn read(path: &Path, get: &dyn Fn(&str) -> Option<String>) -> Option<Frame> {
        let num =
            |key: &str| -> Option<f64> { get(key)?.trim().replace(['D', 'd'], "E").parse().ok() };
        let text = |key: &str| {
            get(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let (width, height) = ["ZNAXIS", "NAXIS"].iter().find_map(|prefix| {
            let w = num(&format!("{prefix}1"))?;
            let h = num(&format!("{prefix}2"))?;
            (w >= 1.0 && h >= 1.0).then_some((w as usize, h as usize))
        })?;
        let imagetyp = text("IMAGETYP").or_else(|| text("FRAME"));
        let folders: Vec<String> = path
            .ancestors()
            .skip(1)
            .take(FOLDER_DEPTH)
            .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
            .collect();
        let kind = imagetyp
            .as_deref()
            .and_then(Kind::from_word)
            .or_else(|| folders.iter().find_map(|f| Kind::from_word(f)))
            .unwrap_or(Kind::Light);
        let stem = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(normalized)
            .unwrap_or_default();
        let stacked = ["NCOMBINE", "STACKCNT"]
            .iter()
            .any(|k| num(k).is_some_and(|n| n >= 2.0));
        let master = imagetyp
            .as_deref()
            .is_some_and(|t| normalized(t).starts_with("master"))
            || stem.contains("master")
            || stacked
            || folders.iter().any(|f| normalized(f).starts_with("master"));
        let binning = (
            num("XBINNING").map_or(1, |v| v as i64),
            num("YBINNING").map_or(1, |v| v as i64),
        );
        Some(Frame {
            path: path.to_path_buf(),
            kind,
            master,
            width,
            height,
            binning,
            exposure: num("EXPTIME").or_else(|| num("EXPOSURE")),
            temperature: num("CCD-TEMP").or_else(|| num("SET-TEMP")),
            gain: num("GAIN"),
            offset: num("OFFSET"),
            filter: text("FILTER"),
            time: get("DATE-OBS").as_deref().and_then(seconds),
        })
    }

    /// 写した夜。昼の 12 時で日を区切るので、日付をまたいだ一晩は同じ夜になる。
    pub fn night(&self) -> Option<i64> {
        Some(((self.time? - 12.0 * 3600.0) / 86400.0).floor() as i64)
    }
}

/// `YYYY-MM-DD[Thh:mm:ss[.fff]]` を、1970-01-01 からの秒にする。
pub fn seconds(date: &str) -> Option<f64> {
    let date = date.trim();
    let (day, time) = date.split_once('T').unwrap_or((date, ""));
    let mut ymd = day.split('-');
    let y: i64 = ymd.next()?.parse().ok()?;
    let m: i64 = ymd.next()?.parse().ok()?;
    let d: i64 = ymd.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut hms = time.split(':');
    let mut part = || -> f64 { hms.next().and_then(|v| v.parse().ok()).unwrap_or(0.0) };
    let (h, min, s) = (part(), part(), part());
    Some(days_from_civil(y, m, d) as f64 * 86400.0 + h * 3600.0 + min * 60.0 + s)
}

/// グレゴリオ暦の日付の、1970-01-01 からの日数（H. Hinnant の式）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
