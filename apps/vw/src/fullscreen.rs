//! 全画面スライドショー。起点のフォルダ以下の、画像のあるフォルダを順に回る。
//! 最も新しいファイルを追う（テザー撮影向け）こともできる。
//!
//! 通常の画面のスロットとは別に、見せる画像の並びと読み込みを自分で持つ。スロットに
//! 読み込むと、フォルダ全体（数万枚になりうる）の縮小画像を作ることになり、
//! 1 枚ずつ大きく見せるだけの用途には重すぎるため。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, Rect, TextureHandle, TextureOptions, pos2};
use vw_core::Registry;

use crate::frames;
use crate::i18n::{tr, tr1};
use crate::keymap::{Action, Keymap, Scope};
use crate::loader::{ImageInfo, Job, Loader};
use crate::settings::{Fullscreen, FullscreenMode, Language};
use crate::sort::{SortKey, SortOrder};
use crate::tree;
use crate::watch::{ChangeKind, FolderWatcher};

/// 画像のあるフォルダ 1 つと、その中の画像（並べた順）。
#[derive(Debug, Clone, PartialEq)]
pub struct Folder {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
}

/// フォルダとファイルの並べ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub key: SortKey,
    pub folders_descending: bool,
    pub files_descending: bool,
}

impl Order {
    pub fn of(settings: &Fullscreen) -> Self {
        Self {
            key: settings.sort,
            folders_descending: settings.folders_descending,
            files_descending: settings.files_descending,
        }
    }
}

/// `root` 以下の画像のあるフォルダを、見せる順に `found` へ渡す。親のフォルダの画像を、
/// その中のフォルダより先にする。ZIP / 7z の中には入らない（パスワードを尋ねて止まらないよう）。
/// `cancel` が立ったらそこでやめる。
pub fn walk(
    root: &Path,
    registry: &Registry,
    order: Order,
    cancel: &AtomicBool,
    found: &mut dyn FnMut(Folder),
) {
    let mut visited = HashSet::new();
    walk_dir(root, registry, order, cancel, &mut visited, found);
}

fn walk_dir(
    dir: &Path,
    registry: &Registry,
    order: Order,
    cancel: &AtomicBool,
    visited: &mut HashSet<PathBuf>,
    found: &mut dyn FnMut(Folder),
) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    // シンボリックリンクで親を指すフォルダを、いつまでも回り続けないため。
    let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(real) {
        return;
    }
    let folder_order = SortOrder {
        key: order.key,
        descending: order.folders_descending,
    };
    let listing = tree::list_folder(dir, registry, folder_order);
    let mut files = listing.images;
    // 並べ方は名前が同じでも順が決まる（全順序）ので、逆にすれば逆の向きに並べたのと同じになる。
    if order.files_descending != order.folders_descending {
        files.reverse();
    }
    if !files.is_empty() {
        found(Folder {
            dir: dir.to_path_buf(),
            files,
        });
    }
    for sub in listing.folders {
        walk_dir(&sub, registry, order, cancel, visited, found);
    }
}

/// 見せている画像の場所（何番目のフォルダの何番目のファイルか）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub folder: usize,
    pub file: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Next,
    Previous,
    NextFolder,
    PreviousFolder,
}

/// `lens`（フォルダごとの画像の数）の並びで、`pos` から `mv` だけ動いた先。
/// フォルダの端では隣のフォルダへ続く。全体の端では、`wrap` なら反対の端へ回り、
/// そうでなければ `None`（動かない）。
pub fn step(lens: &[usize], pos: Pos, mv: Move, wrap: bool) -> Option<Pos> {
    let folders = lens.len();
    if folders == 0 {
        return None;
    }
    let last = |folder: usize| Pos {
        folder,
        file: lens[folder].saturating_sub(1),
    };
    let first = |folder: usize| Pos { folder, file: 0 };
    match mv {
        Move::Next if pos.file + 1 < lens[pos.folder] => Some(Pos {
            file: pos.file + 1,
            ..pos
        }),
        Move::Next | Move::NextFolder if pos.folder + 1 < folders => Some(first(pos.folder + 1)),
        Move::Next | Move::NextFolder => wrap.then(|| first(0)),
        Move::Previous if pos.file > 0 => Some(Pos {
            file: pos.file - 1,
            ..pos
        }),
        Move::Previous if pos.folder > 0 => Some(last(pos.folder - 1)),
        Move::Previous => wrap.then(|| last(folders - 1)),
        Move::PreviousFolder if pos.folder > 0 => Some(first(pos.folder - 1)),
        Move::PreviousFolder => wrap.then(|| first(folders - 1)),
    }
}

/// 右下に出す、起点からの場所。起点のフォルダ名から始める（起点そのものの画像も分かるよう）。
pub fn relative_label(root: &Path, file: &Path) -> String {
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    match file.strip_prefix(root) {
        Ok(rel) => Path::new(&name).join(rel).display().to_string(),
        Err(_) => file.display().to_string(),
    }
}

/// 最新を追うときに、届いた変化のうち見せる対象になるファイルか。隠しフォルダ（ゴミ箱を含む）の
/// 中と、ZIP / 7z は除く。
pub fn is_candidate(root: &Path, path: &Path, registry: &Registry) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let hidden = rel
        .components()
        .any(|c| tree::is_hidden_name(&c.as_os_str().to_string_lossy()));
    !hidden && !crate::archive::is_archive(path) && path.is_file() && registry.supports_file(path)
}

