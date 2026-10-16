//! 画像の読み込み。ファイル → VwImage（プラグイン）→ RGBA8 の変換を UI スレッドの外で行う。
//!
//! UI スレッドで読み込むと、大きな画像の間ウィンドウが固まるため。

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage};
use vw_core::{display, ColorModel, DisplayImage, DisplayParams, Metadata, Registry, VwImage};

use crate::frames;
use crate::memory::THUMB_SIDE;

/// 読み込みが成功したときの結果。
pub struct Loaded {
    pub info: ImageInfo,
    /// 縮小画像。要求で `thumb` を指定したときだけ作る。
    pub thumb: Option<ColorImage>,
    /// フル解像度の画像。要求で `full` を指定したときだけ返す。
    pub full: Option<FullImage>,
}

/// フル解像度の画像と、その描画用バッファ。
pub struct FullImage {
    pub image: Arc<VwImage>,
    pub display: ColorImage,
}

impl FullImage {
    /// 持っている間に使うメモリ。描画用バッファはテクスチャに移すが、GPU 側にも
    /// 同じ大きさを取るので、その分も数える。
    pub fn bytes(&self) -> usize {
        image_bytes(&self.image) + self.display.pixels.len() * 4
    }
}

/// 読み込んだ画像の情報。画素は持たない。
/// 各段階の所要時間はログに出して速度を確認するのに使う。
#[derive(Clone)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub color: ColorModel,
    pub bits_per_sample: u32,
    pub significant_bits: u32,
    pub metadata: Metadata,
    /// 現像すると作れる画像の大きさ（[`VwImage::develop_size`]）。
    pub develop_size: Option<(u32, u32)>,
    pub plugin: String,
    /// ファイルに入っているフレームの数。フレームを指すパスで読んだときは 1。
    pub frames: usize,
    pub file_size: u64,
    pub read: Duration,
    pub decode: Duration,
    pub convert: Duration,
}

impl ImageInfo {
    /// フル解像度で持ったときの見込みバイト数（[`FullImage::bytes`] と同じ数え方）。
    pub fn full_bytes(&self) -> usize {
        estimate_bytes(
            (self.width, self.height),
            self.color.channels(),
            self.bits_per_sample,
        )
    }

    /// 現像したときの見込みバイト数。現像した画像は 16bit の RGB。
    pub fn developed_bytes(&self) -> Option<usize> {
        self.develop_size.map(|size| estimate_bytes(size, 3, 16))
    }
}

/// 幅 x 高さ `size` の画像と描画用バッファで使うバイト数（[`FullImage::bytes`] と同じ数え方）。
fn estimate_bytes(size: (u32, u32), channels: usize, bits_per_sample: u32) -> usize {
    let (w, h) = (size.0 as usize, size.1 as usize);
    let sample = w * h * channels * (bits_per_sample as usize).div_ceil(8);
    // 描画用バッファは to_display と同じ整数倍の間引きで上限に収める。
    let step = w
        .max(h)
        .div_ceil(DisplayParams::default().max_side as usize)
        .max(1);
    sample + w.div_ceil(step) * h.div_ceil(step) * 4
}

fn image_bytes(img: &VwImage) -> usize {
    img.data.len() * (img.data.bits_per_sample() as usize).div_ceil(8)
}

/// 読み込みスレッドから UI スレッドへ送る結果。どのスロットのどの要求かを添える。
pub struct LoadResult {
    pub slot: u64,
    pub request: u64,
    pub path: PathBuf,
    pub result: Result<Loaded, String>,
}

/// 読み込み要求。
#[derive(Clone)]
pub struct Job {
    pub slot: u64,
    pub request: u64,
    pub path: PathBuf,
    pub thumb: bool,
    pub full: bool,
    /// 普通に読む代わりに現像する（[`develop`]）。`thumb` と `full` は見ない。
    pub develop: bool,
    /// `full` の描画用バッファを、縦横がこの大きさを下回らない範囲で縮める（[`cover_side`]）。
    /// 全画面に出すだけの画像のために、画面より大きなテクスチャを作らないため。
    pub fit_side: Option<u32>,
}

