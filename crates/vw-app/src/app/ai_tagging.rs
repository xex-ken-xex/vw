//! AI タグ付け：選んだ画像やフォルダの画像に、裏のスレッドでタグを付けて
//! `.vw-ai-tags.json` に書く。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use eframe::egui;
use vw_core::Registry;

use super::VwApp;
use crate::ai_models;
use crate::ai_tags::Entry;
use crate::i18n::tr;
use crate::tagger::{self, Thresholds};

/// 読み込む画像の長辺。モデルの入力（448）より少し大きく、縮小の平均が効く程度に。
const LOAD_SIDE: u32 = 896;

pub(super) enum Event {
    Tagged(PathBuf, Entry),
    Failed(PathBuf, String),
    Done,
}

pub(super) struct Job {
    total: usize,
    done: Arc<AtomicUsize>,
    rx: Receiver<Event>,
}

impl Job {
    pub(super) fn progress(&self) -> (usize, usize) {
        (self.done.load(Ordering::Relaxed), self.total)
    }
}

struct Paths {
    runtime: PathBuf,
    model: PathBuf,
    tags: PathBuf,
}

fn run(
    ctx: egui::Context,
    registry: Registry,
    paths: Paths,
    files: Vec<PathBuf>,
    thresholds: Thresholds,
    threads: usize,
    done: Arc<AtomicUsize>,
) -> Receiver<Event> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut guard = tagger::shared().lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            match tagger::Tagger::load(&paths.runtime, &paths.model, &paths.tags, threads) {
                Ok(t) => *guard = Some(t),
                Err(e) => {
                    for f in files {
                        let _ = tx.send(Event::Failed(f, e.clone()));
                    }
                    let _ = tx.send(Event::Done);
                    ctx.request_repaint();
                    return;
                }
            }
        }
        let tagger = guard.as_mut().expect("loaded above");
        for file in files {
            let result = crate::loader::load_fit(&registry, &file, false, true, Some(LOAD_SIDE))
                .and_then(|loaded| {
                    let full = loaded.full.ok_or("no image")?;
                    tagger.tag(&full.display, thresholds)
                });
            let event = match result {
                Ok(tags) => Event::Tagged(
                    file,
                    Entry {
                        model: ai_models::TAGGER_NAME.to_owned(),
                        tags,
                    },
                ),
                Err(e) => Event::Failed(file, e),
            };
            done.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(event);
            ctx.request_repaint();
        }
        let _ = tx.send(Event::Done);
        ctx.request_repaint();
    });
    rx
}

impl VwApp {
    /// 画像にタグを付ける。モデルが揃っていなければ「AI のモデル」を開いて知らせる。
    pub(super) fn ai_tag_paths(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        let lang = self.lang();
        let mut files = Vec::new();
        for p in paths {
            let file = crate::frames::file_of(&p).to_path_buf();
            if let Some(mount) = self.archives.mount_of(&file) {
                self.log.warn(format!(
                    "AI tags cannot be put on images inside {}",
                    mount.zip.display()
                ));
                continue;
            }
            if !files.contains(&file) {
                files.push(file);
            }
        }
        if files.is_empty() {
            return;
        }
        let Some(store) = self.ai_store.clone() else {
            self.log.error(tr(
                lang,
                "The settings folder is unknown, so models cannot be stored.",
            ));
            return;
        };
        let missing = store.missing();
        if !missing.is_empty() {
            let names: Vec<_> = missing.iter().map(|i| i.title()).collect();
            self.log.error(format!(
                "AI tags need {} (Help > AI models...)",
                names.join(" and ")
            ));
            self.ai_models.open = true;
            return;
        }
        if self.ai_job.is_some() {
            self.ai_queue.extend(files);
            return;
        }
        self.start_ai_job(ctx, store, files);
    }

    fn start_ai_job(&mut self, ctx: &egui::Context, store: ai_models::Store, files: Vec<PathBuf>) {
        let ai = &self.settings().ai;
        let (thresholds, threads) = (ai.thresholds(), ai.threads);
        let done = Arc::new(AtomicUsize::new(0));
        self.log
            .info(format!("AI tagging {} image(s)", files.len()));
        let rx = run(
            ctx.clone(),
            self.loader.registry().clone(),
            Paths {
                runtime: store.runtime(),
                model: store.tagger_model(),
                tags: store.tagger_tags(),
            },
            files.clone(),
            thresholds,
            threads,
            done.clone(),
        );
        self.ai_job = Some(Job {
            total: files.len(),
            done,
            rx,
        });
    }

    pub(super) fn ai_tag_folder(&mut self, ctx: &egui::Context, dir: &Path) {
        let registry = self.loader.registry().clone();
        let images = crate::tree::list_folder(dir, &registry, self.settings().tree.order()).images;
        if images.is_empty() {
            self.log.warn(format!("No images in {}", dir.display()));
            return;
        }
        self.ai_tag_paths(ctx, images);
    }

    pub(super) fn ai_tag_current_folder(&mut self, ctx: &egui::Context) {
        match self.current.clone() {
            Some(dir) => self.ai_tag_folder(ctx, &dir),
            None => self.log.warn("No folder is open"),
        }
    }

    /// 毎フレーム：タグ付けの結果を受け取り、ダウンロードの結果をログに出し、ウインドウを描く。
    pub(super) fn ai_ui(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        if let Some(job) = &self.ai_job {
            let mut finished = false;
            let events: Vec<Event> = job.rx.try_iter().collect();
            for event in events {
                match event {
                    Event::Tagged(path, entry) => {
                        let n = entry.tags.general.len() + entry.tags.characters.len();
                        if let Err(e) = self.ai_tags.set(&path, entry) {
                            self.log.error(format!("Cannot save AI tags: {e}"));
                        } else {
                            self.log.info(format!("{} tag(s): {}", n, path.display()));
                        }
                    }
                    Event::Failed(path, e) => {
                        self.log
                            .error(format!("AI tags failed for {}: {e}", path.display()));
                    }
                    Event::Done => finished = true,
                }
            }
            if finished {
                self.ai_job = None;
                if !self.ai_queue.is_empty() {
                    if let Some(store) = self.ai_store.clone() {
                        let files = std::mem::take(&mut self.ai_queue);
                        self.start_ai_job(ctx, store, files);
                    }
                }
            }
        }
        for (item, result) in self.ai_models.finished() {
            match result {
                Ok(()) => self.log.info(format!("Downloaded {}", item.title())),
                Err(e) => self
                    .log
                    .error(format!("Download of {} failed: {e}", item.title())),
            }
        }
        for p in self.ai_tags.take_problems() {
            self.log.error(p);
        }
        self.ai_models.ui(ctx, lang, self.ai_store.as_ref());
    }

    /// メタデータ欄の「AI tags」の行。タグがなければ `None`。
    pub(super) fn ai_tag_rows(&self, path: &Path) -> Option<Vec<(String, String)>> {
        let lang = self.lang();
        let file = crate::frames::file_of(path);
        let entry = self.ai_tags.get(&file)?;
        let mut rows = Vec::new();
        if let Some(r) = &entry.tags.rating {
            rows.push((
                tr(lang, "Rating").to_owned(),
                format!("{} ({:.2})", r.name, r.score),
            ));
        }
        for c in &entry.tags.characters {
            rows.push((
                tr(lang, "Character").to_owned(),
                format!("{} ({:.2})", c.name, c.score),
            ));
        }
        for g in &entry.tags.general {
            rows.push((g.name.clone(), format!("{:.2}", g.score)));
        }
        rows.push((tr(lang, "AI model").to_owned(), entry.model.clone()));
        Some(rows)
    }
}