/// 新しいファイル `file` を、フォルダの並び `folders` に入れて、その場所を返す。
/// 知っているフォルダならその中で並べ直し（並べ方は [`Order`]）、知らないフォルダなら最後に足す。
/// 新しく来るフォルダはたいてい撮影中の最新のフォルダで、途中に差し込む順を決めるには
/// フォルダの更新日時を取り直す必要があるため。
pub fn insert_file(folders: &mut Vec<Folder>, file: &Path, order: Order) -> Pos {
    let dir = file.parent().unwrap_or(Path::new("")).to_path_buf();
    let Some(folder) = folders.iter().position(|f| f.dir == dir) else {
        folders.push(Folder {
            dir,
            files: vec![file.to_path_buf()],
        });
        return Pos {
            folder: folders.len() - 1,
            file: 0,
        };
    };
    let files = &mut folders[folder].files;
    if !files.iter().any(|f| f == file) {
        files.push(file.to_path_buf());
        sort_files(files, order);
    }
    let file = files.iter().position(|f| f == file).unwrap_or(0);
    Pos { folder, file }
}

fn sort_files(files: &mut [PathBuf], order: Order) {
    let sort_order = SortOrder {
        key: order.key,
        descending: order.files_descending,
    };
    let names: Vec<String> = files
        .iter()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    let modified: Vec<Option<SystemTime>> = files
        .iter()
        .map(|p| {
            (order.key == SortKey::Modified)
                .then(|| std::fs::metadata(p).ok()?.modified().ok())
                .flatten()
        })
        .collect();
    let mut idx: Vec<usize> = (0..files.len()).collect();
    idx.sort_by(|&a, &b| {
        let item = |i: usize| crate::sort::SortItem {
            name: &names[i],
            is_dir: false,
            modified: modified[i],
        };
        crate::sort::compare(sort_order, &item(a), &item(b))
    });
    let sorted: Vec<PathBuf> = idx.iter().map(|&i| files[i].clone()).collect();
    files.clone_from_slice(&sorted);
}

/// 消えたファイル `file` を並びから除き、見せていた場所 `pos` を直して返す。
/// 見せていたファイルが消えたら、その次（フォルダの最後なら新しい最後）を見せる。
pub fn remove_file(folders: &mut Vec<Folder>, file: &Path, pos: Option<Pos>) -> Option<Pos> {
    let Some(fi) = folders
        .iter()
        .position(|f| f.files.iter().any(|p| p == file))
    else {
        return pos;
    };
    let index = folders[fi].files.iter().position(|p| p == file)?;
    folders[fi].files.remove(index);
    let emptied = folders[fi].files.is_empty();
    if emptied {
        folders.remove(fi);
    }
    let pos = pos?;
    if folders.is_empty() {
        return None;
    }
    let fixed = match pos.folder.cmp(&fi) {
        std::cmp::Ordering::Less => pos,
        std::cmp::Ordering::Greater if emptied => Pos {
            folder: pos.folder - 1,
            ..pos
        },
        std::cmp::Ordering::Greater => pos,
        std::cmp::Ordering::Equal if emptied => Pos {
            folder: fi.min(folders.len() - 1),
            file: 0,
        },
        std::cmp::Ordering::Equal => Pos {
            file: if pos.file > index {
                pos.file - 1
            } else {
                pos.file.min(folders[fi].files.len() - 1)
            },
            ..pos
        },
    };
    Some(fixed)
}

/// 裏で探したフォルダの知らせ。
enum Scan {
    Found {
        folder: Folder,
        /// 最新を追うときだけ: このフォルダで最も新しいファイル。
        newest: Option<(SystemTime, PathBuf)>,
    },
    Done,
}

fn newest_of(files: &[PathBuf]) -> Option<(SystemTime, PathBuf)> {
    files
        .iter()
        .filter_map(|p| Some((std::fs::metadata(p).ok()?.modified().ok()?, p.clone())))
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
}

/// 色導出を描いている 1 枚。
struct Rendering {
    rx: Receiver<Option<egui::ColorImage>>,
    loaded: Box<crate::loader::Loaded>,
}

/// 読み込んだ 1 枚（ファイルかフレーム）。
struct Picture {
    texture: TextureHandle,
    info: ImageInfo,
    /// 8bit より深い画像の元と色の表。深く描く（[`crate::deep`]）ときに使う。
    deep_source: Option<(Arc<vw_core::VwImage>, Option<Arc<vw_core::Palette>>)>,
    deep: Option<crate::deep::Layer>,
}

/// 画面に合わせて縮めてあるので、拡大するときもなめらかにする（スロットのように
/// 画素を 1 つずつ見る使い方ではないため）。
const PICTURE_OPTIONS: TextureOptions = TextureOptions {
    mipmap_mode: Some(egui::TextureFilter::Linear),
    ..TextureOptions::LINEAR
};

/// `picture` を深く描くなら、描き方を用意して返す。描き終わるまでは `None`（標準の表示で出す）。
fn deep_picture<'p>(
    ctx: &egui::Context,
    picture: &'p mut Picture,
    mode: Option<crate::deep::Mode>,
    surface: crate::deep::Surface,
    screen_side: u32,
) -> Option<&'p crate::deep::Layer> {
    let (Some(mode), Some((image, palette))) = (mode, &picture.deep_source) else {
        picture.deep = None;
        return None;
    };
    let max_side = crate::loader::cover_side(image.width.max(image.height), screen_side.max(1));
    if !picture
        .deep
        .as_ref()
        .is_some_and(|d| d.is_for(image, palette.as_ref(), max_side, mode))
    {
        picture.deep = Some(crate::deep::Layer::image(
            image,
            palette.as_ref(),
            max_side,
            mode,
            surface,
        ));
    }
    let layer = picture.deep.as_mut()?;
    if layer.poll(ctx, PICTURE_OPTIONS) {
        Some(layer)
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        None
    }
}

/// 書き込み途中で読めなかった最新のファイルを読み直す間隔と回数。
const RETRY_AFTER: Duration = Duration::from_millis(500);
const RETRIES: u32 = 10;
/// マウスを動かさないでいると、カーソルを隠すまでの時間。
const HIDE_CURSOR_AFTER: Duration = Duration::from_secs(2);
/// 先読みする前後の画像の数。
const AHEAD: usize = 2;
const BEHIND: usize = 1;
/// アニメーションで先読みするフレームの数。
const FRAMES_AHEAD: usize = 4;