/// 待っている要求と、取り消された要求。
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    cancelled: HashSet<u64>,
}

/// 読み込み要求を受け付け、決まった数のスレッドで読み、結果をチャネルで返す。
///
/// 要求ごとにスレッドを立てないのは、数百枚をまとめて開いたときに全部が同時に
/// デコードを始め、作業用のメモリが一度に膨らむため。
pub struct Loader {
    registry: Registry,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    /// 現像の要求。普通の読み込みとは別のスレッドで処理する。現像は 1 枚に数秒かかるので、
    /// 同じ待ち行列に並べると、フォルダを開いたときのサムネイルが現像の後ろで待たされるため。
    develop_queue: Arc<(Mutex<Queue>, Condvar)>,
    /// 読んでいる最中の要求の数。
    running: Arc<AtomicUsize>,
    workers: usize,
    pub rx: Receiver<LoadResult>,
}

/// 読み込みの込み具合。
pub struct Counts {
    pub waiting: usize,
    pub running: usize,
    pub workers: usize,
}

/// 現像に使うスレッドの数。
///
/// コア数まで増やさないのは、rawler が 1 枚の現像の中で全コアを使う（rayon）ので、
/// 同時に現像する枚数を増やしても速くならず、作業用のメモリ（1 枚に数百 MB）だけが増えるため。
/// 1 ではなく 2 にするのは、ファイルの読み込みなど 1 枚の中で並列にならない部分の間も
/// CPU を使い切るため。
const DEVELOP_WORKERS: usize = 2;

impl Loader {
    pub fn new(registry: Registry, ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 8));
        let develop_queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let running: Arc<AtomicUsize> = Arc::default();
        let queues = std::iter::repeat_n(&queue, workers)
            .chain(std::iter::repeat_n(&develop_queue, DEVELOP_WORKERS));
        for queue in queues {
            let (registry, queue, tx, ctx, running) = (
                registry.clone(),
                queue.clone(),
                tx.clone(),
                ctx.clone(),
                running.clone(),
            );
            std::thread::spawn(move || worker(&registry, &queue, &tx, &ctx, &running));
        }
        Self {
            registry,
            queue,
            develop_queue,
            running,
            workers: workers + DEVELOP_WORKERS,
            rx,
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// 読み込みを予約する。結果は [`Self::rx`] に届く。
    pub fn request(&self, job: Job) {
        let queue = if job.develop {
            &self.develop_queue
        } else {
            &self.queue
        };
        let (lock, cvar) = &**queue;
        lock.lock().unwrap().jobs.push_back(job);
        cvar.notify_one();
    }

    pub fn counts(&self) -> Counts {
        let waiting = [&self.queue, &self.develop_queue]
            .iter()
            .map(|q| q.0.lock().unwrap().jobs.len())
            .sum();
        Counts {
            waiting,
            running: self.running.load(Ordering::Relaxed),
            workers: self.workers,
        }
    }

    /// まだ始まっていない要求を取り消す。始まったものは最後まで読むが、結果は要求番号で捨てられる。
    pub fn cancel(&self, request: u64) {
        for queue in [&self.queue, &self.develop_queue] {
            let mut q = queue.0.lock().unwrap();
            if let Some(i) = q.jobs.iter().position(|j| j.request == request) {
                q.jobs.remove(i);
                return;
            }
        }
        // どちらの待ち行列にもなければ始まっている。どちらで始まったかは分からないので両方に記す。
        for queue in [&self.queue, &self.develop_queue] {
            queue.0.lock().unwrap().cancelled.insert(request);
        }
    }
}

