//! 選んだファイルやフォルダを、設定した送信先（PACS など）へ送る。
//!
//! 送る前に、裏で対象のファイルを集めて数え、件数を出して確かめてから送る。送った先で
//! 消すのは手間なので、件数が少なくても毎回確かめる。送信も裏で行い、送っている間も
//! 画像は普通に見られる。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use eframe::egui;
use vw_dicom::net::{self, Endpoint, Header, SendError, Store, Stored};

use crate::archive::{self, ArchiveError, EntryError};
use crate::dataset_extract::enclosed;
use crate::i18n::tr;
use crate::settings::{Destination, Language};

/// 送信のメニューで選ばれたもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// 設定の送信先の何番目か。
    To(usize),
    /// 送信先がまだないので、設定画面を開く。
    Configure,
}

/// 「送る ▸」の中の、ネットワークの送信先の項目。`destinations` は送信先の名前。
pub fn menu_items(ui: &mut egui::Ui, lang: Language, destinations: &[String]) -> Option<Choice> {
    let mut chosen = None;
    if destinations.is_empty() && ui.button(tr(lang, "Set up destinations...")).clicked() {
        chosen = Some(Choice::Configure);
        ui.close();
    }
    for (i, name) in destinations.iter().enumerate() {
        if ui.button(name).clicked() {
            chosen = Some(Choice::To(i));
            ui.close();
        }
    }
    chosen
}

/// 送るように選ばれたもの。
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// ファイル・フォルダ・ZIP（フォルダと ZIP は中の DICOM ファイルすべて）。
    Path(PathBuf),
    /// ZIP・7z の中の 1 ファイル（データセットの検索で当たったファイル）。
    Member { archive: PathBuf, inner: PathBuf },
}

/// データセット `dir` の送り先。`hits`（検索カタログのパス）があればそのファイルだけ。
/// フォルダの外を指すパスは除く。
pub fn dataset_targets(dir: &Path, hits: Option<&[String]>) -> Vec<Target> {
    let Some(hits) = hits else {
        return vec![Target::Path(dir.to_owned())];
    };
    hits.iter()
        .filter_map(|path| match path.split_once(crate::catalog::IN_ARCHIVE) {
            Some((archive, inner)) => Some(Target::Member {
                archive: dir.join(enclosed(archive)?),
                inner: enclosed(inner)?,
            }),
            None => Some(Target::Path(dir.join(enclosed(path)?))),
        })
        .collect()
}

/// アーカイブの中から送るファイル。`None` なら DICOM ファイルすべて。
type Only = Option<HashSet<PathBuf>>;

/// 送る 1 件。ZIP・7z の中のファイルは、アーカイブを 1 回読む間にまとめて送る。
#[derive(Debug, Clone, PartialEq)]
enum Item {
    File(PathBuf, Header),
    Archive(PathBuf, Only, Vec<Header>),
}

impl Item {
    fn len(&self) -> usize {
        match self {
            Item::File(..) => 1,
            Item::Archive(_, _, members) => members.len(),
        }
    }
}

/// 集めた結果。
#[derive(Debug, Default)]
struct Collected {
    items: Vec<Item>,
    /// 送らないもの（読めない ZIP など）の説明。
    notes: Vec<String>,
}

impl Collected {
    fn total(&self) -> usize {
        self.items.iter().map(Item::len).sum()
    }
}

/// 先頭 132 バイトの `DICM` の印で DICOM ファイルか見分ける。拡張子で絞らないのは、
/// 拡張子のない DICOM（`IM0001` など）が多いため。
fn is_dicom(path: &Path) -> bool {
    let mut head = [0u8; 132];
    std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head))
        .is_ok()
        && &head[128..] == b"DICM"
}

