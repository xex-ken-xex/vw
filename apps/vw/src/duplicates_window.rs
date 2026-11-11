//! 重複データ探索のウインドウ（ツール →「重複データ探索」）。ツリーでクリックしたフォルダか、
//! スロットで選んだファイルを起点にして、その中のもの 1 つずつと同じものがデータセットの
//! フォルダ（`dataset.roots`）以下にあるかを左に、選んだ項目の見つかった場所を右に出す。
//!
//! データセット側のファイルの一覧は開いたときに 1 度だけ作り、ハッシュも開いている間だけ
//! 覚える。ファイルは何も書かない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use eframe::egui::{self, vec2};
use egui_phosphor::regular as icon;

use crate::duplicates::{Cancelled, Hashes, Index, Outcome, Progress, Rules, Search};
use crate::i18n::{tr, tr1};
use crate::settings::{DuplicatesSettings, Language};

/// ウインドウを描くのに要るもの。
pub struct Inputs<'a> {
    pub lang: Language,
    pub roots: &'a [String],
    pub settings: &'a DuplicatesSettings,
    /// 今開いているフォルダ（ツリーでクリックしたフォルダ）。
    pub folder: Option<&'a Path>,
    /// 選んでいるスロットの番号と、そこに出ているもの（ファイルかフォルダ）。
    pub slot: (usize, Option<&'a Path>),
    /// フォルダの画像をスロットへ読み込んでいる途中か。
    pub loading: bool,
}

/// アプリへの頼みごと。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// そこへ移る（フォルダなら開き、ファイルならそのフォルダを開いて選ぶ）。
    Go(PathBuf),
    /// OS のファイルマネージャーでフォルダを開く。
    OpenInFileManager(PathBuf),
}

/// 起点の候補（ツリーとスロットの選択）を見張り、最後に操作した方を起点にする。
#[derive(Debug, Default)]
pub struct Follow {
    started: bool,
    folder: Option<PathBuf>,
    slot: (usize, Option<PathBuf>),
    /// このウインドウから頼んだ移り先。そこへ移った分は起点にしない。
    navigated: Option<PathBuf>,
    /// 起点にする候補と、それが見つかった時刻（秒）。
    pending: Option<(PathBuf, f64)>,
}

impl Follow {
    /// 今の選択を見て、起点にする候補を覚える。開いて最初の 1 回は今のフォルダを起点にする。
    pub fn observe(
        &mut self,
        folder: Option<&Path>,
        slot: (usize, Option<&Path>),
        loading: bool,
        now: f64,
    ) {
        let slot_path = slot.1.map(crate::frames::file_of);
        let slot = (slot.0, slot_path);
        if !self.started {
            self.started = true;
            self.folder = folder.map(Path::to_path_buf);
            self.slot = slot;
            self.pending = self.folder.clone().map(|f| (f, f64::NEG_INFINITY));
            return;
        }
        let folder_changed = folder != self.folder.as_deref();
        let slot_changed = slot != self.slot;
        let mut candidate = None;
        if folder_changed {
            // フォルダを移ると、スロットも一緒に入れ替わる。そのスロットは起点にしない。
            candidate = folder.map(Path::to_path_buf);
        } else if slot_changed {
            // 読み込み中に同じスロットの中身が埋まったのは、操作ではない。
            let filled_by_loading = loading && slot.0 == self.slot.0;
            if !filled_by_loading {
                candidate = slot.1.clone();
            }
        }
        self.folder = folder.map(Path::to_path_buf);
        self.slot = slot;
        if let Some(c) = candidate {
            if self.is_navigation(&c) {
                return;
            }
            self.navigated = None;
            self.pending = Some((c, now));
        }
    }

    fn is_navigation(&self, path: &Path) -> bool {
        self.navigated
            .as_deref()
            .is_some_and(|n| n == path || n.parent() == Some(path))
    }

    /// このウインドウから `path` へ移るのを頼んだ。
    pub fn navigating(&mut self, path: PathBuf) {
        self.navigated = Some(path);
    }

