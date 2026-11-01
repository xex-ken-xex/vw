//! フォルダツリー。基底（`vw-tree`）に、ビューア固有の項目を足す。
//!
//! 基底が持つのは、ルート・遅延読み込み・隠しフォルダ・開閉・復元・読み直し。
//! ここで足すのは、画像ファイルの一覧（ツリーには出さないがスロットとシリーズに要る）、
//! ZIP（クリックで展開して別のルートになる）、DICOM の検査・シリーズ、右クリックメニュー
//! （コピー・ファイラ・AI タグ・データセット・送信）、フォルダの下に項目があるかの先読み。
//!
//! フォルダの中身は初めて開いたときに読む。起動時にドライブ全体を走査すると
//! 起動が遅くなるうえ、ネットワークドライブで固まることがあるため。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::SystemTime;

use eframe::egui::{self, collapsing_header::CollapsingState};
use vw_core::Registry;
use vw_tree::{Hooks, without_toggle};
pub use vw_tree::{display_name, entry_is_dir, is_hidden_name, is_recycle_bin};

use crate::app::context_menu::{self, FolderKind, MenuContext, Picked};
use crate::i18n::tr;
use crate::series::{Scan, Series, SeriesIndex, Study};
use crate::settings::Language;
use crate::sort::{self, SortItem, SortKey, SortOrder};
use crate::viewer::DraggedFile;

/// ビューアが項目に付ける印。
#[derive(Default)]
pub struct Mark {
    /// ZIP ファイル。クリックで展開して別のルートとして開く（[`crate::archive`]）。
    pub archive: bool,
    /// ZIP を展開したルート。右クリックでアンマウントできる。
    pub mounted: bool,
}

/// ツリーの 1 項目（フォルダまたはファイル）。
pub type Node = vw_tree::Node<Mark>;

/// ツリーを描くときに全項目で共通の情報と、描きながら集めるもの。基底の [`Hooks`] の実装。
struct View<'a> {
    registry: &'a Registry,
    /// cd しているフォルダ。ハイライトする。
    current: Option<&'a Path>,
    /// アクティブなスロットの画像。それを含むシリーズをハイライトする。
    active_image: Option<&'a Path>,
    lang: Language,
    order: SortOrder,
    series: &'a SeriesIndex,
    /// DICOM の拡張子。
    dicom_extensions: &'a [&'a str],
    /// まだ読んでいないフォルダの下に項目があるか（[`Peeker`]）。
    peeks: &'a HashMap<PathBuf, Peek>,
    /// 右クリックメニューを操作の表から描くのに要るもの。
    menus: &'a MenuContext<'a>,
    /// ヘッダを読み始めるフォルダと、その中の DICOM らしいファイル。
    scans: Vec<(PathBuf, Vec<PathBuf>)>,
    /// 下に項目があるか先読みしてほしいフォルダ。
    peeks_wanted: Vec<PathBuf>,
}

impl View<'_> {
    /// DICOM かもしれないファイル。拡張子が他のプラグインのもの（PNG など）でなければ、
    /// ヘッダを読んで確かめる。
    fn may_be_dicom(&self, path: &Path) -> bool {
        may_be_dicom(self.registry, self.dicom_extensions, path)
    }

    /// データセット（検索カタログのあるフォルダ）か。先読みで調べ、分かるまでは違うとする。
    fn is_dataset(&mut self, node: &Node) -> bool {
        if node.ext.mounted {
            return false;
        }
        match self.peeks.get(&node.path) {
            Some(Peek::Done { dataset, .. }) => *dataset,
            Some(Peek::Asked) => false,
            None => {
                self.peeks_wanted.push(node.path.clone());
                false
            }
        }
    }

    /// データセットの登録か更新が途中で止まっているフォルダか。先読みで調べる。
    fn is_unfinished(&self, node: &Node) -> bool {
        !node.ext.mounted
            && matches!(
                self.peeks.get(&node.path),
                Some(Peek::Done {
                    unfinished: true,
                    ..
                })
            )
    }
}

/// DICOM かもしれないファイル（[`View::may_be_dicom`]）。
pub fn may_be_dicom(registry: &Registry, dicom_extensions: &[&str], path: &Path) -> bool {
    has_dicom_extension(dicom_extensions, path) || registry.needs_probe(path)
}

fn has_dicom_extension(dicom_extensions: &[&str], path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| dicom_extensions.iter().any(|d| d.eq_ignore_ascii_case(e)))
}

