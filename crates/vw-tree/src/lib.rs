//! フォルダツリーの基底。どのアプリでも同じ部分だけを持つ。
//!
//! - ルート（渡されたフォルダ、ホーム、ドライブまたは `/`）
//! - フォルダの中身を初めて開いたときに読む（起動時にドライブ全体を走査しない）
//! - 隠しフォルダとゴミ箱を出さない
//! - ▶ と見出しを別の部品にした開閉（見出しに選択状態と右クリックメニューを付けられる）
//! - `path` までの親を開いて見せる、開いていたフォルダの保存と復元、中身の読み直し
//!
//! アプリ固有のもの（vw の ZIP・DICOM の検査・AI タグ、画像ファイルの一覧など）は
//! [`Hooks`] で足す。項目ごとの印は [`Node::ext`] に持たせる。
//! ツリーの中身を基底が決めないのは、ビューアは画像ファイルも読んで検査を組み立てるが、
//! フォルダを選ぶだけのツールはサブフォルダしか要らないため。
//!
//! ツリーを使ってフォルダやファイルを選ぶ窓は [`picker`]。

pub mod picker;

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::collapsing_header::CollapsingState;

/// ツリーの 1 項目。
pub struct Node<T> {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// 初めて開くまでは `None`。一度読んだら閉じても保持する。
    pub children: Option<Vec<Node<T>>>,
    /// 開いているか。egui の内部状態に任せず自分で持つのは、終了時に保存して次回復元するため。
    pub open: bool,
    /// 更新日時。更新日時順で並べるときだけ取る。
    pub modified: Option<SystemTime>,
    /// アプリが付ける印（vw なら ZIP かどうかなど）。基底は中身を見ない。
    pub ext: T,
}

impl<T: Default> Node<T> {
    /// フォルダの項目。`name` を省くとパスの末尾を表示名にする。
    pub fn dir(path: PathBuf, name: Option<String>, open: bool) -> Self {
        let name = name.unwrap_or_else(|| display_name(&path));
        Self {
            path,
            name,
            is_dir: true,
            children: None,
            open,
            modified: None,
            ext: T::default(),
        }
    }

    /// フォルダの直下の項目（[`Hooks::list_dir`] で作る）。
    pub fn entry(path: PathBuf, name: String, is_dir: bool, modified: Option<SystemTime>) -> Self {
        Self {
            path,
            name,
            is_dir,
            children: None,
            open: false,
            modified,
            ext: T::default(),
        }
    }
}

/// フォルダの中身を読む関数。[`FolderTree`] の開く・復元する・読み直す操作に渡す。
pub type Lister<'a, T> = &'a dyn Fn(&Path) -> Vec<Node<T>>;
/// 項目の並べ方。
pub type Compare<'a, T> = &'a dyn Fn(&Node<T>, &Node<T>) -> Ordering;

