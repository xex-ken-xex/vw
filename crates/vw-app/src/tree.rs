//! フォルダツリー。フォルダと、対応形式の画像ファイルだけを表示する。
//!
//! フォルダの中身は初めて開いたときに読む。起動時にドライブ全体を走査すると
//! 起動が遅くなるうえ、ネットワークドライブで固まることがあるため。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, collapsing_header::CollapsingState};
use vw_core::Registry;

use crate::i18n::tr;
use crate::labels::{self, Label, LabelStore};
use crate::series::{Scan, Series, SeriesIndex, Study};
use crate::settings::{FolderAction, Language, TreeMode};
use crate::sort::{self, SortItem, SortKey, SortOrder};

/// ツリーの 1 項目（フォルダまたはファイル）。
pub struct Node {
    pub path: PathBuf,
    name: String,
    is_dir: bool,
    /// フォルダを初めて開くまでは `None`。一度読んだら閉じても保持する。
    children: Option<Vec<Node>>,
    /// 開いているか。egui の内部状態に任せず自分で持つのは、終了時に保存して次回復元するため。
    open: bool,
    /// 更新日時。更新日時順で並べるときだけ取る（[`list_dir`]）。
    modified: Option<SystemTime>,
}

/// 項目に付ける印と、出すファイルの絞り込み。
pub struct Marks<'a> {
    pub labels: &'a LabelStore,
    pub show_file: &'a dyn Fn(&Path) -> bool,
}

/// ツリーを描くときに全項目で共通の情報。
struct View<'a> {
    registry: &'a Registry,
    labels: &'a LabelStore,
    /// 絞り込みでファイルを出すか。
    show_file: &'a dyn Fn(&Path) -> bool,
    selected: Option<&'a Path>,
    lang: Language,
    order: SortOrder,
    /// ファイルも出すか（ノーマルモード）。
    show_files: bool,
    series: &'a SeriesIndex,
    /// DICOM の拡張子。拡張子のないファイルと、これらのファイルがあるフォルダだけヘッダを読む。
    dicom_extensions: &'a [&'a str],
}

impl View<'_> {
    fn may_be_dicom(&self, path: &Path) -> bool {
        match path.extension().and_then(|e| e.to_str()) {
            None => true,
            Some(e) => self
                .dicom_extensions
                .iter()
                .any(|d| d.eq_ignore_ascii_case(e)),
        }
    }
}

/// ツリーを描いた結果。
#[derive(Default)]
struct Out {
    action: Option<TreeAction>,
    /// クリックされたフォルダの見出しと、Shift を押していたか。開閉と読み込みのどちらにするかは
    /// [`ClickTracker`] が決める。
    folder_click: Option<(PathBuf, bool)>,
    /// ヘッダを読み始めるフォルダと、その中の DICOM らしいファイル。
    scans: Vec<(PathBuf, Vec<PathBuf>)>,
}

/// ツリーでファイルがクリックされたときの要求。
pub enum TreeAction {
    /// アクティブなスロットに表示する（通常のクリック）。
    Open(PathBuf),
    /// スロットを追加して表示する（Ctrl / Cmd + クリック）。
    Add(PathBuf),
    /// フォルダの見出しをクリックした（ツリーを Ctrl+C の対象にするだけ）。
    Select,
    /// フルパスをコピーする（右クリックメニュー）。
    Copy(PathBuf),
    /// フォルダの画像をスロットに読み込む（画像自動展開モード）。
    Load(PathBuf),
    /// DICOM のシリーズを 1 つのスロットに出す。`add` なら Ctrl / Cmd + クリックでスロットを足す。
    Series {
        folder: PathBuf,
        title: String,
        files: Vec<PathBuf>,
        add: bool,
    },
}

/// フォルダの見出しのクリックの扱い方（設定から作る）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderClicks {
    pub mode: TreeMode,
    pub action: FolderAction,
    pub double_click: Duration,
}

/// 1 回のクリックの結果。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ClickEffect {
    /// 今すぐ開閉するフォルダ。
    pub toggle: Vec<PathBuf>,
    /// 画像を読み込むフォルダ。
    pub load: Option<PathBuf>,
}

