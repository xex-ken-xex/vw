//! 名前を付けて保存した色導出の値（設定フォルダの `pipelines.toml`）。書き方は [`vw_pipeline::preset`]。
//!
//! settings.toml に入れないのは、パイプラインだけを人に渡したり版で管理したりしやすくするため。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub use vw_pipeline::look::Look;
pub use vw_pipeline::preset::*;

pub const FILE_NAME: &str = "pipelines.toml";
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 保存したパイプラインの一覧と、そのファイル。設定フォルダがなければメモリの中だけに持つ。
pub struct PresetStore {
    path: Option<PathBuf>,
    presets: Vec<Preset>,
    known_mtime: Option<SystemTime>,
    last_poll: Instant,
}

impl PresetStore {
    /// 戻り値の 2 つ目はログに赤で出す注意事項。
    pub fn open(path: Option<PathBuf>) -> (Self, Vec<String>) {
        let mut store = Self {
            path,
            presets: Vec::new(),
            known_mtime: None,
            last_poll: Instant::now(),
        };
        let notes = store.read().unwrap_or_default();
        (store, notes)
    }

    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }

    /// 同じ名前があれば値を置き換える。
    pub fn put(&mut self, name: &str, values: Values, look: Option<Look>) -> Result<(), String> {
        match self.presets.iter_mut().find(|p| p.name == name) {
            Some(p) => {
                p.values = values;
                p.look = look;
            }
            None => {
                self.presets.push(Preset {
                    name: name.to_owned(),
                    values,
                    look,
                });
                self.presets.sort_by(|a, b| a.name.cmp(&b.name));
            }
        }
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        self.presets.retain(|p| p.name != name);
        self.save()
    }

    fn save(&mut self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, to_toml(&self.presets))
        };
        write().map_err(|e| format!("{}: {e}", path.display()))?;
        self.known_mtime = mtime(path);
        Ok(())
    }

    /// 外部で書き換えられていたら読み直す。読み直したら `Some(注意事項)`。
    /// 読めなかったときは今の一覧を保ち、書きかけのファイルで一覧が消えないようにする。
    pub fn reload_if_changed(&mut self) -> Option<Vec<String>> {
        if self.last_poll.elapsed() < POLL_INTERVAL {
            return None;
        }
        self.last_poll = Instant::now();
        let now = mtime(self.path.as_ref()?);
        if now == self.known_mtime {
            return None;
        }
        self.read()
    }

    fn read(&mut self) -> Option<Vec<String>> {
        let path = self.path.as_ref()?;
        self.known_mtime = mtime(path);
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.presets.clear();
                return Some(Vec::new());
            }
            Err(e) => return Some(vec![format!("{}: {e}", path.display())]),
        };
        Some(match from_toml(&text) {
            Ok((presets, problems)) => {
                self.presets = presets;
                problems
                    .into_iter()
                    .map(|p| format!("{}: {p}", path.display()))
                    .collect()
            }
            Err(e) => vec![format!("{}: {e}", path.display())],
        })
    }

    #[cfg(test)]
    fn poll_now(&mut self) {
        self.last_poll = Instant::now() - POLL_INTERVAL;
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::Node;
    use crate::pixel_pipeline::{Params, VoiFunction};
    use crate::test_util::TempDir;

    fn lung() -> Values {
        Values {
            params: Some(Params {
                slope: 1.0,
                intercept: -1024.0,
                center: -600.0,
                width: 1500.0,
                function: VoiFunction::LinearExact,
                invert: true,
            }),
            chain: vec![Node::Invert],
        }
    }

    fn nodes_only() -> Values {
        Values {
            params: None,
            chain: vec![Node::Invert],
        }
    }

    #[test]
    fn store_saves_overwrites_removes_and_reloads_external_edits() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE_NAME);
        let (mut store, notes) = PresetStore::open(Some(path.clone()));
        assert!(notes.is_empty() && store.presets().is_empty());
        store.put("b", lung(), None).unwrap();
        store.put("a", nodes_only(), None).unwrap();
        let mut wider = lung();
        wider.params.as_mut().unwrap().width = 2000.0;
        store.put("b", wider, None).unwrap();
        assert_eq!(
            store
                .presets()
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        let (reopened, _) = PresetStore::open(Some(path.clone()));
        assert_eq!(reopened.presets()[1].values.params.unwrap().width, 2000.0);
        assert_eq!(reopened.presets()[0].values, nodes_only());
        store.remove("a").unwrap();
        assert!(store.presets().iter().all(|p| p.name != "a"));

        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(
            &path,
            "[c]\nrescale_slope = 1\nrescale_intercept = 0\nwindow_center = 1\nwindow_width = 2\n",
        )
        .unwrap();
        store.known_mtime = None;
        store.poll_now();
        assert_eq!(store.reload_if_changed(), Some(vec![]));
        assert_eq!(store.presets()[0].name, "c");
    }
}
