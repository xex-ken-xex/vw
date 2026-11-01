//! データセットの検索カタログ（`vw-search-catalog.json`）。データセットのフォルダ直下に置き、
//! 名前・メモ・タグ・足した項目と、フォルダ以下の画像（ZIP / 7z の中も）のメタデータを持つ。
//!
//! メタデータは「種類・キー・値」の 3 つ組にし、同じ組はデータセットで 1 つだけ持つ
//! （`meta`）。各ファイルはその番号の一覧を持つ。ファイルごとに組を書き並べないのは、
//! 同じ装置・同じ患者の画像では大半の値が重なり、カタログが何倍にも膨らむため。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use vw_core::{Metadata, Registry};

use crate::ai_tags::{self, AiTagStore};
use crate::archive::{self, ArchiveError, EntryError};
use crate::labels::{self, LabelStore, Position};

pub const FILE_NAME: &str = "vw-search-catalog.json";
/// 途中で止めた登録の、そこまでの結果。続きから始めるときに読む。
pub const PARTIAL_FILE: &str = "vw-search-catalog.partial.json";
/// 今読んでいるファイル。読む前に書くので、vw が落ちたら最後に読もうとしたファイルが残る。
pub const READING_FILE: &str = "vw-search-catalog.reading";
const FORMAT: u32 = 1;
/// AI タグと人物の組の種類。メタデータ欄のグループ「AI tags」に合わせる。
pub const AI_TAGS: &str = "ai tags";
/// 手で付けたフラグとタグの組の種類。
pub const LABELS: &str = "labels";
/// ZIP の場所と ZIP の中のパスの区切り（`data.zip!/series1/IM0001.dcm`）。
pub const IN_ARCHIVE: &str = "!/";

/// メタデータの 1 組（種類・キー・値）。種類はメタデータ欄のグループ名を小文字にしたもの
/// （`exif`、`dicom` など）と、形式によらない基本情報の `image`、AI タグの [`AI_TAGS`]。
pub type Triple = (String, String, String);

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Catalog {
    pub format: u32,
    pub name: String,
    #[serde(default)]
    pub memo: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// 設定（`dataset.fields`）で足した項目。設定から消した項目や、他の人が足した項目も残す。
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub updated: String,
    /// 取り出して作ったデータセットなら、元のデータセット。
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub meta: Vec<Triple>,
    #[serde(default)]
    pub files: Vec<FileEntry>,
    /// 読めなかったファイルや、パスワードを入れずに飛ばしたアーカイブ。
    #[serde(default)]
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skipped {
    pub path: String,
    /// パスワードを入れずに飛ばしたアーカイブは `password`。ほかは読めなかった理由。
    pub reason: String,
}

impl std::fmt::Display for Skipped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.reason == PASSWORD_SKIPPED {
            write!(f, "{}: skipped (no password)", self.path)
        } else {
            write!(f, "{}: {}", self.path, self.reason)
        }
    }
}

pub const PASSWORD_SKIPPED: &str = "password";
/// 読んでいる最中に vw が落ちたので、続きからでは読まなかったファイルの理由。
pub const CRASHED: &str = "vw stopped while reading this file, so it was skipped when resuming";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    /// データセットのフォルダからの相対パス（区切りは `/`）。ZIP の中は [`IN_ARCHIVE`] でつなぐ。
    pub path: String,
    #[serde(default)]
    pub size: u64,
    /// 更新時刻（UTC、RFC 3339）。ZIP の中のファイルは ZIP の更新時刻。
    #[serde(default)]
    pub modified: String,
    /// [`Catalog::meta`] の番号。
    #[serde(default)]
    pub meta: Vec<u32>,
}

/// 人が入力する部分。登録・編集のダイアログで変える。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Info {
    pub name: String,
    pub memo: String,
    pub tags: Vec<String>,
    pub fields: BTreeMap<String, String>,
}

impl Catalog {
    pub fn set_info(&mut self, info: Info) {
        self.name = info.name;
        self.memo = info.memo;
        self.tags = info.tags;
        self.fields = info.fields;
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

pub fn exists_in(dir: &Path) -> bool {
    path_in(dir).is_file()
}

pub fn load(dir: &Path) -> Result<Catalog, String> {
    let path = path_in(dir);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// 一時ファイルに書いてから置き換える。書いている途中で落ちても、前のカタログが壊れないように。
pub fn save(dir: &Path, catalog: &Catalog) -> Result<(), String> {
    let path = path_in(dir);
    let temp = dir.join(format!("{FILE_NAME}.tmp"));
    let text = serde_json::to_string(catalog).expect("catalog is always representable in JSON");
    std::fs::write(&temp, text)
        .and_then(|()| std::fs::rename(&temp, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            format!("{}: {e}", path.display())
        })
}

/// 登録できるか。カタログを置くので書き込めること。
///
/// データセットの中に別のデータセットがあってもよい。外側は内側のファイルも含めて持つ。
pub fn check_can_register(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!("{FILE_NAME}.write-test"));
    std::fs::write(&probe, b"")
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|e| format!("Cannot write to {}: {e}", dir.display()))
}

/// 途中で止めた登録の、そこまでの結果。
///
/// 読み終えたファイルの組と一覧をそのまま持つ。続きから始めるときは、ここにないファイルだけを読む。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
struct Partial {
    format: u32,
    meta: Vec<Triple>,
    files: Vec<FileEntry>,
    skipped: Vec<Skipped>,
    /// 読み終えたファイルとアーカイブ（データセットのフォルダからの相対パス）。
    done: Vec<String>,
    /// 読んでいる最中に vw が落ちたファイル。続きからでは読まない。
    #[serde(default)]
    crashed: Vec<String>,
    /// 登録のダイアログで入れた名前など。続きから登録するときに入れ直さなくて済むように。
    #[serde(default)]
    info: Option<Info>,
}

/// 途中で止めた登録があれば、その様子（読み終えたファイルの数と、読んでいる最中に落ちたファイル）。
#[derive(Debug, Clone, PartialEq)]
pub struct Unfinished {
    pub done: usize,
    /// 前回、読んでいる最中に vw が落ちたファイル。続きからではこれを飛ばす。
    pub crashed: Option<String>,
    /// 止めたときの登録の名前など。
    pub info: Option<Info>,
}

/// `dir` に途中で止めた登録があるか。
pub fn unfinished(dir: &Path) -> Option<Unfinished> {
    let partial = load_partial(dir)?;
    Some(Unfinished {
        done: partial.done.len(),
        crashed: crashed_file(dir, &partial),
        info: partial.info,
    })
}

fn load_partial(dir: &Path) -> Option<Partial> {
    let text = std::fs::read_to_string(dir.join(PARTIAL_FILE)).ok()?;
    serde_json::from_str::<Partial>(&text)
        .ok()
        .filter(|p| p.format == FORMAT)
}

/// 前回読もうとしたまま終わったファイル。読み終えたものに入っていれば落ちたのではない。
fn crashed_file(dir: &Path, partial: &Partial) -> Option<String> {
    let reading = std::fs::read_to_string(dir.join(READING_FILE)).ok()?;
    let top = reading.split(IN_ARCHIVE).next().unwrap_or(&reading);
    (!reading.is_empty() && !partial.done.iter().any(|d| d == top)).then_some(reading)
}

fn save_partial(dir: &Path, partial: &Partial) {
    let text = serde_json::to_string(partial).expect("partial catalog is always representable");
    let temp = dir.join(format!("{PARTIAL_FILE}.tmp"));
    if std::fs::write(&temp, text)
        .and_then(|()| std::fs::rename(&temp, dir.join(PARTIAL_FILE)))
        .is_err()
    {
        let _ = std::fs::remove_file(&temp);
    }
}

/// 途中の結果を消す。最後まで読んだときと、最初から読み直すとき。
pub fn discard_unfinished(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(PARTIAL_FILE));
    let _ = std::fs::remove_file(dir.join(READING_FILE));
}

