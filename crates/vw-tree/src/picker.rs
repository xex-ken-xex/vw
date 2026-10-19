//! ツリーでフォルダやファイルを選ぶ窓。OS のダイアログの代わり。
//!
//! OS のダイアログ（rfd など）を使わないのは、Linux ではデスクトップポータル経由の
//! 依存（D-Bus、非同期ランタイム）が丸ごと付いてきて重く、見た目もアプリと揃わないため。
//! ツリーはアプリの左の欄と同じものなので、使う人が覚えることも増えない。

use std::path::{Path, PathBuf};

use crate::{by_name, entry_is_dir, is_hidden_name, FolderTree, Node, Plain};

/// 何を選ぶか。
pub enum Mode {
    Folder,
    /// 拡張子（小文字、ドットなし）で絞ったファイルを開く。
    OpenFile {
        extensions: Vec<String>,
    },
    /// フォルダを選び、名前を入れて保存する。
    SaveFile {
        extensions: Vec<String>,
        name: String,
    },
}

/// 窓の文言。アプリの言語に合わせて渡す。
pub struct Labels {
    pub choose: String,
    pub cancel: String,
    pub name: String,
    /// 選んでいるフォルダがないときの表示。
    pub nothing: String,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            choose: "OK".into(),
            cancel: "Cancel".into(),
            name: "Name".into(),
            nothing: "(none)".into(),
        }
    }
}

/// 窓を描いた結果。
pub enum Outcome {
    /// まだ選んでいる。
    Open,
    Chosen(PathBuf),
    Cancelled,
}

pub struct Picker {
    title: String,
    mode: Mode,
    labels: Labels,
    tree: FolderTree<()>,
    /// ツリーで選んでいるフォルダ。
    folder: Option<PathBuf>,
    /// `folder` の直下の、拡張子の合うファイル（ファイルを選ぶときだけ）。
    files: Vec<PathBuf>,
    file: Option<PathBuf>,
}

impl Picker {
    /// `start` があれば、そこまで開いて見せる。
    pub fn new(title: impl Into<String>, mode: Mode, start: Option<&Path>, labels: Labels) -> Self {
        let mut tree = FolderTree::new(None);
        let mut me = Self {
            title: title.into(),
            mode,
            labels,
            tree: FolderTree::with_roots(Vec::new()),
            folder: None,
            files: Vec::new(),
            file: None,
        };
        // 欄が空なら作業フォルダから。何も開いていない木を見せるより、探す手がかりになる
        let start = start
            .filter(|p| p.is_dir())
            .map(Path::to_path_buf)
            .or_else(|| {
                start
                    .and_then(Path::parent)
                    .filter(|p| p.is_dir())
                    .map(Path::to_path_buf)
            })
            .or_else(|| std::env::current_dir().ok());
        if let Some(start) = start {
            tree.reveal(&start, &crate::list_subdirs);
            me.select_folder(start);
        }
        me.tree = tree;
        me
    }

    pub fn folder(title: impl Into<String>, start: Option<&Path>, labels: Labels) -> Self {
        Self::new(title, Mode::Folder, start, labels)
    }

    fn select_folder(&mut self, dir: PathBuf) {
        self.files = match &self.mode {
            Mode::Folder => Vec::new(),
            Mode::OpenFile { extensions } | Mode::SaveFile { extensions, .. } => {
                list_files(&dir, extensions)
            }
        };
        self.file = None;
        self.folder = Some(dir);
    }

