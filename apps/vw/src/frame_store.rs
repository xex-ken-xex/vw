//! マルチフレームのファイル（DICOM のシネやトモシンセシスなど）のフレームを、読み直さずに持つ。
//!
//! フレームはパス `file?frame=N` ごとの別の読み込みなので、そのままでは 1 枚ごとにファイル全体を
//! 読み直して解析し直す。読み方は 2 つある。
//!
//! - フレームの場所だけを読める形式（[`vw_core::FrameSource`]）は、頼まれたフレームをその場所だけ
//!   読み、見ているフレームから先を持てる量まで裏で読む。先へ進んだら、後ろに離れたフレームから
//!   捨ててさらに先を読む（窓をずらす）。上限を超えるファイルでも、全体を読み直すことはない
//! - それ以外は、ファイルを一度読んで解析し、最初に頼まれたフレームから裏で順に取り出す。
//!   持つ量はフル解像度の上限までで、超えたら取り出しをやめ、残りは今までどおり 1 枚ずつ読む

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use vw_core::{FrameSource, Registry, VwImage};

/// アプリ全体で 1 つの置き場。読み込みの作業はスレッドごとに別なので、どこからでも引ける所に置く。
pub static STORE: LazyLock<FrameStore> = LazyLock::new(FrameStore::default);

/// 覚えておく読み口の数。読み口はヘッダとフレームの場所の表だけなので小さい。
const SOURCES_KEPT: usize = 32;

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
    /// 上限に当たって、取り出しや先読みを途中でやめた。
    limited: AtomicBool,
    /// 開いた読み口。同じファイルを開き直しても、場所の表を作り直さないよう覚えておく。
    sources: Mutex<VecDeque<Opened>>,
}

#[derive(Clone)]
struct Opened {
    file: PathBuf,
    stamp: Stamp,
    source: Arc<dyn FrameSource>,
    plugin: String,
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
    /// フレームの場所だけを読める形式なら、その読み口。
    source: Option<Opened>,
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
    /// 読み口があるときの、最後に頼まれたフレーム。先読みはここから先へ進む。
    cursor: usize,
    /// 読んでいる最中のフレーム。同じフレームを 2 つの作業で読まないため。
    busy: HashSet<usize>,
    prefetching: bool,
    /// 最後に読んだフレームの大きさ。先読みの前に、読んでも持てるかを見積もる。
    last_size: usize,
}

impl Entry {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn finish(&self) {
        self.state().finished = true;
        self.ready.notify_all();
    }

    fn count(&self) -> usize {
        self.source.as_ref().map_or(0, |o| o.source.frame_count())
    }
}

/// `path`（フレームを指すパスも）の 1 枚を読み、画像とプラグイン名と読んだおよそのバイト数を返す。
/// フレームの場所だけを読める形式はその場所だけを読み、それ以外はファイル全体を読む。
/// 表示の置き場（[`STORE`]）には入れない。見ている画像の先読みを追い出さないため。
pub fn decode_path(registry: &Registry, path: &Path) -> Result<(VwImage, String, usize), String> {
    if crate::moments::split(path).is_some() {
        return crate::moments::decode(registry, path);
    }
    let (file, frame) = crate::frames::split(path).unwrap_or((path.to_path_buf(), 0));
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", file.display());
    if let Some((source, plugin)) = STORE.source(registry, &file) {
        let image = source.decode_frame(frame).map_err(|e| fail(&e))?;
        let read = image_bytes(&image);
        return Ok((image, plugin, read));
    }
    let bytes = std::fs::read(&file).map_err(|e| fail(&e))?;
    let (image, plugin) = registry
        .decode_frame_bytes(&file, &bytes, frame)
        .map_err(|e| fail(&e))?;
    Ok((image, plugin.to_owned(), bytes.len()))
}