fn worker(
    registry: &Registry,
    queue: &(Mutex<Queue>, Condvar),
    tx: &Sender<LoadResult>,
    ctx: &egui::Context,
    running: &AtomicUsize,
) {
    let (lock, cvar) = queue;
    loop {
        let job = {
            let mut q = lock.lock().unwrap();
            loop {
                if let Some(job) = q.jobs.pop_front() {
                    if q.cancelled.remove(&job.request) {
                        continue;
                    }
                    break job;
                }
                q = cvar.wait(q).unwrap();
            }
        };
        running.fetch_add(1, Ordering::Relaxed);
        let result = if job.develop {
            develop(registry, &job.path)
        } else {
            load_fit(registry, &job.path, job.thumb, job.full, job.fit_side)
        };
        running.fetch_sub(1, Ordering::Relaxed);
        let sent = tx.send(LoadResult {
            slot: job.slot,
            request: job.request,
            path: job.path,
            result,
        });
        if sent.is_err() {
            return; // アプリが終わった。
        }
        ctx.request_repaint();
    }
}

/// [`load_fit`] の縮めない版。
#[cfg(test)]
pub fn load(registry: &Registry, path: &Path, thumb: bool, full: bool) -> Result<Loaded, String> {
    load_fit(registry, path, thumb, full, None)
}

/// 1 枚を読み込む。ファイルは一度に全部読む。メモリマップにしないのは、
/// 読み込み中にファイルが書き換えられると未定義動作になりうるため。
/// `fit_side` があれば、フル解像度の描画用バッファをそれに合わせて縮める。
pub fn load_fit(
    registry: &Registry,
    path: &Path,
    thumb: bool,
    full: bool,
    fit_side: Option<u32>,
) -> Result<Loaded, String> {
    let t0 = Instant::now();
    let (file, frame) =
        frames::split(path).map_or((path.to_path_buf(), None), |(f, i)| (f, Some(i)));
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    let t1 = Instant::now();
    let (image, plugin) = registry
        .decode_frame_bytes(&file, &bytes, frame.unwrap_or(0))
        .map_err(|e| e.to_string())?;
    let frames = match frame {
        Some(_) => 1,
        None => registry.frame_count(&file, &bytes).unwrap_or(1),
    };
    let t2 = Instant::now();
    let times = (t1 - t0, t2 - t1);
    Ok(finish(
        image,
        plugin,
        frames,
        bytes.len(),
        times,
        thumb,
        full,
        fit_side,
    ))
}

/// 1 枚を [`vw_core::DecoderPlugin::develop`] で作り直す。縮小画像とフル解像度の画像を返す。
pub fn develop(registry: &Registry, path: &Path) -> Result<Loaded, String> {
    let t0 = Instant::now();
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let t1 = Instant::now();
    let (image, plugin) = registry
        .develop_bytes(path, &bytes)
        .map_err(|e| e.to_string())?;
    let t2 = Instant::now();
    let times = (t1 - t0, t2 - t1);
    Ok(finish(
        image,
        plugin,
        1,
        bytes.len(),
        times,
        true,
        true,
        None,
    ))
}

/// デコードした画像から、要る分の縮小画像・描画用バッファと情報を作る。
/// `times` は読み込みとデコードにかかった時間。
#[allow(clippy::too_many_arguments)]
fn finish(
    image: VwImage,
    plugin: &str,
    frames: usize,
    file_size: usize,
    (read, decode): (Duration, Duration),
    thumb: bool,
    full: bool,
    fit_side: Option<u32>,
) -> Loaded {
    let t2 = Instant::now();
    let d = display::to_display(&image, &DisplayParams::default());
    let thumb = thumb.then(|| color_image(&display::shrink(&d, THUMB_SIDE)));
    let full = full.then(|| match fit_side {
        Some(side) => color_image(&display::shrink(
            &d,
            cover_side(d.width.max(d.height), side),
        )),
        None => color_image(&d),
    });
    let t3 = Instant::now();
    let info = ImageInfo {
        width: image.width,
        height: image.height,
        color: image.color,
        bits_per_sample: image.data.bits_per_sample(),
        significant_bits: image.significant_bits,
        metadata: image.metadata.clone(),
        develop_size: image.develop_size,
        plugin: plugin.to_owned(),
        frames,
        file_size: file_size as u64,
        read,
        decode,
        convert: t3 - t2,
    };
    let full = full.map(|display| FullImage {
        image: Arc::new(image),
        display,
    });
    Loaded { info, thumb, full }
}

