//! フォルダの読み込み。cd、Images モードの自動読み込み、ZIP のマウント、フォルダ監視の反映。

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui;
use vw_core::Registry;

use crate::archive::{self};
use crate::autoload::{FolderLoad, Question};
use crate::export::Matcher;
use crate::frames;
use crate::i18n::{tr, tr1};
use crate::loader::Job;
use crate::series::Scan;
use crate::settings::LoadInto;
use crate::tree::{self};
use crate::viewer::{Sequence, SequenceKind, Slot};
use crate::watch::{self, ChangeKind, FolderWatcher};

use super::*;

impl VwApp {
    /// `index` 番目のスロットで `path` を読み込み始める。
    pub(super) fn start_load(&mut self, index: usize, path: PathBuf, full: bool) {
        let request = self.next_id;
        self.next_id += 1;
        self.cancel_requests(&self.slots[index]);
        let slot = &mut self.slots[index];
        slot.path = Some(path.clone());
        slot.request = request;
        slot.loading = true;
        slot.error = None;
        slot.full_request = None;
        slot.copy_when_full = false;
        self.loader.request(Job {
            slot: slot.id,
            request,
            path,
            thumb: true,
            full,
            develop: false,
            fit_side: None,
            look: Some(look_of(&self.color_rules, slot)),
        });
    }

    /// フォルダ `path` へ移る。スロットを閉じて、中のサブフォルダと ZIP をフォルダのスロットとして並べ、
    /// DICOM のシリーズと画像をその後ろに読み込んでいく。前の読み込みが続いていれば、そこで打ち切る。
    pub(super) fn cd(&mut self, ctx: &egui::Context, path: PathBuf, into: LoadInto) {
        let tree = self.settings().tree.clone();
        let registry = self.loader.registry().clone();
        self.finish_folder_load();
        self.close_all_slots();
        self.tree.reveal(&path, &registry);
        self.current = Some(path.clone());
        self.load_into = into;
        self.loaded_folders.push((path.clone(), into));
        let listing = tree::list_folder(&path, &registry, tree.order());
        self.warn_unsupported_archives(&path);
        for folder in listing.folders.into_iter().chain(listing.archives) {
            let index = self.push_slot();
            self.slots[index].folder = Some(folder);
        }
        self.scan_series(ctx, &path, &listing.images);
        self.group_series();
        self.active = 0;
        let last = &self.slots[self.slots.len() - 1];
        self.load_slots_from = if is_blank(last) {
            last.id
        } else {
            self.next_id
        };
        self.log.info(format!("Opened folder {}", path.display()));
        if !self.resume_declined.contains(&path) {
            self.resume_offer = crate::catalog::unfinished(&path).map(|u| (path.clone(), u.done));
        }
        let ctx = ctx.clone();
        self.folder_load = Some(FolderLoad::start(
            path,
            registry,
            tree.order(),
            false,
            tree.confirm_every,
            tree.confirm_chunk as usize,
            move || ctx.request_repaint(),
        ));
    }

    /// 末尾の空のスロットを使うか、なければ足して、その添字を返す。
    pub(super) fn push_slot(&mut self) -> usize {
        if !is_blank(&self.slots[self.slots.len() - 1]) {
            self.slots.push(Slot::new(self.next_id));
            self.next_id += 1;
        }
        self.slots.len() - 1
    }

    /// `dir` の DICOM らしいファイルのヘッダを、まだ読んでいなければ読み始める。
    pub(super) fn scan_series(&mut self, ctx: &egui::Context, dir: &Path, images: &[PathBuf]) {
        if !matches!(self.series.scan(dir), Scan::NotStarted) {
            return;
        }
        let registry = self.loader.registry().clone();
        let extensions = tree::dicom_extensions(&registry);
        let files: Vec<PathBuf> = images
            .iter()
            .filter(|p| tree::may_be_dicom(&registry, extensions, p))
            .cloned()
            .collect();
        if files.is_empty() {
            return;
        }
        let ctx = ctx.clone();
        self.series
            .start(dir.to_path_buf(), files, registry, move || {
                ctx.request_repaint()
            });
    }

