//! 灰色の画像を 2〜3 枚、赤・緑・青に割り当てて 1 枚の色の画像に重ねる（天体のナローバンドなど）。
//!
//! 合成した画像もパスで表す（[`crate::frames`] と同じ理由）。スロット・全画面・コピーが、
//! 普通の画像と同じ読み込みの道を通れるようにするため。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vw_core::{ColorModel, Metadata, Palette, PixelData, VwImage};

use crate::colormap::{Applied, Kind, Pick, Scale};

/// 区切り。`?` と `|` は Windows のファイル名に使えないので、実在のファイルと取り違えない。
const MARK: &str = "?rgb=";
const SEP: char = '|';

/// メタデータの見出し。
pub const GROUP: &str = "RGB composite";
/// 読み込みの結果に付けるプラグインの名前。
pub const PLUGIN: &str = "RGB composite";

pub const CHANNELS: [&str; 3] = ["Red", "Green", "Blue"];

/// 赤・緑・青に割り当てたファイル（`None` はその色を使わない）。
pub type Files = [Option<PathBuf>; 3];

/// `files` を重ねた画像を指すパス。先頭は最初のファイル（フォルダやスロットの見出しに使われる）。
pub fn path(files: &Files) -> Option<PathBuf> {
    let first = files.iter().flatten().next()?;
    let mut s = first.as_os_str().to_owned();
    s.push(MARK);
    for (i, f) in files.iter().enumerate() {
        if i > 0 {
            s.push(SEP.to_string());
        }
        if let Some(f) = f {
            s.push(encode(&f.to_string_lossy()));
        }
    }
    Some(PathBuf::from(s))
}

/// パスの区切りを書き換えて、合成のパスを 1 つのファイル名にする（親のフォルダが最初の
/// ファイルのフォルダのままになり、スロットの見出しやフォルダの扱いが普通の画像と同じになる）。
fn encode(path: &str) -> String {
    path.replace('%', "%25")
        .replace('/', "%2F")
        .replace('\\', "%5C")
}

fn decode(s: &str) -> String {
    s.replace("%2F", "/")
        .replace("%5C", "\\")
        .replace("%25", "%")
}

/// 合成を指すパスなら、赤・緑・青のファイルに分ける。
pub fn split(path: &Path) -> Option<Files> {
    let s = path.to_str()?;
    let (_, list) = s.split_once(MARK)?;
    let parts: Vec<&str> = list.split(SEP).collect();
    let [r, g, b] = parts.as_slice() else {
        return None;
    };
    let file = |p: &str| (!p.is_empty()).then(|| PathBuf::from(decode(p)));
    let files = [file(r), file(g), file(b)];
    files.iter().any(Option::is_some).then_some(files)
}

/// 合成を指すパスの、画面に出す名前（「R 1-SII + G 2-Ha + B 3-OIII」）。合成でなければ `None`。
pub fn title(path: &Path) -> Option<String> {
    let files = split(path)?;
    let parts: Vec<String> = files
        .iter()
        .zip(["R", "G", "B"])
        .filter_map(|(f, c)| {
            let stem = f.as_ref()?.file_stem()?.to_string_lossy().into_owned();
            Some(format!("{c} {stem}"))
        })
        .collect();
    Some(parts.join(" + "))
}

/// フィルタの名前から決まる色（0 赤、1 緑、2 青）。ナローバンドはハッブルパレット
/// （SII を赤、Hα を緑、OIII を青）にする。
pub fn channel_of_filter(filter: &str) -> Option<usize> {
    let f: String = filter
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    match f.as_str() {
        "sii" | "s2" | "sulfur" | "sulphur" | "r" | "red" => Some(0),
        "ha" | "halpha" | "hα" | "h" | "g" | "green" | "v" => Some(1),
        "oiii" | "o3" | "oxygen" | "b" | "blue" => Some(2),
        _ => None,
    }
}