/// 長辺 `long` の画像を、長辺が `side` を下回らない範囲で最も小さく整数倍で縮めたときの長辺。
/// [`display::shrink`] に渡す上限に使う。`shrink` は上限以下に収めるので、`side` をそのまま
/// 渡すと画面より小さくなり、全画面に引き伸ばしたときにぼやけるため。
pub fn cover_side(long: u32, side: u32) -> u32 {
    let n = (long / side.max(1)).max(1);
    long.div_ceil(n)
}

/// 既定の表示（ファイルの値）での描画用バッファを、テクスチャの大きさ `size` で作る。
pub fn default_color_image(image: &VwImage, size: [usize; 2]) -> ColorImage {
    let d = display::to_display(image, &DisplayParams::default());
    let d = display::shrink(&d, size[0].max(size[1]) as u32);
    fit_color_image(color_image(&d), size)
}

/// `image` をテクスチャの大きさ `size` に合わせる。読み込みで間引いたテクスチャ
/// （テクスチャの大きさの上限や全画面）に差し替えるとき用で、最も近い画素を取る。
pub fn fit_color_image(image: ColorImage, size: [usize; 2]) -> ColorImage {
    if image.size == size {
        return image;
    }
    let [w, h] = size;
    let [sw, sh] = image.size;
    let pixels = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w * sw / w.max(1), i / w * sh / h.max(1));
            image.pixels[y * sw + x]
        })
        .collect();
    ColorImage::new(size, pixels)
}

