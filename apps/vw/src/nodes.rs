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

    /// 幅と高さが `size` に収まるよう、整数分の 1 に縮める（ブロックの平均、NaN は除く）。縮めた倍率を返す。
    pub fn shrink(&mut self, size: [usize; 2]) -> usize {
        let fit = |len: usize, to: usize| len.div_ceil(to.max(1));
        let factor = fit(self.width, size[0]).max(fit(self.height, size[1]));
        if factor <= 1 {
            return 1;
        }
        let (w, h, n) = (self.width, self.height, self.channels);
        let (ow, oh) = (w.div_ceil(factor), h.div_ceil(factor));
        let mut data = Vec::with_capacity(ow * oh * n);
        for by in 0..oh {
            for bx in 0..ow {
                for c in 0..n {
                    let (mut sum, mut count) = (0.0f32, 0u32);
                    for y in by * factor..((by + 1) * factor).min(h) {
                        for x in bx * factor..((bx + 1) * factor).min(w) {
                            let v = self.data[(y * w + x) * n + c];
                            if !v.is_nan() {
                                sum += v;
                                count += 1;
                            }
                        }
                    }
                    data.push(if count == 0 {
                        f32::NAN
                    } else {
                        sum / count as f32
                    });
                }
            }
        }
        *self = Float::new(ow, oh, n, data);
        factor
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
    /// ガウスぼかし。`radius` は σ（画素）。
    Blur {
        #[serde(default = "one")]
        radius: f32,
    },
    /// ガウスノイズの付加。`sigma` は 0〜1 の値に対する標準偏差、`seed` は乱数の種。
    Noise {
        #[serde(default = "default_sigma")]
        sigma: f32,
        #[serde(default)]
        seed: u32,
    },
    /// メディアンによるノイズ除去。`size` は窓の一辺（3 か 5）。
    Median {
        #[serde(default = "three")]
        size: u32,
    },
    /// 明るさ・コントラスト・S 字。
    Tone {
        #[serde(default)]
        brightness: f32,
        #[serde(default = "one")]
        contrast: f32,
        #[serde(default)]
        curve: f32,
    },
    /// 伸縮（表示のための黒と白と曲線）を当てる位置。これより前のノードは、黒と白で切る前の
    /// 線形の値にかかる。鎖にないときは、鎖の先頭で当てたのと同じ。
    Stretch,
    /// スターレットによるノイズ除去（[`crate::astro::denoise`]）。
    Denoise {
        #[serde(default = "default_denoise")]
        strength: [f32; crate::astro::LEVELS],
        #[serde(default = "default_threshold")]
        threshold: f32,
    },
    /// アンシャープマスク。`radius` は σ（画素）。
    Sharpen {
        #[serde(default = "half")]
        amount: f32,
        #[serde(default = "default_sharpen_radius")]
        radius: f32,
    },
    /// 星の縮小（[`crate::astro::reduce_stars`]）。`size` は削る半径（画素）。
    StarReduce {
        #[serde(default = "half")]
        amount: f32,
        #[serde(default = "two")]
        size: u32,
        #[serde(default = "default_star_threshold")]
        threshold: f32,
    },
}

fn default_denoise() -> [f32; crate::astro::LEVELS] {
    [0.8, 0.6, 0.3, 0.0]
}

fn default_threshold() -> f32 {
    3.0
}

fn default_star_threshold() -> f32 {
    4.0
}

fn default_sharpen_radius() -> f32 {
    1.5
}

fn half() -> f32 {
    0.5
}

fn two() -> u32 {
    2
}

fn default_sigma() -> f32 {
    0.05
}

fn three() -> u32 {
    3
}

pub type Chain = Vec<Node>;

impl Node {
    /// 追加のメニューに並べる順。
    pub const KINDS: [&'static str; 8] = [
        "expression",
        "levels",
        "invert",
        "mix",
        "blur",
        "noise",
        "median",
        "tone",
    ];