/// 全画面スライドショーの最中の状態。
pub struct Show {
    /// 灰色の画像に当てる色の表（設定の種類ごとの既定）。
    pub look: Option<crate::colormap::Look>,
    /// 全画面に入ったときのスロットの色導出の値。どの画像にも同じ値を当てる。
    pub derive: Option<crate::pipelines::Values>,
    /// 色導出を描いている画像。描き終わるまでは前の画像を出し続ける。
    rendering: HashMap<PathBuf, Rendering>,
    root: PathBuf,
    settings: Fullscreen,
    order: Order,
    folders: Vec<Folder>,
    scan: Receiver<Scan>,
    scanning: bool,
    cancel: Arc<AtomicBool>,
    pos: Option<Pos>,
    /// アニメーションやマルチフレームのファイルで見せているフレーム。
    frame: usize,
    loader_slot: u64,
    pictures: HashMap<PathBuf, Picture>,
    /// 読み込み中の要求番号と、読んでいるパス。
    pending: HashMap<u64, PathBuf>,
    /// 読めなかったパスと、読み直した回数・時刻。
    failed: HashMap<PathBuf, (u32, Instant, String)>,
    /// 最後に画面に出したパス。次の画像が読み終わるまでこれを出し続け、黒い画面を挟まない。
    shown: Option<PathBuf>,
    /// 次のコマ（次の画像かフレーム）に進む時刻。
    due: Option<Instant>,
    paused: bool,
    watcher: Option<FolderWatcher>,
    newest: Option<(SystemTime, PathBuf)>,
    pointer_moved: Instant,
    /// ホイールの回転量のうち、まだ 1 枚分に満たない端数。
    wheel: f32,
    /// 画面の長辺（物理ピクセル）。この大きさに合わせて縮めて読む。
    screen_side: u32,
    /// 全画面のモニタでの深い画像の描き方。アプリが毎フレーム決める。
    pub deep_mode: Option<crate::deep::Mode>,
    pub surface: crate::deep::Surface,
}

/// [`Show::ui`] が呼び出し側に頼むこと。
pub enum Request {
    Exit,
}