/// `targets` から送る DICOM ファイルを集める。フォルダはサブフォルダと
/// その中の ZIP・7z も含める。隠しフォルダとゴミ箱は除く。
fn collect(targets: &[Target], cancel: &AtomicBool, counted: &AtomicUsize) -> Collected {
    let mut out = Collected::default();
    let mut seen = HashSet::new();
    let mut members: HashMap<&Path, HashSet<PathBuf>> = HashMap::new();
    for target in targets {
        match target {
            Target::Path(path) => visit(path, cancel, counted, &mut seen, &mut out),
            Target::Member { archive, inner } => {
                members.entry(archive).or_default().insert(inner.clone());
            }
        }
    }
    let mut archives: Vec<_> = members.into_iter().collect();
    archives.sort_by(|a, b| a.0.cmp(b.0));
    for (archive, only) in archives {
        if !seen.contains(archive) {
            add_archive(archive, Some(only), cancel, counted, &mut out);
        }
    }
    out
}

fn add_archive(
    path: &Path,
    only: Only,
    cancel: &AtomicBool,
    counted: &AtomicUsize,
    out: &mut Collected,
) {
    match archive_headers(path, only.as_ref(), cancel) {
        Ok(headers) if headers.is_empty() => {}
        Ok(headers) => {
            counted.fetch_add(headers.len(), Ordering::Relaxed);
            out.items
                .push(Item::Archive(path.to_owned(), only, headers));
        }
        Err(e) => out.notes.push(format!("{}: {e} (skipped)", path.display())),
    }
}

fn visit(
    path: &Path,
    cancel: &AtomicBool,
    counted: &AtomicUsize,
    seen: &mut HashSet<PathBuf>,
    out: &mut Collected,
) {
    if cancel.load(Ordering::Relaxed) || !seen.insert(path.to_owned()) {
        return;
    }
    if path.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            out.notes
                .push(format!("{}: cannot read the folder", path.display()));
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if crate::tree::is_hidden_name(&e.file_name().to_string_lossy()) {
                continue;
            }
            visit(&e.path(), cancel, counted, seen, out);
        }
    } else if archive::is_archive(path) {
        add_archive(path, None, cancel, counted, out);
    } else if is_dicom(path) {
        match net::read_header(path) {
            Ok(header) => {
                counted.fetch_add(1, Ordering::Relaxed);
                out.items.push(Item::File(path.to_owned(), header));
            }
            Err(e) => out.notes.push(format!("{}: {e} (skipped)", path.display())),
        }
    }
}

/// アーカイブの中の DICOM ファイル。パスワードのあるものは送らない（送信の途中で
/// パスワードを聞いて止めないため）。
fn archive_headers(
    path: &Path,
    only: Option<&HashSet<PathBuf>>,
    cancel: &AtomicBool,
) -> Result<Vec<Header>, String> {
    let mut headers = Vec::new();
    for_each_dicom(path, only, cancel, |obj| {
        headers.push(Header::of(&obj));
        Ok(())
    })?;
    Ok(headers)
}

/// アーカイブの中の DICOM ファイルを順に読んで `visit` に渡す。
fn for_each_dicom(
    path: &Path,
    only: Option<&HashSet<PathBuf>>,
    cancel: &AtomicBool,
    mut visit: impl FnMut(net::Object) -> Result<(), String>,
) -> Result<(), String> {
    let result = archive::for_each_entry(path, None, |inner, data| {
        if cancel.load(Ordering::Relaxed) {
            return Err(EntryError::Other("cancelled".into()));
        }
        let Some(data) = data else { return Ok(()) };
        if only.is_some_and(|only| !only.contains(inner)) {
            return Ok(());
        }
        let mut bytes = Vec::new();
        data.read_to_end(&mut bytes).map_err(EntryError::Read)?;
        if bytes.get(128..132) != Some(b"DICM") {
            return Ok(());
        }
        // 中の 1 ファイルが壊れていても、残りは送る。
        match net::open(&bytes[..], false) {
            Ok(obj) => visit(obj).map_err(EntryError::Other),
            Err(_) => Ok(()),
        }
    });
    match result {
        Ok(_) => Ok(()),
        Err(ArchiveError::NeedsPassword) => Err("password protected".into()),
        Err(ArchiveError::Other(e)) => Err(e),
        Err(e) => Err(format!("{e:?}")),
    }
}