/// 途中の結果をこの間隔で書き出す。落ちたときに読み直すのは、最後に書き出してからの分だけ。
const CHECKPOINT_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

/// 登録を途中で止めたときや落ちたときに、続きから始められるようにするか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// 途中の結果を残さない（取り出したものの登録など、すぐ終わるもの）。
    Off,
    /// 前の途中の結果を捨てて最初から読み、途中の結果を残しながら進める。
    Fresh,
    /// 前の途中の結果の続きから読む。
    Continue,
}

/// 調べている途中の様子。画面から読み、キャンセルを伝える。
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub current: Mutex<String>,
    pub cancel: AtomicBool,
    /// 一時中断している。解くか止めるまで、次のファイルを読まずに待つ。
    pub pause: AtomicBool,
}

impl Progress {
    pub(crate) fn paused(&self) -> bool {
        self.pause.load(Ordering::Relaxed)
    }

    /// 一時中断が解けるか止められるまで待つ。
    pub(crate) fn wait_while_paused(&self) {
        while self.paused() && !self.cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub(crate) fn step(&self, current: &str) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = current.to_owned();
    }
}

/// 調べ終わった結果。
#[derive(Debug)]
pub struct Report {
    pub catalog: Catalog,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}

/// `dir` 以下の画像（ZIP / 7z の中も）のメタデータを集めてカタログを作る。`previous` があれば
/// 作った日時を引き継ぎ、ファイルの増減を数える。暗号化されたアーカイブは `ask_password` で
/// パスワードを聞き（`true` なら前に聞いたものが違った）、`None` ならそのアーカイブを飛ばす。
/// `ai_tags` に記録のある画像は、AI タグと写っている人物も組にする。
/// `labels` で地図に置いた位置がある画像は、EXIF の位置の代わりにそれを入れる。
///
/// 前のカタログのうち変わっていないファイルの組を使い回さず、全ファイルを読み直す。
/// 組はデータセット全体で 1 つずつしか持たないので、消えたファイルの組だけを除くことが
/// できないため。
#[allow(clippy::too_many_arguments)]
pub fn scan(
    dir: &Path,
    info: Info,
    previous: Option<&Catalog>,
    registry: &Registry,
    ai_tags: &AiTagStore,
    labels: &LabelStore,
    progress: &Progress,
    ask_password: &mut dyn FnMut(&Path, bool) -> Option<String>,
) -> Result<Report, String> {
    scan_resumable(
        dir,
        info,
        previous,
        registry,
        ai_tags,
        labels,
        progress,
        ask_password,
        Resume::Off,
    )
}