impl Show {
    pub fn start(
        root: PathBuf,
        settings: &Fullscreen,
        registry: &Registry,
        loader_slot: u64,
        ctx: &egui::Context,
    ) -> (Self, Vec<String>) {
        let order = Order::of(settings);
        let newest_mode = settings.mode == FullscreenMode::Newest;
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let (root, registry, cancel, ctx) =
                (root.clone(), registry.clone(), cancel.clone(), ctx.clone());
            std::thread::spawn(move || {
                walk(&root, &registry, order, &cancel, &mut |folder| {
                    let newest = newest_mode.then(|| newest_of(&folder.files)).flatten();
                    let _ = tx.send(Scan::Found { folder, newest });
                    ctx.request_repaint();
                });
                let _ = tx.send(Scan::Done);
                ctx.request_repaint();
            });
        }
        let mut problems = Vec::new();
        let watcher = if newest_mode {
            match FolderWatcher::new(ctx.clone()) {
                Ok(mut w) => {
                    if let Err(e) = w.watch_recursive(&root) {
                        problems.push(e);
                    }
                    Some(w)
                }
                Err(e) => {
                    problems.push(format!("Fullscreen: cannot watch {}: {e}", root.display()));
                    None
                }
            }
        } else {
            None
        };
        let show = Self {
            look: None,
            derive: None,
            rendering: HashMap::new(),
            root,
            settings: settings.clone(),
            order,
            folders: Vec::new(),
            scan: rx,
            scanning: true,
            cancel,
            pos: None,
            frame: 0,
            loader_slot,
            pictures: HashMap::new(),
            pending: HashMap::new(),
            failed: HashMap::new(),
            shown: None,
            due: None,
            paused: false,
            watcher,
            newest: None,
            pointer_moved: Instant::now(),
            wheel: 0.0,
            screen_side: 0,
            deep_mode: None,
            surface: crate::deep::Surface::default(),
        };
        (show, problems)
    }

    pub fn loader_slot(&self) -> u64 {
        self.loader_slot
    }

    /// 読み込みの結果を受け取る。ログに出す誤りがあれば返す。
    pub fn receive(
        &mut self,
        ctx: &egui::Context,
        request: u64,
        path: PathBuf,
        result: Result<crate::loader::Loaded, String>,
    ) -> Option<String> {
        self.pending.remove(&request)?;
        match result {
            Ok(loaded) => {
                if let (Some(values), Some(full)) = (&self.derive, &loaded.full) {
                    let (image, values) = (full.image.clone(), values.clone());
                    let palette = loaded.info.look.as_ref().and_then(|a| a.palette.clone());
                    let (dicom, side) = (
                        loaded.info.plugin == "DICOM",
                        full.display.size[0].max(full.display.size[1]) as u32,
                    );
                    let p = path.clone();
                    let rx = crate::background::compute(ctx, move || {
                        let derived =
                            crate::derive::render_once(&p, &image, dicom, &values, palette)?;
                        let d = vw_core::display::DisplayImage {
                            width: derived.size[0] as u32,
                            height: derived.size[1] as u32,
                            rgba: derived.as_raw().to_vec(),
                        };
                        let d = vw_core::display::shrink(&d, side);
                        Some(egui::ColorImage::from_rgba_unmultiplied(
                            [d.width as usize, d.height as usize],
                            &d.rgba,
                        ))
                    });
                    self.rendering.insert(
                        path,
                        Rendering {
                            rx,
                            loaded: Box::new(loaded),
                        },
                    );
                    return None;
                }
                self.attach(ctx, path, loaded, None);
                None
            }
            Err(e) => {
                let entry =
                    self.failed
                        .entry(path.clone())
                        .or_insert((0, Instant::now(), String::new()));
                entry.0 += 1;
                entry.1 = Instant::now();
                entry.2.clone_from(&e);
                // 読み直すたびに同じ誤りを並べないよう、最初の 1 回だけログに出す。
                (entry.0 == 1).then(|| format!("{}: {e}", path.display()))
            }
        }
    }

    /// 描き終わった色導出を受け取る。当てられなかった画像は標準の表示で出す。
    fn poll_rendering(&mut self, ctx: &egui::Context) {
        let done: Vec<(PathBuf, Option<egui::ColorImage>)> = self
            .rendering
            .iter()
            .filter_map(|(p, r)| r.rx.try_recv().ok().map(|img| (p.clone(), img)))
            .collect();
        for (path, image) in done {
            if let Some(r) = self.rendering.remove(&path) {
                self.attach(ctx, path, *r.loaded, image);
            }
        }
    }

    /// 読んだ画像を出せるようにする。`derived` があれば標準の表示の代わりにそれを出す。
    fn attach(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        loaded: crate::loader::Loaded,
        derived: Option<egui::ColorImage>,
    ) {
        // 色導出をした画像は深く描かない（色導出は 8bit で描く）。
        let deep_source = loaded
            .full
            .as_ref()
            .filter(|_| derived.is_none() && crate::deep::is_deep(loaded.info.bits_per_sample))
            .map(|f| {
                let palette = loaded.info.look.as_ref().and_then(|a| a.palette.clone());
                (Arc::clone(&f.image), palette)
            });
        let Some(image) = derived.or(loaded.full.map(|f| f.display)).or(loaded.thumb) else {
            return;
        };
        let texture = ctx.load_texture(
            format!("{}#fullscreen", path.display()),
            image,
            PICTURE_OPTIONS,
        );
        self.failed.remove(&path);
        self.pictures.insert(
            path,
            Picture {
                texture,
                info: loaded.info,
                deep_source,
                deep: None,
            },
        );
    }

    fn lens(&self) -> Vec<usize> {
        self.folders.iter().map(|f| f.files.len()).collect()
    }

    fn file_at(&self, pos: Pos) -> Option<&Path> {
        self.folders
            .get(pos.folder)?
            .files
            .get(pos.file)
            .map(PathBuf::as_path)
    }

    fn current_file(&self) -> Option<PathBuf> {
        self.file_at(self.pos?).map(Path::to_path_buf)
    }

    /// 今見せたいパス（フレームを見せているならフレームのパス）。
    fn wanted(&self) -> Option<PathBuf> {
        let file = self.current_file()?;
        Some(if self.frame == 0 {
            file
        } else {
            frames::frame_path(&file, self.frame)
        })
    }

    fn go(&mut self, pos: Pos) {
        self.pos = Some(pos);
        self.frame = 0;
        self.due = None;
    }

    fn move_by(&mut self, mv: Move) {
        let Some(pos) = self.pos else {
            return;
        };
        if let Some(next) = step(&self.lens(), pos, mv, self.settings.loop_at_end) {
            self.go(next);
        }
    }

    /// 自動で送るか。最新を追うときは、新しいファイルが来るまで今の画像を見せ続ける。
    fn auto_advance(&self) -> bool {
        self.settings.mode == FullscreenMode::Sequence
            && self.settings.interval_secs > 0.0
            && !self.paused
    }

    fn poll_scan(&mut self) {
        while let Ok(msg) = self.scan.try_recv() {
            match msg {
                Scan::Found { folder, newest } => {
                    self.folders.push(folder);
                    if let Some(candidate) = newest
                        && self.newest.as_ref().is_none_or(|n| candidate.0 > n.0)
                    {
                        self.newest = Some(candidate);
                        self.show_newest();
                    }
                    if self.pos.is_none() && self.settings.mode == FullscreenMode::Sequence {
                        self.go(Pos { folder: 0, file: 0 });
                    }
                }
                Scan::Done => self.scanning = false,
            }
        }
    }

    fn show_newest(&mut self) {
        let Some((_, path)) = &self.newest else {
            return;
        };
        let path = path.clone();
        let found = self.folders.iter().enumerate().find_map(|(fi, f)| {
            let file = f.files.iter().position(|p| *p == path)?;
            Some(Pos { folder: fi, file })
        });
        if let Some(pos) = found {
            self.go(pos);
        }
    }

    /// 最新を追うときの、起点以下の変化。新しいファイルが来たらすぐそれを見せる。
    fn poll_watch(&mut self, registry: &Registry) {
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        let changes = watcher.poll(Instant::now());
        for change in changes {
            match change.kind {
                ChangeKind::Removed => {
                    self.pos = remove_file(&mut self.folders, &change.path, self.pos);
                    self.forget(&change.path);
                }
                ChangeKind::Created | ChangeKind::Modified => {
                    if !is_candidate(&self.root, &change.path, registry) {
                        continue;
                    }
                    // 上書きされたファイルは、前の中身のテクスチャを捨てて読み直す。
                    self.forget(&change.path);
                    let pos = insert_file(&mut self.folders, &change.path, self.order);
                    self.newest = Some((SystemTime::now(), change.path));
                    self.go(pos);
                }
            }
        }
    }

    fn forget(&mut self, file: &Path) {
        self.pictures.retain(|p, _| frames::file_of(p) != file);
        self.failed.retain(|p, _| frames::file_of(p) != file);
        if self
            .shown
            .as_ref()
            .is_some_and(|p| frames::file_of(p) == file)
        {
            self.shown = None;
        }
    }

    /// 見せたい画像と、その前後の先読みする画像のパス。先頭が見せたい画像。
    fn keep_list(&self) -> Vec<PathBuf> {
        let Some(pos) = self.pos else {
            return Vec::new();
        };
        let mut keep: Vec<PathBuf> = self.wanted().into_iter().collect();
        let file = self.current_file();
        if let (Some(file), Some(picture)) =
            (&file, file.as_ref().and_then(|f| self.pictures.get(f)))
            && self.plays_frames(picture)
        {
            let frames = picture.info.frames;
            for k in 1..=FRAMES_AHEAD {
                let i = (self.frame + k) % frames;
                keep.push(if i == 0 {
                    file.clone()
                } else {
                    frames::frame_path(file, i)
                });
            }
        }
        let lens = self.lens();
        let wrap = self.settings.loop_at_end;
        let mut ahead = pos;
        for _ in 0..AHEAD {
            let Some(next) = step(&lens, ahead, Move::Next, wrap) else {
                break;
            };
            ahead = next;
            keep.extend(self.file_at(next).map(Path::to_path_buf));
        }
        let mut behind = pos;
        for _ in 0..BEHIND {
            let Some(previous) = step(&lens, behind, Move::Previous, wrap) else {
                break;
            };
            behind = previous;
            keep.extend(self.file_at(previous).map(Path::to_path_buf));
        }
        if let Some(file) = file {
            keep.push(file);
        }
        keep.dedup();
        keep
    }

    /// フレームを順に見せるか。手で送るときは先頭のフレームだけを見せる。
    fn plays_frames(&self, picture: &Picture) -> bool {
        picture.info.frames > 1 && self.auto_advance()
    }

    /// 要る画像を読み込みに出し、要らなくなった画像を捨てる。
    fn manage_loads(&mut self, loader: &Loader, next_id: &mut u64) {
        let keep = self.keep_list();
        let keep_set: HashSet<&PathBuf> = keep.iter().collect();
        let shown = self.shown.clone();
        self.pictures
            .retain(|p, _| keep_set.contains(p) || shown.as_ref() == Some(p));
        let stale: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| !keep_set.contains(p))
            .map(|(r, _)| *r)
            .collect();
        for request in stale {
            loader.cancel(request);
            self.pending.remove(&request);
        }
        let now = Instant::now();
        let newest_mode = self.settings.mode == FullscreenMode::Newest;
        for path in &keep {
            if self.pictures.contains_key(path)
                || self.rendering.contains_key(path)
                || self.pending.values().any(|p| p == path)
            {
                continue;
            }
            if let Some((count, at, _)) = self.failed.get(path) {
                // 最新を追うときは、書き込み途中で読めなかったファイルを何度か読み直す。
                let retry = newest_mode && *count < RETRIES && now - *at >= RETRY_AFTER;
                if !retry {
                    continue;
                }
            }
            *next_id += 1;
            let request = *next_id;
            loader.request(Job {
                slot: self.loader_slot,
                request,
                path: path.clone(),
                thumb: false,
                full: true,
                develop: false,
                fit_side: Some(self.screen_side.max(1)),
                look: self.look.clone(),
            });
            self.pending.insert(request, path.clone());
        }
    }

    /// 時間が来たら次のコマへ進める。
    fn advance_if_due(&mut self, now: Instant) {
        let Some(wanted) = self.wanted() else {
            return;
        };
        let Some(file) = self.current_file() else {
            return;
        };
        let shown_now = self.shown.as_ref() == Some(&wanted);
        let failed =
            self.failed.contains_key(&wanted) && self.settings.mode == FullscreenMode::Sequence;
        if !self.auto_advance() || !(shown_now || failed) {
            self.due = None;
            return;
        }
        let animation = self
            .pictures
            .get(&file)
            .filter(|p| self.plays_frames(p))
            .map(|p| {
                let delay = p.info.frame_delays.as_ref().and_then(|d| d.get(self.frame));
                (
                    p.info.frames,
                    delay.copied(),
                    crate::app::frame_rate(&p.info.metadata),
                )
            });
        let interval = Duration::from_secs_f32(self.settings.interval_secs);
        let wait = match animation {
            Some((_, Some(ms), _)) => Duration::from_millis(ms.into()),
            Some((_, None, rate)) => Duration::from_secs_f32(1.0 / rate.unwrap_or(10.0).max(0.1)),
            None => interval,
        };
        let due = *self.due.get_or_insert(now + wait);
        if now < due {
            return;
        }
        match animation {
            Some((frames, ..)) if self.frame + 1 < frames => {
                self.frame += 1;
                self.due = None;
            }
            _ => {
                let before = self.pos;
                self.move_by(Move::Next);
                if self.pos == before {
                    // 最後まで来て止まった。
                    self.paused = true;
                    self.due = None;
                }
            }
        }
    }

    /// 1 フレーム分。全画面に画像とパスを描き、キー操作を受け付ける。
    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        keys: &Keymap,
        loader: &Loader,
        next_id: &mut u64,
        lang: Language,
    ) -> Vec<Request> {
        let ctx = ui.ctx().clone();
        let mut out = Vec::new();
        self.poll_rendering(&ctx);
        self.poll_scan();
        self.poll_watch(loader.registry());

        let events = ctx.input(|i| i.events.clone());
        for action in keys.actions_in(Scope::Fullscreen, &events) {
            match action {
                Action::FullscreenExit => out.push(Request::Exit),
                Action::FullscreenNext => self.move_by(Move::Next),
                Action::FullscreenPrevious => self.move_by(Move::Previous),
                Action::FullscreenNextFolder => self.move_by(Move::NextFolder),
                Action::FullscreenPreviousFolder => self.move_by(Move::PreviousFolder),
                Action::FullscreenPause => {
                    self.paused ^= true;
                    self.due = None;
                }
                _ => {}
            }
        }
        let (wheel, moved) = ctx.input(|i| {
            let wheel = crate::viewer::wheel_delta(&i.events, i.viewport_rect().height());
            (wheel, i.pointer.delta() != egui::Vec2::ZERO)
        });
        let steps = crate::viewer::wheel_steps(&mut self.wheel, wheel);
        for _ in 0..steps.unsigned_abs() {
            self.move_by(if steps > 0 {
                Move::Next
            } else {
                Move::Previous
            });
        }
        let now = Instant::now();
        if moved {
            self.pointer_moved = now;
        }

        let rect = ui.max_rect();
        let ppp = ctx.pixels_per_point();
        self.screen_side = (rect.width().max(rect.height()) * ppp).round() as u32;
        self.manage_loads(loader, next_id);
        if let Some(wanted) = self.wanted()
            && self.pictures.contains_key(&wanted)
        {
            self.shown = Some(wanted);
        }
        let before = (self.pos, self.frame);
        self.advance_if_due(now);
        if (self.pos, self.frame) != before {
            // 進めた先がもう読んであれば、読み込みの知らせは来ないので自分で描き直す。
            ctx.request_repaint();
        }
        // 進めた先を、このフレームのうちに読みに出す。
        self.manage_loads(loader, next_id);

        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, Color32::BLACK);
        let text_color = self.settings.path_text_color();
        // 最新を追うときは、書き込み途中で読めなかった間も前の画像を出し続け、読み直しをあきらめた
        // ときだけ誤りを出す。順に見せるときは、読めない画像の代わりに前の画像を出すと取り違えるので
        // すぐ誤りを出す。
        let gave_up = self.wanted().and_then(|w| {
            let (count, _, e) = self.failed.get(&w)?;
            let newest = self.settings.mode == FullscreenMode::Newest;
            (!newest || *count >= RETRIES).then(|| e.clone())
        });
        let (deep_mode, surface, screen_side) = (self.deep_mode, self.surface, self.screen_side);
        let picture = self
            .shown
            .as_ref()
            .filter(|_| gave_up.is_none())
            .and_then(|p| self.pictures.get_mut(p));
        if let Some(picture) = picture {
            let size = picture.texture.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let image_rect = Rect::from_center_size(rect.center(), size * scale);
            let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            let deep = deep_picture(&ctx, picture, deep_mode, surface, screen_side)
                .is_some_and(|layer| layer.paint(painter, image_rect, whole, PICTURE_OPTIONS));
            if !deep {
                painter.image(picture.texture.id(), image_rect, whole, Color32::WHITE);
            }
        }
        let center_text = if self.pos.is_none() {
            Some(if self.scanning {
                tr(lang, "Looking for images...").to_owned()
            } else if self.settings.mode == FullscreenMode::Newest {
                tr(lang, "Waiting for a new file...").to_owned()
            } else {
                tr1(lang, "No images under {}", &self.root.display().to_string())
            })
        } else {
            gave_up.map(|e| tr1(lang, "Cannot open this image: {}", &e))
        };
        let font = egui::FontId::proportional(self.settings.path_size);
        if let Some(text) = center_text {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                font.clone(),
                text_color,
            );
        }
        if self.settings.show_path
            && let (Some(pos), Some(file)) = (self.pos, self.current_file())
        {
            let mut label = relative_label(&self.root, &file);
            let count = self.folders[pos.folder].files.len();
            label.push_str(&format!("    {} / {count}", pos.file + 1));
            if self.frame > 0 {
                label.push_str(&format!("  [{}]", self.frame + 1));
            }
            if self.scanning {
                label.push_str("  …");
            }
            let margin = 12.0;
            painter.text(
                rect.right_bottom() - egui::vec2(margin, margin),
                egui::Align2::RIGHT_BOTTOM,
                label,
                font.clone(),
                text_color,
            );
        }
        if self.paused && self.auto_advance_configured() {
            painter.text(
                rect.right_top() + egui::vec2(-12.0, 12.0),
                egui::Align2::RIGHT_TOP,
                tr(lang, "Paused"),
                font,
                text_color,
            );
        }
        if now - self.pointer_moved >= HIDE_CURSOR_AFTER {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        } else {
            ctx.request_repaint_after(HIDE_CURSOR_AFTER);
        }
        ui.allocate_rect(rect, egui::Sense::hover());

        if let Some(due) = self.due {
            ctx.request_repaint_after(due.saturating_duration_since(now));
        }
        if self.watcher.as_ref().is_some_and(FolderWatcher::waiting) {
            ctx.request_repaint_after(crate::watch::QUIET);
        }
        if !self.failed.is_empty() && self.settings.mode == FullscreenMode::Newest {
            ctx.request_repaint_after(RETRY_AFTER);
        }
        out
    }

    fn auto_advance_configured(&self) -> bool {
        self.settings.mode == FullscreenMode::Sequence && self.settings.interval_secs > 0.0
    }

    /// 止めるときに、まだ始まっていない読み込みを取り消す。
    pub fn stop(&mut self, loader: &Loader) {
        self.cancel.store(true, Ordering::Relaxed);
        for request in self.pending.keys() {
            loader.cancel(*request);
        }
        self.pending.clear();
    }
}