/// 送信の裏の処理から届く知らせ。
#[derive(Debug)]
pub enum Event {
    /// 数え終わった。
    Counted { total: usize, notes: Vec<String> },
    /// 1 ファイル送れなかった。
    Failed { file: String, reason: String },
    /// 受け取られたが警告つき。
    Warning { file: String, status: u16 },
    /// 終わった（取り消したときも）。
    Finished {
        sent: usize,
        failed: usize,
        cancelled: bool,
        /// 接続が切れたなどで残りを送れなかった理由。
        stopped: Option<String>,
    },
}

/// 裏の処理と共有する進み具合。
#[derive(Default)]
struct Shared {
    counted: AtomicUsize,
    done: AtomicUsize,
    total: AtomicUsize,
    cancel: AtomicBool,
}

/// 1 回の送信。数える → 確かめる → 送る。
pub struct Job {
    pub destination: Destination,
    shared: Arc<Shared>,
    events: Receiver<Event>,
    go: Option<Sender<bool>>,
    /// 数え終わった件数。確かめる前は `None`。
    pub total: Option<usize>,
    /// 送り始めた。
    pub sending: bool,
}

impl Job {
    /// `targets` を数え始める。送るのは [`Job::start`] を呼んでから。
    pub fn new(
        destination: Destination,
        calling_ae: String,
        timeout: std::time::Duration,
        targets: Vec<Target>,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        let (events_tx, events) = channel();
        let (go, go_rx) = channel();
        let endpoint = Endpoint {
            calling_ae,
            host: destination.host.clone(),
            port: destination.port,
            called_ae: destination.ae_title.clone(),
            timeout,
        };
        let s = shared.clone();
        std::thread::spawn(move || run(&endpoint, &targets, &s, &events_tx, &go_rx));
        Self {
            destination,
            shared,
            events,
            go: Some(go),
            total: None,
            sending: false,
        }
    }

    /// 数えている途中の件数。
    pub fn counted(&self) -> usize {
        self.shared.counted.load(Ordering::Relaxed)
    }

    /// 確かめたので送る。
    pub fn start(&mut self) {
        if let Some(go) = self.go.take() {
            let _ = go.send(true);
            self.sending = true;
        }
    }

    /// 取り消す。数えている途中でも送っている途中でもよい。
    pub fn cancel(&mut self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
        if let Some(go) = self.go.take() {
            let _ = go.send(false);
        }
    }

    /// 送っている間の (送った数, 全体)。
    pub fn progress(&self) -> Option<(usize, usize)> {
        self.sending.then(|| {
            (
                self.shared.done.load(Ordering::Relaxed),
                self.shared.total.load(Ordering::Relaxed),
            )
        })
    }

    /// 前に呼んでから届いた知らせ。
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(e) = self.events.try_recv() {
            if let Event::Counted { total, .. } = &e {
                self.total = Some(*total);
            }
            out.push(e);
        }
        out
    }
}

fn run(
    endpoint: &Endpoint,
    targets: &[Target],
    shared: &Shared,
    events: &Sender<Event>,
    go: &Receiver<bool>,
) {
    let collected = collect(targets, &shared.cancel, &shared.counted);
    let total = collected.total();
    shared.total.store(total, Ordering::Relaxed);
    let _ = events.send(Event::Counted {
        total,
        notes: collected.notes,
    });
    if total == 0 || !go.recv().unwrap_or(false) {
        return;
    }
    let finished = send_all(endpoint, &collected.items, shared, events);
    let _ = events.send(finished);
}

