//! 画像が空のどこを写したか。ヘッダの WCS（はっきり）か、架台の向き（およそ）から決める。
//!
//! WCS は天頂投影のうち、撮影と公開データでほぼすべてを占める TAN と、SIN・ARC・STG・ZEA を
//! 読む。`-SIP` などの歪みの補正は、星図に枠を描くには差が見えないほど小さいので当てない。
//! CAR や AIT のような全天の投影は、1 枚の写真には使われないので読まない。

use std::f64::consts::PI;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use crate::header::{self, Hdu, BLOCK};

/// 写った範囲。座標は赤経・赤緯（度）。
#[derive(Debug, Clone, PartialEq)]
pub struct Sky {
    pub center: [f64; 2],
    /// 写った範囲の縁を一回りした点。分からなければ空（中心だけ分かる）。
    pub outline: Vec<[f64; 2]>,
    /// ヘッダの WCS から求めたか。`false` なら架台の向きと焦点距離からの見積もり。
    pub certain: bool,
}

/// 縁の 1 辺を何等分して点を取るか。広い画角で縁が曲がって見えるように。
const EDGE_STEPS: usize = 8;

/// ヘッダの値を `lookup` で引いて、写った範囲を求める。どちらも分からなければ `None`。
///
/// `lookup` はカードの名前から値（ファイルに書いてある文字のまま）を返す。表示する画像の
/// HDU を先に、Primary を後に引くようにしておく（研究データは WCS を画像の HDU に、
/// 撮影ソフトは架台の向きを Primary に書くため）。
pub fn sky(lookup: &dyn Fn(&str) -> Option<String>) -> Option<Sky> {
    let num = |key: &str| number(&lookup(key)?);
    let size = ["ZNAXIS", "NAXIS"].iter().find_map(|prefix| {
        let w = num(&format!("{prefix}1"))?;
        let h = num(&format!("{prefix}2"))?;
        (w >= 1.0 && h >= 1.0).then_some([w, h])
    })?;
    if let Some(wcs) = Wcs::read(&lookup) {
        let outline = outline(size, &|x, y| wcs.to_sky(x, y));
        let center = wcs.to_sky((size[0] + 1.0) / 2.0, (size[1] + 1.0) / 2.0)?;
        if outline.len() == 4 * EDGE_STEPS {
            return Some(Sky {
                center,
                outline,
                certain: true,
            });
        }
    }
    let center = pointing(&lookup)?;
    let outline = match field_scale(&lookup) {
        Some([sx, sy]) => {
            // 回転は分からないので北を上（赤経は左が東）にする。
            let wcs = Wcs {
                crpix: [(size[0] + 1.0) / 2.0, (size[1] + 1.0) / 2.0],
                crval: center,
                cd: [[-sx, 0.0], [0.0, sy]],
                projection: Projection::Tan,
                lonpole: 180.0,
            };
            outline(size, &|x, y| wcs.to_sky(x, y))
        }
        None => Vec::new(),
    };
    Some(Sky {
        center,
        outline,
        certain: false,
    })
}

/// `reference` の画素 (x, y)（FITS の 1 始まり）に写っている空の点が、`other` の画像では
/// どの画素に写っているかを、`[[a, b, c], [d, e, f]]`（x' = a x + b y + c、y' = d x + e y + f）
/// で返す。どちらかに WCS がなければ `None`。
///
/// 色ごとに撮った画像を重ねるときの位置合わせに使う。ずれ・回転・倍率の違いを、`reference` の
/// 画像の中で 1 次の式に近づける。画素ごとに空の座標を解くと、大きな画像では重すぎるため。
/// 1 枚の写真の範囲では投影のゆがみは小さく、1 次の式で 0.1 画素も外れない。
pub fn pixel_map(
    reference: &dyn Fn(&str) -> Option<String>,
    other: &dyn Fn(&str) -> Option<String>,
) -> Option<[[f64; 3]; 2]> {
    let num = |key: &str| number(&reference(key)?);
    let size = ["ZNAXIS", "NAXIS"]
        .iter()
        .find_map(|prefix| Some([num(&format!("{prefix}1"))?, num(&format!("{prefix}2"))?]))?;
    let center = [(size[0] + 1.0) / 2.0, (size[1] + 1.0) / 2.0];
    let (from, to) = (Wcs::read(reference)?, Wcs::read(other)?);
    // 中心から縦横に画像の 4 分の 1 ずつ離れた点で、1 次の式の傾きを決める。
    let step = (size[0].max(size[1]) / 4.0).max(1.0);
    let mut guess = center;
    let mut map = |x: f64, y: f64| -> Option<[f64; 2]> {
        let p = pixel_of(&to, from.to_sky(x, y)?, guess)?;
        guess = p;
        Some(p)
    };
    let o = map(center[0], center[1])?;
    let ox = map(center[0] + step, center[1])?;
    let oy = map(center[0], center[1] + step)?;
    let (a, d) = ((ox[0] - o[0]) / step, (ox[1] - o[1]) / step);
    let (b, e) = ((oy[0] - o[0]) / step, (oy[1] - o[1]) / step);
    Some([
        [a, b, o[0] - a * center[0] - b * center[1]],
        [d, e, o[1] - d * center[0] - e * center[1]],
    ])
}