impl<T> Node<T> {
    /// `path` の項目。読み込み済みの子だけを辿る。
    pub fn find(&self, path: &Path) -> Option<&Node<T>> {
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
    pub fn open_towards(&mut self, path: &Path, list: Lister<T>) -> bool {
        if self.path == path {
            return true;
        }
        if !self.is_dir || !path.starts_with(&self.path) {
            return false;
        }
        self.open = true;
        let children = self.children.get_or_insert_with(|| list(&self.path));
        children.iter_mut().any(|c| c.open_towards(path, list))
    }

    /// 開いているフォルダのパス（親が開いているものだけ）。
    pub fn collect_open(&self, out: &mut Vec<PathBuf>) {
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
    pub fn restore_open(&mut self, paths: &[PathBuf], list: Lister<T>) {
        if !self.is_dir || !paths.contains(&self.path) {
            return;
        }
        self.open = true;
        let children = self.children.get_or_insert_with(|| list(&self.path));
        for child in children {
            child.restore_open(paths, list);
        }
    }

    /// 読み込み済みのフォルダ `dir` の中身を読み直す。前からある項目は残すので、
    /// サブフォルダの開閉状態と、読み込み済みの孫はそのまま保たれる。
    pub fn refresh(&mut self, dir: &Path, list: Lister<T>, cmp: Compare<T>) {
        if !dir.starts_with(&self.path) {
            return;
        }
        let Some(children) = &mut self.children else {
            return;
        };
        if self.path != dir {
            for child in children.iter_mut() {
                child.refresh(dir, list, cmp);
            }
            return;
        }
        let mut old: Vec<Node<T>> = std::mem::take(children);
        *children = list(dir)
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
        children.sort_by(cmp);
    }

    /// 読み込み済みの子を `cmp` で並べ直す。フォルダを読み直さないのは、開閉状態と
    /// まだ読んでいない孫の状態を保ったまま、設定の変更をすぐ見せるため。
    /// `prepare` は並べる前に各項目で呼ぶ（更新日時をまだ取っていない項目で取る、など）。
    pub fn resort(&mut self, prepare: &dyn Fn(&mut Node<T>), cmp: Compare<T>) {
        let Some(children) = &mut self.children else {
            return;
        };
        for c in children.iter_mut() {
            prepare(c);
        }
        children.sort_by(cmp);
        for c in children {
            c.resort(prepare, cmp);
        }
    }
}

/// アプリが足す振る舞い。既定は「サブフォルダだけを名前順に出す」。
pub trait Hooks<T> {
    /// 見出しの操作（右クリックメニューなど）で起きたこと。
    type Action;

    /// フォルダの直下の項目。ツリーに出さないもの（画像ファイルなど）を含めてよい。
    fn list_dir(&self, dir: &Path) -> Vec<Node<T>>;

    /// 項目をツリーに出すか。既定はフォルダだけ。
    fn shows(&self, node: &Node<T>) -> bool {
        node.is_dir
    }

    /// 見出しを選択状態で描くか（cd しているフォルダなど）。
    fn is_current(&self, _node: &Node<T>) -> bool {
        false
    }

    /// まだ読んでいないフォルダの下に、ツリーに出す項目がありそうか。
    /// `None` は分からない。分かるまで ▶ を出しておく。遅いドライブで中身のあるフォルダを
    /// 開けなくなるより、末端に ▶ が残るほうが害が小さいため。
    fn may_have_children(&mut self, _node: &Node<T>) -> Option<bool> {
        None
    }

    /// 読み込み済みの子の後に足して描くもの（DICOM の検査など）があるか。▶ を出すかに使う。
    fn has_extra_children(&mut self, _dir: &Path, _children: &[Node<T>]) -> bool {
        false
    }

    /// 読み込み済みの子の後に足して描く。
    fn extra_children(
        &mut self,
        _ui: &mut egui::Ui,
        _dir: &Path,
        _children: &[Node<T>],
        _out: &mut Vec<Self::Action>,
    ) {
    }

    /// フォルダの見出し。
    fn label(&mut self, ui: &mut egui::Ui, node: &Node<T>, is_current: bool) -> egui::Response {
        ui.selectable_label(is_current, node.name.as_str())
    }

    /// 見出しを描いた後（右クリックメニュー、ドロップの受け付けなど）。
    /// クリックとスクロールは基底が扱う。
    fn after_label(
        &mut self,
        _label: &egui::Response,
        _node: &Node<T>,
        _out: &mut Vec<Self::Action>,
    ) {
    }

    /// フォルダでない項目（vw の ZIP など）。既定は ▶ なしの見出しで、クリックは基底が扱う。
    fn leaf(
        &mut self,
        ui: &mut egui::Ui,
        node: &Node<T>,
        _out: &mut Vec<Self::Action>,
    ) -> egui::Response {
        without_toggle(ui, |ui| ui.selectable_label(false, node.name.as_str()))
    }
}

/// サブフォルダだけを名前順に出す、いちばん軽い [`Hooks`]。フォルダを選ぶだけの画面向け。
pub struct Plain<'a> {
    pub current: Option<&'a Path>,
}

impl Hooks<()> for Plain<'_> {
    type Action = ();

    fn list_dir(&self, dir: &Path) -> Vec<Node<()>> {
        list_subdirs(dir)
    }

    fn is_current(&self, node: &Node<()>) -> bool {
        self.current == Some(node.path.as_path())
    }
}

/// ツリーを描いた結果。
pub struct Drawn<A> {
    /// 見出しをクリックしたフォルダ（または ZIP などの葉）。
    pub clicked: Option<PathBuf>,
    /// 見出しをダブルクリックした項目。選ぶ窓で「決定」に使う。
    pub double_clicked: Option<PathBuf>,
    pub actions: Vec<A>,
}

/// 描くときに全項目で共通の情報。
struct View<'a> {
    reveal: Option<&'a Path>,
    click_expands: bool,
}