    /// 「天体」のまとまりに並べる順。
    pub const ASTRO_KINDS: [&'static str; 4] = ["stretch", "denoise", "sharpen", "star_reduce"];

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
            "blur" => Node::Blur { radius: 1.0 },
            "noise" => Node::Noise {
                sigma: default_sigma(),
                seed: 0,
            },
            "median" => Node::Median { size: 3 },
            "tone" => Node::Tone {
                brightness: 0.0,
                contrast: 1.0,
                curve: 0.0,
            },
            "stretch" => Node::Stretch,
            "denoise" => Node::Denoise {
                strength: default_denoise(),
                threshold: default_threshold(),
            },
            "sharpen" => Node::Sharpen {
                amount: half(),
                radius: default_sharpen_radius(),
            },
            "star_reduce" => Node::StarReduce {
                amount: half(),
                size: two(),
                threshold: default_star_threshold(),
            },
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Node::Expression { .. } => "expression",
            Node::Levels { .. } => "levels",
            Node::Invert => "invert",
            Node::Mix { .. } => "mix",
            Node::Blur { .. } => "blur",
            Node::Noise { .. } => "noise",
            Node::Median { .. } => "median",
            Node::Tone { .. } => "tone",
            Node::Stretch => "stretch",
            Node::Denoise { .. } => "denoise",
            Node::Sharpen { .. } => "sharpen",
            Node::StarReduce { .. } => "star_reduce",
        }
    }

    /// 見出し（英語。表示時に訳す）。
    pub fn title(kind: &str) -> &'static str {
        match kind {
            "expression" => "Expression",
            "levels" => "Levels",
            "invert" => "Invert",
            "mix" => "Channel mix",
            "blur" => "Blur",
            "noise" => "Add noise",
            "median" => "Median",
            "tone" => "Tone",
            "stretch" => "Stretch",
            "denoise" => "Noise reduction",
            "sharpen" => "Sharpen",
            "star_reduce" => "Star reduction",
            _ => "Node",
        }
    }

    /// `factor` 分の 1 に縮めた画像に当てるときの値。画素で決まる半径を縮め、縮めた下描きでも
    /// 全画素で描いたときと同じくらいの効き方に見せる（ノイズ除去の階層やメディアンの窓は
    /// 整数の画素でしか決められないので、そのまま）。
    pub fn scaled(&self, factor: usize) -> Node {
        let f = factor.max(1) as f32;
        match self.clone() {
            Node::Blur { radius } => Node::Blur { radius: radius / f },
            Node::Sharpen { amount, radius } => Node::Sharpen {
                amount,
                radius: radius / f,
            },
            Node::StarReduce {
                amount,
                size,
                threshold,
            } => Node::StarReduce {
                amount,
                size: ((size as f32 / f).round() as u32).max(1),
                threshold,
            },
            other => other,
        }
    }

    /// 大きな画像では時間のかかるノード。値を動かしている間は縮めた画像で計算する。
    pub fn heavy(&self) -> bool {
        matches!(
            self,
            Node::Blur { .. }
                | Node::Median { .. }
                | Node::Denoise { .. }
                | Node::Sharpen { .. }
                | Node::StarReduce { .. }
        )
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
            Node::Blur { radius } => blur(img, *radius),
            Node::Noise { sigma, seed } => add_noise(img, *sigma, *seed),
            Node::Median { size } => median(img, *size as usize),
            Node::Tone {
                brightness,
                contrast,
                curve,
            } => {
                for v in &mut img.data {
                    let t = ((*v - 0.5) * contrast + 0.5 + brightness).clamp(0.0, 1.0);
                    // S 字は smoothstep との混ぜ合わせ。負なら逆向き（中間を伸ばす）。
                    let s = t * t * (3.0 - 2.0 * t);
                    *v = (t + curve * (s - t)).clamp(0.0, 1.0);
                }
            }
            // 伸縮は画像の黒と白を知っている呼び手が当てる（[`split_at_stretch`]）。
            Node::Stretch => {}
            Node::Denoise {
                strength,
                threshold,
            } => crate::astro::denoise(img, *strength, *threshold),
            Node::Sharpen { amount, radius } => crate::astro::sharpen(img, *amount, *radius),
            Node::StarReduce {
                amount,
                size,
                threshold,
            } => crate::astro::reduce_stars(img, *amount, *size, *threshold),
        }
        Ok(())
    }
}