/// [`scan`] と同じく調べる。`resume` が [`Resume::Off`] でなければ、読み終えた分を
/// [`PARTIAL_FILE`] に書きながら進め、止めても落ちても続きから始められるようにする。
/// 続きからでは、前回読んでいる最中に落ちたファイルを読まずに [`Catalog::skipped`] に入れる。
/// 途中の結果は残すので、カタログを保存できたら [`discard_unfinished`] で消す。
#[allow(clippy::too_many_arguments)]
pub fn scan_resumable(
    dir: &Path,
    info: Info,
    previous: Option<&Catalog>,
    registry: &Registry,
    ai_tags: &AiTagStore,
    labels: &LabelStore,
    progress: &Progress,
    ask_password: &mut dyn FnMut(&Path, bool) -> Option<String>,
    resume: Resume,
) -> Result<Report, String> {
    let mut candidates = Vec::new();
    walk(dir, registry, &mut candidates)?;
    progress.total.store(candidates.len(), Ordering::Relaxed);
    let mut state = match resume {
        Resume::Continue => load_partial(dir).map(|mut p| {
            if let Some(c) = crashed_file(dir, &p) {
                p.skipped.push(skip(&c, CRASHED));
                p.crashed.push(c);
            }
            p
        }),
        Resume::Fresh => {
            discard_unfinished(dir);
            None
        }
        Resume::Off => None,
    }
    .unwrap_or(Partial {
        format: FORMAT,
        ..Partial::default()
    });
    state.info = Some(info.clone());
    // 止めている間に消えたファイルは、読み終えた分からも外す。
    let present: std::collections::HashSet<String> =
        candidates.iter().map(|p| relative_name(dir, p)).collect();
    let still_there = |path: &str| present.contains(path.split(IN_ARCHIVE).next().unwrap_or(path));
    state.files.retain(|f| still_there(&f.path));
    state.skipped.retain(|f| still_there(&f.path));
    state.done.retain(|d| still_there(d));
    let mut builder = Builder::from_parts(
        std::mem::take(&mut state.meta),
        std::mem::take(&mut state.files),
    );
    let mut problems = std::mem::take(&mut state.skipped);
    let done: std::collections::HashSet<String> = state.done.iter().cloned().collect();
    let keep = resume != Resume::Off;
    let reading = |name: &str| {
        if keep {
            let _ = std::fs::write(dir.join(READING_FILE), name);
        }
    };
    let mut checkpoint_at = std::time::Instant::now();
    let checkpoint = |builder: &Builder, problems: &[Skipped], state: &Partial| {
        if keep {
            save_partial(
                dir,
                &Partial {
                    meta: builder.triples.clone(),
                    files: builder.files.clone(),
                    skipped: problems.to_vec(),
                    ..state.clone()
                },
            );
        }
    };
    for path in &candidates {
        if progress.paused() {
            // 一時中断したまま vw を閉じたり落ちたりしても、そこまでの分から続けられるように。
            checkpoint(&builder, &problems, &state);
            checkpoint_at = std::time::Instant::now();
            reading("");
            progress.wait_while_paused();
        }
        if progress.cancelled() {
            checkpoint(&builder, &problems, &state);
            // 止めただけで落ちたのではないので、読んでいたファイルを次に飛ばさないよう消す。
            let _ = std::fs::remove_file(dir.join(READING_FILE));
            return Err("Cancelled".into());
        }
        let relative = relative_name(dir, path);
        if done.contains(&relative) || state.crashed.contains(&relative) {
            progress.done.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        progress.step(&relative);
        reading(&relative);
        let before = (builder.files.len(), problems.len());
        let modified = modified_time(path);
        if archive::is_archive(path) {
            scan_archive(
                path,
                &relative,
                &modified,
                registry,
                progress,
                ask_password,
                &mut builder,
                &mut problems,
                &state.crashed,
                &reading,
            );
        } else {
            match std::fs::read(path) {
                Ok(data) => {
                    let size = data.len() as u64;
                    let label = labels.get(path);
                    match guarded_triples(registry, path, &data, label.position) {
                        Ok(mut t) => {
                            if let Some(entry) = ai_tags.get(path) {
                                t.extend(ai_triples(&entry));
                            }
                            t.extend(label_triples(&label));
                            builder.add(relative.clone(), size, modified, t)
                        }
                        Err(e) => problems.push(skip(&relative, e)),
                    }
                }
                Err(e) => problems.push(skip(&relative, e.to_string())),
            }
        }
        if progress.cancelled() {
            // 途中まで読んだアーカイブの中身は捨てる。続きからではアーカイブを頭から読み直すので、残すと二重になる。
            builder.files.truncate(before.0);
            problems.truncate(before.1);
        } else {
            state.done.push(relative);
        }
        progress.done.fetch_add(1, Ordering::Relaxed);
        if checkpoint_at.elapsed() >= CHECKPOINT_EVERY {
            checkpoint(&builder, &problems, &state);
            checkpoint_at = std::time::Instant::now();
        }
    }
    if progress.cancelled() {
        checkpoint(&builder, &problems, &state);
        let _ = std::fs::remove_file(dir.join(READING_FILE));
        return Err("Cancelled".into());
    }
    if keep {
        // 途中の結果は、カタログを保存できるまで残す。保存に失敗しても続きから始められるように。
        checkpoint(&builder, &problems, &state);
        let _ = std::fs::remove_file(dir.join(READING_FILE));
    }
    let now = now();
    let mut catalog = Catalog {
        format: FORMAT,
        created: previous.map_or_else(|| now.clone(), |p| p.created.clone()),
        updated: now,
        source: previous.and_then(|p| p.source.clone()),
        meta: builder.triples,
        files: builder.files,
        skipped: problems,
        ..Catalog::default()
    };
    catalog.set_info(info);
    let (added, removed, changed) = previous.map_or((catalog.files.len(), 0, 0), |p| {
        differences(&p.files, &catalog.files)
    });
    Ok(Report {
        catalog,
        added,
        removed,
        changed,
    })
}

pub fn skip(path: &str, reason: impl Into<String>) -> Skipped {
    Skipped {
        path: path.to_owned(),
        reason: reason.into(),
    }
}

/// 調べるファイル（画像とアーカイブ）を名前順に集める。
fn walk(dir: &Path, registry: &Registry, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let Ok(kind) = e.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !crate::tree::is_recycle_bin(&e.file_name().to_string_lossy()) {
                walk(&path, registry, out)?;
            }
        } else if kind.is_file() && (archive::is_archive(&path) || registry.supports_file(&path)) {
            out.push(path);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn scan_archive(
    path: &Path,
    relative: &str,
    modified: &str,
    registry: &Registry,
    progress: &Progress,
    ask_password: &mut dyn FnMut(&Path, bool) -> Option<String>,
    builder: &mut Builder,
    problems: &mut Vec<Skipped>,
    crashed: &[String],
    reading: &dyn Fn(&str),
) {
    let mut password = None;
    loop {
        // パスワードが違うと途中で失敗するので、読み終わるまでは別に貯めておく。
        let mut found = Vec::new();
        let result = archive::for_each_entry(path, password.as_deref(), |inner, data| {
            if progress.paused() {
                // 最後に読んだ項目は読み終えている。止めている間に閉じても、落ちたとみなさないように。
                reading("");
                progress.wait_while_paused();
            }
            if progress.cancelled() {
                return Err(EntryError::Other("Cancelled".into()));
            }
            let Some(data) = data else {
                return Ok(());
            };
            if !registry.supports_path(inner) && !registry.needs_probe(inner) {
                return Ok(());
            }
            // 展開した大きさは項目の見出しに書いてあるとおりとは限らないので、画像の上限まで読み、
            // それでも続く項目はカタログに入れない（画像としても開けない大きさのため）。
            let limit = vw_core::limits::max_image_bytes() as u64;
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(data, limit + 1), &mut bytes)
                .map_err(EntryError::Read)?;
            if bytes.len() as u64 > limit {
                return Ok(());
            }
            let name = format!("{relative}{IN_ARCHIVE}{}", slash_path(inner));
            if crashed.contains(&name) {
                return Ok(());
            }
            reading(&name);
            if let Ok(t) = guarded_triples(registry, inner, &bytes, None) {
                found.push((name, bytes.len() as u64, t));
            } else if registry.supports_path(inner) {
                // 中身で判定するだけのファイル（拡張子のないものなど）は、画像でなくても問題にしない。
                found.push((name, bytes.len() as u64, Vec::new()));
            }
            Ok(())
        });
        match result {
            Ok(_) => {
                for (name, size, t) in found {
                    if t.is_empty() {
                        problems.push(skip(&name, "cannot read the metadata"));
                    } else {
                        builder.add(name, size, modified.to_owned(), t);
                    }
                }
                return;
            }
            Err(ArchiveError::NeedsPassword | ArchiveError::WrongPassword)
                if !progress.cancelled() =>
            {
                let wrong = password.is_some();
                match ask_password(path, wrong) {
                    Some(p) => password = Some(p),
                    None => {
                        problems.push(skip(relative, PASSWORD_SKIPPED));
                        return;
                    }
                }
            }
            Err(ArchiveError::NeedsPassword | ArchiveError::WrongPassword) => return,
            Err(ArchiveError::Other(e)) => {
                if !progress.cancelled() {
                    problems.push(skip(relative, e));
                }
                return;
            }
        }
    }
}

/// 1 ファイルのメタデータを組にする。
/// [`triples`] と同じ。読み手が panic したら、そのファイルを読めなかったことにする。
/// 落ちたら続きから飛ばせるようにはしてあるが、panic で済むものはその場で飛ばして先へ進めるため。
fn guarded_triples(
    registry: &Registry,
    path: &Path,
    data: &[u8],
    placed: Option<Position>,
) -> Result<Vec<Triple>, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        triples(registry, path, data, placed)
    }))
    .unwrap_or_else(|panic| {
        Err(format!(
            "internal error: {}",
            crate::loader::panic_text(&panic)
        ))
    })
}

