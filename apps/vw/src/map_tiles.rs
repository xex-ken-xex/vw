//! 地図のタイルの取得・保存・テクスチャ化。
//!
//! OpenStreetMap のタイルの利用条件（https://operations.osmfoundation.org/policies/tiles/）に
//! 合わせて、vw 独自の User-Agent を付け、取ったタイルはディスクに 7 日以上残し、
//! 画面に見えているタイルしか取らない（先読みや一括のダウンロードはしない）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use vw_core::{DisplayParams, Registry, display};

use crate::geo_map::TileId;

/// この間に取ったタイルは、取り直さずにディスクのものを使う。
const FRESH: Duration = Duration::from_secs(7 * 24 * 3600);
/// 取れなかったタイルを、もう一度取りに行くまでの間。
const RETRY: Duration = Duration::from_secs(30);
/// 持っておくテクスチャの数。地図を動かし続けてもメモリが増え続けないようにする
/// （256 画素のタイル 1 枚で 256 KB）。
const KEEP_TEXTURES: usize = 600;
/// 同時に取りに行く数。OpenStreetMap の条件では、1 つのアプリからの同時の接続を少なくする。
const WORKERS: usize = 2;

pub fn user_agent() -> String {
    format!(
        "vw/{} (+{})",
        crate::about::VERSION,
        crate::about::REPOSITORY
    )
}

/// `{z}`・`{x}`・`{y}` をタイルの番号に置き換えた URL。
pub fn tile_url(template: &str, id: TileId) -> String {
    template
        .replace("{z}", &id.z.to_string())
        .replace("{x}", &id.x.to_string())
        .replace("{y}", &id.y.to_string())
}

/// タイルの URL として使えるか。設定の確認に使う。
pub fn valid_template(template: &str) -> bool {
    (template.starts_with("https://") || template.starts_with("http://"))
        && ["{z}", "{x}", "{y}"].iter().all(|k| template.contains(k))
}

/// 提供元ごとにディスクの置き場を分ける。URL を変えたとき、別の地図の絵が混ざらないため。
fn cache_dir(root: &Path, template: &str) -> PathBuf {
    // FNV-1a。置き場の名前に使うだけなので、暗号のハッシュは要らない。
    let hash = template.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    root.join(format!("{hash:016x}"))
}

fn cache_path(dir: &Path, id: TileId) -> PathBuf {
    dir.join(id.z.to_string())
        .join(id.x.to_string())
        .join(format!("{}.tile", id.y))
}

enum Entry {
    Waiting,
    Ready { texture: TextureHandle, used: u64 },
    Failed(Instant),
}

/// 取りに行くタイル。後から頼んだもの（今見ている所）から取る。
#[derive(Default)]
struct Queue {
    wanted: Vec<TileId>,
    stop: bool,
}

pub struct Tiles {
    template: String,
    entries: HashMap<TileId, Entry>,
    frame: u64,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    results: Receiver<(TileId, Result<ColorImage, String>)>,
    /// 最後に出た取得の失敗。地図の隅に出す。
    pub problem: Option<String>,
}

impl Tiles {
    /// `cache_root` は取ったタイルを残すフォルダ（設定フォルダの `map-tiles`）。
    pub fn new(template: &str, cache_root: Option<&Path>, ctx: &egui::Context) -> Self {
        let registry = tile_registry();
        let queue: Arc<(Mutex<Queue>, Condvar)> = Default::default();
        let (tx, results) = channel();
        let dir = cache_root.map(|r| cache_dir(r, template));
        for _ in 0..WORKERS {
            let worker = Worker {
                template: template.to_owned(),
                dir: dir.clone(),
                registry: registry.clone(),
                queue: queue.clone(),
                tx: tx.clone(),
                ctx: ctx.clone(),
            };
            std::thread::spawn(move || worker.run());
        }
        Self {
            template: template.to_owned(),
            entries: HashMap::new(),
            frame: 0,
            queue,
            results,
            problem: None,
        }
    }

    pub fn template(&self) -> &str {
        &self.template
    }