/// 行を分けて並列に処理する。`f(先頭の行番号, その帯の行のデータ)`。
fn for_row_bands(img: &mut Float, f: impl Fn(usize, &mut [f32]) + Sync) {
    let (w, h, n) = (img.width, img.height, img.channels);
    let rows_per = h.div_ceil(threads(w * h)).max(1);
    std::thread::scope(|scope| {
        for (band, rows) in img.data.chunks_mut(rows_per * w * n).enumerate() {
            let f = &f;
            scope.spawn(move || f(band * rows_per, rows));
        }
    });
}

/// ガウスぼかし。横と縦に分けて 2 回かける（2 次元の畳み込みより半径に対して速い）。
pub(crate) fn blur(img: &mut Float, sigma: f32) {
    if sigma <= 0.0 || img.data.is_empty() {
        return;
    }
    let half = (sigma * 3.0).ceil() as usize;
    let kernel: Vec<f32> = {
        let raw: Vec<f32> = (0..=2 * half)
            .map(|i| {
                let d = i as f32 - half as f32;
                (-d * d / (2.0 * sigma * sigma)).exp()
            })
            .collect();
        let sum: f32 = raw.iter().sum();
        raw.iter().map(|k| k / sum).collect()
    };
    let (w, h, n) = (img.width, img.height, img.channels);
    // 横: 行ごとに独立なので、行を帯に分けてそのまま並列にできる。
    for_row_bands(img, |_, rows| {
        let mut line = vec![0.0f32; w * n];
        for row in rows.chunks_exact_mut(w * n) {
            line.copy_from_slice(row);
            for x in 0..w {
                for c in 0..n {
                    let mut acc = 0.0;
                    for (k, weight) in kernel.iter().enumerate() {
                        let sx = (x + k).saturating_sub(half).min(w - 1);
                        acc += line[sx * n + c] * weight;
                    }
                    row[x * n + c] = acc;
                }
            }
        }
    });
    // 縦: 出力の行ごとに、横をぼかした画像の上下の行を読む。読むだけなので共有できる。
    let source = img.data.clone();
    for_row_bands(img, |first, rows| {
        for (i, row) in rows.chunks_exact_mut(w * n).enumerate() {
            let y = first + i;
            row.fill(0.0);
            for (k, weight) in kernel.iter().enumerate() {
                let sy = (y + k).saturating_sub(half).min(h - 1);
                let src = &source[sy * w * n..(sy + 1) * w * n];
                for (o, s) in row.iter_mut().zip(src) {
                    *o += s * weight;
                }
            }
        }
    });
}

/// ガウスノイズを足す。行ごとに種を変えた xorshift で、同じ種なら同じ模様になる。
fn add_noise(img: &mut Float, sigma: f32, seed: u32) {
    if sigma <= 0.0 {
        return;
    }
    let (w, n) = (img.width, img.channels);
    for_row_bands(img, |first, rows| {
        for (i, row) in rows.chunks_exact_mut(w * n).enumerate() {
            let mut state = splitmix(u64::from(seed) << 32 | (first + i) as u64);
            for v in row.iter_mut() {
                *v += gaussian(&mut state) * sigma;
            }
        }
    });
}

fn splitmix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) | 1
}

fn xorshift(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    // 上位 24 ビットを 0〜1 の一様乱数にする。
    (*state >> 40) as f32 / (1u32 << 24) as f32
}

