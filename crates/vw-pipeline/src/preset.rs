//! 名前を付けて保存したパイプラインの TOML。ビューアの一覧（設定フォルダの `pipelines.toml`）と、
//! 1 つ分を人に渡すファイル（`.vwpipe`）が同じ書き方を使う。
//!
//! 全体を入れられる標準の形式はない（CLF や .cube は画素ごとの色の処理だけで、ぼかしやノイズ除去が
//! 入らない）ので、darktable や RawTherapee と同じく自前の形式にし、色の部分だけを標準でも書き出す。

use crate::look::Look;
use crate::nodes::{Chain, Node};
use crate::pixel::{Params, VoiFunction};

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
    /// 灰色の画像の見た目。`None` は入れていない（かける側の既定のまま）。
    pub look: Option<Look>,
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
            .and_then(entry_of)
        {
            Ok((values, look)) if !name.trim().is_empty() => presets.push(Preset {
                name: name.clone(),
                values,
                look,
            }),
            Ok(_) => problems.push("a pipeline with an empty name was skipped".to_owned()),
            Err(e) => problems.push(format!("[{name}]: {e} (skipped)")),
        }
    }
    Ok((presets, problems))
}

fn entry_of(t: &toml::Table) -> Result<(Values, Option<Look>), String> {
    let look = match t.get("look") {
        None => None,
        Some(v) => Some(
            v.clone()
                .try_into::<Look>()
                .map_err(|e| format!("look: {}", e.message()))?,
        ),
    };
    Ok((values_of(t)?, look.filter(|l| !l.is_empty())))
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
    #[serde(skip_serializing_if = "Option::is_none")]
    look: Option<Look>,
}

impl Entry {
    fn of(values: &Values, look: Option<&Look>) -> Self {
        let v = values.params;
        Entry {
            rescale_slope: v.map(|v| v.slope),
            rescale_intercept: v.map(|v| v.intercept),
            window_center: v.map(|v| v.center),
            window_width: v.map(|v| v.width),
            voi_lut_function: v.map(|v| v.function.dicom_name()),
            invert: v.map(|v| v.invert),
            nodes: values.chain.clone(),
            look: look.filter(|l| !l.is_empty()).cloned(),
        }
    }
}

pub fn to_toml(presets: &[Preset]) -> String {
    let table: std::collections::BTreeMap<&str, Entry> = presets
        .iter()
        .map(|p| (p.name.as_str(), Entry::of(&p.values, p.look.as_ref())))
        .collect();
    toml::to_string(&table).unwrap_or_default()
}

/// 1 つ分を人に渡すファイル（`.vwpipe`）の拡張子。
pub const FILE_EXTENSION: &str = "vwpipe";

/// 1 つ分のファイルの中身。一覧の 1 表と同じ項目を、表の名前なしで書く（名前はファイル名）。
pub fn to_file(values: &Values, look: Option<&Look>) -> String {
    let body = toml::to_string(&Entry::of(values, look)).unwrap_or_default();
    format!("# vw pipeline\n{body}")
}

pub fn from_file(text: &str) -> Result<(Values, Option<Look>), String> {
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| e.message().to_owned())?;
    entry_of(&table)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                look: None,
            },
            Preset {
                name: "肺野".into(),
                values: lung(),
                look: None,
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
}