fn triples(
    registry: &Registry,
    path: &Path,
    data: &[u8],
    placed: Option<Position>,
) -> Result<Vec<Triple>, String> {
    let (mut metadata, plugin) = registry
        .read_all_metadata_bytes(path, data)
        .map_err(|e| e.to_string())?;
    if let Some(p) = placed {
        labels::apply_position(&mut metadata, p);
    }
    let mut out: Vec<Triple> = basics(plugin, &metadata)
        .into_iter()
        .map(|(k, v)| ("image".to_owned(), k.to_owned(), v))
        .collect();
    let location = vw_core::exif::LOCATION.to_lowercase();
    if let (Some(g), Some(gps)) = (
        crate::geonames::current(),
        vw_core::exif::Gps::from_metadata(&metadata),
    ) {
        out.extend(
            crate::geonames::catalog_triples(&g, gps.latitude, gps.longitude)
                .into_iter()
                .map(|(k, v)| (location.clone(), k, v)),
        );
    }
    out.extend(
        metadata
            .entries
            .into_iter()
            .map(|(group, key, value)| (group.to_lowercase(), key, limit(value))),
    );
    Ok(out)
}

/// AI タグ（一般・キャラクター・レーティング）と、名前の付いた顔の人物を組にする。
///
/// 確信度は入れない。タグを付けるときに閾値で選んだものだけが残っているうえ、確信度まで
/// 入れると同じタグでも画像ごとに別の組になり、組を 1 つにまとめて小さくした意味がなくなるため。
/// ZIP / 7z の中の画像には AI タグを付けられないので、ここでも見ない。
fn ai_triples(entry: &ai_tags::Entry) -> Vec<Triple> {
    let triple =
        |key: &str, value: &str| (AI_TAGS.to_owned(), key.to_owned(), limit(value.to_owned()));
    let tags = &entry.tags;
    let mut out: Vec<Triple> = entry
        .people()
        .into_iter()
        .map(|p| triple("person", p))
        .collect();
    out.extend(tags.general.iter().map(|t| triple("tag", &t.name)));
    out.extend(tags.characters.iter().map(|t| triple("character", &t.name)));
    out.extend(tags.rating.iter().map(|t| triple("rating", &t.name)));
    if let Some((model, version)) = entry.tagger() {
        out.push(triple("model", model));
        out.push(triple("model version", version));
    }
    if let Some((model, version)) = entry.face_finder() {
        out.push(triple("face model", model));
        out.push(triple("face model version", version));
    }
    out
}

/// 手で付けたフラグ（good / bad）とタグを組にする。
/// ZIP / 7z の中の画像にはフラグもタグも付けられないので、ここでも見ない。
fn label_triples(label: &labels::Label) -> Vec<Triple> {
    let triple = |key: &str, value: &str| (LABELS.to_owned(), key.to_owned(), value.to_owned());
    let flag = label.flag.map(|f| match f {
        labels::Flag::Good => "good",
        labels::Flag::Bad => "bad",
    });
    flag.into_iter()
        .map(|f| triple("flag", f))
        .chain(label.tags.iter().map(|t| triple("tag", t)))
        .collect()
}

/// これより長い値はカタログに入れない。画像のデータを文字列のタグに入れたファイルがあり、
/// そのまま入れるとカタログが膨らむうえ検索の役にも立たないため。
const LONGEST_VALUE: usize = 2048;

/// 長すぎる値は、キーを「存在する」で探せるよう、値の代わりに長さを入れる。
fn limit(value: String) -> String {
    if value.len() <= LONGEST_VALUE {
        value
    } else {
        format!("(2KB 超: {} バイト)", value.len())
    }
}