/// 先読みの距離。`cursor` から先へ数えて何枚目か（最後の先は頭に戻る）。
fn ahead(frame: usize, cursor: usize, count: usize) -> usize {
    (frame + count - cursor % count) % count
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

    /// `file` の置き場。ファイルが変わっていたら前のものは捨てる。
    fn lookup(&self, file: &Path, now: Stamp) -> Option<Arc<Entry>> {
        let mut entries = self.entries();
        let i = entries.iter().position(|e| e.file == file)?;
        let e = entries.remove(i);
        if e.stamp == now {
            entries.push(e.clone());
            Some(e)
        } else {
            self.drop_entry(&e);
            None
        }
    }

    /// 頼まれる前に別の作業が同じファイルの置き場を作っていれば、そちらを使う。
    fn register(&self, entry: Arc<Entry>) -> Arc<Entry> {
        let mut entries = self.entries();
        if let Some(e) = entries
            .iter()
            .find(|e| e.file == entry.file && e.stamp == entry.stamp)
        {
            return e.clone();
        }
        entries.push(entry.clone());
        entry
    }

    /// `file` の読み口。覚えていなければ開く。開けなければ `None`。
    fn open(&self, registry: &Registry, file: &Path, now: Stamp) -> Option<Opened> {
        {
            let mut sources = self.sources.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = sources.iter().position(|o| o.file == file) {
                let o = sources.remove(i).expect("found just above");
                if o.stamp == now {
                    sources.push_back(o.clone());
                    return Some(o);
                }
            }
        }
        let (source, plugin) = registry.open_frames(file)?;
        if source.frame_count() < 2 {
            return None;
        }
        let opened = Opened {
            file: file.to_path_buf(),
            stamp: now,
            source,
            plugin: plugin.to_owned(),
        };
        let mut sources = self.sources.lock().unwrap_or_else(|e| e.into_inner());
        sources.push_back(opened.clone());
        while sources.len() > SOURCES_KEPT {
            sources.pop_front();
        }
        Some(opened)
    }

    /// `file` のフレームの場所だけを読む口と、そのプラグイン名。ほかの道具（ROI のグラフ、
    /// つなぎ合わせ）がフレームを読むときに使う。開けなければ `None`。
    pub fn source(
        &self,
        registry: &Registry,
        file: &Path,
    ) -> Option<(Arc<dyn FrameSource>, String)> {
        let opened = self.open(registry, file, stamp(file)?)?;
        Some((opened.source, opened.plugin))
    }

    fn start_source(&self, registry: &Registry, file: &Path, now: Stamp) -> Option<Arc<Entry>> {
        let opened = self.open(registry, file, now)?;
        let file_size = opened.source.file_size() as usize;
        let plugin = opened.plugin.clone();
        Some(self.register(Arc::new(Entry {
            file: file.to_path_buf(),
            stamp: now,
            source: Some(opened),
            state: Mutex::new(State {
                file_size,
                plugin,
                ..State::default()
            }),
            ready: Condvar::new(),
        })))
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
        let entry = match self.lookup(file, now) {
            Some(e) => e,
            None => match self.start_source(registry, file, now) {
                Some(e) => e,
                None => self.start(registry, file, now, frame)?,
            },
        };
        if entry.source.is_some() {
            return Some(self.read_frame(&entry, frame));
        }
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

    /// 開いたファイル（フレームを指さないパス）の最初のフレームとフレーム数。フレームの場所だけを
    /// 読める形式でなければ `None` で、呼ぶ側がファイル全体を読む。
    pub fn first(
        &'static self,
        registry: &Registry,
        file: &Path,
    ) -> Option<Result<(Frame, usize), String>> {
        let now = stamp(file)?;
        let entry = match self.lookup(file, now) {
            Some(e) => e,
            None => self.start_source(registry, file, now)?,
        };
        entry.source.as_ref()?;
        let count = entry.count();
        Some(self.read_frame(&entry, 0).map(|f| (f, count)))
    }

    /// 読み口のある置き場から `frame` 番目を渡す。持っていなければその場所だけを読む。
    fn read_frame(&'static self, entry: &Arc<Entry>, frame: usize) -> Result<Frame, String> {
        let source = entry
            .source
            .as_ref()
            .expect("only for entries with a source");
        let count = source.source.frame_count();
        if frame >= count {
            return Err(format!("no frame {frame} (the file has {count})"));
        }
        let mut state = entry.state();
        state.cursor = frame;
        loop {
            if let Some(image) = state.images.get(&frame) {
                let got = Frame {
                    image: VwImage::clone(image),
                    plugin: state.plugin.clone(),
                    file_size: state.file_size,
                    read: Duration::ZERO,
                    decode: Duration::ZERO,
                };
                drop(state);
                self.prefetch(entry);
                return Ok(got);
            }
            if !state.busy.contains(&frame) {
                break;
            }
            state = entry.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        state.busy.insert(frame);
        let (plugin, file_size) = (state.plugin.clone(), state.file_size);
        drop(state);
        let t0 = Instant::now();
        let result = source.source.decode_frame(frame);
        let decode = t0.elapsed();
        let got = match result {
            Ok(image) => {
                if !self.keep(entry, frame, &image) {
                    self.limited.store(true, Ordering::Relaxed);
                }
                Ok(Frame {
                    image,
                    plugin,
                    file_size,
                    read: Duration::ZERO,
                    decode,
                })
            }
            Err(e) => {
                entry.state().busy.remove(&frame);
                Err(e.to_string())
            }
        };
        entry.ready.notify_all();
        self.prefetch(entry);
        got
    }

    /// 読んだフレームを持つ。持てなければ `false`（読んだ印だけを外す）。
    fn keep(&self, entry: &Arc<Entry>, frame: usize, image: &VwImage) -> bool {
        let size = image_bytes(image);
        let room = self.make_room(size, entry, Some(frame));
        let mut state = entry.state();
        state.busy.remove(&frame);
        state.last_size = size;
        if !room {
            return false;
        }
        if state.evicted {
            self.held.fetch_sub(size, Ordering::Relaxed);
            return false;
        }
        state.images.insert(frame, Arc::new(image.clone()));
        state.bytes += size;
        true
    }

    /// 見ているフレームから先を、持てる分だけ裏で読む。もう読んでいれば何もしない。
    fn prefetch(&'static self, entry: &Arc<Entry>) {
        {
            let mut state = entry.state();
            if state.prefetching || state.evicted {
                return;
            }
            state.prefetching = true;
        }
        let entry = entry.clone();
        std::thread::spawn(move || {
            self.prefetch_ahead(&entry);
            entry.state().prefetching = false;
            entry.ready.notify_all();
        });
    }

    fn prefetch_ahead(&self, entry: &Arc<Entry>) {
        let source = entry
            .source
            .as_ref()
            .expect("only for entries with a source");
        let count = source.source.frame_count();
        loop {
            let (next, estimate) = {
                let mut state = entry.state();
                if state.evicted {
                    return;
                }
                let cursor = state.cursor;
                let next = (1..count)
                    .map(|k| (cursor + k) % count)
                    .find(|i| !state.images.contains_key(i) && !state.busy.contains(i));
                let Some(next) = next else {
                    return;
                };
                state.busy.insert(next);
                (next, state.last_size)
            };
            // 読んでから持てないと分かると、ネットワーク越しでは 1 フレーム分の読み込みが無駄になる。
            if !self.has_room(estimate, entry, next) {
                entry.state().busy.remove(&next);
                self.limited.store(true, Ordering::Relaxed);
                return;
            }
            match source.source.decode_frame(next) {
                Ok(image) => {
                    let kept = self.keep(entry, next, &image);
                    entry.ready.notify_all();
                    if !kept {
                        self.limited.store(true, Ordering::Relaxed);
                        return;
                    }
                }
                Err(_) => {
                    // 頼まれたときに読み直してエラーを伝える。
                    entry.state().busy.remove(&next);
                    entry.ready.notify_all();
                    return;
                }
            }
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
        // 形式の名前は、取り出しを始める前に決めておく。取り出し終えてから入れると、それまでに
        // 渡した面が形式の名前なしで出て、形式ごとの色の表などが当たらない。
        let plugin = registry.find(file, &bytes)?.name().to_owned();
        let entry = Arc::new(Entry {
            file: file.to_path_buf(),
            stamp,
            source: None,
            state: Mutex::new(State {
                file_size: bytes.len(),
                read,
                plugin,
                ..State::default()
            }),
            ready: Condvar::new(),
        });
        let registered = self.register(entry.clone());
        if !Arc::ptr_eq(&registered, &entry) {
            return Some(registered);
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
            if !self.make_room(size, entry, None) {
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
        if let Err(e) = result {
            let mut state = entry.state();
            for &i in order {
                state.errors.entry(i).or_insert_with(|| e.to_string());
            }
        }
    }

    /// `keep` の `frame` に `size` バイトを足しても、ほかのファイルと、`keep` の中で `frame` より
    /// 先に離れたフレームを捨てれば上限に収まるか。
    fn has_room(&self, size: usize, keep: &Arc<Entry>, frame: usize) -> bool {
        let budget = self.budget.load(Ordering::Relaxed);
        let entries = self.entries();
        let others: usize = entries
            .iter()
            .filter(|e| !Arc::ptr_eq(e, keep))
            .map(|e| e.state().bytes)
            .sum();
        drop(entries);
        let state = keep.state();
        let count = keep.count().max(1);
        let farther: usize = state
            .images
            .iter()
            .filter(|(i, _)| ahead(**i, state.cursor, count) > ahead(frame, state.cursor, count))
            .map(|(_, img)| image_bytes(img))
            .sum();
        self.held.load(Ordering::Relaxed) + size <= budget + others + farther
    }

    /// `size` バイトを足せるよう、ほかのファイルのフレームを古いものから捨てる。それでも足りなければ、
    /// `keep` の中で `frame` より先に離れたフレーム（見ている所の後ろに残ったもの）を遠い順に捨てる。
    /// 足せないなら `false`。
    fn make_room(&self, size: usize, keep: &Arc<Entry>, frame: Option<usize>) -> bool {
        let budget = self.budget.load(Ordering::Relaxed);
        let mut entries = self.entries();
        while self.held.load(Ordering::Relaxed) + size > budget {
            if let Some(i) = entries.iter().position(|e| !Arc::ptr_eq(e, keep)) {
                let e = entries.remove(i);
                self.drop_entry(&e);
                continue;
            }
            let Some(frame) = frame else {
                return false;
            };
            let mut state = keep.state();
            let count = keep.count().max(1);
            let cursor = state.cursor;
            let limit = ahead(frame, cursor, count);
            let victim = state
                .images
                .keys()
                .copied()
                .filter(|&i| ahead(i, cursor, count) > limit)
                .max_by_key(|&i| ahead(i, cursor, count));
            let Some(victim) = victim else {
                return false;
            };
            let img = state.images.remove(&victim).expect("picked from the map");
            let bytes = image_bytes(&img);
            state.bytes -= bytes;
            self.held.fetch_sub(bytes, Ordering::Relaxed);
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
    use std::sync::Mutex;
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

    /// [`Frames`] と同じ形で、最初のフレームを渡した後は `release` が立つまで次を渡さない。
    #[derive(Default)]
    struct Slow {
        release: AtomicBool,
    }

    impl DecoderPlugin for Slow {
        fn name(&self) -> &str {
            "Slow"
        }
        fn extensions(&self) -> &[&str] {
            &["slow"]
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
            Ok(gray(frame as u8))
        }
        fn decode_frames(
            &self,
            _path: &Path,
            _data: &[u8],
            order: &[usize],
            each: &mut dyn FnMut(usize, Result<VwImage, DecodeError>) -> bool,
        ) {
            for (n, &i) in order.iter().enumerate() {
                if n > 0 {
                    let until = Instant::now() + Duration::from_secs(5);
                    while !self.release.load(Ordering::Relaxed) && Instant::now() < until {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                if !each(i, Ok(gray(i as u8))) {
                    return;
                }
            }
        }
    }

    #[test]
    fn frames_handed_out_while_the_rest_are_still_decoding_carry_the_format_name() {
        let store: &'static FrameStore = Box::leak(Box::default());
        store.set_budget(1 << 20);
        let plugin = Arc::new(Slow::default());
        let mut registry = Registry::new();
        registry.register(plugin.clone());
        let dir = std::env::temp_dir().join(format!("vw-frame-store-slow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cube.slow");
        std::fs::write(&file, [4]).unwrap();
        let frame = store.frame(&registry, &file, 0).unwrap().unwrap();
        assert_eq!(frame.plugin, "Slow");
        plugin.release.store(true, Ordering::Relaxed);
    }

    #[test]
    fn a_single_frame_file_is_left_to_the_caller() {
        let (store, registry, _, file) = setup(1, 1 << 20);
        assert!(store.frame(&registry, &file, 0).is_none());
    }

    /// フレームの場所だけを読める形式。拡張子 `placed`、先頭のバイトがフレーム数で、`n` 番目の
    /// フレームが画素値 `n` の 1x1 のグレー。フレームごとに読んだ回数を数え、ファイル全体の
    /// 解析（decode_frame）は数えて 0 であることを確かめる。
    #[derive(Default)]
    struct Placed {
        whole: AtomicUsize,
        reads: Arc<Mutex<HashMap<usize, usize>>>,
    }

    struct PlacedSource {
        count: usize,
        reads: Arc<Mutex<HashMap<usize, usize>>>,
    }

    impl FrameSource for PlacedSource {
        fn frame_count(&self) -> usize {
            self.count
        }
        fn file_size(&self) -> u64 {
            1
        }
        fn decode_frame(&self, frame: usize) -> Result<VwImage, DecodeError> {
            *self.reads.lock().unwrap().entry(frame).or_default() += 1;
            Ok(gray(frame as u8))
        }
    }

    impl DecoderPlugin for Placed {
        fn name(&self) -> &str {
            "Placed"
        }
        fn extensions(&self) -> &[&str] {
            &["placed"]
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
            self.whole.fetch_add(1, Ordering::Relaxed);
            Ok(gray(frame as u8))
        }
        fn open_frames(&self, path: &Path) -> Option<Arc<dyn FrameSource>> {
            let count = std::fs::read(path).ok()?[0] as usize;
            Some(Arc::new(PlacedSource {
                count,
                reads: self.reads.clone(),
            }))
        }
    }

    fn setup_placed(
        frames: u8,
        budget: usize,
        name: &str,
    ) -> (&'static FrameStore, Registry, Arc<Placed>, PathBuf) {
        let store: &'static FrameStore = Box::leak(Box::default());
        store.set_budget(budget);
        let plugin = Arc::new(Placed::default());
        let mut registry = Registry::new();
        registry.register(plugin.clone());
        let dir =
            std::env::temp_dir().join(format!("vw-frame-store-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tomo.placed");
        std::fs::write(&file, [frames]).unwrap();
        (store, registry, plugin, file)
    }

    /// 裏の先読みが止まるまで待ち、持っているフレームを返す。
    fn settled(store: &FrameStore, file: &Path) -> Vec<usize> {
        for _ in 0..500 {
            let entries = store.entries();
            let entry = entries.iter().find(|e| e.file == file).unwrap().clone();
            drop(entries);
            let state = entry.state();
            if !state.prefetching && state.busy.is_empty() {
                let mut held: Vec<usize> = state.images.keys().copied().collect();
                held.sort();
                return held;
            }
            drop(state);
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("prefetching did not stop");
    }

    #[test]
    fn frames_of_a_placed_file_are_read_one_by_one_without_reading_the_whole_file() {
        let (store, registry, plugin, file) = setup_placed(5, 1 << 20, "whole");
        for i in [3, 0, 4] {
            assert_eq!(
                value(store.frame(&registry, &file, i).unwrap().unwrap()),
                i as u8
            );
        }
        assert_eq!(settled(store, &file), [0, 1, 2, 3, 4]);
        assert_eq!(plugin.whole.load(Ordering::Relaxed), 0);
        assert!(plugin.reads.lock().unwrap().values().all(|&n| n == 1));
    }

    #[test]
    fn prefetching_keeps_the_frames_ahead_of_the_one_shown_within_the_budget() {
        // 1 フレーム 1 バイトで、3 フレーム分だけ持てる。
        let (store, registry, plugin, file) = setup_placed(10, 3, "window");
        store.frame(&registry, &file, 0).unwrap().unwrap();
        assert_eq!(settled(store, &file), [0, 1, 2]);
        store.frame(&registry, &file, 5).unwrap().unwrap();
        assert_eq!(settled(store, &file), [5, 6, 7]);
        store.frame(&registry, &file, 8).unwrap().unwrap();
        // 最後の先は頭に戻る。
        assert_eq!(settled(store, &file), [0, 8, 9]);
        assert!(store.take_limited());
        let reads = plugin.reads.lock().unwrap();
        assert_eq!(reads[&5], 1);
        assert_eq!(reads[&0], 2, "read again only after it was dropped behind");
    }

    #[test]
    fn a_frame_larger_than_the_budget_is_still_handed_out() {
        let (store, registry, _, file) = setup_placed(3, 0, "tiny");
        assert_eq!(value(store.frame(&registry, &file, 1).unwrap().unwrap()), 1);
        assert!(store.take_limited());
        assert_eq!(settled(store, &file), Vec::<usize>::new());
    }

    #[test]
    fn opening_a_placed_file_gives_its_first_frame_and_frame_count() {
        let (store, registry, plugin, file) = setup_placed(4, 1 << 20, "first");
        let (frame, count) = store.first(&registry, &file).unwrap().unwrap();
        assert_eq!((value(frame), count), (0, 4));
        assert_eq!(plugin.whole.load(Ordering::Relaxed), 0);
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