impl<T> Node<T> {
    fn ui<H: Hooks<T>>(
        &mut self,
        ui: &mut egui::Ui,
        view: &View,
        hooks: &mut H,
        out: &mut Drawn<H::Action>,
    ) {
        if !self.is_dir {
            let r = hooks.leaf(ui, self, &mut out.actions);
            self.after(&r, view, out);
            return;
        }
        let is_current = hooks.is_current(self);
        if !self.expandable(hooks) {
            let label = without_toggle(ui, |ui| hooks.label(ui, self, is_current));
            self.after(&label, view, out);
            hooks.after_label(&label, self, &mut out.actions);
            return;
        }
        // CollapsingHeader を使わないのは、見出しに選択状態を出せず、右クリックメニューも
        // 付けられないため。開閉の ▶ と見出しの文字を別の部品にして組み立てる。
        let id = ui.make_persistent_id(&self.path);
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, self.open);
        state.set_open(self.open);
        let (toggle, header, _) = state
            .show_header(ui, |ui| hooks.label(ui, self, is_current))
            .body(|ui| {
                let children = self
                    .children
                    .get_or_insert_with(|| hooks.list_dir(&self.path));
                for child in children.iter_mut() {
                    if hooks.shows(child) {
                        child.ui(ui, view, hooks, out);
                    }
                }
                hooks.extra_children(ui, &self.path, children, &mut out.actions);
            });
        if toggle.clicked() {
            self.open = !self.open;
        } else if view.click_expands && header.inner.clicked() {
            // 開いているフォルダをもう一度押しても閉じない。エクスプローラも閉じず、
            // cd し直すたびに開閉が切り替わると見ていた中身が消えてしまうため。
            self.open = true;
        }
        self.after(&header.inner, view, out);
        hooks.after_label(&header.inner, self, &mut out.actions);
    }

    /// 見出しのクリックとスクロール。
    fn after<A>(&self, label: &egui::Response, view: &View, out: &mut Drawn<A>) {
        if label.double_clicked() {
            out.double_clicked = Some(self.path.clone());
        }
        if label.clicked() {
            out.clicked = Some(self.path.clone());
        }
        if view.reveal == Some(self.path.as_path()) {
            label.scroll_to_me(Some(egui::Align::Center));
        }
    }

    /// 開いたときに下へ出す項目があるか。▶ を出すかを決める。
    /// 読み込み済みなら中身で決める。まだなら [`Hooks::may_have_children`] に聞く。
    fn expandable<H: Hooks<T>>(&self, hooks: &mut H) -> bool {
        let Some(children) = &self.children else {
            return hooks.may_have_children(self).unwrap_or(true);
        };
        children.iter().any(|c| hooks.shows(c)) || hooks.has_extra_children(&self.path, children)
    }
}