/// `files` を色に割り当てる。フィルタで決まるものを先に置き、決まらないもの（や、同じ色に
/// 2 枚が当たったときの後ろのもの）は空いている色に、並んでいる順に入れる。
pub fn assign(files: &[(PathBuf, Option<String>)]) -> Files {
    let mut out: Files = Default::default();
    let mut rest = Vec::new();
    for (path, filter) in files.iter().take(3) {
        match filter.as_deref().and_then(channel_of_filter) {
            Some(c) if out[c].is_none() => out[c] = Some(path.clone()),
            _ => rest.push(path.clone()),
        }
    }
    for path in rest {
        if let Some(free) = out.iter_mut().find(|c| c.is_none()) {
            *free = Some(path);
        }
    }
    out
}

/// FITS の `FILTER`。画像の HDU のもの（後ろの見出し）を先に見る。
pub fn filter_of(meta: &Metadata) -> Option<String> {
    meta.entries
        .iter()
        .rev()
        .find(|(g, k, _)| g.starts_with(vw_fits::GROUP) && k == "FILTER")
        .map(|(_, _, v)| v.trim().trim_matches('\'').trim().to_owned())
        .filter(|f| !f.is_empty())
}

/// 合成の 1 色。
#[derive(Debug, Clone)]
pub struct Channel {
    pub path: PathBuf,
    pub filter: Option<String>,
    /// ダーク・フラットを当てたか（FITS の校正の状態の文）。
    pub calibration: Option<String>,
    /// 黒と白（ファイルの値の単位）。ゲージは黒からこの色までの帯で描く。
    pub levels: Applied,
    /// この画像の大きさ。
    pub size: (u32, u32),
    /// 基準（最初の色）の画像に重ねるための動かし方。
    pub placement: Placement,
    pub aligned: Aligned,
}

/// 基準の画像の画素 (列, 行) が、この画像のどこに写っているか（画面の向き、画素の中心が整数）。
/// `[[a, b, c], [d, e, f]]` で、列' = a 列 + b 行 + c、行' = d 列 + e 行 + f。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement(pub [[f64; 3]; 2]);

impl Placement {
    const SAME: Placement = Placement([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);

    fn shifted(dx: f64, dy: f64) -> Self {
        Placement([[1.0, 0.0, dx], [0.0, 1.0, dy]])
    }

    fn at(&self, col: f64, row: f64) -> [f64; 2] {
        let m = &self.0;
        [
            m[0][0] * col + m[0][1] * row + m[0][2],
            m[1][0] * col + m[1][1] * row + m[1][2],
        ]
    }

    /// 基準の画像の中心が、この画像の中心から何画素ずれて写っているか（右、下）。
    pub fn shift(&self, reference: (u32, u32), this: (u32, u32)) -> [f64; 2] {
        let center = |(w, h): (u32, u32)| [(f64::from(w) - 1.0) / 2.0, (f64::from(h) - 1.0) / 2.0];
        let (r, t) = (center(reference), center(this));
        let p = self.at(r[0], r[1]);
        [p[0] - t[0], p[1] - t[1]]
    }

    /// 基準の画像に対して、この画像が何度回っているか（画面で時計回り）。
    pub fn rotation(&self) -> f64 {
        let m = &self.0;
        // 基準の右向きが、この画像でどちらを向くか。画面は下向きが正なので、時計回りが正の角になる。
        let d = m[1][0].atan2(m[0][0]).to_degrees();
        if d.abs() < 1e-9 { 0.0 } else { d }
    }

    /// 基準の 1 画素が、この画像の何画素ぶんか（この画像の画素の細かさの比）。
    pub fn scale(&self) -> f64 {
        let m = &self.0;
        (m[0][0] * m[1][1] - m[0][1] * m[1][0]).abs().sqrt()
    }
}

/// 位置の合わせ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aligned {
    /// 基準の画像（動かさない）。
    Reference,
    /// どちらにも WCS があり、空の座標で合わせた。
    Wcs,
    /// WCS がないので、そのまま重ねた。
    Unaligned,
    /// WCS がなく、大きさが違うので、中心を揃えた。
    Centered,
}