    /// いま選んでいるもの（決定したときに返すパス）。
    fn choice(&self) -> Option<PathBuf> {
        match &self.mode {
            Mode::Folder => self.folder.clone(),
            Mode::OpenFile { .. } => self.file.clone(),
            Mode::SaveFile { extensions, name } => {
                let name = name.trim();
                if name.is_empty() {
                    return None;
                }
                let mut p = self.folder.clone()?.join(name);
                if p.extension().is_none() {
                    if let Some(ext) = extensions.first() {
                        p.set_extension(ext);
                    }
                }
                Some(p)
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Open;
        let modal =
            egui::Modal::new(egui::Id::new(("vw_tree_picker", &self.title))).show(ctx, |ui| {
                ui.set_width(560.0);
                vw_ui::panel::header(ui, &self.title);
                ui.add_space(4.0);
                let shown = self.folder.as_ref().map(|p| p.display().to_string());
                vw_ui::text::tail_label(ui, shown.as_deref().unwrap_or(&self.labels.nothing), None);
                ui.add_space(4.0);

                let lists_files = !matches!(self.mode, Mode::Folder);
                let height = 360.0;
                ui.horizontal(|ui| {
                    let tree_w = if lists_files {
                        ui.available_width() * 0.55
                    } else {
                        ui.available_width()
                    };
                    ui.allocate_ui_with_layout(
                        egui::vec2(tree_w, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::both()
                                .id_salt("picker_tree")
                                .auto_shrink(false)
                                .show(ui, |ui| {
                                    let mut hooks = Plain {
                                        current: self.folder.as_deref(),
                                    };
                                    let drawn = self.tree.ui(ui, &mut hooks);
                                    if let Some(p) = drawn.clicked {
                                        self.select_folder(p);
                                    }
                                    if let (Some(p), Mode::Folder) =
                                        (drawn.double_clicked, &self.mode)
                                    {
                                        outcome = Outcome::Chosen(p);
                                    }
                                });
                        },
                    );
                    if lists_files {
                        ui.separator();
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("picker_files")
                                    .auto_shrink(false)
                                    .show(ui, |ui| {
                                        let mut clicked = None;
                                        let mut double = None;
                                        for f in &self.files {
                                            let name = crate::display_name(f);
                                            let r = ui.selectable_label(
                                                self.file.as_deref() == Some(f.as_path()),
                                                name,
                                            );
                                            if r.clicked() {
                                                clicked = Some(f.clone());
                                            }
                                            if r.double_clicked() {
                                                double = Some(f.clone());
                                            }
                                        }
                                        if let Some(f) = clicked {
                                            if let Mode::SaveFile { name, .. } = &mut self.mode {
                                                *name = crate::display_name(&f);
                                            }
                                            self.file = Some(f);
                                        }
                                        if let Some(f) = double {
                                            if matches!(self.mode, Mode::OpenFile { .. }) {
                                                outcome = Outcome::Chosen(f);
                                            }
                                        }
                                    });
                            },
                        );
                    }
                });

                ui.add_space(6.0);
                if let Mode::SaveFile { name, .. } = &mut self.mode {
                    ui.horizontal(|ui| {
                        ui.label(&self.labels.name);
                        ui.add(
                            egui::TextEdit::singleline(name).desired_width(ui.available_width()),
                        );
                    });
                    ui.add_space(6.0);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let choice = self.choice();
                    if ui
                        .add_enabled(choice.is_some(), egui::Button::new(&self.labels.choose))
                        .clicked()
                    {
                        if let Some(p) = choice {
                            outcome = Outcome::Chosen(p);
                        }
                    }
                    if ui.button(&self.labels.cancel).clicked() {
                        outcome = Outcome::Cancelled;
                    }
                });
            });
        if modal.should_close() && matches!(outcome, Outcome::Open) {
            outcome = Outcome::Cancelled;
        }
        outcome
    }
}

/// `dir` の直下の、拡張子が `extensions` のどれかに合うファイルを名前順で返す。
/// `extensions` が空なら全部。
fn list_files(dir: &Path, extensions: &[String]) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut nodes: Vec<Node<()>> =
        rd.flatten()
            .filter(|e| !entry_is_dir(e))
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if is_hidden_name(&name) {
                    return None;
                }
                let path = e.path();
                let ok = extensions.is_empty()
                    || path.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                        extensions.iter().any(|want| want.eq_ignore_ascii_case(x))
                    });
                ok.then(|| Node::entry(path, name, false, None))
            })
            .collect();
    nodes.sort_by(by_name);
    nodes.into_iter().map(|n| n.path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_filtered_by_extension_ignoring_case_and_sorted() {
        let dir = std::env::temp_dir().join(format!("vw-tree-picker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["b.CSV", "a.csv", "notes.txt", ".hidden.csv"] {
            std::fs::write(dir.join(n), b"").unwrap();
        }
        std::fs::create_dir_all(dir.join("sub.csv")).unwrap();
        let names: Vec<String> = list_files(&dir, &["csv".into()])
            .iter()
            .map(|p| crate::display_name(p))
            .collect();
        assert_eq!(names, ["a.csv", "b.CSV"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_choice_adds_the_first_extension_when_the_name_has_none() {
        let mut p = Picker::new(
            "t",
            Mode::SaveFile {
                extensions: vec!["toml".into()],
                name: "preset".into(),
            },
            None,
            Labels::default(),
        );
        p.folder = Some(PathBuf::from("/x"));
        assert_eq!(p.choice(), Some(PathBuf::from("/x/preset.toml")));
        if let Mode::SaveFile { name, .. } = &mut p.mode {
            *name = "  ".into();
        }
        assert_eq!(p.choice(), None, "an empty name cannot be saved");
    }
}
