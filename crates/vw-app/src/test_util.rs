//! テスト用の補助。一時フォルダとテスト画像の作成。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use vw_core::Registry;

/// PNG プラグインだけを登録したレジストリ。
pub fn png_registry() -> Registry {
    let mut r = Registry::new();
    r.register(Arc::new(vw_png::PngPlugin));
    r
}

/// アプリと同じく、組み込みのプラグインをすべて登録したもの。
pub fn all_registry() -> Registry {
    let mut r = png_registry();
    r.register(Arc::new(vw_jpeg::JpegPlugin));
    r.register(Arc::new(vw_bmp::BmpPlugin));
    r.register(Arc::new(vw_webp::WebpPlugin::default()));
    r.register(Arc::new(vw_raw::RawPlugin));
    r.register(Arc::new(vw_dicom::DicomPlugin));
    r
}

/// 8bit RGB の PNG を書き出す。
pub fn write_png(path: &Path, w: u32, h: u32, rgb: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(rgb).unwrap();
}

/// テストごとの一時フォルダ。破棄時に中身ごと消す。
/// tempfile クレートを足さないのは、この程度なら std だけで書けるため。
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("vw-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn mkdir(&self, name: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    pub fn touch(&self, name: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, b"").unwrap();
        p
    }

    /// 更新日時を UNIX 時刻の `secs` 秒にした空のファイルを作る。
    pub fn touch_at(&self, name: &str, secs: u64) -> PathBuf {
        let p = self.touch(name);
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(time)
            .unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
