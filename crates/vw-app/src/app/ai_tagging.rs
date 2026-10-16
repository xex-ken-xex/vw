//! AI タグ付けと顔探し：選んだ画像やフォルダの画像を裏のスレッドで順に処理し、
//! `.vw-ai-tags.json`（タグ・顔の枠・人物名）と設定フォルダ（顔の特徴）に書く。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use eframe::egui;
use vw_core::Registry;

use super::VwApp;
use crate::ai_models::{self, Item};
use crate::faces::{self, FaceMark, Found};
use crate::i18n::tr;
use crate::people::Sample;
use crate::tagger::{self, Tags, Thresholds};

/// タグ付けで読み込む画像の長辺。モデルの入力（448）より少し大きく、縮小の平均が効く程度に。
const TAG_SIDE: u32 = 896;
/// 顔探しで読み込む画像の長辺。検出は 640 で足りるが、顔の切り出しは元の画素から取るので、
/// 集合写真の小さな顔もつぶれない大きさにする。
const FACE_SIDE: u32 = 2048;

/// 裏でする仕事の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Work {
    Tag,
    Faces,
}

impl Work {
    fn needs(self) -> [Item; 2] {
        match self {
            Work::Tag => [Item::Runtime, Item::Tagger],
            Work::Faces => [Item::Runtime, Item::Faces],
        }
    }
}

pub(super) enum Event {
    Tagged(PathBuf, Tags),
    Faces {
        path: PathBuf,
        found: Vec<Found>,
        size: [usize; 2],
    },
    Failed(PathBuf, String),
    Done,
}

pub(super) struct Job {
    work: Work,
    total: usize,
    done: Arc<AtomicUsize>,
    rx: Receiver<Event>,
}

impl Job {
    pub(super) fn progress(&self) -> (usize, usize) {
        (self.done.load(Ordering::Relaxed), self.total)
    }
}

struct Run {
    registry: Registry,
    store: ai_models::Store,
    files: Vec<PathBuf>,
    thresholds: Thresholds,
    threads: usize,
    done: Arc<AtomicUsize>,
}

fn run(ctx: egui::Context, work: Work, r: Run) -> Receiver<Event> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let send = |e: Event| {
            let _ = tx.send(e);
            ctx.request_repaint();
        };
        let fail_all = |e: String| {
            for f in &r.files {
                send(Event::Failed(f.clone(), e.clone()));
            }
            send(Event::Done);
        };
        let side = match work {
            Work::Tag => TAG_SIDE,
            Work::Faces => FACE_SIDE,
        };
        let load = |file: &Path| {
            crate::loader::load_fit(&r.registry, file, false, true, Some(side)).and_then(|l| {
                l.full
                    .map(|f| f.display)
                    .ok_or_else(|| "no image".to_owned())
            })
        };
        match work {
            Work::Tag => {
                let mut guard = tagger::shared().lock().unwrap_or_else(|e| e.into_inner());
                if guard.is_none() {
                    let s = &r.store;
                    match tagger::Tagger::load(
                        &s.runtime(),
                        &s.tagger_model(),
                        &s.tagger_tags(),
                        r.threads,
                    ) {
                        Ok(t) => *guard = Some(t),
                        Err(e) => return fail_all(e),
                    }
                }
                let tagger = guard.as_mut().expect("loaded above");
                for file in &r.files {
                    let event = match load(file).and_then(|img| tagger.tag(&img, r.thresholds)) {
                        Ok(tags) => Event::Tagged(file.clone(), tags),
                        Err(e) => Event::Failed(file.clone(), e),
                    };
                    r.done.fetch_add(1, Ordering::Relaxed);
                    send(event);
                }
            }
            Work::Faces => {
                let mut guard = faces::shared().lock().unwrap_or_else(|e| e.into_inner());
                if guard.is_none() {
                    let s = &r.store;
                    match faces::FaceModels::load(
                        &s.runtime(),
                        &s.face_detector(),
                        &s.face_recognizer(),
                        r.threads,
                    ) {
                        Ok(m) => *guard = Some(m),
                        Err(e) => return fail_all(e),
                    }
                }
                let models = guard.as_mut().expect("loaded above");
                for file in &r.files {
                    let event = match load(file).and_then(|img| Ok((models.find(&img)?, img.size)))
                    {
                        Ok((found, size)) => Event::Faces {
                            path: file.clone(),
                            found,
                            size,
                        },
                        Err(e) => Event::Failed(file.clone(), e),
                    };
                    r.done.fetch_add(1, Ordering::Relaxed);
                    send(event);
                }
            }
        }
        send(Event::Done);
    });
    rx
}

