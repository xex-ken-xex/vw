//! スペクトルの軸（周波数・波長・速度など）の値を、FITS の WCS の論文 III（Greisen ほか 2006）の
//! 式で出す。軸は線形とは限らず、`FREQ` で線形に並べたチャンネルを `WAVE-F2W` のように波長で
//! 書いたり、`-LOG`（対数）・`-GRI`（グリズム）で書いたりする。
//!
//! 式の流れ: 画素 → 中間の座標 x（`CDELT` × 画素のずれ）→ 軸が線形な量 X → 周波数を通して
//! 書いてある種類の基本の量 P → 書いてある量 S。

use vw_core::spectrum::{Axis, AxisKind, LIGHT_M_S};

use crate::header::Hdu;

const PLANCK_J_S: f64 = 6.626_070_15e-34;
const ELECTRON_VOLT_J: f64 = 1.602_176_634e-19;

/// 4 つの基本の量。どの種類の軸も、このどれかに 1 次の式で直せる。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Basic {
    /// 周波数（Hz）。
    Frequency,
    /// 真空の波長（m）。
    Wavelength,
    /// 空気の中の波長（m）。
    AirWavelength,
    /// 相対論の視線速度（m/s）。
    Velocity,
}

impl Basic {
    fn read(letter: char) -> Option<Self> {
        Some(match letter {
            'F' => Basic::Frequency,
            'W' => Basic::Wavelength,
            'A' => Basic::AirWavelength,
            'V' => Basic::Velocity,
            _ => return None,
        })
    }
}

/// `CTYPE` の頭の 4 文字（書いてある量の種類）。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Freq,
    Ener,
    Wavn,
    Vrad,
    Wave,
    Vopt,
    Zopt,
    Awav,
    Velo,
    Beta,
}

impl Kind {
    fn read(name: &str) -> Option<Self> {
        Some(match name {
            "FREQ" => Kind::Freq,
            "ENER" => Kind::Ener,
            "WAVN" => Kind::Wavn,
            "VRAD" => Kind::Vrad,
            "WAVE" => Kind::Wave,
            "VOPT" => Kind::Vopt,
            "ZOPT" => Kind::Zopt,
            "AWAV" => Kind::Awav,
            "VELO" => Kind::Velo,
            "BETA" => Kind::Beta,
            _ => return None,
        })
    }

    /// この量に 1 次の式で直せる基本の量。
    fn basic(self) -> Basic {
        match self {
            Kind::Freq | Kind::Ener | Kind::Wavn | Kind::Vrad => Basic::Frequency,
            Kind::Wave | Kind::Vopt | Kind::Zopt => Basic::Wavelength,
            Kind::Awav => Basic::AirWavelength,
            Kind::Velo | Kind::Beta => Basic::Velocity,
        }
    }

    /// `CUNIT` の単位の値を基本の単位（SI）にする倍率。書いていない・読めない単位は、
    /// 規格の既定の SI として 1 倍。
    fn si_factor(self, unit: &str) -> f64 {
        let unit = unit.trim();
        match self {
            Kind::Freq => match unit {
                "kHz" => 1e3,
                "MHz" => 1e6,
                "GHz" => 1e9,
                "THz" => 1e12,
                _ => 1.0,
            },
            Kind::Ener => match unit {
                "eV" => ELECTRON_VOLT_J,
                "keV" => ELECTRON_VOLT_J * 1e3,
                "MeV" => ELECTRON_VOLT_J * 1e6,
                "erg" => 1e-7,
                _ => 1.0,
            },
            Kind::Wavn => match unit {
                "1/cm" | "/cm" | "cm-1" | "cm^-1" => 100.0,
                _ => 1.0,
            },
            Kind::Vrad | Kind::Vopt | Kind::Velo => match unit.to_lowercase().as_str() {
                "km/s" | "km s-1" | "km.s-1" | "km s^-1" => 1e3,
                "cm/s" => 1e-2,
                _ => 1.0,
            },
            Kind::Wave | Kind::Awav => match unit {
                "cm" => 1e-2,
                "mm" => 1e-3,
                "um" => 1e-6,
                "nm" => 1e-9,
                "Angstrom" | "angstrom" | "A" => 1e-10,
                _ => 1.0,
            },
            Kind::Zopt | Kind::Beta => 1.0,
        }
    }
}

