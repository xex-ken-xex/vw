//! 検出器を並べたカメラ（モザイク）の HDU を、1 枚の画像に組み立てる。
//!
//! 検出器（増幅器）ごとの HDU に、全体の中の位置 `DETSEC`、使う画素 `DATASEC`、オーバースキャン
//! `BIASSEC` が書いてある（どれも `[x1:x2,y1:y2]`、1 から、逆向きもある）。
//!
//! - 画像の HDU がどれも 1 面で `DETSEC` を持ち、2 つ以上あり、互いに重ならないときだけ組み立てる
//! - 増幅器ごとに `BIASSEC` の中央値を引いてから `DATASEC` で切り、`DETSEC` の向きで置く
//! - 検出器のすきまは値なし（NaN、欠損の色で描く）
//! - 全体が上限の画素数を超えたら、2 × 2 画素ずつまとめて（平均して）縮める。値のない画素は数えない

use vw_core::sky::SkyMap;
use vw_core::{DecodeError, PixelData};

use crate::Image;
use crate::header::Hdu;

/// [`crate::Part::index`] で、組み立てた全体を表す番号（HDU の番号とは重ならない）。
pub const MOSAIC: usize = usize::MAX;

/// `[x1:x2,y1:y2]`（1 から、両端を含む）。`x1 > x2` なら逆向き。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub x: (i64, i64),
    pub y: (i64, i64),
}

impl Section {
    pub fn parse(text: &str) -> Option<Self> {
        let inner = text.trim().strip_prefix('[')?.strip_suffix(']')?;
        let (xs, ys) = inner.split_once(',')?;
        let range = |s: &str| -> Option<(i64, i64)> {
            let (a, b) = s.trim().split_once(':')?;
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        };
        let section = Self {
            x: range(xs)?,
            y: range(ys)?,
        };
        (section.x.0 >= 1 && section.x.1 >= 1 && section.y.0 >= 1 && section.y.1 >= 1)
            .then_some(section)
    }

    pub fn width(&self) -> usize {
        (self.x.1 - self.x.0).unsigned_abs() as usize + 1
    }

    pub fn height(&self) -> usize {
        (self.y.1 - self.y.0).unsigned_abs() as usize + 1
    }

    fn x_range(&self) -> (i64, i64) {
        (self.x.0.min(self.x.1), self.x.0.max(self.x.1))
    }

    fn y_range(&self) -> (i64, i64) {
        (self.y.0.min(self.y.1), self.y.0.max(self.y.1))
    }

    fn overlaps(&self, other: &Section) -> bool {
        let (ax, bx) = (self.x_range(), other.x_range());
        let (ay, by) = (self.y_range(), other.y_range());
        ax.0 <= bx.1 && bx.0 <= ax.1 && ay.0 <= by.1 && by.0 <= ay.1
    }

    /// 区画の中の `i` 番目（0 から、区画の書いた向きに数える）の、x の座標（1 から）。
    fn x_at(&self, i: usize) -> i64 {
        if self.x.0 <= self.x.1 {
            self.x.0 + i as i64
        } else {
            self.x.0 - i as i64
        }
    }

    fn y_at(&self, i: usize) -> i64 {
        if self.y.0 <= self.y.1 {
            self.y.0 + i as i64
        } else {
            self.y.0 - i as i64
        }
    }
}

/// 検出器（増幅器）1 つ。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Detector {
    /// HDU の番号。
    pub(crate) index: usize,
    pub(crate) detsec: Section,
    pub(crate) datasec: Section,
    pub(crate) biassec: Option<Section>,
}

/// 組み立て方。全体の大きさは `DETSEC` の外接の長方形。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layout {
    pub(crate) detectors: Vec<Detector>,
    /// 全体の左下の `DETSEC` の座標（1 から）。
    pub(crate) origin: (i64, i64),
    pub(crate) width: usize,
    pub(crate) height: usize,
}

