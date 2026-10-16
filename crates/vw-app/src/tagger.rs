//! Danbooru タグの自動付与（WD tagger）。ONNX Runtime を実行時に読み込んで推論する。
//!
//! ONNX Runtime を vw にリンクせず実行時に読み込む（`load-dynamic`）のは、数十 MB の
//! ランタイムとモデルを使う人だけがダウンロードすれば済むようにするため。
//! 画像の縮小・並べ替えを自前で書くのは、モデルの入力（正方形に白で埋め、BGR、0〜255）が
//! 決まっていて、画像ライブラリを足すほどの処理ではないため。

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use eframe::egui::ColorImage;
use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};

/// タグの区分（selected_tags.csv の category）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    General,
    Character,
    Rating,
    /// 作者・作品名など。付けない。
    Other,
}

impl Category {
    fn of(code: &str) -> Self {
        match code.trim() {
            "0" => Category::General,
            "4" => Category::Character,
            "9" => Category::Rating,
            _ => Category::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TagDef {
    pub name: String,
    pub category: Category,
}

/// `selected_tags.csv` を読む。列は tag_id,name,category,count（先頭行は見出し）。
pub fn parse_tags(csv: &str) -> Result<Vec<TagDef>, String> {
    let mut out = Vec::new();
    for (i, line) in csv.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 3 {
            return Err(format!("line {}: expected tag_id,name,category", i + 1));
        }
        out.push(TagDef {
            name: cols[1].to_owned(),
            category: Category::of(cols[2]),
        });
    }
    if out.is_empty() {
        return Err("no tags".into());
    }
    Ok(out)
}

/// タグを付けるしきい値（確信度 0〜1）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    pub general: f32,
    pub character: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            general: 0.35,
            character: 0.85,
        }
    }
}

/// 確信度つきのタグ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scored {
    pub name: String,
    pub score: f32,
}

/// 1 枚の画像に付いたタグ。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tags {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub general: Vec<Scored>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub characters: Vec<Scored>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<Scored>,
}

/// 確信度の列からタグを選ぶ。一般とキャラクターは別のしきい値、レーティングは一番高いもの。
pub fn pick(defs: &[TagDef], scores: &[f32], t: Thresholds) -> Tags {
    let mut tags = Tags::default();
    for (def, &score) in defs.iter().zip(scores) {
        let scored = || Scored {
            name: def.name.clone(),
            score,
        };
        match def.category {
            Category::General if score >= t.general => tags.general.push(scored()),
            Category::Character if score >= t.character => tags.characters.push(scored()),
            Category::Rating if tags.rating.as_ref().is_none_or(|r| score > r.score) => {
                tags.rating = Some(scored());
            }
            _ => {}
        }
    }
    let by_score = |a: &Scored, b: &Scored| b.score.total_cmp(&a.score);
    tags.general.sort_by(by_score);
    tags.characters.sort_by(by_score);
    tags
}

/// モデルの入力。正方形に白で埋めて `side` 画素に縮め、BGR の 0〜255 を NHWC で並べる。
/// 縮小は元の画素の範囲平均。拡大のときは最寄りの画素。
pub fn input(image: &ColorImage, side: usize) -> Vec<f32> {
    let [w, h] = image.size;
    let square = w.max(h).max(1);
    let offset_x = (square - w) / 2;
    let offset_y = (square - h) / 2;
    let scale = square as f32 / side as f32;
    let mut out = Vec::with_capacity(side * side * 3);
    for y in 0..side {
        let y0 = (y as f32 * scale) as usize;
        let y1 = (((y + 1) as f32 * scale) as usize).clamp(y0 + 1, square);
        for x in 0..side {
            let x0 = (x as f32 * scale) as usize;
            let x1 = (((x + 1) as f32 * scale) as usize).clamp(x0 + 1, square);
            let mut sum = [0u32; 3];
            let mut n = 0;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let (px, py) = (sx.wrapping_sub(offset_x), sy.wrapping_sub(offset_y));
                    let c = if px < w && py < h {
                        image.pixels[py * w + px]
                    } else {
                        eframe::egui::Color32::WHITE
                    };
                    // 透明な所は白の上に置く。
                    let a = c.a() as u32;
                    for (i, v) in [c.r(), c.g(), c.b()].into_iter().enumerate() {
                        sum[i] += (v as u32 * a + 255 * (255 - a)) / 255;
                    }
                    n += 1;
                }
            }
            for i in [2, 1, 0] {
                out.push(sum[i] as f32 / n as f32);
            }
        }
    }
    out
}