/// フォルダの見出しのクリックを、開閉と読み込みに振り分ける。
///
/// egui の `double_clicked()` を使わないのは、ダブルクリックの 1 回目もクリックとして届き、
/// 開閉を遅らせるか戻すかの判断に 1 回目の時刻が要るため。間隔を設定で変えられるようにもしたい。
#[derive(Default)]
pub struct ClickTracker {
    /// 直前のクリック。ダブルクリックの 1 回目かもしれないもの。
    last: Option<(PathBuf, Instant)>,
}

impl ClickTracker {
    pub fn click(
        &mut self,
        how: FolderClicks,
        path: &Path,
        shift: bool,
        now: Instant,
    ) -> ClickEffect {
        let mut out = ClickEffect::default();
        if how.mode == TreeMode::Normal {
            out.toggle.push(path.to_owned());
            return out;
        }
        let second = self
            .last
            .take_if(|(p, at)| p == path && now - *at <= how.double_click);
        match how.action {
            FolderAction::ShiftClick if shift => out.load = Some(path.to_owned()),
            FolderAction::ShiftClick => out.toggle.push(path.to_owned()),
            FolderAction::Immediate => {
                // 2 回目も開閉するので、1 回目の開閉が打ち消されて元に戻る。
                out.toggle.push(path.to_owned());
                match second {
                    Some(_) => out.load = Some(path.to_owned()),
                    None => self.last = Some((path.to_owned(), now)),
                }
            }
            FolderAction::Delayed => match second {
                Some(_) => out.load = Some(path.to_owned()),
                None => {
                    // 別のフォルダを続けてクリックしたら、前のフォルダの開閉は待たずに済ませる。
                    if let Some((other, _)) = self.last.replace((path.to_owned(), now)) {
                        out.toggle.push(other);
                    }
                }
            },
        }
        out
    }

    /// 待ち時間が過ぎた、遅らせていた開閉。
    pub fn due(&mut self, how: FolderClicks, now: Instant) -> Option<PathBuf> {
        let (_, at) = self.last.as_ref()?;
        if now - *at <= how.double_click {
            return None;
        }
        let (path, _) = self.last.take()?;
        (how.mode == TreeMode::AutoLoad && how.action == FolderAction::Delayed).then_some(path)
    }

    /// 次に [`Self::due`] を確かめるまでの時間。
    fn wait(&self, how: FolderClicks, now: Instant) -> Option<Duration> {
        let (_, at) = self.last.as_ref()?;
        Some((*at + how.double_click).saturating_duration_since(now) + Duration::from_millis(1))
    }
}

impl Node {
    /// フォルダの項目を作る。`name` を省くとパスの末尾を表示名にする。
    pub fn dir(path: PathBuf, name: Option<String>, open: bool) -> Self {
        let name = name.unwrap_or_else(|| display_name(&path));
        Self {
            path,
            name,
            is_dir: true,
            children: None,
            open,
            modified: None,
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, view: &View, out: &mut Out) {
        let lang = view.lang;
        let is_selected = view.selected == Some(self.path.as_path());
        if !self.is_dir {
            let r = ui.selectable_label(is_selected, self.text(ui, view.labels.get(&self.path)));
            if r.clicked() {
                out.action = Some(if ui.input(|i| i.modifiers.command) {
                    TreeAction::Add(self.path.clone())
                } else {
                    TreeAction::Open(self.path.clone())
                });
            }
            copy_menu(&r, &self.path, lang, &mut out.action);
            return;
        }
        // CollapsingHeader を使わないのは、見出しに選択状態を出せず、右クリックメニューも
        // 付けられないため。開閉の ▶ と見出しの文字を別の部品にして組み立てる。
        let id = ui.make_persistent_id(&self.path);
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, self.open);
        state.set_open(self.open);
        let (toggle, header, _) = state
            .show_header(ui, |ui| {
                ui.selectable_label(is_selected, self.name.as_str())
            })
            .body(|ui| {
                let children = self
                    .children
                    .get_or_insert_with(|| list_dir(&self.path, view.registry, view.order));
                let studies = dicom_studies(&self.path, children, view, out);
                // シリーズに入ったファイルは、ファイルとしては出さない。数百枚の CT のファイルが
                // シリーズの下に並ぶと、ツリーがそれで埋まるため。
                let in_series: HashSet<&Path> = studies
                    .iter()
                    .flat_map(|s| &s.series)
                    .flat_map(|s| &s.files)
                    .map(PathBuf::as_path)
                    .collect();
                // 画像自動展開モードでもファイルは読んで持っておく。モードを切り替えたときに
                // フォルダを読み直さず、開閉状態もそのまま保てるようにするため。
                let mut shown = 0;
                for child in children.iter_mut().filter(|c| c.is_dir) {
                    child.ui(ui, view, out);
                    shown += 1;
                }
                for study in &studies {
                    study_ui(ui, &self.path, study, view, out);
                    shown += 1;
                }
                if studies.is_empty() && matches!(view.series.scan(&self.path), Scan::Reading) {
                    ui.weak(tr(lang, "Reading DICOM headers…"));
                    shown += 1;
                }
                let shown_file = |c: &&mut Node| {
                    !c.is_dir
                        && view.show_files
                        && !in_series.contains(c.path.as_path())
                        && (view.show_file)(&c.path)
                };
                for child in children.iter_mut().filter(shown_file) {
                    child.ui(ui, view, out);
                    shown += 1;
                }
                if shown == 0 {
                    ui.weak(tr(
                        lang,
                        if view.show_files {
                            "(empty)"
                        } else {
                            "(no subfolders)"
                        },
                    ));
                }
            });
        let label = header.inner;
        if toggle.clicked() {
            self.open = !self.open;
        }
        if label.clicked() {
            out.action = Some(TreeAction::Select);
            out.folder_click = Some((self.path.clone(), ui.input(|i| i.modifiers.shift)));
        }
        copy_menu(&label, &self.path, lang, &mut out.action);
    }

