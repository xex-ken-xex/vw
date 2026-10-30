//! 色の決め方（ICC プロファイルと CICP）をメタデータ欄に出し、画像の色域を求める。
//!
//! vw は画素の値を色の変換をせずに sRGB として出す。sRGB より広い色域（Display P3 や
//! Adobe RGB など）の画像では、鮮やかな色がくすんで見える。それを知らせるために色域を求める。

use moxcms::{ColorProfile, DataColorSpace, ProfileText, ToneReprCurve};

use crate::Metadata;

/// メタデータ欄の見出し。
pub const GROUP: &str = "Color profile";

/// 画像の色域。赤・緑・青を 1 にしたときの色（D50 に順応した XYZ）と、値から光の量へ戻す
/// ガンマ（曲線を 1 つのべき乗で近似したもの）。
#[derive(Debug, Clone, PartialEq)]
pub struct Gamut {
    pub name: String,
    /// 列が赤・緑・青の XYZ（D50）。
    pub to_xyz: [[f32; 3]; 3],
    pub gamma: f32,
}

impl Gamut {
    /// sRGB より広く、sRGB で出せない色を持てる。
    pub fn wider_than_srgb(&self) -> bool {
        (0..3).any(|c| {
            let primary = [self.to_xyz[0][c], self.to_xyz[1][c], self.to_xyz[2][c]];
            xyz_to_linear_srgb(primary).iter().any(|&v| v < -0.01)
        })
    }

    /// 画素の値（0〜1）を、sRGB の光の量（範囲外の色は 0 未満か 1 超）にする。
    pub fn to_linear_srgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        let linear = rgb.map(|v| v.max(0.0).powf(self.gamma));
        let m = &self.to_xyz;
        let xyz =
            [0, 1, 2].map(|r| m[r][0] * linear[0] + m[r][1] * linear[1] + m[r][2] * linear[2]);
        xyz_to_linear_srgb(xyz)
    }
}

/// D50 の XYZ から、光の量の sRGB へ（Bradford で D65 に順応。Lindbloom の表の値）。
fn xyz_to_linear_srgb(xyz: [f32; 3]) -> [f32; 3] {
    const M: [[f32; 3]; 3] = [
        [3.133_856, -1.616_867, -0.490_615],
        [-0.978_768, 1.916_142, 0.033_454],
        [0.071_945, -0.228_991, 1.405_243],
    ];
    M.map(|row| row[0] * xyz[0] + row[1] * xyz[1] + row[2] * xyz[2])
}

/// よく使う色域。名前は表示時に訳さない固有名。
pub fn known() -> Vec<Gamut> {
    const D65: [f64; 2] = [0.3127, 0.3290];
    const D50: [f64; 2] = [0.3457, 0.3585];
    let make = |name: &str, primaries: [[f64; 2]; 3], white: [f64; 2], gamma: f32| Gamut {
        name: name.to_owned(),
        to_xyz: rgb_to_xyz_d50(primaries, white),
        gamma,
    };
    vec![
        make("sRGB", [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]], D65, 2.2),
        make(
            "Display P3",
            [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
            D65,
            2.2,
        ),
        make(
            "Adobe RGB (1998)",
            [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]],
            D65,
            2.2,
        ),
        make(
            "ProPhoto RGB",
            [[0.7347, 0.2653], [0.1596, 0.8404], [0.0366, 0.0001]],
            D50,
            1.8,
        ),
        make(
            "BT.2020",
            [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
            D65,
            2.4,
        ),
    ]
}

/// 原色と白の色度から、D50 に順応した RGB → XYZ の行列（Lindbloom の求め方と Bradford の順応）。
fn rgb_to_xyz_d50(primaries: [[f64; 2]; 3], white: [f64; 2]) -> [[f32; 3]; 3] {
    let xyz = |[x, y]: [f64; 2]| [x / y, 1.0, (1.0 - x - y) / y];
    let cols = primaries.map(xyz);
    let p = [0, 1, 2].map(|r| [cols[0][r], cols[1][r], cols[2][r]]);
    let s = mul_vec(&invert(&p), xyz(white));
    let m = [0, 1, 2].map(|r| [0, 1, 2].map(|c| p[r][c] * s[c]));
    let adapt = bradford(xyz(white), xyz([0.3457, 0.3585]));
    let out = mul(&adapt, &m);
    out.map(|row| row.map(|v| v as f32))
}

fn bradford(from: [f64; 3], to: [f64; 3]) -> [[f64; 3]; 3] {
    const B: [[f64; 3]; 3] = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let (s, d) = (mul_vec(&B, from), mul_vec(&B, to));
    let scale = [
        [d[0] / s[0], 0.0, 0.0],
        [0.0, d[1] / s[1], 0.0],
        [0.0, 0.0, d[2] / s[2]],
    ];
    mul(&invert(&B), &mul(&scale, &B))
}

fn mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    [0, 1, 2].map(|r| [0, 1, 2].map(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

fn mul_vec(a: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|r| (0..3).map(|k| a[r][k] * v[k]).sum())
}

fn invert(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let c =
        |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ]
    .map(|row| row.map(|v| v / det))
}