fn color_image(d: &DisplayImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([d.width as usize, d.height as usize], &d.rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{png_registry, write_png, TempDir};

    #[test]
    fn multiframe_file_reports_its_frames_and_frame_paths_load_one_frame() {
        use vw_dicom::synth::{Synth, XA_IMAGE};
        let dir = TempDir::new();
        let path = dir.path().join("cine.dcm");
        Synth::new(XA_IMAGE, 1, 1)
            .frames_u8(&[&[10], &[200]])
            .write(&path)
            .unwrap();
        let registry = crate::test_util::all_registry();
        let whole = load(&registry, &path, true, false).unwrap();
        assert_eq!(whole.info.frames, 2);
        let second = load(&registry, &frames::frame_path(&path, 1), true, false).unwrap();
        assert_eq!(second.info.frames, 1);
        assert_eq!(
            second.thumb.unwrap().pixels[0],
            egui::Color32::from_gray(200)
        );
    }

    #[test]
    fn loads_png_into_info_and_thumbnail() {
        let dir = TempDir::new();
        let path = dir.path().join("red.png");
        write_png(&path, 2, 1, &[255, 0, 0, 255, 0, 0]);
        let loaded = load(&png_registry(), &path, true, false).unwrap();
        assert_eq!(loaded.info.plugin, "PNG");
        assert_eq!((loaded.info.width, loaded.info.height), (2, 1));
        assert_eq!(
            loaded.info.file_size,
            std::fs::metadata(&path).unwrap().len()
        );
        let thumb = loaded.thumb.unwrap();
        assert_eq!(thumb.size, [2, 1]);
        assert_eq!(thumb.pixels[0], egui::Color32::RED);
        assert!(loaded.full.is_none());
    }

    #[test]
    fn thumbnail_fits_within_512_pixels() {
        let dir = TempDir::new();
        let path = dir.path().join("wide.png");
        write_png(&path, 1500, 3, &vec![9; 1500 * 3 * 3]);
        let loaded = load(&png_registry(), &path, true, true).unwrap();
        let thumb = loaded.thumb.unwrap();
        assert!(thumb.size[0] <= THUMB_SIDE as usize);
        assert_eq!(loaded.full.unwrap().display.size, [1500, 3]);
    }

    #[test]
    fn full_image_bytes_match_the_estimate_from_info() {
        let dir = TempDir::new();
        let path = dir.path().join("p.png");
        write_png(&path, 4, 3, &[0; 36]);
        let loaded = load(&png_registry(), &path, false, true).unwrap();
        assert!(loaded.thumb.is_none());
        assert_eq!(loaded.full.unwrap().bytes(), loaded.info.full_bytes());
    }

    #[test]
    fn fitted_full_image_stays_at_least_as_large_as_the_side() {
        let dir = TempDir::new();
        let path = dir.path().join("big.png");
        write_png(&path, 1000, 10, &vec![9; 1000 * 10 * 3]);
        let loaded = load_fit(&png_registry(), &path, false, true, Some(300)).unwrap();
        // 1000 を 3 で割って 334。4 で割ると 250 で 300 を下回る。
        assert_eq!(loaded.full.unwrap().display.size, [334, 4]);
    }

    #[test]
    fn cover_side_does_not_shrink_images_smaller_than_the_side() {
        assert_eq!(cover_side(200, 300), 200);
        assert_eq!(cover_side(600, 300), 300);
        assert_eq!(cover_side(899, 300), 450);
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = TempDir::new();
        assert!(load(&png_registry(), &dir.path().join("none.png"), true, false).is_err());
    }

    #[test]
    fn unsupported_file_is_an_error() {
        let dir = TempDir::new();
        let path = dir.touch("note.txt");
        let Err(err) = load(&png_registry(), &path, true, false) else {
            panic!("a text file must not load");
        };
        assert!(err.contains("no plugin"), "{err}");
    }

    #[test]
    fn image_that_cannot_be_developed_is_an_error() {
        let dir = TempDir::new();
        let path = dir.path().join("a.png");
        write_png(&path, 1, 1, &[1, 2, 3]);
        let Err(err) = develop(&png_registry(), &path) else {
            panic!("a PNG has nothing to develop");
        };
        assert!(err.contains("cannot be developed"), "{err}");
    }

    #[test]
    fn developed_size_estimate_assumes_16bit_rgb() {
        let dir = TempDir::new();
        let path = dir.path().join("a.png");
        write_png(&path, 1, 1, &[1, 2, 3]);
        let mut info = load(&png_registry(), &path, true, false).unwrap().info;
        assert_eq!(info.developed_bytes(), None);
        info.develop_size = Some((10, 20));
        // 10 x 20 画素 x RGB x 2 バイトと、描画用の RGBA 8bit。
        assert_eq!(info.developed_bytes(), Some(10 * 20 * 3 * 2 + 10 * 20 * 4));
    }

    fn job(request: u64, path: &Path) -> Job {
        Job {
            slot: 0,
            request,
            path: path.to_owned(),
            thumb: true,
            full: false,
            develop: false,
            fit_side: None,
        }
    }

    #[test]
    fn requests_are_answered_through_the_channel() {
        let dir = TempDir::new();
        let path = dir.path().join("a.png");
        write_png(&path, 1, 1, &[1, 2, 3]);
        let loader = Loader::new(png_registry(), egui::Context::default());
        loader.request(job(7, &path));
        let r = loader.rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(r.request, 7);
        assert!(r.result.is_ok());
    }

    #[test]
    fn develop_requests_are_answered_through_the_channel() {
        let dir = TempDir::new();
        let path = dir.path().join("a.png");
        write_png(&path, 1, 1, &[1, 2, 3]);
        let loader = Loader::new(png_registry(), egui::Context::default());
        loader.request(Job {
            develop: true,
            ..job(8, &path)
        });
        let r = loader.rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(r.request, 8);
        assert!(r.result.is_err(), "a PNG has nothing to develop");
    }

    #[test]
    fn cancelled_request_that_has_not_started_is_skipped() {
        let dir = TempDir::new();
        let path = dir.path().join("a.png");
        write_png(&path, 1, 1, &[1, 2, 3]);
        let loader = Loader::new(png_registry(), egui::Context::default());
        // 取り消しが先に届いた要求は、ワーカーが取り出しても読まない。
        loader.cancel(1);
        loader.request(job(1, &path));
        loader.request(job(2, &path));
        let r = loader.rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(r.request, 2);
        assert!(loader.rx.recv_timeout(Duration::from_millis(200)).is_err());
    }
}
