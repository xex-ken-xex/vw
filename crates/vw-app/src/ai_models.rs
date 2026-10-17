//! AI 機能が使うファイル（ONNX Runtime とモデル）の置き場とダウンロード。
//!
//! 配布物に同梱しないのは、合わせて 400 MB を超え、AI タグを使わない人の
//! ダウンロードまで重くなるため。置き場は設定ファイルの隣の `models` フォルダに
//! 固定し、設定にしないのは、モデルを他へ動かす理由がまずなく、「どこに入れたか」を
//! 迷わせたくないため。
//!
//! 取ってくるファイルは版（コミット・リリース）で固定し、決めておいたハッシュと合うものだけを使う。
//! 取得元が乗っ取られたり差し替えられたりしても、知らないモデルや dll を読み込まないため。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui;

use crate::i18n::{tr, tr1};
use crate::settings::Language;

pub const RUNTIME_VERSION: &str = "1.29.0";
pub const TAGGER_NAME: &str = "wd-vit-tagger-v3";
/// Hugging Face の SmilingWolf/wd-vit-tagger-v3 のコミット。
const TAGGER_REVISION: &str = "7f6b584d0bd3f55c4531f14ba3d4761b2bccdc0f";
/// opencv/opencv_zoo のコミット。
const FACES_REVISION: &str = "47534e27c9851bb1128ccc0102f1145e27f23f98";
/// モデルが揃う前に、タグ付けや顔探しの項目に出す説明（英語の原文）。
pub const NOT_READY: &str = "Download all AI models first (AI tags > AI models...)";

/// ダウンロードするもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Item {
    Runtime,
    Tagger,
    Faces,
}

impl Item {
    pub const ALL: [Item; 3] = [Item::Runtime, Item::Tagger, Item::Faces];

    pub fn title(self) -> &'static str {
        match self {
            Item::Runtime => "ONNX Runtime",
            Item::Tagger => "WD tagger v3 (ViT)",
            Item::Faces => "YuNet + SFace (faces)",
        }
    }

    pub fn license(self) -> &'static str {
        match self {
            Item::Runtime => "MIT",
            Item::Tagger => "Apache-2.0",
            Item::Faces => "MIT / Apache-2.0",
        }
    }

    /// おおよその大きさ。表示だけに使う。
    pub fn size_text(self) -> &'static str {
        match self {
            Item::Runtime => "~60 MB",
            Item::Tagger => "~380 MB",
            Item::Faces => "~39 MB",
        }
    }

    pub fn page(self) -> &'static str {
        match self {
            Item::Runtime => "https://github.com/microsoft/onnxruntime/releases",
            Item::Tagger => "https://huggingface.co/SmilingWolf/wd-vit-tagger-v3",
            Item::Faces => "https://github.com/opencv/opencv_zoo",
        }
    }

    /// 取ってくるファイルと、置き場に入れる相対パス（`None` は書庫から取り出す）。
    fn sources(self) -> Vec<Source> {
        match self {
            Item::Runtime => vec![Source {
                url: runtime_url(),
                target: runtime_relative(),
                extract: Some(runtime_entry()),
                hash: Hash::Sha256(runtime_sha256()),
            }],
            Item::Tagger => [
                (
                    "model.onnx",
                    Hash::Sha256(
                        "35f23693620b668f4d53fd3c62bf65e40af739bc52c7eb0fbc49258b58d065b6",
                    ),
                ),
                // Hugging Face が SHA-256 を公開しているのは Git LFS に置いた大きなファイルだけで、
                // この CSV は普通の git のファイルなので、公開されている git の blob の値で確かめる。
                // 自分で SHA-256 を計算して書く案は、ここからは Hugging Face に届かず元のファイルを
                // 取れないため採らなかった。中身はタグの名前だけで、実行されるものではない。
                (
                    "selected_tags.csv",
                    Hash::GitBlob("a2a653a5c89c8e387765eb1de9cfd2aedc92ee8d"),
                ),
            ]
            .into_iter()
            .map(|(f, hash)| Source {
                url: format!(
                    "https://huggingface.co/SmilingWolf/{TAGGER_NAME}/resolve/{TAGGER_REVISION}/{f}"
                ),
                target: PathBuf::from(TAGGER_NAME).join(f),
                extract: None,
                hash,
            })
            .collect(),
            // github.com の raw は Git LFS の中身へ転送されるが、転送先を直接指す。
            // 転送を挟むと、プロキシのある環境で弾かれることがあるため。
            Item::Faces => [
                (
                    FACE_DETECTOR,
                    "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
                ),
                (
                    FACE_RECOGNIZER,
                    "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
                ),
            ]
            .into_iter()
            .map(|((dir, f), sha256)| Source {
                url: format!(
                    "https://media.githubusercontent.com/media/opencv/opencv_zoo/{FACES_REVISION}/models/{dir}/{f}"
                ),
                target: PathBuf::from(FACES_DIR).join(f),
                extract: None,
                hash: Hash::Sha256(sha256),
            })
            .collect(),
        }
    }
}

