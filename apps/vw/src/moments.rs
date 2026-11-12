//! FITS のキューブから作ったモーメント図を指すパス（[`crate::compose`] と同じ理由で、パスで表す）。
//!
//! 図はメモリの上だけにあり、元のファイルは変えない。パスには作り方を全部書くので、読み込みの
//! スレッドはパスだけから作り直せる。作った FITS のバイト列はいくつか覚えておき、縮小画像と元の
//! 大きさの画像を読むたびに作り直さないようにする。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use vw_core::{PixelData, VwImage};
use vw_fits::moments::{Options, Order};

/// 区切り。`?` は Windows のファイル名に使えないので、実在のファイルと取り違えない。
const MARK: &str = "?moment=";

/// 1 次のモーメント図に初めから当てる色の表。0 をはさんで青と赤に分かれる。
pub const VELOCITY_PALETTE: &str = "coolwarm";

/// モーメント図の作り方と、元のキューブ。
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub file: PathBuf,
    pub options: Options,
}

/// `spec` の図を指すパス。`<キューブ>?moment=<次数>,<最初>-<最後>,<しきい値>,<面>,<HDU>`。
/// チャンネルは 0 から、全部なら `all`、HDU は選んでいなければ `-`。
pub fn path(spec: &Spec) -> PathBuf {
    let o = &spec.options;
    let channels = o
        .channels
        .map_or_else(|| "all".to_owned(), |(a, b)| format!("{a}-{b}"));
    let hdu = o.hdu.map_or_else(|| "-".to_owned(), |h| h.to_string());
    let mut s = spec.file.as_os_str().to_owned();
    s.push(format!(
        "{MARK}{},{channels},{},{},{hdu}",
        o.order.number(),
        o.clip,
        o.frame
    ));
    PathBuf::from(s)
}

/// モーメント図を指すパスなら、作り方に分ける。
pub fn split(path: &Path) -> Option<Spec> {
    let s = path.to_str()?;
    let (file, rest) = s.rsplit_once(MARK)?;
    let parts: Vec<&str> = rest.split(',').collect();
    let [order, channels, clip, frame, hdu] = parts.as_slice() else {
        return None;
    };
    let order = Order::from_number(order.parse().ok()?)?;
    let channels = match *channels {
        "all" => None,
        range => {
            let (a, b) = range.split_once('-')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        }
    };
    let hdu = match *hdu {
        "-" => None,
        h => Some(h.parse().ok()?),
    };
    Some(Spec {
        file: PathBuf::from(file),
        options: Options {
            order,
            channels,
            clip: clip.parse().ok()?,
            frame: frame.parse().ok()?,
            hdu,
        },
    })
}

/// 画面に出す名前（「disk-cube · moment 1」）。モーメント図でなければ `None`。
/// 言葉を訳さないのは、スロットの見出しなど言語を知らない所でも同じ名前で出すため。
pub fn title(path: &Path) -> Option<String> {
    let spec = split(path)?;
    let stem = spec
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(format!("{stem} · moment {}", spec.options.order.number()))
}

/// 覚えておく図の数。
const KEPT: usize = 6;

type Made = (PathBuf, Arc<Vec<u8>>);

static MADE: Mutex<VecDeque<Made>> = Mutex::new(VecDeque::new());

/// `path` の図の FITS のバイト列。覚えていなければキューブを読んで作る。
pub fn bytes(path: &Path) -> Result<Arc<Vec<u8>>, String> {
    let spec = split(path).ok_or_else(|| format!("{}: not a moment map", path.display()))?;
    if let Some((_, b)) = made().iter().find(|(p, _)| p == path) {
        return Ok(b.clone());
    }
    let data = std::fs::read(&spec.file).map_err(|e| format!("{}: {e}", spec.file.display()))?;
    let bytes = Arc::new(
        vw_fits::moments::make(&data, &spec.options)
            .map_err(|e| format!("{}: {e}", spec.file.display()))?,
    );
    let mut list = made();
    if list.len() >= KEPT {
        list.pop_front();
    }
    list.push_back((path.to_path_buf(), bytes.clone()));
    Ok(bytes)
}

/// `path` の図を読む。画像と、プラグインの名前と、FITS のバイト数。1 次の図には速度の色の表を
/// 勧め、黒と白を中央の速度で対称にする。
pub fn decode(
    registry: &vw_core::Registry,
    path: &Path,
) -> Result<(VwImage, String, usize), String> {
    let spec = split(path).ok_or_else(|| format!("{}: not a moment map", path.display()))?;
    let bytes = bytes(path)?;
    // 場所を渡さずに読む: 渡すとキューブのフォルダのダークやフラットを探して当てようとするため。
    let plugin = registry
        .find(Path::new("moment.fits"), &bytes)
        .ok_or_else(|| "no FITS reader".to_owned())?;
    let mut image = plugin.decode(&bytes).map_err(|e| e.to_string())?;
    if spec.options.order == Order::One {
        image.metadata.push(
            vw_fits::GROUP,
            crate::colormap::SUGGESTED_PALETTE,
            VELOCITY_PALETTE,
        );
        if let PixelData::F32(v) = &image.data {
            image.window = velocity_window(v).or(image.window);
        }
    }
    Ok((image, plugin.name().to_owned(), bytes.len()))
}

fn made() -> std::sync::MutexGuard<'static, VecDeque<Made>> {
    MADE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 1 次の図（速度）の黒と白。中央の速度をはさんで対称にし、外れた 1% の画素は端に寄せる。
pub fn velocity_window(values: &[f32]) -> Option<(f32, f32)> {
    let mut v: Vec<f32> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let median = v[v.len() / 2];
    let mut d: Vec<f32> = v.iter().map(|x| (x - median).abs()).collect();
    d.sort_by(f32::total_cmp);
    let reach = d[(d.len() - 1) * 99 / 100];
    let reach = if reach > 0.0 { reach } else { 1.0 };
    Some((median - reach, median + reach))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(channels: Option<(usize, usize)>, hdu: Option<usize>) -> Spec {
        Spec {
            file: PathBuf::from("/data/radio/disk cube.fits"),
            options: Options {
                order: Order::Two,
                channels,
                clip: 2.5,
                frame: 3,
                hdu,
            },
        }
    }

    #[test]
    fn a_moment_path_gives_back_how_the_map_was_made() {
        for s in [spec(None, None), spec(Some((4, 20)), Some(1))] {
            assert_eq!(split(&path(&s)), Some(s));
        }
        assert!(split(Path::new("/data/radio/disk cube.fits")).is_none());
    }

    #[test]
    fn a_moment_map_stays_in_the_folder_of_its_cube() {
        let p = path(&spec(None, None));
        assert_eq!(p.parent(), Some(Path::new("/data/radio")));
        assert_eq!(title(&p).as_deref(), Some("disk cube · moment 2"));
    }

    #[test]
    fn the_velocity_map_is_shown_symmetric_around_its_middle_velocity() {
        let mut v: Vec<f32> = (0..=200).map(|i| i as f32 - 100.0 + 30.0).collect();
        v.push(f32::NAN);
        let (lo, hi) = velocity_window(&v).unwrap();
        assert!(((lo + hi) / 2.0 - 30.0).abs() < 1e-3);
        assert!((hi - 30.0 - 99.0).abs() < 1.01, "{lo} {hi}");
    }
}
