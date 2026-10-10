//! フォルダツリー。フォルダと、対応形式の画像ファイルだけを表示する。
//!
//! フォルダの中身は初めて開いたときに読む。起動時にドライブ全体を走査すると
//! 起動が遅くなるうえ、ネットワークドライブで固まることがあるため。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui::{self, collapsing_header::CollapsingState};
use vw_core::Registry;

use crate::i18n::tr;
use crate::settings::Language;
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

/// ツリーを描くときに全項目で共通の情報。
struct View<'a> {
    registry: &'a Registry,
    selected: Option<&'a Path>,
    lang: Language,
    order: SortOrder,
}

/// ツリーでファイルがクリックされたときの要求。
pub enum TreeAction {
    /// アクティブなスロットに表示する（通常のクリック）。
    Open(PathBuf),
    /// スロットを追加して表示する（Ctrl / Cmd + クリック）。
    Add(PathBuf),
    /// フォルダを選んだ（クリック。開閉もする）。
    Select(PathBuf),
    /// フルパスをコピーする（右クリックメニュー）。
    Copy(PathBuf),
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

    fn ui(&mut self, ui: &mut egui::Ui, view: &View, out: &mut Option<TreeAction>) {
        let lang = view.lang;
        let is_selected = view.selected == Some(self.path.as_path());
        if !self.is_dir {
            let r = ui.selectable_label(is_selected, &self.name);
            if r.clicked() {
                *out = Some(if ui.input(|i| i.modifiers.command) {
                    TreeAction::Add(self.path.clone())
                } else {
                    TreeAction::Open(self.path.clone())
                });
            }
            copy_menu(&r, &self.path, lang, out);
            return;
        }
        // CollapsingHeader を使わないのは、見出しに選択状態を出せず、右クリックメニューも
        // 付けられないため。開閉の ▶ と見出しの文字を別の部品にして組み立てる。
        let id = ui.make_persistent_id(&self.path);
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, self.open);
        state.set_open(self.open);
        let (toggle, header, _) = state
            .show_header(ui, |ui| ui.selectable_label(is_selected, &self.name))
            .body(|ui| {
                let children = self
                    .children
                    .get_or_insert_with(|| list_dir(&self.path, view.registry, view.order));
                if children.is_empty() {
                    ui.weak("(empty)");
                }
                for child in children {
                    child.ui(ui, view, out);
                }
            });
        let label = header.inner;
        if toggle.clicked() || label.clicked() {
            self.open = !self.open;
        }
        if label.clicked() {
            *out = Some(TreeAction::Select(self.path.clone()));
        }
        copy_menu(&label, &self.path, lang, out);
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
        Self { roots, order }
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

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        registry: &Registry,
        selected: Option<&Path>,
        lang: Language,
    ) -> Option<TreeAction> {
        let view = View {
            registry,
            selected,
            lang,
            order: self.order,
        };
        let mut out = None;
        for root in &mut self.roots {
            root.ui(ui, &view, &mut out);
        }
        out
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
            if !is_dir && !registry.supports_path(&path) {
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
                .filter(|p| registry.supports_path(p) && p.is_file())
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

    fn tree_at(root: &Path) -> FolderTree {
        FolderTree {
            roots: vec![Node::dir(root.to_path_buf(), None, false)],
            order: SortOrder::default(),
        }
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