/// 形式によらない基本情報（形式名・幅・高さ・ビット深度）。形式ごとのメタデータの書き方から拾う。
/// プラグインに別の口を足さないのは、どの形式もすでに同じ情報をメタデータ欄に出しているため。
fn basics(plugin: &str, m: &Metadata) -> Vec<(&'static str, String)> {
    let mut out = vec![("format", plugin.to_owned())];
    let size = m
        .entries
        .iter()
        .find(|(_, k, _)| k == "Size")
        .and_then(|(_, _, v)| v.split_once(" x "))
        .map(|(w, h)| (w.trim().to_owned(), h.trim().to_owned()));
    let dicom = |key| m.get("DICOM", key).map(str::to_owned);
    let (width, height) = match size {
        Some((w, h)) => (Some(w), Some(h)),
        None => (dicom("0028,0011"), dicom("0028,0010")),
    };
    let depth = ["Bit depth", "Bits per pixel"]
        .iter()
        .find_map(|key| m.entries.iter().find(|(_, k, _)| k == key))
        .map(|(_, _, v)| v.clone())
        .or_else(|| dicom("0028,0101"));
    for (key, value) in [("width", width), ("height", height), ("bit depth", depth)] {
        if let Some(v) = value {
            out.push((key, v));
        }
    }
    out
}

/// 組の辞書とファイルの一覧を作る。
#[derive(Default)]
struct Builder {
    triples: Vec<Triple>,
    index: HashMap<Triple, u32>,
    files: Vec<FileEntry>,
}

impl Builder {
    /// 途中の結果から作り直す。
    fn from_parts(triples: Vec<Triple>, files: Vec<FileEntry>) -> Self {
        let index = triples
            .iter()
            .enumerate()
            .map(|(i, t)| (t.clone(), i as u32))
            .collect();
        Self {
            triples,
            index,
            files,
        }
    }

    fn add(&mut self, path: String, size: u64, modified: String, triples: Vec<Triple>) {
        let mut ids: Vec<u32> = triples
            .into_iter()
            .map(|t| {
                let next = self.triples.len() as u32;
                *self.index.entry(t.clone()).or_insert_with(|| {
                    self.triples.push(t);
                    next
                })
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        self.files.push(FileEntry {
            path,
            size,
            modified,
            meta: ids,
        });
    }
}

/// 増えた・消えた・変わった（大きさか更新時刻が違う）ファイルの数。
fn differences(old: &[FileEntry], new: &[FileEntry]) -> (usize, usize, usize) {
    let before: HashMap<&str, &FileEntry> = old.iter().map(|f| (f.path.as_str(), f)).collect();
    let after: HashMap<&str, &FileEntry> = new.iter().map(|f| (f.path.as_str(), f)).collect();
    let added = new
        .iter()
        .filter(|f| !before.contains_key(f.path.as_str()))
        .count();
    let removed = old
        .iter()
        .filter(|f| !after.contains_key(f.path.as_str()))
        .count();
    let changed = new
        .iter()
        .filter(|f| {
            before
                .get(f.path.as_str())
                .is_some_and(|b| b.size != f.size || b.modified != f.modified)
        })
        .count();
    (added, removed, changed)
}

fn relative_name(root: &Path, path: &Path) -> String {
    slash_path(path.strip_prefix(root).unwrap_or(path))
}

/// OS によらず `/` で区切る。Windows で作ったカタログを Linux でも使えるように。
pub fn slash_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn modified_time(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(rfc3339)
        .unwrap_or_default()
}

fn now() -> String {
    rfc3339(std::time::SystemTime::now())
}

/// UTC で書く。地方時にしないのは、時差の違う場所で作ったカタログを比べても、同じ時刻が
/// 同じ文字列になるようにするため。
fn rfc3339(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{TempDir, all_registry, png_bytes, png_registry, write_png, write_zip};

    fn no_password(_: &Path, _: bool) -> Option<String> {
        None
    }

    fn run(dir: &Path, previous: Option<&Catalog>) -> Result<Report, String> {
        let info = Info {
            name: "set".into(),
            ..Info::default()
        };
        scan(
            dir,
            info,
            previous,
            &png_registry(),
            &AiTagStore::new(None),
            &LabelStore::new(None),
            &Progress::default(),
            &mut no_password,
        )
    }

    fn values_of<'a>(c: &'a Catalog, file: &str, kind: &str, key: &str) -> Vec<&'a str> {
        let f = c.files.iter().find(|f| f.path == file).unwrap();
        f.meta
            .iter()
            .map(|&i| &c.meta[i as usize])
            .filter(|(k, key2, _)| k == kind && key2 == key)
            .map(|(_, _, v)| v.as_str())
            .collect()
    }

    #[test]
    fn images_in_the_recycle_bin_are_left_out_of_the_catalog() {
        let tmp = TempDir::new();
        write_png(&tmp.mkdir("$RECYCLE.BIN").join("$R1.png"), 2, 3, &[0; 18]);
        write_png(&tmp.path().join("kept.png"), 2, 3, &[0; 18]);
        let report = run(tmp.path(), None).unwrap();
        let paths: Vec<_> = report
            .catalog
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(paths, ["kept.png"]);
    }

    #[test]
    fn images_in_subfolders_are_listed_with_slash_separated_paths() {
        let tmp = TempDir::new();
        let sub = tmp.mkdir("a/b");
        write_png(&sub.join("x.png"), 2, 3, &[0; 18]);
        std::fs::write(tmp.path().join("notes.txt"), "not an image").unwrap();
        let report = run(tmp.path(), None).unwrap();
        let paths: Vec<_> = report
            .catalog
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(paths, ["a/b/x.png"]);
        assert_eq!(report.added, 1);
        let c = &report.catalog;
        assert_eq!(values_of(c, "a/b/x.png", "image", "format"), ["PNG"]);
        assert_eq!(values_of(c, "a/b/x.png", "image", "width"), ["2"]);
        assert_eq!(values_of(c, "a/b/x.png", "image", "height"), ["3"]);
        assert_eq!(values_of(c, "a/b/x.png", "png", "Bit depth"), ["8"]);
    }