/// 静止周波数と静止波長。どちらか一方だけ書いてあれば、もう一方はそれから出す。
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rest {
    frequency: Option<f64>,
    wavelength: Option<f64>,
}

impl Rest {
    fn read(hdu: &Hdu) -> Self {
        let positive = |a: &str, b: &str| {
            hdu.float(a)
                .or_else(|| hdu.float(b))
                .filter(|v| v.is_finite() && *v > 0.0)
        };
        Self {
            frequency: positive("RESTFRQ", "RESTFREQ"),
            wavelength: positive("RESTWAV", "RESTWAVE"),
        }
    }

    fn hz(self) -> Option<f64> {
        self.frequency.or(self.wavelength.map(|w| LIGHT_M_S / w))
    }

    fn metres(self) -> Option<f64> {
        self.wavelength.or(self.frequency.map(|f| LIGHT_M_S / f))
    }
}

/// 中間の座標 x から軸が線形な量 X への式。
#[derive(Debug, Clone, PartialEq)]
enum Algorithm {
    /// 書いてある量そのものが線形。
    Linear,
    /// `-LOG`: S = S_r exp(x / S_r)。
    Log,
    /// `-F2W` など: X が線形で、S へは周波数を通して直す。
    Through { x: Basic, x_ref: f64, dx_ds: f64 },
    /// `-GRI`・`-GRA`: グリズムの式で、X（波長）に直す。
    Grism { x: Basic, w: [f64; 5] },
    /// `-TAB`: 表の列から引く（[`crate::header::Lookup`]）。値は SI にしてある。
    Table {
        index: Option<std::sync::Arc<[f64]>>,
        coords: std::sync::Arc<[f64]>,
    },
}

/// 1 本のスペクトルの軸。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpectralAxis {
    kind: Kind,
    name: String,
    unit: String,
    algorithm: Algorithm,
    /// 基準点の値と、1 画素の幅（SI）。
    crval: f64,
    cdelt: f64,
    crpix: f64,
    rest: Rest,
    /// 速度の基準（`SPECSYS`、古い書き方では `-LSR` などから）。
    pub(crate) frame: String,
}

