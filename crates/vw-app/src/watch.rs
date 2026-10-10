//! フォルダの監視。ツリーで開いているフォルダとスロットに出しているフォルダの変化を受け取り、
//! 書き込みが落ち着いてから（[`QUIET`]）まとめて知らせる。
//!
//! 監視は OS の通知（Linux は inotify、Windows は ReadDirectoryChangesW）に任せる。
//! 定期的にフォルダを読み直す方式を採らないのは、数千枚のフォルダを何か所も開いていると、
//! 変化がなくても読み直しの負荷がかかり続けるため。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use eframe::egui;
use notify::event::{AccessKind, AccessMode, EventKind, ModifyKind, RenameMode};
use notify::{RecursiveMode, Watcher as _};

/// 最後の変化からこれだけ経ってから知らせる。書き込み途中のファイルを読まないため。
pub const QUIET: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Modified,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub kind: ChangeKind,
}

/// ファイルごとの変化を、静かになるまでためてから 1 つにまとめる。
#[derive(Default)]
pub struct Debouncer {
    pending: HashMap<PathBuf, (Option<ChangeKind>, Instant)>,
}

impl Debouncer {
    pub fn note(&mut self, path: PathBuf, kind: ChangeKind, now: Instant) {
        let entry = self.pending.entry(path).or_insert((None, now));
        entry.0 = merge(entry.0, kind);
        entry.1 = now;
    }

    /// 最後の変化から [`QUIET`] 経ったものを取り出す。作ってすぐ消したファイルは何も知らせない。
    pub fn ready(&mut self, now: Instant) -> Vec<Change> {
        let mut out = Vec::new();
        self.pending.retain(|path, (kind, at)| {
            if now.saturating_duration_since(*at) < QUIET {
                return true;
            }
            if let Some(kind) = kind {
                out.push(Change {
                    path: path.clone(),
                    kind: *kind,
                });
            }
            false
        });
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// 前の変化 `before` の後に `next` が起きたときの、まとめた変化。
/// `None` は「何も起きなかったのと同じ」（作ってから消した）。
fn merge(before: Option<ChangeKind>, next: ChangeKind) -> Option<ChangeKind> {
    use ChangeKind::*;
    match (before, next) {
        (Some(Created), Modified) => Some(Created),
        (Some(Created), Removed) => None,
        (Some(Removed), Created) => Some(Modified),
        (_, next) => Some(next),
    }
}

/// notify の通知を、ファイルの変化に読み替える。読み込みのために開いただけの通知は捨てる
/// （vw 自身が画像を読むたびに届き、読み直しが止まらなくなるため）。
fn changes_of(event: &notify::Event) -> Vec<(PathBuf, ChangeKind)> {
    use ChangeKind::*;
    let all = |kind| event.paths.iter().map(|p| (p.clone(), kind)).collect();
    match event.kind {
        EventKind::Create(_) => all(Created),
        EventKind::Remove(_) => all(Removed),
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => all(Removed),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => all(Created),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => match event.paths.as_slice() {
            [from, to] => vec![(from.clone(), Removed), (to.clone(), Created)],
            _ => Vec::new(),
        },
        EventKind::Modify(ModifyKind::Metadata(_)) => Vec::new(),
        EventKind::Modify(_) => all(Modified),
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => all(Modified),
        _ => Vec::new(),
    }
}

/// 並び `paths` で `current` を表示しているスロットが、新しく来た画像 `created` を追って
/// 移る先の位置。`current` が新着を除いた並びの最後だったときだけ移る。
/// 途中の画像を見ているときに勝手に動かすと、見比べている最中の画像が消えるため。
pub fn follow_newest(
    paths: &[PathBuf],
    current: &Path,
    created: &HashSet<PathBuf>,
) -> Option<usize> {
    let pos = paths.iter().position(|p| p == current)?;
    let after = &paths[pos + 1..];
    let only_new_after = !after.is_empty() && after.iter().all(|p| created.contains(p));
    only_new_after.then(|| paths.len() - 1)
}

/// フォルダの監視。監視するフォルダは [`Self::set_dirs`] で毎回まるごと渡す。
pub struct FolderWatcher {
    inner: Option<notify::RecommendedWatcher>,
    rx: Receiver<notify::Result<notify::Event>>,
    watched: HashSet<PathBuf>,
    debouncer: Debouncer,
}

impl FolderWatcher {
    /// 通知が届いたら、まとめる時間の後に画面を描き直させる。
    pub fn new(ctx: egui::Context) -> Result<Self, String> {
        let (tx, rx) = channel();
        let inner = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
            ctx.request_repaint_after(QUIET + Duration::from_millis(20));
        })
        .map_err(|e| e.to_string())?;
        Ok(Self {
            inner: Some(inner),
            rx,
            watched: HashSet::new(),
            debouncer: Debouncer::default(),
        })
    }

    /// 監視するフォルダを `dirs` にそろえる。監視を始められなかったフォルダの説明を返す
    /// （消えたフォルダや権限のないフォルダ）。
    pub fn set_dirs(&mut self, dirs: HashSet<PathBuf>) -> Vec<String> {
        let mut problems = Vec::new();
        if dirs == self.watched {
            return problems;
        }
        let Some(inner) = &mut self.inner else {
            return problems;
        };
        for gone in self.watched.difference(&dirs) {
            let _ = inner.unwatch(gone);
        }
        for new in dirs.difference(&self.watched) {
            if let Err(e) = inner.watch(new, RecursiveMode::NonRecursive) {
                problems.push(format!("{}: not watched ({e})", new.display()));
            }
        }
        self.watched = dirs;
        problems
    }