    /// 取れたタイルをテクスチャにする。描く前に毎フレーム呼ぶ。
    pub fn receive(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        while let Ok((id, result)) = self.results.try_recv() {
            let entry = match result {
                Ok(image) => {
                    self.problem = None;
                    let name = format!("map_tile_{}_{}_{}", id.z, id.x, id.y);
                    Entry::Ready {
                        texture: ctx.load_texture(name, image, TextureOptions::LINEAR),
                        used: self.frame,
                    }
                }
                Err(e) => {
                    self.problem = Some(e);
                    Entry::Failed(Instant::now())
                }
            };
            self.entries.insert(id, entry);
        }
        self.forget_old();
    }

    /// 持っていればそのテクスチャ。なければ取りに行く（このフレームで見えるタイルだけを頼む）。
    pub fn get(&mut self, id: TileId) -> Option<TextureHandle> {
        let frame = self.frame;
        match self.entries.get_mut(&id) {
            Some(Entry::Ready { texture, used }) => {
                *used = frame;
                return Some(texture.clone());
            }
            Some(Entry::Waiting) => return None,
            Some(Entry::Failed(at)) if at.elapsed() < RETRY => return None,
            _ => {}
        }
        self.entries.insert(id, Entry::Waiting);
        let (lock, wake) = &*self.queue;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .wanted
            .push(id);
        wake.notify_one();
        None
    }

    /// 持っているときだけ返す（取りには行かない）。読み込み中のタイルの代わりに、
    /// 広い範囲のタイルを引き伸ばして描くのに使う。
    pub fn cached(&mut self, id: TileId) -> Option<TextureHandle> {
        let frame = self.frame;
        match self.entries.get_mut(&id) {
            Some(Entry::Ready { texture, used }) => {
                *used = frame;
                Some(texture.clone())
            }
            _ => None,
        }
    }

    /// 画面から外れて、まだ取りに行っていないタイルを頼むのをやめる。地図を大きく動かしたとき、
    /// 通り過ぎた場所のタイルまで取らないため。
    pub fn keep_only(&mut self, visible: &[TileId]) {
        let (lock, _) = &*self.queue;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        let dropped: Vec<TileId> = q
            .wanted
            .iter()
            .filter(|id| !visible.contains(id))
            .copied()
            .collect();
        q.wanted.retain(|id| visible.contains(id));
        drop(q);
        for id in dropped {
            if matches!(self.entries.get(&id), Some(Entry::Waiting)) {
                self.entries.remove(&id);
            }
        }
    }

    pub fn loading(&self) -> bool {
        self.entries.values().any(|e| matches!(e, Entry::Waiting))
    }

    fn forget_old(&mut self) {
        let ready = self
            .entries
            .values()
            .filter(|e| matches!(e, Entry::Ready { .. }))
            .count();
        if ready <= KEEP_TEXTURES {
            return;
        }
        let mut used: Vec<(u64, TileId)> = self
            .entries
            .iter()
            .filter_map(|(id, e)| match e {
                Entry::Ready { used, .. } => Some((*used, *id)),
                _ => None,
            })
            .collect();
        used.sort();
        for (_, id) in used.into_iter().take(ready - KEEP_TEXTURES) {
            self.entries.remove(&id);
        }
    }
}

impl Drop for Tiles {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        wake.notify_all();
    }
}

struct Worker {
    template: String,
    dir: Option<PathBuf>,
    registry: Registry,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    tx: Sender<(TileId, Result<ColorImage, String>)>,
    ctx: egui::Context,
}

impl Worker {
    fn run(self) {
        let agent = agent();
        loop {
            let id = {
                let (lock, wake) = &*self.queue;
                let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if q.stop {
                        return;
                    }
                    if let Some(id) = q.wanted.pop() {
                        break id;
                    }
                    q = wake.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            let result = self.bytes(&agent, id).and_then(|b| self.decode(&b));
            if self.tx.send((id, result)).is_err() {
                return;
            }
            self.ctx.request_repaint();
        }
    }

    fn bytes(&self, agent: &ureq::Agent, id: TileId) -> Result<Vec<u8>, String> {
        let path = self.dir.as_ref().map(|d| cache_path(d, id));
        let stored = path.as_ref().and_then(|p| {
            let age = std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| SystemTime::now().duration_since(t).ok())?;
            Some((std::fs::read(p).ok()?, age))
        });
        if let Some((bytes, age)) = &stored
            && *age < FRESH
        {
            return Ok(bytes.clone());
        }
        match fetch(agent, &tile_url(&self.template, id)) {
            Ok(bytes) => {
                if let Some(p) = &path {
                    // 残せなくても地図は描ける。次に同じ所を見たとき取り直すだけなので、知らせない。
                    let _ = p
                        .parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| std::fs::write(p, &bytes));
                }
                Ok(bytes)
            }
            // 古くても、つながらないときは残っているものを出す。
            Err(e) => stored.map(|(b, _)| b).ok_or(e),
        }
    }

    fn decode(&self, bytes: &[u8]) -> Result<ColorImage, String> {
        let (image, _) = self
            .registry
            .decode_bytes(Path::new("tile"), bytes)
            .map_err(|e| e.to_string())?;
        let d = display::to_display(&image, &DisplayParams::default());
        Ok(ColorImage::from_rgba_unmultiplied(
            [d.width as usize, d.height as usize],
            &d.rgba,
        ))
    }
}