    /// 表示する名前。フラグやタグがあれば一緒に出す。
    fn text(&self, ui: &egui::Ui, label: Label) -> egui::WidgetText {
        if label.is_empty() {
            return self.name.as_str().into();
        }
        let font = egui::TextStyle::Button.resolve(ui.style());
        let weak = ui.visuals().weak_text_color();
        labels::labeled_name(&self.name, &label, egui::Color32::PLACEHOLDER, weak, font).into()
    }

    /// `path` の項目。読み込み済みの子だけを辿る。
    fn find_mut(&mut self, path: &Path) -> Option<&mut Node> {
        if self.path == path {
            return Some(self);
        }
        if !path.starts_with(&self.path) {
            return None;
        }
        self.children
            .iter_mut()
            .flatten()
            .find_map(|c| c.find_mut(path))
    }

    fn collect_open(&self, out: &mut Vec<PathBuf>) {
        if !self.open {
            return;
        }
        out.push(self.path.clone());
        for child in self.children.iter().flatten() {
            child.collect_open(out);
        }
    }

    /// `paths` に含まれるフォルダを開き、その中身を読んで子にも同じことをする。
    /// 含まれないフォルダは読まないので、復元にかかる時間は開いていたフォルダの数で決まる。
    fn restore_open(&mut self, paths: &[PathBuf], registry: &Registry, order: SortOrder) {
        if !self.is_dir || !paths.contains(&self.path) {
            return;
        }
        self.open = true;
        let children = self
            .children
            .get_or_insert_with(|| list_dir(&self.path, registry, order));
        for child in children {
            child.restore_open(paths, registry, order);
        }
    }

    /// 読み込み済みのフォルダ `dir` の中身を読み直す。前からある項目は残すので、
    /// サブフォルダの開閉状態と、読み込み済みの孫はそのまま保たれる。
    fn refresh(&mut self, dir: &Path, registry: &Registry, order: SortOrder) {
        if !dir.starts_with(&self.path) {
            return;
        }
        let Some(children) = &mut self.children else {
            return;
        };
        if self.path != dir {
            for child in children.iter_mut() {
                child.refresh(dir, registry, order);
            }
            return;
        }
        let mut old: Vec<Node> = std::mem::take(children);
        *children = list_dir(dir, registry, order)
            .into_iter()
            .map(
                |fresh| match old.iter().position(|o| o.path == fresh.path) {
                    Some(i) => {
                        let mut kept = old.swap_remove(i);
                        kept.modified = fresh.modified;
                        kept
                    }
                    None => fresh,
                },
            )
            .collect();
        sort_nodes(children, order);
    }