/// 空の点 `target`（赤経・赤緯、度）が `wcs` の画像で写る画素。`guess` から探す。
fn pixel_of(wcs: &Wcs, target: [f64; 2], guess: [f64; 2]) -> Option<[f64; 2]> {
    // 空の座標の差を、赤緯の cos を掛けて度の平面にしたもの。
    let miss = |p: [f64; 2]| -> Option<[f64; 2]> {
        let s = wcs.to_sky(p[0], p[1])?;
        let dra = (s[0] - target[0] + 540.0).rem_euclid(360.0) - 180.0;
        Some([dra * target[1].to_radians().cos(), s[1] - target[1]])
    };
    // ずれの式は画素に対してほぼ平らなので、数値で微分したニュートン法がすぐ収まる。
    let mut p = guess;
    for _ in 0..20 {
        let r = miss(p)?;
        let (ax, ay) = (miss([p[0] + 1.0, p[1]])?, miss([p[0], p[1] + 1.0])?);
        let j = [[ax[0] - r[0], ay[0] - r[0]], [ax[1] - r[1], ay[1] - r[1]]];
        let det = j[0][0] * j[1][1] - j[0][1] * j[1][0];
        if det == 0.0 {
            return None;
        }
        let step = [
            (j[1][1] * r[0] - j[0][1] * r[1]) / det,
            (j[0][0] * r[1] - j[1][0] * r[0]) / det,
        ];
        p = [p[0] - step[0], p[1] - step[1]];
        if step[0].hypot(step[1]) < 1e-6 {
            return Some(p);
        }
    }
    None
}

/// 表示する画像の HDU と Primary のヘッダ。
#[derive(Debug, Clone)]
pub struct ImageHeader {
    image: Hdu,
    primary: Hdu,
}

impl ImageHeader {
    /// カードの値。画像の HDU になければ Primary から引く。
    pub fn get(&self, key: &str) -> Option<&str> {
        self.image.get(key).or_else(|| self.primary.get(key))
    }

    pub fn sky(&self) -> Option<Sky> {
        sky(&|key| self.get(key).map(str::to_owned))
    }
}

/// ファイルのヘッダだけを読む。画像がなければ `None`。データ部は読み飛ばす
/// （フォルダの数百枚を星図に出すのに、1 枚数十 MB の画素を読まないため）。
pub fn header_of_file(path: &Path) -> Result<Option<ImageHeader>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(file);
    let gzipped = reader
        .fill_buf()
        .map_err(|e| e.to_string())?
        .starts_with(&[0x1f, 0x8b]);
    let hdus = if gzipped {
        read_headers(
            &mut flate2::bufread::MultiGzDecoder::new(reader),
            &mut skip_by_reading,
        )
    } else {
        read_headers(&mut reader, &mut |r: &mut BufReader<std::fs::File>, n| {
            r.seek_relative(i64::try_from(n).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        })
    }?;
    let Ok(image) = crate::find_image(&hdus) else {
        return Ok(None);
    };
    Ok(Some(ImageHeader {
        image: image.hdu.clone(),
        primary: hdus[0].clone(),
    }))
}

