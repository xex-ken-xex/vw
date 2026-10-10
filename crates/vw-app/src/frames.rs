//! マルチフレームのファイル（DICOM のシネなど）の 1 フレームを、パスとして指す。
//!
//! スロット・ラベル・スライドショーは画像をパスで扱うので、フレームもパスで表す。
//! 「ファイル + フレーム番号」の型を作ってアプリ全体に通す方式は、変更が読み込み・ラベル・
//! 監視・書き出しのすべてに及ぶので採らない。
//! 区切りに `?` を使うのは、Windows ではファイル名に使えない文字で、実在のファイルと
//! 取り違えないため（Linux では使えるが、`?frame=` で終わる名前はまず見かけない）。

use std::path::{Path, PathBuf};

const MARK: &str = "?frame=";

/// `file` の `frame` 番目（0 始まり）のフレームを指すパス。
pub fn frame_path(file: &Path, frame: usize) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(format!("{MARK}{frame}"));
    PathBuf::from(s)
}

/// `file` の全フレームのパス。
pub fn frame_paths(file: &Path, count: usize) -> Vec<PathBuf> {
    (0..count).map(|i| frame_path(file, i)).collect()
}

/// フレームを指すパスなら、元のファイルとフレーム番号に分ける。
pub fn split(path: &Path) -> Option<(PathBuf, usize)> {
    let s = path.to_str()?;
    let (file, frame) = s.rsplit_once(MARK)?;
    let frame = frame.parse().ok()?;
    Some((PathBuf::from(file), frame))
}

/// 実際に読むファイル。フレームを指すパスならその元のファイル。
pub fn file_of(path: &Path) -> PathBuf {
    split(path).map_or_else(|| path.to_path_buf(), |(file, _)| file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_paths_round_trip_to_file_and_index() {
        let file = Path::new("/data/cine.dcm");
        let p = frame_path(file, 12);
        assert_eq!(split(&p), Some((file.to_path_buf(), 12)));
        assert_eq!(file_of(&p), file);
        assert_eq!(p.parent(), file.parent());
    }

    #[test]
    fn ordinary_paths_are_not_frames() {
        assert_eq!(split(Path::new("/data/a.png")), None);
        assert_eq!(split(Path::new("/data/odd?frame=x")), None);
        assert_eq!(file_of(Path::new("/data/a.png")), Path::new("/data/a.png"));
    }
}