/// 組み立てられるファイルなら、その組み立て方。
pub(crate) fn layout(hdus: &[Hdu]) -> Option<Layout> {
    let mut detectors = Vec::new();
    for index in 0..hdus.len() {
        let Ok(Some(image)) = crate::image_at(hdus, index, 2) else {
            continue;
        };
        if image.planes != 1 {
            return None;
        }
        let detsec = Section::parse(image.hdu.get("DETSEC")?)?;
        let full = Section {
            x: (1, image.width as i64),
            y: (1, image.height as i64),
        };
        let datasec = match image.hdu.get("DATASEC") {
            Some(s) => Section::parse(s)?,
            None => full,
        };
        let inside = |s: &Section| {
            let (x, y) = (s.x_range(), s.y_range());
            x.1 <= full.x.1 && y.1 <= full.y.1
        };
        // DATASEC と DETSEC の大きさが違うのは、検出器をまとめて読んだ（ビニングした）ファイル。
        // 置く場所の縮尺が分からなくなるので組み立てない。
        if !inside(&datasec)
            || datasec.width() != detsec.width()
            || datasec.height() != detsec.height()
        {
            return None;
        }
        let biassec = image
            .hdu
            .get("BIASSEC")
            .and_then(Section::parse)
            .filter(inside);
        detectors.push(Detector {
            index,
            detsec,
            datasec,
            biassec,
        });
    }
    if detectors.len() < 2 {
        return None;
    }
    for (i, a) in detectors.iter().enumerate() {
        if detectors[i + 1..]
            .iter()
            .any(|b| a.detsec.overlaps(&b.detsec))
        {
            return None;
        }
    }
    let x0 = detectors.iter().map(|d| d.detsec.x_range().0).min()?;
    let x1 = detectors.iter().map(|d| d.detsec.x_range().1).max()?;
    let y0 = detectors.iter().map(|d| d.detsec.y_range().0).min()?;
    let y1 = detectors.iter().map(|d| d.detsec.y_range().1).max()?;
    Some(Layout {
        detectors,
        origin: (x0, y0),
        width: (x1 - x0 + 1) as usize,
        height: (y1 - y0 + 1) as usize,
    })
}

/// 組み立てた全体。値は FITS の向き（最初の行が下）で並ぶ。
pub(crate) struct Assembled {
    pub(crate) values: Vec<f32>,
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// 縮めた倍率（1 は縮めていない、2 は 2 × 2 画素を 1 つに）。
    pub(crate) bin: usize,
}

/// `layout` のとおりに組み立てる。全体が `max_pixels` を超えたら、収まるまで 2 × 2 ずつまとめる。
///
/// 縮める倍率を先に決めて、検出器の画素を縮めた先に足していく。全体を縮めずに組み立ててから
/// 縮めると、数億画素のカメラでは組み立てるだけで数 GB を使うため。
pub(crate) fn assemble(
    data: &[u8],
    hdus: &[Hdu],
    layout: &Layout,
    max_pixels: usize,
) -> Result<Assembled, DecodeError> {
    let mut bin = 1;
    while (layout.width / bin) * (layout.height / bin) > max_pixels.max(1)
        && layout.width / bin > 1
        && layout.height / bin > 1
    {
        bin *= 2;
    }
    // 端の、倍率に満たない余りの画素は捨てる。
    let (w, h) = (layout.width / bin, layout.height / bin);
    vw_core::limits::check_image(w as u64, h as u64, 8)?;
    let mut sums = vec![0.0f32; w * h];
    let mut counts = vec![0u32; w * h];
    for d in &layout.detectors {
        let image = crate::image_at(hdus, d.index, 2)?
            .ok_or_else(|| DecodeError::Invalid("FITS mosaic detector is not an image".into()))?;
        let pixels = plane_values(data, &image)?;
        let bias = d
            .biassec
            .map_or(0.0, |b| median_in(&pixels, image.width, &b));
        for j in 0..d.datasec.height() {
            let sy = d.datasec.y_at(j) as usize - 1;
            let ty = (d.detsec.y_at(j) - layout.origin.1) as usize / bin;
            if ty >= h {
                continue;
            }
            for i in 0..d.datasec.width() {
                let tx = (d.detsec.x_at(i) - layout.origin.0) as usize / bin;
                let sx = d.datasec.x_at(i) as usize - 1;
                let v = pixels[sy * image.width + sx];
                if tx < w && v.is_finite() {
                    sums[ty * w + tx] += v - bias;
                    counts[ty * w + tx] += 1;
                }
            }
        }
    }
    let values = sums
        .into_iter()
        .zip(counts)
        .map(|(sum, n)| if n == 0 { f32::NAN } else { sum / n as f32 })
        .collect();
    Ok(Assembled {
        values,
        width: w,
        height: h,
        bin,
    })
}