const FACES_DIR: &str = "faces";
const FACE_DETECTOR: (&str, &str) = ("face_detection_yunet", "face_detection_yunet_2023mar.onnx");
const FACE_RECOGNIZER: (&str, &str) = (
    "face_recognition_sface",
    "face_recognition_sface_2021dec.onnx",
);

struct Source {
    url: String,
    target: PathBuf,
    extract: Option<&'static str>,
    /// 取ってきたファイル（書庫なら書庫そのもの）のハッシュ。
    hash: Hash,
}

/// 確かめるハッシュ。16 進の小文字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hash {
    Sha256(&'static str),
    /// git の blob の id（`blob <長さ>\0` に続けて中身の SHA-1）。
    GitBlob(&'static str),
}

impl Hash {
    fn expected(self) -> &'static str {
        match self {
            Hash::Sha256(h) | Hash::GitBlob(h) => h,
        }
    }

    fn of_file(self, path: &Path) -> Result<String, String> {
        use sha2::Digest;
        let read = |path: &Path, update: &mut dyn FnMut(&[u8])| -> Result<(), String> {
            let mut file =
                std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut buf = vec![0u8; 1 << 16];
            loop {
                let n = file
                    .read(&mut buf)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                if n == 0 {
                    return Ok(());
                }
                update(&buf[..n]);
            }
        };
        let bytes = match self {
            Hash::Sha256(_) => {
                let mut h = sha2::Sha256::new();
                read(path, &mut |b| h.update(b))?;
                h.finalize().to_vec()
            }
            Hash::GitBlob(_) => {
                let len = std::fs::metadata(path)
                    .map_err(|e| format!("{}: {e}", path.display()))?
                    .len();
                let mut h = sha1::Sha1::new();
                h.update(format!("blob {len}\0").as_bytes());
                read(path, &mut |b| h.update(b))?;
                h.finalize().to_vec()
            }
        };
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// `path` が決めたハッシュと合うか。合わなければ理由を返す。
    fn check(self, path: &Path, url: &str) -> Result<(), String> {
        let actual = self.of_file(path)?;
        if actual == self.expected() {
            Ok(())
        } else {
            Err(format!(
                "{url}: the file does not match the expected hash and was not used (expected {}, got {actual})",
                self.expected()
            ))
        }
    }
}

/// 確かめ済みの印。中身は確かめたハッシュ。ハッシュを変えた版に上げると印が合わなくなり、
/// 取り直す（同じ中身なら確かめ直すだけ）ことになる。
fn stamp_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".verified");
    PathBuf::from(name)
}

fn runtime_url() -> String {
    let base = "https://github.com/microsoft/onnxruntime/releases/download";
    let (os, ext) = if cfg!(windows) {
        ("win", "zip")
    } else {
        ("linux", "tgz")
    };
    let arch = if cfg!(target_arch = "aarch64") {
        if cfg!(windows) {
            "arm64"
        } else {
            "aarch64"
        }
    } else {
        "x64"
    };
    format!("{base}/v{RUNTIME_VERSION}/onnxruntime-{os}-{arch}-{RUNTIME_VERSION}.{ext}")
}