fn send_all(endpoint: &Endpoint, items: &[Item], shared: &Shared, events: &Sender<Event>) -> Event {
    let headers: Vec<&Header> = items
        .iter()
        .flat_map(|i| match i {
            Item::File(_, h) => std::slice::from_ref(h).iter(),
            Item::Archive(_, _, hs) => hs.iter(),
        })
        .collect();
    let mut store = match Store::connect(endpoint, headers) {
        Ok(s) => s,
        Err(e) => {
            return Event::Finished {
                sent: 0,
                failed: 0,
                cancelled: false,
                stopped: Some(e),
            }
        }
    };
    let (mut sent, mut failed) = (0, 0);
    let mut stopped = None;
    let mut one = |label: String, obj: &net::Object| -> Result<(), String> {
        let result = store.send(obj);
        shared.done.fetch_add(1, Ordering::Relaxed);
        match result {
            Ok(Stored::Success) => sent += 1,
            Ok(Stored::Warning(status)) => {
                sent += 1;
                let _ = events.send(Event::Warning {
                    file: label,
                    status,
                });
            }
            Err(SendError::File(reason)) => {
                failed += 1;
                let _ = events.send(Event::Failed {
                    file: label,
                    reason,
                });
            }
            Err(SendError::Link(reason)) => {
                failed += 1;
                return Err(reason);
            }
        }
        Ok(())
    };
    for item in items {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let result = match item {
            Item::File(path, _) => match std::fs::read(path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| net::open(&bytes, false))
            {
                Ok(obj) => one(path.display().to_string(), &obj),
                Err(reason) => {
                    shared.done.fetch_add(1, Ordering::Relaxed);
                    let _ = events.send(Event::Failed {
                        file: path.display().to_string(),
                        reason,
                    });
                    Ok(())
                }
            },
            Item::Archive(path, only, _) => {
                let mut link = None;
                let read = for_each_dicom(path, only.as_ref(), &shared.cancel, |obj| {
                    let label = format!("{} ({})", path.display(), Header::of(&obj).sop_instance);
                    one(label, &obj).inspect_err(|e| link = Some(e.clone()))
                });
                match (link, read) {
                    (Some(e), _) => Err(e),
                    (None, Err(e)) if !shared.cancel.load(Ordering::Relaxed) => {
                        let _ = events.send(Event::Failed {
                            file: path.display().to_string(),
                            reason: e,
                        });
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
        };
        if let Err(e) = result {
            stopped = Some(e);
            break;
        }
    }
    let cancelled = shared.cancel.load(Ordering::Relaxed);
    if cancelled || stopped.is_some() {
        store.abort();
    } else {
        store.release();
    }
    Event::Finished {
        sent,
        failed,
        cancelled,
        stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_dicom::synth::Synth;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            use std::sync::atomic::AtomicUsize;
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "vw-send-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dicom(path: &Path, instance: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        Synth::ct(2, 2).instance(instance, 1).write(path).unwrap();
    }

    fn collected(paths: &[PathBuf]) -> Collected {
        let targets: Vec<Target> = paths.iter().cloned().map(Target::Path).collect();
        collect(&targets, &AtomicBool::new(false), &AtomicUsize::new(0))
    }

    fn zip_of(zip: &Path, members: &[(&str, &str)]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(zip).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, uid) in members {
            w.start_file(*name, options).unwrap();
            let bytes = Synth::ct(2, 2).instance(uid, 1).bytes();
            std::io::Write::write_all(&mut w, &bytes).unwrap();
        }
        w.start_file("readme.txt", options).unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn folder_includes_subfolders_and_dicom_without_extension() {
        let t = Tmp::new();
        dicom(&t.0.join("a/IM0001"), "1.2.1");
        dicom(&t.0.join("a/b/x.dcm"), "1.2.2");
        std::fs::write(t.0.join("a/not-dicom.png"), b"\x89PNG....").unwrap();
        let c = collected(&[t.0.join("a")]);
        assert_eq!(c.total(), 2);
        assert!(c.notes.is_empty());
    }

    #[test]
    fn hidden_folders_and_recycle_bin_are_skipped() {
        let t = Tmp::new();
        dicom(&t.0.join(".cache/x.dcm"), "1.2.3");
        dicom(&t.0.join("$RECYCLE.BIN/x.dcm"), "1.2.4");
        dicom(&t.0.join("ok.dcm"), "1.2.5");
        assert_eq!(collected(std::slice::from_ref(&t.0)).total(), 1);
    }

    #[test]
    fn dicom_inside_a_zip_is_counted() {
        let t = Tmp::new();
        zip_of(
            &t.0.join("set.zip"),
            &[("s/1.dcm", "1.2.6"), ("s/2", "1.2.7")],
        );
        let c = collected(std::slice::from_ref(&t.0));
        assert_eq!(c.total(), 2);
        assert!(matches!(&c.items[0], Item::Archive(_, None, h) if h[1].sop_instance == "1.2.7"));
    }

    #[test]
    fn dataset_hits_become_files_and_zip_members_inside_the_dataset() {
        let dir = Path::new("set");
        let hits = [
            "a/1.dcm".to_owned(),
            "z.zip!/s/2.dcm".to_owned(),
            "../x.dcm".to_owned(),
        ];
        assert_eq!(
            dataset_targets(dir, Some(&hits)),
            vec![
                Target::Path(dir.join("a/1.dcm")),
                Target::Member {
                    archive: dir.join("z.zip"),
                    inner: PathBuf::from("s/2.dcm"),
                },
            ]
        );
        assert_eq!(
            dataset_targets(dir, None),
            vec![Target::Path(dir.to_owned())]
        );
    }

    #[test]
    fn only_the_chosen_members_of_a_zip_are_counted() {
        let t = Tmp::new();
        let zip = t.0.join("set.zip");
        zip_of(&zip, &[("s/1.dcm", "1.2.11"), ("s/2.dcm", "1.2.12")]);
        let targets = [Target::Member {
            archive: zip,
            inner: PathBuf::from("s/2.dcm"),
        }];
        let c = collect(&targets, &AtomicBool::new(false), &AtomicUsize::new(0));
        assert_eq!(c.total(), 1);
        assert!(
            matches!(&c.items[0], Item::Archive(_, Some(_), h) if h[0].sop_instance == "1.2.12")
        );
    }

    #[test]
    fn the_same_file_named_twice_is_sent_once() {
        let t = Tmp::new();
        let f = t.0.join("x.dcm");
        dicom(&f, "1.2.8");
        assert_eq!(collected(&[f, t.0.clone()]).total(), 1);
    }

    #[test]
    fn counted_files_are_sent_to_the_destination() {
        let t = Tmp::new();
        dicom(&t.0.join("1.dcm"), "1.2.9");
        dicom(&t.0.join("2.dcm"), "1.2.10");
        let inbox = Tmp::new();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let receiver = vw_dicom::net::receiver::Receiver {
            ae_title: "PACS".into(),
            accept_compressed: true,
            dir: inbox.0.clone(),
            store_status: 0,
        };
        let scp = std::thread::spawn(move || receiver.serve(listener.accept().unwrap().0));
        let destination = Destination {
            name: "test".into(),
            host: "127.0.0.1".into(),
            port,
            ae_title: "PACS".into(),
        };
        let mut job = Job::new(
            destination,
            "VW".into(),
            std::time::Duration::from_secs(10),
            vec![Target::Path(t.0.clone())],
        );
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !events.iter().any(|e| matches!(e, Event::Finished { .. })) {
            assert!(std::time::Instant::now() < deadline, "{events:?}");
            for e in job.poll() {
                if matches!(e, Event::Counted { total: 2, .. }) {
                    job.start();
                }
                events.push(e);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(matches!(
            events.last(),
            Some(Event::Finished {
                sent: 2,
                failed: 0,
                cancelled: false,
                stopped: None
            })
        ));
        assert_eq!(scp.join().unwrap().unwrap().files.len(), 2);
    }
}