    /// cd したフォルダの DICOM のシリーズ（2 枚以上のもの）。ヘッダを読み終えていなければ空。
    pub(super) fn current_series(&self) -> Vec<crate::series::Series> {
        let Some(dir) = &self.current else {
            return Vec::new();
        };
        match self.series.scan(dir) {
            Scan::Done(studies) => studies
                .iter()
                .flat_map(|s| s.series.iter())
                .filter(|q| q.files.len() > 1)
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// cd したフォルダの DICOM のシリーズを、1 シリーズ 1 スロットにまとめる。
    /// シリーズの画像を 1 枚ずつ並べたスロットは閉じる。CT は 1 シリーズが数百枚あり、
    /// 1 枚 1 スロットではスロットがそれで埋まってしまうため。
    ///
    /// ヘッダを読み終えるのは画像を並べ始めた後のことが多いので、読み終えたときにも呼ぶ。
    pub(super) fn group_series(&mut self) {
        let Some(dir) = self.current.clone() else {
            return;
        };
        let active_id = self.slots[self.active].id;
        let mut first_new = None;
        for series in self.current_series() {
            let has_slot = self.slots.iter().any(|s| {
                s.sequence
                    .as_ref()
                    .is_some_and(|q| q.kind == SequenceKind::Series && q.paths == series.files)
            });
            if has_slot {
                continue;
            }
            for index in (0..self.slots.len()).rev() {
                let slot = &self.slots[index];
                if slot.from_folder && slot.path.as_ref().is_some_and(|p| series.files.contains(p))
                {
                    self.close_slot(index);
                }
            }
            // フォルダのスロットと、先に作ったシリーズのスロットの後ろに挟む。
            let at = self
                .slots
                .iter()
                .rposition(|s| s.folder.is_some() || s.sequence.is_some())
                .map_or(0, |i| i + 1);
            if !self.slots.get(at).is_some_and(is_blank) {
                self.insert_folder_slot(at);
            }
            let slot = &mut self.slots[at];
            slot.from_folder = false;
            let first = series.files[0].clone();
            slot.sequence = Some(Sequence::series(dir.clone(), series.title(), series.files));
            let full = self.cell_needs_full();
            self.start_load(at, first, full);
            first_new.get_or_insert(self.slots[at].id);
        }
        // 見ていた画像のスロットがシリーズにまとめられて消えたら、最初のシリーズを見せる。
        let alive = |id| self.slots.iter().position(|s| s.id == id);
        if alive(active_id).is_none() {
            if let Some(index) = first_new.and_then(alive) {
                self.active = index;
            }
        }
    }

    /// フォルダのスロットを開く。サブフォルダまで含めた画像をそのスロットに読み込む
    /// （サブフォルダがあれば先に確認する）。ZIP はその中へ移る。
    pub(super) fn explore(&mut self, ctx: &egui::Context, index: usize) {
        let Some(folder) = self.slots[index].folder.clone() else {
            return;
        };
        if archive::is_archive(&folder) {
            self.open_folder(ctx, folder);
            return;
        }
        let tree = self.settings().tree.clone();
        let slot = &mut self.slots[index];
        slot.sequence = Some(Sequence::subtree(folder.clone()));
        slot.loading = true;
        let id = slot.id;
        let ctx = ctx.clone();
        let load = FolderLoad::start(
            folder,
            self.loader.registry().clone(),
            tree.order(),
            true,
            tree.confirm_every,
            tree.confirm_chunk as usize,
            move || ctx.request_repaint(),
        );
        self.explores.push((id, load));
    }

    /// フォルダ（または ZIP）の中へ移る。ZIP は展開してから移る。
    pub(super) fn open_folder(&mut self, ctx: &egui::Context, folder: PathBuf) {
        if archive::is_archive(&folder) {
            self.mount(ctx, &folder);
            return;
        }
        let tree = &self.settings().tree;
        let other = ctx.input(|i| tree.other_load_modifier.held(i.modifiers));
        let into = if other {
            tree.load_into.other()
        } else {
            tree.load_into
        };
        self.cd(ctx, folder, into);
    }

    /// 1 つ上のフォルダへ移る。展開した ZIP のルートからは、ZIP のあるフォルダへ戻る。
    pub(super) fn parent_folder(&mut self, ctx: &egui::Context) {
        let Some(current) = self.current.clone() else {
            return;
        };
        let parent = match self.archives.mount_of(&current) {
            Some(mount) if mount.dir == current => mount.zip.parent().map(Path::to_path_buf),
            _ => current.parent().map(Path::to_path_buf),
        };
        if let Some(parent) = parent {
            let into = self.settings().tree.load_into;
            self.cd(ctx, parent, into);
        }
    }

    /// 読み込み中のフォルダで見つかった画像を、末尾のスロットに足していく。
    pub(super) fn pump_folder_load(&mut self) {
        let Some(load) = &mut self.folder_load else {
            return;
        };
        let paths = load.take_ready();
        let finished = load.finished();
        let root = load.root.clone();
        let matcher = Matcher {
            filter: &self.filter,
            labels: &self.labels,
        };
        let in_series: HashSet<PathBuf> = if paths.is_empty() {
            HashSet::new()
        } else {
            self.current_series()
                .into_iter()
                .flat_map(|q| q.files)
                .collect()
        };
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| matcher.matches(p) && !in_series.contains(p))
            .collect();
        if !paths.is_empty() {
            // 先にスロットを全部足してから枠の大きさを見積もる。1 枚ずつ見積もると、
            // スロットが少ない最初のうちだけフル解像度を読んでしまうため。
            let mut reuse_last = is_blank(&self.slots[self.slots.len() - 1]);
            let mut targets = Vec::with_capacity(paths.len());
            for path in paths {
                let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
                let chunk = self.load_into == LoadInto::Folders || folder != root;
                if chunk {
                    let last = self.slots.last_mut().expect("there is always a slot");
                    if let Some(seq) = last
                        .sequence
                        .as_mut()
                        .filter(|s| s.folder == folder && last.id >= self.load_slots_from)
                    {
                        seq.paths.push(path);
                        continue;
                    }
                }
                if !std::mem::take(&mut reuse_last) {
                    self.slots.push(Slot::new(self.next_id));
                    self.next_id += 1;
                }
                let index = self.slots.len() - 1;
                self.slots[index].from_folder = !chunk;
                if chunk {
                    self.slots[index].sequence = Some(Sequence::new(folder, path.clone()));
                }
                targets.push((index, path));
            }
            let full = self.cell_needs_full();
            for (index, path) in targets {
                self.start_load(index, path, full);
            }
        }
        if finished {
            self.finish_folder_load();
        }
    }

    /// 開いたフォルダのスロットに、見つかった画像を足していく。
    pub(super) fn pump_explores(&mut self) {
        let full = self.cell_needs_full();
        let mut i = 0;
        while i < self.explores.len() {
            let (id, load) = &mut self.explores[i];
            let id = *id;
            let paths = load.take_ready();
            let finished = load.finished();
            let Some(index) = self.slots.iter().position(|s| s.id == id) else {
                self.explores.remove(i);
                continue;
            };
            let matcher = self.matcher();
            let slot = &self.slots[index];
            let first = (slot.path.is_none())
                .then(|| paths.iter().position(|p| matcher.matches(p)))
                .flatten();
            let seq = self.slots[index]
                .sequence
                .as_mut()
                .expect("explored slots have a sequence");
            let offset = seq.paths.len();
            seq.paths.extend(paths);
            if let Some(at) = first {
                seq.index = offset + at;
                let path = seq.paths[offset + at].clone();
                self.start_load(index, path, full);
            }
            if finished {
                let (_, load) = self.explores.remove(i);
                let slot = &mut self.slots[index];
                if slot.path.is_none() {
                    // 合う画像がなかった。読み込み中のまま残さない。
                    slot.loading = false;
                    if slot.sequence.as_ref().is_some_and(|s| s.paths.is_empty()) {
                        // 画像が 1 枚もないフォルダは、空の並び（「1/0」）ではなくフォルダの姿に戻す。
                        slot.sequence = None;
                    }
                }
                self.log.info(format!(
                    "Loaded {} images from {}",
                    load.loaded,
                    load.root.display()
                ));
            } else {
                i += 1;
            }
        }
    }

    pub(super) fn finish_folder_load(&mut self) {
        if let Some(load) = self.folder_load.take() {
            self.log.info(format!(
                "Loaded {} images from {}",
                load.loaded,
                load.root.display()
            ));
        }
    }

    pub(super) fn folder_load_dialog(&mut self, ctx: &egui::Context) {
        // cd したフォルダの読み込みを先に、次に開いたフォルダのスロットを開いた順に尋ねる。
        let asking = if self
            .folder_load
            .as_ref()
            .and_then(FolderLoad::question)
            .is_some()
        {
            Some(None)
        } else {
            self.explores
                .iter()
                .position(|(_, l)| l.question().is_some())
                .map(Some)
        };
        let Some(which) = asking else {
            return;
        };
        let load = match which {
            None => self.folder_load.as_ref(),
            Some(i) => Some(&self.explores[i].1),
        };
        let Some(load) = load else {
            return;
        };
        let Some(question) = load.question() else {
            return;
        };
        let lang = self.lang();
        let name = load.root.file_name().map_or_else(
            || load.root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let chunk = self.settings().tree.confirm_chunk;
        let (message, yes, no) = match question {
            Question::Subfolders => (
                tr1(
                    lang,
                    "\"{}\" has subfolders. Load the images in all of them too?",
                    &name,
                ),
                tr(lang, "Load subfolders"),
                tr(lang, "This folder only"),
            ),
            Question::More => (
                tr1(
                    lang,
                    "{} images loaded. Load the next {}?",
                    &load.loaded.to_string(),
                )
                .replacen("{}", &chunk.to_string(), 1),
                tr(lang, "Load more"),
                tr(lang, "Stop"),
            ),
        };
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("folder_load_question")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.label(message);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(yes).clicked() {
                    answer = Some(true);
                }
                if ui.button(no).clicked() {
                    answer = Some(false);
                }
            });
        });
        // Esc や枠の外のクリックは、続けない方の答えとして扱う。
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        let Some(yes) = answer else {
            return;
        };
        let load = match which {
            None => self.folder_load.as_mut(),
            Some(i) => Some(&mut self.explores[i].1),
        };
        let Some(load) = load else {
            return;
        };
        match question {
            Question::Subfolders => load.answer_subfolders(yes),
            Question::More => {
                if !load.answer_more(yes) {
                    match which {
                        None => self.finish_folder_load(),
                        Some(i) => {
                            let (id, _) = self.explores.remove(i);
                            if let Some(slot) = self.slots.iter_mut().find(|s| s.id == id) {
                                slot.loading &= slot.path.is_some();
                            }
                        }
                    }
                }
            }
        }
    }

    /// 監視するフォルダ。ツリーで開いているフォルダと、スロットに出している画像のフォルダ。
    pub(super) fn watched_dirs(&self) -> HashSet<PathBuf> {
        let mut dirs: HashSet<PathBuf> = self.tree.open_paths().into_iter().collect();
        dirs.extend(self.loaded_folders.iter().map(|(p, _)| p.clone()));
        for slot in &self.slots {
            if let Some(seq) = slot
                .sequence
                .as_ref()
                .filter(|q| matches!(q.kind, SequenceKind::Folder | SequenceKind::Subtree))
            {
                dirs.insert(seq.folder.clone());
            } else if let Some(dir) = slot.path.as_deref().and_then(Path::parent) {
                dirs.insert(dir.to_path_buf());
            }
        }
        dirs
    }

    /// フォルダの変化を受け取り、ツリーとスロットに反映する。
    pub(super) fn watch_folders(&mut self, ctx: &egui::Context) {
        if !self.settings().behavior.auto_reload {
            self.watcher = None;
            self.watcher_failed = false;
            return;
        }
        if self.watcher.is_none() && !self.watcher_failed {
            match FolderWatcher::new(ctx.clone()) {
                Ok(w) => self.watcher = Some(w),
                Err(e) => {
                    self.log
                        .error(format!("Auto reload: cannot watch folders ({e})"));
                    self.watcher_failed = true;
                }
            }
        }
        let dirs = self.watched_dirs();
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        for problem in watcher.set_dirs(dirs) {
            self.log.warn(format!("Auto reload: {problem}"));
        }
        let changes = watcher.poll(Instant::now());
        if watcher.waiting() {
            ctx.request_repaint_after(watch::QUIET);
        }
        if !changes.is_empty() {
            self.apply_changes(ctx, changes);
        }
    }

    pub(super) fn apply_changes(&mut self, ctx: &egui::Context, changes: Vec<watch::Change>) {
        let registry = self.loader.registry().clone();
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut created = HashSet::new();
        let mut modified = HashSet::new();
        let mut removed = HashSet::new();
        for c in changes {
            // 閉じた ZIP の展開先を消したときの変化。画像が消えたとログに出さない。
            if self.archives.is_closed(&c.path) {
                continue;
            }
            if let Some(dir) = c.path.parent().filter(|d| !dirs.iter().any(|x| x == d)) {
                dirs.push(dir.to_path_buf());
            }
            // ラベルファイルが外で書き換えられたら（別の PC との同期など）、次に使うときに読み直す。
            if c.path
                .file_name()
                .is_some_and(|n| n == crate::labels::FILE_NAME)
            {
                if let Some(dir) = c.path.parent() {
                    self.labels.forget(dir);
                }
                continue;
            }
            if c.path
                .file_name()
                .is_some_and(|n| n == crate::ai_tags::FILE_NAME)
            {
                if let Some(dir) = c.path.parent() {
                    self.ai_tags.forget(dir);
                }
                continue;
            }
            // 消えたファイルは中身を読めないので、拡張子で決まらないものは画像だったとみなす。
            // 画像でないファイルが消えても、フォルダを読み直して件数をログに出すだけで済む。
            let image = match c.kind {
                ChangeKind::Removed => {
                    registry.supports_path(&c.path) || registry.needs_probe(&c.path)
                }
                _ => registry.supports_file(&c.path),
            };
            if !image {
                continue;
            }
            match c.kind {
                ChangeKind::Created => created.insert(c.path),
                ChangeKind::Modified => modified.insert(c.path),
                ChangeKind::Removed => removed.insert(c.path),
            };
        }
        for dir in &dirs {
            self.tree.refresh(dir, &registry);
            self.series.forget(dir);
            let count =
                |set: &HashSet<PathBuf>| set.iter().filter(|p| p.parent() == Some(dir)).count();
            let (new, gone) = (count(&created), count(&removed));
            if new + gone > 0 {
                self.log.info(format!(
                    "Auto reload: {new} new / {gone} removed image(s) in {}",
                    dir.display()
                ));
            }
        }
        if let Some(current) = self.current.clone().filter(|c| dirs.contains(c)) {
            self.sync_folder_slots(&current, &registry);
            let images =
                tree::list_folder(&current, &registry, self.settings().tree.order()).images;
            self.scan_series(ctx, &current, &images);
        }
        // フォルダを読み込み中は、スロットの並びを読み込みのほうに任せる。並びを読み直すと、
        // まだ読み込みが足していない画像と重なるため。
        if self.folder_load.is_some() {
            return;
        }
        let matcher = self.matcher();
        created.retain(|p| matcher.matches(p));
        let order = self.settings().tree.order();
        for dir in &dirs {
            self.sync_loaded_folder(dir, &created, &removed, &registry);
        }
        let playing = self.playback.as_ref().map(|pb| pb.slot);
        let one_up = self.shows_one_slot();
        let active = self.active;
        let mut siblings: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        let mut loads = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if playing == Some(slot.id) || slot.from_folder {
                continue;
            }
            let Some(current) = slot.path.clone() else {
                continue;
            };
            // フレームの並びは元のファイルの中のもの。フォルダの画像に並べ替えず、ファイルが
            // 書き換わったときに今のフレームを読み直すだけにする。
            if let Some((file, _)) = frames::split(&current) {
                if modified.contains(&file) {
                    loads.push((index, current));
                }
                continue;
            }
            // シリーズの並びは UID で決まり、フォルダの並び順とは関係ない。ファイルが
            // 書き換わったときに読み直すだけにする。
            if slot
                .sequence
                .as_ref()
                .is_some_and(|q| q.kind == SequenceKind::Series)
            {
                if modified.contains(&current) {
                    loads.push((index, current));
                }
                continue;
            }
            // 開いたフォルダのスロットはサブフォルダの画像も持つので、フォルダを読み直して並べ直す
            // ことはせず、増えた画像を後ろに足し、消えた画像を除くだけにする。
            if let Some(seq) = slot
                .sequence
                .as_mut()
                .filter(|q| q.kind == SequenceKind::Subtree)
            {
                seq.paths.retain(|p| !removed.contains(p));
                let mut new: Vec<&PathBuf> = created
                    .iter()
                    .filter(|p| p.starts_with(&seq.folder) && !seq.paths.contains(p))
                    .collect();
                new.sort();
                seq.paths.extend(new.into_iter().cloned());
                match seq.paths.iter().position(|p| *p == current) {
                    Some(at) => {
                        seq.index = at;
                        if modified.contains(&current) {
                            loads.push((index, current));
                        }
                    }
                    None if !seq.paths.is_empty() => {
                        seq.index = seq.index.min(seq.paths.len() - 1);
                        loads.push((index, seq.paths[seq.index].clone()));
                    }
                    None => {}
                }
                continue;
            }
            let Some(dir) = current.parent().filter(|d| dirs.iter().any(|x| x == d)) else {
                continue;
            };
            // 同じフォルダの画像を出しているスロットが多くても、フォルダは 1 回だけ読む。
            let paths = siblings
                .entry(dir.to_path_buf())
                .or_insert_with(|| tree::list_folder(dir, &registry, order).images);
            if paths.is_empty() {
                continue;
            }
            // 1 枚だけ表示しているときは、見ている位置に関わらず新しい画像を出す。見比べる
            // 相手がいないので、来た画像をすぐ見られるほうが役に立つ。
            let newest = (one_up && index == active)
                .then(|| paths.iter().rposition(|p| created.contains(p)))
                .flatten();
            let at = newest
                .or_else(|| watch::follow_newest(paths, &current, &created))
                .or_else(|| paths.iter().position(|p| *p == current))
                .unwrap_or_else(|| watch::position_of_removed(paths, &current, order));
            if let Some(seq) = &mut slot.sequence {
                seq.paths = paths.clone();
                seq.index = at;
            }
            let target = paths[at].clone();
            if target != current || modified.contains(&current) {
                loads.push((index, target));
            }
        }
        let full = self.cell_needs_full();
        for (index, path) in loads {
            self.start_load(index, path, full);
        }
    }

    /// `dir` の直下にある読めない形式のアーカイブ（RAR など）を、1 つにつき 1 度だけログに黄色で出す。
    /// ツリーやスロットには出さないので、置いてあるのに見えない理由が分かるようにする。
    pub(super) fn warn_unsupported_archives(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let lang = self.lang();
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| archive::is_unsupported_archive(p))
            .collect();
        found.sort();
        for path in found {
            if self.warned_archives.insert(path.clone()) {
                let path = path.display().to_string();
                self.log
                    .warn(tr1(lang, archive::UNSUPPORTED_MESSAGE, &path));
            }
        }
    }

    /// cd したフォルダのサブフォルダと ZIP のスロットを、フォルダの今の中身に合わせる。
    /// 消えたもののスロットは閉じ、新しいものはフォルダのスロットの最後に足す。
    pub(super) fn sync_folder_slots(&mut self, current: &Path, registry: &Registry) {
        let listing = tree::list_folder(current, registry, self.settings().tree.order());
        self.warn_unsupported_archives(current);
        let wanted: Vec<PathBuf> = listing
            .folders
            .into_iter()
            .chain(listing.archives)
            .collect();
        for index in (0..self.slots.len()).rev() {
            let gone = self.slots[index]
                .folder
                .as_ref()
                .is_some_and(|f| !wanted.contains(f));
            if gone {
                self.close_slot(index);
            }
        }
        for folder in wanted {
            if self
                .slots
                .iter()
                .any(|s| s.folder.as_ref() == Some(&folder))
            {
                continue;
            }
            let at = self
                .slots
                .iter()
                .rposition(|s| s.folder.is_some())
                .map_or(0, |i| i + 1);
            if self.slots.get(at).is_some_and(is_blank) {
                self.slots[at].folder = Some(folder);
            } else {
                self.insert_folder_slot(at);
                let slot = &mut self.slots[at];
                slot.from_folder = false;
                slot.folder = Some(folder);
            }
        }
    }

    /// フォルダの読み込み（1 枚 1 スロット）で並べたスロットを、フォルダの今の中身に合わせる。
    /// 消えた画像のスロットは閉じ、新しい画像はそのフォルダの最後のスロットの後ろに足す。
    /// 1 つのスロットの中身を差し替えないのは、並べて見ている他の画像が入れ替わってしまうため。
    pub(super) fn sync_loaded_folder(
        &mut self,
        dir: &Path,
        created: &HashSet<PathBuf>,
        removed: &HashSet<PathBuf>,
        registry: &Registry,
    ) {
        let in_dir =
            |s: &Slot| s.from_folder && s.path.as_deref().and_then(Path::parent) == Some(dir);
        for index in (0..self.slots.len()).rev() {
            let slot = &self.slots[index];
            if in_dir(slot) && slot.path.as_ref().is_some_and(|p| removed.contains(p)) {
                self.close_slot(index);
            }
        }
        let last = self.slots.iter().rposition(in_dir);
        let root = self
            .loaded_folders
            .iter()
            .find(|(p, _)| p == dir)
            .map(|(_, into)| *into);
        // 読み込んだ時点で画像がなかったフォルダには、まだこのフォルダのスロットがない。
        if last.is_none() && root.is_none() {
            return;
        }
        let order = self.settings().tree.order();
        let new: Vec<PathBuf> = tree::list_folder(dir, registry, order)
            .images
            .into_iter()
            .filter(|p| created.contains(p))
            .collect();
        if new.is_empty() {
            return;
        }
        if last.is_none() && root == Some(LoadInto::Folders) {
            let has_slot = self
                .slots
                .iter()
                .any(|s| s.sequence.as_ref().is_some_and(|q| q.folder == dir));
            if !has_slot {
                self.add_folder_slot(dir, registry);
            }
            return;
        }
        // 最後の画像か、まだ何も出していないスロットを見ていたなら、新しい画像に移る。
        let follow = match last {
            Some(last) => self.active == last || self.shows_one_slot(),
            None => is_blank(&self.slots[self.active]),
        };
        let followed_id = self.slots[self.active].id;
        let mut index = last.map_or(self.slots.len(), |l| l + 1);
        // 読み込みで使い回すのと同じく、末尾の空のスロットは新しい画像に使う。
        if last.is_none() && is_blank(&self.slots[self.slots.len() - 1]) {
            index -= 1;
            self.slots[index].from_folder = true;
        } else {
            self.insert_folder_slot(index);
        }
        let full = self.cell_needs_full();
        for (i, path) in new.into_iter().enumerate() {
            if i > 0 {
                index += 1;
                self.insert_folder_slot(index);
            }
            self.start_load(index, path, full);
        }
        if follow {
            self.active = index;
            if self.maximized == Some(followed_id) {
                self.maximized = Some(self.slots[index].id);
            }
        }
    }

    /// 1 枚 1 スロットの読み込みのスロットを `index` に挟む。
    pub(super) fn insert_folder_slot(&mut self, index: usize) {
        let mut slot = Slot::new(self.next_id);
        self.next_id += 1;
        slot.from_folder = true;
        self.slots.insert(index, slot);
        if self.active >= index && self.active + 1 < self.slots.len() {
            self.active += 1;
        }
    }

    /// 読み込んだ時点で空だったフォルダに画像が来たら、フォルダスロットを作って最新の画像を出す。
    pub(super) fn add_folder_slot(&mut self, dir: &Path, registry: &Registry) {
        let order = self.settings().tree.order();
        let paths = tree::list_folder(dir, registry, order).images;
        let Some(newest) = paths.last().cloned() else {
            return;
        };
        let follow = is_blank(&self.slots[self.active]);
        let index = self.push_slot();
        let mut seq = Sequence::new(dir.to_path_buf(), newest.clone());
        seq.index = paths.len() - 1;
        seq.paths = paths;
        self.slots[index].sequence = Some(seq);
        if follow {
            self.active = index;
        }
        let full = self.cell_needs_full();
        self.start_load(index, newest, full);
    }

    /// ZIP をツリーで開く。展開は別スレッドで行い、終わったら [`Self::receive_archives`] で
    /// ルートに足す。
    pub(super) fn mount(&mut self, ctx: &egui::Context, zip: &Path) {
        if let Some(mount) = self.archives.mounted_zip(zip) {
            let (dir, into) = (mount.dir.clone(), self.settings().tree.load_into);
            self.cd(ctx, dir, into);
            return;
        }
        let ctx = ctx.clone();
        if self
            .archives
            .mount(zip, None, false, move || ctx.request_repaint())
        {
            // 展開している間はログ欄の下に 1 行で出し、済んだら「Extracted」の行を積むので、
            // 始めたことはファイルにだけ残す。
            self.log.detail(
                crate::log::Level::Info,
                format!("Extracting {}…", zip.display()),
            );
        }
    }

    /// 展開し終えた ZIP をツリーのルートに足し、その中へ移る。
    pub(super) fn receive_archives(&mut self, ctx: &egui::Context) {
        for event in self.archives.poll() {
            match event {
                archive::Event::Mounted { mount, files } => {
                    self.log.done(
                        "Extracted",
                        format!("{} ({files} files)", mount.zip.display()),
                    );
                    self.tree.add_mounted_root(mount.dir.clone(), mount.name());
                    let into = self.settings().tree.load_into;
                    self.cd(ctx, mount.dir, into);
                }
                archive::Event::Failed { zip, error } => {
                    self.log
                        .error(format!("Cannot open {}: {error}", zip.display()));
                }
                archive::Event::NeedsPassword { zip, wrong } => {
                    if wrong {
                        self.log
                            .warn(format!("Wrong password for {}", zip.display()));
                    }
                    self.password_prompt = Some(PasswordPrompt {
                        zip,
                        input: String::new(),
                        wrong,
                        focused: false,
                    });
                }
                archive::Event::TooLarge {
                    zip,
                    size,
                    limit,
                    password,
                } => {
                    self.log.warn(format!(
                        "{} is larger than the archive size limit",
                        zip.display()
                    ));
                    self.large_archive_prompt = Some(LargeArchivePrompt {
                        zip,
                        size,
                        limit,
                        password,
                    });
                }
            }
        }
    }

    /// 展開した ZIP を閉じる。中の画像を出しているスロットも閉じる。展開したファイルを消すので、
    /// スロットに残しておいても読み直せなくなるため。ZIP の中にいたら、ZIP のあるフォルダへ戻る。
    pub(super) fn unmount(&mut self, ctx: &egui::Context, dir: &Path) {
        if self.current.as_deref().is_some_and(|c| c.starts_with(dir)) {
            let back = self
                .archives
                .mount_of(dir)
                .and_then(|m| m.zip.parent())
                .map(Path::to_path_buf);
            self.finish_folder_load();
            self.close_all_slots();
            self.current = None;
            if let Some(back) = back {
                let into = self.settings().tree.load_into;
                self.cd(ctx, back, into);
            }
        }
        for index in (0..self.slots.len()).rev() {
            let slot = &self.slots[index];
            let inside = slot.path.as_deref().is_some_and(|p| p.starts_with(dir))
                || slot
                    .sequence
                    .as_ref()
                    .is_some_and(|q| q.folder.starts_with(dir));
            if inside {
                self.close_slot(index);
            }
        }
        self.loaded_folders.retain(|(p, _)| !p.starts_with(dir));
        self.tree.remove_root(dir);
        if let Some(mount) = self.archives.unmount(dir) {
            self.log.info(format!("Closed {}", mount.zip.display()));
        }
    }

    /// ツリーで DICOM のシリーズを選んだ。そのフォルダへ移り、シリーズのスロットを選ぶ。
    pub(super) fn show_series(&mut self, ctx: &egui::Context, folder: PathBuf, files: &[PathBuf]) {
        if self.current.as_ref() != Some(&folder) {
            let into = self.settings().tree.load_into;
            self.cd(ctx, folder, into);
        }
        let found = self.slots.iter().position(|s| match &s.sequence {
            Some(q) => q.kind == SequenceKind::Series && q.paths == files,
            None => s.path.as_ref().is_some_and(|p| files.contains(p)),
        });
        if let Some(index) = found {
            self.active = index;
            if self.maximized.is_some() {
                self.maximized = Some(self.slots[index].id);
            }
        }
    }

    /// 暗号化された ZIP / 7z のパスワードを尋ね、入力されたらそのパスワードで展開し直す。
    /// パスワードは覚えない。設定ファイルなどに平文で残したくないため。
    pub(super) fn password_dialog(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.password_prompt else {
            return;
        };
        let lang = self.settings.settings.appearance.language;
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("archive_password")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            let name = prompt
                .zip
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            ui.label(tr1(lang, "{} is encrypted. Enter the password.", &name));
            if prompt.wrong {
                ui.colored_label(ui.visuals().error_fg_color, tr(lang, "Wrong password."));
            }
            ui.add_space(4.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut prompt.input)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
            // 毎フレーム focus を求めると、ボタンのクリックまで取られてしまうので、開いたときだけ。
            if !prompt.focused {
                field.request_focus();
                prompt.focused = true;
            }
            let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr(lang, "Open")).clicked() || entered {
                    answer = Some(true);
                }
                if ui.button(tr(lang, "Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        let Some(open) = answer else {
            return;
        };
        let Some(prompt) = self.password_prompt.take() else {
            return;
        };
        if open {
            self.log
                .info(format!("Extracting {}…", prompt.zip.display()));
            let ctx = ctx.clone();
            self.archives
                .mount(&prompt.zip, Some(prompt.input), false, move || {
                    ctx.request_repaint()
                });
        }
    }
    /// 展開すると大きさの上限を超えるアーカイブを、展開するか尋ねる。展開するなら上限なしで
    /// 展開し直す（空き容量を超えるものは展開しない）。
    pub(super) fn large_archive_dialog(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &self.large_archive_prompt else {
            return;
        };
        let lang = self.settings.settings.appearance.language;
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("large_archive")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            let name = prompt
                .zip
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            ui.label(tr1(
                lang,
                "{} is larger than the archive size limit.",
                &name,
            ));
            let size = prompt
                .size
                .map_or_else(|| tr(lang, "more than the limit").to_owned(), archive::gib);
            ui.label(tr1(lang, "Size when extracted: {}", &size));
            ui.label(tr1(lang, "Limit: {}", &archive::gib(prompt.limit)));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr(lang, "Extract anyway")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr(lang, "Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        let Some(extract) = answer else {
            return;
        };
        let Some(prompt) = self.large_archive_prompt.take() else {
            return;
        };
        if extract {
            self.log
                .info(format!("Extracting {}…", prompt.zip.display()));
            let ctx = ctx.clone();
            self.archives
                .mount(&prompt.zip, prompt.password, true, move || {
                    ctx.request_repaint()
                });
        }
    }
}
