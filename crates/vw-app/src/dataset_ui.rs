//! データセットの登録・編集・更新のダイアログ。フォルダ以下を調べるのは別スレッドで行い、
//! 進み具合を出す。暗号化されたアーカイブに当たったら、調べる側を止めてパスワードを聞く。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use vw_core::Registry;

use crate::catalog::{self, Catalog, Info, Progress, Report};
use crate::dataset_search::Index;
use crate::i18n::{tr, tr1};
use crate::log::Log;
use crate::settings::Language;

/// ツリーの右クリックメニューから頼まれること。
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Register(PathBuf),
    Edit(PathBuf),
    Update(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Register,
    Edit,
    Update,
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
}

struct ScanJob {
    progress: Arc<Progress>,
    result: Receiver<Result<Report, String>>,
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
    /// Datasets 画面で集めたデータセットの一覧で、タグと項目の値の候補に使う。
    pub fn open(
        request: Request,
        fields: &[String],
        known: Option<&Index>,
        registry: &Registry,
        ctx: &egui::Context,
    ) -> Self {
        let (dir, kind) = match request {
            Request::Register(d) => (d, Kind::Register),
            Request::Edit(d) => (d, Kind::Edit),
            Request::Update(d) => (d, Kind::Update),
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
        } else {
            match catalog::load(&dialog.dir) {
                Ok(c) => dialog.fill(c),
                Err(e) => dialog.error = Some(e),
            }
        }
        if kind == Kind::Update && dialog.error.is_none() {
            dialog.start(registry, ctx);
        }
        dialog
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn fill(&mut self, c: Catalog) {
        self.name = c.name.clone();
        self.memo = c.memo.clone();
        self.tags = c.tags.join(", ");
        for (key, value) in &c.fields {
            match self.fields.iter_mut().find(|(k, _)| k == key) {
                Some((_, v)) => v.clone_from(value),
                None => self.fields.push((key.clone(), value.clone())),
            }
        }
        self.previous = Some(c);
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

    fn start(&mut self, registry: &Registry, ctx: &egui::Context) {
        let progress = Arc::new(Progress::default());
        let (done, result) = channel();
        let (ask, asks) = channel();
        let info = self.info();
        let dir = self.dir.clone();
        let previous = self.previous.clone();
        let registry = registry.clone();
        let worker_progress = progress.clone();
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
            let report = catalog::scan(
                &dir,
                info,
                previous.as_ref(),
                &registry,
                &worker_progress,
                &mut ask_password,
            )
            .and_then(|r| catalog::save(&dir, &r.catalog).map(|()| r));
            let _ = done.send(report);
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
        };
        let mut open = true;
        let mut run = false;
        let mut save = false;
        let mut cancel = false;
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
                ui.add_enabled_ui(!running && self.kind != Kind::Update, |ui| {
                    self.inputs_ui(ui, lang);
                });
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
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                                    .text(format!("{done} / {total}"))
                                    .desired_width(300.0),
                            );
                            cancel = ui.button(tr(lang, "Cancel")).clicked();
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
        if !open {
            self.cancel();
            return false;
        }
        if save {
            return !self.save_info(log);
        }
        if run {
            self.start(registry, ctx);
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
                log.info(format!(
                    "Dataset \"{}\" saved: {}",
                    c.name,
                    self.dir.display()
                ));
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
        if self.password.is_none() {
            if let Ok(request) = job.asks.try_recv() {
                self.password = Some(PasswordAsk {
                    request,
                    input: String::new(),
                    focused: false,
                });
            }
        }
        let result = match job.result.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("The scan stopped unexpectedly".into()),
        };
        self.job = None;
        self.password = None;
        match result {
            Ok(report) => {
                let c = &report.catalog;
                let verb = match self.kind {
                    Kind::Register => "registered",
                    Kind::Edit | Kind::Update => "updated",
                };
                log.info(format!(
                    "Dataset \"{}\" {verb}: {} files ({} added, {} removed, {} changed) in {}",
                    c.name,
                    c.files.len(),
                    report.added,
                    report.removed,
                    report.changed,
                    self.dir.display()
                ));
                for p in &c.skipped {
                    log.warn(format!("Dataset \"{}\": {p}", c.name));
                }
                Some(true)
            }
            Err(e) if e == "Cancelled" => {
                log.info(tr1(
                    lang,
                    "Stopped reading {}.",
                    &self.dir.display().to_string(),
                ));
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
        if let Some(a) = answer {
            if let Some(ask) = self.password.take() {
                let _ = ask.request.reply.send(a);
            }
        }
    }
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
        ui.menu_button(egui_phosphor::regular::CARET_DOWN, |ui| {
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
