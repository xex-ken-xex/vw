//! 色導出の自由なノード。ファイルの段を通った後の表示の値（チャンネルごとに 0〜1）を順に変える。
//!
//! 値を格納値（HU や 16bit の整数）ではなく 0〜1 で扱うのは、保存したノードの列を
//! 形式の違う画像にも同じ意味で当てられるようにするため。

use serde::{Deserialize, Serialize};

use crate::expr::{Env, Program, Target};

/// ノードが流す画像。アルファは持たない（標準の表示のものをそのまま使う）。
#[derive(Debug, Clone, PartialEq)]
pub struct Float {
    pub width: usize,
    pub height: usize,
    /// 1（グレー）か 3（RGB）。
    pub channels: usize,
    /// 行ごと、画素ごと、チャンネルごとの順。
    pub data: Vec<f32>,
}

impl Float {
    pub fn new(width: usize, height: usize, channels: usize, data: Vec<f32>) -> Self {
        debug_assert_eq!(data.len(), width * height * channels);
        Self {
            width,
            height,
            channels,
            data,
        }
    }

    /// 0〜1 を 0〜255 に丸めた RGB（アルファなし、画素ごとに 3 バイト）。
    pub fn to_rgb8(&self) -> Vec<u8> {
        let level = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
        match self.channels {
            1 => self.data.iter().flat_map(|&v| [level(v); 3]).collect(),
            _ => self.data.iter().map(|&v| level(v)).collect(),
        }
    }
}

fn one() -> f32 {
    1.0
}

fn identity() -> [[f32; 3]; 3] {
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

/// 1 つのノード。`kind` で見分けて保存する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    /// 画素ごとの式（[`crate::expr`]）。
    Expression {
        text: String,
    },
    /// 黒点と白点を 0 と 1 に伸ばし、ガンマで中間を持ち上げる。
    Levels {
        #[serde(default)]
        black: f32,
        #[serde(default = "one")]
        white: f32,
        #[serde(default = "one")]
        gamma: f32,
    },
    Invert,
    /// RGB の混合。出力 = matrix × [r, g, b]。グレーは 1 行目の和を掛ける。
    Mix {
        #[serde(default = "identity")]
        matrix: [[f32; 3]; 3],
    },
}

pub type Chain = Vec<Node>;

