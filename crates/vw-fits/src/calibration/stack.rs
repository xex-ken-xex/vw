//! 1 枚ずつの校正画像を重ねてマスターを作り、キャッシュのフォルダに FITS で残す。
//!
//! 重ね方は、画素ごとに中央値から σ（中央値からの外れの中央値 × 1.4826）の何倍も外れた値を
//! 除いた平均（σ クリップ平均）。宇宙線や、撮影中に横切った人工衛星の筋を消す。

use std::path::{Path, PathBuf};

use vw_core::{ColorModel, PixelData, VwImage};

use super::frame::{Frame, Kind};

/// 一度に持つ値の上限（バイト）。超える分は、行の帯に分けて重ねる。
const BUDGET_BYTES: usize = 1 << 30;

/// 作ったマスターのファイル名の頭。キャッシュを消すとき、これで始まるものだけを消す。
pub(super) const PREFIX: &str = "vw-master-";

/// 重ねて 1 枚のマスターにする、同じ条件で撮った 1 枚ずつの画像。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Group<'a> {
    pub kind: Kind,
    pub frames: Vec<&'a Frame>,
}

/// 同じマスターになる条件。ダークは露出・温度（1℃ に丸める）・ゲイン・オフセット、
/// フラットはフィルターと夜（ゴミの影は夜ごとに変わるため）、バイアスはゲイン・オフセット。
fn condition(f: &Frame) -> String {
    let shape = format!("{}x{} bin {:?}", f.width, f.height, f.binning);
    let num = |v: Option<f64>, digits: usize| v.map_or("-".to_owned(), |v| format!("{v:.digits$}"));
    match f.kind {
        Kind::Dark | Kind::DarkFlat => format!(
            "{} {shape} exp {} temp {} gain {} offset {}",
            f.kind.name(),
            num(f.exposure, 3),
            num(f.temperature.map(f64::round), 0),
            num(f.gain, 3),
            num(f.offset, 3)
        ),
        Kind::Flat => format!(
            "flat {shape} filter {} night {}",
            f.filter.as_deref().unwrap_or("-"),
            f.night().map_or("-".to_owned(), |n| n.to_string())
        ),
        Kind::Bias => format!(
            "bias {shape} gain {} offset {}",
            num(f.gain, 3),
            num(f.offset, 3)
        ),
        Kind::Light => format!("light {shape}"),
    }
}

/// マスターのない条件の、1 枚ずつの画像の組。2 枚以上あるものだけ（1 枚では重ねる意味がない）。
pub(super) fn groups(frames: &[Frame]) -> Vec<Group<'_>> {
    let with_master: Vec<String> = frames.iter().filter(|f| f.master).map(condition).collect();
    let mut out: Vec<(String, Group)> = Vec::new();
    for f in frames.iter().filter(|f| !f.master && f.kind != Kind::Light) {
        let key = condition(f);
        if with_master.contains(&key) {
            continue;
        }
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, g)) => g.frames.push(f),
            None => out.push((
                key,
                Group {
                    kind: f.kind,
                    frames: vec![f],
                },
            )),
        }
    }
    out.into_iter()
        .map(|(_, g)| g)
        .filter(|g| g.frames.len() >= 2)
        .collect()
}

