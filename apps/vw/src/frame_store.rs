//! マルチフレームのファイル（DICOM のシネなど）のフレームを、一度の読み込みと解析で
//! 取り出して持つ。
//!
//! フレームはパス `file?frame=N` ごとの別の読み込みなので、そのままでは 1 枚ごとにファイル全体を
//! 読み直して解析し直し、フレームの多いファイルほど 1 枚が遅くなる。最初に頼まれたフレームから
//! 裏で順に取り出し、取り出せたものからすぐ渡す。持つ量はフル解像度の上限までで、超えたら
//! 取り出しをやめ、残りのフレームは今までどおり 1 枚ずつ読む。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use vw_core::{Registry, VwImage};

/// アプリ全体で 1 つの置き場。読み込みの作業はスレッドごとに別なので、どこからでも引ける所に置く。
pub static STORE: LazyLock<FrameStore> = LazyLock::new(FrameStore::default);

/// 置き場から渡したフレーム。
pub struct Frame {
    pub image: VwImage,
    pub plugin: String,
    pub file_size: usize,
    pub read: Duration,
    pub decode: Duration,
}

#[derive(Default)]
pub struct FrameStore {
    /// 古いものが先。
    entries: Mutex<Vec<Arc<Entry>>>,
    budget: AtomicUsize,
    held: AtomicUsize,
    /// 上限に当たって、取り出しを途中でやめた。
    limited: AtomicBool,
}

/// ファイルが変わったかを見分ける印。
#[derive(Clone, Copy, PartialEq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn stamp(file: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(file).ok()?;
    Some(Stamp {
        len: m.len(),
        modified: m.modified().ok(),
    })
}

struct Entry {
    file: PathBuf,
    stamp: Stamp,
    state: Mutex<State>,
    ready: Condvar,
}

#[derive(Default)]
struct State {
    images: HashMap<usize, Arc<VwImage>>,
    errors: HashMap<usize, String>,
    plugin: String,
    file_size: usize,
    read: Duration,
    decode: Duration,
    bytes: usize,
    /// 取り出しを終えた（やめた・追い出された・失敗した場合も）。
    finished: bool,
    evicted: bool,
}

impl Entry {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn finish(&self) {
        self.state().finished = true;
        self.ready.notify_all();
    }
}

fn image_bytes(img: &VwImage) -> usize {
    img.data.len() * (img.data.bits_per_sample() as usize).div_ceil(8)
}

impl FrameStore {
    /// 持ってよい量（バイト）。
    pub fn set_budget(&self, bytes: usize) {
        self.budget.store(bytes, Ordering::Relaxed);
    }

    /// 上限に当たって持ちきれなかったことがあれば、一度だけ `true`。
    pub fn take_limited(&self) -> bool {
        self.limited.swap(false, Ordering::Relaxed)
    }