    /// 読み込み済みの子を `order` で並べ直す。フォルダを読み直さないのは、開閉状態と
    /// まだ読んでいない孫の状態を保ったまま、設定の変更をすぐ見せるため。
    fn resort(&mut self, order: SortOrder) {
        let Some(children) = &mut self.children else {
            return;
        };
        if order.key == SortKey::Modified {
            for c in children.iter_mut().filter(|c| c.modified.is_none()) {
                c.modified = modified(&c.path);
            }
        }
        sort_nodes(children, order);
        for c in children {
            c.resort(order);
        }
    }
}

/// フォルダ `dir` の DICOM の検査。まだヘッダを読んでいなければ読むよう `out` に頼む。
fn dicom_studies(dir: &Path, children: &[Node], view: &View, out: &mut Out) -> Vec<Study> {
    match view.series.scan(dir) {
        Scan::Done(studies) => studies.to_vec(),
        Scan::Reading => Vec::new(),
        Scan::NotStarted => {
            let files: Vec<PathBuf> = children
                .iter()
                .filter(|c| !c.is_dir && view.may_be_dicom(&c.path))
                .map(|c| c.path.clone())
                .collect();
            if !files.is_empty() {
                out.scans.push((dir.to_path_buf(), files));
            }
            Vec::new()
        }
    }
}

/// 検査の見出しと、その下のシリーズ。
fn study_ui(ui: &mut egui::Ui, dir: &Path, study: &Study, view: &View, out: &mut Out) {
    let id = ui.make_persistent_id((dir, &study.uid));
    CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| ui.label(study.title()))
        .body(|ui| {
            for series in &study.series {
                series_ui(ui, dir, series, view, out);
            }
        });
}

fn series_ui(ui: &mut egui::Ui, dir: &Path, series: &Series, view: &View, out: &mut Out) {
    let selected = view
        .selected
        .is_some_and(|p| series.files.iter().any(|f| f == p));
    let title = series.title();
    let r = ui.selectable_label(selected, &title);
    if r.clicked() {
        out.action = Some(TreeAction::Series {
            folder: dir.to_path_buf(),
            title,
            files: series.files.clone(),
            add: ui.input(|i| i.modifiers.command),
        });
    }
}

/// 右クリックメニュー。選んだ項目のフルパスをコピーする。
fn copy_menu(r: &egui::Response, path: &Path, lang: Language, out: &mut Option<TreeAction>) {
    r.context_menu(|ui| {
        if ui.button(tr(lang, "Copy")).clicked() {
            *out = Some(TreeAction::Copy(path.to_owned()));
            ui.close();
        }
    });
}

/// ツリー全体。ルートは、起動時に渡されたフォルダ、ホーム、ドライブ（Windows）または / の順。
pub struct FolderTree {
    pub roots: Vec<Node>,
    order: SortOrder,
    clicks: ClickTracker,
    series: SeriesIndex,
}

impl FolderTree {
    pub fn new(extra_root: Option<PathBuf>, order: SortOrder) -> Self {
        let mut roots = Vec::new();
        if let Some(p) = extra_root {
            roots.push(Node::dir(p, None, true));
        }
        if let Some(home) = home_dir() {
            roots.push(Node::dir(home, Some("Home".into()), false));
        }
        #[cfg(windows)]
        for letter in b'A'..=b'Z' {
            let drive = PathBuf::from(format!("{}:\\", letter as char));
            if drive.exists() {
                roots.push(Node::dir(
                    drive.clone(),
                    Some(drive.display().to_string()),
                    false,
                ));
            }
        }
        #[cfg(not(windows))]
        roots.push(Node::dir(PathBuf::from("/"), Some("/".into()), false));
        Self {
            roots,
            order,
            clicks: ClickTracker::default(),
            series: SeriesIndex::default(),
        }
    }

    /// 並び順を変え、表示中のフォルダもすぐ並べ直す。
    pub fn set_order(&mut self, order: SortOrder) {
        if order == self.order {
            return;
        }
        self.order = order;
        for root in &mut self.roots {
            root.resort(order);
        }
    }

