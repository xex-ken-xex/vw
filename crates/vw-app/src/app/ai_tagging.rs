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
use crate::i18n::{tr, tr1};
use crate::people::Sample;
use crate::tagger::{self, Tags, Thresholds};

/// メタデータ欄の AI タグのグループの英語の名前。
pub(super) const AI_TAGS: &str = "AI tags";

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

pub(super) enum Event {
    Tagged(PathBuf, Tags),
    Faces {
        path: PathBuf,
        found: Vec<Found>,
        size: [usize; 2],
    },
    Failed(PathBuf, String),
    /// モデルを読み込めなかった。どの画像も処理していない。
    Unusable(String),
    Done,
}

pub(super) struct Job {
    work: Work,
    total: usize,
    done: Arc<AtomicUsize>,
    rx: Receiver<Event>,
    /// 終わってからまとめてログに出すもの。
    saved: usize,
    faces: usize,
    failed: Vec<(PathBuf, String)>,
}

/// 失敗した画像を 1 枚ずつログに出す上限。フォルダ全部が同じ理由で失敗したとき、
/// ログがその行で埋まらないように。
const FAILURES_SHOWN: usize = 5;

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
        let unusable = |e: String| {
            send(Event::Unusable(e));
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
                        Err(e) => return unusable(e),
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
                        Err(e) => return unusable(e),
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
        // 仕事に要るものだけでなく全部を求めるのは、ダウンロードを 1 回で済ませ、
        // 後で別の機能を使うときに改めて待たせないため。
        if !Item::AI.iter().all(|&i| store.has(i)) {
            self.log.error(tr(lang, ai_models::NOT_READY));
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
                .info(format!("AI tagging {} image(s)…", files.len())),
            Work::Faces => self
                .log
                .info(format!("Finding faces in {} image(s)…", files.len())),
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
            saved: 0,
            faces: 0,
            failed: Vec::new(),
        });
    }

    /// モデルが全部揃っているか。揃うまで、タグ付けと顔探しは押せない。
    pub(super) fn ai_ready(&self) -> bool {
        self.ai_store
            .as_ref()
            .is_some_and(|s| Item::AI.iter().all(|&i| s.has(i)))
    }

    /// 終わった仕事を、ZIP の展開と同じく 1 行にまとめてログに出す。
    fn log_ai_job(&mut self, job: &Job) {
        let what = match job.work {
            Work::Tag => format!("AI tags added to {} image(s)", job.saved),
            Work::Faces => format!("Found {} face(s) in {} image(s)", job.faces, job.saved),
        };
        if job.failed.is_empty() {
            self.log.info(what);
            return;
        }
        self.log
            .warn(format!("{what}, {} failed", job.failed.len()));
        for (path, e) in job.failed.iter().take(FAILURES_SHOWN) {
            self.log.error(format!("{}: {e}", path.display()));
        }
        if job.failed.len() > FAILURES_SHOWN {
            self.log
                .error(format!("… and {} more", job.failed.len() - FAILURES_SHOWN));
        }
    }

    pub(super) fn ai_job_name(&self) -> &'static str {
        match self.ai_job.as_ref().map(|j| j.work) {
            Some(Work::Faces) => "Finding faces",
            _ => "AI tagging",
        }
    }

    pub(super) fn folder_images(&mut self, dir: &Path) -> Vec<PathBuf> {
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

    /// 見つけた顔を書く。書けたら顔の数。
    fn save_faces(&mut self, path: PathBuf, found: Vec<Found>, size: [usize; 2]) -> Option<usize> {
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
            return None;
        }
        let n = marks.len();
        match self.ai_tags.update(&path, |e| e.faces = marks) {
            Err(e) => {
                self.log.error(format!("Cannot save AI tags: {e}"));
                None
            }
            Ok(()) => Some(n),
        }
    }

    /// 毎フレーム：裏の仕事の結果を受け取り、ダウンロードの結果をログに出し、ウインドウを描く。
    pub(super) fn ai_ui(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        if let Some(mut job) = self.ai_job.take() {
            let mut finished = false;
            let mut unusable = None;
            let events: Vec<Event> = job.rx.try_iter().collect();
            for event in events {
                match event {
                    Event::Tagged(path, mut tags) => {
                        tags.remove(&self.settings().ai.excluded_tags);
                        let saved = self.ai_tags.update(&path, |e| {
                            e.model = ai_models::TAGGER_NAME.to_owned();
                            e.tags = tags;
                        });
                        match saved {
                            Err(e) => self.log.error(format!("Cannot save AI tags: {e}")),
                            Ok(()) => job.saved += 1,
                        }
                    }
                    Event::Faces { path, found, size } => {
                        self.people_window.counts = None;
                        if let Some(n) = self.save_faces(path, found, size) {
                            job.saved += 1;
                            job.faces += n;
                        }
                    }
                    Event::Failed(path, e) => job.failed.push((path, e)),
                    Event::Unusable(e) => unusable = Some(e),
                    Event::Done => finished = true,
                }
            }
            if let Some(e) = unusable {
                self.log.error(format!("Cannot load the AI models: {e}"));
            } else if finished {
                self.log_ai_job(&job);
            }
            if finished {
                self.start_next_ai_job(ctx);
            } else {
                self.ai_job = Some(job);
            }
        }
        for (item, result) in self.ai_models.finished() {
            match result {
                Ok(()) => {
                    self.log.info(format!("Downloaded {}", item.title()));
                    if item == Item::PlaceNames {
                        self.reload_place_names();
                    }
                }
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
        self.people_graph_ui(ctx);
        self.remove_dialog(ctx);
        self.ai_tidy_ui(ctx);
    }

    /// フォルダの AI タグを消してよいか聞く。記録がなければ、そう知らせるだけ。
    pub(super) fn ask_remove_ai_tags(&mut self, dir: &Path) {
        let images: Vec<PathBuf> = self
            .folder_images(dir)
            .into_iter()
            .filter(|p| self.ai_tags.get(p).is_some())
            .collect();
        if images.is_empty() {
            self.log.warn(format!("No AI tags in {}", dir.display()));
            return;
        }
        self.ai_remove = Some((dir.to_path_buf(), images));
    }

    /// 画像の AI タグ・顔の枠・顔の特徴を消す。登録した人物（people.json）はそのまま。
    pub(super) fn remove_ai_tags(&mut self, images: Vec<PathBuf>) {
        if let Err(e) = self.face_store.remove(&images) {
            self.log.error(format!("Cannot remove faces: {e}"));
        }
        match self.ai_tags.remove(&images) {
            Ok(n) => self.log.info(format!("Removed AI tags from {n} image(s)")),
            Err(e) => self.log.error(format!("Cannot remove AI tags: {e}")),
        }
        self.people_window.counts = None;
    }

    fn remove_dialog(&mut self, ctx: &egui::Context) {
        let Some((dir, images)) = &self.ai_remove else {
            return;
        };
        let lang = self.lang();
        let mut answer = None;
        egui::Window::new(tr(lang, "Remove AI tags"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(tr1(
                    lang,
                    "Remove the AI tags and faces of {} image(s)?",
                    &images.len().to_string(),
                ));
                ui.weak(dir.display().to_string());
                ui.weak(tr(lang, "Registered people are kept."));
                ui.horizontal(|ui| {
                    if ui.button(tr(lang, "Remove")).clicked() {
                        answer = Some(true);
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        answer = Some(false);
                    }
                });
            });
        match answer {
            Some(true) => {
                let (_, images) = self.ai_remove.take().expect("shown above");
                self.remove_ai_tags(images);
            }
            Some(false) => self.ai_remove = None,
            None => {}
        }
    }

    /// メタデータ欄の「AI tags」の行。消せるタグの行はタグ名も持つ。タグも顔もなければ `None`。
    pub(super) fn ai_tag_rows(&self, path: &Path) -> Option<Vec<crate::meta_view::Row>> {
        let lang = self.lang();
        let file = crate::frames::file_of(path);
        let entry = self.ai_tags.get(&file)?;
        let mut rows = Vec::new();
        let mut push = |key: &str, shown: String, value: String, tag: Option<String>| {
            let mut row = crate::meta_view::Row::new(AI_TAGS, key, shown, value);
            row.tag = tag;
            rows.push(row);
        };
        let people = entry.people();
        if !people.is_empty() {
            push(
                "People",
                tr(lang, "People").to_owned(),
                people.join(", "),
                None,
            );
        }
        for (i, f) in entry.faces.iter().enumerate() {
            let who = match &f.person {
                Some(name) => format!("{name} ({:.2})", f.similarity),
                None => tr(lang, "Unknown").to_owned(),
            };
            let n = i + 1;
            push(
                &format!("Face {n}"),
                format!("{} {n}", tr(lang, "Face")),
                who,
                None,
            );
        }
        if let Some(r) = &entry.tags.rating {
            push(
                "Rating",
                tr(lang, "Rating").to_owned(),
                format!("{} ({:.2})", r.name, r.score),
                None,
            );
        }
        for c in &entry.tags.characters {
            push(
                "Character",
                tr(lang, "Character").to_owned(),
                format!("{} ({:.2})", c.name, c.score),
                Some(c.name.clone()),
            );
        }
        for g in &entry.tags.general {
            push(
                &g.name,
                g.name.clone(),
                format!("{:.2}", g.score),
                Some(g.name.clone()),
            );
        }
        if !entry.model.is_empty() {
            push(
                "AI model",
                tr(lang, "AI model").to_owned(),
                entry.model.clone(),
                None,
            );
        }
        (!rows.is_empty()).then_some(rows)
    }
}