/// 読んだ HDU の上限。画像はふつう 1 つ目か 2 つ目にある。
const MAX_HEADERS: usize = 16;
/// 1 つのヘッダの大きさの上限（壊れたファイルで END を探し続けないように）。
const MAX_HEADER_BYTES: usize = 100 * BLOCK;

/// 画像の HDU に行き着くまでヘッダを読む。データ部は `skip` で飛ばす。
fn read_headers<R: Read>(
    r: &mut R,
    skip: &mut dyn FnMut(&mut R, u64) -> Result<(), String>,
) -> Result<Vec<Hdu>, String> {
    let mut hdus = Vec::new();
    while hdus.len() < MAX_HEADERS {
        let mut bytes = Vec::new();
        loop {
            let mut block = [0u8; BLOCK];
            match r.read_exact(&mut block) {
                Ok(()) => bytes.extend_from_slice(&block),
                Err(_) if !hdus.is_empty() => return Ok(hdus),
                Err(e) => return Err(e.to_string()),
            }
            if block
                .chunks(80)
                .any(|c| c.starts_with(b"END") && c[3..].iter().all(|&b| b == b' '))
            {
                break;
            }
            if bytes.len() >= MAX_HEADER_BYTES {
                return Err("FITS header has no END".into());
            }
        }
        if hdus.is_empty() && !header::is_fits(&bytes) {
            return Err("not a FITS file".into());
        }
        let hdu = header::hdu_at(&bytes, 0).map_err(|e| e.to_string())?;
        let data = hdu.data_len.next_multiple_of(BLOCK) as u64;
        hdus.push(hdu);
        if crate::find_image(&hdus).is_ok() {
            break;
        }
        skip(r, data)?;
    }
    Ok(hdus)
}

fn skip_by_reading<R: Read>(r: &mut R, n: u64) -> Result<(), String> {
    let copied = std::io::copy(&mut r.take(n), &mut std::io::sink()).map_err(|e| e.to_string())?;
    if copied < n {
        return Err("FITS file is truncated".into());
    }
    Ok(())
}

/// 数のカード。Fortran 流の指数（1.0D+03）も読む。
pub(crate) fn number(v: &str) -> Option<f64> {
    v.trim()
        .replace(['D', 'd'], "E")
        .parse()
        .ok()
        .filter(|x: &f64| x.is_finite())
}

/// `"05 35 17.3"` や `"-05:23:28"` のような 60 進数（度か時）。
fn sexagesimal(v: &str) -> Option<f64> {
    let v = v.trim();
    let negative = v.starts_with('-');
    let parts: Vec<f64> = v
        .trim_start_matches(['-', '+'])
        .split([' ', ':'])
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().ok())
        .collect::<Option<_>>()?;
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let value = parts.iter().rev().fold(0.0, |acc, p| p + acc / 60.0);
    Some(if negative { -value } else { value })
}

/// 架台の向き（赤経・赤緯の度）。撮影ソフトは `RA` `DEC` を度で書き、`OBJCTRA` `OBJCTDEC` を
/// 時・度の 60 進数で書く。
fn pointing(lookup: &dyn Fn(&str) -> Option<String>) -> Option<[f64; 2]> {
    let degrees = |key: &str| {
        let v = lookup(key)?;
        number(&v).or_else(|| sexagesimal(&v))
    };
    let (ra, dec) = match (degrees("RA"), degrees("DEC")) {
        (Some(ra), Some(dec)) => (ra, dec),
        _ => (
            sexagesimal(&lookup("OBJCTRA")?)? * 15.0,
            sexagesimal(&lookup("OBJCTDEC")?)?,
        ),
    };
    ((-90.0..=90.0).contains(&dec)).then_some([ra.rem_euclid(360.0), dec])
}

