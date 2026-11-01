//! データセットの登録・編集・更新・取り出しのダイアログ。フォルダ以下を調べるのは別スレッドで行い、
//! 進み具合を出す。暗号化されたアーカイブに当たったら、調べる側を止めてパスワードを聞く。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Duration;

use eframe::egui;
use vw_core::Registry;

use crate::ai_tags::AiTagStore;
use crate::catalog::{self, Catalog, Info, Progress, Report};
use crate::dataset_extract;
use crate::dataset_search::Index;
use crate::i18n::{tr, tr_text, tr1};
use crate::labels::LabelStore;
use crate::log::Log;
use crate::settings::Language;

/// ツリーの右クリックメニューや Datasets 画面から頼まれること。
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Register(PathBuf),
    Edit(PathBuf),
    Update(PathBuf),
    /// 取り出す。検索でファイルを絞り込んでいれば、当たったファイルのパスも渡す。
    Extract(PathBuf, Option<Vec<String>>),
    /// 途中で止めた登録か更新を、続きからすぐに読み始める。
    Resume(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Register,
    Edit,
    Update,
    Extract,
}

pub struct DatasetDialog {
    dir: PathBuf,
    kind: Kind,
    name: String,
    memo: String,
    tags: String,
    fields: Vec<(String, String)>,
    previous: Option<Catalog>,
    /// 登録・保存できない理由。
    error: Option<String>,
    job: Option<ScanJob>,
    password: Option<PasswordAsk>,
    /// 他のデータセットで使っているタグ。表記ゆれを減らすため候補に出す。
    known_tags: Vec<String>,
    /// 項目ごとの、他のデータセットで使っている値。
    known_values: BTreeMap<String, Vec<String>>,
    /// 取り出しの出力先。
    dest: String,
    /// 取り出した先をデータセットとして登録するか。
    register_extracted: bool,
    /// 取り出した先を含むデータセット。取り出し後に更新するか聞くため。
    outer: Option<PathBuf>,
    /// 取り出しが終わった出力先。
    extracted_to: Option<PathBuf>,
    /// 検索で当たったファイル（カタログのパス）。取り出しを検索から頼まれたときだけ。
    hits: Option<Vec<String>>,
    /// 当たったファイルだけを取り出すか。
    only_hits: bool,
    /// AI タグをフォルダに書けなかったときの置き場所。
    ai_tag_fallback: Option<PathBuf>,
    /// ラベルをフォルダに書けなかったときの置き場所。
    label_fallback: Option<PathBuf>,
    /// 前回途中で止めた登録。
    unfinished: Option<catalog::Unfinished>,
    /// 前回の続きから読むか。
    resume: bool,
    /// キャンセルされたので、止まったら途中の結果を捨てる。
    discard: bool,
}

/// 別スレッドでの処理の結果。
enum Outcome {
    Scanned(Report),
    Extracted(dataset_extract::Extracted),
}

struct ScanJob {
    progress: Arc<Progress>,
    result: Receiver<Result<Outcome, String>>,
    asks: Receiver<PasswordRequest>,
}

struct PasswordRequest {
    archive: PathBuf,
    wrong: bool,
    reply: Sender<Option<String>>,
}

struct PasswordAsk {
    request: PasswordRequest,
    input: String,
    focused: bool,
}

