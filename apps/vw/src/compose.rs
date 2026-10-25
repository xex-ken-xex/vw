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
    /// 基準（最初の色）の画像に重ねるために動かした画素数（右、下）。
    pub shift: (i64, i64),
    pub aligned: Aligned,
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
}

impl Aligned {
    pub fn title(self) -> &'static str {
        match self {
            Aligned::Reference => "Reference (not moved)",
            Aligned::Wcs => "Moved by the sky coordinates (WCS)",
            Aligned::Unaligned => "Not moved (no WCS in both images)",
        }
    }
}

/// 合成の内訳。パイプラインの図とゲージに使う。
#[derive(Debug, Clone)]
pub struct Composite {
    pub channels: [Option<Channel>; 3],
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
/// スロットで色ごとに変えた黒と白。最初の画像を基準にして、ほかは WCS で位置を合わせる。
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
        if (img.width, img.height) != (w, h) {
            return Err(format!(
                "{} is {} x {}, not {w} x {h}; images of different sizes cannot be composed yet",
                name(path),
                img.width,
                img.height
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
        let (shift, aligned) = if std::ptr::eq(img, &reference.1) {
            ((0, 0), Aligned::Reference)
        } else {
            match offset(&reference.1, img) {
                Some(s) => (s, Aligned::Wcs),
                None => ((0, 0), Aligned::Unaligned),
            }
        };
        let values = samples(img);
        for y in 0..h as i64 {
            let sy = y + shift.1;
            if sy < 0 || sy >= h as i64 {
                continue;
            }
            for x in 0..w as i64 {
                let sx = x + shift.0;
                if sx < 0 || sx >= w as i64 {
                    continue;
                }
                let at = (y * w as i64 + x) as usize;
                data[at * 3 + c] = values[(sy * w as i64 + sx) as usize];
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
                scale: Scale::of(&img.metadata),
            },
            shift,
            aligned,
        });
    }
    let mut out =
        VwImage::new(w, h, ColorModel::Rgb, PixelData::F32(data)).map_err(|e| e.to_string())?;
    out.channel_windows = Some(channel_windows);
    out.curve = reference.1.curve;
    out.metadata = metadata(&channels, &reference.1.metadata);
    Ok((out, Composite { channels }))
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

/// `other` を `reference` に重ねるとき、`other` の何画素先を取ればよいか（画面の右と下）。
fn offset(reference: &VwImage, other: &VwImage) -> Option<(i64, i64)> {
    let d = vw_fits::pixel_offset(&lookup(&reference.metadata), &lookup(&other.metadata))?;
    // FITS の y は下から上へ数える。行を返さずに出した画像では画面の下へ向かう向きと同じ、
    // 返した画像では逆になる。
    let down = |m: &Metadata| {
        if flipped(m) {
            -1.0
        } else {
            1.0
        }
    };
    if flipped(&reference.metadata) != flipped(&other.metadata) {
        // 行の向きが違う 2 枚は、ずれだけでは重ならない。
        return None;
    }
    Some((
        d[0].round() as i64,
        (d[1] * down(&other.metadata)).round() as i64,
    ))
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
        assert_eq!((g.shift, g.aligned), ((1, 0), Aligned::Wcs));
        // 基準の (0, 0) には、緑の (1, 0) が重なる。右端は重なるものがない。
        assert_eq!((v[0], v[1]), (0.0, 1.0));
        assert!(v[3 * 3 + 1].is_nan());
    }

    #[test]
    fn images_of_different_sizes_are_refused() {
        let a = gray(vec![0.0; 12], 4, 3, &[]);
        let b = gray(vec![0.0; 6], 3, 2, &[]);
        let err = compose([Some((p("a"), a)), Some((p("b"), b)), None], [None; 3]).unwrap_err();
        assert!(err.contains("different sizes"), "{err}");
    }
}