/// `hdu` の `axis` 本目（1 から）が、スペクトルの軸なら読む。読めない書き方や、必要な静止周波数が
/// ないとき、`-TAB` の表が見つからないときは `None`。
pub(crate) fn read(hdu: &Hdu, axis: usize) -> Option<SpectralAxis> {
    let key = |k: &str| format!("{k}{axis}");
    let ctype = hdu.get(&key("CTYPE"))?.trim().to_uppercase();
    let unit = hdu
        .get(&key("CUNIT"))
        .map(|u| u.trim().to_owned())
        .unwrap_or_default();
    let base = ctype.get(..4).unwrap_or(&ctype);
    let code = ctype.get(4..).unwrap_or_default();
    let mut frame = hdu
        .get("SPECSYS")
        .map(|s| s.trim().to_uppercase())
        .unwrap_or_default();
    let (kind, code) = match (base, code) {
        // 速度を全部書いた古い書き方。線形の相対論の速度として読む。
        ("VELO", "CITY") => (Kind::Velo, ""),
        (_, "-LSR" | "-HEL" | "-OBS") => {
            if frame.is_empty() {
                frame = match code {
                    "-LSR" => "LSRK",
                    "-HEL" => "BARYCENT",
                    _ => "TOPOCENT",
                }
                .to_owned();
            }
            // 古い書き方の VELO は、VELREF の 256 の位で電波か光学か、FELO は周波数で線形な光学の速度。
            match base {
                "VELO" if hdu.int("VELREF").unwrap_or(0) / 256 == 1 => (Kind::Vrad, ""),
                "VELO" => (Kind::Vopt, ""),
                "FELO" => (Kind::Vopt, "-F2W"),
                _ => (Kind::read(base)?, ""),
            }
        }
        ("FELO", "") => (Kind::Vopt, "-F2W"),
        _ => (Kind::read(base)?, code),
    };
    let factor = kind.si_factor(&unit);
    let lookup = code.trim_start_matches('-').trim() == "TAB";
    // `-TAB` の CRVAL と CDELT は表の番号の上の値で、単位は表の値の方に付く。
    let scale = if lookup { 1.0 } else { factor };
    let crval = hdu.float(&key("CRVAL")).unwrap_or(0.0) * scale;
    let cdelt = hdu
        .float(&key("CDELT"))
        .or_else(|| hdu.float(&format!("CD{axis}_{axis}")))
        .unwrap_or(1.0)
        * scale;
    let rest = Rest::read(hdu);
    let mut spectral = SpectralAxis {
        kind,
        name: base.to_owned(),
        unit,
        algorithm: Algorithm::Linear,
        crval,
        cdelt,
        crpix: hdu.float(&key("CRPIX")).unwrap_or(1.0),
        rest,
        frame,
    };
    spectral.algorithm = match code.trim_start_matches('-').trim() {
        "" => Algorithm::Linear,
        "LOG" => Algorithm::Log,
        "TAB" => {
            let table = hdu.lookups.iter().find(|(a, _)| *a == axis)?;
            Algorithm::Table {
                index: table.1.index.as_deref().map(Into::into),
                coords: table.1.coords.iter().map(|v| v * factor).collect(),
            }
        }
        "GRI" | "GRA" => {
            let x = if code.ends_with('I') {
                Basic::Wavelength
            } else {
                Basic::AirWavelength
            };
            let (x_ref, dx_ds) = spectral.reference_in(x)?;
            let pv = |m: u32, default: f64| hdu.float(&format!("PV{axis}_{m}")).unwrap_or(default);
            grism(
                x,
                x_ref,
                dx_ds,
                [0, 1, 2, 3, 4, 5, 6].map(|m| pv(m, if m == 3 { 1.0 } else { 0.0 })),
            )?
        }
        c if c.len() == 3 && &c[1..2] == "2" => {
            let mut letters = c.chars();
            let x = Basic::read(letters.next()?)?;
            let p = Basic::read(letters.nth(1)?)?;
            if p != kind.basic() {
                return None;
            }
            if x == p {
                Algorithm::Linear
            } else {
                let (x_ref, dx_ds) = spectral.reference_in(x)?;
                Algorithm::Through { x, x_ref, dx_ds }
            }
        }
        _ => return None,
    };
    Some(spectral)
}

/// グリズムの式の係数（wcslib と同じ並び）。`pv` は `PVi_0`〜`PVi_6`: 刻線の密度 G、次数 m、
/// 入射角 α、屈折率 n_r、その波長の傾き n'_r、傾き ε、カメラの角度 θ（角度は度）。
fn grism(x: Basic, lambda_r: f64, dx_ds: f64, pv: [f64; 7]) -> Option<Algorithm> {
    let [g, m, alpha, n_r, dn_r, epsilon, theta] = pv;
    let (alpha, epsilon, theta) = (alpha.to_radians(), epsilon.to_radians(), theta.to_radians());
    let mut t = g * m / epsilon.cos();
    let s = t * lambda_r - n_r * alpha.sin();
    if s.abs() > 1.0 {
        return None;
    }
    let beta_r = s.asin();
    t -= dn_r * alpha.sin();
    if t == 0.0 {
        return None;
    }
    Some(Algorithm::Grism {
        x,
        w: [
            -theta.tan(),
            dx_ds * t / (beta_r.cos() * theta.cos() * theta.cos()),
            beta_r + theta,
            (n_r - dn_r * lambda_r) * alpha.sin(),
            1.0 / t,
        ],
    })
}