impl Group<'_> {
    /// キャッシュのファイル名。元のファイル（パス・大きさ・更新時刻）と σ が変われば変わるので、
    /// 撮り足した・撮り直したときは作り直す。
    ///
    /// 標準のハッシュ（`DefaultHasher`）を使わないのは、計算が Rust の版で変わってよいことに
    /// なっていて、vw を新しくするたびに名前が変わり、作ったマスターを作り直してしまうため。
    pub fn file_name(&self, sigma: f64) -> String {
        let mut h = Fnv::default();
        let mut paths: Vec<&PathBuf> = self.frames.iter().map(|f| &f.path).collect();
        paths.sort();
        for p in paths {
            h.write(p.as_os_str().as_encoded_bytes());
            h.write(&[0]);
            if let Ok(m) = std::fs::metadata(p) {
                h.write(&m.len().to_le_bytes());
                if let Ok(t) = m.modified()
                    && let Ok(since) = t.duration_since(std::time::UNIX_EPOCH)
                {
                    h.write(&since.as_secs().to_le_bytes());
                    h.write(&since.subsec_nanos().to_le_bytes());
                }
            }
        }
        h.write(&sigma.to_bits().to_le_bytes());
        let kind = self.kind.name().replace(' ', "");
        format!("{PREFIX}{kind}-{:016x}.fits", h.0)
    }

    /// 重ねて `out` に書く。途中のファイルは別の名前で書いてから置き換えるので、書きかけを読まない。
    pub fn build(&self, sigma: f64, out: &Path) -> Result<(), String> {
        let first = self.frames[0];
        let (w, h) = (first.width, first.height);
        let n = w * h;
        let read = |f: &Frame| -> Result<Vec<f32>, String> {
            let bytes = std::fs::read(&f.path).map_err(|e| format!("{}: {e}", f.path.display()))?;
            let v =
                crate::mosaic_values(&bytes).map_err(|e| format!("{}: {e}", f.path.display()))?;
            if v.len() == n {
                Ok(v)
            } else {
                Err(format!("{}: size differs", f.path.display()))
            }
        };
        let count = self.frames.len();
        let rows_per_band = (BUDGET_BYTES / (count * w * 4).max(1)).clamp(1, h);
        let whole = rows_per_band >= h;
        // 1 回で収まるなら読んだ値を持ち続け、収まらなければ帯ごとに読み直す（読み直しは遅いが、
        // 何十枚もの大きな画像を全部持つとメモリが足りなくなるため）。
        let mut kept: Vec<Vec<f32>> = Vec::new();
        let scales = if self.kind == Kind::Flat {
            // フラットは 1 枚ずつ明るさが違う（空の明るさ・パネルの揺らぎ）ので、中央値を揃えてから重ねる。
            let mut medians = Vec::with_capacity(count);
            for f in &self.frames {
                let v = read(f)?;
                medians.push(f64::from(sampled_median(&v)));
                if whole {
                    kept.push(v);
                }
            }
            let mean = medians.iter().sum::<f64>() / count as f64;
            medians
                .iter()
                .map(|&m| if m > 0.0 { (mean / m) as f32 } else { 1.0 })
                .collect()
        } else {
            vec![1.0f32; count]
        };
        if whole && kept.is_empty() {
            for f in &self.frames {
                kept.push(read(f)?);
            }
        }
        let mut master = vec![0.0f32; n];
        let mut column = Vec::with_capacity(count);
        for top in (0..h).step_by(rows_per_band) {
            let rows = rows_per_band.min(h - top);
            let (start, end) = (top * w, (top + rows) * w);
            let band: Vec<Vec<f32>> = if whole {
                Vec::new()
            } else {
                self.frames
                    .iter()
                    .map(|f| read(f).map(|v| v[start..end].to_vec()))
                    .collect::<Result<_, _>>()?
            };
            for i in start..end {
                column.clear();
                for (k, scale) in scales.iter().enumerate() {
                    let v = if whole {
                        kept[k][i]
                    } else {
                        band[k][i - start]
                    };
                    if v.is_finite() {
                        column.push(v * scale);
                    }
                }
                master[i] = clipped_mean(&mut column, sigma);
            }
        }
        write(self, &master, sigma, out)
    }
}

