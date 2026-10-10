//! Lazily expanded folder tree showing directories and supported image files.

use std::path::{Path, PathBuf};

use eframe::egui;
use vw_core::Registry;

pub struct Node {
    pub path: PathBuf,
    name: String,
    is_dir: bool,
    /// `None` until the directory is first expanded.
    children: Option<Vec<Node>>,
    default_open: bool,
}

pub enum TreeAction {
    /// Show the file in the active image slot.
    Open(PathBuf),
    /// Show the file in a new image slot.
    Add(PathBuf),
}

impl Node {
    pub fn dir(path: PathBuf, name: Option<String>, default_open: bool) -> Self {
        let name = name.unwrap_or_else(|| display_name(&path));
        Self {
            path,
            name,
            is_dir: true,
            children: None,
            default_open,
        }
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        registry: &Registry,
        selected: Option<&Path>,
        out: &mut Option<TreeAction>,
    ) {
        if !self.is_dir {
            let is_selected = selected == Some(self.path.as_path());
            let r = ui.selectable_label(is_selected, &self.name);
            if r.clicked() {
                *out = Some(if ui.input(|i| i.modifiers.command) {
                    TreeAction::Add(self.path.clone())
                } else {
                    TreeAction::Open(self.path.clone())
                });
            }
            return;
        }
        egui::CollapsingHeader::new(&self.name)
            .id_salt(&self.path)
            .default_open(self.default_open)
            .show(ui, |ui| {
                let children = self
                    .children
                    .get_or_insert_with(|| list_dir(&self.path, registry));
                if children.is_empty() {
                    ui.weak("(empty)");
                }
                for child in children {
                    child.ui(ui, registry, selected, out);
                }
            });
    }
}

pub struct FolderTree {
    pub roots: Vec<Node>,
}

impl FolderTree {
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
        Self { roots }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        registry: &Registry,
        selected: Option<&Path>,
    ) -> Option<TreeAction> {
        let mut out = None;
        for root in &mut self.roots {
            root.ui(ui, registry, selected, &mut out);
        }
        out
    }
}

fn list_dir(path: &Path, registry: &Registry) -> Vec<Node> {
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
            // file_type() avoids a stat per entry on most platforms; follow symlinks only when needed.
            let is_dir = match e.file_type() {
                Ok(t) if t.is_symlink() => path.is_dir(),
                Ok(t) => t.is_dir(),
                Err(_) => false,
            };
            (is_dir || registry.supports_path(&path)).then_some(Node {
                path,
                name,
                is_dir,
                children: None,
                default_open: false,
            })
        })
        .collect();
    nodes.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    nodes
}

/// Supported image files next to `path`, sorted by name (for previous/next navigation).
pub fn sibling_images(path: &Path, registry: &Registry) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| registry.supports_path(p) && p.is_file())
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    files
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}
