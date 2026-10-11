//! フォルダツリー。フォルダ（ZIP や DICOM の検査・シリーズといった仮想のフォルダを含む）だけを
//! 表示する。ファイルはツリーに出さず、クリックで cd したフォルダの中身としてスロットに並べる。
//!
//! フォルダの中身は初めて開いたときに読む。起動時にドライブ全体を走査すると
//! 起動が遅くなるうえ、ネットワークドライブで固まることがあるため。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::SystemTime;

use eframe::egui::{self, collapsing_header::CollapsingState};
use vw_core::Registry;

use crate::i18n::tr;
use crate::series::{Scan, Series, SeriesIndex, Study};
use crate::settings::Language;
use crate::sort::{self, SortItem, SortKey, SortOrder};

/// ツリーの 1 項目（フォルダまたはファイル）。
pub struct Node {
    pub path: PathBuf,
    name: String,
    is_dir: bool,
    /// フォルダを初めて開くまでは `None`。一度読んだら閉じても保持する。
    /// ファイルも持っておくのは、DICOM のシリーズを作るのにフォルダの中のファイルが要るため。
    children: Option<Vec<Node>>,
    /// 開いているか。egui の内部状態に任せず自分で持つのは、終了時に保存して次回復元するため。
    open: bool,
    /// 更新日時。更新日時順で並べるときだけ取る（[`list_dir`]）。
    modified: Option<SystemTime>,
    /// ZIP ファイル。クリックで展開して別のルートとして開く（[`crate::archive`]）。
    archive: bool,
    /// ZIP を展開したルート。右クリックでアンマウントできる。
    mounted: bool,
}

/// ツリーを描くときに全項目で共通の情報。
struct View<'a> {
    registry: &'a Registry,
    /// cd しているフォルダ。ハイライトする。
    current: Option<&'a Path>,
    /// アクティブなスロットの画像。それを含むシリーズをハイライトする。
    active_image: Option<&'a Path>,
    /// 見える位置までスクロールするフォルダ。
    reveal: Option<&'a Path>,
    lang: Language,
    order: SortOrder,
    series: &'a SeriesIndex,
    /// DICOM の拡張子。
    dicom_extensions: &'a [&'a str],
    /// まだ読んでいないフォルダの下に項目があるか（[`Peeker`]）。
    peeks: &'a HashMap<PathBuf, Peek>,
}