/// 知っている色域のうち、`to_xyz` がほぼ同じもの。
fn name_of(to_xyz: &[[f32; 3]; 3]) -> Option<String> {
    known()
        .into_iter()
        .find(|g| {
            g.to_xyz
                .iter()
                .flatten()
                .zip(to_xyz.iter().flatten())
                .all(|(a, b)| (a - b).abs() < 0.006)
        })
        .map(|g| g.name)
}

/// ICC プロファイルを読み、分かったことを [`GROUP`] に足す。RGB の行列のプロファイルなら色域を返す。
pub fn push_profile(m: &mut Metadata, icc: &[u8]) -> Option<Gamut> {
    let profile = match ColorProfile::new_from_slice(icc) {
        Ok(p) => p,
        Err(e) => {
            m.push(
                GROUP,
                "ICC profile",
                format!("unreadable ({} bytes, {e:?})", icc.len()),
            );
            return None;
        }
    };
    if let Some(d) = profile.description.as_ref().and_then(text) {
        m.push(GROUP, "Description", d);
    }
    let v = profile.version() as u32;
    m.push(
        GROUP,
        "ICC version",
        format!("{}.{}", v >> 24, (v >> 20) & 0xF),
    );
    m.push(
        GROUP,
        "Profile class",
        format!("{:?}", profile.profile_class),
    );
    m.push(GROUP, "Color space", format!("{:?}", profile.color_space));
    m.push(
        GROUP,
        "Rendering intent",
        format!("{:?}", profile.rendering_intent),
    );
    let w = profile.media_white_point.unwrap_or(profile.white_point);
    if w.y > 0.0 {
        let sum = w.x + w.y + w.z;
        m.push(
            GROUP,
            "White point",
            format!("x {:.4}, y {:.4}", w.x / sum, w.y / sum),
        );
    }
    if let Some(cicp) = &profile.cicp {
        push_cicp(
            m,
            GROUP,
            [
                cicp.color_primaries as u8,
                cicp.transfer_characteristics as u8,
                cicp.matrix_coefficients as u8,
            ],
            cicp.full_range,
        );
    }
    if let Some(trc) = profile.red_trc.as_ref().or(profile.gray_trc.as_ref()) {
        m.push(GROUP, "Tone curve", describe_trc(trc));
    }
    if profile.color_space != DataColorSpace::Rgb {
        return None;
    }
    let cols = [
        profile.red_colorant,
        profile.green_colorant,
        profile.blue_colorant,
    ];
    if cols.iter().all(|c| c.y == 0.0) {
        m.push(GROUP, "Primaries", "lookup table (not a matrix profile)");
        return None;
    }
    let to_xyz = [0, 1, 2].map(|r| {
        cols.map(|c| match r {
            0 => c.x as f32,
            1 => c.y as f32,
            _ => c.z as f32,
        })
    });
    let gamma = profile.red_trc.as_ref().map_or(2.2, approximate_gamma);
    let gamut = Gamut {
        name: name_of(&to_xyz).unwrap_or_else(|| "Custom".to_owned()),
        to_xyz,
        gamma,
    };
    push_gamut(m, &gamut);
    Some(gamut)
}

/// ICC プロファイルの色域だけを求める（メタデータは作らない）。
pub fn gamut(icc: &[u8]) -> Option<Gamut> {
    push_profile(&mut Metadata::default(), icc)
}

/// 色域の名前と、sRGB より広いときの注意を足す。
pub fn push_gamut(m: &mut Metadata, gamut: &Gamut) {
    m.push(GROUP, "Primaries", &gamut.name);
    if gamut.wider_than_srgb() {
        m.push(
            GROUP,
            "Gamut",
            "wider than sRGB: vivid colors look duller (shown as sRGB without converting; Shift+O marks them)",
        );
    }
}

/// CICP（ITU-T H.273 の番号で色の決め方を表す。PNG の cICP、ICC の cicp タグ）を足す。
pub fn push_cicp(
    m: &mut Metadata,
    group: &str,
    [primaries, transfer, matrix]: [u8; 3],
    full_range: bool,
) {
    let primaries_name = match primaries {
        1 => "BT.709 / sRGB",
        4 => "BT.470 M",
        5 => "BT.601 (625 lines)",
        6 => "BT.601 (525 lines)",
        7 => "SMPTE 240M",
        8 => "Generic film",
        9 => "BT.2020 / BT.2100",
        10 => "SMPTE ST 428 (XYZ)",
        11 => "DCI-P3 (SMPTE RP 431-2)",
        12 => "Display P3 (SMPTE EG 432-1)",
        22 => "EBU Tech 3213",
        _ => "",
    };
    let transfer_name = match transfer {
        1 => "BT.709",
        4 => "Gamma 2.2",
        5 => "Gamma 2.8",
        6 => "BT.601",
        7 => "SMPTE 240M",
        8 => "Linear",
        11 => "IEC 61966-2-4 (xvYCC)",
        13 => "sRGB",
        14 => "BT.2020 (10 bit)",
        15 => "BT.2020 (12 bit)",
        16 => "PQ (SMPTE ST 2084, HDR)",
        17 => "SMPTE ST 428",
        18 => "HLG (ARIB STD-B67, HDR)",
        _ => "",
    };
    let matrix_name = match matrix {
        0 => "Identity (RGB)",
        1 => "BT.709",
        5 | 6 => "BT.601",
        9 => "BT.2020 (non-constant luminance)",
        10 => "BT.2020 (constant luminance)",
        _ => "",
    };
    let named = |code: u8, name: &str| {
        if name.is_empty() {
            code.to_string()
        } else {
            format!("{name} ({code})")
        }
    };
    m.push(group, "CICP primaries", named(primaries, primaries_name));
    m.push(group, "CICP transfer", named(transfer, transfer_name));
    m.push(group, "CICP matrix", named(matrix, matrix_name));
    m.push(
        group,
        "CICP range",
        if full_range { "full" } else { "limited" },
    );
}