/// 64 ビットの FNV-1a。どの版の vw でも、同じバイト列から同じ値になる。
pub(super) struct Fnv(pub(super) u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub(super) fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// `values` の σ クリップ平均。値が 2 つ以下なら、ただの平均。値がなければ NaN。
pub(super) fn clipped_mean(values: &mut [f32], sigma: f64) -> f32 {
    if values.is_empty() {
        return f32::NAN;
    }
    let mean = |v: &[f32]| (v.iter().map(|&x| f64::from(x)).sum::<f64>() / v.len() as f64) as f32;
    if values.len() <= 2 {
        return mean(values);
    }
    values.sort_by(f32::total_cmp);
    let median = median_of_sorted(values);
    let mut deviations: Vec<f32> = values.iter().map(|v| (v - median).abs()).collect();
    deviations.sort_by(f32::total_cmp);
    let spread = 1.4826 * median_of_sorted(&deviations);
    if spread <= 0.0 {
        // 半分以上が同じ値なら、それがいちばん確かな値。
        return median;
    }
    let limit = sigma as f32 * spread;
    let kept: Vec<f32> = values
        .iter()
        .copied()
        .filter(|v| (v - median).abs() <= limit)
        .collect();
    if kept.is_empty() { median } else { mean(&kept) }
}

fn median_of_sorted(v: &[f32]) -> f32 {
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    }
}

/// 間引いて見た中央値（明るさを揃えるためだけなので、全画素は見ない）。
fn sampled_median(v: &[f32]) -> f32 {
    let step = v.len().div_ceil(100_000).max(1);
    let mut s: Vec<f32> = v
        .iter()
        .step_by(step)
        .copied()
        .filter(|x| x.is_finite())
        .collect();
    if s.is_empty() {
        return 0.0;
    }
    s.sort_by(f32::total_cmp);
    median_of_sorted(&s)
}

/// 平均した撮影の条件と、元のファイルの一覧を書いた FITS。
fn write(group: &Group, values: &[f32], sigma: f64, out: &Path) -> Result<(), String> {
    let first = group.frames[0];
    let image = VwImage::new(
        first.width as u32,
        first.height as u32,
        ColorModel::Gray,
        PixelData::F32(values.to_vec()),
    )?;
    let name = match group.kind {
        Kind::Dark => "Master Dark",
        Kind::Flat => "Master Flat",
        Kind::Bias => "Master Bias",
        Kind::DarkFlat => "Master DarkFlat",
        Kind::Light => "Master Light",
    };
    let mean = |get: fn(&Frame) -> Option<f64>| -> Option<f64> {
        let v: Vec<f64> = group.frames.iter().filter_map(|f| get(f)).collect();
        (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
    };
    let mut cards = vec![
        ("IMAGETYP".to_owned(), name.to_owned()),
        ("NCOMBINE".to_owned(), group.frames.len().to_string()),
        ("XBINNING".to_owned(), first.binning.0.to_string()),
        ("YBINNING".to_owned(), first.binning.1.to_string()),
        ("VWSIGMA".to_owned(), format!("{sigma}")),
    ];
    let number = |v: f64| format!("{v:.6E}");
    for (key, value) in [
        ("EXPTIME", mean(|f| f.exposure)),
        ("CCD-TEMP", mean(|f| f.temperature)),
        ("GAIN", mean(|f| f.gain)),
        ("OFFSET", mean(|f| f.offset)),
    ] {
        if let Some(v) = value {
            cards.push((key.to_owned(), number(v)));
        }
    }
    if let Some(filter) = &first.filter {
        cards.push(("FILTER".to_owned(), filter.clone()));
    }
    if let Some(date) = header_value(&first.path, "DATE-OBS") {
        cards.push(("DATE-OBS".to_owned(), date));
    }
    let history: Vec<String> = std::iter::once(format!(
        "vw: sigma-clipped mean ({sigma} sigma) of {} frames",
        group.frames.len()
    ))
    .chain(
        group
            .frames
            .iter()
            .map(|f| format!("vw source: {}", f.path.display())),
    )
    .collect();
    let bytes = crate::write::encode(&image, &cards, &history)?;
    let dir = out.parent().ok_or("no folder for the master")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let partial = out.with_extension("partial");
    std::fs::write(&partial, bytes).map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(&partial, out).map_err(|e| format!("{}: {e}", out.display()))
}

fn header_value(path: &Path, key: &str) -> Option<String> {
    crate::header_of_file(path)
        .ok()
        .flatten()
        .and_then(|h| h.get(key).map(|v| v.trim().to_owned()))
}
