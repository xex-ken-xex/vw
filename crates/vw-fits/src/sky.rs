//! 画像が空のどこを写したか。ヘッダの WCS（はっきり）か、架台の向き（およそ）から決める。
//!
//! WCS は天頂投影のうち、撮影と公開データでほぼすべてを占める TAN と、SIN・ARC・STG・ZEA を
//! 読む。光学系の歪みの多項式（`-SIP` と `TPV`）も当てる。星図の枠では差が見えないほど小さいが、
//! 十字で画素の空の座標を出すと、画像の端で数画素ずれるため。
//! 全天図の投影 CAR・AIT・MOL と、銀河座標（GLON・GLAT）も読む。座標は内側で赤経・赤緯（ICRS）に
//! 直すので、星図・色の合成・貼り合わせは座標系を気にしなくてよい。

use std::f64::consts::PI;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use crate::header::{self, BLOCK, Hdu};

/// 写った範囲。座標は赤経・赤緯（度）。
#[derive(Debug, Clone, PartialEq)]
pub struct Sky {
    pub center: [f64; 2],
    /// 写った範囲の縁を一回りした点。分からなければ空（中心だけ分かる）。
    pub outline: Vec<[f64; 2]>,
    /// ヘッダの WCS から求めたか。`false` なら架台の向きと焦点距離からの見積もり。
    pub certain: bool,
    /// 半球より広い範囲（全天図や、天球を一周する帯）。縁を一回りしても枠にならないので、
    /// `outline` は空。
    pub wide: bool,
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
        let whole = outline.len() == 4 * EDGE_STEPS;
        if whole || wcs.projection.cylindrical() {
            let wide = !whole || spans_more_than_a_hemisphere(size, center, &wcs);
            return Some(Sky {
                center,
                outline: if wide { Vec::new() } else { outline },
                certain: true,
                wide,
            });
        }
    }
    let center = pointing(&lookup)?;
    let outline = match field_scale(&lookup) {
        Some([sx, sy]) => {
            // 回転は分からないので北を上（赤経は左が東）にする。
            match Wcs::new(
                [(size[0] + 1.0) / 2.0, (size[1] + 1.0) / 2.0],
                center,
                [[-sx, 0.0], [0.0, sy]],
                Projection::Tan,
                (180.0, 90.0),
                Distortion::None,
                Frame::Equatorial,
            ) {
                Some(wcs) => outline(size, &|x, y| wcs.to_sky(x, y)),
                None => Vec::new(),
            }
        }
        None => Vec::new(),
    };
    Some(Sky {
        center,
        outline,
        certain: false,
        wide: false,
    })
}