/// レジストリの DICOM プラグインの拡張子。
pub fn dicom_extensions(registry: &Registry) -> &[&str] {
    registry
        .plugins()
        .iter()
        .find(|p| p.name() == "DICOM")
        .map_or(&[][..], |p| p.extensions())
}

/// ツリーの操作の要求。
pub enum TreeAction {
    /// 右クリックメニューで選んだこと（操作の表から。`app/context_menu.rs`）。
    Picked(Picked),
    /// フォルダへ移る（中身をスロットに並べる）。
    Cd(PathBuf),
    /// ZIP を展開して別のルートとして開く。
    Mount(PathBuf),
    /// スロットからドラッグした画像 `file` をフォルダ `dir` にドロップした。
    Drop { file: PathBuf, dir: PathBuf },
    /// DICOM のシリーズを出す。そのシリーズのあるフォルダへ移り、シリーズのスロットを選ぶ。
    Series {
        folder: PathBuf,
        files: Vec<PathBuf>,
    },
    /// 検査・シリーズの全ファイルを送信先やアプリへ送る（右クリックの「送る ▸」）。
    Send {
        choice: crate::send_app::Choice,
        targets: Vec<PathBuf>,
    },
}

impl Hooks<Mark> for View<'_> {
    type Action = TreeAction;

    fn list_dir(&self, dir: &Path) -> Vec<Node> {
        list_dir(dir, self.registry, self.order)
    }

    fn shows(&self, node: &Node) -> bool {
        node.is_dir || node.ext.archive
    }

    fn is_current(&self, node: &Node) -> bool {
        self.current == Some(node.path.as_path())
    }

    /// 先読みの結果を使う。分かるまでは ▶ を出しておく（基底の既定）。
    fn may_have_children(&mut self, node: &Node) -> Option<bool> {
        match self.peeks.get(&node.path) {
            Some(Peek::Done { children, .. }) => Some(*children),
            Some(Peek::Asked) => None,
            None => {
                self.peeks_wanted.push(node.path.clone());
                None
            }
        }
    }

    /// DICOM の検査があるか。ヘッダを読んでいる間と、まだ読んでいない DICOM らしいファイルが
    /// あるときも「ある」として ▶ を残す。
    fn has_extra_children(&mut self, dir: &Path, children: &[Node]) -> bool {
        match self.series.scan(dir) {
            Scan::Done(studies) => !studies.is_empty(),
            Scan::Reading => true,
            Scan::NotStarted => children
                .iter()
                .any(|c| !c.is_dir && self.may_be_dicom(&c.path)),
        }
    }

    fn extra_children(
        &mut self,
        ui: &mut egui::Ui,
        dir: &Path,
        children: &[Node],
        out: &mut Vec<TreeAction>,
    ) {
        let studies = self.dicom_studies(dir, children);
        for study in &studies {
            study_ui(ui, dir, study, self, out);
        }
        if studies.is_empty() && matches!(self.series.scan(dir), Scan::Reading) {
            ui.weak(tr(self.lang, "Reading DICOM headers…"));
        }
    }

    fn label(&mut self, ui: &mut egui::Ui, node: &Node, is_current: bool) -> egui::Response {
        if node.ext.mounted {
            let text = format!("{} {}", egui_phosphor::regular::FILE_ZIP, node.name);
            ui.selectable_label(is_current, text)
        } else if self.is_unfinished(node) {
            let text = egui::RichText::new(node.name.as_str()).color(crate::theme::UNFINISHED);
            ui.selectable_label(is_current, text).on_hover_text(tr(
                self.lang,
                "Registering this dataset stopped partway. Open the folder to continue.",
            ))
        } else if self.is_dataset(node) {
            let text = egui::RichText::new(node.name.as_str()).color(crate::theme::DATASET);
            ui.selectable_label(is_current, text)
        } else {
            ui.selectable_label(is_current, node.name.as_str())
        }
    }

    /// ドロップの受け付けと右クリックメニュー。クリック（cd）とスクロールは基底が扱う。
    fn after_label(&mut self, label: &egui::Response, node: &Node, out: &mut Vec<TreeAction>) {
        // 展開した ZIP の中は書き込めないので、ドロップ先にしない。
        if !node.ext.mounted {
            if label.dnd_hover_payload::<DraggedFile>().is_some() {
                label.clone().highlight();
            }
            if let Some(file) = label.dnd_release_payload::<DraggedFile>() {
                out.push(TreeAction::Drop {
                    file: file.0.clone(),
                    dir: node.path.clone(),
                });
            }
        }
        let kind = if node.ext.mounted {
            FolderKind::MountedRoot
        } else {
            FolderKind::Plain
        };
        let picked = context_menu::folder_menu(label, self.menus, &node.path, kind);
        out.extend(picked.into_iter().map(TreeAction::Picked));
    }

    /// ZIP ファイル。クリックで展開する（基底の `clicked` から [`FolderTree::ui`] が Mount にする）。
    fn leaf(
        &mut self,
        ui: &mut egui::Ui,
        node: &Node,
        out: &mut Vec<TreeAction>,
    ) -> egui::Response {
        let text = format!("{} {}", egui_phosphor::regular::FILE_ZIP, node.name);
        let r = without_toggle(ui, |ui| ui.selectable_label(false, text));
        let picked = context_menu::folder_menu(&r, self.menus, &node.path, FolderKind::Zip);
        out.extend(picked.into_iter().map(TreeAction::Picked));
        r
    }
}