    /// 待ち時間が過ぎた候補を取り出す。
    pub fn take(&mut self, now: f64, delay: f64) -> Option<PathBuf> {
        match &self.pending {
            Some((_, at)) if now - at >= delay => self.pending.take().map(|(p, _)| p),
            _ => None,
        }
    }

    /// 次に起点を決めるまでの残り（秒）。
    pub fn waiting(&self, now: f64, delay: f64) -> Option<f64> {
        self.pending
            .as_ref()
            .map(|(_, at)| (delay - (now - at)).max(0.0))
    }
}

/// 左の一覧の 1 行。
#[derive(Debug, Clone)]
struct Item {
    path: PathBuf,
    is_dir: bool,
    depth: usize,
    parent: Option<usize>,
    bytes: Option<u64>,
    outcome: Option<Outcome>,
    expanded: bool,
    /// 子を一覧に足した。
    listed: bool,
}

/// 裏で調べる頼み（`items` の添字と、そのパス）。
type Job = (usize, PathBuf);
/// 裏からの答え（`items` の添字、大きさ、結果）。
type Answer = (usize, u64, Outcome);

struct Running {
    jobs: Sender<Job>,
    answers: Receiver<Answer>,
    progress: Arc<Progress>,
    asked: usize,
    answered: usize,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// データセット側の一覧を作るときの前提（変わったら作り直す）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexKey {
    roots: Vec<String>,
    ignore_names: String,
}

enum IndexState {
    None,
    Building {
        key: IndexKey,
        rx: Receiver<Result<Index, Cancelled>>,
        progress: Arc<Progress>,
    },
    Ready {
        key: IndexKey,
        index: Arc<Index>,
    },
}

/// 左の一覧の絞り込み。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Found,
    Missing,
}

/// 左の一覧の並べ替えに使う列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Name,
    Result,
    Size,
}

pub struct DuplicatesWindow {
    pub open: bool,
    follow: Follow,
    pinned: bool,
    origin: Option<PathBuf>,
    index: IndexState,
    hashes: Arc<Hashes>,
    running: Option<Running>,
    items: Vec<Item>,
    /// 一覧ができる前に頼まれた調べもの。
    waiting_jobs: Vec<Job>,
    selected: Option<usize>,
    filter: Filter,
    sort: (Column, bool),
    problems: Vec<String>,
}

impl Default for DuplicatesWindow {
    fn default() -> Self {
        Self {
            open: false,
            follow: Follow::default(),
            pinned: false,
            origin: None,
            index: IndexState::None,
            hashes: Arc::default(),
            running: None,
            items: Vec::new(),
            waiting_jobs: Vec::new(),
            selected: None,
            filter: Filter::All,
            sort: (Column::Name, true),
            problems: Vec::new(),
        }
    }
}

impl DuplicatesWindow {
    /// ログに出す、読めなかったもの。
    pub fn take_problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    /// 閉じたら、集めた一覧と覚えたハッシュを手放す。
    pub fn forget_while_closed(&mut self) {
        if !self.open && (self.follow.started || !matches!(self.index, IndexState::None)) {
            *self = Self {
                filter: self.filter,
                sort: self.sort,
                ..Self::default()
            };
        }
    }

    /// 起点を固定する／固定をやめる。
    pub fn toggle_pin(&mut self) {
        self.pinned = !self.pinned;
    }