/// 画像の中に、中心から 90° より離れた点があるか。縁と中を 16 x 16 の格子で調べる。
fn spans_more_than_a_hemisphere(size: [f64; 2], center: [f64; 2], wcs: &Wcs) -> bool {
    const STEPS: usize = 16;
    (0..=STEPS).any(|i| {
        (0..=STEPS).any(|j| {
            let x = 0.5 + size[0] * i as f64 / STEPS as f64;
            let y = 0.5 + size[1] * j as f64 / STEPS as f64;
            wcs.to_sky(x, y).is_some_and(|p| angle(center, p) > 90.0)
        })
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
    if from == to {
        // 同じ格子（全天図を同じ WCS で作った組など）。式を解かずにそのまま重ねる。
        return Some([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    }
    if [reference, other]
        .iter()
        .any(|lookup| sky(lookup).is_some_and(|s| s.wide))
    {
        // 全天図では 1 次の式に近づけられない（画素ごとに解くと重い）ので、位置合わせをしない。
        return None;
    }
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

/// `lookup` の WCS を、別の画素の並びに置き直したカード（名前, 値）。値は `lookup` と同じく、
/// 文字のカードも引用符を付けない。
/// 新しい画素 (x, y)（1 始まり）が、`lookup` の画像の画素 `map · (x, y, 1)`（1 始まり）に当たるとき。
/// WCS がなければ `None`。
///
/// 何枚かを貼り合わせた画像に、もとの 1 枚の WCS を引き継ぐのに使う。画素の並びを 1 次の式で
/// 動かしただけなら、投影はそのままで CRPIX と CD を書き換えれば同じ空の点を指す。
pub fn wcs_through(
    lookup: &dyn Fn(&str) -> Option<String>,
    map: [[f64; 3]; 2],
) -> Option<Vec<(String, String)>> {
    let wcs = Wcs::read(lookup)?;
    let [[a, b, c], [d, e, f]] = map;
    let det = a * e - b * d;
    if det.abs() < 1e-12 {
        return None;
    }
    // CRPIX = map · CRPIX'、CD' = CD · M。
    let (px, py) = (wcs.crpix[0] - c, wcs.crpix[1] - f);
    let crpix = [(e * px - b * py) / det, (a * py - d * px) / det];
    let cd = [
        [
            wcs.cd[0][0] * a + wcs.cd[0][1] * d,
            wcs.cd[0][0] * b + wcs.cd[0][1] * e,
        ],
        [
            wcs.cd[1][0] * a + wcs.cd[1][1] * d,
            wcs.cd[1][0] * b + wcs.cd[1][1] * e,
        ],
    ];
    // 歪みの多項式は引き継がない。SIP は元の画素の並びの CRPIX からの式なので、貼り合わせで
    // 動かした画素には書き直せない。1 枚の写真の範囲では、星図の枠や重ね合わせには差が見えない。
    let code = wcs.projection.code();
    let (lon, lat) = wcs.frame.prefixes();
    let dashes = |prefix: &str| format!("{prefix:-<5}{code}");
    let number = |v: f64| format!("{v:.15E}");
    let mut cards = vec![
        ("CTYPE1".to_owned(), dashes(lon)),
        ("CTYPE2".to_owned(), dashes(lat)),
        ("CRVAL1".to_owned(), number(wcs.crval[0])),
        ("CRVAL2".to_owned(), number(wcs.crval[1])),
        ("CRPIX1".to_owned(), number(crpix[0])),
        ("CRPIX2".to_owned(), number(crpix[1])),
        ("CD1_1".to_owned(), number(cd[0][0])),
        ("CD1_2".to_owned(), number(cd[0][1])),
        ("CD2_1".to_owned(), number(cd[1][0])),
        ("CD2_2".to_owned(), number(cd[1][1])),
    ];
    if let Projection::Sin { xi, eta } = wcs.projection
        && (xi != 0.0 || eta != 0.0)
    {
        cards.push(("PV2_1".to_owned(), number(xi)));
        cards.push(("PV2_2".to_owned(), number(eta)));
    }
    // 既定の値は投影と基準点で変わるので、いつも書く。
    cards.push(("LONPOLE".to_owned(), number(wcs.lonpole)));
    cards.push(("LATPOLE".to_owned(), number(wcs.latpole)));
    for key in ["RADESYS", "EQUINOX"] {
        if let Some(v) = lookup(key) {
            cards.push((key.to_owned(), v.trim().to_owned()));
        }
    }
    Some(cards)
}

/// 表示した画像の画素と空の座標の対応（十字に出す）。
#[derive(Debug)]
pub(crate) struct ImageSky {
    wcs: Wcs,
    height: f64,
    /// 表示で上下を返したか（FITS の最初の行が下になる）。
    flipped: bool,
}

impl ImageSky {
    /// ヘッダに WCS があれば作る。架台の向きからの見積もりは使わない（およその座標を、
    /// 画素ごとの正しい座標のように見せないため）。
    pub(crate) fn read(
        lookup: &dyn Fn(&str) -> Option<String>,
        height: usize,
        flipped: bool,
    ) -> Option<Self> {
        Some(Self {
            wcs: Wcs::read(lookup)?,
            height: height as f64,
            flipped,
        })
    }
}

impl ImageSky {
    /// 表示の画素（0 始まり、上が 0）を FITS の画素（1 始まり、最初の行が y = 1）にする。
    fn fits_pixel(&self, x: f64, y: f64) -> (f64, f64) {
        let fits_y = if self.flipped {
            self.height - y
        } else {
            y + 1.0
        };
        (x + 1.0, fits_y)
    }
}

impl vw_core::sky::SkyMap for ImageSky {
    fn at(&self, x: f64, y: f64) -> Option<[f64; 2]> {
        let (x, y) = self.fits_pixel(x, y);
        self.wcs.to_sky(x, y)
    }

    fn galactic(&self, x: f64, y: f64) -> Option<[f64; 2]> {
        if self.wcs.frame != Frame::Galactic {
            return None;
        }
        let (x, y) = self.fits_pixel(x, y);
        self.wcs.to_frame(x, y)
    }
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
    let hdus = headers_of_file(path, Until::FirstImage)?;
    let Ok(image) = crate::default_image(&hdus) else {
        return Ok(None);
    };
    Ok(Some(ImageHeader {
        image: image.hdu.clone(),
        primary: hdus[0].clone(),
    }))
}

/// ファイルの画像の HDU 全部のヘッダ（[`crate::parts`] と同じ順）。データ部は読み飛ばす。
/// 検出器ごとに拡張を並べるモザイクカメラの、1 枚ずつの写した範囲を星図に出すのに使う。
pub fn image_headers_of_file(path: &Path) -> Result<Vec<ImageHeader>, String> {
    let hdus = headers_of_file(path, Until::End)?;
    Ok((0..hdus.len())
        .filter_map(|i| crate::image_at(&hdus, i, 1).ok().flatten())
        .map(|image| ImageHeader {
            image: image.hdu.clone(),
            primary: hdus[0].clone(),
        })
        .collect())
}

/// ファイルの全部の HDU のヘッダ。データ部は読み飛ばす。
pub(crate) fn headers_of_file_all(path: &Path) -> Result<Vec<Hdu>, String> {
    headers_of_file(path, Until::End)
}

/// ヘッダをどこまで読むか。
#[derive(Clone, Copy, PartialEq)]
enum Until {
    FirstImage,
    End,
}

fn headers_of_file(path: &Path, until: Until) -> Result<Vec<Hdu>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(file);
    let gzipped = reader
        .fill_buf()
        .map_err(|e| e.to_string())?
        .starts_with(&[0x1f, 0x8b]);
    if gzipped {
        read_headers(
            &mut flate2::bufread::MultiGzDecoder::new(reader),
            until,
            &mut skip_by_reading,
        )
    } else {
        read_headers(
            &mut reader,
            until,
            &mut |r: &mut BufReader<std::fs::File>, n| {
                r.seek_relative(i64::try_from(n).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())
            },
        )
    }
}

/// 最初の画像を探すときに読む HDU の上限。画像はふつう 1 つ目か 2 つ目にある。
const MAX_HEADERS: usize = 16;
/// 全部の HDU を読むときの上限。すばる HSC の 1 ファイルは検出器 1 枚ずつなので、
/// 何百枚を並べたファイルはまずないが、壊れたファイルで読み続けないように。
const MAX_ALL_HEADERS: usize = 1024;
/// 1 つのヘッダの大きさの上限（壊れたファイルで END を探し続けないように）。
const MAX_HEADER_BYTES: usize = 100 * BLOCK;

/// 画像の HDU に行き着くまで（`until`）ヘッダを読む。データ部は `skip` で飛ばす。
fn read_headers<R: Read>(
    r: &mut R,
    until: Until,
    skip: &mut dyn FnMut(&mut R, u64) -> Result<(), String>,
) -> Result<Vec<Hdu>, String> {
    let max = match until {
        Until::FirstImage => MAX_HEADERS,
        Until::End => MAX_ALL_HEADERS,
    };
    let mut hdus = Vec::new();
    while hdus.len() < max {
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
        // 1 次元のデータは、後ろに画像があればそちらを使うので、読み進める。
        if until == Until::FirstImage && crate::find_image_with_axes(&hdus, 2).is_ok() {
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
    /// 傾けた SIN（`PV2_1` = ξ、`PV2_2` = η）。干渉計の古い書き方の NCP は、ξ = 0、η = cot δ0 の SIN。
    Sin {
        xi: f64,
        eta: f64,
    },
    Arc,
    Stg,
    Zea,
    /// 緯度経度の格子（plate carrée）。
    Car,
    /// ハンメル・エイトフ。
    Ait,
    /// モルワイデ。
    Mol,
    /// サンソン・フラムスティード（正弦曲線）。
    Sfl,
    /// 古い書き方の GLS。式は SFL と同じだが、基準の緯度 `CRVAL2` は格子を傾けず、
    /// 地図をその緯度まで平行に動かすだけ（基準点の投影の球の緯度 θ0 = `CRVAL2` とした SFL）。
    Gls {
        theta0: f64,
    },
}

impl Projection {
    /// `pv(m)` は緯度の軸の `PVi_m`、`lat0` は基準点の緯度（度）。
    fn read(code: &str, pv: &dyn Fn(u32) -> Option<f64>, lat0: f64) -> Option<Self> {
        Some(match code {
            "TAN" | "TPV" => Projection::Tan,
            "SIN" => Projection::Sin {
                xi: pv(1).unwrap_or(0.0),
                eta: pv(2).unwrap_or(0.0),
            },
            "NCP" => {
                let t = lat0.to_radians().tan();
                if t.abs() < 1e-12 {
                    return None;
                }
                Projection::Sin {
                    xi: 0.0,
                    eta: 1.0 / t,
                }
            }
            "SFL" => Projection::Sfl,
            "GLS" => Projection::Gls { theta0: lat0 },
            "ARC" => Projection::Arc,
            "STG" => Projection::Stg,
            "ZEA" => Projection::Zea,
            "CAR" => Projection::Car,
            "AIT" => Projection::Ait,
            "MOL" => Projection::Mol,
            _ => return None,
        })
    }

    /// `CTYPE` の後ろの 3 文字。
    fn code(self) -> &'static str {
        match self {
            Projection::Tan => "TAN",
            Projection::Sin { .. } => "SIN",
            Projection::Arc => "ARC",
            Projection::Stg => "STG",
            Projection::Zea => "ZEA",
            Projection::Car => "CAR",
            Projection::Ait => "AIT",
            Projection::Mol => "MOL",
            Projection::Sfl => "SFL",
            Projection::Gls { .. } => "GLS",
        }
    }

    /// 基準点の投影の球の上の緯度 θ0（度）。天頂投影は極（90°）、円筒・擬円筒は赤道（0°）。
    fn theta0(self) -> f64 {
        match self {
            Projection::Gls { theta0 } => theta0,
            _ if self.cylindrical() => 0.0,
            _ => 90.0,
        }
    }

    /// 円筒・擬円筒の投影（全天を 1 枚に描けるもの）か。
    fn cylindrical(self) -> bool {
        matches!(
            self,
            Projection::Car
                | Projection::Ait
                | Projection::Mol
                | Projection::Sfl
                | Projection::Gls { .. }
        )
    }

    /// 投影面の座標 (x, y)（度）→ 投影の球の経度・緯度 (φ, θ)（ラジアン）。投影の外なら `None`。
    fn native(self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (xr, yr) = (x.to_radians(), y.to_radians());
        let zenithal = |theta: f64| Some((x.atan2(-y), theta));
        let r = xr.hypot(yr);
        match self {
            Projection::Tan => zenithal(1.0f64.atan2(r)),
            Projection::Sin { xi, eta } if xi == 0.0 && eta == 0.0 => {
                (r <= 1.0).then(|| r.acos()).and_then(zenithal)
            }
            Projection::Sin { xi, eta } => {
                // 論文の式 (40)〜(41)。sin θ の 2 次式の解のうち、極（θ = 90°）に近い方。
                let (dx, dy) = (xr - xi, yr - eta);
                let a = xi * xi + eta * eta + 1.0;
                let b = xi * dx + eta * dy;
                let c = dx * dx + dy * dy - 1.0;
                let disc = b * b - a * c;
                if disc < 0.0 {
                    return None;
                }
                let sin_theta = [(-b + disc.sqrt()) / a, (-b - disc.sqrt()) / a]
                    .into_iter()
                    .filter(|s| s.abs() <= 1.0 + 1e-12)
                    .max_by(f64::total_cmp)?
                    .clamp(-1.0, 1.0);
                let k = 1.0 - sin_theta;
                Some(((xr - xi * k).atan2(-(yr - eta * k)), sin_theta.asin()))
            }
            Projection::Arc => zenithal(PI / 2.0 - r),
            Projection::Stg => zenithal(PI / 2.0 - 2.0 * (r / 2.0).atan()),
            Projection::Zea => (r <= 2.0)
                .then(|| PI / 2.0 - 2.0 * (r / 2.0).asin())
                .and_then(zenithal),
            Projection::Car => (yr.abs() <= PI / 2.0 && xr.abs() <= PI + 1e-9).then_some((xr, yr)),
            Projection::Ait => {
                let zz = 1.0 - (xr / 4.0).powi(2) - (yr / 2.0).powi(2);
                if zz < 0.5 {
                    return None;
                }
                let z = zz.sqrt();
                let phi = 2.0 * (z * xr / 2.0).atan2(2.0 * zz - 1.0);
                Some((phi, (yr * z).clamp(-1.0, 1.0).asin()))
            }
            Projection::Mol => {
                let s = yr / 2f64.sqrt();
                if s.abs() > 1.0 {
                    return None;
                }
                let gamma = s.asin();
                let phi = if gamma.cos() < 1e-12 {
                    0.0
                } else {
                    PI * xr / (2.0 * 2f64.sqrt() * gamma.cos())
                };
                if phi.abs() > PI + 1e-9 {
                    return None;
                }
                let t = (2.0 * gamma + (2.0 * gamma).sin()) / PI;
                Some((phi, t.clamp(-1.0, 1.0).asin()))
            }
            Projection::Sfl => sinusoidal(xr, yr),
            // 基準点が (0, θ0) なので、投影面の原点を θ0 だけずらしてから SFL で戻す。
            Projection::Gls { theta0 } => sinusoidal(xr, yr + theta0.to_radians()),
        }
    }
}

/// SFL の (x, y)（ラジアン）→ (φ, θ)。
fn sinusoidal(x: f64, y: f64) -> Option<(f64, f64)> {
    if y.abs() > PI / 2.0 + 1e-12 {
        return None;
    }
    let c = y.cos();
    let phi = if c < 1e-12 { 0.0 } else { x / c };
    (phi.abs() <= PI + 1e-9).then_some((phi, y.clamp(-PI / 2.0, PI / 2.0)))
}

/// 座標系。内側では赤道座標（ICRS）に直して持つ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Frame {
    Equatorial,
    Galactic,
}

impl Frame {
    /// `CTYPE` の頭（経度・緯度の 4 文字ずつ）。
    fn prefixes(self) -> (&'static str, &'static str) {
        match self {
            Frame::Equatorial => ("RA--", "DEC-"),
            Frame::Galactic => ("GLON", "GLAT"),
        }
    }

    /// この座標系の経度・緯度（度）を、赤経・赤緯（ICRS、度）にする。
    fn to_icrs(self, lon_lat: [f64; 2]) -> [f64; 2] {
        match self {
            Frame::Equatorial => lon_lat,
            Frame::Galactic => rotate(&GALACTIC_TO_ICRS, lon_lat),
        }
    }
}

/// 銀河座標の単位ベクトル → ICRS の単位ベクトル。astropy 8.0.2 の Galactic → ICRS の変換から求めた。
const GALACTIC_TO_ICRS: [[f64; 3]; 3] = [
    [
        -0.054_875_657_712_592_2,
        0.494_109_437_192_726_7,
        -0.867_666_137_559_658_5,
    ],
    [
        -0.873_437_051_955_616_2,
        -0.444_829_721_223_295_7,
        -0.198_076_337_273_000_6,
    ],
    [
        -0.483_835_073_616_715_6,
        0.746_982_183_986_667_7,
        0.455_983_813_687_302_1,
    ],
];

fn rotate(m: &[[f64; 3]; 3], lon_lat: [f64; 2]) -> [f64; 2] {
    let (l, b) = (lon_lat[0].to_radians(), lon_lat[1].to_radians());
    let v = [b.cos() * l.cos(), b.cos() * l.sin(), b.sin()];
    let w = [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]);
    [
        w[1].atan2(w[0]).to_degrees().rem_euclid(360.0),
        w[2].clamp(-1.0, 1.0).asin().to_degrees(),
    ]
}

/// 2 点の間の角度（度）。
fn angle(a: [f64; 2], b: [f64; 2]) -> f64 {
    let unit = |p: [f64; 2]| {
        let (l, b) = (p[0].to_radians(), p[1].to_radians());
        [b.cos() * l.cos(), b.cos() * l.sin(), b.sin()]
    };
    let (u, v) = (unit(a), unit(b));
    (u[0] * v[0] + u[1] * v[1] + u[2] * v[2])
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

/// 光学系の歪みの多項式。
#[derive(Debug, Clone, PartialEq)]
enum Distortion {
    None,
    /// SIP（Shupe ほか 2005）。画素のずれ (u, v) に足す多項式の係数 `(p, q, 係数)`。
    Sip {
        a: Vec<(i32, i32, f64)>,
        b: Vec<(i32, i32, f64)>,
    },
    /// TPV（SCAMP の書き方）。投影面の座標（度）を書き直す、`PV1_0`〜`PV1_39` と `PV2_0`〜`PV2_39`。
    Tpv {
        pv1: Box<[f64; 40]>,
        pv2: Box<[f64; 40]>,
    },
}

impl Distortion {
    /// `CTYPE` の後ろの印（`-SIP`）と、投影の名前（`TPV`）から読む。係数が 1 つもなければ歪みなし。
    fn read(num: &dyn Fn(&str) -> Option<f64>, sip: bool, tpv: bool) -> Self {
        if sip {
            let terms = |name: &str| -> Vec<(i32, i32, f64)> {
                let Some(order) = num(&format!("{name}_ORDER")) else {
                    return Vec::new();
                };
                let order = order.clamp(0.0, 9.0) as i32;
                let mut out = Vec::new();
                for p in 0..=order {
                    for q in 0..=order - p {
                        if let Some(c) = num(&format!("{name}_{p}_{q}")).filter(|&c| c != 0.0) {
                            out.push((p, q, c));
                        }
                    }
                }
                out
            };
            let (a, b) = (terms("A"), terms("B"));
            if !a.is_empty() || !b.is_empty() {
                return Distortion::Sip { a, b };
            }
        }
        if tpv {
            // 書いていない係数は、1 次の PVi_1 が 1、ほかは 0（歪みなし）。
            let coefficients = |axis: u32| {
                let mut c = [0.0; 40];
                c[1] = 1.0;
                for (k, v) in c.iter_mut().enumerate() {
                    if let Some(x) = num(&format!("PV{axis}_{k}")) {
                        *v = x;
                    }
                }
                c
            };
            return Distortion::Tpv {
                pv1: Box::new(coefficients(1)),
                pv2: Box::new(coefficients(2)),
            };
        }
        Distortion::None
    }

    /// 画素のずれ (u, v) を、CD を掛ける前に直す（SIP）。
    fn before_cd(&self, u: f64, v: f64) -> (f64, f64) {
        match self {
            Distortion::Sip { a, b } => {
                let sum = |terms: &[(i32, i32, f64)]| -> f64 {
                    terms
                        .iter()
                        .map(|&(p, q, c)| c * u.powi(p) * v.powi(q))
                        .sum()
                };
                (u + sum(a), v + sum(b))
            }
            _ => (u, v),
        }
    }

    /// 投影面の座標（度）を、CD を掛けた後に直す（TPV）。
    fn after_cd(&self, x: f64, y: f64) -> (f64, f64) {
        match self {
            Distortion::Tpv { pv1, pv2 } => (tpv(pv1, x, y), tpv(pv2, y, x)),
            _ => (x, y),
        }
    }
}

/// TPV の多項式。`PV1` は (ξ, η)、`PV2` は (η, ξ) の順に入れる。項の並びは
/// 1, x, y, r, x², xy, y², x³, x²y, xy², y³, r³, x⁴ … y⁴, x⁵ … y⁵, r⁵, x⁶ … y⁶, x⁷ … y⁷, r⁷。
fn tpv(c: &[f64; 40], x: f64, y: f64) -> f64 {
    let r = x.hypot(y);
    let mut sum = c[0];
    let mut k = 1;
    for degree in 1..=7 {
        for i in 0..=degree {
            sum += c[k] * x.powi(degree - i) * y.powi(i);
            k += 1;
        }
        if degree % 2 == 1 {
            sum += c[k] * r.powi(degree);
            k += 1;
        }
    }
    sum
}

/// WCS（論文 Calabretta & Greisen 2002 の式）。
#[derive(Debug, Clone, PartialEq)]
struct Wcs {
    crpix: [f64; 2],
    /// 基準点の経度・緯度（[`Wcs::frame`] の座標系、度）。
    crval: [f64; 2],
    /// 画素のずれ → 投影面の座標（度）。
    cd: [[f64; 2]; 2],
    projection: Projection,
    lonpole: f64,
    latpole: f64,
    distortion: Distortion,
    frame: Frame,
    /// 投影の球の北極の、座標系での経度・緯度（ラジアン）。`crval` と `lonpole` から決まる。
    pole: [f64; 2],
}

impl Wcs {
    fn read(lookup: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let num = |key: &str| number(&lookup(key)?);
        let ctype = |i: u32| lookup(&format!("CTYPE{i}")).map(|v| v.trim().to_uppercase());
        let (c1, c2) = (ctype(1)?, ctype(2)?);
        // 経度が 2 軸目に書かれたファイルは、軸を入れ替えて読む。
        let (frame, swapped) =
            [Frame::Equatorial, Frame::Galactic]
                .into_iter()
                .find_map(|frame| {
                    let (lon, lat) = frame.prefixes();
                    match (&c1[..c1.len().min(4)], &c2[..c2.len().min(4)]) {
                        (a, b) if a == lon && b == lat => Some((frame, false)),
                        (a, b) if a == lat && b == lon => Some((frame, true)),
                        _ => None,
                    }
                })?;
        let code = c1.get(5..8)?;
        let (lon_axis, lat_axis) = if swapped { (2, 1) } else { (1, 2) };
        let lat0 = num(&format!("CRVAL{lat_axis}"))?;
        let projection = Projection::read(code, &|m| num(&format!("PV{lat_axis}_{m}")), lat0)?;
        let cd = cd_matrix(&num)?;
        let cd = if swapped { [cd[1], cd[0]] } else { cd };
        let sip = c1.get(8..).is_some_and(|rest| rest.trim() == "-SIP");
        let mut distortion = Distortion::read(&num, sip, code == "TPV");
        if swapped && let Distortion::Tpv { pv1, pv2 } = &mut distortion {
            // PV1 は 1 本目の軸（ここでは緯度）の式なので、経度・緯度の順に入れ替える。
            std::mem::swap(pv1, pv2);
        }
        let crval = [
            num(&format!("CRVAL{lon_axis}"))?,
            num(&format!("CRVAL{lat_axis}"))?,
        ];
        // 書いていなければ、基準点が投影の球の極より北なら 0°、そうでなければ 180°（論文の既定）。
        let lonpole = num("LONPOLE").unwrap_or(if crval[1] >= projection.theta0() {
            0.0
        } else {
            180.0
        });
        Self::new(
            [num("CRPIX1")?, num("CRPIX2")?],
            crval,
            cd,
            projection,
            (lonpole, num("LATPOLE").unwrap_or(90.0)),
            distortion,
            frame,
        )
    }

    fn new(
        crpix: [f64; 2],
        crval: [f64; 2],
        cd: [[f64; 2]; 2],
        projection: Projection,
        (lonpole, latpole): (f64, f64),
        distortion: Distortion,
        frame: Frame,
    ) -> Option<Self> {
        let pole = native_pole(crval, projection.theta0(), lonpole, latpole)?;
        Some(Self {
            crpix,
            crval,
            cd,
            projection,
            lonpole,
            latpole,
            distortion,
            frame,
            pole,
        })
    }

    /// 画素 `(x, y)`（1 始まり）の、画像の座標系での経度・緯度（度）。
    fn to_frame(&self, x: f64, y: f64) -> Option<[f64; 2]> {
        let (dx, dy) = self
            .distortion
            .before_cd(x - self.crpix[0], y - self.crpix[1]);
        let (px, py) = self.distortion.after_cd(
            self.cd[0][0] * dx + self.cd[0][1] * dy,
            self.cd[1][0] * dx + self.cd[1][1] * dy,
        );
        let (phi, theta) = self.projection.native(px, py)?;
        let [lon_p, lat_p] = self.pole;
        let dphi = phi - self.lonpole.to_radians();
        let lat = (theta.sin() * lat_p.sin() + theta.cos() * lat_p.cos() * dphi.cos())
            .clamp(-1.0, 1.0)
            .asin();
        let lon = lon_p
            + (-theta.cos() * dphi.sin())
                .atan2(theta.sin() * lat_p.cos() - theta.cos() * lat_p.sin() * dphi.cos());
        let p = [lon.to_degrees().rem_euclid(360.0), lat.to_degrees()];
        p.iter().all(|v| v.is_finite()).then_some(p)
    }

    /// 画素 `(x, y)`（1 始まり）の赤経・赤緯（ICRS、度）。
    fn to_sky(&self, x: f64, y: f64) -> Option<[f64; 2]> {
        Some(self.frame.to_icrs(self.to_frame(x, y)?))
    }
}

/// 投影の球の北極の、座標系での経度・緯度（ラジアン）。論文の式 (8)〜(10)。
/// 基準点 `crval`（度）が投影の球の (0, θ0) に来て、北極が投影の球の経度 `lonpole` の向きにある。
/// 解が 2 つあるときは `latpole` に近い方。
fn native_pole(crval: [f64; 2], theta0: f64, lonpole: f64, latpole: f64) -> Option<[f64; 2]> {
    let (a0, d0) = (crval[0].to_radians(), crval[1].to_radians());
    if theta0 == 90.0 {
        return Some([a0, d0]);
    }
    let t0 = theta0.to_radians();
    let dp = lonpole.to_radians(); // φp − φ0（φ0 = 0）
    let base = t0.sin().atan2(t0.cos() * dp.cos());
    let denom = (1.0 - (t0.cos() * dp.sin()).powi(2)).sqrt();
    if denom < 1e-12 {
        return None;
    }
    let ratio = d0.sin() / denom;
    if ratio.abs() > 1.0 + 1e-12 {
        return None;
    }
    let spread = ratio.clamp(-1.0, 1.0).acos();
    // 角度は 2π の幅で同じなので、-π〜π に直してから緯度になれるものを選ぶ。
    let candidates = [base + spread, base - spread]
        .into_iter()
        .map(|d| (d + PI).rem_euclid(2.0 * PI) - PI)
        .filter(|d| d.abs() <= PI / 2.0 + 1e-12)
        .map(|d| d.clamp(-PI / 2.0, PI / 2.0));
    let lat_p = candidates.min_by(|a, b| {
        let near = |d: &f64| (d.to_degrees() - latpole).abs();
        near(a).total_cmp(&near(b))
    })?;
    let lon_p = if (d0.abs() - PI / 2.0).abs() < 1e-12 {
        a0
    } else if (lat_p - PI / 2.0).abs() < 1e-12 {
        a0 + dp - PI
    } else if (lat_p + PI / 2.0).abs() < 1e-12 {
        a0 - dp
    } else {
        let y = dp.sin() * t0.cos() / d0.cos();
        let x = (t0.sin() - lat_p.sin() * d0.sin()) / (d0.cos() * lat_p.cos());
        a0 - y.atan2(x)
    };
    Some([lon_p, lat_p])
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
    fn a_wcs_carried_through_a_turn_and_shift_points_at_the_same_sky() {
        // 新しい画素 (x, y) は、元の画像の (y + 20, −x + 300) に当たる（回して、ずらした並び）。
        let map = [[0.0, 1.0, 20.0], [-1.0, 0.0, 300.0]];
        let cards = wcs_through(&lookup(&ORION), map).unwrap();
        let carried: Vec<(&str, &str)> = cards
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let (old, new) = (
            Wcs::read(&lookup(&ORION)).unwrap(),
            Wcs::read(&lookup(&carried)).unwrap(),
        );
        for (x, y) in [(1.0, 1.0), (250.0, 30.0), (120.5, 77.0)] {
            let o = apply(map, x, y);
            let (want, got) = (old.to_sky(o[0], o[1]).unwrap(), new.to_sky(x, y).unwrap());
            assert!(close(got, want, 1e-9), "{got:?} {want:?}");
        }
        assert!(wcs_through(&lookup(&ORION[..4]), map).is_none());
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
    /// 2048 x 1536 の画像の、強めの SIP（隅で約 4 画素ずれる）。
    const SIP_CARDS: [(&str, &str); 22] = [
        ("CTYPE1", "RA---TAN-SIP"),
        ("CTYPE2", "DEC--TAN-SIP"),
        ("CRPIX1", "1024.5"),
        ("CRPIX2", "768.5"),
        ("CRVAL1", "83.82"),
        ("CRVAL2", "-5.39"),
        ("CD1_1", "-2.8E-4"),
        ("CD1_2", "1.2E-5"),
        ("CD2_1", "1.1E-5"),
        ("CD2_2", "2.8E-4"),
        ("A_ORDER", "3"),
        ("A_0_2", "2.1E-6"),
        ("A_1_1", "-3.3E-6"),
        ("A_2_0", "4.0E-6"),
        ("A_3_0", "-1.2E-9"),
        ("A_1_2", "8.0E-10"),
        ("B_ORDER", "3"),
        ("B_0_2", "-2.5E-6"),
        ("B_1_1", "3.1E-6"),
        ("B_2_0", "1.7E-6"),
        ("B_0_3", "9.0E-10"),
        ("B_2_1", "-6.0E-10"),
    ];

    #[test]
    fn sip_distortion_matches_astropy() {
        // astropy 8.0.2 の all_pix2world（1 始まり）。
        let expected = [
            ([1.0, 1.0], [84.097_713_210_9, -5.615_262_483_5]),
            ([2048.0, 1.0], [83.520_744_482_1, -5.594_059_857_9]),
            ([1.0, 1536.0], [84.114_494_402_7, -5.186_810_840_3]),
            ([2048.0, 1536.0], [83.540_965_649_6, -5.163_012_453_5]),
            ([300.25, 1200.75], [84.027_765_030_2, -5.277_030_394_5]),
        ];
        let wcs = Wcs::read(&lookup(&SIP_CARDS)).unwrap();
        for ([x, y], sky) in expected {
            let p = wcs.to_sky(x, y).unwrap();
            assert!(close(p, sky, 1e-8), "({x}, {y}): {p:?}");
        }
    }

    #[test]
    fn sip_moves_the_corners_by_a_few_pixels() {
        let plain: Vec<(&str, &str)> = SIP_CARDS[..10]
            .iter()
            .map(|&(k, v)| (k, v.trim_end_matches("-SIP")))
            .collect();
        let with = Wcs::read(&lookup(&SIP_CARDS))
            .unwrap()
            .to_sky(1.0, 1.0)
            .unwrap();
        let without = Wcs::read(&lookup(&plain))
            .unwrap()
            .to_sky(1.0, 1.0)
            .unwrap();
        // astropy で SIP を外した隅（84.098 703 472 3, -5.616 091 307 4）。
        assert!(close(without, [84.098_703_472_3, -5.616_091_307_4], 1e-8));
        let pixels = (with[1] - without[1]).abs() / 2.8e-4;
        assert!(pixels > 2.0, "{pixels}");
    }

    #[test]
    fn tpv_distortion_matches_astropy() {
        let cards = [
            ("CTYPE1", "RA---TPV"),
            ("CTYPE2", "DEC--TPV"),
            ("CRPIX1", "-50.0"),
            ("CRPIX2", "4200.0"),
            ("CRVAL1", "150.1"),
            ("CRVAL2", "2.2"),
            ("CD1_1", "0.0"),
            ("CD1_2", "7.3E-5"),
            ("CD2_1", "-7.3E-5"),
            ("CD2_2", "0.0"),
            ("PV1_0", "-1.2E-4"),
            ("PV1_1", "1.0012"),
            ("PV1_2", "3.0E-4"),
            ("PV1_4", "-2.0E-3"),
            ("PV1_5", "1.5E-3"),
            ("PV1_6", "-8.0E-4"),
            ("PV1_7", "-1.0E-2"),
            ("PV1_9", "-9.0E-3"),
            ("PV1_11", "4.0E-3"),
            ("PV2_0", "2.0E-4"),
            ("PV2_1", "1.0009"),
            ("PV2_2", "-2.0E-4"),
            ("PV2_4", "-1.2E-3"),
            ("PV2_5", "9.0E-4"),
            ("PV2_6", "-5.0E-4"),
            ("PV2_7", "-9.5E-3"),
            ("PV2_9", "-1.1E-2"),
            ("PV2_11", "3.0E-3"),
        ];
        // astropy 8.0.2 の all_pix2world（1 始まり）。
        let expected = [
            ([1.0, 1.0], [149.792_978_449_9, 2.196_547_750_8]),
            ([2048.0, 1.0], [149.793_124_755_2, 2.047_220_733_3]),
            ([1.0, 4096.0], [150.092_272_018_6, 2.196_475_132_4]),
            ([2048.0, 4096.0], [150.092_226_904_8, 2.046_927_937_3]),
            ([1000.0, 2000.0], [149.938_988_913_5, 2.123_538_866_8]),
        ];
        let wcs = Wcs::read(&lookup(&cards)).unwrap();
        for ([x, y], sky) in expected {
            let p = wcs.to_sky(x, y).unwrap();
            assert!(close(p, sky, 1e-8), "({x}, {y}): {p:?}");
        }
    }

    /// 銀河座標の全天図（800 x 400、0.45°）。
    fn all_sky(code: &str) -> Vec<(&'static str, String)> {
        vec![
            ("NAXIS1", "800".into()),
            ("NAXIS2", "400".into()),
            ("CTYPE1", format!("GLON-{code}")),
            ("CTYPE2", format!("GLAT-{code}")),
            ("CRPIX1", "400.5".into()),
            ("CRPIX2", "200.5".into()),
            ("CRVAL1", "0.0".into()),
            ("CRVAL2", "0.0".into()),
            ("CDELT1", "-0.45".into()),
            ("CDELT2", "0.45".into()),
        ]
    }

    fn owned(cards: &[(&'static str, String)]) -> impl Fn(&str) -> Option<String> {
        let cards = cards.to_vec();
        move |key| {
            cards
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn aitoff_and_mollweide_match_astropy_in_galactic_and_icrs() {
        // astropy 8.0.2 の all_pix2world（銀河座標）と、SkyCoord で ICRS にした値。
        let expected = [
            (
                "AIT",
                [
                    (
                        [100.0, 200.0],
                        [144.637_397_357_8, -0.181_660_503_7],
                        [53.791_022_822_8, 55.632_677_977_1],
                    ),
                    (
                        [700.0, 300.0],
                        [180.413_581_718, 33.601_510_271],
                        [125.182_040_382, 40.604_629_888_9],
                    ),
                    (
                        [250.5, 120.25],
                        [80.488_278_365_8, -34.654_907_976_2],
                        [337.518_754_819, 16.317_360_656_4],
                    ),
                ],
            ),
            (
                "MOL",
                [
                    (
                        [100.0, 200.0],
                        [150.197_790_388_9, -0.202_571_332_8],
                        [61.138_839_530_5, 52.137_381_266_7],
                    ),
                    (
                        [700.0, 300.0],
                        [180.389_897_366_8, 41.752_509_253_7],
                        [135.939_913_133, 41.360_931_442_6],
                    ),
                    (
                        [250.5, 120.25],
                        [83.751_287_687_4, -33.234_305_990_1],
                        [338.858_885_013, 19.094_881_075_3],
                    ),
                ],
            ),
        ];
        for (code, points) in expected {
            let wcs = Wcs::read(&owned(&all_sky(code))).unwrap();
            for ([x, y], galactic, icrs) in points {
                let g = wcs.to_frame(x, y).unwrap();
                assert!(close(g, galactic, 1e-8), "{code} ({x}, {y}): {g:?}");
                let p = wcs.to_sky(x, y).unwrap();
                assert!(close(p, icrs, 2e-7), "{code} ({x}, {y}): {p:?}");
            }
            // 楕円の外（上の端の真ん中より上）は空の外。
            assert_eq!(wcs.to_sky(400.5, 390.0), None, "{code}");
        }
    }

    #[test]
    fn car_in_galactic_puts_the_centre_at_the_galactic_centre() {
        let mut cards = all_sky("CAR");
        cards[0].1 = "360".into();
        cards[1].1 = "180".into();
        cards[4].1 = "180.5".into();
        cards[5].1 = "90.5".into();
        cards[8].1 = "-1.0".into();
        cards[9].1 = "1.0".into();
        let wcs = Wcs::read(&owned(&cards)).unwrap();
        let expected = [
            (
                [1.0, 1.0],
                [179.5, -89.5],
                [13.327_176_806_8, -26.852_005_36],
            ),
            (
                [100.25, 40.75],
                [80.25, -49.75],
                [346.803_252_525_7, 4.284_477_720_1],
            ),
            (
                [180.5, 90.5],
                [0.0, 0.0],
                [266.404_988_286_5, -28.936_177_761_8],
            ),
        ];
        for ([x, y], galactic, icrs) in expected {
            assert!(
                close(wcs.to_frame(x, y).unwrap(), galactic, 1e-8),
                "({x}, {y})"
            );
            let p = wcs.to_sky(x, y).unwrap();
            assert!(close(p, icrs, 2e-7), "({x}, {y}): {p:?}");
        }
    }

    #[test]
    fn cylindrical_and_pseudocylindrical_away_from_the_equator_match_astropy() {
        let car = [
            ("CTYPE1", "RA---CAR"),
            ("CTYPE2", "DEC--CAR"),
            ("CRPIX1", "50.5"),
            ("CRPIX2", "20.5"),
            ("CRVAL1", "120.0"),
            ("CRVAL2", "30.0"),
            ("CDELT1", "-0.5"),
            ("CDELT2", "0.5"),
        ];
        let wcs = Wcs::read(&lookup(&car)).unwrap();
        for ([x, y], sky) in [
            ([1.0, 1.0], [145.636_317_251_6, 17.508_785_275_3]),
            ([100.0, 40.0], [89.137_130_925_8, 36.453_769_111_1]),
            ([10.0, 35.0], [144.805_139_959_7, 35.074_196_682_8]),
        ] {
            let p = wcs.to_sky(x, y).unwrap();
            assert!(close(p, sky, 1e-8), "CAR ({x}, {y}): {p:?}");
        }
        // 南の基準点と LATPOLE（極の解を南の方に選ぶ）。
        let ait = [
            ("CTYPE1", "RA---AIT"),
            ("CTYPE2", "DEC--AIT"),
            ("CRPIX1", "100.5"),
            ("CRPIX2", "50.5"),
            ("CRVAL1", "266.4"),
            ("CRVAL2", "-28.9"),
            ("CDELT1", "-0.2"),
            ("CDELT2", "0.2"),
            ("LATPOLE", "-50.0"),
        ];
        let wcs = Wcs::read(&lookup(&ait)).unwrap();
        for ([x, y], sky) in [
            ([1.0, 1.0], [245.581_452_924_9, -17.268_443_715_6]),
            ([200.0, 100.0], [291.430_107_124_7, -36.663_172_922_6]),
            ([100.5, 50.5], [266.4, -28.9]),
            ([30.0, 80.0], [249.370_666_230_2, -33.777_736_159_2]),
        ] {
            let p = wcs.to_sky(x, y).unwrap();
            assert!(close(p, sky, 1e-8), "AIT ({x}, {y}): {p:?}");
        }
    }

    #[test]
    fn galactic_tan_images_are_placed_in_icrs() {
        let cards = [
            ("CTYPE1", "GLON-TAN"),
            ("CTYPE2", "GLAT-TAN"),
            ("CRPIX1", "50.5"),
            ("CRPIX2", "50.5"),
            ("CRVAL1", "184.557"),
            ("CRVAL2", "-5.784"),
            ("CDELT1", "-0.01"),
            ("CDELT2", "0.01"),
        ];
        let wcs = Wcs::read(&lookup(&cards)).unwrap();
        // かに星雲のあたり。astropy の SkyCoord（galactic → icrs）。
        assert!(close(
            wcs.to_sky(50.5, 50.5).unwrap(),
            [83.633_145_275_2, 22.015_075_728_1],
            2e-7
        ));
        assert!(close(
            wcs.to_sky(1.0, 1.0).unwrap(),
            [83.468_777_441_2, 21.331_943_369_4],
            2e-7
        ));
    }

    #[test]
    fn all_sky_maps_are_known_but_have_no_outline() {
        for code in ["CAR", "AIT", "MOL"] {
            let s = sky(&owned(&all_sky(code))).unwrap();
            assert!(s.certain && s.wide, "{code}");
            assert!(s.outline.is_empty(), "{code}");
            assert!(
                close(s.center, [266.404_988_286_5, -28.936_177_761_8], 1e-5),
                "{code}"
            );
        }
    }

    #[test]
    fn maps_on_the_same_grid_line_up_without_solving() {
        let cards = all_sky("AIT");
        assert_eq!(
            pixel_map(&owned(&cards), &owned(&cards)),
            Some([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]])
        );
        let mut moved = cards.clone();
        moved[6].1 = "10.0".into();
        assert_eq!(pixel_map(&owned(&cards), &owned(&moved)), None);
    }

    #[test]
    fn a_galactic_wcs_carried_through_a_shift_stays_galactic() {
        let cards = all_sky("CAR");
        let map = [[1.0, 0.0, 10.0], [0.0, 1.0, 5.0]];
        let carried = wcs_through(&owned(&cards), map).unwrap();
        let carried: Vec<(&str, &str)> = carried
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let (old, new) = (
            Wcs::read(&owned(&cards)).unwrap(),
            Wcs::read(&lookup(&carried)).unwrap(),
        );
        assert_eq!(new.frame, Frame::Galactic);
        for (x, y) in [(1.0, 1.0), (300.0, 150.0)] {
            let o = apply(map, x, y);
            let (want, got) = (old.to_sky(o[0], o[1]).unwrap(), new.to_sky(x, y).unwrap());
            assert!(close(got, want, 1e-9), "{got:?} {want:?}");
        }
    }

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

    /// `code` の投影で、`size` の画像の中心を `crval` にした WCS のカード。
    fn centred(
        frame: (&str, &str),
        code: &str,
        size: [u32; 2],
        step: f64,
        crval: [f64; 2],
    ) -> Vec<(&'static str, String)> {
        vec![
            ("CTYPE1", format!("{:-<5}{code}", frame.0)),
            ("CTYPE2", format!("{:-<5}{code}", frame.1)),
            ("CRPIX1", format!("{}", f64::from(size[0]) / 2.0 + 0.5)),
            ("CRPIX2", format!("{}", f64::from(size[1]) / 2.0 + 0.5)),
            ("CDELT1", format!("{}", -step)),
            ("CDELT2", format!("{step}")),
            ("CRVAL1", format!("{}", crval[0])),
            ("CRVAL2", format!("{}", crval[1])),
        ]
    }

    #[test]
    fn ncp_and_slanted_sin_match_astropy() {
        // astropy 8.0.2 の all_pix2world（NCP は wcslib が PV2_2 = cot δ0 の SIN に直して読む）。
        let ncp = centred(("RA", "DEC"), "NCP", [200, 150], 0.01, [150.0, 35.0]);
        let mut slanted = centred(("RA", "DEC"), "SIN", [200, 150], 0.01, [150.0, 35.0]);
        slanted.push(("PV2_1", "0.2".into()));
        slanted.push(("PV2_2", "-0.3".into()));
        let expected = [
            (
                ncp,
                [
                    ([1.0, 1.0], [151.203_534_370_4, 34.229_354_992_1]),
                    ([200.0, 150.0], [148.774_355_575_9, 35.720_003_219_7]),
                    ([37.25, 120.5], [150.776_362_210_3, 35.440_119_055_5]),
                ],
            ),
            (
                slanted,
                [
                    ([1.0, 1.0], [151.200_633_256_2, 34.253_030_960_1]),
                    ([200.0, 150.0], [148.770_653_920_9, 35.742_956_796_2]),
                    ([37.25, 120.5], [150.775_157_816_8, 35.449_132_364_5]),
                ],
            ),
        ];
        for (cards, points) in expected {
            let wcs = Wcs::read(&owned(&cards)).unwrap();
            for ([x, y], sky) in points {
                let p = wcs.to_sky(x, y).unwrap();
                assert!(close(p, sky, 1e-8), "{} ({x}, {y}): {p:?}", cards[0].1);
            }
        }
    }

    #[test]
    fn sfl_and_old_gls_match_astropy() {
        // GLS は基準の緯度で格子を傾けず、地図を平行に動かすだけなので、SFL と値が違う。
        let expected = [
            (
                ("RA", "DEC"),
                "SFL",
                [120.0, 30.0],
                [
                    ([1.0, 1.0], [168.329_272_605_2, -5.966_200_741_3]),
                    ([400.0, 200.0], [47.945_180_836_3, 38.654_895_051_9]),
                    ([77.5, 160.25], [161.196_816_554_1, 39.323_962_605_3]),
                ],
            ),
            (
                ("RA", "DEC"),
                "GLS",
                [120.0, 30.0],
                [
                    ([1.0, 1.0], [170.075_191_444_5, 5.125]),
                    ([400.0, 200.0], [33.315_470_635_8, 54.875]),
                    ([77.5, 160.25], [163.439_707_526_9, 44.9375]),
                ],
            ),
            (
                ("GLON", "GLAT"),
                "GLS",
                [30.0, -10.0],
                [
                    ([1.0, 1.0], [90.793_408_39, -34.875]),
                    ([400.0, 200.0], [338.395_644_257, 14.875]),
                    ([77.5, 160.25], [60.864_532_801_3, 4.9375]),
                ],
            ),
        ];
        for (frame, code, crval, points) in expected {
            let wcs = Wcs::read(&owned(&centred(frame, code, [400, 200], 0.25, crval))).unwrap();
            for ([x, y], at) in points {
                let p = wcs.to_frame(x, y).unwrap();
                assert!(close(p, at, 1e-8), "{code} ({x}, {y}): {p:?}");
            }
        }
    }

    #[test]
    fn a_slanted_sin_carried_through_a_shift_keeps_its_slant() {
        let ncp = centred(("RA", "DEC"), "NCP", [200, 150], 0.01, [150.0, 35.0]);
        let shifted = wcs_through(&owned(&ncp), [[1.0, 0.0, 10.0], [0.0, 1.0, 5.0]]).unwrap();
        let again =
            Wcs::read(&|k: &str| shifted.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()))
                .unwrap();
        let before = Wcs::read(&owned(&ncp)).unwrap().to_sky(30.0, 40.0).unwrap();
        assert!(close(again.to_sky(20.0, 35.0).unwrap(), before, 1e-9));
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
        cards[2] = ("CTYPE1", "RA---HPX");
        cards[3] = ("CTYPE2", "DEC--HPX");
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