impl View<'_> {
    /// フォルダ `dir` の DICOM の検査。まだヘッダを読んでいなければ読むよう頼む。
    fn dicom_studies(&mut self, dir: &Path, children: &[Node]) -> Vec<Study> {
        match self.series.scan(dir) {
            Scan::Done(studies) => studies.to_vec(),
            Scan::Reading => Vec::new(),
            Scan::NotStarted => {
                let files: Vec<PathBuf> = children
                    .iter()
                    .filter(|c| !c.is_dir && self.may_be_dicom(&c.path))
                    .map(|c| c.path.clone())
                    .collect();
                if !files.is_empty() {
                    self.scans.push((dir.to_path_buf(), files));
                }
                Vec::new()
            }
        }
    }
}

/// 検査の見出しと、その下のシリーズ。
fn study_ui(ui: &mut egui::Ui, dir: &Path, study: &Study, view: &View, out: &mut Vec<TreeAction>) {
    let id = ui.make_persistent_id((dir, &study.uid));
    let (_, header, _) = CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| ui.label(study.title()))
        .body(|ui| {
            for series in &study.series {
                series_ui(ui, dir, series, view, out);
            }
        });
    let files = study.series.iter().flat_map(|s| s.files.clone()).collect();
    files_menu(&header.inner, files, view, out);
}

fn series_ui(
    ui: &mut egui::Ui,
    dir: &Path,
    series: &Series,
    view: &View,
    out: &mut Vec<TreeAction>,
) {
    let selected = view
        .active_image
        .is_some_and(|p| series.files.iter().any(|f| f == p));
    let r = ui.selectable_label(selected, series.title());
    if r.clicked() {
        out.push(TreeAction::Series {
            folder: dir.to_path_buf(),
            files: series.files.clone(),
        });
    }
    files_menu(&r, series.files.clone(), view, out);
}

/// 検査・シリーズの右クリックメニュー。その全ファイルを送る。
fn files_menu(r: &egui::Response, files: Vec<PathBuf>, view: &View, out: &mut Vec<TreeAction>) {
    vw_ui::menu::context(r, |ui| {
        let first = files.first().map(PathBuf::as_path).unwrap_or(Path::new(""));
        let menus = view.menus;
        let offer =
            crate::send_app::Offer::for_path(menus.apps, Some(menus.destinations), first, false);
        if let Some(choice) = crate::send_app::menu(ui, menus.lang, &offer) {
            out.push(TreeAction::Send {
                choice,
                targets: files.clone(),
            });
        }
    });
}

/// まだ読んでいないフォルダの下に項目があるかの先読みの状態。
#[derive(Clone, Copy)]
enum Peek {
    Asked,
    Done {
        /// 開いたときに下へ出す項目があるか（[`has_tree_children`]）。
        children: bool,
        /// 検索カタログがある（データセットとして登録した）フォルダか。
        dataset: bool,
        /// データセットの登録か更新が途中で止まっているか。
        unfinished: bool,
    },
}

/// フォルダの下に項目があるかと、データセットかを、裏のスレッドで 1 段だけ先読みする
/// （[`has_tree_children`]）。
///
/// スレッドを 1 本にして順に調べるのは、表示中のフォルダが多いときに、ネットワークドライブへ
/// 同時にたくさん問い合わせて詰まらせないため。
struct Peeker {
    requests: Sender<PathBuf>,
    results: Receiver<(PathBuf, Peek)>,
    known: HashMap<PathBuf, Peek>,
}