    /// 開いているフォルダのパス（親が開いているものだけ）。
    pub fn open_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for root in &self.roots {
            root.collect_open(&mut out);
        }
        out
    }

    /// 前回開いていたフォルダを開き直す。もうないフォルダは閉じたままになる。
    pub fn restore_open(&mut self, paths: &[PathBuf], registry: &Registry) {
        for root in &mut self.roots {
            root.restore_open(paths, registry, self.order);
        }
    }

    /// フォルダ `dir` の中身を読み直す（フォルダの監視で変化を知ったとき）。
    /// 同じフォルダが Home と / の両方の下に出ていることがあるので、すべてのルートを辿る。
    pub fn refresh(&mut self, dir: &Path, registry: &Registry) {
        self.series.forget(dir);
        for root in &mut self.roots {
            root.refresh(dir, registry, self.order);
        }
    }

    fn toggle(&mut self, path: &Path) {
        if let Some(node) = self.roots.iter_mut().find_map(|r| r.find_mut(path)) {
            node.open = !node.open;
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        registry: &Registry,
        marks: Marks,
        selected: Option<&Path>,
        lang: Language,
        how: FolderClicks,
    ) -> Option<TreeAction> {
        let now = Instant::now();
        if let Some(path) = self.clicks.due(how, now) {
            self.toggle(&path);
        }
        self.series.poll();
        let dicom_extensions = registry
            .plugins()
            .iter()
            .find(|p| p.name() == "DICOM")
            .map_or(&[][..], |p| p.extensions());
        let view = View {
            registry,
            labels: marks.labels,
            show_file: marks.show_file,
            selected,
            lang,
            order: self.order,
            show_files: how.mode == TreeMode::Normal,
            series: &self.series,
            dicom_extensions,
        };
        let mut out = Out::default();
        for root in &mut self.roots {
            root.ui(ui, &view, &mut out);
        }
        for (dir, files) in out.scans {
            let ctx = ui.ctx().clone();
            self.series
                .start(dir, files, registry.clone(), move || ctx.request_repaint());
        }
        if let Some((path, shift)) = out.folder_click {
            let effect = self.clicks.click(how, &path, shift, now);
            for p in &effect.toggle {
                self.toggle(p);
            }
            if let Some(p) = effect.load {
                out.action = Some(TreeAction::Load(p));
            }
        }
        if let Some(wait) = self.clicks.wait(how, now) {
            ui.ctx().request_repaint_after(wait);
        }
        out.action
    }
}

/// フォルダの直下を、フォルダ → ファイルの順、それぞれ `order` の順で返す。
/// ドットで始まる隠しファイル・フォルダと、対応していない形式のファイルは除く。
/// 読めないフォルダ（権限がないなど）は空として扱い、ツリー全体の表示は止めない。
fn list_dir(path: &Path, registry: &Registry, order: SortOrder) -> Vec<Node> {
    let Ok(rd) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut nodes: Vec<Node> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let path = e.path();
            // 種別は DirEntry::file_type() で取る。path.is_dir() は項目ごとに stat を発行し、
            // 数万件のフォルダで目に見えて遅くなる。シンボリックリンクのときだけ辿って確かめる。
            let is_dir = match e.file_type() {
                Ok(t) if t.is_symlink() => path.is_dir(),
                Ok(t) => t.is_dir(),
                Err(_) => false,
            };
            if !is_dir && !registry.supports_file(&path) {
                return None;
            }
            // 更新日時は並べるのに使うときだけ取る。Linux では項目ごとに stat が要り、
            // 数万件のフォルダでは名前順のときまで遅くなるため。
            let modified = (order.key == SortKey::Modified)
                .then(|| e.metadata().ok()?.modified().ok())
                .flatten();
            Some(Node {
                path,
                name,
                is_dir,
                children: None,
                open: false,
                modified,
            })
        })
        .collect();
    sort_nodes(&mut nodes, order);
    nodes
}

/// フォルダの直下の (サブフォルダ, 対応画像) を、ツリーと同じ規則と順で返す。
pub fn list_folder(
    path: &Path,
    registry: &Registry,
    order: SortOrder,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (dirs, files): (Vec<Node>, Vec<Node>) = list_dir(path, registry, order)
        .into_iter()
        .partition(|n| n.is_dir);
    let paths = |v: Vec<Node>| v.into_iter().map(|n| n.path).collect();
    (paths(dirs), paths(files))
}