/// 1 画素の角度（度）`[横, 縦]`。焦点距離 `FOCALLEN`（mm）と画素の大きさ `XPIXSZ` `YPIXSZ`
/// （µm、ビニング後）から。
///
/// `XBINNING` を掛けないのは、MaxIm DL に始まり N.I.N.A. や SharpCap も、`XPIXSZ` を
/// ビニング後の大きさで書くため。
fn field_scale(lookup: &dyn Fn(&str) -> Option<String>) -> Option<[f64; 2]> {
    let num = |key: &str| number(&lookup(key)?).filter(|v| *v > 0.0);
    let focal = num("FOCALLEN")?;
    let x = num("XPIXSZ")?;
    let y = num("YPIXSZ").unwrap_or(x);
    let degrees = |pixel_um: f64| (pixel_um / 1000.0 / focal).atan().to_degrees();
    Some([degrees(x), degrees(y)])
}

/// 画像の縁（画素の外側の線、1 始まりの座標で 0.5〜n+0.5）を一回りする点を、空の座標にする。
/// 写らない点（投影の外）が 1 つでもあれば、点の数が足りなくなる。
fn outline(size: [f64; 2], to_sky: &dyn Fn(f64, f64) -> Option<[f64; 2]>) -> Vec<[f64; 2]> {
    let [w, h] = size;
    let (x0, x1, y0, y1) = (0.5, w + 0.5, 0.5, h + 0.5);
    let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]];
    corners
        .windows(2)
        .flat_map(|pair| {
            let ([ax, ay], [bx, by]) = (pair[0], pair[1]);
            (0..EDGE_STEPS).map(move |i| {
                let t = i as f64 / EDGE_STEPS as f64;
                (ax + (bx - ax) * t, ay + (by - ay) * t)
            })
        })
        .filter_map(|(x, y)| to_sky(x, y))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Projection {
    Tan,
    Sin,
    Arc,
    Stg,
    Zea,
}

/// 天頂投影の WCS（論文 Calabretta & Greisen 2002 の式）。
#[derive(Debug, Clone, PartialEq)]
struct Wcs {
    crpix: [f64; 2],
    /// 基準点の赤経・赤緯（度）。
    crval: [f64; 2],
    /// 画素のずれ → 投影面の座標（度）。
    cd: [[f64; 2]; 2],
    projection: Projection,
    lonpole: f64,
}

impl Wcs {
    fn read(lookup: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let num = |key: &str| number(&lookup(key)?);
        let ctype = |i: u32| lookup(&format!("CTYPE{i}")).map(|v| v.trim().to_uppercase());
        let (c1, c2) = (ctype(1)?, ctype(2)?);
        // 赤経が 2 軸目に書かれたファイルは、軸を入れ替えて読む。
        let swapped = match (&c1[..c1.len().min(4)], &c2[..c2.len().min(4)]) {
            ("RA--", "DEC-") => false,
            ("DEC-", "RA--") => true,
            _ => return None,
        };
        let projection = match c1.get(5..8)? {
            "TAN" => Projection::Tan,
            "SIN" => Projection::Sin,
            "ARC" => Projection::Arc,
            "STG" => Projection::Stg,
            "ZEA" => Projection::Zea,
            _ => return None,
        };
        let cd = cd_matrix(&num)?;
        let (ra_axis, dec_axis) = if swapped { (2, 1) } else { (1, 2) };
        let cd = if swapped { [cd[1], cd[0]] } else { cd };
        Some(Self {
            crpix: [num("CRPIX1")?, num("CRPIX2")?],
            crval: [
                num(&format!("CRVAL{ra_axis}"))?,
                num(&format!("CRVAL{dec_axis}"))?,
            ],
            cd,
            projection,
            lonpole: num("LONPOLE").unwrap_or(180.0),
        })
    }

