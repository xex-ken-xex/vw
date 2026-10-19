//! 顔の検出（YuNet）と顔の特徴（SFace）。どちらも OpenCV Zoo の ONNX モデルを
//! ONNX Runtime で動かす。
//!
//! OpenCV（`FaceDetectorYN` / `FaceRecognizerSF`）を使わず前処理・後処理を自前で書くのは、
//! OpenCV を vw に組み込むと配布物が数十 MB 増え、ビルドにも C++ の環境が要るため。
//! 処理は OpenCV の実装（face_detect.cpp / face_recognize.cpp）に合わせてある。

use std::path::Path;
use std::sync::Mutex;

use eframe::egui::{Color32, ColorImage};
use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};

/// YuNet の入力の一辺。2023mar のモデルは入力の大きさが 640 x 640 に決まっている。
const DETECT_SIDE: usize = 640;
const STRIDES: [usize; 3] = [8, 16, 32];
/// 顔とみなす確からしさの下限。OpenCV の既定値と同じ。
const SCORE_THRESHOLD: f32 = 0.9;
/// 重なった枠をまとめる IoU。OpenCV の既定値と同じ。
const NMS_THRESHOLD: f32 = 0.3;
/// SFace の入力の一辺。
const ALIGN_SIDE: usize = 112;
/// SFace が学習に使った、112 x 112 の中の両目・鼻・口の両端の位置。
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// 見つけた顔。座標は画像の画素（左上が 0）。
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    /// 左上 x, 左上 y, 幅, 高さ。
    pub rect: [f32; 4],
    /// 右目、左目、鼻、口の右端、口の左端（画像の中で左に見える目が先）。
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
}

/// 画像に置く顔の枠と、照合で決まった人物。座標は画像の幅・高さに対する 0〜1。
///
/// 読み込みの縮小の具合によらず同じ位置に描けるよう、画素ではなく割合で持つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceMark {
    pub rect: [f32; 4],
    pub score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// 人物との類似度（コサイン）。手で登録した顔は 1。
    #[serde(default)]
    pub similarity: f32,
    /// 人が「人物の紐付けを外す」を選んだ顔。照合し直しても名前を付けない。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unlinked: bool,
    /// 人が「未登録の顔」から隠した顔（通りがかりの人など）。一覧に出さない。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// 顔の特徴ベクトル（長さ 1 にそろえたもの）の、コサイン類似度。
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// `image` を 640 x 640 の左上に、長辺が収まるよう縮めて置く（余白は黒）。
/// YuNet の入力（BGR、0〜255、NCHW）と、元の画素への倍率を返す。
pub fn detect_input(image: &ColorImage) -> (Vec<f32>, f32) {
    let [w, h] = image.size;
    let scale = DETECT_SIDE as f32 / w.max(h).max(1) as f32;
    let (sw, sh) = (
        ((w as f32 * scale).round() as usize).min(DETECT_SIDE),
        ((h as f32 * scale).round() as usize).min(DETECT_SIDE),
    );
    let plane = DETECT_SIDE * DETECT_SIDE;
    let mut out = vec![0.0; plane * 3];
    for y in 0..sh {
        let (y0, y1) = (y as f32 / scale, (y + 1) as f32 / scale);
        for x in 0..sw {
            let (x0, x1) = (x as f32 / scale, (x + 1) as f32 / scale);
            let [r, g, b] = area_average(image, x0, y0, x1, y1);
            let i = y * DETECT_SIDE + x;
            out[i] = b;
            out[plane + i] = g;
            out[2 * plane + i] = r;
        }
    }
    (out, scale)
}

/// 範囲 [x0, x1) x [y0, y1) にかかる画素の平均。縮めるときに細かい模様が
/// 取りこぼされないよう、最寄りの画素ではなく範囲の平均を取る。
fn area_average(image: &ColorImage, x0: f32, y0: f32, x1: f32, y1: f32) -> [f32; 3] {
    let [w, h] = image.size;
    let (ix0, iy0) = (x0 as usize, y0 as usize);
    let ix1 = (x1.ceil() as usize).clamp(ix0 + 1, w);
    let iy1 = (y1.ceil() as usize).clamp(iy0 + 1, h);
    let mut sum = [0.0f32; 3];
    let mut n = 0.0;
    for sy in iy0.min(h - 1)..iy1 {
        for sx in ix0.min(w - 1)..ix1 {
            let c = image.pixels[sy * w + sx];
            sum[0] += c.r() as f32;
            sum[1] += c.g() as f32;
            sum[2] += c.b() as f32;
            n += 1.0;
        }
    }
    sum.map(|v| v / n)
}