impl Aligned {
    pub fn title(self) -> &'static str {
        match self {
            Aligned::Reference => "Reference (not moved)",
            Aligned::Wcs => "Moved by the sky coordinates (WCS)",
            Aligned::Unaligned => "Not moved (no WCS in both images)",
            Aligned::Centered => "Centers put together (no WCS in both images)",
        }
    }
}

/// 合成の内訳。パイプラインの図とゲージに使う。
#[derive(Debug, Clone)]
pub struct Composite {
    pub channels: [Option<Channel>; 3],
    /// 合成した画像の大きさ（基準の画像の大きさ）。
    pub size: (u32, u32),
}

impl Composite {
    pub fn files(&self) -> Files {
        std::array::from_fn(|c| self.channels[c].as_ref().map(|ch| ch.path.clone()))
    }
}

/// 黒からその色までの表（ゲージに使う）。
pub fn ramp(channel: usize) -> Arc<Palette> {
    let mut top = [0u8; 3];
    top[channel] = 255;
    Arc::new(
        Palette::from_stops(CHANNELS[channel], &[(0.0, [0, 0, 0]), (1.0, top)])
            .expect("two stops make a palette"),
    )
}

/// 読んだ灰色の画像 `images`（赤・緑・青、`None` は使わない色）を重ねる。`windows` は
/// スロットで色ごとに変えた黒と白。最初の画像を基準にして、その大きさで出す。ほかの画像は、
/// WCS があればずれ・回転・倍率を合わせ、なければ中心を揃えて重ねる。
pub fn compose(
    images: [Option<(PathBuf, VwImage)>; 3],
    windows: [Option<(f32, f32)>; 3],
) -> Result<(VwImage, Composite), String> {
    let reference = images
        .iter()
        .flatten()
        .next()
        .ok_or("no image to compose")?;
    let (w, h) = (reference.1.width, reference.1.height);
    for (path, img) in images.iter().flatten() {
        if !matches!(img.color, ColorModel::Gray) {
            return Err(format!(
                "{} is not a gray image; only gray images can be composed",
                name(path)
            ));
        }
    }
    let n = w as usize * h as usize;
    let mut data = vec![f32::NAN; n * 3];
    let mut channels: [Option<Channel>; 3] = Default::default();
    let mut channel_windows = [(0.0, 1.0); 3];
    for (c, entry) in images.iter().enumerate() {
        let Some((path, img)) = entry else {
            continue;
        };
        let (placement, aligned) = if std::ptr::eq(img, &reference.1) {
            (Placement::SAME, Aligned::Reference)
        } else {
            place(&reference.1, img)
        };
        let values = samples(img);
        for row in 0..h as usize {
            for col in 0..w as usize {
                let [sx, sy] = placement.at(col as f64, row as f64);
                data[(row * w as usize + col) * 3 + c] =
                    bilinear(&values, img.width, img.height, sx, sy);
            }
        }
        let file_window = vw_core::display::window_of(img, &Default::default());
        let window = windows[c].unwrap_or(file_window);
        channel_windows[c] = window;
        channels[c] = Some(Channel {
            path: path.clone(),
            filter: filter_of(&img.metadata),
            calibration: img
                .metadata
                .get(vw_fits::calibration::GROUP, vw_fits::calibration::STATE)
                .map(str::to_owned),
            levels: Applied {
                kind: Kind::Fits,
                palette: Some(ramp(c)),
                pick: Pick::Auto,
                window,
                file_window,
                range: vw_core::display::gray_range(img),
                curve: img.curve,
                file_curve: img.curve,
                scale: Scale::of(&img.metadata),
                black_white: Default::default(),
            },
            size: (img.width, img.height),
            placement,
            aligned,
        });
    }
    let mut out = VwImage::new(w, h, ColorModel::Rgb, PixelData::F32(data))?;
    out.channel_windows = Some(channel_windows);
    out.curve = reference.1.curve;
    out.metadata = metadata(&channels, &reference.1.metadata);
    Ok((
        out,
        Composite {
            channels,
            size: (w, h),
        },
    ))
}