fn sort_nodes(nodes: &mut [Node], order: SortOrder) {
    nodes.sort_by(|a, b| sort::compare(order, &a.sort_item(), &b.sort_item()));
}

impl Node {
    fn sort_item(&self) -> SortItem<'_> {
        SortItem {
            name: &self.name,
            is_dir: self.is_dir,
            modified: self.modified,
        }
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
                .map(|path| Node {
                    name: display_name(&path),
                    is_dir: false,
                    children: None,
                    open: false,
                    modified: (order.key == SortKey::Modified)
                        .then(|| modified(&path))
                        .flatten(),
                    path,
                })
                .collect()
        })
        .unwrap_or_default();
    sort_nodes(&mut files, order);
    files.into_iter().map(|n| n.path).collect()
}

/// パスの末尾。ルート（`/` や `C:\\`）のように末尾がないときはパス全体。
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// ホームフォルダ。`dirs` クレートを使わないのは、環境変数 1 つで足りるため。
fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{png_registry, TempDir};

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
            names(tree.roots[0].children.as_ref().unwrap()),
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
        assert_eq!(tree.roots[0].path, dir.path());
        assert!(tree.roots[0].open);
        assert!(tree.roots.len() >= 2, "home or filesystem roots follow");
    }

    #[test]
    fn refresh_shows_new_files_and_keeps_open_subfolders() {
        let dir = TempDir::new();
        let sub = dir.mkdir("sub");
        dir.touch("a.png");
        let mut tree = tree_at(dir.path());
        tree.restore_open(&[dir.path().to_path_buf(), sub.clone()], &png_registry());
        dir.touch("b.png");
        std::fs::remove_file(dir.path().join("a.png")).unwrap();
        tree.refresh(dir.path(), &png_registry());
        let children = tree.roots[0].children.as_ref().unwrap();
        assert_eq!(names(children), ["sub", "b.png"]);
        assert!(children[0].open && children[0].children.is_some());
    }

    #[test]
    fn refresh_skips_folders_never_opened() {
        let dir = TempDir::new();
        dir.touch("a.png");
        let mut tree = tree_at(dir.path());
        tree.refresh(dir.path(), &png_registry());
        assert!(tree.roots[0].children.is_none());
    }

    fn tree_at(root: &Path) -> FolderTree {
        FolderTree {
            roots: vec![Node::dir(root.to_path_buf(), None, false)],
            order: SortOrder::default(),
            clicks: ClickTracker::default(),
            series: SeriesIndex::default(),
        }
    }

    #[test]
    fn folder_listing_splits_subfolders_and_images_in_tree_order() {
        let dir = TempDir::new();
        dir.mkdir("sub2");
        dir.mkdir("sub10");
        dir.touch("img10.png");
        dir.touch("img2.png");
        dir.touch("notes.txt");
        let (dirs, files) = list_folder(dir.path(), &png_registry(), SortOrder::default());
        let names = |v: &[PathBuf]| v.iter().map(|p| display_name(p)).collect::<Vec<_>>();
        assert_eq!(names(&dirs), ["sub2", "sub10"]);
        assert_eq!(names(&files), ["img2.png", "img10.png"]);
    }

    #[test]
    fn toggling_by_path_opens_a_loaded_subfolder() {
        let dir = TempDir::new();
        let sub = dir.mkdir("sub");
        let mut tree = tree_at(dir.path());
        tree.restore_open(&[dir.path().to_path_buf()], &png_registry());
        tree.toggle(&sub);
        assert_eq!(tree.open_paths(), [dir.path().to_path_buf(), sub]);
    }

    const MS: Duration = Duration::from_millis(1);

    fn how(mode: TreeMode, action: FolderAction) -> FolderClicks {
        FolderClicks {
            mode,
            action,
            double_click: 300 * MS,
        }
    }

    fn toggles(paths: &[&Path]) -> ClickEffect {
        ClickEffect {
            toggle: paths.iter().map(|p| p.to_path_buf()).collect(),
            load: None,
        }
    }

    fn loads(path: &Path) -> ClickEffect {
        ClickEffect {
            toggle: vec![],
            load: Some(path.to_path_buf()),
        }
    }

    #[test]
    fn normal_mode_toggles_on_every_click_and_never_loads() {
        let (a, t) = (Path::new("/a"), Instant::now());
        let h = how(TreeMode::Normal, FolderAction::Delayed);
        let mut c = ClickTracker::default();
        assert_eq!(c.click(h, a, false, t), toggles(&[a]));
        assert_eq!(c.click(h, a, true, t + MS), toggles(&[a]));
    }

    #[test]
    fn delayed_click_toggles_only_after_the_double_click_time() {
        let (a, t) = (Path::new("/a"), Instant::now());
        let h = how(TreeMode::AutoLoad, FolderAction::Delayed);
        let mut c = ClickTracker::default();
        assert_eq!(c.click(h, a, false, t), ClickEffect::default());
        assert_eq!(c.due(h, t + 200 * MS), None);
        assert_eq!(c.due(h, t + 301 * MS), Some(a.to_path_buf()));
        assert_eq!(c.due(h, t + 400 * MS), None, "toggled once");
    }

    #[test]
    fn delayed_double_click_loads_without_toggling() {
        let (a, t) = (Path::new("/a"), Instant::now());
        let h = how(TreeMode::AutoLoad, FolderAction::Delayed);
        let mut c = ClickTracker::default();
        c.click(h, a, false, t);
        assert_eq!(c.click(h, a, false, t + 250 * MS), loads(a));
        assert_eq!(c.due(h, t + 600 * MS), None);
    }

    #[test]
    fn delayed_clicks_slower_than_the_double_click_time_toggle_twice() {
        let (a, t) = (Path::new("/a"), Instant::now());
        let h = how(TreeMode::AutoLoad, FolderAction::Delayed);
        let mut c = ClickTracker::default();
        c.click(h, a, false, t);
        assert_eq!(c.due(h, t + 350 * MS), Some(a.to_path_buf()));
        assert_eq!(c.click(h, a, false, t + 400 * MS), ClickEffect::default());
        assert_eq!(c.due(h, t + 800 * MS), Some(a.to_path_buf()));
    }

    #[test]
    fn delayed_click_on_another_folder_toggles_the_first_at_once() {
        let (a, b, t) = (Path::new("/a"), Path::new("/b"), Instant::now());
        let h = how(TreeMode::AutoLoad, FolderAction::Delayed);
        let mut c = ClickTracker::default();
        c.click(h, a, false, t);
        assert_eq!(c.click(h, b, false, t + 100 * MS), toggles(&[a]));
        assert_eq!(c.due(h, t + 500 * MS), Some(b.to_path_buf()));
    }

    #[test]
    fn immediate_double_click_toggles_back_and_loads() {
        let (a, t) = (Path::new("/a"), Instant::now());
        let h = how(TreeMode::AutoLoad, FolderAction::Immediate);
        let mut c = ClickTracker::default();
        assert_eq!(c.click(h, a, false, t), toggles(&[a]));
        assert_eq!(
            c.click(h, a, false, t + 100 * MS),
            ClickEffect {
                toggle: vec![a.to_path_buf()],
                load: Some(a.to_path_buf()),
            }
        );
        assert_eq!(c.due(h, t + 600 * MS), None);
    }

    #[test]
    fn immediate_third_click_starts_over() {
        let (a, t) = (Path::new("/a"), Instant::now());
        let h = how(TreeMode::AutoLoad, FolderAction::Immediate);
        let mut c = ClickTracker::default();
        c.click(h, a, false, t);
        c.click(h, a, false, t + 100 * MS);
        assert_eq!(c.click(h, a, false, t + 200 * MS), toggles(&[a]));
    }

    #[test]
    fn shift_click_loads_and_plain_click_toggles_at_once() {
        let (a, t) = (Path::new("/a"), Instant::now());
        let h = how(TreeMode::AutoLoad, FolderAction::ShiftClick);
        let mut c = ClickTracker::default();
        assert_eq!(c.click(h, a, false, t), toggles(&[a]));
        assert_eq!(c.click(h, a, false, t + MS), toggles(&[a]));
        assert_eq!(c.click(h, a, true, t + 2 * MS), loads(a));
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
        let paths = vec![dir.path().to_path_buf(), a.clone(), b.clone()];
        tree.restore_open(&paths, &png_registry());
        assert_eq!(tree.open_paths(), paths);
        let b_node = &tree.roots[0].children.as_ref().unwrap()[0]
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
            tree.roots[0].children.is_none(),
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