/// ▶ を出さない項目。▶ の幅だけ空けて、▶ のある兄弟と名前の位置を揃える。
pub fn without_toggle(
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

/// ツリー全体。ルートは、渡されたフォルダ、ホーム、ドライブ（Windows）または / の順。
pub struct FolderTree<T> {
    pub roots: Vec<Node<T>>,
    /// 見出しのクリックでフォルダも開くか。
    pub click_expands: bool,
    /// 次に描くときに、見える位置までスクロールするフォルダ。
    reveal: Option<PathBuf>,
}

impl<T: Default> FolderTree<T> {
    pub fn new(extra_root: Option<PathBuf>) -> Self {
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
        Self::with_roots(roots)
    }
}

impl<T> FolderTree<T> {
    pub fn with_roots(roots: Vec<Node<T>>) -> Self {
        Self {
            roots,
            click_expands: false,
            reveal: None,
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
    pub fn restore_open(&mut self, paths: &[PathBuf], list: Lister<T>) {
        for root in &mut self.roots {
            root.restore_open(paths, list);
        }
    }

    /// フォルダ `dir` の中身を読み直す（フォルダの監視で変化を知ったとき）。
    /// 同じフォルダが Home と / の両方の下に出ていることがあるので、すべてのルートを辿る。
    pub fn refresh(&mut self, dir: &Path, list: Lister<T>, cmp: Compare<T>) {
        for root in &mut self.roots {
            root.refresh(dir, list, cmp);
        }
    }

    /// 読み込み済みの項目を並べ直す（[`Node::resort`]）。
    pub fn resort(&mut self, prepare: &dyn Fn(&mut Node<T>), cmp: Compare<T>) {
        for root in &mut self.roots {
            root.resort(prepare, cmp);
        }
    }

    /// `path` が見えるよう親フォルダを開き、次に描くときにそこまでスクロールする。
    ///
    /// 含むルートのうち先頭の 1 つだけで開く。Home の下のフォルダを / の下でも開くと、
    /// 同じフォルダが 2 か所に出てどちらを見ればよいか分からなくなるため。
    pub fn reveal(&mut self, path: &Path, list: Lister<T>) {
        if let Some(root) = self.roots.iter_mut().find(|r| path.starts_with(&r.path)) {
            root.open_towards(path, list);
        }
        self.reveal = Some(path.to_path_buf());
    }

    /// ルートを先頭に足す（展開した ZIP など）。
    pub fn add_root_first(&mut self, node: Node<T>) {
        self.roots.insert(0, node);
    }

    pub fn remove_root(&mut self, path: &Path) {
        self.roots.retain(|r| r.path != path);
    }

    pub fn ui<H: Hooks<T>>(&mut self, ui: &mut egui::Ui, hooks: &mut H) -> Drawn<H::Action> {
        let view = View {
            reveal: self.reveal.as_deref(),
            click_expands: self.click_expands,
        };
        let mut out = Drawn {
            clicked: None,
            double_clicked: None,
            actions: Vec::new(),
        };
        for root in &mut self.roots {
            root.ui(ui, &view, hooks, &mut out);
        }
        self.reveal = None;
        out
    }
}

/// ツリーに出さない名前。ドットで始まる隠しファイル・フォルダと、ゴミ箱。
pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.') || is_recycle_bin(name)
}

/// Windows がドライブごとに作るゴミ箱のフォルダ。中身は消したファイルの残骸で、
/// 名前も元と違う（$R1A2B3C.png など）ため、見せても探しても役に立たない。
/// Linux のゴミ箱（.Trash-1000 や ~/.local/share/Trash）はドットで始まるので隠しとして除かれる。
pub fn is_recycle_bin(name: &str) -> bool {
    ["$RECYCLE.BIN", "RECYCLER", "RECYCLED"]
        .iter()
        .any(|bin| name.eq_ignore_ascii_case(bin))
}

/// 種別は DirEntry::file_type() で取る。path.is_dir() は項目ごとに stat を発行し、
/// 数万件のフォルダで目に見えて遅くなる。シンボリックリンクのときだけ辿って確かめる。
pub fn entry_is_dir(e: &std::fs::DirEntry) -> bool {
    match e.file_type() {
        Ok(t) if t.is_symlink() => e.path().is_dir(),
        Ok(t) => t.is_dir(),
        Err(_) => false,
    }
}

/// フォルダの直下のサブフォルダを名前順（大文字小文字を区別しない）で返す。
/// 読めないフォルダ（権限がないなど）は空として扱い、ツリー全体の表示は止めない。
pub fn list_subdirs<T: Default>(dir: &Path) -> Vec<Node<T>> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut nodes: Vec<Node<T>> = rd
        .flatten()
        .filter(entry_is_dir)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (!is_hidden_name(&name)).then(|| Node::entry(e.path(), name, true, None))
        })
        .collect();
    nodes.sort_by(|a, b| by_name(a, b));
    nodes
}

/// 名前順（大文字小文字を区別しない）。フォルダが先。
pub fn by_name<T>(a: &Node<T>, b: &Node<T>) -> Ordering {
    b.is_dir
        .cmp(&a.is_dir)
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.name.cmp(&b.name))
}