impl SpectralAxis {
    /// 基準点の値を X の量にしたものと、そこでの dX/dS。
    ///
    /// 微分は 1 チャンネルの幅の中心差分を、半分の幅の差分と組み合わせて（リチャードソンの外挿）
    /// 出す。12 通りの組の式を書き分けるより短く、論文の式の値と 10 桁ほど合う（テストで確かめて
    /// いる）。幅をもっと細かくすると、X の値の桁落ちの方が大きくなる。
    fn reference_in(&self, x: Basic) -> Option<(f64, f64)> {
        let to_x = |s: f64| {
            let p = s_to_p(self.kind, s, self.rest)?;
            from_frequency(x, to_frequency(self.kind.basic(), p, self.rest)?, self.rest)
        };
        let h = self.cdelt.abs().max(self.crval.abs() * 1e-9);
        if h == 0.0 {
            return None;
        }
        let x_ref = to_x(self.crval)?;
        let slope = |h: f64| -> Option<f64> {
            Some((to_x(self.crval + h)? - to_x(self.crval - h)?) / (2.0 * h))
        };
        let dx_ds = (4.0 * slope(h / 2.0)? - slope(h)?) / 3.0;
        (x_ref.is_finite() && dx_ds.is_finite() && dx_ds != 0.0).then_some((x_ref, dx_ds))
    }

    /// `index` 番目（0 から、小数も可）の点の値（SI）。
    fn si_value(&self, index: f64) -> Option<f64> {
        let x = self.cdelt * (index + 1.0 - self.crpix);
        let from_x = |basic: Basic, value: f64| {
            let p = from_frequency(
                self.kind.basic(),
                to_frequency(basic, value, self.rest)?,
                self.rest,
            )?;
            p_to_s(self.kind, p, self.rest)
        };
        let v = match &self.algorithm {
            Algorithm::Linear => Some(self.crval + x),
            Algorithm::Log => (self.crval != 0.0).then(|| self.crval * (x / self.crval).exp()),
            Algorithm::Through {
                x: basic,
                x_ref,
                dx_ds,
            } => from_x(*basic, x_ref + dx_ds * x),
            Algorithm::Grism { x: basic, w } => {
                let beta = (w[0] + x * w[1]).atan() + w[2];
                from_x(*basic, (beta.sin() + w[3]) * w[4])
            }
            Algorithm::Table { index, coords } => {
                let psi = self.crval + x;
                let at = match index {
                    Some(index) => position_in(index, psi)?,
                    None => psi,
                };
                interpolate(coords, at)
            }
        }?;
        v.is_finite().then_some(v)
    }

    /// `index` 番目の点の、見せる単位の値。周波数・速度・波長は SI、それ以外は `CUNIT` の単位。
    pub(crate) fn value(&self, index: f64) -> Option<f64> {
        let v = self.si_value(index)?;
        Some(match self.axis_kind() {
            AxisKind::Other { .. } => v / self.kind.si_factor(&self.unit),
            _ => v,
        })
    }

    pub(crate) fn axis_kind(&self) -> AxisKind {
        match self.kind {
            Kind::Freq => AxisKind::Frequency,
            Kind::Vrad | Kind::Vopt | Kind::Velo => AxisKind::Velocity,
            Kind::Wave | Kind::Awav => AxisKind::Wavelength,
            Kind::Ener | Kind::Wavn | Kind::Zopt | Kind::Beta => AxisKind::Other {
                name: self.name.clone(),
                unit: self.unit.clone(),
            },
        }
    }

    /// `n` 点の軸。値の出せない点（式の外）は NaN。
    pub(crate) fn axis(&self, n: usize) -> Axis {
        // 空気の中の波長の軸は、静止波長（規格では真空）も空気の中に直して渡す。真空のまま
        // 比べると、屈折の分（Hα で約 80 km/s）だけ速度がずれる。
        let (rest_frequency, rest_wavelength) = match self.kind {
            Kind::Awav => (None, self.rest.metres().map(air_of_vacuum)),
            _ => (self.rest.frequency, self.rest.wavelength),
        };
        Axis {
            kind: self.axis_kind(),
            values: (0..n)
                .map(|i| self.value(i as f64).unwrap_or(f64::NAN))
                .collect(),
            rest_frequency,
            rest_wavelength,
            frame: self.frame.clone(),
        }
    }
}