    #[test]
    fn the_same_triple_is_stored_once_for_the_whole_dataset() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("1.png"), 4, 4, &[0; 48]);
        write_png(&tmp.path().join("2.png"), 4, 4, &[9; 48]);
        let c = run(tmp.path(), None).unwrap().catalog;
        let widths = c.meta.iter().filter(|(_, k, _)| k == "width").count();
        assert_eq!(widths, 1);
        assert_eq!(c.files[0].meta, c.files[1].meta);
    }

    #[test]
    fn dicom_tags_are_stored_by_number() {
        let tmp = TempDir::new();
        let bytes = vw_dicom::synth::Synth::ct(2, 2).patient("Doe^Jane").bytes();
        std::fs::write(tmp.path().join("IM0001"), bytes).unwrap();
        let info = Info::default();
        let report = scan(
            tmp.path(),
            info,
            None,
            &all_registry(),
            &AiTagStore::new(None),
            &LabelStore::new(None),
            &Progress::default(),
            &mut no_password,
        )
        .unwrap();
        let c = &report.catalog;
        assert_eq!(values_of(c, "IM0001", "dicom", "0010,0010"), ["Doe^Jane"]);
        assert_eq!(values_of(c, "IM0001", "image", "width"), ["2"]);
    }

    #[test]
    fn position_placed_on_the_map_is_stored_for_searching() {
        let tmp = TempDir::new();
        let image = tmp.path().join("a.png");
        write_png(&image, 1, 1, &[0; 3]);
        let labels = LabelStore::new(None);
        labels
            .set_position(
                &image,
                Some(Position {
                    latitude: 43.0621,
                    longitude: 141.3544,
                }),
            )
            .unwrap();
        let report = scan(
            tmp.path(),
            Info::default(),
            None,
            &png_registry(),
            &AiTagStore::new(None),
            &labels,
            &Progress::default(),
            &mut no_password,
        )
        .unwrap();
        let c = &report.catalog;
        assert_eq!(values_of(c, "a.png", "location", "Latitude"), ["43.062100"]);
        assert_eq!(
            values_of(c, "a.png", "location", labels::POSITION_FROM),
            [labels::FROM_MAP]
        );
    }

    #[test]
    fn flags_and_tags_put_on_by_hand_are_stored_for_searching() {
        let tmp = TempDir::new();
        let (a, b) = (tmp.path().join("a.png"), tmp.path().join("b.png"));
        write_png(&a, 1, 1, &[0; 3]);
        write_png(&b, 1, 1, &[0; 3]);
        let labels = LabelStore::new(None);
        labels.toggle_flag(&a, labels::Flag::Good).unwrap();
        labels.toggle_tag(&a, "cat").unwrap();
        labels.toggle_tag(&a, "night").unwrap();
        labels.toggle_flag(&b, labels::Flag::Bad).unwrap();
        let report = scan(
            tmp.path(),
            Info::default(),
            None,
            &png_registry(),
            &AiTagStore::new(None),
            &labels,
            &Progress::default(),
            &mut no_password,
        )
        .unwrap();
        let c = &report.catalog;
        assert_eq!(values_of(c, "a.png", LABELS, "flag"), ["good"]);
        assert_eq!(values_of(c, "a.png", LABELS, "tag"), ["cat", "night"]);
        assert_eq!(values_of(c, "b.png", LABELS, "flag"), ["bad"]);
        assert!(values_of(c, "b.png", LABELS, "tag").is_empty());
    }

    #[test]
    fn ai_tags_and_named_people_are_stored_for_searching() {
        use crate::faces::FaceMark;
        use crate::tagger::Scored;
        let tmp = TempDir::new();
        let image = tmp.path().join("a.png");
        write_png(&image, 1, 1, &[0; 3]);
        write_png(&tmp.path().join("b.png"), 1, 1, &[0; 3]);
        let face = |person: Option<&str>| FaceMark {
            rect: [0.1, 0.1, 0.2, 0.2],
            score: 0.9,
            person: person.map(str::to_owned),
            similarity: 0.6,
            unlinked: false,
            hidden: false,
        };
        let scored = |name: &str| Scored {
            name: name.into(),
            score: 0.8,
        };
        let store = AiTagStore::new(None);
        store
            .update(&image, |e| {
                e.model = "tagger".into();
                e.tags.general = vec![scored("1girl"), scored("smile")];
                e.tags.characters = vec![scored("hatsune miku")];
                e.tags.rating = Some(scored("general"));
                e.faces = vec![face(Some("Taro")), face(None), face(Some("Taro"))];
            })
            .unwrap();

        let report = scan(
            tmp.path(),
            Info::default(),
            None,
            &png_registry(),
            &AiTagStore::new(None),
            &LabelStore::new(None),
            &Progress::default(),
            &mut no_password,
        )
        .unwrap();

        let c = &report.catalog;
        assert_eq!(values_of(c, "a.png", AI_TAGS, "person"), ["Taro"]);
        let mut tags = values_of(c, "a.png", AI_TAGS, "tag");
        tags.sort_unstable();
        assert_eq!(tags, ["1girl", "smile"]);
        assert_eq!(
            values_of(c, "a.png", AI_TAGS, "character"),
            ["hatsune miku"]
        );
        assert_eq!(values_of(c, "a.png", AI_TAGS, "rating"), ["general"]);
        assert_eq!(values_of(c, "a.png", AI_TAGS, "model"), ["tagger"]);
        assert_eq!(
            values_of(c, "a.png", AI_TAGS, "face model"),
            [crate::ai_models::FACES_NAME]
        );
        assert!(values_of(c, "b.png", AI_TAGS, "tag").is_empty());
    }

    #[test]
    fn images_inside_zip_files_are_listed_after_an_exclamation_mark() {
        let tmp = TempDir::new();
        let png = png_bytes(1, 1);
        write_zip(
            &tmp.path().join("data.zip"),
            &[("s1/a.png", &png), ("readme.txt", b"hi")],
            None,
        );
        let c = run(tmp.path(), None).unwrap().catalog;
        let paths: Vec<_> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["data.zip!/s1/a.png"]);
    }

    #[test]
    fn encrypted_zip_is_read_with_the_password_given_after_a_wrong_one() {
        let tmp = TempDir::new();
        let png = png_bytes(1, 1);
        write_zip(
            &tmp.path().join("secret.zip"),
            &[("a.png", &png)],
            Some("vw"),
        );
        let mut asked = Vec::new();
        let mut answer = |_: &Path, wrong: bool| {
            asked.push(wrong);
            Some(if wrong { "vw" } else { "nope" }.to_owned())
        };
        let report = scan(
            tmp.path(),
            Info::default(),
            None,
            &png_registry(),
            &AiTagStore::new(None),
            &LabelStore::new(None),
            &Progress::default(),
            &mut answer,
        )
        .unwrap();
        assert_eq!(asked, [false, true]);
        assert_eq!(report.catalog.files[0].path, "secret.zip!/a.png");
        assert!(report.catalog.skipped.is_empty());
    }

    #[test]
    fn skipping_the_password_leaves_the_zip_out_and_says_so() {
        let tmp = TempDir::new();
        let png = png_bytes(1, 1);
        write_zip(
            &tmp.path().join("secret.zip"),
            &[("a.png", &png)],
            Some("vw"),
        );
        write_png(&tmp.path().join("b.png"), 1, 1, &[0; 3]);
        let report = run(tmp.path(), None).unwrap();
        assert_eq!(report.catalog.files.len(), 1);
        let skipped = &report.catalog.skipped;
        assert_eq!(skipped, &[skip("secret.zip", "password")]);
        assert_eq!(skipped[0].to_string(), "secret.zip: skipped (no password)");
    }

    #[test]
    fn updating_counts_added_removed_and_changed_files_and_keeps_the_creation_time() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("keep.png"), 1, 1, &[0; 3]);
        write_png(&tmp.path().join("change.png"), 1, 1, &[0; 3]);
        write_png(&tmp.path().join("remove.png"), 1, 1, &[0; 3]);
        let mut first = run(tmp.path(), None).unwrap().catalog;
        first.created = "2020-01-01T00:00:00Z".into();
        write_png(&tmp.path().join("change.png"), 2, 2, &[0; 12]);
        std::fs::remove_file(tmp.path().join("remove.png")).unwrap();
        write_png(&tmp.path().join("new.png"), 1, 1, &[0; 3]);
        let report = run(tmp.path(), Some(&first)).unwrap();
        assert_eq!((report.added, report.removed, report.changed), (1, 1, 1));
        assert_eq!(report.catalog.created, "2020-01-01T00:00:00Z");
    }

    #[test]
    fn a_dataset_inside_the_folder_is_included_in_the_outer_one() {
        let tmp = TempDir::new();
        let inner = tmp.mkdir("inner");
        write_png(&inner.join("a.png"), 1, 1, &[0; 3]);
        std::fs::write(inner.join(FILE_NAME), "{}").unwrap();
        let c = run(tmp.path(), None).unwrap().catalog;
        let paths: Vec<_> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["inner/a.png"]);
        assert!(check_can_register(&inner).is_ok());
    }

    #[test]
    fn values_longer_than_2kb_keep_only_their_length() {
        assert_eq!(limit("x".repeat(2048)).len(), 2048);
        assert_eq!(limit("x".repeat(3000)), "(2KB 超: 3000 バイト)");
    }

    #[test]
    fn saved_catalog_is_read_back_with_unknown_fields_kept() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        let mut c = run(tmp.path(), None).unwrap().catalog;
        c.fields.insert("Source".into(), "Hospital A".into());
        save(tmp.path(), &c).unwrap();
        assert_eq!(load(tmp.path()).unwrap(), c);
        assert!(!tmp.path().join(format!("{FILE_NAME}.tmp")).exists());
    }

    #[test]
    fn catalog_written_by_hand_with_only_a_name_can_be_read() {
        let tmp = TempDir::new();
        std::fs::write(tmp.path().join(FILE_NAME), r#"{"format":1,"name":"x"}"#).unwrap();
        assert_eq!(load(tmp.path()).unwrap().name, "x");
    }

    #[test]
    fn cancelling_stops_the_scan() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        let progress = Progress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        let result = scan(
            tmp.path(),
            Info::default(),
            None,
            &png_registry(),
            &AiTagStore::new(None),
            &LabelStore::new(None),
            &progress,
            &mut no_password,
        );
        assert!(result.is_err());
    }

    fn run_resumable(dir: &Path, resume: Resume, progress: &Progress) -> Result<Report, String> {
        let info = Info {
            name: "set".into(),
            ..Info::default()
        };
        scan_resumable(
            dir,
            info,
            None,
            &png_registry(),
            &AiTagStore::new(None),
            &LabelStore::new(None),
            progress,
            &mut no_password,
            resume,
        )
    }

    fn size_of(c: &Catalog, file: &str) -> u64 {
        c.files.iter().find(|f| f.path == file).unwrap().size
    }

    #[test]
    fn stopping_keeps_what_was_read_for_the_next_time() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        let progress = Progress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        assert!(run_resumable(tmp.path(), Resume::Fresh, &progress).is_err());
        assert_eq!(
            unfinished(tmp.path()),
            Some(Unfinished {
                done: 0,
                crashed: None,
                info: Some(Info {
                    name: "set".into(),
                    ..Info::default()
                })
            })
        );
        assert!(!tmp.path().join(READING_FILE).exists());
    }

    #[test]
    fn continuing_reads_only_the_files_not_read_yet() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        // 途中の結果は保存できるまで残るので、a.png まで読んで止まったのと同じになる。
        let first = run_resumable(tmp.path(), Resume::Fresh, &Progress::default()).unwrap();
        write_png(&tmp.path().join("a.png"), 4, 4, &[9; 48]);
        write_png(&tmp.path().join("b.png"), 2, 2, &[0; 12]);
        let c = run_resumable(tmp.path(), Resume::Continue, &Progress::default())
            .unwrap()
            .catalog;
        let paths: Vec<_> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.png", "b.png"]);
        assert_eq!(size_of(&c, "a.png"), size_of(&first.catalog, "a.png"));
        assert_eq!(values_of(&c, "b.png", "image", "width"), ["2"]);
    }

    #[test]
    fn continuing_gives_the_same_catalog_as_reading_from_the_start() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        run_resumable(tmp.path(), Resume::Fresh, &Progress::default()).unwrap();
        write_png(&tmp.path().join("b.png"), 2, 2, &[7; 12]);
        let resumed = run_resumable(tmp.path(), Resume::Continue, &Progress::default())
            .unwrap()
            .catalog;
        let fresh = run(tmp.path(), None).unwrap().catalog;
        assert_eq!(resumed.files, fresh.files);
        assert_eq!(resumed.meta, fresh.meta);
    }

    #[test]
    fn a_file_vw_crashed_on_is_skipped_when_continuing() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        run_resumable(tmp.path(), Resume::Fresh, &Progress::default()).unwrap();
        write_png(&tmp.path().join("b.png"), 2, 2, &[0; 12]);
        write_png(&tmp.path().join("c.png"), 3, 3, &[0; 27]);
        std::fs::write(tmp.path().join(READING_FILE), "b.png").unwrap();
        assert_eq!(
            unfinished(tmp.path()).unwrap().crashed.as_deref(),
            Some("b.png")
        );
        let c = run_resumable(tmp.path(), Resume::Continue, &Progress::default())
            .unwrap()
            .catalog;
        let paths: Vec<_> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.png", "c.png"]);
        assert_eq!(c.skipped, [skip("b.png", CRASHED)]);
    }

    #[test]
    fn an_image_in_a_zip_vw_crashed_on_is_skipped_but_the_rest_of_the_zip_is_read() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        run_resumable(tmp.path(), Resume::Fresh, &Progress::default()).unwrap();
        let png = png_bytes(1, 1);
        write_zip(
            &tmp.path().join("data.zip"),
            &[("x.png", &png), ("y.png", &png)],
            None,
        );
        std::fs::write(tmp.path().join(READING_FILE), "data.zip!/x.png").unwrap();
        let c = run_resumable(tmp.path(), Resume::Continue, &Progress::default())
            .unwrap()
            .catalog;
        let paths: Vec<_> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.png", "data.zip!/y.png"]);
        assert_eq!(c.skipped, [skip("data.zip!/x.png", CRASHED)]);
    }

    #[test]
    fn reading_the_last_file_to_the_end_is_not_taken_as_a_crash() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        run_resumable(tmp.path(), Resume::Fresh, &Progress::default()).unwrap();
        std::fs::write(tmp.path().join(READING_FILE), "a.png").unwrap();
        assert_eq!(unfinished(tmp.path()).unwrap().crashed, None);
    }

    #[test]
    fn reading_from_the_start_throws_away_what_was_left() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        run_resumable(tmp.path(), Resume::Fresh, &Progress::default()).unwrap();
        write_png(&tmp.path().join("a.png"), 4, 4, &[9; 48]);
        let c = run_resumable(tmp.path(), Resume::Fresh, &Progress::default())
            .unwrap()
            .catalog;
        assert_eq!(values_of(&c, "a.png", "image", "width"), ["4"]);
    }

    #[test]
    fn a_plain_scan_leaves_nothing_to_continue() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        run(tmp.path(), None).unwrap();
        assert_eq!(unfinished(tmp.path()), None);
    }

    #[test]
    fn files_removed_while_stopped_are_left_out_when_continuing() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        write_png(&tmp.path().join("b.png"), 1, 1, &[0; 3]);
        run_resumable(tmp.path(), Resume::Fresh, &Progress::default()).unwrap();
        std::fs::remove_file(tmp.path().join("a.png")).unwrap();
        let c = run_resumable(tmp.path(), Resume::Continue, &Progress::default())
            .unwrap()
            .catalog;
        let paths: Vec<_> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["b.png"]);
    }

    #[test]
    fn a_zip_stopped_partway_is_read_again_from_its_start_when_continuing() {
        let tmp = TempDir::new();
        let png = png_bytes(1, 1);
        write_zip(
            &tmp.path().join("secret.zip"),
            &[("a.png", &png)],
            Some("vw"),
        );
        let progress = Progress::default();
        let mut stop = |_: &Path, _: bool| {
            progress.cancel.store(true, Ordering::Relaxed);
            None
        };
        let stopped = scan_resumable(
            tmp.path(),
            Info::default(),
            None,
            &png_registry(),
            &AiTagStore::new(None),
            &LabelStore::new(None),
            &progress,
            &mut stop,
            Resume::Fresh,
        );
        assert!(stopped.is_err());
        assert_eq!(unfinished(tmp.path()).unwrap().done, 0);
        let mut answer = |_: &Path, _: bool| Some("vw".to_owned());
        let c = scan_resumable(
            tmp.path(),
            Info::default(),
            None,
            &png_registry(),
            &AiTagStore::new(None),
            &LabelStore::new(None),
            &Progress::default(),
            &mut answer,
            Resume::Continue,
        )
        .unwrap()
        .catalog;
        assert_eq!(c.files.len(), 1);
        assert!(c.skipped.is_empty(), "{:?}", c.skipped);
    }

    #[test]
    fn a_paused_scan_reads_nothing_until_resumed_and_keeps_its_place_meanwhile() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        let progress = Progress::default();
        progress.pause.store(true, Ordering::Relaxed);
        std::thread::scope(|s| {
            let scan = s.spawn(|| run_resumable(tmp.path(), Resume::Fresh, &progress));
            while unfinished(tmp.path()).is_none() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            assert_eq!(progress.done.load(Ordering::Relaxed), 0);
            progress.pause.store(false, Ordering::Relaxed);
            let c = scan.join().unwrap().unwrap().catalog;
            assert_eq!(c.files.len(), 1);
        });
    }

    #[test]
    fn stopping_a_paused_scan_ends_it() {
        let tmp = TempDir::new();
        write_png(&tmp.path().join("a.png"), 1, 1, &[0; 3]);
        let progress = Progress::default();
        progress.pause.store(true, Ordering::Relaxed);
        std::thread::scope(|s| {
            let scan = s.spawn(|| run_resumable(tmp.path(), Resume::Fresh, &progress));
            std::thread::sleep(std::time::Duration::from_millis(100));
            progress.cancel.store(true, Ordering::Relaxed);
            assert!(scan.join().unwrap().is_err());
        });
    }
}