/// YuNet の出力（ストライドごとの cls, obj, bbox, kps）から顔を取り出す。
/// `scale` は [`detect_input`] の倍率で、座標を元の画素に戻すのに使う。
pub fn decode(outputs: &[[&[f32]; 4]; 3], scale: f32) -> Vec<Face> {
    let mut faces = Vec::new();
    for (s, &stride) in STRIDES.iter().enumerate() {
        let [cls, obj, bbox, kps] = outputs[s];
        let cols = DETECT_SIDE / stride;
        for (idx, (&c, &o)) in cls.iter().zip(obj).enumerate() {
            let score = (c.clamp(0.0, 1.0) * o.clamp(0.0, 1.0)).sqrt();
            if score < SCORE_THRESHOLD {
                continue;
            }
            let (col, row) = ((idx % cols) as f32, (idx / cols) as f32);
            let st = stride as f32;
            let b = &bbox[idx * 4..idx * 4 + 4];
            let (cx, cy) = ((col + b[0]) * st, (row + b[1]) * st);
            let (w, h) = (b[2].exp() * st, b[3].exp() * st);
            let k = &kps[idx * 10..idx * 10 + 10];
            let landmarks = std::array::from_fn(|n| {
                [
                    (k[2 * n] + col) * st / scale,
                    (k[2 * n + 1] + row) * st / scale,
                ]
            });
            faces.push(Face {
                rect: [
                    (cx - w / 2.0) / scale,
                    (cy - h / 2.0) / scale,
                    w / scale,
                    h / scale,
                ],
                landmarks,
                score,
            });
        }
    }
    nms(faces)
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union > 0.0 {
        inter / union
    } else {
        0.0
    }
}

/// 確からしさの高い順に残し、それと重なりすぎる枠を捨てる。
fn nms(mut faces: Vec<Face>) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::new();
    for f in faces {
        if kept.iter().all(|k| iou(&k.rect, &f.rect) <= NMS_THRESHOLD) {
            kept.push(f);
        }
    }
    kept
}