impl Drop for Show {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// 全画面スライドショーを始める前の画面（パラメータを入れる）。メインの領域を置き換える。
#[derive(Default)]
pub struct Setup {
    pub root: String,
}

/// 準備の画面で押されたもの。
pub enum SetupAction {
    Close,
    Start(PathBuf),
    /// 設定の値が変わった（保存する）。
    Changed,
}

impl Setup {
    /// 画面を開いたときの起点。前回の起点がまだあればそれ、なければ今いるフォルダ。
    pub fn open(&mut self, last_root: Option<&Path>, current: Option<&Path>) {
        let root = last_root.filter(|p| p.is_dir()).or(current);
        self.root = root.map(|p| p.display().to_string()).unwrap_or_default();
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut crate::settings::Settings,
        current: Option<&Path>,
    ) -> Option<SetupAction> {
        use egui_phosphor::regular as icon;
        let lang = settings.appearance.language;
        let mut action = None;
        let root = PathBuf::from(self.root.trim());
        let valid = !self.root.trim().is_empty() && root.is_dir();
        egui::Panel::top("fullscreen_header").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} {}", icon::ARROW_LEFT, tr(lang, "Back")))
                    .clicked()
                {
                    action = Some(SetupAction::Close);
                }
                ui.heading(tr(lang, "Slideshow setup"));
                // 項目が多く下まで流れるので、開始のボタンは見出しの横に置いていつでも押せるようにする。
                ui.add_space(16.0);
                let start = ui.add_enabled(
                    valid,
                    egui::Button::new(
                        egui::RichText::new(format!("{} {}", icon::PLAY, tr(lang, "Start")))
                            .size(15.0),
                    )
                    .min_size(egui::vec2(110.0, 26.0)),
                );
                if start.clicked() {
                    action = Some(SetupAction::Start(root.clone()));
                }
            });
            ui.add_space(2.0);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.set_max_width(760.0);
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(tr(lang, "Folder")).strong());
                    ui.weak(tr(
                        lang,
                        "Images in this folder and all folders below it are shown. Folders without images are skipped.",
                    ));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.root)
                                .desired_width((ui.available_width() - 160.0).max(200.0)),
                        );
                        if ui
                            .add_enabled(
                                current.is_some(),
                                egui::Button::new(tr(lang, "Current folder")),
                            )
                            .clicked()
                            && let Some(c) = current {
                                self.root = c.display().to_string();
                            }
                    });
                    if !valid && !self.root.trim().is_empty() {
                        ui.colored_label(ui.visuals().error_fg_color, tr(lang, "Folder not found"));
                    }
                    ui.add_space(8.0);
                    if crate::settings_ui::fullscreen_items_ui(ui, settings) {
                        action = Some(SetupAction::Changed);
                    }
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(tr(lang, "Keys")).strong());
                    egui::Grid::new("fullscreen_keys")
                        .num_columns(2)
                        .spacing([24.0, 2.0])
                        .show(ui, |ui| {
                            for a in Action::ALL.iter().copied().filter(|a| a.scope() == Scope::Fullscreen) {
                                ui.label(tr(lang, a.title()));
                                let keys: Vec<String> =
                                    settings.keys.keys(a).iter().map(ToString::to_string).collect();
                                ui.weak(keys.join(", "));
                                ui.end_row();
                            }
                            ui.label(tr(lang, "Mouse wheel"));
                            ui.weak(tr(lang, "Previous / next image"));
                            ui.end_row();
                        });
                    ui.weak(tr(lang, "Keys can be changed in Settings."));
                });
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{TempDir, png_registry, write_png};

    const POS: fn(usize, usize) -> Pos = |folder, file| Pos { folder, file };

    #[test]
    fn next_moves_through_a_folder_then_to_the_next_folder() {
        let lens = [2, 3];
        assert_eq!(step(&lens, POS(0, 0), Move::Next, false), Some(POS(0, 1)));
        assert_eq!(step(&lens, POS(0, 1), Move::Next, false), Some(POS(1, 0)));
    }

    #[test]
    fn previous_at_the_start_of_a_folder_goes_to_the_end_of_the_previous_folder() {
        let lens = [2, 3];
        assert_eq!(
            step(&lens, POS(1, 0), Move::Previous, false),
            Some(POS(0, 1))
        );
        assert_eq!(
            step(&lens, POS(1, 2), Move::Previous, false),
            Some(POS(1, 1))
        );
    }

    #[test]
    fn folder_moves_go_to_the_first_image_of_the_neighbor_folder() {
        let lens = [2, 3, 1];
        assert_eq!(
            step(&lens, POS(1, 2), Move::NextFolder, false),
            Some(POS(2, 0))
        );
        assert_eq!(
            step(&lens, POS(1, 2), Move::PreviousFolder, false),
            Some(POS(0, 0))
        );
    }

    #[test]
    fn ends_stop_unless_wrapping() {
        let lens = [2, 3];
        assert_eq!(step(&lens, POS(1, 2), Move::Next, false), None);
        assert_eq!(step(&lens, POS(0, 0), Move::Previous, false), None);
        assert_eq!(step(&lens, POS(1, 0), Move::NextFolder, false), None);
        assert_eq!(step(&lens, POS(0, 1), Move::PreviousFolder, false), None);
        assert_eq!(step(&lens, POS(1, 2), Move::Next, true), Some(POS(0, 0)));
        assert_eq!(
            step(&lens, POS(0, 0), Move::Previous, true),
            Some(POS(1, 2))
        );
        assert_eq!(
            step(&lens, POS(1, 1), Move::NextFolder, true),
            Some(POS(0, 0))
        );
        assert_eq!(
            step(&lens, POS(0, 1), Move::PreviousFolder, true),
            Some(POS(1, 0))
        );
    }

    #[test]
    fn nothing_to_step_through_when_no_folders() {
        assert_eq!(step(&[], POS(0, 0), Move::Next, true), None);
    }

    fn png(dir: &TempDir, rel: &str) -> PathBuf {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_png(&path, 1, 1, &[1, 2, 3]);
        path
    }

    fn walked(root: &Path, order: Order) -> Vec<Folder> {
        let mut out = Vec::new();
        walk(
            root,
            &png_registry(),
            order,
            &AtomicBool::new(false),
            &mut |f| out.push(f),
        );
        out
    }

    fn by_name(folders_descending: bool, files_descending: bool) -> Order {
        Order {
            key: SortKey::NameNatural,
            folders_descending,
            files_descending,
        }
    }

    #[test]
    fn walk_lists_only_folders_with_images_parent_first() {
        let dir = TempDir::new();
        let top = png(&dir, "top.png");
        let a1 = png(&dir, "a/1.png");
        dir.mkdir("empty");
        dir.mkdir("only_folders/deeper");
        let deep = png(&dir, "only_folders/deeper/x.png");
        let folders = walked(dir.path(), by_name(false, false));
        let dirs: Vec<&Path> = folders.iter().map(|f| f.dir.as_path()).collect();
        assert_eq!(
            dirs,
            [
                dir.path(),
                dir.path().join("a").as_path(),
                dir.path().join("only_folders/deeper").as_path(),
            ]
        );
        assert_eq!(folders[0].files, [top]);
        assert_eq!(folders[1].files, [a1]);
        assert_eq!(folders[2].files, [deep]);
    }

    #[test]
    fn folder_and_file_orders_are_independent() {
        let dir = TempDir::new();
        let a = [
            png(&dir, "a/1.png"),
            png(&dir, "a/2.png"),
            png(&dir, "a/10.png"),
        ];
        png(&dir, "b/1.png");
        let folders = walked(dir.path(), by_name(true, false));
        assert_eq!(folders[0].dir, dir.path().join("b"));
        assert_eq!(folders[1].files, a);
        let folders = walked(dir.path(), by_name(false, true));
        assert_eq!(folders[0].dir, dir.path().join("a"));
        assert_eq!(folders[0].files, [a[2].clone(), a[1].clone(), a[0].clone()]);
    }

    #[test]
    fn walk_skips_hidden_folders_and_archives() {
        let dir = TempDir::new();
        png(&dir, ".hidden/x.png");
        png(&dir, "$RECYCLE.BIN/x.png");
        dir.touch("pack.zip");
        let shown = png(&dir, "ok/x.png");
        let folders = walked(dir.path(), by_name(false, false));
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].files, [shown]);
    }

    #[test]
    fn cancelled_walk_finds_nothing() {
        let dir = TempDir::new();
        png(&dir, "a.png");
        let mut out = Vec::new();
        walk(
            dir.path(),
            &png_registry(),
            by_name(false, false),
            &AtomicBool::new(true),
            &mut |f| out.push(f),
        );
        assert!(out.is_empty());
    }

    #[test]
    fn label_starts_at_the_root_folder_name() {
        let root = Path::new("/data/shoot");
        let label = relative_label(root, &root.join("day1").join("a.jpg"));
        let expected = Path::new("shoot").join("day1").join("a.jpg");
        assert_eq!(label, expected.display().to_string());
    }

    fn folder(dir: &str, files: &[&str]) -> Folder {
        Folder {
            dir: PathBuf::from(dir),
            files: files.iter().map(|f| Path::new(dir).join(f)).collect(),
        }
    }

    #[test]
    fn new_file_in_a_known_folder_takes_its_sorted_place() {
        let mut folders = vec![folder("/r/a", &["1.png", "3.png"])];
        let pos = insert_file(&mut folders, Path::new("/r/a/2.png"), by_name(false, false));
        assert_eq!(pos, POS(0, 1));
        assert_eq!(folders[0].files[1], Path::new("/r/a/2.png"));
    }

    #[test]
    fn new_file_in_an_unknown_folder_adds_the_folder_at_the_end() {
        let mut folders = vec![folder("/r/a", &["1.png"])];
        let pos = insert_file(&mut folders, Path::new("/r/b/9.png"), by_name(false, false));
        assert_eq!(pos, POS(1, 0));
        assert_eq!(folders[1].dir, Path::new("/r/b"));
    }

    #[test]
    fn overwritten_file_is_not_added_twice() {
        let mut folders = vec![folder("/r/a", &["1.png", "2.png"])];
        let pos = insert_file(&mut folders, Path::new("/r/a/2.png"), by_name(false, false));
        assert_eq!(pos, POS(0, 1));
        assert_eq!(folders[0].files.len(), 2);
    }

    #[test]
    fn removing_the_shown_file_shows_the_next_one() {
        let mut folders = vec![folder("/r/a", &["1.png", "2.png", "3.png"])];
        let pos = remove_file(&mut folders, Path::new("/r/a/2.png"), Some(POS(0, 1)));
        assert_eq!(pos, Some(POS(0, 1)));
        assert_eq!(folders[0].files[1], Path::new("/r/a/3.png"));
    }

    #[test]
    fn removing_the_last_file_of_a_folder_drops_the_folder() {
        let mut folders = vec![folder("/r/a", &["1.png"]), folder("/r/b", &["1.png"])];
        let pos = remove_file(&mut folders, Path::new("/r/a/1.png"), Some(POS(1, 0)));
        assert_eq!(pos, Some(POS(0, 0)));
        assert_eq!(folders.len(), 1);
        let pos = remove_file(&mut folders, Path::new("/r/b/1.png"), pos);
        assert_eq!(pos, None);
    }

    #[test]
    fn removing_an_earlier_file_keeps_showing_the_same_file() {
        let mut folders = vec![folder("/r/a", &["1.png", "2.png", "3.png"])];
        let pos = remove_file(&mut folders, Path::new("/r/a/1.png"), Some(POS(0, 2)));
        assert_eq!(pos, Some(POS(0, 1)));
    }

    #[test]
    fn candidates_exclude_hidden_folders_and_unsupported_files() {
        let dir = TempDir::new();
        let ok = png(&dir, "a/x.png");
        let hidden = png(&dir, ".cache/x.png");
        let text = dir.touch("a/note.txt");
        let registry = png_registry();
        assert!(is_candidate(dir.path(), &ok, &registry));
        assert!(!is_candidate(dir.path(), &hidden, &registry));
        assert!(!is_candidate(dir.path(), &text, &registry));
        assert!(!is_candidate(&dir.path().join("a"), &hidden, &registry));
    }

    #[test]
    fn setup_prefers_the_last_root_while_it_exists() {
        let dir = TempDir::new();
        let last = dir.mkdir("last");
        let current = dir.mkdir("current");
        let mut setup = Setup::default();
        setup.open(Some(&last), Some(&current));
        assert_eq!(setup.root, last.display().to_string());
        setup.open(Some(&dir.path().join("gone")), Some(&current));
        assert_eq!(setup.root, current.display().to_string());
    }
}