impl Node {
    /// 追加のメニューに並べる順。
    pub const KINDS: [&'static str; 4] = ["expression", "levels", "invert", "mix"];

    pub fn new(kind: &str) -> Option<Self> {
        Some(match kind {
            "expression" => Node::Expression { text: "v".into() },
            "levels" => Node::Levels {
                black: 0.0,
                white: 1.0,
                gamma: 1.0,
            },
            "invert" => Node::Invert,
            "mix" => Node::Mix { matrix: identity() },
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Node::Expression { .. } => "expression",
            Node::Levels { .. } => "levels",
            Node::Invert => "invert",
            Node::Mix { .. } => "mix",
        }
    }

    /// 見出し（英語。表示時に訳す）。
    pub fn title(kind: &str) -> &'static str {
        match kind {
            "expression" => "Expression",
            "levels" => "Levels",
            "invert" => "Invert",
            "mix" => "Channel mix",
            _ => "Node",
        }
    }

    fn apply(&self, img: &mut Float) -> Result<(), String> {
        match self {
            Node::Expression { text } => {
                let program = Program::compile(text).map_err(|e| e.to_string())?;
                run_expression(&program, img);
            }
            Node::Levels {
                black,
                white,
                gamma,
            } => {
                let span = (white - black).abs().max(f32::EPSILON) * (white - black).signum();
                let exponent = 1.0 / gamma.max(1e-3);
                for v in &mut img.data {
                    *v = ((*v - black) / span).clamp(0.0, 1.0).powf(exponent);
                }
            }
            Node::Invert => {
                for v in &mut img.data {
                    *v = 1.0 - *v;
                }
            }
            Node::Mix { matrix } => {
                if img.channels == 1 {
                    let k: f32 = matrix[0].iter().sum();
                    for v in &mut img.data {
                        *v *= k;
                    }
                } else {
                    for px in img.data.as_chunks_mut::<3>().0 {
                        let [r, g, b] = *px;
                        for (c, row) in px.iter_mut().zip(matrix) {
                            *c = row[0] * r + row[1] * g + row[2] * b;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn run_expression(program: &Program, img: &mut Float) {
    let (w, h, n) = (img.width, img.height, img.channels);
    let rows_per = h.div_ceil(threads(w * h)).max(1);
    let mut stack = Vec::new();
    let mut out = Vec::new();
    std::thread::scope(|scope| {
        for (band, rows) in img.data.chunks_mut(rows_per * w * n).enumerate() {
            let (mut stack, mut out) = (std::mem::take(&mut stack), std::mem::take(&mut out));
            scope.spawn(move || {
                for (i, px) in rows.chunks_exact_mut(n).enumerate() {
                    let index = band * rows_per * w + i;
                    let (r, g, b) = if n == 1 {
                        (px[0], px[0], px[0])
                    } else {
                        (px[0], px[1], px[2])
                    };
                    let env = Env {
                        r,
                        g,
                        b,
                        a: 1.0,
                        v: px[0],
                        // 位置は 0〜1（左端・上端が 0、右端・下端が 1）。画素数で割らないのは、
                        // 同じ式を大きさの違う画像にも当てられるようにするため。画素数は w・h。
                        x: (index % w) as f32 / (w - 1).max(1) as f32,
                        y: (index / w) as f32 / (h - 1).max(1) as f32,
                        w: w as f32,
                        h: h as f32,
                    };
                    if n == 1 {
                        program.eval_into(&env, &mut stack, &mut out);
                        for (target, value) in &out {
                            if matches!(target, Target::All | Target::R) {
                                px[0] = *value;
                            }
                        }
                    } else {
                        // 「すべてのチャンネル」の行はチャンネルごとに v を入れ替えて評価する。
                        let mut next = [r, g, b];
                        for (c, slot) in next.iter_mut().enumerate() {
                            let env = Env {
                                v: [r, g, b][c],
                                ..env
                            };
                            program.eval_into(&env, &mut stack, &mut out);
                            for (target, value) in &out {
                                let hit = match target {
                                    Target::All => true,
                                    Target::R => c == 0,
                                    Target::G => c == 1,
                                    Target::B => c == 2,
                                };
                                if hit {
                                    *slot = *value;
                                }
                            }
                        }
                        px.copy_from_slice(&next);
                    }
                }
            });
        }
    });
}

fn threads(pixels: usize) -> usize {
    let available = std::thread::available_parallelism().map_or(1, |n| n.get());
    available.min(pixels.div_ceil(1 << 16)).max(1)
}

/// `chain` を順に当てる。失敗したら (ノードの番号, 理由)。
pub fn apply(chain: &[Node], img: &mut Float) -> Result<(), (usize, String)> {
    for (i, node) in chain.iter().enumerate() {
        node.apply(img).map_err(|e| (i, e))?;
    }
    Ok(())
}

/// 式ノードの文法の確認（図に赤で出すため）。
pub fn check(node: &Node) -> Option<String> {
    match node {
        Node::Expression { text } => Program::compile(text).err().map(|e| e.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(values: &[f32]) -> Float {
        Float::new(values.len(), 1, 1, values.to_vec())
    }

    #[test]
    fn levels_stretch_black_and_white_points_and_lift_midtones_with_gamma() {
        let mut img = gray(&[0.0, 0.2, 0.6, 1.0]);
        let node = Node::Levels {
            black: 0.2,
            white: 0.6,
            gamma: 1.0,
        };
        apply(&[node], &mut img).unwrap();
        assert_eq!(img.data, vec![0.0, 0.0, 1.0, 1.0]);
        let mut img = gray(&[0.25]);
        let node = Node::Levels {
            black: 0.0,
            white: 1.0,
            gamma: 2.0,
        };
        apply(&[node], &mut img).unwrap();
        assert_eq!(img.data, vec![0.5]);
    }

    #[test]
    fn invert_and_mix_change_each_pixel() {
        let mut img = gray(&[0.25]);
        apply(&[Node::Invert], &mut img).unwrap();
        assert_eq!(img.data, vec![0.75]);
        let mut rgb = Float::new(1, 1, 3, vec![1.0, 0.5, 0.0]);
        let swap = Node::Mix {
            matrix: [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
        };
        apply(&[swap], &mut rgb).unwrap();
        assert_eq!(rgb.data, vec![0.0, 0.5, 1.0]);
        let mut g = gray(&[0.5]);
        let half = Node::Mix {
            matrix: [[0.5, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        };
        apply(&[half], &mut g).unwrap();
        assert_eq!(g.data, vec![0.25]);
    }

    #[test]
    fn expression_sees_rgb_and_position_and_writes_per_channel() {
        let mut rgb = Float::new(2, 1, 3, vec![0.2, 0.4, 0.6, 1.0, 1.0, 1.0]);
        let node = Node::Expression {
            text: "r: b\nb: r\ng: x".into(),
        };
        apply(&[node], &mut rgb).unwrap();
        assert_eq!(rgb.data, vec![0.6, 0.0, 0.2, 1.0, 1.0, 1.0]);
        let mut img = gray(&[0.0, 0.5, 1.0]);
        let node = Node::Expression {
            text: "v * 2 + x * (w - 1)".into(),
        };
        apply(&[node], &mut img).unwrap();
        assert_eq!(img.data, vec![0.0, 2.0, 4.0]);
        let mut column = Float::new(1, 3, 1, vec![0.0; 3]);
        apply(&[Node::Expression { text: "y".into() }], &mut column).unwrap();
        assert_eq!(column.data, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn bad_expression_reports_its_node() {
        let chain = vec![
            Node::Invert,
            Node::Expression {
                text: "nope(v)".into(),
            },
        ];
        let mut img = gray(&[0.5]);
        let (index, message) = apply(&chain, &mut img).unwrap_err();
        assert_eq!(index, 1);
        assert!(message.contains("nope"));
        assert!(check(&chain[1]).is_some() && check(&chain[0]).is_none());
    }

    #[test]
    fn nodes_round_trip_through_toml_with_defaults_for_missing_fields() {
        let chain = vec![
            Node::Expression {
                text: "r: pow(r, 0.9)".into(),
            },
            Node::Levels {
                black: 0.1,
                white: 0.9,
                gamma: 1.2,
            },
            Node::Invert,
            Node::Mix { matrix: identity() },
        ];
        let text = toml::to_string(&toml::Table::from_iter([(
            "nodes".to_owned(),
            toml::Value::try_from(&chain).unwrap(),
        )]))
        .unwrap();
        assert!(text.contains("kind = \"levels\""), "{text}");
        let table: toml::Table = text.parse().unwrap();
        let back: Chain = table["nodes"].clone().try_into().unwrap();
        assert_eq!(back, chain);

        let partial: Chain = "[[nodes]]\nkind = \"levels\"\nblack = 0.2\n"
            .parse::<toml::Table>()
            .unwrap()["nodes"]
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(
            partial,
            vec![Node::Levels {
                black: 0.2,
                white: 1.0,
                gamma: 1.0
            }]
        );
    }

    #[test]
    fn float_rounds_to_display_levels() {
        assert_eq!(
            gray(&[0.0, 0.5, 1.0, 1.5]).to_rgb8()[..6],
            [0, 0, 0, 128, 128, 128]
        );
        assert_eq!(
            Float::new(1, 1, 3, vec![0.0, 0.5, 1.0]).to_rgb8(),
            [0, 128, 255]
        );
    }
}