/// パスの末尾。ルート（`/` や `C:\\`）のように末尾がないときはパス全体。
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// ホームフォルダ。`dirs` クレートを使わないのは、環境変数 1 つで足りるため。
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};

    /// テストごとの一時フォルダ。終わったら消す。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("vw-tree-test-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn mkdir(&self, name: &str) -> PathBuf {
            let p = self.0.join(name);
            std::fs::create_dir_all(&p).unwrap();
            p
        }
        fn touch(&self, name: &str) -> PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, b"").unwrap();
            p
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(nodes: &[Node<()>]) -> Vec<&str> {
        nodes.iter().map(|n| n.name.as_str()).collect()
    }

    fn list(dir: &Path) -> Vec<Node<()>> {
        list_subdirs(dir)
    }

    #[test]
    fn lists_only_subfolders_by_name_ignoring_case_and_hidden_ones() {
        let dir = TempDir::new();
        dir.mkdir("Zeta");
        dir.mkdir("alpha");
        dir.mkdir(".git");
        dir.mkdir("$RECYCLE.BIN");
        dir.touch("photo.png");
        assert_eq!(names(&list(dir.path())), ["alpha", "Zeta"]);
    }

    #[test]
    fn unreadable_folder_is_empty() {
        let dir = TempDir::new();
        assert!(list(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn display_name_falls_back_to_whole_path_for_roots() {
        assert_eq!(display_name(Path::new("/tmp/abc")), "abc");
        assert_eq!(display_name(Path::new("/")), "/");
    }

    #[test]
    fn extra_root_comes_first_and_is_open() {
        let dir = TempDir::new();
        let tree: FolderTree<()> = FolderTree::new(Some(dir.path().to_path_buf()));
        assert_eq!(tree.roots[0].path, dir.path());
        assert!(tree.roots[0].open);
        assert!(tree.roots.len() >= 2, "home or filesystem roots follow");
    }

    #[test]
    fn revealing_a_folder_opens_its_parents_but_not_the_folder() {
        let dir = TempDir::new();
        let sub = dir.mkdir("a/b");
        let mut tree =
            FolderTree::with_roots(vec![Node::<()>::dir(dir.path().to_path_buf(), None, false)]);
        tree.reveal(&sub, &list);
        assert!(tree.roots[0].open);
        assert!(tree.roots[0]
            .find(&dir.path().join("a"))
            .is_some_and(|n| n.open));
        assert!(tree.roots[0].find(&sub).is_some_and(|n| !n.open));
    }

    #[test]
    fn revealing_opens_only_the_first_root_that_holds_the_folder() {
        let dir = TempDir::new();
        let sub = dir.mkdir("a");
        let mut tree = FolderTree::with_roots(vec![
            Node::<()>::dir(dir.path().to_path_buf(), None, false),
            Node::<()>::dir(dir.path().parent().unwrap().to_path_buf(), None, false),
        ]);
        tree.reveal(&sub, &list);
        assert!(tree.roots[0].open);
        assert!(!tree.roots[1].open);
    }

    #[test]
    fn open_folders_are_saved_and_restored_with_their_contents() {
        let dir = TempDir::new();
        let b = dir.mkdir("a/b");
        dir.mkdir("a/b/c");
        let mut tree =
            FolderTree::with_roots(vec![Node::<()>::dir(dir.path().to_path_buf(), None, false)]);
        assert!(
            tree.open_paths().is_empty(),
            "closed tree has no open paths"
        );
        tree.restore_open(
            &[dir.path().to_path_buf(), dir.path().join("a"), b.clone()],
            &list,
        );
        assert_eq!(
            tree.open_paths(),
            [dir.path().to_path_buf(), dir.path().join("a"), b.clone()]
        );
        let b_node = tree.roots[0].find(&b).unwrap();
        assert_eq!(names(b_node.children.as_ref().unwrap()), ["c"]);
    }

    #[test]
    fn folder_inside_a_closed_parent_is_not_restored() {
        let dir = TempDir::new();
        let b = dir.mkdir("a/b");
        let mut tree =
            FolderTree::with_roots(vec![Node::<()>::dir(dir.path().to_path_buf(), None, false)]);
        tree.restore_open(&[b], &list);
        assert!(
            tree.roots[0].children.is_none(),
            "the root was not in the list, so nothing is read"
        );
    }

    #[test]
    fn refresh_shows_new_folders_and_keeps_open_subfolders() {
        let dir = TempDir::new();
        let sub = dir.mkdir("sub");
        let mut tree =
            FolderTree::with_roots(vec![Node::<()>::dir(dir.path().to_path_buf(), None, false)]);
        tree.restore_open(&[dir.path().to_path_buf(), sub.clone()], &list);
        dir.mkdir("added");
        tree.refresh(dir.path(), &list, &by_name);
        let children = tree.roots[0].children.as_ref().unwrap();
        assert_eq!(names(children), ["added", "sub"]);
        assert!(children[1].open && children[1].children.is_some());
    }

    #[test]
    fn refresh_skips_folders_never_opened() {
        let dir = TempDir::new();
        let mut tree =
            FolderTree::with_roots(vec![Node::<()>::dir(dir.path().to_path_buf(), None, false)]);
        tree.refresh(dir.path(), &list, &by_name);
        assert!(tree.roots[0].children.is_none());
    }

    #[test]
    fn recycle_bins_are_hidden_in_any_letter_case() {
        assert!(is_hidden_name("$RECYCLE.BIN"));
        assert!(is_hidden_name("$recycle.bin"));
        assert!(is_hidden_name("Recycler"));
        assert!(is_hidden_name(".Trash-1000"));
        assert!(!is_hidden_name("Pictures"));
    }
}