/// 標準正規分布の乱数（Box–Muller）。
fn gaussian(state: &mut u64) -> f32 {
    let u1 = xorshift(state).max(1e-7);
    let u2 = xorshift(state);
    (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}

/// メディアンフィルタ。窓の一辺は 3 か 5（ほかは 3 にする）。
fn median(img: &mut Float, size: usize) {
    let size = if size >= 5 { 5 } else { 3 };
    let half = size / 2;
    let (w, h, n) = (img.width, img.height, img.channels);
    if w == 0 || h == 0 {
        return;
    }
    let source = img.data.clone();
    for_row_bands(img, |first, rows| {
        let mut window = Vec::with_capacity(size * size);
        for (i, row) in rows.chunks_exact_mut(w * n).enumerate() {
            let y = first + i;
            for x in 0..w {
                for c in 0..n {
                    window.clear();
                    for dy in 0..size {
                        let sy = (y + dy).saturating_sub(half).min(h - 1);
                        for dx in 0..size {
                            let sx = (x + dx).saturating_sub(half).min(w - 1);
                            window.push(source[(sy * w + sx) * n + c]);
                        }
                    }
                    let mid = window.len() / 2;
                    let (_, m, _) = window.select_nth_unstable_by(mid, f32::total_cmp);
                    row[x * n + c] = *m;
                }
            }
        }
    });
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

/// 伸縮ノードで分けた鎖。
#[derive(Debug, PartialEq)]
pub struct Staged<'a> {
    pub before: &'a [Node],
    pub after: &'a [Node],
    /// `after` の先頭の、鎖の中の番号（誤りを鎖の番号で返すため）。
    pub offset: usize,
}

/// 鎖を伸縮ノードの前と後ろに分ける。伸縮ノードがなければ `None`。
/// 伸縮ノードが 2 つ以上あるときは誤り（2 つ目の番号と理由）。
pub fn split_at_stretch(chain: &[Node]) -> Result<Option<Staged<'_>>, (usize, String)> {
    let mut at = chain
        .iter()
        .enumerate()
        .filter(|(_, n)| matches!(n, Node::Stretch))
        .map(|(i, _)| i);
    let Some(first) = at.next() else {
        return Ok(None);
    };
    if let Some(second) = at.next() {
        return Err((second, "Only one stretch node can be used.".into()));
    }
    Ok(Some(Staged {
        before: &chain[..first],
        after: &chain[first + 1..],
        offset: first + 1,
    }))
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
    fn the_chain_is_split_at_the_one_stretch_node() {
        assert_eq!(split_at_stretch(&[Node::Invert]), Ok(None));
        let chain = [Node::Invert, Node::Stretch, Node::Invert, Node::Invert];
        let staged = split_at_stretch(&chain).unwrap().unwrap();
        assert_eq!(
            (staged.before.len(), staged.after.len(), staged.offset),
            (1, 2, 2)
        );
        let twice = [Node::Stretch, Node::Invert, Node::Stretch];
        assert_eq!(split_at_stretch(&twice).unwrap_err().0, 2);
    }

    #[test]
    fn shrinking_averages_blocks_and_skips_nan() {
        let mut img = Float::new(4, 2, 1, vec![1.0, 3.0, 5.0, f32::NAN, 1.0, 3.0, 5.0, 7.0]);
        assert_eq!(img.shrink([2, 1]), 2);
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.data, [2.0, 17.0 / 3.0]);
        assert_eq!(img.shrink([10, 10]), 1);
    }

    #[test]
    fn preview_shrinks_pixel_radii_with_the_image() {
        let blur = Node::Blur { radius: 4.0 }.scaled(4);
        assert_eq!(blur, Node::Blur { radius: 1.0 });
        let stars = Node::StarReduce {
            amount: 0.5,
            size: 2,
            threshold: 4.0,
        }
        .scaled(8);
        assert!(matches!(stars, Node::StarReduce { size: 1, .. }));
    }

    #[test]
    fn astronomy_nodes_are_saved_with_their_values() {
        for kind in Node::ASTRO_KINDS {
            let node = Node::new(kind).unwrap();
            assert_eq!(node.kind(), kind);
            let text = toml::to_string(&Wrap { node: node.clone() }).unwrap();
            let back: Wrap = toml::from_str(&text).unwrap();
            assert_eq!(back.node, node, "{text}");
        }
    }

    #[derive(Serialize, Deserialize)]
    struct Wrap {
        node: Node,
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
    fn blur_spreads_a_point_symmetrically_and_keeps_the_total() {
        // 9 × 9 なら σ = 1 の核（半径 3）が中央の点から端まで収まる。
        let mut img = Float::new(9, 9, 1, vec![0.0; 81]);
        img.data[40] = 1.0;
        apply(&[Node::Blur { radius: 1.0 }], &mut img).unwrap();
        let total: f32 = img.data.iter().sum();
        assert!((total - 1.0).abs() < 1e-4, "{total}");
        assert!(img.data[40] > img.data[31]);
        assert_eq!(img.data[31], img.data[49]);
        assert_eq!(img.data[39], img.data[41]);
        // 平らな画像は変わらない（端は折り返す）。
        let mut flat = Float::new(4, 3, 3, vec![0.25; 36]);
        apply(&[Node::Blur { radius: 2.0 }], &mut flat).unwrap();
        assert!(flat.data.iter().all(|v| (v - 0.25).abs() < 1e-5));
    }

    #[test]
    fn noise_is_repeatable_per_seed_and_has_the_asked_spread() {
        let flat = Float::new(200, 100, 1, vec![0.5; 20_000]);
        let mut a = flat.clone();
        let mut b = flat.clone();
        let mut c = flat.clone();
        let node = |seed| Node::Noise { sigma: 0.1, seed };
        apply(&[node(1)], &mut a).unwrap();
        apply(&[node(1)], &mut b).unwrap();
        apply(&[node(2)], &mut c).unwrap();
        assert_eq!(a.data, b.data);
        assert_ne!(a.data, c.data);
        let mean = a.data.iter().sum::<f32>() / a.data.len() as f32;
        let sd =
            (a.data.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / a.data.len() as f32).sqrt();
        assert!((mean - 0.5).abs() < 0.005, "{mean}");
        assert!((sd - 0.1).abs() < 0.005, "{sd}");
    }

    #[test]
    fn median_removes_single_pixel_noise_and_keeps_edges() {
        let mut img = Float::new(5, 5, 1, vec![0.0; 25]);
        img.data[12] = 1.0;
        apply(&[Node::Median { size: 3 }], &mut img).unwrap();
        assert!(img.data.iter().all(|&v| v == 0.0));
        let mut edge = Float::new(6, 3, 1, vec![0.0; 18]);
        for y in 0..3 {
            for x in 3..6 {
                edge.data[y * 6 + x] = 1.0;
            }
        }
        let before = edge.clone();
        apply(&[Node::Median { size: 5 }], &mut edge).unwrap();
        assert_eq!(edge, before);
    }

    #[test]
    fn tone_moves_brightness_contrast_and_the_s_curve() {
        let mut img = gray(&[0.0, 0.25, 0.5, 0.75, 1.0]);
        apply(
            &[Node::Tone {
                brightness: 0.1,
                contrast: 1.0,
                curve: 0.0,
            }],
            &mut img,
        )
        .unwrap();
        assert_eq!(img.data, vec![0.1, 0.35, 0.6, 0.85, 1.0]);
        let mut img = gray(&[0.25, 0.5, 0.75]);
        apply(
            &[Node::Tone {
                brightness: 0.0,
                contrast: 2.0,
                curve: 0.0,
            }],
            &mut img,
        )
        .unwrap();
        assert_eq!(img.data, vec![0.0, 0.5, 1.0]);
        let mut img = gray(&[0.25, 0.5, 0.75]);
        apply(
            &[Node::Tone {
                brightness: 0.0,
                contrast: 1.0,
                curve: 1.0,
            }],
            &mut img,
        )
        .unwrap();
        assert!(img.data[0] < 0.25 && img.data[1] == 0.5 && img.data[2] > 0.75);
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
            Node::Blur { radius: 2.5 },
            Node::Noise {
                sigma: 0.02,
                seed: 7,
            },
            Node::Median { size: 5 },
            Node::Tone {
                brightness: -0.1,
                contrast: 1.5,
                curve: 0.3,
            },
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
        let bare: Chain = "[[nodes]]\nkind = \"blur\"\n[[nodes]]\nkind = \"noise\"\n[[nodes]]\nkind = \"median\"\n[[nodes]]\nkind = \"tone\"\n"
            .parse::<toml::Table>()
            .unwrap()["nodes"]
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(
            bare,
            ["blur", "noise", "median", "tone"]
                .map(|k| Node::new(k).unwrap())
                .to_vec()
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