    fn entries(&self) -> MutexGuard<'_, Vec<Arc<Entry>>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `file` の `frame` 番目。置き場で扱えない（1 枚だけのファイル、上限で持ちきれなかった
    /// フレーム）ときは `None` で、呼ぶ側が今までどおり読む。
    pub fn frame(
        &'static self,
        registry: &Registry,
        file: &Path,
        frame: usize,
    ) -> Option<Result<Frame, String>> {
        let now = stamp(file)?;
        let entry = {
            let mut entries = self.entries();
            if let Some(i) = entries.iter().position(|e| e.file == file) {
                let e = entries.remove(i);
                if e.stamp == now {
                    entries.push(e.clone());
                    Some(e)
                } else {
                    self.drop_entry(&e);
                    None
                }
            } else {
                None
            }
        };
        let entry = match entry {
            Some(e) => e,
            None => self.start(registry, file, now, frame)?,
        };
        let mut state = entry.state();
        loop {
            if let Some(image) = state.images.get(&frame) {
                return Some(Ok(Frame {
                    image: VwImage::clone(image),
                    plugin: state.plugin.clone(),
                    file_size: state.file_size,
                    read: state.read,
                    decode: state.decode,
                }));
            }
            if let Some(e) = state.errors.get(&frame) {
                return Some(Err(e.clone()));
            }
            if state.finished {
                return None;
            }
            state = entry.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// ファイルを読み、フレームが 2 枚以上なら裏で取り出し始める。
    fn start(
        &'static self,
        registry: &Registry,
        file: &Path,
        stamp: Stamp,
        first: usize,
    ) -> Option<Arc<Entry>> {
        let t0 = Instant::now();
        let bytes = std::fs::read(file).ok()?;
        let read = t0.elapsed();
        let count = registry.frame_count(file, &bytes).ok()?;
        if count < 2 || first >= count {
            return None;
        }
        let entry = Arc::new(Entry {
            file: file.to_path_buf(),
            stamp,
            state: Mutex::new(State {
                file_size: bytes.len(),
                read,
                ..State::default()
            }),
            ready: Condvar::new(),
        });
        {
            let mut entries = self.entries();
            // 同時に頼まれて、別の作業が先に始めていればそちらを使う。
            if let Some(e) = entries.iter().find(|e| e.file == file && e.stamp == stamp) {
                return Some(e.clone());
            }
            entries.push(entry.clone());
        }
        let order: Vec<usize> = (first..count).chain(0..first).collect();
        let registry = registry.clone();
        let producing = entry.clone();
        std::thread::spawn(move || {
            self.produce(&registry, &producing, &bytes, &order);
            producing.finish();
        });
        Some(entry)
    }

    fn produce(&self, registry: &Registry, entry: &Arc<Entry>, bytes: &[u8], order: &[usize]) {
        let mut last = Instant::now();
        let result = registry.decode_frames_bytes(&entry.file, bytes, order, &mut |i, image| {
            let decode = last.elapsed();
            last = Instant::now();
            if entry.state().evicted {
                return false;
            }
            let image = match image {
                Ok(image) => image,
                Err(e) => {
                    entry.state().errors.insert(i, e.to_string());
                    entry.ready.notify_all();
                    return true;
                }
            };
            let size = image_bytes(&image);
            if !self.make_room(size, entry) {
                self.limited.store(true, Ordering::Relaxed);
                return false;
            }
            let mut state = entry.state();
            if state.evicted {
                self.held.fetch_sub(size, Ordering::Relaxed);
                return false;
            }
            state.images.insert(i, Arc::new(image));
            state.bytes += size;
            state.decode = decode;
            drop(state);
            entry.ready.notify_all();
            true
        });
        match result {
            Ok(plugin) => {
                let mut state = entry.state();
                state.plugin = plugin.to_owned();
            }
            Err(e) => {
                let mut state = entry.state();
                for &i in order {
                    state.errors.entry(i).or_insert_with(|| e.to_string());
                }
            }
        }
    }

    /// `size` バイトを足せるよう、ほかのファイルのフレームを古いものから捨てる。足せないなら `false`。
    fn make_room(&self, size: usize, keep: &Arc<Entry>) -> bool {
        let budget = self.budget.load(Ordering::Relaxed);
        let mut entries = self.entries();
        while self.held.load(Ordering::Relaxed) + size > budget {
            let Some(i) = entries.iter().position(|e| !Arc::ptr_eq(e, keep)) else {
                return false;
            };
            let e = entries.remove(i);
            self.drop_entry(&e);
        }
        self.held.fetch_add(size, Ordering::Relaxed);
        true
    }

    /// 置き場から外したフレームを捨てる。取り出し中なら止める。
    fn drop_entry(&self, e: &Entry) {
        let mut state = e.state();
        self.held.fetch_sub(state.bytes, Ordering::Relaxed);
        state.bytes = 0;
        state.images.clear();
        state.evicted = true;
        state.finished = true;
        drop(state);
        e.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_core::{ColorModel, DecodeError, DecoderPlugin, PixelData};

    /// 先頭のバイトがフレーム数で、`n` 番目のフレームが画素値 `n` の 1x1 のグレー。
    /// 解析（decode_frames と decode_frame）の回数を数える。
    #[derive(Default)]
    struct Frames {
        parses: AtomicUsize,
    }

    fn gray(v: u8) -> VwImage {
        VwImage::new(1, 1, ColorModel::Gray, PixelData::U8(vec![v])).unwrap()
    }

    impl DecoderPlugin for Frames {
        fn name(&self) -> &str {
            "Frames"
        }
        fn extensions(&self) -> &[&str] {
            &["frames"]
        }
        fn probe(&self, _header: &[u8]) -> bool {
            false
        }
        fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
            self.decode_frame(data, 0)
        }
        fn frame_count(&self, data: &[u8]) -> Result<usize, DecodeError> {
            Ok(data[0] as usize)
        }
        fn decode_frame(&self, _data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
            self.parses.fetch_add(1, Ordering::Relaxed);
            Ok(gray(frame as u8))
        }
        fn decode_frames(
            &self,
            _path: &Path,
            data: &[u8],
            order: &[usize],
            each: &mut dyn FnMut(usize, Result<VwImage, DecodeError>) -> bool,
        ) {
            self.parses.fetch_add(1, Ordering::Relaxed);
            for &i in order {
                let image = if i < data[0] as usize {
                    Ok(gray(i as u8))
                } else {
                    Err(DecodeError::Invalid(format!("no frame {i}")))
                };
                if !each(i, image) {
                    return;
                }
            }
        }
    }

    fn setup(frames: u8, budget: usize) -> (&'static FrameStore, Registry, Arc<Frames>, PathBuf) {
        let store: &'static FrameStore = Box::leak(Box::default());
        store.set_budget(budget);
        let plugin = Arc::new(Frames::default());
        let mut registry = Registry::new();
        registry.register(plugin.clone());
        let dir = std::env::temp_dir().join(format!(
            "vw-frame-store-{}-{}",
            std::process::id(),
            frames as usize * 1000 + budget
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cine.frames");
        std::fs::write(&file, [frames]).unwrap();
        (store, registry, plugin, file)
    }

    fn value(frame: Frame) -> u8 {
        match frame.image.data {
            PixelData::U8(v) => v[0],
            _ => panic!("8-bit frames"),
        }
    }

    #[test]
    fn every_frame_of_a_multiframe_file_comes_from_one_parse() {
        let (store, registry, plugin, file) = setup(5, 1 << 20);
        for i in [2, 3, 4, 0, 1] {
            let frame = store.frame(&registry, &file, i).unwrap().unwrap();
            assert_eq!(value(frame), i as u8);
        }
        assert_eq!(plugin.parses.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_changed_file_is_read_again() {
        let (store, registry, plugin, file) = setup(3, 1 << 20);
        store.frame(&registry, &file, 1).unwrap().unwrap();
        std::fs::write(&file, [4, 0]).unwrap();
        let frame = store.frame(&registry, &file, 3).unwrap().unwrap();
        assert_eq!(value(frame), 3);
        assert_eq!(plugin.parses.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn frames_over_the_budget_are_left_to_the_caller_and_reported() {
        // 1 フレーム 1 バイトで、2 フレーム分だけ持てる。
        let (store, registry, _, file) = setup(4, 2);
        assert_eq!(value(store.frame(&registry, &file, 0).unwrap().unwrap()), 0);
        assert_eq!(value(store.frame(&registry, &file, 1).unwrap().unwrap()), 1);
        assert!(store.frame(&registry, &file, 3).is_none());
        assert!(store.take_limited());
        assert!(!store.take_limited());
    }

    #[test]
    fn a_single_frame_file_is_left_to_the_caller() {
        let (store, registry, _, file) = setup(1, 1 << 20);
        assert!(store.frame(&registry, &file, 0).is_none());
    }

    #[test]
    fn older_files_make_room_for_the_newest() {
        let (store, registry, _, a) = setup(2, 3);
        let b = a.with_file_name("other.frames");
        std::fs::write(&b, [2]).unwrap();
        store.frame(&registry, &a, 1).unwrap().unwrap();
        store.frame(&registry, &a, 0).unwrap().unwrap();
        store.frame(&registry, &b, 0).unwrap().unwrap();
        store.frame(&registry, &b, 1).unwrap().unwrap();
        assert!(store.held.load(Ordering::Relaxed) <= 3);
        assert!(!store.take_limited());
    }
}