impl VwApp {
    /// 画像にタグを付ける。
    pub(super) fn ai_tag_paths(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        self.queue_ai(ctx, Work::Tag, paths);
    }

    /// 画像の顔を探す。
    pub(super) fn find_faces_paths(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        self.queue_ai(ctx, Work::Faces, paths);
    }

    /// 仕事を積む。モデルが揃っていなければ「AI のモデル」を開いて知らせる。
    fn queue_ai(&mut self, ctx: &egui::Context, work: Work, paths: Vec<PathBuf>) {
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
        let missing: Vec<_> = work
            .needs()
            .into_iter()
            .filter(|&i| !store.has(i))
            .collect();
        if !missing.is_empty() {
            let names: Vec<_> = missing.iter().map(|i| i.title()).collect();
            self.log.error(format!(
                "This needs {} (Help > AI models...)",
                names.join(" and ")
            ));
            self.ai_models.open = true;
            return;
        }
        self.ai_queue.extend(files.into_iter().map(|f| (work, f)));
        if self.ai_job.is_none() {
            self.start_next_ai_job(ctx);
        }
    }

    /// 積んである仕事のうち、先頭と同じ種類のものをまとめて始める。
    fn start_next_ai_job(&mut self, ctx: &egui::Context) {
        let Some(&(work, _)) = self.ai_queue.first() else {
            return;
        };
        let Some(store) = self.ai_store.clone() else {
            return;
        };
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.ai_queue)
            .into_iter()
            .partition(|(w, _)| *w == work);
        self.ai_queue = later;
        let files: Vec<PathBuf> = now.into_iter().map(|(_, f)| f).collect();
        let ai = &self.settings().ai;
        let (thresholds, threads) = (ai.thresholds(), ai.threads);
        let done = Arc::new(AtomicUsize::new(0));
        match work {
            Work::Tag => self
                .log
                .info(format!("AI tagging {} image(s)", files.len())),
            Work::Faces => self
                .log
                .info(format!("Finding faces in {} image(s)", files.len())),
        }
        let total = files.len();
        let rx = run(
            ctx.clone(),
            work,
            Run {
                registry: self.loader.registry().clone(),
                store,
                files,
                thresholds,
                threads,
                done: done.clone(),
            },
        );
        self.ai_job = Some(Job {
            work,
            total,
            done,
            rx,
        });
    }

    pub(super) fn ai_job_name(&self) -> &'static str {
        match self.ai_job.as_ref().map(|j| j.work) {
            Some(Work::Faces) => "Finding faces",
            _ => "AI tagging",
        }
    }

    fn folder_images(&mut self, dir: &Path) -> Vec<PathBuf> {
        let registry = self.loader.registry().clone();
        let images = crate::tree::list_folder(dir, &registry, self.settings().tree.order()).images;
        if images.is_empty() {
            self.log.warn(format!("No images in {}", dir.display()));
        }
        images
    }

    pub(super) fn ai_tag_folder(&mut self, ctx: &egui::Context, dir: &Path) {
        let images = self.folder_images(dir);
        self.ai_tag_paths(ctx, images);
    }

    pub(super) fn find_faces_folder(&mut self, ctx: &egui::Context, dir: &Path) {
        let images = self.folder_images(dir);
        self.find_faces_paths(ctx, images);
    }

    pub(super) fn ai_tag_current_folder(&mut self, ctx: &egui::Context, work: Work) {
        match self.current.clone() {
            Some(dir) if work == Work::Tag => self.ai_tag_folder(ctx, &dir),
            Some(dir) => self.find_faces_folder(ctx, &dir),
            None => self.log.warn("No folder is open"),
        }
    }

    fn save_faces(&mut self, path: PathBuf, found: Vec<Found>, size: [usize; 2]) {
        let threshold = self.settings().ai.face_threshold;
        let marks: Vec<FaceMark> = found
            .iter()
            .map(|f| {
                let mut m = faces::mark(&f.face, size);
                if let Some((name, s)) = self.people.best_match(&f.feature, threshold) {
                    m.person = Some(name);
                    m.similarity = s;
                }
                m
            })
            .collect();
        let samples = found
            .into_iter()
            .map(|f| Sample {
                feature: f.feature,
                thumb: f.thumb,
                source: Some(path.clone()),
            })
            .collect();
        if let Err(e) = self.face_store.set(&path, samples) {
            self.log.error(format!("Cannot save faces: {e}"));
            return;
        }
        let names: Vec<String> = marks.iter().filter_map(|m| m.person.clone()).collect();
        let n = marks.len();
        match self.ai_tags.update(&path, |e| e.faces = marks) {
            Err(e) => self.log.error(format!("Cannot save AI tags: {e}")),
            Ok(()) if names.is_empty() => {
                self.log.info(format!("{n} face(s): {}", path.display()));
            }
            Ok(()) => self.log.info(format!(
                "{n} face(s) ({}): {}",
                names.join(", "),
                path.display()
            )),
        }
    }

    /// 毎フレーム：裏の仕事の結果を受け取り、ダウンロードの結果をログに出し、ウインドウを描く。
    pub(super) fn ai_ui(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        if let Some(job) = &self.ai_job {
            let mut finished = false;
            let events: Vec<Event> = job.rx.try_iter().collect();
            for event in events {
                match event {
                    Event::Tagged(path, tags) => {
                        let n = tags.general.len() + tags.characters.len();
                        let saved = self.ai_tags.update(&path, |e| {
                            e.model = ai_models::TAGGER_NAME.to_owned();
                            e.tags = tags;
                        });
                        match saved {
                            Err(e) => self.log.error(format!("Cannot save AI tags: {e}")),
                            Ok(()) => self.log.info(format!("{} tag(s): {}", n, path.display())),
                        }
                    }
                    Event::Faces { path, found, size } => self.save_faces(path, found, size),
                    Event::Failed(path, e) => {
                        self.log
                            .error(format!("AI failed for {}: {e}", path.display()));
                    }
                    Event::Done => finished = true,
                }
            }
            if finished {
                self.ai_job = None;
                self.start_next_ai_job(ctx);
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
        self.people_ui(ctx);
    }

    /// メタデータ欄の「AI tags」の行。タグも顔もなければ `None`。
    pub(super) fn ai_tag_rows(&self, path: &Path) -> Option<Vec<(String, String)>> {
        let lang = self.lang();
        let file = crate::frames::file_of(path);
        let entry = self.ai_tags.get(&file)?;
        let mut rows = Vec::new();
        let people = entry.people();
        if !people.is_empty() {
            rows.push((tr(lang, "People").to_owned(), people.join(", ")));
        }
        for (i, f) in entry.faces.iter().enumerate() {
            let who = match &f.person {
                Some(name) => format!("{name} ({:.2})", f.similarity),
                None => tr(lang, "Unknown").to_owned(),
            };
            rows.push((format!("{} {}", tr(lang, "Face"), i + 1), who));
        }
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
        if !entry.model.is_empty() {
            rows.push((tr(lang, "AI model").to_owned(), entry.model.clone()));
        }
        (!rows.is_empty()).then_some(rows)
    }
}