/// `values`（`w` × `h`）の (x, y) の値を、まわりの 4 画素から線形に補う。画像の外は `NaN`。
fn bilinear(values: &[f32], w: u32, h: u32, x: f64, y: f64) -> f32 {
    let (wf, hf) = (f64::from(w), f64::from(h));
    if !(x >= -0.5 && x <= wf - 0.5 && y >= -0.5 && y <= hf - 0.5) {
        return f32::NAN;
    }
    let (x, y) = (x.clamp(0.0, wf - 1.0), y.clamp(0.0, hf - 1.0));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w as usize - 1), (y0 + 1).min(h as usize - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let at = |x: usize, y: usize| values[y * w as usize + x];
    // 画素にちょうど重なるときは隣を混ぜない（隣の NaN を 0 倍しても NaN になるため）。
    let lerp = |a: f32, b: f32, t: f32| if t == 0.0 { a } else { a + (b - a) * t };
    lerp(
        lerp(at(x0, y0), at(x1, y0), fx),
        lerp(at(x0, y1), at(x1, y1), fx),
        fy,
    )
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// 画素の値を実数で。
fn samples(img: &VwImage) -> Vec<f32> {
    match &img.data {
        PixelData::U8(v) => v.iter().map(|&x| f32::from(x)).collect(),
        PixelData::U16(v) => v.iter().map(|&x| f32::from(x)).collect(),
        PixelData::F32(v) => v.clone(),
    }
}

/// `other` を `reference` に重ねる動かし方。
fn place(reference: &VwImage, other: &VwImage) -> (Placement, Aligned) {
    match wcs_placement(reference, other) {
        Some(p) => (p, Aligned::Wcs),
        None if (reference.width, reference.height) == (other.width, other.height) => {
            (Placement::SAME, Aligned::Unaligned)
        }
        None => (
            Placement::shifted(
                (f64::from(other.width) - f64::from(reference.width)) / 2.0,
                (f64::from(other.height) - f64::from(reference.height)) / 2.0,
            ),
            Aligned::Centered,
        ),
    }
}

/// WCS で決めた動かし方。FITS の画素（1 始まり、y は下から上）と画面の画素（0 始まり、
/// 行は上から下。行を返して出した画像は逆）の向きの違いも、ここで繋ぐ。
fn wcs_placement(reference: &VwImage, other: &VwImage) -> Option<Placement> {
    let m = vw_fits::pixel_map(&lookup(&reference.metadata), &lookup(&other.metadata))?;
    // 画面の行 → FITS の y：y = s 行 + t。
    let rows = |img: &VwImage| {
        if flipped(&img.metadata) {
            (-1.0, f64::from(img.height))
        } else {
            (1.0, 1.0)
        }
    };
    let (rs, rt) = rows(reference);
    let (os, ot) = rows(other);
    // 基準の画面 (列, 行) → 基準の FITS (列 + 1, rs 行 + rt) → 相手の FITS (x', y')
    // → 相手の画面 (x' - 1, (y' - ot) / os)。
    let fits = |r: [f64; 3]| [r[0], r[1] * rs, r[0] + r[1] * rt + r[2]];
    let (fx, fy) = (fits(m[0]), fits(m[1]));
    Some(Placement([
        [fx[0], fx[1], fx[2] - 1.0],
        [fy[0] / os, fy[1] / os, (fy[2] - ot) / os],
    ]))
}

fn flipped(m: &Metadata) -> bool {
    m.get(vw_fits::GROUP, vw_fits::ROWS) == Some(vw_fits::ROWS_FLIPPED)
}

/// FITS のカードを引く。画像の HDU のもの（後ろの見出し）を先に見る。
fn lookup(m: &Metadata) -> impl Fn(&str) -> Option<String> + '_ {
    move |key| {
        m.entries
            .iter()
            .rev()
            .find(|(g, k, _)| g.starts_with(vw_fits::GROUP) && k == key)
            .map(|(_, _, v)| v.trim().trim_matches('\'').trim().to_owned())
    }
}

