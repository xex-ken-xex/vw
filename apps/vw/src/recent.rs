//! 最近開いたフォルダの一覧。

use std::path::{Path, PathBuf};

/// 覚えておく数。
pub const KEEP: usize = 5;

/// `path` を一覧の先頭に置く。前からあれば先頭へ移し、古いものは [`KEEP`] を超えた分だけ捨てる。
pub fn remember(list: &mut Vec<PathBuf>, path: &Path) {
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(KEEP);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[PathBuf]) -> Vec<&str> {
        list.iter().map(|p| p.to_str().unwrap()).collect()
    }

    #[test]
    fn newest_folder_comes_first() {
        let mut list = Vec::new();
        remember(&mut list, Path::new("/a"));
        remember(&mut list, Path::new("/b"));
        assert_eq!(paths(&list), ["/b", "/a"]);
    }

    #[test]
    fn reopened_folder_moves_to_the_front_without_a_duplicate() {
        let mut list = vec![PathBuf::from("/b"), PathBuf::from("/a")];
        remember(&mut list, Path::new("/a"));
        assert_eq!(paths(&list), ["/a", "/b"]);
    }

    #[test]
    fn keeps_only_the_five_newest() {
        let mut list = Vec::new();
        for name in ["/1", "/2", "/3", "/4", "/5", "/6"] {
            remember(&mut list, Path::new(name));
        }
        assert_eq!(paths(&list), ["/6", "/5", "/4", "/3", "/2"]);
    }
}
