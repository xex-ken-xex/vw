//! 電波の画像やキューブに多い書き方を読む。キューブの面が何の値か（周波数・速度・偏光）と、
//! ビーム（分解能）の楕円。

use vw_core::Metadata;

use crate::header::Hdu;
use crate::sky::{cd_matrix, number};

/// 光の速さ（km/s）。周波数から電波の定義の速度を出すのに使う。
const LIGHT_KM_S: f64 = 299_792.458;

/// キューブの `frame` 枚目の面が、3 次元目以降の軸でどこにあたるかの文。
/// 長さ 1 の軸は書かない。軸の値が読めなければ `None`。
pub(crate) fn plane_text(hdu: &Hdu, extra_axes: &[usize], frame: usize) -> Option<String> {
    let mut rest = frame;
    let mut parts = Vec::new();
    for (k, &n) in extra_axes.iter().enumerate() {
        let index = rest % n.max(1);
        rest /= n.max(1);
        if n <= 1 {
            continue;
        }
        if let Some(text) = axis_text(hdu, k + 3, index) {
            parts.push(text);
        }
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// 軸 `axis`（3 以上）の `index` 番目（0 から）の値。
fn axis_text(hdu: &Hdu, axis: usize, index: usize) -> Option<String> {
    let key = |k: &str| format!("{k}{axis}");
    let ctype = hdu.get(&key("CTYPE"))?.trim().to_uppercase();
    let unit = hdu
        .get(&key("CUNIT"))
        .map(|u| u.trim().to_owned())
        .unwrap_or_default();
    let crval = hdu.float(&key("CRVAL")).unwrap_or(0.0);
    let crpix = hdu.float(&key("CRPIX")).unwrap_or(1.0);
    let cdelt = hdu
        .float(&key("CDELT"))
        .or_else(|| hdu.float(&format!("CD{axis}_{axis}")))
        .unwrap_or(1.0);
    let value = crval + (index as f64 + 1.0 - crpix) * cdelt;
    let kind = ctype.split('-').next().unwrap_or_default();
    Some(match kind {
        "STOKES" => stokes(value),
        "FREQ" => {
            let hz = value * unit_factor(&unit, "Hz");
            let rest = hdu.float("RESTFRQ").or_else(|| hdu.float("RESTFREQ"));
            match rest.filter(|f| *f > 0.0) {
                Some(f0) => format!(
                    "{}, {:.2} km/s",
                    frequency(hz),
                    LIGHT_KM_S * (1.0 - hz / f0)
                ),
                None => frequency(hz),
            }
        }
        "VRAD" | "VOPT" | "VELO" | "FELO" | "VELOCITY" => {
            let m_s = value * unit_factor(&unit, "m/s");
            format!("{:.2} km/s", m_s / 1000.0)
        }
        _ => {
            let unit = if unit.is_empty() {
                String::new()
            } else {
                format!(" {unit}")
            };
            format!("{ctype} {}{unit}", short(value))
        }
    })
}

/// `unit` を基本の単位 `base`（Hz か m/s）にする倍率。書いていなければ規格の既定（基本の単位）。
fn unit_factor(unit: &str, base: &str) -> f64 {
    match (base, unit.trim()) {
        ("Hz", "kHz") => 1e3,
        ("Hz", "MHz") => 1e6,
        ("Hz", "GHz") => 1e9,
        ("m/s", "km/s") | ("m/s", "KM/S") => 1e3,
        _ => 1.0,
    }
}

fn frequency(hz: f64) -> String {
    if hz.abs() >= 1e9 {
        format!("{:.6} GHz", hz / 1e9)
    } else if hz.abs() >= 1e6 {
        format!("{:.4} MHz", hz / 1e6)
    } else {
        format!("{} Hz", short(hz))
    }
}

/// FITS の偏光の番号（1〜4 が I Q U V、-1〜-8 が円・直線の偏波の組）。
fn stokes(value: f64) -> String {
    let name = match value.round() as i64 {
        1 => "I",
        2 => "Q",
        3 => "U",
        4 => "V",
        -1 => "RR",
        -2 => "LL",
        -3 => "RL",
        -4 => "LR",
        -5 => "XX",
        -6 => "YY",
        -7 => "XY",
        -8 => "YX",
        _ => return format!("Stokes {}", short(value)),
    };
    format!("Stokes {name}")
}

fn short(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e12 {
        format!("{v:.0}")
    } else {
        format!("{v:.6}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

/// ビームの楕円。`major` と `minor` は中心から楕円の端までの向きと長さ（画素、x は右、y は FITS の
/// 行の増える向き）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beam {
    pub major: [f64; 2],
    pub minor: [f64; 2],
    /// 長径・短径（秒角）と、北から東へ測った長径の向き（度）。札に出す。
    pub bmaj_arcsec: f64,
    pub bmin_arcsec: f64,
    pub bpa_deg: f64,
}

/// メタデータの `BMAJ`・`BMIN`・`BPA`（度）と WCS から、ビームの楕円を画素で出す。
/// 画像の HDU のカード（後ろの見出し）を先に見る。どれかが欠ければ `None`。
pub fn beam(meta: &Metadata) -> Option<Beam> {
    let get = |key: &str| -> Option<f64> {
        meta.entries
            .iter()
            .rev()
            .find(|(g, k, _)| g.starts_with(crate::GROUP) && k == key)
            .and_then(|(_, _, v)| number(v))
    };
    let (bmaj, bmin) = (get("BMAJ")?, get("BMIN")?);
    if bmaj <= 0.0 || bmin <= 0.0 {
        return None;
    }
    let bpa = get("BPA").unwrap_or(0.0);
    let cd = cd_matrix(&get)?;
    // 投影面の (東, 北) の向きを、CD の逆行列で画素の向きにする。
    let det = cd[0][0] * cd[1][1] - cd[0][1] * cd[1][0];
    let to_pixels = |east: f64, north: f64| {
        [
            (cd[1][1] * east - cd[0][1] * north) / det,
            (-cd[1][0] * east + cd[0][0] * north) / det,
        ]
    };
    let pa = bpa.to_radians();
    let (half_major, half_minor) = (bmaj / 2.0, bmin / 2.0);
    Some(Beam {
        major: to_pixels(half_major * pa.sin(), half_major * pa.cos()),
        minor: to_pixels(half_minor * pa.cos(), -half_minor * pa.sin()),
        bmaj_arcsec: bmaj * 3600.0,
        bmin_arcsec: bmin * 3600.0,
        bpa_deg: bpa,
    })
}