/// ONNX Runtime の書庫の SHA-256。GitHub の Releases の v1.29.0 から取って計算した値。
fn runtime_sha256() -> &'static str {
    match (cfg!(windows), cfg!(target_arch = "aarch64")) {
        (true, false) => "c9b4b7086b529ad814f428c1bad028e20a25d7dc0699836775faace4ab5b78b2",
        (true, true) => "a094a49c3ced0f9fca554647cc7566ae99d93a63a8ce6bf47975561c2de7608e",
        (false, false) => "c3fddc4f139a045b0c4902c57410f0694f1c2fdf9b6939fbe38b1aeae7cd14ba",
        (false, true) => "e1799098ebc054b370f6176a450f158720f297818c613e5dc99b92e2ec82346f",
    }
}

/// 書庫の中の共有ライブラリ。先頭フォルダ名は書庫によるので、末尾で探す。
fn runtime_entry() -> &'static str {
    if cfg!(windows) {
        "lib/onnxruntime.dll"
    } else {
        "lib/libonnxruntime.so"
    }
}

fn runtime_relative() -> PathBuf {
    PathBuf::from("onnxruntime").join(if cfg!(windows) {
        "onnxruntime.dll"
    } else {
        "libonnxruntime.so"
    })
}

/// 置き場。
#[derive(Debug, Clone)]
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(settings_path: Option<&Path>) -> Option<Self> {
        Some(Self {
            dir: settings_path?.parent()?.join("models"),
        })
    }

    pub fn runtime(&self) -> PathBuf {
        self.dir.join(runtime_relative())
    }

    pub fn tagger_model(&self) -> PathBuf {
        self.dir.join(TAGGER_NAME).join("model.onnx")
    }

    pub fn tagger_tags(&self) -> PathBuf {
        self.dir.join(TAGGER_NAME).join("selected_tags.csv")
    }

    pub fn face_detector(&self) -> PathBuf {
        self.dir.join(FACES_DIR).join(FACE_DETECTOR.1)
    }

    pub fn face_recognizer(&self) -> PathBuf {
        self.dir.join(FACES_DIR).join(FACE_RECOGNIZER.1)
    }

    /// 揃っていて、確かめ済みか。起動のたびに数百 MB のハッシュを計算し直さないよう、
    /// 確かめたときに残した印で見る。
    pub fn has(&self, item: Item) -> bool {
        item.sources().iter().all(|s| {
            let target = self.dir.join(&s.target);
            // 空のファイルは数えない。以前の版は展開に失敗すると空の dll を残していた。
            std::fs::metadata(&target).is_ok_and(|m| m.is_file() && m.len() > 0)
                && std::fs::read_to_string(stamp_path(&target))
                    .is_ok_and(|h| h.trim() == s.hash.expected())
        })
    }
}

/// 進み具合。バイト数で持つ（合計が分からないうちは 0）。
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
}

impl Progress {
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| self.done.load(Ordering::Relaxed) as f32 / total as f32)
    }
}

pub struct Download {
    pub progress: Arc<Progress>,
    rx: Receiver<Result<(), String>>,
}

impl Download {
    pub fn start(ctx: &egui::Context, store: &Store, item: Item) -> Self {
        let progress = Arc::new(Progress::default());
        let p = progress.clone();
        let dir = store.dir.clone();
        let rx = crate::background::compute(ctx, move || fetch_all(&dir, item, &p));
        Self { progress, rx }
    }

    pub fn poll(&self) -> Option<Result<(), String>> {
        self.rx.try_recv().ok()
    }
}

fn fetch_all(dir: &Path, item: Item, progress: &Progress) -> Result<(), String> {
    let sources: Vec<Source> = item
        .sources()
        .into_iter()
        .filter(|s| !already_verified(dir, s))
        .collect();
    let agent = agent();
    // 合計を先に知る。進み具合を 1 本の棒で出すため。
    let mut sizes = Vec::new();
    for s in &sources {
        let size = content_length(&agent, &s.url)?;
        sizes.push(size);
    }
    progress.total.store(sizes.iter().sum(), Ordering::Relaxed);
    for s in &sources {
        let target = dir.join(&s.target);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let part = target.with_extension("part");
        fetch(&agent, &s.url, &part, progress)?;
        let result = s.hash.check(&part, &s.url).and_then(|()| match s.extract {
            Some(entry) => extract(&part, entry, &target).and_then(|()| {
                std::fs::remove_file(&part).map_err(|e| format!("{}: {e}", part.display()))
            }),
            None => {
                std::fs::rename(&part, &target).map_err(|e| format!("{}: {e}", target.display()))
            }
        });
        if result.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        result?;
        stamp(&target, s.hash)?;
    }
    Ok(())
}

