//! 名前を付けて保存した色導出の値（設定フォルダの `pipelines.toml`）。
//!
//! settings.toml に入れないのは、パイプラインだけを人に渡したり版で管理したりしやすくするため。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::nodes::{Chain, Node};
use crate::pixel_pipeline::{Params, VoiFunction};

pub const FILE_NAME: &str = "pipelines.toml";
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 1 つのパイプラインの中身: ファイルの段の値（DICOM のグレースケールだけに効く。`None` は
/// ファイルの値のまま）と、その後ろに当てるノードの列。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Values {
    pub params: Option<Params>,
    pub chain: Chain,
}

impl Values {
    /// 標準の表示と同じ（何も変えていない）。
    pub fn is_standard(&self) -> bool {
        self.params.is_none() && self.chain.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub values: Values,
}

const PARAM_KEYS: [&str; 6] = [
    "rescale_slope",
    "rescale_intercept",
    "window_center",
    "window_width",
    "voi_lut_function",
    "invert",
];

/// 名前の順に並ぶ。表の名前がそのまま保存した名前になる。
pub fn from_toml(text: &str) -> Result<(Vec<Preset>, Vec<String>), String> {
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| e.message().to_owned())?;
    let mut presets = Vec::new();
    let mut problems = Vec::new();
    for (name, value) in &table {
        match value
            .as_table()
            .ok_or("not a table".to_owned())
            .and_then(values_of)
        {
            Ok(values) if !name.trim().is_empty() => presets.push(Preset {
                name: name.clone(),
                values,
            }),
            Ok(_) => problems.push("a pipeline with an empty name was skipped".to_owned()),
            Err(e) => problems.push(format!("[{name}]: {e} (skipped)")),
        }
    }
    Ok((presets, problems))
}

fn values_of(t: &toml::Table) -> Result<Values, String> {
    let params = if PARAM_KEYS.iter().any(|k| t.contains_key(*k)) {
        Some(params_of(t)?)
    } else {
        None
    };
    let chain = match t.get("nodes") {
        None => Vec::new(),
        Some(v) => v
            .clone()
            .try_into::<Chain>()
            .map_err(|e| format!("nodes: {}", e.message()))?,
    };
    Ok(Values { params, chain })
}

fn params_of(t: &toml::Table) -> Result<Params, String> {
    let number = |key: &str| -> Result<f64, String> {
        match t.get(key) {
            Some(toml::Value::Float(v)) if v.is_finite() => Ok(*v),
            #[allow(clippy::cast_precision_loss)]
            Some(toml::Value::Integer(v)) => Ok(*v as f64),
            Some(_) => Err(format!("{key} is not a number")),
            None => Err(format!("{key} is missing")),
        }
    };
    let function = match t.get("voi_lut_function") {
        None => VoiFunction::Linear,
        Some(v) => v
            .as_str()
            .and_then(VoiFunction::from_dicom)
            .ok_or("voi_lut_function is not LINEAR, LINEAR_EXACT or SIGMOID")?,
    };
    let invert = match t.get("invert") {
        None => false,
        Some(v) => v.as_bool().ok_or("invert is not true or false")?,
    };
    Ok(Params {
        slope: number("rescale_slope")?,
        intercept: number("rescale_intercept")?,
        center: number("window_center")?,
        width: number("window_width")?,
        function,
        invert,
    })
}

/// 書き出す 1 表。手で読み書きしやすいよう、項目は画素の変換の順に並べる。
#[derive(serde::Serialize)]
struct Entry {
    #[serde(skip_serializing_if = "Option::is_none")]
    rescale_slope: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rescale_intercept: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_center: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    voi_lut_function: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    invert: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    nodes: Vec<Node>,
}