    /// 画素 `(x, y)`（1 始まり）の赤経・赤緯（度）。
    fn to_sky(&self, x: f64, y: f64) -> Option<[f64; 2]> {
        let (dx, dy) = (x - self.crpix[0], y - self.crpix[1]);
        let px = self.cd[0][0] * dx + self.cd[0][1] * dy;
        let py = self.cd[1][0] * dx + self.cd[1][1] * dy;
        let r = px.hypot(py);
        let phi = px.atan2(-py);
        let r_rad = r.to_radians();
        let theta = match self.projection {
            Projection::Tan => (1.0f64).atan2(r_rad),
            Projection::Sin if r_rad <= 1.0 => r_rad.acos(),
            Projection::Sin => return None,
            Projection::Arc => PI / 2.0 - r_rad,
            Projection::Stg => PI / 2.0 - 2.0 * (r_rad / 2.0).atan(),
            Projection::Zea if r_rad <= 2.0 => PI / 2.0 - 2.0 * (r_rad / 2.0).asin(),
            Projection::Zea => return None,
        };
        let (ra0, dec0) = (self.crval[0].to_radians(), self.crval[1].to_radians());
        let dphi = phi - self.lonpole.to_radians();
        let dec = (theta.sin() * dec0.sin() + theta.cos() * dec0.cos() * dphi.cos()).asin();
        let ra = ra0
            + (-theta.cos() * dphi.sin())
                .atan2(theta.sin() * dec0.cos() - theta.cos() * dec0.sin() * dphi.cos());
        let sky = [ra.to_degrees().rem_euclid(360.0), dec.to_degrees()];
        sky.iter().all(|v| v.is_finite()).then_some(sky)
    }
}