/// 前の版で取ったファイルが決めたハッシュと合えば、印を付けて取り直さない。
/// 書庫から出した dll は書庫のハッシュで確かめられないので、取り直す。
fn already_verified(dir: &Path, s: &Source) -> bool {
    let target = dir.join(&s.target);
    if s.extract.is_some() || !target.is_file() {
        return false;
    }
    s.hash.check(&target, &s.url).is_ok() && stamp(&target, s.hash).is_ok()
}

fn stamp(target: &Path, hash: Hash) -> Result<(), String> {
    let path = stamp_path(target);
    std::fs::write(&path, hash.expected()).map_err(|e| format!("{}: {e}", path.display()))
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    // OS の証明書を使うのは、社内プロキシが証明書を差し替える環境でも動かすため。
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .http_status_as_error(true)
        .build()
        .new_agent()
}

fn content_length(agent: &ureq::Agent, url: &str) -> Result<u64, String> {
    let response = agent.head(url).call().map_err(|e| format!("{url}: {e}"))?;
    Ok(response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

fn fetch(agent: &ureq::Agent, url: &str, to: &Path, progress: &Progress) -> Result<(), String> {
    let response = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let mut body = response.into_body().into_reader();
    let mut file = std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("{url}: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("{}: {e}", to.display()))?;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.flush().map_err(|e| format!("{}: {e}", to.display()))
}

fn is_zip(archive: &Path) -> Result<bool, String> {
    let mut magic = [0u8; 2];
    std::fs::File::open(archive)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(|e| format!("{}: {e}", archive.display()))?;
    Ok(magic == *b"PK")
}

/// 書庫から `entry` で終わる 1 つのファイルを `target` に出す。
fn extract(archive: &Path, entry: &str, target: &Path) -> Result<(), String> {
    // 別名に書いてから置き換える。途中で失敗したとき、壊れた target を残さないため。
    let partial = target.with_extension("extracting");
    let result = std::fs::File::create(&partial)
        .map_err(|e| format!("{}: {e}", partial.display()))
        .and_then(|mut out| extract_into(archive, entry, &mut out))
        .and_then(|()| {
            std::fs::rename(&partial, target).map_err(|e| format!("{}: {e}", target.display()))
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

fn extract_into(archive: &Path, entry: &str, out: &mut std::fs::File) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let matches = |name: &str| {
        let name = name.replace('\\', "/");
        // Linux の .so は版の番号つき（libonnxruntime.so.1.29.0）で入っている。
        name.ends_with(entry) || name.contains(&format!("{entry}."))
    };
    // 拡張子では見分けない。落としている途中のファイルは .part なので、zip でも tgz に見えてしまう。
    let found = if is_zip(archive)? {
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let index = (0..zip.len()).find(|&i| {
            zip.by_index(i)
                .is_ok_and(|f| matches(&String::from_utf8_lossy(f.name_raw())))
        });
        match index {
            Some(i) => {
                let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
                std::io::copy(&mut f, out).map_err(|e| e.to_string())?;
                true
            }
            None => false,
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        let mut found = false;
        for e in tar.entries().map_err(|e| e.to_string())? {
            let mut e = e.map_err(|e| e.to_string())?;
            let path = e
                .path()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned();
            if e.header().entry_type().is_file() && matches(&path) {
                std::io::copy(&mut e, out).map_err(|e| e.to_string())?;
                found = true;
                break;
            }
        }
        found
    };
    if !found {
        return Err(format!(
            "{}: {entry} not found in the archive",
            archive.display()
        ));
    }
    Ok(())
}

/// 「AI のモデル」ウインドウ。
#[derive(Default)]
pub struct Dialog {
    pub open: bool,
    downloads: Vec<(Item, Download)>,
    status: Vec<(Item, Result<(), String>)>,
}

impl Dialog {
    /// 進み具合（活動ラインに出す）。合計バイトに対する済みバイト。
    pub fn progress(&self) -> Option<(usize, usize)> {
        if self.downloads.is_empty() {
            return None;
        }
        let (done, total) = self.downloads.iter().fold((0, 0), |(d, t), (_, dl)| {
            (
                d + dl.progress.done.load(Ordering::Relaxed),
                t + dl.progress.total.load(Ordering::Relaxed),
            )
        });
        // 1 MB 単位。活動ラインは usize で受けるため。
        Some(((done >> 20) as usize, (total >> 20).max(1) as usize))
    }

    /// 終わったダウンロードの結果（ログに出す）。
    pub fn finished(&mut self) -> Vec<(Item, Result<(), String>)> {
        let mut done = Vec::new();
        self.downloads.retain(|(item, dl)| match dl.poll() {
            Some(result) => {
                done.push((*item, result));
                false
            }
            None => true,
        });
        for (item, result) in &done {
            self.status.retain(|(i, _)| i != item);
            self.status.push((*item, result.clone()));
        }
        done
    }

    pub fn ui(&mut self, ctx: &egui::Context, lang: Language, store: Option<&Store>) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        let mut start = None;
        egui::Window::new(tr(lang, "AI models"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let Some(store) = store else {
                    ui.label(tr(
                        lang,
                        "The settings folder is unknown, so models cannot be stored.",
                    ));
                    return;
                };
                ui.label(tr1(
                    lang,
                    "Models are stored in {}",
                    &store.dir.display().to_string(),
                ));
                ui.add_space(6.0);
                egui::Grid::new("ai_models")
                    .num_columns(5)
                    .spacing([12.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for item in Item::ALL {
                            ui.label(item.title());
                            ui.label(item.size_text());
                            ui.hyperlink_to(item.license(), item.page());
                            let state =
                                self.status.iter().find(|(i, _)| *i == item).map(|(_, r)| r);
                            if let Some((_, dl)) = self.downloads.iter().find(|(i, _)| *i == item) {
                                let f = dl.progress.fraction().unwrap_or(0.0);
                                ui.add(
                                    egui::ProgressBar::new(f)
                                        .desired_width(120.0)
                                        .show_percentage(),
                                );
                                ui.label("");
                            } else {
                                match state {
                                    Some(Err(e)) => {
                                        ui.colored_label(
                                            ui.visuals().error_fg_color,
                                            tr(lang, "Failed"),
                                        )
                                        .on_hover_text(e);
                                    }
                                    _ if store.has(item) => {
                                        ui.label(tr(lang, "Downloaded"));
                                    }
                                    _ => {
                                        ui.label(tr(lang, "Not downloaded"));
                                    }
                                }
                                let text = if store.has(item) {
                                    tr(lang, "Download again")
                                } else {
                                    tr(lang, "Download")
                                };
                                if ui.button(text).clicked() {
                                    start = Some(item);
                                }
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(tr(lang, "Open folder")).clicked() {
                        let _ = std::fs::create_dir_all(&store.dir);
                        let _ = crate::launch::open_folder(&store.dir);
                    }
                    ui.label(tr(
                        lang,
                        "AI tags need ONNX Runtime and WD tagger; faces need ONNX Runtime and YuNet + SFace. Models run on the CPU.",
                    ));
                });
            });
        if let (Some(item), Some(store)) = (start, store) {
            self.status.retain(|(i, _)| *i != item);
            self.downloads
                .push((item, Download::start(ctx, store, item)));
        }
        self.open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_knows_what_is_missing() {
        let dir = std::env::temp_dir().join(format!("vw-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = dir.join("settings.toml");
        let store = Store::new(Some(&settings)).unwrap();
        assert_eq!(store.dir, dir.join("models"));
        assert!(Item::ALL.iter().all(|&i| !store.has(i)));
        std::fs::create_dir_all(store.runtime().parent().unwrap()).unwrap();
        std::fs::write(store.runtime(), b"").unwrap();
        assert!(
            !store.has(Item::Runtime),
            "an empty file is a failed download"
        );
        std::fs::write(store.runtime(), b"dll").unwrap();
        assert!(!store.has(Item::Runtime), "a file nobody verified");
        stamp(&store.runtime(), Hash::Sha256(runtime_sha256())).unwrap();
        assert!(store.has(Item::Runtime));
        assert!(!store.has(Item::Tagger) && !store.has(Item::Faces));
        std::fs::write(stamp_path(&store.runtime()), "0123").unwrap();
        assert!(
            !store.has(Item::Runtime),
            "verified against a hash this version does not expect"
        );
    }

    #[test]
    fn checks_sha256_and_git_blob_hashes() {
        let dir = std::env::temp_dir().join(format!("vw-hash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.txt");
        std::fs::write(&file, b"hello\n").unwrap();
        // `sha256sum` と `git hash-object` の値。
        let sha256 =
            Hash::Sha256("5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03");
        let blob = Hash::GitBlob("ce013625030ba8dba906f756967f9e9ca394464a");
        assert!(sha256.check(&file, "u").is_ok());
        assert!(blob.check(&file, "u").is_ok());
        std::fs::write(&file, b"hellO\n").unwrap();
        let error = sha256.check(&file, "https://example.com/f").unwrap_err();
        assert!(error.contains("https://example.com/f"), "{error}");
        assert!(blob.check(&file, "u").is_err());
    }

    #[test]
    fn a_file_from_an_earlier_version_is_kept_only_if_it_matches() {
        let dir = std::env::temp_dir().join(format!("vw-earlier-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hello.txt"), b"hello\n").unwrap();
        let source = |hash| Source {
            url: "u".into(),
            target: "hello.txt".into(),
            extract: None,
            hash,
        };
        let good = source(Hash::GitBlob("ce013625030ba8dba906f756967f9e9ca394464a"));
        assert!(already_verified(&dir, &good));
        assert!(stamp_path(&dir.join("hello.txt")).is_file());
        let bad = source(Hash::GitBlob("0000000000000000000000000000000000000000"));
        assert!(!already_verified(&dir, &bad));
    }

    /// 本物の ONNX Runtime を GitHub から取る。ネットワークが要るので普段は走らせない。
    /// `VW_TEST_MODELS=<dir> cargo test -- --ignored downloads_the_runtime`
    #[test]
    #[ignore]
    fn downloads_the_runtime_and_extracts_the_library() {
        let dir = PathBuf::from(std::env::var("VW_TEST_MODELS").expect("VW_TEST_MODELS"));
        let progress = Progress::default();
        fetch_all(&dir, Item::Runtime, &progress).unwrap();
        let store = Store { dir };
        assert!(store.has(Item::Runtime));
        assert!(std::fs::metadata(store.runtime()).unwrap().len() > 1 << 20);
        assert_eq!(
            progress.done.load(Ordering::Relaxed),
            progress.total.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn extracts_the_versioned_library_from_a_tgz() {
        let dir = std::env::temp_dir().join(format!("vw-extract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("rt.tgz");
        {
            let gz = flate2::write::GzEncoder::new(
                std::fs::File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(
                &mut header,
                "onnxruntime-linux-x64-1.29.0/lib/libonnxruntime.so.1.29.0",
                &b"abc"[..],
            )
            .unwrap();
            tar.finish().unwrap();
        }
        let target = dir.join("libonnxruntime.so");
        extract(&archive, "lib/libonnxruntime.so", &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"abc");
        assert!(extract(&archive, "lib/other.so", &dir.join("x")).is_err());
    }

    #[test]
    fn extracts_the_dll_from_a_zip_whatever_the_file_is_named() {
        let dir = std::env::temp_dir().join(format!("vw-extract-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("onnxruntime.part");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            zip.start_file(
                "onnxruntime-win-x64-1.29.0/lib/onnxruntime.dll",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"dll").unwrap();
            zip.finish().unwrap();
        }
        let target = dir.join("onnxruntime.dll");
        extract(&archive, "lib/onnxruntime.dll", &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"dll");
    }

    #[test]
    fn a_failed_extraction_leaves_no_file_behind() {
        let dir = std::env::temp_dir().join(format!("vw-extract-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("broken.part");
        std::fs::write(&archive, b"not an archive").unwrap();
        let target = dir.join("onnxruntime.dll");
        assert!(extract(&archive, "lib/onnxruntime.dll", &target).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }
}