impl Peeker {
    fn start(registry: Registry, wake: impl Fn() + Send + 'static) -> Self {
        let (requests, todo) = std::sync::mpsc::channel::<PathBuf>();
        let (done, results) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for dir in todo {
                let peek = Peek::Done {
                    children: has_tree_children(&dir, &registry),
                    dataset: crate::catalog::exists_in(&dir),
                    unfinished: dir.join(crate::catalog::PARTIAL_FILE).is_file(),
                };
                if done.send((dir, peek)).is_err() {
                    break;
                }
                wake();
            }
        });
        Self {
            requests,
            results,
            known: HashMap::new(),
        }
    }

    fn receive(&mut self) {
        for (dir, peek) in self.results.try_iter() {
            self.known.insert(dir, peek);
        }
    }

    fn ask(&mut self, dirs: Vec<PathBuf>) {
        for dir in dirs {
            if self.known.contains_key(&dir) {
                continue;
            }
            if self.requests.send(dir.clone()).is_ok() {
                self.known.insert(dir, Peek::Asked);
            }
        }
    }
}

/// 中身を読んで DICOM か確かめるファイルの数の上限（[`has_tree_children`]）。
const PEEK_PROBES: usize = 32;

/// フォルダ `dir` を開いたときにツリーへ出す項目（サブフォルダ・ZIP・DICOM の検査）がありそうか。
/// 1 つ見つけたらそこで止める。読めないフォルダは開いても何も出ないので、ないとする。
pub fn has_tree_children(dir: &Path, registry: &Registry) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    let dicom_extensions = dicom_extensions(registry);
    let mut probes = 0;
    for e in rd.flatten() {
        if is_hidden_name(&e.file_name().to_string_lossy()) {
            continue;
        }
        let path = e.path();
        if entry_is_dir(&e) || crate::archive::is_archive(&path) {
            return true;
        }
        if has_dicom_extension(dicom_extensions, &path) {
            return true;
        }
        if registry.needs_probe(&path) {
            // 中身を読むのは決まった数まで。/usr/bin のように拡張子のないファイルが数千ある
            // フォルダで、先読みのためだけに全部を開かないため。超えたら ▶ を残す側に倒す。
            if probes == PEEK_PROBES {
                return true;
            }
            probes += 1;
            if registry.supports_file(&path) {
                return true;
            }
        }
    }
    false
}

/// ツリーで描くときに外から渡すもの。
pub struct Context<'a> {
    pub registry: &'a Registry,
    pub series: &'a SeriesIndex,
    pub current: Option<&'a Path>,
    pub active_image: Option<&'a Path>,
    pub lang: Language,
    pub menus: &'a MenuContext<'a>,
    pub click_expands: bool,
}

/// ツリーを描いた結果。
pub struct Drawn {
    pub action: Option<TreeAction>,
    /// ヘッダを読み始めてほしいフォルダと、その中の DICOM らしいファイル。
    pub scans: Vec<(PathBuf, Vec<PathBuf>)>,
}

/// ツリー全体。基底のツリーに、並び順と先読みを足したもの。
pub struct FolderTree {
    base: vw_tree::FolderTree<Mark>,
    order: SortOrder,
    /// 初めて描くときに始める。スレッドを起こすのに egui の Context が要るため。
    peeker: Option<Peeker>,
}

/// 並び順 `order` で `registry` の対応ファイルも含めて読む関数（基底に渡す）。
fn lister(registry: &Registry, order: SortOrder) -> impl Fn(&Path) -> Vec<Node> + '_ {
    move |dir| list_dir(dir, registry, order)
}

fn comparer(order: SortOrder) -> impl Fn(&Node, &Node) -> std::cmp::Ordering {
    move |a, b| sort::compare(order, &sort_item(a), &sort_item(b))
}

impl FolderTree {
    /// ルートは、起動時に渡されたフォルダ、ホーム、ドライブ（Windows）または / の順。
    pub fn new(extra_root: Option<PathBuf>, order: SortOrder) -> Self {
        Self::from_base(vw_tree::FolderTree::new(extra_root), order)
    }

    #[cfg(test)]
    pub fn with_roots(roots: Vec<Node>, order: SortOrder) -> Self {
        Self::from_base(vw_tree::FolderTree::with_roots(roots), order)
    }

    fn from_base(base: vw_tree::FolderTree<Mark>, order: SortOrder) -> Self {
        Self {
            base,
            order,
            peeker: None,
        }
    }

    #[cfg(test)]
    pub fn roots(&self) -> &[Node] {
        &self.base.roots
    }