impl View<'_> {
    /// DICOM かもしれないファイル。拡張子が他のプラグインのもの（PNG など）でなければ、
    /// ヘッダを読んで確かめる。
    fn may_be_dicom(&self, path: &Path) -> bool {
        may_be_dicom(self.registry, self.dicom_extensions, path)
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

/// ツリーを描いた結果。
#[derive(Default)]
struct Out {
    action: Option<TreeAction>,
    /// ヘッダを読み始めるフォルダと、その中の DICOM らしいファイル。
    scans: Vec<(PathBuf, Vec<PathBuf>)>,
    /// 下に項目があるか先読みしてほしいフォルダ。
    peeks: Vec<PathBuf>,
}

/// ツリーの操作の要求。
pub enum TreeAction {
    /// フルパスをコピーする（右クリックメニュー）。
    Copy(PathBuf),
    /// フォルダへ移る（中身をスロットに並べる）。
    Cd(PathBuf),
    /// ZIP を展開して別のルートとして開く。
    Mount(PathBuf),
    /// 展開した ZIP を閉じる。展開先のフォルダを渡す。
    Unmount(PathBuf),
    /// DICOM のシリーズを出す。そのシリーズのあるフォルダへ移り、シリーズのスロットを選ぶ。
    Series {
        folder: PathBuf,
        files: Vec<PathBuf>,
    },
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
            archive: false,
            mounted: false,
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, view: &View, out: &mut Out) {
        let lang = view.lang;
        let is_current = view.current == Some(self.path.as_path());
        if self.archive {
            let text = format!("{} {}", egui_phosphor::regular::FILE_ZIP, self.name);
            let r = without_toggle(ui, |ui| ui.selectable_label(false, text));
            if r.clicked() {
                out.action = Some(TreeAction::Mount(self.path.clone()));
            }
            copy_menu(&r, &self.path, lang, &mut out.action);
            return;
        }
        if !self.shows_children(view, out) {
            let label = without_toggle(ui, |ui| self.label(ui, is_current));
            self.label_actions(&label, view, out);
            return;
        }
        // CollapsingHeader を使わないのは、見出しに選択状態を出せず、右クリックメニューも
        // 付けられないため。開閉の ▶ と見出しの文字を別の部品にして組み立てる。
        let id = ui.make_persistent_id(&self.path);
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, self.open);
        state.set_open(self.open);
        let (toggle, header, _) =
            state
                .show_header(ui, |ui| self.label(ui, is_current))
                .body(|ui| {
                    let children = self
                        .children
                        .get_or_insert_with(|| list_dir(&self.path, view.registry, view.order));
                    let studies = dicom_studies(&self.path, children, view, out);
                    for child in children.iter_mut().filter(|c| c.is_dir || c.archive) {
                        child.ui(ui, view, out);
                    }
                    for study in &studies {
                        study_ui(ui, &self.path, study, view, out);
                    }
                    if studies.is_empty() && matches!(view.series.scan(&self.path), Scan::Reading) {
                        ui.weak(tr(lang, "Reading DICOM headers…"));
                    }
                });
        if toggle.clicked() {
            self.open = !self.open;
        }
        self.label_actions(&header.inner, view, out);
    }

    fn label(&self, ui: &mut egui::Ui, is_current: bool) -> egui::Response {
        if self.mounted {
            let text = format!("{} {}", egui_phosphor::regular::FILE_ZIP, self.name);
            ui.selectable_label(is_current, text)
        } else {
            ui.selectable_label(is_current, self.name.as_str())
        }
    }

    /// 見出しのクリック（cd）・スクロール・右クリックメニュー。
    fn label_actions(&self, label: &egui::Response, view: &View, out: &mut Out) {
        let lang = view.lang;
        if label.clicked() {
            out.action = Some(TreeAction::Cd(self.path.clone()));
        }
        if view.reveal == Some(self.path.as_path()) {
            label.scroll_to_me(Some(egui::Align::Center));
        }
        if self.mounted {
            label.context_menu(|ui| {
                if ui.button(tr(lang, "Copy")).clicked() {
                    out.action = Some(TreeAction::Copy(self.path.clone()));
                    ui.close();
                }
                if ui.button(tr(lang, "Close ZIP")).clicked() {
                    out.action = Some(TreeAction::Unmount(self.path.clone()));
                    ui.close();
                }
            });
        } else {
            copy_menu(label, &self.path, lang, &mut out.action);
        }
    }

    /// 開いたときに下へ出す項目（サブフォルダ・ZIP・DICOM の検査）があるか。▶ を出すかを決める。
    ///
    /// 読み込み済みなら中身で決める。まだなら先読みの結果を使い、分かるまでは ▶ を出しておく。
    /// 分からないうちに ▶ を消さないのは、遅いドライブで中身のあるフォルダを開けなくなるより、
    /// 末端に ▶ が残るほうが害が小さいため。
    fn shows_children(&self, view: &View, out: &mut Out) -> bool {
        let Some(children) = &self.children else {
            return match view.peeks.get(&self.path) {
                Some(Peek::Done(has)) => *has,
                Some(Peek::Asked) => true,
                None => {
                    out.peeks.push(self.path.clone());
                    true
                }
            };
        };
        children.iter().any(|c| c.is_dir || c.archive)
            || match view.series.scan(&self.path) {
                Scan::Done(studies) => !studies.is_empty(),
                Scan::Reading => true,
                Scan::NotStarted => children
                    .iter()
                    .any(|c| !c.is_dir && view.may_be_dicom(&c.path)),
            }
    }

    /// `path` の項目。読み込み済みの子だけを辿る。
    #[cfg(test)]
    fn find(&self, path: &Path) -> Option<&Node> {
        if self.path == path {
            return Some(self);
        }
        if !path.starts_with(&self.path) {
            return None;
        }
        self.children.iter().flatten().find_map(|c| c.find(path))
    }

    /// `path` までの親フォルダを開く（中身を読みながら）。`path` 自体は開閉を変えない。
    /// `path` が見つかったら true。
    fn open_towards(&mut self, path: &Path, registry: &Registry, order: SortOrder) -> bool {
        if self.path == path {
            return true;
        }
        if !self.is_dir || !path.starts_with(&self.path) {
            return false;
        }
        self.open = true;
        let children = self
            .children
            .get_or_insert_with(|| list_dir(&self.path, registry, order));
        children
            .iter_mut()
            .any(|c| c.open_towards(path, registry, order))
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
        .active_image
        .is_some_and(|p| series.files.iter().any(|f| f == p));
    let r = ui.selectable_label(selected, series.title());
    if r.clicked() {
        out.action = Some(TreeAction::Series {
            folder: dir.to_path_buf(),
            files: series.files.clone(),
        });
    }
}