/// 1 から数えた番号 `at`（小数も可）の値。点の間は直線でつなぎ、両端の外は半点まで延ばす。
fn interpolate(values: &[f64], at: f64) -> Option<f64> {
    let n = values.len();
    if n == 0 || !(0.5..=n as f64 + 0.5).contains(&at) {
        return None;
    }
    if n == 1 {
        return Some(values[0]);
    }
    let k = ((at - 1.0).floor() as isize).clamp(0, n as isize - 2) as usize;
    let t = at - 1.0 - k as f64;
    Some(values[k] + (values[k + 1] - values[k]) * t)
}

/// 単調に並んだ `index` の中で、値が `value` になる所の番号（1 から、小数も可）。
fn position_in(index: &[f64], value: f64) -> Option<f64> {
    let n = index.len();
    if n == 1 {
        return (index[0] == value).then_some(1.0);
    }
    (0..n - 1).find_map(|k| {
        let (a, b) = (index[k], index[k + 1]);
        let inside = (a <= value && value <= b) || (b <= value && value <= a);
        (inside && a != b).then(|| k as f64 + 1.0 + (value - a) / (b - a))
    })
}

/// 書いてある量 S → 基本の量 P。
fn s_to_p(kind: Kind, s: f64, rest: Rest) -> Option<f64> {
    let c = LIGHT_M_S;
    Some(match kind {
        Kind::Freq | Kind::Wave | Kind::Awav | Kind::Velo => s,
        Kind::Ener => s / PLANCK_J_S,
        Kind::Wavn => s * c,
        Kind::Vrad => rest.hz()? * (1.0 - s / c),
        Kind::Vopt => rest.metres()? * (1.0 + s / c),
        Kind::Zopt => rest.metres()? * (1.0 + s),
        Kind::Beta => s * c,
    })
}

/// 基本の量 P → 書いてある量 S。
fn p_to_s(kind: Kind, p: f64, rest: Rest) -> Option<f64> {
    let c = LIGHT_M_S;
    Some(match kind {
        Kind::Freq | Kind::Wave | Kind::Awav | Kind::Velo => p,
        Kind::Ener => p * PLANCK_J_S,
        Kind::Wavn => p / c,
        Kind::Vrad => c * (1.0 - p / rest.hz()?),
        Kind::Vopt => c * (p / rest.metres()? - 1.0),
        Kind::Zopt => p / rest.metres()? - 1.0,
        Kind::Beta => p / c,
    })
}

fn to_frequency(basic: Basic, value: f64, rest: Rest) -> Option<f64> {
    let c = LIGHT_M_S;
    let nu = match basic {
        Basic::Frequency => value,
        Basic::Wavelength => c / value,
        Basic::AirWavelength => c / vacuum_of_air(value),
        Basic::Velocity => {
            if value.abs() >= c {
                return None;
            }
            rest.hz()? * ((c - value) / (c + value)).sqrt()
        }
    };
    nu.is_finite().then_some(nu)
}

fn from_frequency(basic: Basic, nu: f64, rest: Rest) -> Option<f64> {
    let c = LIGHT_M_S;
    let v = match basic {
        Basic::Frequency => nu,
        Basic::Wavelength => c / nu,
        Basic::AirWavelength => air_of_vacuum(c / nu),
        Basic::Velocity => {
            let nu0 = rest.hz()?;
            c * (nu0 * nu0 - nu * nu) / (nu0 * nu0 + nu * nu)
        }
    };
    v.is_finite().then_some(v)
}

/// 空気の屈折率（論文 III の式 65、波長は m）。`s` は波数の 2 乗（m⁻²）。
fn air_index(s: f64) -> f64 {
    1.000_064_328 + 2.554e8 / (0.41e14 - s) + 294.981e8 / (1.46e14 - s)
}

fn vacuum_of_air(air: f64) -> f64 {
    air * air_index(1.0 / (air * air))
}

/// 真空の波長 → 空気の中の波長。屈折率が空気の中の波長の式なので、4 回で詰める（wcslib と同じ）。
fn air_of_vacuum(vacuum: f64) -> f64 {
    let mut n = 1.0;
    for _ in 0..4 {
        let k = n / vacuum;
        n = air_index(k * k);
    }
    vacuum / n
}