    /// 並び順を変え、表示中のフォルダもすぐ並べ直す。
    pub fn set_order(&mut self, order: SortOrder) {
        if order == self.order {
            return;
        }
        self.order = order;
        // 更新日時順にしたときは、まだ取っていない項目の更新日時をここで取る。
        let prepare = move |n: &mut Node| {
            if order.key == SortKey::Modified && n.modified.is_none() {
                n.modified = modified(&n.path);
            }
        };
        self.base.resort(&prepare, &comparer(order));
    }

    /// 開いているフォルダのパス（親が開いているものだけ）。
    pub fn open_paths(&self) -> Vec<PathBuf> {
        self.base.open_paths()
    }

    /// 前回開いていたフォルダを開き直す。もうないフォルダは閉じたままになる。
    pub fn restore_open(&mut self, paths: &[PathBuf], registry: &Registry) {
        self.base.restore_open(paths, &lister(registry, self.order));
    }

    /// フォルダ `dir` の中身を読み直す（フォルダの監視で変化を知ったとき）。
    pub fn refresh(&mut self, dir: &Path, registry: &Registry) {
        self.forget_peek(dir);
        self.base
            .refresh(dir, &lister(registry, self.order), &comparer(self.order));
    }

    /// `dir` の先読みの結果を捨て、次に描くときに調べ直す。データセットとして登録したときにも使う。
    pub fn forget_peek(&mut self, dir: &Path) {
        if let Some(p) = &mut self.peeker {
            p.known.remove(dir);
        }
    }

    /// 先読みの結果をすべて捨てる。DICOM として中身を調べる拡張子が変わったときに使う。
    pub fn forget_peeks(&mut self) {
        if let Some(p) = &mut self.peeker {
            p.known.clear();
        }
    }

    /// `path` が見えるよう親フォルダを開き、次に描くときにそこまでスクロールする。
    pub fn reveal(&mut self, path: &Path, registry: &Registry) {
        self.base.reveal(path, &lister(registry, self.order));
    }

    /// 展開した ZIP をツリーの先頭にルートとして開く。
    pub fn add_mounted_root(&mut self, dir: PathBuf, name: String) {
        let mut node = Node::dir(dir, Some(name), true);
        node.ext.mounted = true;
        self.base.add_root_first(node);
    }

    pub fn remove_root(&mut self, dir: &Path) {
        self.base.remove_root(dir);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: Context) -> Drawn {
        let peeker = self.peeker.get_or_insert_with(|| {
            let wake = ui.ctx().clone();
            Peeker::start(cx.registry.clone(), move || wake.request_repaint())
        });
        peeker.receive();
        let mut view = View {
            registry: cx.registry,
            current: cx.current,
            active_image: cx.active_image,
            lang: cx.lang,
            order: self.order,
            series: cx.series,
            dicom_extensions: dicom_extensions(cx.registry),
            peeks: &peeker.known,
            menus: cx.menus,
            scans: Vec::new(),
            peeks_wanted: Vec::new(),
        };
        self.base.click_expands = cx.click_expands;
        let drawn = self.base.ui(ui, &mut view);
        let View {
            scans,
            peeks_wanted,
            ..
        } = view;
        peeker.ask(peeks_wanted);
        // メニューの操作を優先する。見出しのクリックは、ZIP なら展開、フォルダなら cd。
        let action = drawn.actions.into_iter().next().or_else(|| {
            let p = drawn.clicked?;
            let archive = self
                .base
                .roots
                .iter()
                .find_map(|r| r.find(&p))
                .is_some_and(|n| n.ext.archive);
            Some(if archive {
                TreeAction::Mount(p)
            } else {
                TreeAction::Cd(p)
            })
        });
        Drawn { action, scans }
    }
}

/// フォルダの直下を、フォルダ → ファイルの順、それぞれ `order` の順で返す。
/// 隠すもの（[`is_hidden_name`]）と、対応していない形式のファイルは除く。
/// 読めないフォルダ（権限がないなど）は空として扱い、ツリー全体の表示は止めない。
fn list_dir(path: &Path, registry: &Registry, order: SortOrder) -> Vec<Node> {
    let Ok(rd) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut nodes: Vec<Node> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_hidden_name(&name) {
                return None;
            }
            let path = e.path();
            let is_dir = entry_is_dir(&e);
            let archive = !is_dir && crate::archive::is_archive(&path);
            if !is_dir && !archive && !registry.supports_file(&path) {
                return None;
            }
            // 更新日時は並べるのに使うときだけ取る。Linux では項目ごとに stat が要り、
            // 数万件のフォルダでは名前順のときまで遅くなるため。
            let modified = (order.key == SortKey::Modified)
                .then(|| e.metadata().ok()?.modified().ok())
                .flatten();
            let mut node = Node::entry(path, name, is_dir, modified);
            node.ext.archive = archive;
            Some(node)
        })
        .collect();
    sort_nodes(&mut nodes, order);
    nodes
}