/// 読み込んだモデル。重いので一度読んだら持ち続け、次の仕事でも使い回す。
pub struct Tagger {
    session: Session,
    defs: Vec<TagDef>,
    side: usize,
    input_name: String,
    output_name: String,
}

impl Tagger {
    /// ランタイムとモデルを読む。`threads` が 0 なら ONNX Runtime に任せる。
    pub fn load(runtime: &Path, model: &Path, csv: &Path, threads: usize) -> Result<Self, String> {
        let defs = parse_tags(
            &std::fs::read_to_string(csv).map_err(|e| format!("{}: {e}", csv.display()))?,
        )
        .map_err(|e| format!("{}: {e}", csv.display()))?;
        let env = environment(runtime)?;
        let mut builder = Session::builder(&env).map_err(|e| e.to_string())?;
        if threads > 0 {
            builder = builder
                .with_intra_threads(threads)
                .map_err(|e| e.to_string())?;
        }
        let session = builder
            .commit_from_file(model)
            .map_err(|e| format!("{}: {e}", model.display()))?;
        let input = session.inputs().first().ok_or("model has no input")?;
        let output = session.outputs().first().ok_or("model has no output")?;
        let side = input
            .dtype()
            .tensor_shape()
            .and_then(|s| s.get(1).copied())
            .filter(|&d| d > 0)
            .map_or(448, |d| d as usize);
        Ok(Self {
            input_name: input.name().to_owned(),
            output_name: output.name().to_owned(),
            session,
            defs,
            side,
        })
    }

    /// 1 枚の確信度の列（`defs` と同じ並び）。
    pub fn scores(&mut self, image: &ColorImage) -> Result<Vec<f32>, String> {
        let side = self.side;
        let data = input(image, side);
        let tensor = Tensor::from_array(([1i64, side as i64, side as i64, 3], data))
            .map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => tensor])
            .map_err(|e| e.to_string())?;
        let value = outputs
            .get(&self.output_name)
            .ok_or("model gave no output")?;
        let (_, scores) = value
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        if scores.len() != self.defs.len() {
            return Err(format!(
                "model gives {} scores but the tag list has {} tags",
                scores.len(),
                self.defs.len()
            ));
        }
        Ok(scores.to_vec())
    }

    pub fn tag(&mut self, image: &ColorImage, t: Thresholds) -> Result<Tags, String> {
        let scores = self.scores(image)?;
        Ok(pick(&self.defs, &scores, t))
    }
}

/// ONNX Runtime はプロセスに 1 つしか読み込めないので、最初に読んだものを持ち続ける。
fn environment(runtime: &Path) -> Result<ort::environment::Environment, String> {
    static ENV: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        ort::init_from(runtime)
            .map_err(|e| format!("{}: {e}", runtime.display()))?
            .with_name("vw")
            .with_telemetry(false)
            .build()
            .map_err(|e| e.to_string())
    })
    .clone()
}

