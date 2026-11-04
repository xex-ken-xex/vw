//! ファイルの中で見るもの（NetCDF の変数、FITS の HDU）の、ファイルごとの選択。起動している間だけ覚える。
//!
//! 鍵はファイルの中身から作る（[`file_key`]）。デコーダにはファイルの場所が届かないことがあり、
//! フレームの数（[`crate::DecoderPlugin::frame_count`]）も中身だけから答えるため。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

/// ファイルごとに選んだもの。
#[derive(Debug, Clone)]
pub struct Choices<T>(Arc<Mutex<HashMap<u64, T>>>);

impl<T> Default for Choices<T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<T: Clone> Choices<T> {
    pub fn get(&self, data: &[u8]) -> Option<T> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&file_key(data))
            .cloned()
    }

    pub fn set(&self, data: &[u8], choice: impl Into<T>) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(file_key(data), choice.into());
    }
}

/// ファイルの中身の鍵。先頭 64 KiB（見出しとヘッダ）と長さ。全体を読むと大きなファイルで遅いため。
pub fn file_key(data: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data[..data.len().min(64 << 10)].hash(&mut h);
    data.len().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_is_remembered_for_the_same_contents_only() {
        let choices = Choices::<usize>::default();
        choices.set(b"file a", 2usize);
        assert_eq!(choices.get(b"file a"), Some(2));
        assert_eq!(choices.get(b"file b"), None);
    }

    #[test]
    fn copies_share_what_was_chosen() {
        let choices = Choices::<String>::default();
        let copy = choices.clone();
        copy.set(b"data", "t2m");
        assert_eq!(choices.get(b"data").as_deref(), Some("t2m"));
    }
}