/// タイルを読むデコーダ。地図のタイルは PNG・JPEG・WebP のどれかなので、それだけにする。
/// 画像を開くのと同じすべての形式のデコーダを通さないのは、タイルの URL は `http://` も
/// 許していて、途中で差し替えられた中身を受け取りうるため。受け付ける形式は少ないほうがよい。
fn tile_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(vw_png::PngPlugin));
    registry.register(Arc::new(vw_jpeg::JpegPlugin));
    registry.register(Arc::new(vw_webp::WebpPlugin::default()));
    registry
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .user_agent(user_agent())
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(true)
        .build()
        .new_agent()
}

/// タイルより大きいものは受け取らない。設定の URL が地図でないものを指していても、
/// メモリを使い切らないため。
const LARGEST_TILE: u64 = 4 << 20;

fn fetch(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    let response = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    response
        .into_body()
        .with_config()
        .limit(LARGEST_TILE)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_template_is_filled_with_the_tile_number() {
        let id = TileId { z: 5, x: 28, y: 12 };
        assert_eq!(
            tile_url("https://tile.openstreetmap.org/{z}/{x}/{y}.png", id),
            "https://tile.openstreetmap.org/5/28/12.png"
        );
    }

    #[test]
    fn template_needs_a_web_address_and_all_three_numbers() {
        assert!(valid_template("https://a.example/{z}/{x}/{y}.png"));
        assert!(!valid_template("https://a.example/{z}/{x}.png"));
        assert!(!valid_template("file:///{z}/{x}/{y}.png"));
    }

    #[test]
    fn each_provider_gets_its_own_cache_folder() {
        let root = Path::new("/c");
        let a = cache_dir(root, "https://a.example/{z}/{x}/{y}.png");
        let b = cache_dir(root, "https://b.example/{z}/{x}/{y}.png");
        assert_ne!(a, b);
        assert_eq!(a, cache_dir(root, "https://a.example/{z}/{x}/{y}.png"));
    }

    #[test]
    fn user_agent_names_vw_and_its_repository() {
        let ua = user_agent();
        assert!(ua.starts_with("vw/") && ua.contains("github.com/xex-ken-xex/vw"));
    }

    #[test]
    fn stored_tile_is_used_without_the_network_while_fresh() {
        let dir = crate::test_util::TempDir::new();
        let id = TileId { z: 1, x: 0, y: 1 };
        let png = crate::test_util::png_bytes(4, 4);
        let path = cache_path(dir.path(), id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &png).unwrap();
        let (tx, _rx) = channel();
        let worker = Worker {
            // つながらない宛先にして、ディスクから読んだことを確かめる。
            template: "http://127.0.0.1:9/{z}/{x}/{y}.png".into(),
            dir: Some(dir.path().to_path_buf()),
            registry: crate::test_util::png_registry(),
            queue: Default::default(),
            tx,
            ctx: egui::Context::default(),
        };
        let bytes = worker.bytes(&agent(), id).unwrap();
        assert_eq!(worker.decode(&bytes).unwrap().size, [4, 4]);
    }

    #[test]
    fn tiles_are_read_only_as_png_jpeg_or_webp() {
        let registry = tile_registry();
        assert!(
            registry
                .decode_bytes(Path::new("tile"), &crate::test_util::png_bytes(2, 2))
                .is_ok()
        );
        // 中身で形式が分かる DICOM でも、タイルとしては読まない。
        let dicom = vw_dicom::synth::Synth::ct(1, 1).bytes();
        assert!(registry.decode_bytes(Path::new("tile"), &dicom).is_err());
    }
}