    /// 1 フレーム分、ウインドウの中を描く。
    pub fn ui(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) -> Vec<Command> {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        let delay = inputs.settings.follow_delay_ms as f64 / 1000.0;
        let rules = match Rules::new(
            &inputs.settings.ignore_names,
            inputs.settings.min_file_bytes,
        ) {
            Ok(r) => r,
            Err(_) => Rules::new("", inputs.settings.min_file_bytes).expect("empty pattern"),
        };
        self.follow
            .observe(inputs.folder, inputs.slot, inputs.loading, now);
        if self.pinned {
            self.follow.pending = None;
        } else if let Some(origin) = self.follow.take(now, delay) {
            if self.origin.as_ref() != Some(&origin) {
                self.set_origin(origin, &rules);
            }
        } else if let Some(left) = self.follow.waiting(now, delay) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
        self.keep_index(&ctx, inputs, &rules);
        self.start_search(&ctx, &rules);
        self.receive();

        let mut commands = Vec::new();
        let lang = inputs.lang;
        egui::Panel::top("duplicates_bar").show(ui, |ui| self.bar(ui, inputs));
        egui::Panel::bottom("duplicates_summary").show(ui, |ui| self.summary(ui, lang));
        egui::Panel::right("duplicates_places")
            .default_size(360.0)
            .min_size(200.0)
            .show(ui, |ui| self.places(ui, lang, inputs.roots, &mut commands));
        let rest = ui.available_rect_before_wrap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
            self.list(ui, lang, &rules)
        });
        for c in &commands {
            if let Command::Go(path) = c {
                self.follow.navigating(path.clone());
            }
        }
        commands
    }

    fn set_origin(&mut self, origin: PathBuf, rules: &Rules) {
        self.running = None;
        self.waiting_jobs.clear();
        self.selected = Some(0);
        self.origin = Some(origin.clone());
        self.items = vec![Item {
            is_dir: origin.is_dir(),
            path: origin.clone(),
            depth: 0,
            parent: None,
            bytes: None,
            outcome: None,
            expanded: true,
            listed: false,
        }];
        self.waiting_jobs.push((0, origin));
        if self.items[0].is_dir {
            self.list_children(0, rules);
        }
    }

    fn list_children(&mut self, at: usize, rules: &Rules) {
        if self.items[at].listed {
            return;
        }
        self.items[at].listed = true;
        let children = match crate::duplicates::children(&self.items[at].path, rules) {
            Ok(c) => c,
            Err(e) => {
                self.problems.push(format!(
                    "Cannot read {}: {e}",
                    self.items[at].path.display()
                ));
                return;
            }
        };
        let depth = self.items[at].depth + 1;
        for path in children {
            let i = self.items.len();
            self.items.push(Item {
                is_dir: path.is_dir(),
                path: path.clone(),
                depth,
                parent: Some(at),
                bytes: None,
                outcome: None,
                expanded: false,
                listed: false,
            });
            self.ask(i, path);
        }
    }

    fn ask(&mut self, i: usize, path: PathBuf) {
        match &mut self.running {
            Some(r) if r.jobs.send((i, path.clone())).is_ok() => r.asked += 1,
            _ => self.waiting_jobs.push((i, path)),
        }
    }

    fn keep_index(&mut self, ctx: &egui::Context, inputs: &Inputs<'_>, rules: &Rules) {
        let key = IndexKey {
            roots: inputs.roots.to_vec(),
            ignore_names: inputs.settings.ignore_names.clone(),
        };
        let current = match &self.index {
            IndexState::None => None,
            IndexState::Building { key, .. } | IndexState::Ready { key, .. } => Some(key),
        };
        if current != Some(&key) {
            self.rebuild_index(ctx, key, rules);
            return;
        }
        if let IndexState::Building { rx, key, .. } = &self.index {
            match rx.try_recv() {
                Ok(Ok(index)) => {
                    for p in &index.problems {
                        self.problems.push(p.clone());
                    }
                    self.index = IndexState::Ready {
                        key: key.clone(),
                        index: Arc::new(index),
                    };
                }
                Ok(Err(Cancelled)) | Err(TryRecvError::Disconnected) => {
                    self.index = IndexState::None;
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(REFRESH),
            }
        }
    }

    fn rebuild_index(&mut self, ctx: &egui::Context, key: IndexKey, rules: &Rules) {
        if let IndexState::Building { progress, .. } = &self.index {
            progress.cancel.store(true, Ordering::Relaxed);
        }
        // 一覧が変わると前の答えは当てにならないので、起点から調べ直す。
        if let Some(origin) = self.origin.clone() {
            self.set_origin(origin, rules);
        }
        let progress = Arc::new(Progress::default());
        let roots: Vec<PathBuf> = key.roots.iter().map(PathBuf::from).collect();
        let rx = {
            let (progress, rules) = (progress.clone(), rules.clone());
            crate::background::compute(ctx, move || Index::build(&roots, &rules, &progress))
        };
        self.index = IndexState::Building { key, rx, progress };
    }

    fn start_search(&mut self, ctx: &egui::Context, rules: &Rules) {
        if self.running.is_some() || self.waiting_jobs.is_empty() {
            return;
        }
        let IndexState::Ready { index, .. } = &self.index else {
            return;
        };
        let Some(origin) = self.origin.clone() else {
            return;
        };
        let (jobs, job_rx) = channel::<Job>();
        let (answer_tx, answers) = channel::<Answer>();
        let progress = Arc::new(Progress::default());
        {
            let (index, hashes, rules, progress) = (
                index.clone(),
                self.hashes.clone(),
                rules.clone(),
                progress.clone(),
            );
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let search = Search {
                    index: &index,
                    hashes: &hashes,
                    rules: &rules,
                    progress: &progress,
                    origin: &origin,
                };
                while let Ok((i, path)) = job_rx.recv() {
                    let Ok(outcome) = search.find(&path) else {
                        return;
                    };
                    let bytes = crate::duplicates::size_of(&path, &rules);
                    if answer_tx.send((i, bytes, outcome)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        }
        let mut running = Running {
            jobs,
            answers,
            progress,
            asked: 0,
            answered: 0,
        };
        for job in self.waiting_jobs.drain(..) {
            if running.jobs.send(job).is_ok() {
                running.asked += 1;
            }
        }
        self.running = Some(running);
    }

    fn receive(&mut self) {
        let Some(r) = &mut self.running else {
            return;
        };
        while let Ok((i, bytes, outcome)) = r.answers.try_recv() {
            r.answered += 1;
            if let Some(item) = self.items.get_mut(i) {
                if let Outcome::Failed(e) = &outcome {
                    self.problems
                        .push(format!("Cannot compare {}: {e}", item.path.display()));
                }
                item.bytes = Some(bytes);
                item.outcome = Some(outcome);
            }
        }
    }

    fn searching(&self) -> Option<&Running> {
        self.running.as_ref().filter(|r| r.answered < r.asked)
    }

    fn stop(&mut self) {
        if let Some(r) = &self.running {
            r.progress.cancel.store(true, Ordering::Relaxed);
        }
        self.running = None;
    }

    fn bar(&mut self, ui: &mut egui::Ui, inputs: &Inputs<'_>) {
        let lang = inputs.lang;
        ui.horizontal(|ui| {
            ui.label(tr(lang, "Origin"));
            match self.items.first() {
                Some(origin) => {
                    let mark = if origin.is_dir {
                        icon::FOLDER
                    } else {
                        icon::FILE
                    };
                    ui.label(format!("{mark} {}", origin.path.display()));
                }
                None => {
                    ui.weak(tr(
                        lang,
                        "Click a folder in the tree or choose a file in a slot.",
                    ));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let pin = ui
                    .add(egui::Button::selectable(
                        self.pinned,
                        format!("{} {}", icon::PUSH_PIN, tr(lang, "Pin")),
                    ))
                    .on_hover_text(tr(
                        lang,
                        "Keep this origin while you click other folders and files.",
                    ));
                if pin.clicked() {
                    self.toggle_pin();
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label(tr(lang, "Search in"));
            if inputs.roots.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    tr(
                        lang,
                        "No dataset folders. Add them to Dataset folders (dataset.roots) in the settings.",
                    ),
                );
                return;
            }
            let files = match &self.index {
                IndexState::Ready { index, .. } => thousands(index.file_count()),
                IndexState::Building { progress, .. } => {
                    thousands(progress.listed.load(Ordering::Relaxed))
                }
                IndexState::None => "-".into(),
            };
            ui.label(format!(
                "{} · {}",
                tr1(lang, "{} dataset folders", &inputs.roots.len().to_string()),
                tr1(lang, "{} files", &files)
            ))
            .on_hover_text(inputs.roots.join("\n"));
            let reload = ui
                .button(icon::ARROW_CLOCKWISE)
                .on_hover_text(tr(lang, "Read the dataset folders again"));
            if reload.clicked() {
                // 鍵を変えると、次のフレームで作り直す。
                self.index = IndexState::None;
                self.hashes = Arc::default();
            }
        });
        ui.horizontal(|ui| {
            if let IndexState::Building { progress, .. } = &self.index {
                ui.spinner();
                ui.weak(tr1(
                    lang,
                    "Listing the dataset files: {}",
                    &thousands(progress.listed.load(Ordering::Relaxed)),
                ));
            } else if let Some(r) = self.searching() {
                ui.spinner();
                ui.weak(format!(
                    "{} {} / {} · {}",
                    tr(lang, "Comparing"),
                    r.answered,
                    r.asked,
                    tr1(
                        lang,
                        "{} read",
                        &bytes_text(r.progress.hashed_bytes.load(Ordering::Relaxed))
                    ),
                ));
                if ui
                    .button(icon::STOP)
                    .on_hover_text(tr(lang, "Stop comparing"))
                    .clicked()
                {
                    self.stop();
                }
            } else {
                ui.weak(" ");
            }
        });
    }

    /// 表示する順の行（並べ替えと絞り込みのあと）。親の行の下に子の行が続く。
    fn rows(&self) -> Vec<usize> {
        let mut children: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
        for (i, item) in self.items.iter().enumerate() {
            children.entry(item.parent).or_default().push(i);
        }
        let (column, ascending) = self.sort;
        for list in children.values_mut() {
            list.sort_by(|&a, &b| {
                let (a, b) = (&self.items[a], &self.items[b]);
                let order = match column {
                    Column::Name => b.is_dir.cmp(&a.is_dir).then(a.path.cmp(&b.path)),
                    Column::Result => found_count(a).cmp(&found_count(b)),
                    Column::Size => a.bytes.cmp(&b.bytes),
                };
                if ascending { order } else { order.reverse() }
            });
        }
        let mut out = Vec::new();
        let mut stack: Vec<usize> = children.get(&None).cloned().unwrap_or_default();
        stack.reverse();
        while let Some(i) = stack.pop() {
            let item = &self.items[i];
            if i == 0 || self.passes(item) {
                out.push(i);
            }
            if item.expanded
                && let Some(kids) = children.get(&Some(i))
            {
                stack.extend(kids.iter().rev());
            }
        }
        out
    }

    fn passes(&self, item: &Item) -> bool {
        match (self.filter, &item.outcome) {
            (Filter::All, _) => true,
            (Filter::Found, Some(Outcome::Found(f))) => !f.is_empty(),
            (Filter::Missing, Some(Outcome::Found(f))) => f.is_empty(),
            _ => false,
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, lang: Language, rules: &Rules) {
        ui.horizontal(|ui| {
            for (filter, title) in [
                (Filter::All, "All"),
                (Filter::Found, "Found only"),
                (Filter::Missing, "Not found only"),
            ] {
                ui.selectable_value(&mut self.filter, filter, tr(lang, title));
            }
        });
        let rows = self.rows();
        let mut expand = None;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("duplicates_items")
                .striped(true)
                .spacing(vec2(16.0, 2.0))
                .show(ui, |ui| {
                    for (column, title) in [
                        (Column::Name, "In the origin"),
                        (Column::Result, "Result"),
                        (Column::Size, "Size"),
                    ] {
                        let mark = match self.sort {
                            (c, true) if c == column => " ⏶",
                            (c, false) if c == column => " ⏷",
                            _ => "",
                        };
                        let text = format!("{}{mark}", tr(lang, title));
                        if ui.add(egui::Button::new(text).frame(false)).clicked() {
                            self.sort = if self.sort.0 == column {
                                (column, !self.sort.1)
                            } else {
                                (column, true)
                            };
                        }
                    }
                    ui.end_row();
                    for i in rows {
                        let item = &self.items[i];
                        ui.horizontal(|ui| {
                            ui.add_space(item.depth.saturating_sub(1) as f32 * 16.0);
                            let empty = matches!(item.outcome, Some(Outcome::Empty));
                            if item.is_dir && i != 0 && !empty {
                                let caret = if item.expanded {
                                    icon::CARET_DOWN
                                } else {
                                    icon::CARET_RIGHT
                                };
                                if ui.add(egui::Button::new(caret).frame(false)).clicked() {
                                    expand = Some(i);
                                }
                            } else if i != 0 {
                                ui.add_space(18.0);
                            }
                            let mark = if item.is_dir {
                                icon::FOLDER
                            } else {
                                icon::FILE
                            };
                            let mut name = format!("{mark} {}", file_name(&item.path));
                            if i == 0 {
                                name = format!("{name}  ({})", tr(lang, "origin"));
                            }
                            let r =
                                ui.add(egui::Button::selectable(self.selected == Some(i), name));
                            if r.clicked() {
                                self.selected = Some(i);
                            }
                            r.context_menu(|ui| {
                                if ui.button(tr(lang, "Copy path")).clicked() {
                                    ui.ctx().copy_text(item.path.display().to_string());
                                    ui.close();
                                }
                            });
                        });
                        ui.label(outcome_text(lang, item.outcome.as_ref(), ui.visuals()));
                        ui.label(item.bytes.map(bytes_text).unwrap_or_default());
                        ui.end_row();
                    }
                });
        });
        if let Some(i) = expand {
            self.items[i].expanded = !self.items[i].expanded;
            self.list_children(i, rules);
        }
    }

    fn places(
        &self,
        ui: &mut egui::Ui,
        lang: Language,
        roots: &[String],
        commands: &mut Vec<Command>,
    ) {
        let Some(item) = self.selected.and_then(|i| self.items.get(i)) else {
            return;
        };
        let found = match &item.outcome {
            Some(Outcome::Found(f)) => f,
            other => {
                ui.label(outcome_text(lang, other.as_ref(), ui.visuals()));
                return;
            }
        };
        ui.label(format!(
            "{}  ({})",
            tr1(lang, "Same as \"{}\"", &file_name(&item.path)),
            tr1(lang, "{} places", &found.len().to_string())
        ));
        if found.is_empty() {
            ui.weak(tr(lang, "Nothing the same is in the dataset folders."));
            return;
        }
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            for place in found {
                let r = ui
                    .add(
                        egui::Label::new(shown_place(roots, place))
                            .extend()
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text(format!(
                        "{}\n{}",
                        place.display(),
                        tr(lang, "Click to go there.")
                    ));
                if r.clicked() {
                    commands.push(Command::Go(place.clone()));
                }
                r.context_menu(|ui| {
                    if ui.button(tr(lang, "Open in file manager")).clicked() {
                        let dir = if place.is_dir() {
                            place.clone()
                        } else {
                            place
                                .parent()
                                .map_or_else(|| place.clone(), Path::to_path_buf)
                        };
                        commands.push(Command::OpenInFileManager(dir));
                        ui.close();
                    }
                    if ui.button(tr(lang, "Copy path")).clicked() {
                        ui.ctx().copy_text(place.display().to_string());
                        ui.close();
                    }
                });
            }
        });
    }

    fn summary(&self, ui: &mut egui::Ui, lang: Language) {
        let done: Vec<&Item> = self.items.iter().filter(|i| i.outcome.is_some()).collect();
        let found = done.iter().filter(|i| found_count(i) > 0).count();
        let missing = done
            .iter()
            .filter(|i| matches!(&i.outcome, Some(Outcome::Found(f)) if f.is_empty()))
            .count();
        ui.horizontal(|ui| {
            ui.weak(format!(
                "{} · {} · {}",
                tr1(lang, "{} items", &self.items.len().to_string()),
                tr1(lang, "found {}", &found.to_string()),
                tr1(lang, "not found {}", &missing.to_string()),
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let copy = ui
                    .add_enabled(!self.items.is_empty(), egui::Button::new(format!(
                        "{} {}",
                        icon::COPY,
                        tr(lang, "Copy the list")
                    )))
                    .on_hover_text(tr(
                        lang,
                        "Copy every item and the places found, separated by tabs, to paste into a spreadsheet.",
                    ));
                if copy.clicked() {
                    ui.ctx().copy_text(list_text(&self.items, lang));
                }
            });
        });
    }
}

/// 進み具合を覗く間隔。
const REFRESH: std::time::Duration = std::time::Duration::from_millis(200);

fn found_count(item: &Item) -> usize {
    match &item.outcome {
        Some(Outcome::Found(f)) => f.len(),
        _ => 0,
    }
}

fn outcome_text(
    lang: Language,
    outcome: Option<&Outcome>,
    visuals: &egui::Visuals,
) -> egui::RichText {
    match outcome {
        None => egui::RichText::new("…").weak(),
        Some(Outcome::Found(f)) if f.is_empty() => egui::RichText::new(tr(lang, "not found")),
        Some(Outcome::Found(f)) => egui::RichText::new(tr1(lang, "found {}", &f.len().to_string()))
            .color(visuals.warn_fg_color),
        Some(Outcome::Empty) => egui::RichText::new(tr(lang, "empty folder")).weak(),
        Some(Outcome::Small) => egui::RichText::new(tr(lang, "too small to compare")).weak(),
        Some(Outcome::Skipped) => egui::RichText::new(tr(lang, "not compared")).weak(),
        Some(Outcome::Failed(_)) => {
            egui::RichText::new(tr(lang, "cannot read")).color(visuals.error_fg_color)
        }
    }
}

/// 「一覧をコピー」の中身。1 行に項目と見つかった場所を 1 つずつ、タブで区切る。
fn list_text(items: &[Item], lang: Language) -> String {
    let mut out = format!(
        "{}\t{}\t{}\n",
        tr(lang, "In the origin"),
        tr(lang, "Result"),
        tr(lang, "Same thing in the dataset folders")
    );
    for item in items {
        let path = item.path.display();
        match &item.outcome {
            Some(Outcome::Found(f)) if !f.is_empty() => {
                for place in f {
                    out.push_str(&format!(
                        "{path}\t{}\t{}\n",
                        tr(lang, "found"),
                        place.display()
                    ));
                }
            }
            other => {
                let text = outcome_text(lang, other.as_ref(), &egui::Visuals::dark());
                out.push_str(&format!("{path}\t{}\t\n", text.text()));
            }
        }
    }
    out
}

/// 見つかった場所を、それがあるデータセットのフォルダの名前から書く。全部のパスだと右の欄で
/// 折り返して、どこが違うのかが読みにくいため（全部のパスはマウスを置くと出す）。
fn shown_place(roots: &[String], place: &Path) -> String {
    roots
        .iter()
        .map(Path::new)
        .filter(|r| place.starts_with(r))
        .max_by_key(|r| r.components().count())
        .and_then(|root| {
            let rest = place.strip_prefix(root).ok()?;
            let name = root.file_name()?.to_string_lossy();
            Some(Path::new(name.as_ref()).join(rest).display().to_string())
        })
        .unwrap_or_else(|| place.display().to_string())
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn bytes_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn started(folder: &str) -> Follow {
        let mut f = Follow::default();
        f.observe(Some(&p(folder)), (0, None), false, 0.0);
        assert_eq!(f.take(0.0, 0.3), Some(p(folder)));
        f
    }

    #[test]
    fn opening_takes_the_current_folder_as_origin() {
        started("/data/in");
    }

    #[test]
    fn clicking_a_folder_in_the_tree_makes_it_the_origin_after_the_delay() {
        let mut f = started("/data/in");
        f.observe(Some(&p("/data/other")), (0, None), true, 1.0);
        assert_eq!(f.take(1.1, 0.3), None);
        assert_eq!(f.take(1.3, 0.3), Some(p("/data/other")));
    }

    #[test]
    fn slots_filled_while_the_folder_loads_do_not_change_the_origin() {
        let mut f = started("/data/in");
        f.observe(Some(&p("/data/other")), (0, None), true, 1.0);
        f.observe(
            Some(&p("/data/other")),
            (0, Some(&p("/data/other/a.png"))),
            true,
            1.1,
        );
        assert_eq!(f.take(2.0, 0.3), Some(p("/data/other")));
    }

    #[test]
    fn choosing_a_slot_makes_its_file_the_origin() {
        let mut f = started("/data/in");
        f.observe(
            Some(&p("/data/in")),
            (2, Some(&p("/data/in/b.png"))),
            false,
            1.0,
        );
        assert_eq!(f.take(2.0, 0.3), Some(p("/data/in/b.png")));
    }

    #[test]
    fn a_frame_of_a_file_makes_the_file_the_origin() {
        let mut f = started("/data/in");
        f.observe(
            Some(&p("/data/in")),
            (1, Some(&p("/data/in/c.dcm?frame=3"))),
            false,
            1.0,
        );
        assert_eq!(f.take(2.0, 0.3), Some(p("/data/in/c.dcm")));
    }

    #[test]
    fn going_to_a_place_found_keeps_the_origin() {
        let mut f = started("/data/in");
        f.navigating(p("/sets/old/a.png"));
        f.observe(Some(&p("/sets/old")), (0, None), true, 1.0);
        f.observe(
            Some(&p("/sets/old")),
            (3, Some(&p("/sets/old/a.png"))),
            false,
            1.5,
        );
        assert_eq!(f.take(3.0, 0.3), None);
    }

    #[test]
    fn choosing_something_else_after_going_there_changes_the_origin_again() {
        let mut f = started("/data/in");
        f.navigating(p("/sets/old"));
        f.observe(Some(&p("/sets/old")), (0, None), false, 1.0);
        f.observe(Some(&p("/sets/new")), (0, None), false, 2.0);
        assert_eq!(f.take(3.0, 0.3), Some(p("/sets/new")));
    }

    #[test]
    fn the_copied_list_has_one_line_per_place_found() {
        let item = |path: &str, outcome| Item {
            path: p(path),
            is_dir: false,
            depth: 1,
            parent: Some(0),
            bytes: Some(1),
            outcome: Some(outcome),
            expanded: false,
            listed: false,
        };
        let items = [
            item(
                "/in/a.png",
                Outcome::Found(vec![p("/s/1.png"), p("/s/2.png")]),
            ),
            item("/in/b.png", Outcome::Found(vec![])),
        ];
        let text = list_text(&items, Language::English);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "In the origin\tResult\tSame thing in the dataset folders",
                "/in/a.png\tfound\t/s/1.png",
                "/in/a.png\tfound\t/s/2.png",
                "/in/b.png\tnot found\t",
            ]
        );
    }

    #[test]
    fn places_are_written_from_the_dataset_folder_name() {
        let roots = ["/mnt/sets".to_owned(), "/mnt/sets/inner".to_owned()];
        assert_eq!(
            shown_place(&roots, &p("/mnt/sets/inner/day1")),
            Path::new("inner").join("day1").display().to_string()
        );
        assert_eq!(shown_place(&roots, &p("/other/x")), "/other/x");
    }

    #[test]
    fn sizes_are_written_with_units() {
        assert_eq!(bytes_text(512), "512 B");
        assert_eq!(bytes_text(1536), "1.5 KB");
        assert_eq!(bytes_text(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
