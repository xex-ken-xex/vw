//! 校正用の画像を探して、ヘッダを覚えておく。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::frame::{is_calibration_folder, Frame};

/// ライブラリのフォルダを下へたどる深さ。
const LIBRARY_DEPTH: usize = 6;
/// 近くの種類別のフォルダを下へたどる深さ（`calibration/darks/300s` のような分け方まで）。
const NEARBY_DEPTH: usize = 3;
/// ライトのフォルダから、種類別のフォルダを探しに上がる数（N.I.N.A. の 対象/日付/LIGHT）。
const NEARBY_UP: usize = 2;
/// 1 回に見るファイルの上限。ライブラリに写真を丸ごと入れられても止まらないように。
const MAX_FILES: usize = 5000;

/// フォルダの中身とファイルのヘッダを覚えておく。フォルダやファイルの更新時刻が変わったら
/// 読み直す（撮影中に足されたダークも使えるように）。
#[derive(Default)]
pub struct Index {
    dirs: HashMap<PathBuf, Listing>,
    files: HashMap<PathBuf, Cached>,
}

struct Listing {
    modified: Option<SystemTime>,
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
}

struct Cached {
    modified: Option<SystemTime>,
    len: u64,
    frame: Option<Frame>,
}

/// 探す場所。
pub struct Places<'a> {
    pub nearby: bool,
    pub library: &'a [PathBuf],
}

impl Index {
    /// `light` の近くとライブラリにある FITS の、ヘッダから読んだ情報。
    pub fn frames_near(&mut self, light: &Path, places: &Places) -> Vec<Frame> {
        let mut roots: Vec<(PathBuf, usize)> = Vec::new();
        if places.nearby
            && let Some(dir) = light.parent() {
                // ライトと同じフォルダと、その親に直に置いたマスターも見る。
                roots.push((dir.to_path_buf(), 0));
                if let Some(parent) = dir.parent() {
                    roots.push((parent.to_path_buf(), 0));
                }
                for up in dir.ancestors().take(NEARBY_UP + 1) {
                    for sub in self.listing(up).dirs.clone() {
                        let name = sub.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if is_calibration_folder(name) {
                            roots.push((sub, NEARBY_DEPTH));
                        }
                    }
                }
            }
        for lib in places.library {
            roots.push((lib.clone(), LIBRARY_DEPTH));
        }
        let mut files = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (root, depth) in roots {
            self.collect(&root, depth, &mut files, &mut seen);
        }
        files.retain(|f| f != light);
        files.into_iter().filter_map(|f| self.frame(&f)).collect()
    }

    fn collect(
        &mut self,
        dir: &Path,
        depth: usize,
        out: &mut Vec<PathBuf>,
        seen: &mut std::collections::HashSet<PathBuf>,
    ) {
        if !seen.insert(dir.to_path_buf()) {
            return;
        }
        let listing = self.listing(dir);
        let (files, dirs) = (listing.files.clone(), listing.dirs.clone());
        for f in files {
            if out.len() >= MAX_FILES {
                return;
            }
            if !out.contains(&f) {
                out.push(f);
            }
        }
        if depth > 0 {
            for d in dirs {
                self.collect(&d, depth - 1, out, seen);
            }
        }
    }

    fn listing(&mut self, dir: &Path) -> &Listing {
        let modified = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
        let fresh = self
            .dirs
            .get(dir)
            .is_some_and(|l| l.modified.is_some() && l.modified == modified);
        if !fresh {
            let (mut files, mut dirs) = (Vec::new(), Vec::new());
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                match entry.file_type() {
                    Ok(t) if t.is_dir() => dirs.push(path),
                    Ok(_) if is_fits_name(&path) => files.push(path),
                    _ => {}
                }
            }
            files.sort();
            dirs.sort();
            self.dirs.insert(
                dir.to_path_buf(),
                Listing {
                    modified,
                    files,
                    dirs,
                },
            );
        }
        &self.dirs[dir]
    }

    fn frame(&mut self, path: &Path) -> Option<Frame> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta.modified().ok();
        let len = meta.len();
        if let Some(c) = self.files.get(path)
            && c.modified.is_some() && c.modified == modified && c.len == len {
                return c.frame.clone();
            }
        let frame = crate::header_of_file(path)
            .ok()
            .flatten()
            .and_then(|h| Frame::read(path, &|k| h.get(k).map(str::to_owned)));
        self.files.insert(
            path.to_path_buf(),
            Cached {
                modified,
                len,
                frame: frame.clone(),
            },
        );
        frame
    }
}

/// FITS の拡張子か（`fits.gz` のような二重の拡張子も）。
fn is_fits_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    ["fits", "fit", "fts", "fz", "fits.gz", "fit.gz", "fts.gz"]
        .iter()
        .any(|ext| name.ends_with(&format!(".{ext}")))
}