/// CICP の番号から色域を求める。HDR の曲線（PQ・HLG）はべき乗で近似できないので `None`。
pub fn cicp_gamut(primaries: u8, transfer: u8) -> Option<Gamut> {
    let gamma = match transfer {
        1 | 6 | 14 | 15 => 2.4,
        4 | 13 => 2.2,
        5 => 2.8,
        8 => 1.0,
        _ => return None,
    };
    let name = match primaries {
        1 => "sRGB",
        12 => "Display P3",
        9 => "BT.2020",
        _ => return None,
    };
    known()
        .into_iter()
        .find(|g| g.name == name)
        .map(|g| Gamut { gamma, ..g })
}

fn text(t: &ProfileText) -> Option<String> {
    let s = match t {
        ProfileText::PlainString(s) => s.clone(),
        ProfileText::Localizable(list) => list
            .iter()
            .find(|l| l.language == "en")
            .or(list.first())?
            .value
            .clone(),
        ProfileText::Description(d) => d.ascii_string.clone(),
    };
    let s = s.trim_matches(char::from(0)).trim().to_owned();
    (!s.is_empty()).then_some(s)
}

/// 曲線の値（0〜1 の入力で）。ICC の parametricCurveType の 5 つの式。
fn eval_trc(trc: &ToneReprCurve, x: f32) -> f32 {
    match trc {
        ToneReprCurve::Lut(t) if t.is_empty() => x,
        ToneReprCurve::Lut(t) if t.len() == 1 => x.powf(f32::from(t[0]) / 256.0),
        ToneReprCurve::Lut(t) => {
            let pos = x.clamp(0.0, 1.0) * (t.len() - 1) as f32;
            let i = (pos as usize).min(t.len() - 2);
            let f = pos - i as f32;
            (f32::from(t[i]) * (1.0 - f) + f32::from(t[i + 1]) * f) / 65535.0
        }
        ToneReprCurve::Parametric(p) => {
            let g = p.first().copied().unwrap_or(1.0);
            let at = |i: usize| p.get(i).copied().unwrap_or(0.0);
            let (a, b, c, d, e, f) = (at(1), at(2), at(3), at(4), at(5), at(6));
            let pow = |v: f32| v.max(0.0).powf(g);
            match p.len() {
                1 => pow(x),
                3 => {
                    if x >= -b / a {
                        pow(a * x + b)
                    } else {
                        0.0
                    }
                }
                4 => {
                    if x >= -b / a {
                        pow(a * x + b) + c
                    } else {
                        c
                    }
                }
                5 => {
                    if x >= d {
                        pow(a * x + b)
                    } else {
                        c * x
                    }
                }
                _ => {
                    if x >= d {
                        pow(a * x + b) + e
                    } else {
                        c * x + f
                    }
                }
            }
        }
    }
}

/// 曲線を 1 つのべき乗で近似したときのガンマ（真ん中の値が合うように）。
fn approximate_gamma(trc: &ToneReprCurve) -> f32 {
    let y = eval_trc(trc, 0.5);
    if y <= 0.0 || y >= 1.0 {
        return 1.0;
    }
    y.ln() / 0.5f32.ln()
}

fn describe_trc(trc: &ToneReprCurve) -> String {
    let srgb = (eval_trc(trc, 0.5) - 0.214).abs() < 0.002
        && (eval_trc(trc, 0.02) - 0.001_548).abs() < 0.0002;
    match trc {
        _ if srgb => "sRGB".to_owned(),
        ToneReprCurve::Lut(t) if t.is_empty() => "Linear".to_owned(),
        ToneReprCurve::Lut(t) if t.len() == 1 => format!("Gamma {:.2}", f32::from(t[0]) / 256.0),
        ToneReprCurve::Parametric(p) if p.len() == 1 => format!("Gamma {:.2}", p[0]),
        ToneReprCurve::Lut(t) => format!(
            "{}-point curve (about gamma {:.2})",
            t.len(),
            approximate_gamma(trc)
        ),
        ToneReprCurve::Parametric(_) => format!(
            "Parametric curve (about gamma {:.2})",
            approximate_gamma(trc)
        ),
    }
}

#[cfg(test)]
mod tests;