impl DatasetDialog {
    /// `fields` は設定で足した項目。カタログにしかない項目もその後ろに出す。`known` は
    /// Datasets 画面で集めたデータセットの一覧で、タグと項目の値の候補に使う。`extract_base` は
    /// 取り出しの出力先を置く場所の初期値。
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        request: Request,
        fields: &[String],
        known: Option<&Index>,
        extract_base: &Path,
        ai_tag_fallback: Option<&Path>,
        label_fallback: Option<&Path>,
        registry: &Registry,
        ctx: &egui::Context,
    ) -> Self {
        let resuming = matches!(request, Request::Resume(_));
        let (dir, kind, hits) = match request {
            Request::Resume(d) if catalog::exists_in(&d) => (d, Kind::Update, None),
            Request::Resume(d) => (d, Kind::Register, None),
            Request::Register(d) => (d, Kind::Register, None),
            Request::Edit(d) => (d, Kind::Edit, None),
            Request::Update(d) => (d, Kind::Update, None),
            Request::Extract(d, hits) => (d, Kind::Extract, hits),
        };
        let mut dialog = Self {
            name: String::new(),
            memo: String::new(),
            tags: String::new(),
            fields: fields.iter().map(|f| (f.clone(), String::new())).collect(),
            previous: None,
            error: None,
            job: None,
            password: None,
            known_tags: known.map(Index::tags).unwrap_or_default(),
            known_values: known.map(Index::field_values).unwrap_or_default(),
            dest: default_destination(&dir, extract_base),
            extracted_to: None,
            register_extracted: false,
            outer: None,
            only_hits: hits.is_some(),
            hits,
            ai_tag_fallback: ai_tag_fallback.map(Path::to_path_buf),
            label_fallback: label_fallback.map(Path::to_path_buf),
            unfinished: (kind != Kind::Extract)
                .then(|| catalog::unfinished(&dir))
                .flatten(),
            resume: true,
            discard: false,
            dir,
            kind,
        };
        if kind == Kind::Register {
            dialog.name = dialog
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            dialog.error = catalog::check_can_register(&dialog.dir).err();
            if let Some(info) = dialog.unfinished.as_ref().and_then(|u| u.info.clone())
                && !info.name.is_empty()
            {
                dialog.fill_info(info);
            }
        } else {
            match catalog::load(&dialog.dir) {
                Ok(c) => dialog.fill(c),
                Err(e) => dialog.error = Some(e),
            }
        }
        if (kind == Kind::Update || resuming) && dialog.error.is_none() {
            dialog.start(registry, ctx, Language::English);
        }
        dialog
    }

    /// 調べたデータセットのフォルダ。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 取り出した先を含むデータセット。取り出しが終わったときだけ。
    pub fn outer(&self) -> Option<&Path> {
        self.outer.as_deref()
    }

    /// 調べている・取り出している最中なら、その進み具合（済んだ数, 全部の数）。
    pub fn progress(&self) -> Option<(usize, usize)> {
        let job = self.job.as_ref()?;
        Some((
            job.progress.done.load(Ordering::Relaxed),
            job.progress.total.load(Ordering::Relaxed),
        ))
    }

    /// 取り出しが終わった出力先。
    pub fn extracted_to(&self) -> Option<&Path> {
        self.extracted_to.as_deref()
    }

    fn fill(&mut self, c: Catalog) {
        self.fill_info(Info {
            name: c.name.clone(),
            memo: c.memo.clone(),
            tags: c.tags.clone(),
            fields: c.fields.clone(),
        });
        self.previous = Some(c);
    }

    fn fill_info(&mut self, info: Info) {
        self.name = info.name;
        self.memo = info.memo;
        self.tags = info.tags.join(", ");
        for (key, value) in info.fields {
            match self.fields.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => *v = value,
                None => self.fields.push((key, value)),
            }
        }
    }

    fn info(&self) -> Info {
        let mut tags: Vec<String> = Vec::new();
        for t in self
            .tags
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            if !tags.iter().any(|x| x == t) {
                tags.push(t.to_owned());
            }
        }
        Info {
            name: self.name.trim().to_owned(),
            memo: self.memo.trim().to_owned(),
            tags,
            // 空の項目も残す。設定から項目を消しても、他の人が入れた値を消さないのと同じく、
            // 入力欄を空にしただけで項目そのものが消えたように見えないようにするため。
            fields: self
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), v.trim().to_owned()))
                .collect(),
        }
    }

    fn start(&mut self, registry: &Registry, ctx: &egui::Context, lang: Language) {
        let dest = PathBuf::from(self.dest.trim());
        if self.kind == Kind::Extract
            && let Err(e) = dataset_extract::check_destination(&self.dir, &dest)
        {
            self.error = Some(tr_text(lang, &e));
            return;
        }
        let kind = self.kind;
        let register = self.register_extracted;
        let progress = Arc::new(Progress::default());
        let (done, result) = channel();
        let (ask, asks) = channel();
        let info = self.info();
        let dir = self.dir.clone();
        let previous = match (&self.previous, &self.hits) {
            (Some(c), Some(hits)) if self.only_hits => Some(dataset_extract::only(c, hits)),
            _ => self.previous.clone(),
        };
        let registry = registry.clone();
        let ai_tag_fallback = self.ai_tag_fallback.clone();
        let label_fallback = self.label_fallback.clone();
        let worker_progress = progress.clone();
        let resume = match (&self.unfinished, self.resume) {
            (Some(_), true) => catalog::Resume::Continue,
            _ => catalog::Resume::Fresh,
        };
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let repaint = ctx.clone();
            let mut ask_password = |archive: &Path, wrong: bool| {
                let (reply, answer) = channel();
                ask.send(PasswordRequest {
                    archive: archive.to_path_buf(),
                    wrong,
                    reply,
                })
                .ok()?;
                repaint.request_repaint();
                answer.recv().ok().flatten()
            };
            // 画面のスレッドの AiTagStore・LabelStore は RefCell を持ち別のスレッドへ渡せないので、同じファイルを読むものをここで作る。
            let ai_tags = AiTagStore::new(ai_tag_fallback);
            let labels = LabelStore::new(label_fallback);
            let outcome = match (kind, &previous) {
                (Kind::Extract, Some(source)) => dataset_extract::extract(
                    &dir,
                    source,
                    &dest,
                    register.then_some((info, &registry, &ai_tags)),
                    &worker_progress,
                    &mut ask_password,
                )
                .map(Outcome::Extracted),
                _ => catalog::scan_resumable(
                    &dir,
                    info,
                    previous.as_ref(),
                    &registry,
                    &ai_tags,
                    &labels,
                    &worker_progress,
                    &mut ask_password,
                    resume,
                )
                .and_then(|r| catalog::save(&dir, &r.catalog).map(|()| r))
                .inspect(|_| catalog::discard_unfinished(&dir))
                .map(Outcome::Scanned),
            };
            let _ = done.send(outcome);
            ctx.request_repaint();
        });
        self.job = Some(ScanJob {
            progress,
            result,
            asks,
        });
    }

    fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.progress.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(ask) = self.password.take() {
            let _ = ask.request.reply.send(None);
        }
    }

    /// ダイアログを描く。閉じるときは `false`。
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        lang: Language,
        registry: &Registry,
        log: &mut Log,
    ) -> bool {
        if let Some(finished) = self.poll(log, lang) {
            return !finished;
        }
        let title = match self.kind {
            Kind::Register => tr(lang, "Register as dataset"),
            Kind::Edit => tr(lang, "Edit dataset"),
            Kind::Update => tr(lang, "Update dataset"),
            Kind::Extract => tr(lang, "Extract dataset"),
        };
        let mut open = true;
        let mut run = false;
        let mut save = false;
        let mut cancel = false;
        let mut discard = false;
        let running = self.job.is_some();
        egui::Window::new(title)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_max_width(480.0);
                ui.label(egui::RichText::new(self.dir.display().to_string()).monospace());
                ui.add_space(4.0);
                if self.kind == Kind::Extract {
                    ui.add_enabled_ui(!running, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(tr(lang, "Destination"));
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.dest)
                                        .desired_width(f32::INFINITY),
                                )
                                .changed()
                            {
                                self.error = None;
                            }
                        });
                        ui.weak(tr(
                            lang,
                            "The files are copied with their folders. Files that already exist there are not overwritten.",
                        ));
                        if let (Some(hits), Some(c)) = (&self.hits, &self.previous) {
                            ui.checkbox(
                                &mut self.only_hits,
                                tr1(
                                    lang,
                                    "Extract only the files found by the search ({})",
                                    &format!("{} / {}", hits.len(), c.files.len()),
                                ),
                            );
                        }
                        ui.checkbox(
                            &mut self.register_extracted,
                            tr(lang, "Register the destination as a dataset"),
                        );
                    });
                    if self.register_extracted {
                        ui.add_enabled_ui(!running, |ui| self.inputs_ui(ui, lang));
                    }
                } else {
                    ui.add_enabled_ui(!running && self.kind != Kind::Update, |ui| {
                        self.inputs_ui(ui, lang);
                    });
                }
                if let Some(u) = &self.unfinished {
                    ui.add_space(4.0);
                    ui.add_enabled_ui(!running && self.kind != Kind::Update, |ui| {
                        ui.label(tr1(
                            lang,
                            "The previous reading stopped partway ({} files done).",
                            &u.done.to_string(),
                        ));
                        if let Some(c) = &u.crashed {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                tr1(lang, "vw stopped while reading {}. It will be skipped.", c),
                            );
                        }
                        ui.checkbox(&mut self.resume, tr(lang, "Continue from where it stopped"));
                    });
                }
                if let Some(e) = &self.error {
                    ui.add_space(4.0);
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.separator();
                match &self.job {
                    Some(job) => {
                        let done = job.progress.done.load(Ordering::Relaxed);
                        let total = job.progress.total.load(Ordering::Relaxed);
                        let current = job
                            .progress
                            .current
                            .lock()
                            .map(|c| c.clone())
                            .unwrap_or_default();
                        let paused = job.progress.paused();
                        ui.horizontal(|ui| {
                            let text = if paused {
                                format!("{done} / {total}  {}", tr(lang, "(paused)"))
                            } else {
                                format!("{done} / {total}")
                            };
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                                    .text(text)
                                    .desired_width(300.0),
                            );
                            if self.kind == Kind::Extract {
                                cancel = ui.button(tr(lang, "Cancel")).clicked();
                                return;
                            }
                            let (label, hint) = if paused {
                                ("Resume reading", "Continue reading the files.")
                            } else {
                                (
                                    "Pause reading",
                                    "Stop reading for now. Even if vw is closed while paused, reading continues from here the next time.",
                                )
                            };
                            if ui.button(tr(lang, label)).on_hover_text(tr(lang, hint)).clicked() {
                                job.progress.pause.store(!paused, Ordering::Relaxed);
                            }
                            discard = ui
                                .button(tr(lang, "Cancel"))
                                .on_hover_text(tr(
                                    lang,
                                    "Stop reading and discard the files read so far. The next time, reading starts over.",
                                ))
                                .clicked();
                        });
                        ui.add(egui::Label::new(egui::RichText::new(current).weak()).truncate());
                    }
                    None => {
                        let ok = self.error.is_none() && !self.name.trim().is_empty();
                        ui.horizontal(|ui| match self.kind {
                            Kind::Register => {
                                run = ui
                                    .add_enabled(ok, egui::Button::new(tr(lang, "Register")))
                                    .clicked();
                            }
                            Kind::Edit => {
                                save = ui
                                    .add_enabled(ok, egui::Button::new(tr(lang, "Save")))
                                    .clicked();
                                run = ui
                                    .add_enabled(ok, egui::Button::new(tr(lang, "Save and update")))
                                    .on_hover_text(tr(
                                        lang,
                                        "Read all files again and rebuild the search catalog.",
                                    ))
                                    .clicked();
                            }
                            Kind::Extract => {
                                run = ui
                                    .add_enabled(
                                        ok && !self.dest.trim().is_empty(),
                                        egui::Button::new(tr(lang, "Extract")),
                                    )
                                    .clicked();
                            }
                            Kind::Update => {}
                        });
                    }
                }
            });
        self.password_ui(ctx, lang);
        if running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if cancel {
            self.cancel();
        }
        if discard {
            self.discard = true;
            self.cancel();
        }
        if !open {
            self.cancel();
            return false;
        }
        if save {
            return !self.save_info(log);
        }
        if run {
            self.start(registry, ctx, lang);
        }
        true
    }

    fn inputs_ui(&mut self, ui: &mut egui::Ui, lang: Language) {
        egui::Grid::new("dataset-inputs")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label(tr(lang, "Name"));
                ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(340.0));
                ui.end_row();
                ui.label(tr(lang, "Memo"));
                ui.add(
                    egui::TextEdit::multiline(&mut self.memo)
                        .desired_rows(3)
                        .desired_width(340.0),
                );
                ui.end_row();
                ui.label(tr(lang, "Tags"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.tags)
                            .hint_text(tr(lang, "Separate with commas"))
                            .desired_width(340.0),
                    );
                    if let Some(tag) = pick(ui, "dataset-tag-choices", &self.known_tags) {
                        let mut tags = self.tags.trim().trim_end_matches(',').to_owned();
                        if !tags.is_empty() {
                            tags.push_str(", ");
                        }
                        tags.push_str(&tag);
                        self.tags = tags;
                    }
                });
                ui.end_row();
                for (key, value) in &mut self.fields {
                    ui.label(key.as_str());
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(value).desired_width(340.0));
                        let known = self.known_values.get(key).map_or(&[][..], Vec::as_slice);
                        if let Some(v) = pick(ui, ("dataset-field-choices", key.as_str()), known) {
                            *value = v;
                        }
                    });
                    ui.end_row();
                }
            });
    }

    /// 名前・メモ・タグ・項目だけを書き換える。ファイルは読み直さない。
    fn save_info(&mut self, log: &mut Log) -> bool {
        let Some(mut c) = self.previous.clone() else {
            return false;
        };
        c.set_info(self.info());
        match catalog::save(&self.dir, &c) {
            Ok(()) => {
                log.done(
                    "Saved",
                    format!("dataset \"{}\": {}", c.name, self.dir.display()),
                );
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    /// 調べ終わったら結果をログに出す。`Some(true)` ならダイアログを閉じる。
    fn poll(&mut self, log: &mut Log, lang: Language) -> Option<bool> {
        let job = self.job.as_ref()?;
        if self.password.is_none()
            && let Ok(request) = job.asks.try_recv()
        {
            self.password = Some(PasswordAsk {
                request,
                input: String::new(),
                focused: false,
            });
        }
        let result = match job.result.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("The scan stopped unexpectedly".into()),
        };
        self.job = None;
        self.password = None;
        match result {
            Ok(Outcome::Extracted(x)) => {
                let dest = PathBuf::from(self.dest.trim());
                log.done(
                    "Extracted",
                    format!(
                        "dataset \"{}\": {} files from {} to {}",
                        self.name.trim(),
                        x.copied,
                        self.dir.display(),
                        dest.display()
                    ),
                );
                for f in &x.failed {
                    log.warn(format!("Dataset \"{}\": not copied: {f}", self.name.trim()));
                }
                if let Some(report) = &x.registered {
                    let c = &report.catalog;
                    log.done(
                        "Registered",
                        format!(
                            "dataset \"{}\": {} files in {}",
                            c.name,
                            c.files.len(),
                            dest.display()
                        ),
                    );
                    for p in &c.skipped {
                        log.warn(format!("Dataset \"{}\": {p}", c.name));
                    }
                }
                self.outer = dataset_extract::outer_dataset(&dest);
                self.extracted_to = Some(dest);
                Some(true)
            }
            Ok(Outcome::Scanned(report)) => {
                let c = &report.catalog;
                let verb = match self.kind {
                    Kind::Register => "Registered",
                    Kind::Edit | Kind::Update | Kind::Extract => "Updated",
                };
                log.done(
                    verb,
                    format!(
                        "dataset \"{}\": {} files ({} added, {} removed, {} changed) in {}",
                        c.name,
                        c.files.len(),
                        report.added,
                        report.removed,
                        report.changed,
                        self.dir.display()
                    ),
                );
                for p in &c.skipped {
                    log.warn(format!("Dataset \"{}\": {p}", c.name));
                }
                Some(true)
            }
            Err(e) if e == "Cancelled" && self.kind == Kind::Extract => {
                // 写した分は消さない。出力先を人が選んだので、消してよいものか分からないため。
                log.info(tr1(
                    lang,
                    "Stopped extracting. The files already copied remain in {}.",
                    self.dest.trim(),
                ));
                Some(false)
            }
            Err(e) if e == "Cancelled" && self.discard => {
                // 読んでいたスレッドは終わっているので、ここで消しても書き戻されない。
                catalog::discard_unfinished(&self.dir);
                self.discard = false;
                self.unfinished = None;
                log.info(tr1(
                    lang,
                    "Cancelled reading {}.",
                    &self.dir.display().to_string(),
                ));
                Some(self.kind == Kind::Update)
            }
            Err(e) if e == "Cancelled" => {
                log.info(tr1(
                    lang,
                    "Stopped reading {}. Register or update it again to continue from there.",
                    &self.dir.display().to_string(),
                ));
                self.unfinished = catalog::unfinished(&self.dir);
                Some(self.kind == Kind::Update)
            }
            Err(e) => {
                log.error(format!("Dataset: {e}"));
                self.error = Some(e);
                Some(self.kind == Kind::Update)
            }
        }
    }

    fn password_ui(&mut self, ctx: &egui::Context, lang: Language) {
        let Some(ask) = &mut self.password else {
            return;
        };
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("dataset_password")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            let name = ask
                .request
                .archive
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            ui.label(tr1(lang, "{} is encrypted. Enter the password.", &name));
            if ask.request.wrong {
                ui.colored_label(ui.visuals().error_fg_color, tr(lang, "Wrong password."));
            }
            ui.add_space(4.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut ask.input)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
            if !ask.focused {
                field.request_focus();
                ask.focused = true;
            }
            let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr(lang, "Open")).clicked() || entered {
                    answer = Some(Some(std::mem::take(&mut ask.input)));
                }
                if ui.button(tr(lang, "Skip")).clicked() {
                    answer = Some(None);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(None);
        }
        if let Some(a) = answer
            && let Some(ask) = self.password.take()
        {
            let _ = ask.request.reply.send(a);
        }
    }
}

/// 取り出しの出力先の初期値。`base` の下の、データセットのフォルダと同じ名前のまだないフォルダ。
fn default_destination(dir: &Path, base: &Path) -> String {
    let Some(name) = dir.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return String::new();
    };
    let candidate = |n: usize| {
        let suffix = if n == 1 {
            String::new()
        } else {
            format!("-{n}")
        };
        base.join(format!("{name}{suffix}"))
    };
    let mut n = 1;
    while candidate(n).exists() {
        n += 1;
    }
    candidate(n).display().to_string()
}

/// 今ある値から選ぶボタン。値がなければ出さない。
fn pick(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    values: &[String],
) -> Option<String> {
    if values.is_empty() {
        return None;
    }
    let mut chosen = None;
    ui.push_id(id, |ui| {
        vw_ui::menu::button(ui, egui_phosphor::regular::CARET_DOWN, |ui| {
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .show(ui, |ui| {
                    for v in values {
                        if ui.button(v).clicked() {
                            chosen = Some(v.clone());
                            ui.close();
                        }
                    }
                });
        });
    });
    chosen
}