/// 読んだモデルの置き場。仕事をまたいで使い回す。
pub fn shared() -> &'static Mutex<Option<Tagger>> {
    static TAGGER: Mutex<Option<Tagger>> = Mutex::new(None);
    &TAGGER
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;

    #[test]
    fn parses_the_tag_list_with_its_categories() {
        let csv = "tag_id,name,category,count\n1,1girl,0,100\n2,hatsune_miku,4,50\n3,general,9,10\n4,artist,1,1\n";
        let defs = parse_tags(csv).unwrap();
        assert_eq!(defs.len(), 4);
        assert_eq!(defs[0].category, Category::General);
        assert_eq!(defs[1].category, Category::Character);
        assert_eq!(defs[2].category, Category::Rating);
        assert_eq!(defs[3].category, Category::Other);
        assert!(parse_tags("tag_id,name\n").is_err());
    }

    #[test]
    fn picks_by_category_thresholds_and_the_best_rating() {
        let defs = parse_tags(
            "h\n1,a,0,1\n2,b,0,1\n3,c,4,1\n4,d,4,1\n5,safe,9,1\n6,nsfw,9,1\n7,artist,1,1\n",
        )
        .unwrap();
        let scores = [0.9, 0.2, 0.95, 0.5, 0.3, 0.6, 0.99];
        let tags = pick(&defs, &scores, Thresholds::default());
        let names = |v: &[Scored]| v.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&tags.general), ["a"]);
        assert_eq!(names(&tags.characters), ["c"]);
        assert_eq!(tags.rating.unwrap().name, "nsfw");
    }

    #[test]
    fn general_tags_are_sorted_by_score() {
        let defs = parse_tags("h\n1,low,0,1\n2,high,0,1\n").unwrap();
        let tags = pick(&defs, &[0.4, 0.8], Thresholds::default());
        assert_eq!(tags.general[0].name, "high");
    }

    /// 本物の ONNX Runtime と、WD tagger と同じ入出力の形の小さなモデルで端から端まで。
    /// `VW_TEST_MODELS=<dir> cargo test -- --ignored tags_with_the_runtime`
    #[test]
    #[ignore]
    fn tags_with_the_runtime_and_a_tiny_model() {
        let dir =
            std::path::PathBuf::from(std::env::var("VW_TEST_MODELS").expect("VW_TEST_MODELS"));
        let store = crate::ai_models::Store { dir };
        let mut tagger = Tagger::load(
            &store.runtime(),
            &store.tagger_model(),
            &store.tagger_tags(),
            2,
        )
        .unwrap();
        let red = ColorImage {
            size: [3, 2],
            pixels: vec![Color32::from_rgb(255, 0, 0); 6],
            ..Default::default()
        };
        let tags = tagger.tag(&red, Thresholds::default()).unwrap();
        let names = |v: &[Scored]| v.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&tags.general), ["red"]);
        assert!(tags.characters.is_empty());
        assert_eq!(tags.rating.unwrap().name, "safe");
        let blue = ColorImage {
            size: [2, 2],
            pixels: vec![Color32::from_rgb(0, 0, 255); 4],
            ..Default::default()
        };
        let tags = tagger.tag(&blue, Thresholds::default()).unwrap();
        assert_eq!(names(&tags.general), ["blue"]);
        assert_eq!(names(&tags.characters), ["not_red"]);
    }

    #[test]
    fn input_pads_to_a_white_square_in_bgr() {
        // 2x1 の画像（赤、青）を 2x2 に。下の行は白で埋まる。
        let image = ColorImage {
            size: [2, 1],
            pixels: vec![Color32::from_rgb(255, 0, 0), Color32::from_rgb(0, 0, 255)],
            ..Default::default()
        };
        let data = input(&image, 2);
        assert_eq!(data.len(), 12);
        assert_eq!(&data[0..3], &[0.0, 0.0, 255.0]); // 赤は BGR で (0,0,255)
        assert_eq!(&data[3..6], &[255.0, 0.0, 0.0]);
        assert_eq!(&data[6..9], &[255.0, 255.0, 255.0]);
    }

    #[test]
    fn input_averages_blocks_when_shrinking() {
        let mut pixels = vec![Color32::BLACK; 4];
        pixels[0] = Color32::WHITE;
        let image = ColorImage {
            size: [2, 2],
            pixels,
            ..Default::default()
        };
        let data = input(&image, 1);
        assert_eq!(data, vec![63.75, 63.75, 63.75]);
    }
}