pub fn to_toml(presets: &[Preset]) -> String {
    let table: std::collections::BTreeMap<&str, Entry> = presets
        .iter()
        .map(|p| {
            let v = p.values.params;
            let entry = Entry {
                rescale_slope: v.map(|v| v.slope),
                rescale_intercept: v.map(|v| v.intercept),
                window_center: v.map(|v| v.center),
                window_width: v.map(|v| v.width),
                voi_lut_function: v.map(|v| v.function.dicom_name()),
                invert: v.map(|v| v.invert),
                nodes: p.values.chain.clone(),
            };
            (p.name.as_str(), entry)
        })
        .collect();
    toml::to_string(&table).unwrap_or_default()
}

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
    pub fn put(&mut self, name: &str, values: Values) -> Result<(), String> {
        match self.presets.iter_mut().find(|p| p.name == name) {
            Some(p) => p.values = values,
            None => {
                self.presets.push(Preset {
                    name: name.to_owned(),
                    values,
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
            chain: vec![
                Node::Levels {
                    black: 0.1,
                    white: 0.9,
                    gamma: 1.0,
                },
                Node::Expression {
                    text: "r: pow(r, 0.9)".into(),
                },
            ],
        }
    }

    fn nodes_only() -> Values {
        Values {
            params: None,
            chain: vec![Node::Invert],
        }
    }

    #[test]
    fn saved_pipelines_round_trip_through_toml_by_name() {
        let presets = vec![
            Preset {
                name: "反転".into(),
                values: nodes_only(),
            },
            Preset {
                name: "肺野".into(),
                values: lung(),
            },
        ];
        let text = to_toml(&presets);
        assert!(text.contains("[[\"肺野\".nodes]]"), "{text}");
        assert!(
            !text.contains("\"反転\".window_center") && !text.contains("[\"反転\"]\nwindow"),
            "{text}"
        );
        assert_eq!(from_toml(&text).unwrap(), (presets, vec![]));
    }

    #[test]
    fn hand_written_entries_accept_integers_and_default_to_linear_without_invert() {
        let text = "[mediastinum]\nrescale_slope = 1\nrescale_intercept = -1024\n\
                    window_center = 40\nwindow_width = 400\n";
        let (presets, problems) = from_toml(text).unwrap();
        assert!(problems.is_empty());
        let p = presets[0].values.params.unwrap();
        assert_eq!(
            (p.center, p.width, p.function, p.invert),
            (40.0, 400.0, VoiFunction::Linear, false)
        );
        assert!(presets[0].values.chain.is_empty());
    }

    #[test]
    fn an_entry_without_file_values_keeps_them_at_the_file() {
        let text = "[gamma]\n[[gamma.nodes]]\nkind = \"levels\"\ngamma = 2.2\n";
        let (presets, problems) = from_toml(text).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(presets[0].values.params, None);
        assert_eq!(
            presets[0].values.chain,
            vec![Node::Levels {
                black: 0.0,
                white: 1.0,
                gamma: 2.2
            }]
        );
    }

    #[test]
    fn broken_entries_are_skipped_and_reported_while_the_rest_load() {
        let text = "[good]\nrescale_slope = 1\nrescale_intercept = 0\nwindow_center = 40\nwindow_width = 400\n\
                    [bad]\nrescale_slope = \"one\"\n\
                    [worse]\n[[worse.nodes]]\nkind = \"teleport\"\n";
        let (presets, problems) = from_toml(text).unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "good");
        assert!(problems[0].contains("[bad]"), "{problems:?}");
        assert!(problems[1].contains("[worse]: nodes"), "{problems:?}");
    }

    #[test]
    fn store_saves_overwrites_removes_and_reloads_external_edits() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE_NAME);
        let (mut store, notes) = PresetStore::open(Some(path.clone()));
        assert!(notes.is_empty() && store.presets().is_empty());
        store.put("b", lung()).unwrap();
        store.put("a", nodes_only()).unwrap();
        let mut wider = lung();
        wider.params.as_mut().unwrap().width = 2000.0;
        store.put("b", wider).unwrap();
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