/// HDU の 1 面目の値（`BSCALE`・`BZERO` をかけた実数）。
fn plane_values(data: &[u8], image: &Image) -> Result<Vec<f32>, DecodeError> {
    let stored = if image.compressed {
        crate::compressed_plane(data, image, 0)?
    } else {
        crate::plain_plane(data, image, 0)?
    };
    Ok(match crate::to_pixels(stored, image.hdu) {
        PixelData::U8(v) => v.into_iter().map(f32::from).collect(),
        PixelData::U16(v) => v.into_iter().map(f32::from).collect(),
        PixelData::F32(v) => v,
    })
}

/// 区画 `section` の値の中央値。値がなければ 0。
fn median_in(pixels: &[f32], width: usize, section: &Section) -> f32 {
    let (x0, x1) = section.x_range();
    let (y0, y1) = section.y_range();
    let mut v: Vec<f32> = (y0..=y1)
        .flat_map(|y| (x0..=x1).map(move |x| ((y - 1) as usize, (x - 1) as usize)))
        .filter_map(|(y, x)| pixels.get(y * width + x).copied())
        .filter(|v| v.is_finite())
        .collect();
    if v.is_empty() {
        return 0.0;
    }
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, f32::total_cmp).1
}

/// 組み立てた全体の画素と空の座標。カーソルの下の検出器の WCS で出す。
#[derive(Debug)]
pub(crate) struct MosaicSky {
    /// 検出器ごとの、`DETSEC` と `DATASEC` と、その HDU の WCS（上下を返さない向き）。
    detectors: Vec<(Section, Section, crate::sky::ImageSky)>,
    origin: (i64, i64),
    /// 描く画像の高さ（縮めた後）。
    rows: usize,
    bin: usize,
    flipped: bool,
}

impl MosaicSky {
    pub(crate) fn read(
        hdus: &[Hdu],
        layout: &Layout,
        bin: usize,
        rows: usize,
        flipped: bool,
    ) -> Option<Self> {
        let detectors: Vec<_> = layout
            .detectors
            .iter()
            .filter_map(|d| {
                let hdu = &hdus[d.index];
                let lookup =
                    |key: &str| hdu.get(key).or_else(|| hdus[0].get(key)).map(str::to_owned);
                let height = hdu.int("NAXIS2")? as usize;
                Some((
                    d.detsec,
                    d.datasec,
                    crate::sky::ImageSky::read(&lookup, height, false)?,
                ))
            })
            .collect();
        (!detectors.is_empty()).then_some(Self {
            detectors,
            origin: layout.origin,
            rows,
            bin,
            flipped,
        })
    }

    /// 表示の画素（0 から）を、その下の検出器と、その HDU の画素（0 から、ファイルの並び）にする。
    fn detector_pixel(&self, x: f64, y: f64) -> Option<(&crate::sky::ImageSky, f64, f64)> {
        // 縮めた画像の中で上下を戻してから、縮める前の画素（まとめた画素の真ん中）にする。
        let y = if self.flipped {
            self.rows as f64 - 1.0 - y
        } else {
            y
        };
        let bin = self.bin as f64;
        let mx = (x + 0.5) * bin - 0.5;
        let fy = (y + 0.5) * bin - 0.5;
        let dx = self.origin.0 as f64 + mx;
        let dy = self.origin.1 as f64 + fy;
        self.detectors.iter().find_map(|(det, data, sky)| {
            let (x0, x1) = det.x_range();
            let (y0, y1) = det.y_range();
            let inside = |v: f64, a: i64, b: i64| v >= a as f64 - 0.5 && v <= b as f64 + 0.5;
            if !inside(dx, x0, x1) || !inside(dy, y0, y1) {
                return None;
            }
            // DETSEC の書いた向きに数えた位置 → DATASEC の同じ位置。
            let along = |v: f64, s: (i64, i64)| {
                if s.0 <= s.1 {
                    v - s.0 as f64
                } else {
                    s.0 as f64 - v
                }
            };
            let place = |n: f64, s: (i64, i64)| {
                if s.0 <= s.1 {
                    s.0 as f64 + n
                } else {
                    s.0 as f64 - n
                }
            };
            let sx = place(along(dx, det.x), data.x) - 1.0;
            let sy = place(along(dy, det.y), data.y) - 1.0;
            Some((sky, sx, sy))
        })
    }
}

impl SkyMap for MosaicSky {
    fn at(&self, x: f64, y: f64) -> Option<[f64; 2]> {
        let (sky, x, y) = self.detector_pixel(x, y)?;
        sky.at(x, y)
    }

    fn galactic(&self, x: f64, y: f64) -> Option<[f64; 2]> {
        let (sky, x, y) = self.detector_pixel(x, y)?;
        sky.galactic(x, y)
    }
}