    /// 届いた通知を取り込み、落ち着いた変化を返す。
    pub fn poll(&mut self, now: Instant) -> Vec<Change> {
        while let Ok(res) = self.rx.try_recv() {
            let Ok(event) = res else { continue };
            for (path, kind) in changes_of(&event) {
                self.debouncer.note(path, kind, now);
            }
        }
        self.debouncer.ready(now)
    }

    /// まだ落ち着いていない変化があるか。あれば描き直しを予約し続ける必要がある。
    pub fn waiting(&self) -> bool {
        !self.debouncer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, RemoveKind};

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn change_is_reported_only_after_the_folder_is_quiet() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.note(p("a.png"), ChangeKind::Created, t0);
        assert!(d.ready(t0 + QUIET / 2).is_empty());
        let out = d.ready(t0 + QUIET);
        assert_eq!(
            out,
            [Change {
                path: p("a.png"),
                kind: ChangeKind::Created
            }]
        );
        assert!(d.is_empty());
    }

    #[test]
    fn writes_while_copying_keep_postponing_the_report() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.note(p("a.png"), ChangeKind::Created, t0);
        d.note(p("a.png"), ChangeKind::Modified, t0 + QUIET / 2);
        assert!(d.ready(t0 + QUIET).is_empty());
        let out = d.ready(t0 + QUIET * 2);
        assert_eq!(out[0].kind, ChangeKind::Created);
    }

    #[test]
    fn file_created_and_deleted_before_quiet_is_not_reported() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.note(p("tmp.png"), ChangeKind::Created, t0);
        d.note(p("tmp.png"), ChangeKind::Removed, t0);
        assert!(d.ready(t0 + QUIET).is_empty());
    }

    #[test]
    fn file_replaced_by_delete_and_create_is_a_modification() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.note(p("a.png"), ChangeKind::Removed, t0);
        d.note(p("a.png"), ChangeKind::Created, t0);
        assert_eq!(d.ready(t0 + QUIET)[0].kind, ChangeKind::Modified);
    }

    #[test]
    fn slot_on_the_last_image_moves_to_the_new_last_image() {
        let paths = [p("1.png"), p("2.png"), p("3.png"), p("4.png")];
        let created = [p("3.png"), p("4.png")].into();
        assert_eq!(follow_newest(&paths, &p("2.png"), &created), Some(3));
    }

    #[test]
    fn slot_on_an_earlier_image_stays_put() {
        let paths = [p("1.png"), p("2.png"), p("3.png")];
        let created = [p("3.png")].into();
        assert_eq!(follow_newest(&paths, &p("1.png"), &created), None);
    }

    #[test]
    fn new_image_sorted_before_the_shown_one_does_not_move_the_slot() {
        let paths = [p("0.png"), p("1.png")];
        let created = [p("0.png")].into();
        assert_eq!(follow_newest(&paths, &p("1.png"), &created), None);
    }

    fn event(kind: EventKind, paths: &[&str]) -> notify::Event {
        let mut e = notify::Event::new(kind);
        for path in paths {
            e = e.add_path(p(path));
        }
        e
    }

    #[test]
    fn rename_into_the_folder_counts_as_a_new_file() {
        let e = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["a.tmp", "a.png"],
        );
        assert_eq!(
            changes_of(&e),
            [
                (p("a.tmp"), ChangeKind::Removed),
                (p("a.png"), ChangeKind::Created)
            ]
        );
    }

    #[test]
    fn reading_a_file_is_not_a_change() {
        let open = event(
            EventKind::Access(AccessKind::Open(AccessMode::Any)),
            &["a.png"],
        );
        let close = event(
            EventKind::Access(AccessKind::Close(AccessMode::Read)),
            &["a.png"],
        );
        assert!(changes_of(&open).is_empty());
        assert!(changes_of(&close).is_empty());
    }

    #[test]
    fn create_write_and_delete_are_reported() {
        let created = event(EventKind::Create(CreateKind::File), &["a.png"]);
        let written = event(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            &["a.png"],
        );
        let removed = event(EventKind::Remove(RemoveKind::File), &["a.png"]);
        assert_eq!(changes_of(&created)[0].1, ChangeKind::Created);
        assert_eq!(changes_of(&written)[0].1, ChangeKind::Modified);
        assert_eq!(changes_of(&removed)[0].1, ChangeKind::Removed);
    }

    #[test]
    fn real_folder_reports_a_new_file() {
        let dir = crate::test_util::TempDir::new();
        let ctx = egui::Context::default();
        let mut w = FolderWatcher::new(ctx).unwrap();
        assert!(w.set_dirs([dir.path().to_path_buf()].into()).is_empty());
        std::fs::write(dir.path().join("new.png"), b"x").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        while seen.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            seen = w.poll(Instant::now());
        }
        assert_eq!(seen[0].path.file_name().unwrap(), "new.png");
        assert_eq!(seen[0].kind, ChangeKind::Created);
    }
}