/// ▶ を出さない項目。▶ の幅だけ空けて、▶ のある兄弟と名前の位置を揃える。
fn without_toggle(
    ui: &mut egui::Ui,
    label: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    ui.horizontal(|ui| {
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.allocate_space(egui::vec2(ui.spacing().indent, ui.spacing().icon_width));
        ui.spacing_mut().item_spacing = spacing;
        label(ui)
    })
    .inner
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

/// まだ読んでいないフォルダの下に項目があるかの先読みの状態。
#[derive(Clone, Copy)]
enum Peek {
    Asked,
    Done(bool),
}

/// フォルダの下に項目があるかを、裏のスレッドで 1 段だけ先読みする（[`has_tree_children`]）。
///
/// スレッドを 1 本にして順に調べるのは、表示中のフォルダが多いときに、ネットワークドライブへ
/// 同時にたくさん問い合わせて詰まらせないため。
struct Peeker {
    requests: Sender<PathBuf>,
    results: Receiver<(PathBuf, bool)>,
    known: HashMap<PathBuf, Peek>,
}

impl Peeker {
    fn start(registry: Registry, wake: impl Fn() + Send + 'static) -> Self {
        let (requests, todo) = std::sync::mpsc::channel::<PathBuf>();
        let (done, results) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for dir in todo {
                let has = has_tree_children(&dir, &registry);
                if done.send((dir, has)).is_err() {
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
        for (dir, has) in self.results.try_iter() {
            self.known.insert(dir, Peek::Done(has));
        }
    }

    fn ask(&mut self, dirs: Vec<PathBuf>) {
        for dir in dirs {
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
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = e.path();
        if entry_is_dir(&e) || crate::archive::is_zip(&path) {
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

/// 種別は DirEntry::file_type() で取る。path.is_dir() は項目ごとに stat を発行し、
/// 数万件のフォルダで目に見えて遅くなる。シンボリックリンクのときだけ辿って確かめる。
fn entry_is_dir(e: &std::fs::DirEntry) -> bool {
    match e.file_type() {
        Ok(t) if t.is_symlink() => e.path().is_dir(),
        Ok(t) => t.is_dir(),
        Err(_) => false,
    }
}

/// ツリーで描くときに外から渡すもの。
pub struct Context<'a> {
    pub registry: &'a Registry,
    pub series: &'a SeriesIndex,
    pub current: Option<&'a Path>,
    pub active_image: Option<&'a Path>,
    pub lang: Language,
}

/// ツリーを描いた結果。
pub struct Drawn {
    pub action: Option<TreeAction>,
    /// ヘッダを読み始めてほしいフォルダと、その中の DICOM らしいファイル。
    pub scans: Vec<(PathBuf, Vec<PathBuf>)>,
}

/// ツリー全体。ルートは、起動時に渡されたフォルダ、ホーム、ドライブ（Windows）または / の順。
pub struct FolderTree {
    pub roots: Vec<Node>,
    order: SortOrder,
    /// 次に描くときに、見える位置までスクロールするフォルダ。
    reveal: Option<PathBuf>,
    /// 初めて描くときに始める。スレッドを起こすのに egui の Context が要るため。
    peeker: Option<Peeker>,
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
            reveal: None,
            peeker: None,
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
        self.forget_peek(dir);
        for root in &mut self.roots {
            root.refresh(dir, registry, self.order);
        }
    }

    /// `dir` の先読みの結果を捨て、次に描くときに調べ直す。
    fn forget_peek(&mut self, dir: &Path) {
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
    ///
    /// 含むルートのうち先頭の 1 つだけで開く。Home の下のフォルダを / の下でも開くと、
    /// 同じフォルダが 2 か所に出てどちらを見ればよいか分からなくなるため。
    pub fn reveal(&mut self, path: &Path, registry: &Registry) {
        let order = self.order;
        if let Some(root) = self.roots.iter_mut().find(|r| path.starts_with(&r.path)) {
            root.open_towards(path, registry, order);
        }
        self.reveal = Some(path.to_path_buf());
    }

    /// 展開した ZIP をツリーの先頭にルートとして開く。
    pub fn add_mounted_root(&mut self, dir: PathBuf, name: String) {
        let mut node = Node::dir(dir, Some(name), true);
        node.mounted = true;
        self.roots.insert(0, node);
    }

    pub fn remove_root(&mut self, dir: &Path) {
        self.roots.retain(|r| r.path != dir);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: Context) -> Drawn {
        let peeker = self.peeker.get_or_insert_with(|| {
            let wake = ui.ctx().clone();
            Peeker::start(cx.registry.clone(), move || wake.request_repaint())
        });
        peeker.receive();
        let view = View {
            registry: cx.registry,
            current: cx.current,
            active_image: cx.active_image,
            reveal: self.reveal.as_deref(),
            lang: cx.lang,
            order: self.order,
            series: cx.series,
            dicom_extensions: dicom_extensions(cx.registry),
            peeks: &peeker.known,
        };
        let mut out = Out::default();
        for root in &mut self.roots {
            root.ui(ui, &view, &mut out);
        }
        peeker.ask(out.peeks);
        self.reveal = None;
        Drawn {
            action: out.action,
            scans: out.scans,
        }
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
            let is_dir = entry_is_dir(&e);
            let archive = !is_dir && crate::archive::is_zip(&path);
            if !is_dir && !archive && !registry.supports_file(&path) {
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
                archive,
                mounted: false,
            })
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
        } else if n.archive {
            out.archives.push(n.path);
        } else {
            out.images.push(n.path);
        }
    }
    out
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
                    archive: false,
                    mounted: false,
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
    use crate::test_util::{all_registry, png_registry, TempDir};

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
            reveal: None,
            peeker: None,
        }
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
        assert!(tree.roots[0].find(&sub).is_some_and(|n| !n.open));
    }

    #[test]
    fn revealing_opens_only_the_first_root_that_holds_the_folder() {
        let dir = TempDir::new();
        let sub = dir.mkdir("a");
        let mut tree = FolderTree {
            roots: vec![
                Node::dir(dir.path().to_path_buf(), None, false),
                Node::dir(dir.path().parent().unwrap().to_path_buf(), None, false),
            ],
            order: SortOrder::default(),
            reveal: None,
            peeker: None,
        };
        tree.reveal(&sub, &png_registry());
        assert!(tree.roots[0].open);
        assert!(!tree.roots[1].open);
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
