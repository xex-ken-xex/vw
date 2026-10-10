//! 画像の読み込み。ファイル → VwImage（プラグイン）→ RGBA8 の変換を UI スレッドの外で行う。
//!
//! UI スレッドで読み込むと、大きな画像の間ウィンドウが固まるため。

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage};
use vw_core::{display, ColorModel, DisplayImage, DisplayParams, Metadata, Registry, VwImage};

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
    pub plugin: String,
    pub file_size: u64,
    pub read: Duration,
    pub decode: Duration,
    pub convert: Duration,
}

impl ImageInfo {
    /// フル解像度で持ったときの見込みバイト数（[`FullImage::bytes`] と同じ数え方）。
    pub fn full_bytes(&self) -> usize {
        let pixels = self.width as usize * self.height as usize;
        let sample = pixels * self.color.channels() * (self.bits_per_sample as usize).div_ceil(8);
        // 描画用バッファは to_display と同じ整数倍の間引きで上限に収める。
        let (w, h) = (self.width as usize, self.height as usize);
        let step = w
            .max(h)
            .div_ceil(DisplayParams::default().max_side as usize)
            .max(1);
        sample + w.div_ceil(step) * h.div_ceil(step) * 4
    }
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
    pub rx: Receiver<LoadResult>,
}

impl Loader {
    pub fn new(registry: Registry, ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 8));
        for _ in 0..workers {
            let (registry, queue, tx, ctx) =
                (registry.clone(), queue.clone(), tx.clone(), ctx.clone());
            std::thread::spawn(move || worker(&registry, &queue, &tx, &ctx));
        }
        Self {
            registry,
            queue,
            rx,
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// 読み込みを予約する。結果は [`Self::rx`] に届く。
    pub fn request(&self, job: Job) {
        let (lock, cvar) = &*self.queue;
        lock.lock().unwrap().jobs.push_back(job);
        cvar.notify_one();
    }

    /// まだ始まっていない要求を取り消す。始まったものは最後まで読むが、結果は要求番号で捨てられる。
    pub fn cancel(&self, request: u64) {
        let mut q = self.queue.0.lock().unwrap();
        if let Some(i) = q.jobs.iter().position(|j| j.request == request) {
            q.jobs.remove(i);
        } else {
            q.cancelled.insert(request);
        }
    }
}

fn worker(
    registry: &Registry,
    queue: &(Mutex<Queue>, Condvar),
    tx: &Sender<LoadResult>,
    ctx: &egui::Context,
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
        let result = load(registry, &job.path, job.thumb, job.full);
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

/// 1 枚を読み込む。ファイルは一度に全部読む。メモリマップにしないのは、
/// 読み込み中にファイルが書き換えられると未定義動作になりうるため。
pub fn load(registry: &Registry, path: &Path, thumb: bool, full: bool) -> Result<Loaded, String> {
    let t0 = Instant::now();
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let t1 = Instant::now();
    let (image, plugin) = registry
        .decode_bytes(path, &bytes)
        .map_err(|e| e.to_string())?;
    let plugin = plugin.to_owned();
    let t2 = Instant::now();
    let d = display::to_display(&image, &DisplayParams::default());
    let thumb = thumb.then(|| color_image(&display::shrink(&d, THUMB_SIDE)));
    let full = full.then(|| color_image(&d));
    let t3 = Instant::now();
    let info = ImageInfo {
        width: image.width,
        height: image.height,
        color: image.color,
        bits_per_sample: image.data.bits_per_sample(),
        significant_bits: image.significant_bits,
        metadata: image.metadata.clone(),
        plugin,
        file_size: bytes.len() as u64,
        read: t1 - t0,
        decode: t2 - t1,
        convert: t3 - t2,
    };
    let full = full.map(|display| FullImage {
        image: Arc::new(image),
        display,
    });
    Ok(Loaded { info, thumb, full })
}

fn color_image(d: &DisplayImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([d.width as usize, d.height as usize], &d.rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{png_registry, write_png, TempDir};

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

    fn job(request: u64, path: &Path) -> Job {
        Job {
            slot: 0,
            request,
            path: path.to_owned(),
            thumb: true,
            full: false,
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