/// 5 点 `src` を `dst` に重ねる相似変換（回転・拡大・平行移動）を最小二乗で求める。
/// 戻り値は [a, -b, tx, b, a, ty]（`x' = a x - b y + tx`, `y' = b x + a y + ty`）。
pub fn similarity(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> [f32; 6] {
    let mean = |p: &[[f32; 2]; 5]| {
        let s = p
            .iter()
            .fold([0.0, 0.0], |acc, q| [acc[0] + q[0], acc[1] + q[1]]);
        [s[0] / 5.0, s[1] / 5.0]
    };
    let (ms, md) = (mean(src), mean(dst));
    let (mut num_a, mut num_b, mut den) = (0.0, 0.0, 0.0);
    for (s, d) in src.iter().zip(dst) {
        let (sx, sy) = (s[0] - ms[0], s[1] - ms[1]);
        let (dx, dy) = (d[0] - md[0], d[1] - md[1]);
        num_a += sx * dx + sy * dy;
        num_b += sx * dy - sy * dx;
        den += sx * sx + sy * sy;
    }
    let (a, b) = if den > 0.0 {
        (num_a / den, num_b / den)
    } else {
        (1.0, 0.0)
    };
    let tx = md[0] - (a * ms[0] - b * ms[1]);
    let ty = md[1] - (b * ms[0] + a * ms[1]);
    [a, -b, tx, b, a, ty]
}

/// 顔を目・鼻・口の位置で 112 x 112 に切り出し、SFace の入力（RGB、0〜255、NCHW）にする。
/// 画像の外は黒。
pub fn align_input(image: &ColorImage, face: &Face) -> Vec<f32> {
    let m = similarity(&face.landmarks, &TEMPLATE);
    // 出力の画素から元の画素へ戻す逆変換。
    let det = m[0] * m[4] - m[1] * m[3];
    let inv = if det.abs() > f32::EPSILON {
        [m[4] / det, -m[1] / det, -m[3] / det, m[0] / det]
    } else {
        [1.0, 0.0, 0.0, 1.0]
    };
    let plane = ALIGN_SIDE * ALIGN_SIDE;
    let mut out = vec![0.0; plane * 3];
    for v in 0..ALIGN_SIDE {
        for u in 0..ALIGN_SIDE {
            let (du, dv) = (u as f32 - m[2], v as f32 - m[5]);
            let x = inv[0] * du + inv[1] * dv;
            let y = inv[2] * du + inv[3] * dv;
            let [r, g, b] = bilinear(image, x, y);
            let i = v * ALIGN_SIDE + u;
            out[i] = r;
            out[plane + i] = g;
            out[2 * plane + i] = b;
        }
    }
    out
}

fn bilinear(image: &ColorImage, x: f32, y: f32) -> [f32; 3] {
    let [w, h] = image.size;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |px: f32, py: f32| -> [f32; 3] {
        if px < 0.0 || py < 0.0 || px >= w as f32 || py >= h as f32 {
            return [0.0; 3];
        }
        let c: Color32 = image.pixels[py as usize * w + px as usize];
        [c.r() as f32, c.g() as f32, c.b() as f32]
    };
    let (p00, p10, p01, p11) = (
        at(x0, y0),
        at(x0 + 1.0, y0),
        at(x0, y0 + 1.0),
        at(x0 + 1.0, y0 + 1.0),
    );
    std::array::from_fn(|i| {
        let top = p00[i] * (1.0 - fx) + p10[i] * fx;
        let bottom = p01[i] * (1.0 - fx) + p11[i] * fx;
        top * (1.0 - fy) + bottom * fy
    })
}

/// 検出した顔と、その特徴ベクトル（長さ 1）と、目の位置で合わせた顔の縮小画像。
#[derive(Debug, Clone)]
pub struct Found {
    pub face: Face,
    pub feature: Vec<f32>,
    pub thumb: Vec<u8>,
}

/// 縮小画像の一辺。人物の一覧に並べる大きさ。
pub const THUMB_SIDE: usize = ALIGN_SIDE / 2;

/// [`align_input`] の顔を半分に縮めた RGB の並び。
pub fn thumb(aligned: &[f32]) -> Vec<u8> {
    let plane = ALIGN_SIDE * ALIGN_SIDE;
    let mut out = Vec::with_capacity(THUMB_SIDE * THUMB_SIDE * 3);
    for y in 0..THUMB_SIDE {
        for x in 0..THUMB_SIDE {
            for c in 0..3 {
                let at = |dx: usize, dy: usize| {
                    aligned[c * plane + (2 * y + dy) * ALIGN_SIDE + 2 * x + dx]
                };
                let v = (at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) / 4.0;
                out.push(v.round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    out
}

/// 読み込んだ 2 つのモデル。重いので一度読んだら持ち続ける。
pub struct FaceModels {
    detector: Session,
    recognizer: Session,
    recognizer_input: String,
}

impl FaceModels {
    pub fn load(
        runtime: &Path,
        detector: &Path,
        recognizer: &Path,
        threads: usize,
    ) -> Result<Self, String> {
        let open = |model: &Path| -> Result<Session, String> {
            let env = crate::tagger::environment(runtime)?;
            let mut builder = Session::builder(&env).map_err(|e| e.to_string())?;
            if threads > 0 {
                builder = builder
                    .with_intra_threads(threads)
                    .map_err(|e| e.to_string())?;
            }
            builder
                .commit_from_file(model)
                .map_err(|e| format!("{}: {e}", model.display()))
        };
        let detector = open(detector)?;
        let recognizer = open(recognizer)?;
        // 古い形式の ONNX なので、重みまで入力の一覧に並ぶ。画像は最初の入力。
        let recognizer_input = recognizer
            .inputs()
            .first()
            .ok_or("face recognizer has no input")?
            .name()
            .to_owned();
        Ok(Self {
            detector,
            recognizer,
            recognizer_input,
        })
    }

    pub fn detect(&mut self, image: &ColorImage) -> Result<Vec<Face>, String> {
        let (data, scale) = detect_input(image);
        let side = DETECT_SIDE as i64;
        let tensor =
            Tensor::from_array(([1i64, 3, side, side], data)).map_err(|e| e.to_string())?;
        let outputs = self
            .detector
            .run(ort::inputs!["input" => tensor])
            .map_err(|e| e.to_string())?;
        let mut owned: Vec<Vec<f32>> = Vec::new();
        for s in STRIDES {
            for kind in ["cls", "obj", "bbox", "kps"] {
                let name = format!("{kind}_{s}");
                let value = outputs
                    .get(&name)
                    .ok_or_else(|| format!("face detector gave no {name}"))?;
                let (_, v) = value
                    .try_extract_tensor::<f32>()
                    .map_err(|e| e.to_string())?;
                owned.push(v.to_vec());
            }
        }
        let parts: [[&[f32]; 4]; 3] =
            std::array::from_fn(|s| std::array::from_fn(|k| owned[s * 4 + k].as_slice()));
        Ok(decode(&parts, scale))
    }

    #[cfg(test)]
    pub fn feature(&mut self, image: &ColorImage, face: &Face) -> Result<Vec<f32>, String> {
        self.feature_of_aligned(align_input(image, face))
    }

    fn feature_of_aligned(&mut self, aligned: Vec<f32>) -> Result<Vec<f32>, String> {
        let side = ALIGN_SIDE as i64;
        let tensor =
            Tensor::from_array(([1i64, 3, side, side], aligned)).map_err(|e| e.to_string())?;
        let outputs = self
            .recognizer
            .run(ort::inputs![self.recognizer_input.as_str() => tensor])
            .map_err(|e| e.to_string())?;
        let (_, v) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        Ok(normalized(v.to_vec()))
    }

    pub fn find(&mut self, image: &ColorImage) -> Result<Vec<Found>, String> {
        let faces = self.detect(image)?;
        faces
            .into_iter()
            .map(|face| {
                let aligned = align_input(image, &face);
                let thumb = thumb(&aligned);
                let feature = self.feature_of_aligned(aligned)?;
                Ok(Found {
                    face,
                    feature,
                    thumb,
                })
            })
            .collect()
    }
}

/// 読んだモデルの置き場。仕事をまたいで使い回す。
pub fn shared() -> &'static Mutex<Option<FaceModels>> {
    static MODELS: Mutex<Option<FaceModels>> = Mutex::new(None);
    &MODELS
}

/// 枠を画像の幅・高さに対する割合にする。
pub fn mark(face: &Face, size: [usize; 2]) -> FaceMark {
    let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
    let r = face.rect;
    FaceMark {
        rect: [r[0] / w, r[1] / h, r[2] / w, r[3] / h],
        score: face.score,
        person: None,
        similarity: 0.0,
        unlinked: false,
        hidden: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_image(path: &std::path::Path) -> ColorImage {
        let bytes = std::fs::read(path).unwrap();
        let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let pixels = bytes[8..]
            .chunks(3)
            .map(|c| Color32::from_rgb(c[0], c[1], c[2]))
            .collect();
        ColorImage {
            size: [w, h],
            pixels,
            ..Default::default()
        }
    }

    /// 本物のモデルで、OpenCV の FaceDetectorYN / FaceRecognizerSF と同じ結果になるか。
    /// `VW_TEST_MODELS=<models> VW_TEST_FACES=<参照画像> cargo test -- --ignored matches_opencv`
    /// 参照画像と OpenCV の結果は、プロジェクトの tools/make_face_reference.py で作る。
    #[test]
    #[ignore]
    fn matches_opencv_on_the_reference_images() {
        let models = std::path::PathBuf::from(std::env::var("VW_TEST_MODELS").unwrap());
        let refs = std::path::PathBuf::from(std::env::var("VW_TEST_FACES").unwrap());
        let store = crate::ai_models::Store { dir: models };
        let mut m = FaceModels::load(
            &store.runtime(),
            &store.face_detector(),
            &store.face_recognizer(),
            2,
        )
        .unwrap();
        let image = raw_image(&refs.join("astronaut.rgb"));
        let found = m.find(&image).unwrap();
        assert_eq!(found.len(), 1);
        // OpenCV: 178.17, 62.91, 89.9, 113.33（入力の縮め方が違うので数画素の差は許す）
        let r = found[0].face.rect;
        for (a, b) in r.iter().zip([178.17, 62.91, 89.9, 113.33]) {
            assert!((a - b).abs() < 4.0, "{r:?}");
        }
        let opencv: Vec<f32> = std::fs::read(refs.join("feat_astronaut.f32"))
            .unwrap()
            .chunks(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let opencv = normalized(opencv);
        // 目・鼻・口の位置を OpenCV と同じにすれば、切り出しと特徴もほぼ同じになる。
        let opencv_face = Face {
            rect: [178.17, 62.91, 89.9, 113.33],
            landmarks: [
                [200.12, 101.82],
                [247.13, 102.62],
                [220.37, 124.54],
                [198.61, 142.77],
                [239.67, 143.64],
            ],
            score: 0.93,
        };
        let aligned_like_opencv = cosine(&m.feature(&image, &opencv_face).unwrap(), &opencv);
        assert!(aligned_like_opencv > 0.99, "{aligned_like_opencv}");
        // 自前の検出の位置は少しずれるが、同じ人と言える範囲。
        let same_as_opencv = cosine(&found[0].feature, &opencv);
        assert!(same_as_opencv > 0.9, "{same_as_opencv}");
        let small = m
            .find(&raw_image(&refs.join("astronaut_small.rgb")))
            .unwrap();
        assert_eq!(small.len(), 1);
        let same_person = cosine(&found[0].feature, &small[0].feature);
        assert!(same_person > 0.6, "{same_person}");
        assert!(m
            .find(&raw_image(&refs.join("coffee.rgb")))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn similarity_recovers_a_known_transform() {
        // 2 倍、90 度回転、(10, 20) 移動。
        let src = TEMPLATE;
        let dst = src.map(|[x, y]| [-2.0 * y + 10.0, 2.0 * x + 20.0]);
        let m = similarity(&src, &dst);
        let expect = [0.0, -2.0, 10.0, 2.0, 0.0, 20.0];
        for (a, b) in m.iter().zip(expect) {
            assert!((a - b).abs() < 1e-3, "{m:?}");
        }
    }

    #[test]
    fn overlapping_boxes_keep_only_the_most_confident() {
        let f = |x: f32, score: f32| Face {
            rect: [x, 0.0, 10.0, 10.0],
            landmarks: [[0.0; 2]; 5],
            score,
        };
        let kept = nms(vec![f(0.0, 0.92), f(1.0, 0.95), f(50.0, 0.91)]);
        let scores: Vec<f32> = kept.iter().map(|k| k.score).collect();
        assert_eq!(scores, [0.95, 0.91]);
    }

    #[test]
    fn decode_maps_a_cell_back_to_image_pixels() {
        let cells8 = (DETECT_SIDE / 8).pow(2);
        let cells16 = (DETECT_SIDE / 16).pow(2);
        let cells32 = (DETECT_SIDE / 32).pow(2);
        let mut cls8 = vec![0.0; cells8];
        let obj8 = vec![1.0; cells8];
        let mut bbox8 = vec![0.0; cells8 * 4];
        let kps8 = vec![0.0; cells8 * 10];
        // 行 2、列 3 のセルに、幅・高さがストライド 1 個分（exp(0) * 8）の顔。
        let idx = 2 * (DETECT_SIDE / 8) + 3;
        cls8[idx] = 1.0;
        bbox8[idx * 4] = 0.5;
        bbox8[idx * 4 + 1] = 0.5;
        let zeros = |n: usize| vec![0.0; n];
        let (c16, o16, b16, k16) = (
            zeros(cells16),
            zeros(cells16),
            zeros(cells16 * 4),
            zeros(cells16 * 10),
        );
        let (c32, o32, b32, k32) = (
            zeros(cells32),
            zeros(cells32),
            zeros(cells32 * 4),
            zeros(cells32 * 10),
        );
        let parts = [
            [cls8.as_slice(), &obj8, &bbox8, &kps8],
            [c16.as_slice(), &o16, &b16, &k16],
            [c32.as_slice(), &o32, &b32, &k32],
        ];
        // 倍率 0.5 = 元の画像は 1280 画素。
        let faces = decode(&parts, 0.5);
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].rect, [48.0, 32.0, 16.0, 16.0]);
        assert_eq!(faces[0].landmarks[0], [48.0, 32.0]);
    }

    #[test]
    fn detect_input_letterboxes_into_the_top_left() {
        let image = ColorImage {
            size: [2, 1],
            pixels: vec![Color32::from_rgb(10, 20, 30); 2],
            ..Default::default()
        };
        let (data, scale) = detect_input(&image);
        assert_eq!(scale, 320.0);
        let plane = DETECT_SIDE * DETECT_SIDE;
        assert_eq!((data[0], data[plane], data[2 * plane]), (30.0, 20.0, 10.0));
        // 320 行目から下は余白。
        assert_eq!(data[320 * DETECT_SIDE], 0.0);
    }

    #[test]
    fn cosine_of_normalized_vectors() {
        let a = normalized(vec![3.0, 4.0]);
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-6);
        assert!(cosine(&a, &normalized(vec![-4.0, 3.0])).abs() < 1e-6);
    }
}