fn metadata(channels: &[Option<Channel>; 3], reference: &Metadata) -> Metadata {
    let mut m = Metadata::default();
    for (c, ch) in channels.iter().enumerate() {
        let Some(ch) = ch else {
            continue;
        };
        let value = match &ch.filter {
            Some(f) => format!("{} ({f})", name(&ch.path)),
            None => name(&ch.path),
        };
        m.push(GROUP, CHANNELS[c], value);
    }
    m.entries.extend(reference.entries.iter().cloned());
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn composite_paths_round_trip_to_the_three_files() {
        let files = [Some(p("/sky/s2.fits")), None, Some(p("/sky/o3.fits"))];
        let path = path(&files).unwrap();
        assert_eq!(split(&path), Some(files));
        assert_eq!(path.parent(), Some(Path::new("/sky")));
        assert_eq!(split(Path::new("/sky/a.fits")), None);
        assert_eq!(title(&path).as_deref(), Some("R s2 + B o3"));
    }

    #[test]
    fn narrowband_filters_follow_the_hubble_palette() {
        let files = [
            (p("ha.fits"), Some("H-alpha".to_owned())),
            (p("o3.fits"), Some("OIII".to_owned())),
            (p("s2.fits"), Some("SII".to_owned())),
        ];
        assert_eq!(
            assign(&files),
            [Some(p("s2.fits")), Some(p("ha.fits")), Some(p("o3.fits"))]
        );
    }

    #[test]
    fn unknown_filters_fill_the_free_colors_in_order() {
        let files = [
            (p("a.fits"), None),
            (p("b.fits"), Some("Blue".to_owned())),
            (p("c.fits"), Some("Lum".to_owned())),
        ];
        assert_eq!(
            assign(&files),
            [Some(p("a.fits")), Some(p("c.fits")), Some(p("b.fits"))]
        );
    }

    fn gray(values: Vec<f32>, w: u32, h: u32, cards: &[(&str, &str)]) -> VwImage {
        let mut img = VwImage::new(w, h, ColorModel::Gray, PixelData::F32(values)).unwrap();
        img.window = Some((0.0, 10.0));
        for (k, v) in cards {
            img.metadata.push(vw_fits::GROUP, *k, (*v).to_owned());
        }
        img
    }

    const WCS: [(&str, &str); 10] = [
        ("NAXIS1", "4"),
        ("NAXIS2", "3"),
        ("CTYPE1", "RA---TAN"),
        ("CTYPE2", "DEC--TAN"),
        ("CRPIX1", "2.5"),
        ("CRPIX2", "2.0"),
        ("CRVAL1", "83.8"),
        ("CRVAL2", "-5.4"),
        ("CDELT1", "-0.01"),
        ("CDELT2", "0.01"),
    ];

    #[test]
    fn each_color_keeps_its_file_values_and_its_own_black_and_white() {
        let r = gray(vec![1.0; 12], 4, 3, &[]);
        let mut b = gray(vec![5.0; 12], 4, 3, &[]);
        b.window = Some((0.0, 5.0));
        let (img, comp) = compose(
            [Some((p("r"), r)), None, Some((p("b"), b))],
            [Some((0.0, 2.0)), None, None],
        )
        .unwrap();
        let PixelData::F32(v) = &img.data else {
            panic!()
        };
        assert_eq!(v[0], 1.0);
        assert!(v[1].is_nan());
        assert_eq!(v[2], 5.0);
        assert_eq!(img.channel_windows.unwrap()[0], (0.0, 2.0));
        assert_eq!(img.channel_windows.unwrap()[2], (0.0, 5.0));
        let d = vw_core::display::to_display(&img, &Default::default());
        assert_eq!(&d.rgba[..4], &[128, 0, 255, 255]);
        assert!(comp.channels[1].is_none());
        assert_eq!(
            comp.channels[0].as_ref().unwrap().aligned,
            Aligned::Reference
        );
        assert_eq!(
            comp.channels[2].as_ref().unwrap().aligned,
            Aligned::Unaligned
        );
    }

    #[test]
    fn images_with_wcs_are_moved_onto_the_reference() {
        let values: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let r = gray(values.clone(), 4, 3, &WCS);
        let mut moved = WCS;
        // 同じ星が 1 画素右に写っている。
        moved[4] = ("CRPIX1", "3.5");
        let g = gray(values, 4, 3, &moved);
        let (img, comp) = compose([Some((p("r"), r)), Some((p("g"), g)), None], [None; 3]).unwrap();
        let PixelData::F32(v) = &img.data else {
            panic!()
        };
        let g = comp.channels[1].as_ref().unwrap();
        assert_eq!(g.aligned, Aligned::Wcs);
        let [dx, dy] = g.placement.shift((4, 3), (4, 3));
        assert!((dx - 1.0).abs() < 1e-6 && dy.abs() < 1e-6, "{dx} {dy}");
        // 基準の (0, 0) には、緑の (1, 0) が重なる。右端は重なるものがない。
        assert_eq!((v[0], v[1]), (0.0, 1.0));
        assert!(v[3 * 3 + 1].is_nan());
    }

    #[test]
    fn images_of_another_size_without_wcs_are_centered() {
        let a = gray(vec![0.0; 12], 4, 3, &[]);
        let values: Vec<f32> = (0..30).map(|i| i as f32).collect();
        let b = gray(values, 6, 5, &[]);
        let (img, comp) = compose([Some((p("a"), a)), Some((p("b"), b)), None], [None; 3]).unwrap();
        assert_eq!((img.width, img.height), (4, 3));
        let PixelData::F32(v) = &img.data else {
            panic!()
        };
        // 基準の (0, 0) には、6 x 5 の画像の (1, 1) が重なる。
        assert_eq!(v[1], 7.0);
        assert_eq!(
            comp.channels[1].as_ref().unwrap().aligned,
            Aligned::Centered
        );
    }

    #[test]
    fn images_with_wcs_are_turned_and_scaled_onto_the_reference() {
        let values: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let r = gray(values, 4, 3, &WCS);
        // 90 度回して、画素を半分の大きさにした 8 x 6 の画像。
        let turned = [
            ("NAXIS1", "6"),
            ("NAXIS2", "8"),
            ("CTYPE1", "RA---TAN"),
            ("CTYPE2", "DEC--TAN"),
            ("CRPIX1", "3.5"),
            ("CRPIX2", "4.5"),
            ("CRVAL1", "83.8"),
            ("CRVAL2", "-5.4"),
            ("CDELT1", "-0.005"),
            ("CDELT2", "0.005"),
            ("CROTA2", "90.0"),
        ];
        let g = gray(vec![1.0; 48], 6, 8, &turned);
        let (_, comp) = compose([Some((p("r"), r)), Some((p("g"), g)), None], [None; 3]).unwrap();
        let g = comp.channels[1].as_ref().unwrap();
        assert_eq!(g.aligned, Aligned::Wcs);
        assert!(
            (g.placement.scale() - 2.0).abs() < 1e-3,
            "{:?}",
            g.placement
        );
        assert!(
            (g.placement.rotation().abs() - 90.0).abs() < 0.1,
            "{:?}",
            g.placement
        );
    }

    #[test]
    fn rotation_is_clockwise_on_the_screen() {
        // 基準の右向きが、この画像では下向き（画面で時計回りに 90 度）。
        let p = Placement([[0.0, -1.0, 0.0], [1.0, 0.0, 0.0]]);
        assert!((p.rotation() - 90.0).abs() < 1e-9);
        assert!((p.scale() - 1.0).abs() < 1e-9);
    }
}