/// 画素のずれを投影面の度にする行列。`CDi_j` があればそれ、なければ `PCi_j`（省略は単位行列）
/// × `CDELTi`、古い書き方の `CROTA2` も読む。
pub(crate) fn cd_matrix(num: &dyn Fn(&str) -> Option<f64>) -> Option<[[f64; 2]; 2]> {
    let cd = |i, j| num(&format!("CD{i}_{j}"));
    if (1..=2).any(|i| (1..=2).any(|j| cd(i, j).is_some())) {
        let m = [
            [cd(1, 1).unwrap_or(0.0), cd(1, 2).unwrap_or(0.0)],
            [cd(2, 1).unwrap_or(0.0), cd(2, 2).unwrap_or(0.0)],
        ];
        return (m[0][0] * m[1][1] - m[0][1] * m[1][0] != 0.0).then_some(m);
    }
    let (d1, d2) = (num("CDELT1")?, num("CDELT2")?);
    if d1 == 0.0 || d2 == 0.0 {
        return None;
    }
    let pc = |i, j| num(&format!("PC{i}_{j}"));
    let has_pc = (1..=2).any(|i| (1..=2).any(|j| pc(i, j).is_some()));
    let m = if has_pc {
        let p = |i, j| pc(i, j).unwrap_or(if i == j { 1.0 } else { 0.0 });
        [[d1 * p(1, 1), d1 * p(1, 2)], [d2 * p(2, 1), d2 * p(2, 2)]]
    } else {
        let rot = num("CROTA2").unwrap_or(0.0).to_radians();
        [
            [d1 * rot.cos(), -d2 * rot.sin()],
            [d1 * rot.sin(), d2 * rot.cos()],
        ]
    };
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(cards: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            cards
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn close(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
        let dra = ((a[0] - b[0] + 540.0).rem_euclid(360.0) - 180.0).abs();
        dra < tol && (a[1] - b[1]).abs() < tol
    }

    const ORION: [(&str, &str); 11] = [
        ("NAXIS1", "200"),
        ("NAXIS2", "100"),
        ("CTYPE1", "RA---TAN"),
        ("CTYPE2", "DEC--TAN"),
        ("CRPIX1", "100.5"),
        ("CRPIX2", "50.5"),
        ("CRVAL1", "83.8221"),
        ("CRVAL2", "-5.3911"),
        ("CDELT1", "-0.01"),
        ("CDELT2", "0.01"),
        ("CROTA2", "0.0"),
    ];

    fn apply(m: [[f64; 3]; 2], x: f64, y: f64) -> [f64; 2] {
        [
            m[0][0] * x + m[0][1] * y + m[0][2],
            m[1][0] * x + m[1][1] * y + m[1][2],
        ]
    }

    #[test]
    fn map_finds_where_reference_pixels_lie_in_a_shifted_image() {
        let mut moved = ORION;
        moved[4] = ("CRPIX1", "103.5");
        moved[5] = ("CRPIX2", "48.5");
        let m = pixel_map(&lookup(&ORION), &lookup(&moved)).unwrap();
        for (x, y) in [(1.0, 1.0), (100.5, 50.5), (200.0, 100.0)] {
            assert!(close(apply(m, x, y), [x + 3.0, y - 2.0], 1e-3), "{m:?}");
        }
        assert_eq!(pixel_map(&lookup(&ORION), &lookup(&ORION[..4])), None);
    }

    #[test]
    fn map_follows_a_turned_and_finer_image() {
        // 90 度回して、画素を半分の大きさにした画像。中心は同じ空の点。
        let turned = [
            ("CTYPE1", "RA---TAN"),
            ("CTYPE2", "DEC--TAN"),
            ("CRPIX1", "150.5"),
            ("CRPIX2", "250.5"),
            ("CRVAL1", "83.8221"),
            ("CRVAL2", "-5.3911"),
            ("CDELT1", "-0.005"),
            ("CDELT2", "0.005"),
            ("CROTA2", "90.0"),
        ];
        let m = pixel_map(&lookup(&ORION), &lookup(&turned)).unwrap();
        let c = apply(m, 100.5, 50.5);
        assert!(close(c, [150.5, 250.5], 1e-3), "{c:?}");
        // 基準で右へ 10 画素は、回した画像では縦へ 20 画素。
        let r = apply(m, 110.5, 50.5);
        assert!((r[0] - c[0]).abs() < 0.05, "{r:?}");
        assert!(((r[1] - c[1]).abs() - 20.0).abs() < 0.05, "{r:?}");
    }

    #[test]
    fn tan_wcs_puts_the_reference_pixel_at_crval() {
        let wcs = Wcs::read(&lookup(&ORION)).unwrap();
        assert!(close(
            wcs.to_sky(100.5, 50.5).unwrap(),
            [83.8221, -5.3911],
            1e-9
        ));
        // 1 画素右は 0.01 度西（赤経が小さい）。赤緯の cos の分だけ赤経の差は広がる。
        let east = wcs.to_sky(101.5, 50.5).unwrap();
        let expected = 83.8221 - 0.01 / (-5.3911f64).to_radians().cos();
        assert!(close(east, [expected, -5.3911], 1e-6), "{east:?}");
    }

    /// astropy 7 の `WCS.all_pix2world`（SIP の係数なし）で求めた値。
    #[test]
    fn cd_matrix_with_rotation_matches_astropy() {
        let cards = [
            ("NAXIS1", "1000"),
            ("NAXIS2", "800"),
            ("CTYPE1", "RA---TAN-SIP"),
            ("CTYPE2", "DEC--TAN-SIP"),
            ("CRPIX1", "512.0"),
            ("CRPIX2", "400.0"),
            ("CRVAL1", "10.6847"),
            ("CRVAL2", "41.2690"),
            ("CD1_1", "-2.0E-4"),
            ("CD1_2", "1.0D-4"),
            ("CD2_1", "1.0E-4"),
            ("CD2_2", "2.0E-4"),
        ];
        let wcs = Wcs::read(&lookup(&cards)).unwrap();
        let p = wcs.to_sky(1.0, 1.0).unwrap();
        assert!(close(p, [10.767_421_6, 41.138_070_64], 1e-6), "{p:?}");
        let p = wcs.to_sky(1000.0, 800.0).unwrap();
        assert!(close(p, [10.607_914_21, 41.397_774_26], 1e-6), "{p:?}");
    }

    /// astropy 7 で、上の CD 行列のまま投影だけ変えて求めた左下の角。
    #[test]
    fn sin_arc_stg_and_zea_match_astropy() {
        let expected = [
            ("SIN", [10.767_421_86, 41.138_070_22]),
            ("ARC", [10.767_421_78, 41.138_070_36]),
            ("STG", [10.767_421_73, 41.138_070_43]),
            ("ZEA", [10.767_421_8, 41.138_070_33]),
        ];
        for (code, at_corner) in expected {
            let ra = format!("RA---{code}");
            let dec = format!("DEC--{code}");
            let cards = [
                ("CTYPE1", ra.as_str()),
                ("CTYPE2", dec.as_str()),
                ("CRPIX1", "512.0"),
                ("CRPIX2", "400.0"),
                ("CRVAL1", "10.6847"),
                ("CRVAL2", "41.2690"),
                ("CD1_1", "-2.0E-4"),
                ("CD1_2", "1.0E-4"),
                ("CD2_1", "1.0E-4"),
                ("CD2_2", "2.0E-4"),
            ];
            let p = Wcs::read(&lookup(&cards))
                .unwrap()
                .to_sky(1.0, 1.0)
                .unwrap();
            assert!(close(p, at_corner, 1e-7), "{code}: {p:?}");
        }
    }

    #[test]
    fn swapped_axes_are_read_as_ra_and_dec() {
        let cards = [
            ("NAXIS1", "10"),
            ("NAXIS2", "10"),
            ("CTYPE1", "DEC--TAN"),
            ("CTYPE2", "RA---TAN"),
            ("CRPIX1", "5"),
            ("CRPIX2", "5"),
            ("CRVAL1", "20"),
            ("CRVAL2", "150"),
            ("CDELT1", "0.1"),
            ("CDELT2", "-0.1"),
        ];
        let s = sky(&lookup(&cards)).unwrap();
        assert!(s.certain);
        // astropy 7 の値（軸の順のまま赤緯・赤経で返るのを入れ替えたもの）。
        assert!(
            close(s.center, [149.946_774_22, 20.049_992_03], 1e-7),
            "{:?}",
            s.center
        );
    }

    #[test]
    fn wcs_gives_a_certain_outline_around_the_image() {
        let s = sky(&lookup(&ORION)).unwrap();
        assert!(s.certain);
        assert_eq!(s.outline.len(), 4 * EDGE_STEPS);
        assert!(close(s.center, [83.8221, -5.3911], 1e-6));
        // 縁は中心から横に 1 度、縦に 0.5 度。
        let widest = s
            .outline
            .iter()
            .map(|p| (p[1] - s.center[1]).abs())
            .fold(0.0, f64::max);
        assert!((widest - 0.5).abs() < 1e-3, "{widest}");
    }

    #[test]
    fn unsupported_projection_falls_back_to_the_pointing() {
        let mut cards = ORION.to_vec();
        cards[2] = ("CTYPE1", "RA---CAR");
        cards[3] = ("CTYPE2", "DEC--CAR");
        assert_eq!(sky(&lookup(&cards)), None);
        cards.push(("RA", "83.8"));
        cards.push(("DEC", "-5.4"));
        let s = sky(&lookup(&cards)).unwrap();
        assert!(!s.certain);
        assert!(s.outline.is_empty());
    }

    #[test]
    fn pointing_and_focal_length_give_an_approximate_north_up_frame() {
        let cards = [
            ("NAXIS1", "6000"),
            ("NAXIS2", "4000"),
            ("RA", "83.8221"),
            ("DEC", "-5.3911"),
            ("FOCALLEN", "400.0"),
            ("XPIXSZ", "3.76"),
            ("YPIXSZ", "3.76"),
            ("XBINNING", "1"),
        ];
        let s = sky(&lookup(&cards)).unwrap();
        assert!(!s.certain);
        assert!(close(s.center, [83.8221, -5.3911], 1e-9));
        // 3.76 µm / 400 mm = 1.94 秒角 / 画素。縦 4000 画素で 2.15 度。
        let top = s.outline.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
        let bottom = s.outline.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
        assert!((top - bottom - 2.154).abs() < 0.01, "{}", top - bottom);
        // 北が上：上の縁の 2 つの角は同じ赤緯。
        let corner = |i: usize| s.outline[i * EDGE_STEPS];
        assert!((corner(0)[1] - corner(1)[1]).abs() < 1e-9);
    }

    #[test]
    fn sexagesimal_pointing_is_read_in_hours_and_degrees() {
        let cards = [
            ("NAXIS1", "100"),
            ("NAXIS2", "100"),
            ("OBJCTRA", "05 35 17.3"),
            ("OBJCTDEC", "-05 23 28"),
        ];
        let s = sky(&lookup(&cards)).unwrap();
        assert!(
            close(s.center, [83.822_083, -5.391_111], 1e-5),
            "{:?}",
            s.center
        );
        assert!(s.outline.is_empty());
    }

    #[test]
    fn nothing_is_known_without_a_wcs_or_a_pointing() {
        let cards = [("NAXIS1", "100"), ("NAXIS2", "100"), ("OBJECT", "M42")];
        assert_eq!(sky(&lookup(&cards)), None);
    }
}