/// フォルダの直下の中身。それぞれツリーと同じ規則と順で並ぶ。
#[derive(Debug, Default)]
pub struct Listing {
    pub folders: Vec<PathBuf>,
    pub archives: Vec<PathBuf>,
    pub images: Vec<PathBuf>,
}

pub fn list_folder(path: &Path, registry: &Registry, order: SortOrder) -> Listing {
    let mut out = Listing::default();
    for n in list_dir(path, registry, order) {
        if n.is_dir {
            out.folders.push(n.path);
        } else if n.ext.archive {
            out.archives.push(n.path);
        } else {
            out.images.push(n.path);
        }
    }
    out
}

fn sort_nodes(nodes: &mut [Node], order: SortOrder) {
    nodes.sort_by(comparer(order));
}

fn sort_item(n: &Node) -> SortItem<'_> {
    SortItem {
        name: &n.name,
        is_dir: n.is_dir,
        modified: n.modified,
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// `path` と同じフォルダにある対応画像を、ツリーと同じ `order` で返す（前後の画像への移動用）。
pub fn sibling_images(path: &Path, registry: &Registry, order: SortOrder) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut files: Vec<Node> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| registry.supports_file(p) && p.is_file())
                .map(|path| {
                    let modified = (order.key == SortKey::Modified)
                        .then(|| modified(&path))
                        .flatten();
                    Node::entry(path.clone(), display_name(&path), false, modified)
                })
                .collect()
        })
        .unwrap_or_default();
    sort_nodes(&mut files, order);
    files.into_iter().map(|n| n.path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{TempDir, all_registry, png_registry};

    fn names(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn lists_folders_first_then_supported_files_by_name() {
        let dir = TempDir::new();
        dir.mkdir("Zeta");
        dir.mkdir("alpha");
        dir.touch("b.PNG");
        dir.touch("A.png");
        dir.touch("notes.txt");
        let nodes = list_dir(dir.path(), &png_registry(), SortOrder::default());
        assert_eq!(names(&nodes), ["alpha", "Zeta", "A.png", "b.PNG"]);
        assert!(nodes[0].is_dir && !nodes[2].is_dir);
    }

    #[test]
    fn hides_dot_files_and_dot_folders() {
        let dir = TempDir::new();
        dir.mkdir(".git");
        dir.touch(".hidden.png");
        dir.touch("shown.png");
        assert_eq!(
            names(&list_dir(dir.path(), &png_registry(), SortOrder::default())),
            ["shown.png"]
        );
    }

    #[test]
    fn unreadable_folder_is_empty() {
        let dir = TempDir::new();
        let missing = dir.path().join("does-not-exist");
        assert!(list_dir(&missing, &png_registry(), SortOrder::default()).is_empty());
    }

    #[test]
    fn siblings_are_supported_files_in_the_same_folder_sorted() {
        let dir = TempDir::new();
        dir.touch("c.png");
        dir.touch("a.png");
        dir.touch("B.png");
        dir.touch("skip.txt");
        dir.mkdir("folder.png");
        let files = sibling_images(
            &dir.path().join("a.png"),
            &png_registry(),
            SortOrder::default(),
        );
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, ["a.png", "B.png", "c.png"]);
    }

    fn by(key: SortKey, descending: bool) -> SortOrder {
        SortOrder { key, descending }
    }

    #[test]
    fn numbered_files_are_listed_by_number_by_default() {
        let dir = TempDir::new();
        dir.touch("img10.png");
        dir.touch("img2.png");
        dir.touch("img1.png");
        let nodes = list_dir(dir.path(), &png_registry(), SortOrder::default());
        assert_eq!(names(&nodes), ["img1.png", "img2.png", "img10.png"]);
    }

    #[test]
    fn files_can_be_listed_by_modified_date_newest_first() {
        let dir = TempDir::new();
        dir.touch_at("old.png", 1_000);
        dir.touch_at("new.png", 3_000);
        dir.touch_at("mid.png", 2_000);
        dir.mkdir("folder");
        let order = by(SortKey::Modified, true);
        let nodes = list_dir(dir.path(), &png_registry(), order);
        assert_eq!(names(&nodes), ["folder", "new.png", "mid.png", "old.png"]);
    }

    #[test]
    fn changing_the_order_resorts_open_folders_and_keeps_them_open() {
        let dir = TempDir::new();
        dir.touch_at("a.png", 2_000);
        dir.touch_at("b.png", 1_000);
        let mut tree = tree_at(dir.path());
        tree.restore_open(&[dir.path().to_path_buf()], &png_registry());
        tree.set_order(by(SortKey::Modified, false));
        assert_eq!(
            names(tree.roots()[0].children.as_ref().unwrap()),
            ["b.png", "a.png"]
        );
        assert_eq!(tree.open_paths(), [dir.path().to_path_buf()]);
    }

    #[test]
    fn siblings_follow_the_same_order_as_the_tree() {
        let dir = TempDir::new();
        dir.touch_at("x10.png", 1_000);
        dir.touch_at("x9.png", 2_000);
        let registry = png_registry();
        let file_names = |order| -> Vec<String> {
            sibling_images(&dir.path().join("x9.png"), &registry, order)
                .iter()
                .map(|p| display_name(p))
                .collect()
        };
        assert_eq!(
            file_names(by(SortKey::NameNatural, false)),
            ["x9.png", "x10.png"]
        );
        assert_eq!(
            file_names(by(SortKey::NameString, false)),
            ["x10.png", "x9.png"]
        );
        assert_eq!(
            file_names(by(SortKey::Modified, true)),
            ["x9.png", "x10.png"]
        );
    }

    #[test]
    fn display_name_falls_back_to_whole_path_for_roots() {
        assert_eq!(display_name(Path::new("/tmp/pic.png")), "pic.png");
        assert_eq!(display_name(Path::new("/")), "/");
    }

    #[test]
    fn extra_root_comes_first_and_is_open() {
        let dir = TempDir::new();
        let tree = FolderTree::new(Some(dir.path().to_path_buf()), SortOrder::default());
        assert_eq!(tree.roots()[0].path, dir.path());
        assert!(tree.roots()[0].open);
        assert!(tree.roots().len() >= 2, "home or filesystem roots follow");
    }

    #[test]
    fn refresh_shows_new_files_and_keeps_open_subfolders() {
        let dir = TempDir::new();
        let sub = dir.mkdir("sub");
        dir.touch("a.png");
        let mut tree = tree_at(dir.path());
        tree.restore_open(&[dir.path().to_path_buf(), sub], &png_registry());
        dir.touch("b.png");
        std::fs::remove_file(dir.path().join("a.png")).unwrap();
        tree.refresh(dir.path(), &png_registry());
        let children = tree.roots()[0].children.as_ref().unwrap();
        assert_eq!(names(children), ["sub", "b.png"]);
        assert!(children[0].open && children[0].children.is_some());
    }

    #[test]
    fn refresh_skips_folders_never_opened() {
        let dir = TempDir::new();
        dir.touch("a.png");
        let mut tree = tree_at(dir.path());
        tree.refresh(dir.path(), &png_registry());
        assert!(tree.roots()[0].children.is_none());
    }

    fn tree_at(root: &Path) -> FolderTree {
        FolderTree::with_roots(
            vec![Node::dir(root.to_path_buf(), None, false)],
            SortOrder::default(),
        )
    }

    #[test]
    fn folder_listing_splits_subfolders_zips_and_images_in_tree_order() {
        let dir = TempDir::new();
        dir.mkdir("sub2");
        dir.mkdir("sub10");
        dir.touch("img10.png");
        dir.touch("img2.png");
        dir.touch("notes.txt");
        dir.touch("pack.zip");
        let listing = list_folder(dir.path(), &png_registry(), SortOrder::default());
        let names = |v: &[PathBuf]| v.iter().map(|p| display_name(p)).collect::<Vec<_>>();
        assert_eq!(names(&listing.folders), ["sub2", "sub10"]);
        assert_eq!(names(&listing.archives), ["pack.zip"]);
        assert_eq!(names(&listing.images), ["img2.png", "img10.png"]);
    }

    #[test]
    fn folder_with_a_subfolder_or_zip_shows_children() {
        let with_sub = TempDir::new();
        with_sub.mkdir("inner");
        assert!(has_tree_children(with_sub.path(), &all_registry()));
        let with_zip = TempDir::new();
        with_zip.touch("scans.zip");
        assert!(has_tree_children(with_zip.path(), &all_registry()));
    }

    #[test]
    fn recycle_bins_are_not_listed_in_any_letter_case() {
        let dir = TempDir::new();
        dir.mkdir("$RECYCLE.BIN");
        dir.mkdir("$Recycle.Bin2");
        dir.mkdir("recycler");
        dir.mkdir("photos");
        let nodes = list_dir(dir.path(), &png_registry(), SortOrder::default());
        assert_eq!(names(&nodes), ["$Recycle.Bin2", "photos"]);
    }

    #[test]
    fn folder_with_only_a_recycle_bin_is_a_leaf() {
        let dir = TempDir::new();
        dir.mkdir("$RECYCLE.BIN");
        assert!(!has_tree_children(dir.path(), &all_registry()));
    }

    #[test]
    fn folder_with_only_images_and_hidden_folders_is_a_leaf() {
        let dir = TempDir::new();
        dir.touch("a.png");
        dir.touch("notes.txt");
        dir.mkdir(".git");
        assert!(!has_tree_children(dir.path(), &all_registry()));
    }

    #[test]
    fn folder_with_a_dicom_file_shows_children_but_one_with_plain_extensionless_files_does_not() {
        let plain = TempDir::new();
        std::fs::write(plain.path().join("README"), b"hello").unwrap();
        assert!(!has_tree_children(plain.path(), &all_registry()));
        let dicom = TempDir::new();
        let mut data = vec![0u8; 128];
        data.extend_from_slice(b"DICM");
        std::fs::write(dicom.path().join("IM0001"), data).unwrap();
        assert!(has_tree_children(dicom.path(), &all_registry()));
    }

    #[test]
    fn unreadable_folder_is_a_leaf() {
        let dir = TempDir::new();
        assert!(!has_tree_children(
            &dir.path().join("missing"),
            &all_registry()
        ));
    }

    #[test]
    fn revealing_a_folder_opens_its_parents_but_not_the_folder() {
        let dir = TempDir::new();
        let sub = dir.mkdir("a/b");
        let mut tree = tree_at(dir.path());
        tree.reveal(&sub, &png_registry());
        assert_eq!(
            tree.open_paths(),
            [dir.path().to_path_buf(), dir.path().join("a")]
        );
        assert!(tree.roots()[0].find(&sub).is_some_and(|n| !n.open));
    }

    #[test]
    fn revealing_opens_only_the_first_root_that_holds_the_folder() {
        let dir = TempDir::new();
        let sub = dir.mkdir("a");
        let mut tree = FolderTree::with_roots(
            vec![
                Node::dir(dir.path().to_path_buf(), None, false),
                Node::dir(dir.path().parent().unwrap().to_path_buf(), None, false),
            ],
            SortOrder::default(),
        );
        tree.reveal(&sub, &png_registry());
        assert!(tree.roots()[0].open);
        assert!(!tree.roots()[1].open);
    }

    #[test]
    fn closed_tree_has_no_open_paths() {
        let dir = TempDir::new();
        assert!(tree_at(dir.path()).open_paths().is_empty());
    }

    #[test]
    fn open_folders_are_restored_with_their_contents() {
        let dir = TempDir::new();
        let a = dir.mkdir("a");
        let b = dir.mkdir("a/b");
        dir.touch("a/b/pic.png");
        let mut tree = tree_at(dir.path());
        let paths = vec![dir.path().to_path_buf(), a, b];
        tree.restore_open(&paths, &png_registry());
        assert_eq!(tree.open_paths(), paths);
        let b_node = &tree.roots()[0].children.as_ref().unwrap()[0]
            .children
            .as_ref()
            .unwrap()[0];
        assert_eq!(names(b_node.children.as_ref().unwrap()), ["pic.png"]);
    }

    #[test]
    fn folder_inside_a_closed_parent_is_not_restored() {
        let dir = TempDir::new();
        let a = dir.mkdir("a");
        dir.mkdir("a/b");
        let mut tree = tree_at(dir.path());
        tree.restore_open(&[a.join("b")], &png_registry());
        assert!(tree.open_paths().is_empty());
        assert!(
            tree.roots()[0].children.is_none(),
            "closed folders are not read"
        );
    }

    #[test]
    fn vanished_folders_are_skipped() {
        let dir = TempDir::new();
        let mut tree = tree_at(dir.path());
        let gone = dir.path().join("gone");
        tree.restore_open(&[dir.path().to_path_buf(), gone], &png_registry());
        assert_eq!(tree.open_paths(), [dir.path().to_path_buf()]);
    }
}
