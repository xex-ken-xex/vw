//! 貼り合わせた画像。貼り合わせのウインドウ（[`crate::stitch_window`]）で決めた置き方を覚え、
//! スロットに出すときに元のファイルを読んで 1 枚に重ねる。
//!
//! 貼り合わせた画像もパスで表す（[`crate::compose`] と同じ理由）。スロット・全画面・コピーが、
//! 普通の画像と同じ読み込みの道を通れるようにするため。置き方は何十枚ぶんにもなるので
//! パスには書かず、このプロセスの中の表に置いて番号で引く。ファイルには書かない
//! （利用者が保存の場所を選んだときだけ書く）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use vw_compare::Affine;
use vw_compare::mosaic::{self, Blend, Canvas, Grid, Layer};
use vw_core::{ColorModel, Curve, Metadata, PixelData, Registry, VwImage};

/// メタデータの見出しと、読み込みの結果に付けるプラグインの名前。
pub const GROUP: &str = "Stitching";
pub const PLUGIN: &str = "Stitching";

/// 区切り。`?` は Windows のファイル名に使えないので、実在のファイルと取り違えない。
const MARK: &str = "?stitch=";

/// どうやって置いたか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// 置く基準にした 1 枚。
    Reference,
    /// ヘッダの WCS から。
    Wcs,
    /// 重なった所の絵を合わせて。
    Matched,
    /// 利用者が大まかに置いた所から、絵を合わせて。
    ByHand,
}

impl How {
    pub fn title(self) -> &'static str {
        match self {
            How::Reference => "Reference",
            How::Wcs => "By WCS",
            How::Matched => "By matching the overlap",
            How::ByHand => "Placed by hand",
        }
    }
}

/// 値の型。元の画像の型をそろえたもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    U8,
    /// 意味のあるビット数。
    U16(u32),
    F32,
}

/// WCS のカード（FITS のヘッダから）と、画面の画素（0 始まり）から FITS の画素（1 始まり）への写し方。
pub type Wcs = (Vec<(String, String)>, Affine);

/// 貼り合わせる 1 枚。
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub path: PathBuf,
    pub size: [usize; 2],
    /// 元の画素の座標（0 始まり、画面の向き）から、貼り合わせの座標への写し方。
    pub placed: Affine,
    pub how: How,
    /// 置くのに使った組の相関（WCS や基準なら `None`）。
    pub correlation: Option<f64>,
    /// 値に掛ける比（明るさをそろえないときは 1）。
    pub gain: f64,
    /// WCS のカード（FITS のヘッダから）と、画面の画素から FITS の画素（1 始まり）への写し方。
    pub wcs: Option<Wcs>,
}

/// 貼り合わせの置き方。
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// 使う画像を、重ねる順に。
    pub sources: Vec<Source>,
    pub blend: Blend,
    /// 明るさをそろえたか。
    pub matched_brightness: bool,
    /// 出す画像の画素の数の上限。
    pub max_pixels: usize,
    /// 色の画像があるか（なければ灰色で出す）。
    pub color: bool,
    pub depth: Depth,
    /// 表示の黒と白と曲線（基準の 1 枚のもの。同じ撮り方なら値の目盛りは同じ）。
    pub window: Option<(f32, f32)>,
    pub curve: Curve,
}

impl Plan {
    pub fn grid(&self) -> Option<Grid> {
        let sizes: Vec<[usize; 2]> = self.sources.iter().map(|s| s.size).collect();
        let placed: Vec<Option<Affine>> = self.sources.iter().map(|s| Some(s.placed)).collect();
        Grid::covering(mosaic::bounds(&sizes, &placed)?, self.max_pixels)
    }

    /// 出す画像の WCS のカード。WCS のある元の画像から引き継ぐ（基準の 1 枚を先に見る）。
    pub fn wcs(&self, grid: &Grid) -> Option<(Vec<(String, String)>, &Source)> {
        let mut order: Vec<&Source> = self.sources.iter().collect();
        order.sort_by_key(|s| s.how != How::Reference);
        order.into_iter().find_map(|s| {
            let (cards, to_fits) = s.wcs.as_ref()?;
            // 出す画像の FITS の画素（1 始まり）→ 画面の画素 → 貼り合わせの座標 → 元の画面の画素
            // → 元の FITS の画素。
            let one_based = Affine {
                c: -1.0,
                f: -1.0,
                ..Affine::IDENTITY
            };
            let map = one_based
                .then(&grid.to_mosaic)
                .then(&s.placed.inverse()?)
                .then(to_fits);
            let lookup = lookup(cards);
            Some((vw_fits::wcs_through(&lookup, rows(&map))?, s))
        })
    }
}

/// カードの並びを引く。
pub fn lookup(cards: &[(String, String)]) -> impl Fn(&str) -> Option<String> + '_ {
    move |key| cards.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// [`Affine`] を、vw-fits の `[[a, b, c], [d, e, f]]` にする。
pub fn rows(t: &Affine) -> [[f64; 3]; 2] {
    [[t.a, t.b, t.c], [t.d, t.e, t.f]]
}

/// 置き方を覚える表。番号は増える一方で、使い終えた置き方も起動している間は残す
/// （スロットや「最近」の一覧からいつ引かれるか分からないため。置き方は数 KB しかない）。
static PLANS: Mutex<Vec<Arc<Plan>>> = Mutex::new(Vec::new());

/// `plan` を覚えて、それを指すパスを返す。先頭は最初の画像（フォルダやスロットの見出しに使われる）。
pub fn remember(plan: Plan) -> Option<PathBuf> {
    let first = plan.sources.first()?.path.clone();
    let mut plans = PLANS.lock().unwrap_or_else(|e| e.into_inner());
    plans.push(Arc::new(plan));
    let mut s = first.into_os_string();
    s.push(format!("{MARK}{}", plans.len() - 1));
    Some(PathBuf::from(s))
}

/// 貼り合わせを指すパスなら、その置き方。
pub fn split(path: &Path) -> Option<Arc<Plan>> {
    let s = path.to_str()?;
    let (_, number) = s.rsplit_once(MARK)?;
    let i: usize = number.parse().ok()?;
    PLANS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(i)
        .cloned()
}

/// 貼り合わせを指すパスか（置き方を覚えていなくても）。
pub fn is_stitched(path: &Path) -> bool {
    path.to_str().is_some_and(|s| s.contains(MARK))
}

/// 画面に出す名前（「M31-01 +5」。最初の画像と、ほかの枚数）。貼り合わせでなければ `None`。
/// 言葉を入れないのは、スロットの見出しなど言語を知らない所でも同じ名前で出すため。
pub fn title(path: &Path) -> Option<String> {
    let plan = split(path)?;
    let first = plan.sources.first()?;
    let stem = crate::frames::file_of(&first.path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(format!("{stem} +{}", plan.sources.len() - 1))
}

/// 元のファイルを読んで重ねる。返すのは画像と、読んだバイト数。
pub fn render(plan: &Plan, registry: &Registry) -> Result<(VwImage, usize), String> {
    let grid = plan.grid().ok_or_else(|| "nothing to stitch".to_owned())?;
    let channels = if plan.color { 3 } else { 1 };
    let mut canvas = Canvas::new(grid, channels, plan.blend);
    let mut read = 0;
    // 縮めて出すときは、先に元の画像を整数分の 1 に縮めて、細かい模様の折り返しを防ぐ。
    let shrink = (grid.step().floor() as usize).max(1);
    for s in &plan.sources {
        let (file, frame) = crate::frames::split(&s.path).unwrap_or((s.path.clone(), 0));
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        read += bytes.len();
        let (image, _) = registry
            .decode_frame_bytes(&file, &bytes, frame)
            .map_err(|e| format!("{}: {e}", file.display()))?;
        drop(bytes);
        let (w, h) = (image.width as usize, image.height as usize);
        if [w, h] != s.size {
            return Err(format!(
                "{}: the size changed since it was aligned",
                file.display()
            ));
        }
        let values = layer_values(&image, channels);
        drop(image);
        let (data, lw, lh, placed) = if shrink > 1 {
            let (d, lw, lh) = shrunk(&values, w, h, channels, shrink);
            let k = shrink as f64;
            let from_small = Affine {
                a: k,
                b: 0.0,
                c: 0.5 * k - 0.5,
                d: 0.0,
                e: k,
                f: 0.5 * k - 0.5,
            };
            (d, lw, lh, from_small.then(&s.placed))
        } else {
            (values, w, h, s.placed)
        };
        canvas.add(&Layer {
            width: lw,
            height: lh,
            channels,
            data: &data,
            placed,
            gain: s.gain as f32,
        });
    }
    let values = canvas.finish();
    let mut image = output(plan, &grid, channels, values)?;
    image.metadata = metadata(plan, &grid);
    Ok((image, read))
}

/// 重ねる値。色の数を `channels` にそろえ、アルファが 0 の所は NaN（写っていない）にする。
fn layer_values(img: &VwImage, channels: usize) -> Vec<f32> {
    let n = img.color.channels();
    let alpha = matches!(img.color, ColorModel::GrayAlpha | ColorModel::Rgba);
    let value = |i: usize| -> f32 {
        match &img.data {
            PixelData::U8(v) => f32::from(v[i]),
            PixelData::U16(v) => f32::from(v[i]),
            PixelData::F32(v) => v[i],
        }
    };
    let pixels = img.width as usize * img.height as usize;
    let mut out = Vec::with_capacity(pixels * channels);
    for p in 0..pixels {
        let base = p * n;
        let hidden = alpha && value(base + n - 1) == 0.0;
        let colors = if alpha { n - 1 } else { n };
        for c in 0..channels {
            out.push(if hidden {
                f32::NAN
            } else {
                value(base + c.min(colors - 1))
            });
        }
    }
    out
}

/// 面積の平均で `k` 分の 1 に縮める（写っていない画素は数えない）。
fn shrunk(data: &[f32], w: usize, h: usize, ch: usize, k: usize) -> (Vec<f32>, usize, usize) {
    let (sw, sh) = (w.div_ceil(k), h.div_ceil(k));
    let mut out = vec![0.0f32; sw * sh * ch];
    for sy in 0..sh {
        for sx in 0..sw {
            for c in 0..ch {
                let (mut sum, mut count) = (0.0f32, 0u32);
                for y in sy * k..((sy + 1) * k).min(h) {
                    for x in sx * k..((sx + 1) * k).min(w) {
                        let v = data[(y * w + x) * ch + c];
                        if v.is_finite() {
                            sum += v;
                            count += 1;
                        }
                    }
                }
                out[(sy * sw + sx) * ch + c] = if count > 0 {
                    sum / count as f32
                } else {
                    f32::NAN
                };
            }
        }
    }
    (out, sw, sh)
}

/// 重ねた値を、元の型の画像にする。整数の型ではアルファで写っていない所を表す。
fn output(plan: &Plan, grid: &Grid, channels: usize, values: Vec<f32>) -> Result<VwImage, String> {
    let (w, h) = (grid.width as u32, grid.height as u32);
    let with_alpha = |max: f32| -> Vec<f32> {
        values
            .chunks_exact(channels)
            .flat_map(|px| {
                let covered = px[0].is_finite();
                px.iter()
                    .map(move |&v| {
                        if covered {
                            v.round().clamp(0.0, max)
                        } else {
                            0.0
                        }
                    })
                    .chain(std::iter::once(if covered { max } else { 0.0 }))
            })
            .collect()
    };
    let (model, data, bits) = match plan.depth {
        Depth::F32 => (
            if channels == 3 {
                ColorModel::Rgb
            } else {
                ColorModel::Gray
            },
            PixelData::F32(values),
            32,
        ),
        Depth::U8 => (
            alpha_model(channels),
            PixelData::U8(with_alpha(255.0).into_iter().map(|v| v as u8).collect()),
            8,
        ),
        Depth::U16(bits) => {
            let top = ((1u32 << bits.clamp(1, 16)) - 1) as f32;
            let mut data = with_alpha(top);
            // アルファは表示が 16bit の全範囲で見るので、値の上限ではなく 65535 にする。
            for px in data.chunks_exact_mut(channels + 1) {
                if px[channels] > 0.0 {
                    px[channels] = 65535.0;
                }
            }
            (
                alpha_model(channels),
                PixelData::U16(data.into_iter().map(|v| v as u16).collect()),
                bits,
            )
        }
    };
    let mut image = VwImage::new(w, h, model, data)?;
    image.significant_bits = bits;
    image.window = plan.window;
    image.curve = plan.curve;
    Ok(image)
}

fn alpha_model(channels: usize) -> ColorModel {
    if channels == 3 {
        ColorModel::Rgba
    } else {
        ColorModel::GrayAlpha
    }
}

/// メタデータの鍵。
pub const IMAGES: &str = "Images";
pub const SIZE: &str = "Size";
pub const SCALE: &str = "Scale";
pub const BLEND: &str = "Blend";
pub const BRIGHTNESS: &str = "Brightness";
pub const WCS: &str = "WCS";

pub fn blend_title(blend: Blend) -> &'static str {
    match blend {
        Blend::Feather => "Feathered toward the edges",
        Blend::Top => "Later images on top",
    }
}

/// 貼り合わせの内訳。元の画像ごとに、出した画像のどこに置いたか（左上の位置、幅と高さ、
/// 回転と倍率）と、どうやって置いたか。
pub fn metadata(plan: &Plan, grid: &Grid) -> Metadata {
    let mut m = Metadata::default();
    m.push(GROUP, IMAGES, plan.sources.len().to_string());
    m.push(GROUP, SIZE, format!("{} x {}", grid.width, grid.height));
    let step = grid.step();
    m.push(
        GROUP,
        SCALE,
        if step <= 1.0 + 1e-9 {
            "1:1".to_owned()
        } else {
            format!("1/{step:.2}")
        },
    );
    m.push(GROUP, BLEND, blend_title(plan.blend));
    m.push(
        GROUP,
        BRIGHTNESS,
        if plan.matched_brightness {
            "Matched in the overlaps"
        } else {
            "As in the files"
        },
    );
    if let Some((_, from)) = plan.wcs(grid) {
        m.push(GROUP, WCS, crate::compare::file_name(&from.path));
    }
    for s in &plan.sources {
        m.push(
            GROUP,
            crate::compare::file_name(&s.path),
            placement_text(s, grid),
        );
    }
    m
}

/// 値と、どう置いたかの区切り。メタデータの欄で、後ろの文だけを訳すのに使う。
pub const HOW_MARK: &str = " · ";

/// 「x 120, y 40, 1024 x 768, 0.30°, ×1.003, r 0.93 · By matching the overlap」。位置は出した画像の
/// 画素で左上の角、角度は時計回り、r は重なった所の相関。
pub fn placement_text(s: &Source, grid: &Grid) -> String {
    let Some(to_out) = grid.to_mosaic.inverse() else {
        return String::new();
    };
    let t = s.placed.then(&to_out);
    let corners = mosaic::corners(s.size).map(|c| t.apply(c));
    let left = corners.iter().map(|c| c[0]).fold(f64::MAX, f64::min) + 0.5;
    let top = corners.iter().map(|c| c[1]).fold(f64::MAX, f64::min) + 0.5;
    let size = [s.size[0] as f64, s.size[1] as f64];
    let parts = t.parts(size, size);
    let mut text = format!(
        "x {}, y {}, {} x {}, {}°, ×{:.3}",
        fixed(left, 0),
        fixed(top, 0),
        s.size[0],
        s.size[1],
        fixed(parts.rotation, 2),
        parts.scale,
    );
    if let Some(c) = s.correlation {
        text.push_str(&format!(", r {c:.2}"));
    }
    text.push_str(HOW_MARK);
    text.push_str(s.how.title());
    text
}

/// 小数点以下 `decimals` 桁。丸めると 0 になる負の数を「-0」と書かない。
fn fixed(v: f64, decimals: usize) -> String {
    let text = format!("{v:.decimals$}");
    if text.starts_with('-') && text[1..].chars().all(|c| c == '0' || c == '.') {
        text[1..].to_owned()
    } else {
        text
    }
}

/// 星図に出す、貼り合わせた範囲（外枠）と、元の画像ごとの枠。出した画像に WCS がなければ空。
pub fn sky_items(plan: &Plan, path: &Path) -> Vec<crate::sky_window::Item> {
    let Some(grid) = plan.grid() else {
        return Vec::new();
    };
    let Some((cards, _)) = plan.wcs(&grid) else {
        return Vec::new();
    };
    let with_size = |cards: &[(String, String)], size: [usize; 2]| {
        let mut all = cards.to_vec();
        all.push(("NAXIS1".into(), size[0].to_string()));
        all.push(("NAXIS2".into(), size[1].to_string()));
        all
    };
    let mut items = Vec::new();
    let whole = with_size(&cards, [grid.width, grid.height]);
    if let Some(sky) = vw_fits::sky(&lookup(&whole)) {
        items.push(crate::sky_window::Item {
            path: path.to_path_buf(),
            sky,
            object: None,
            taken: None,
            kind: crate::sky_window::Kind::Stitched,
        });
    }
    let Some(to_out) = grid.to_mosaic.inverse() else {
        return items;
    };
    for s in &plan.sources {
        // 元の画面の画素（1 始まり）→ 0 始まり → 出した画像の画素 → 出した画像の 1 始まり。
        let shift = |v: f64| Affine {
            c: v,
            f: v,
            ..Affine::IDENTITY
        };
        let map = shift(-1.0).then(&s.placed).then(&to_out).then(&shift(1.0));
        let Some(own) = vw_fits::wcs_through(&lookup(&cards), rows(&map)) else {
            continue;
        };
        let own = with_size(&own, s.size);
        if let Some(sky) = vw_fits::sky(&lookup(&own)) {
            items.push(crate::sky_window::Item {
                path: s.path.clone(),
                sky,
                object: None,
                taken: None,
                kind: crate::sky_window::Kind::Part,
            });
        }
    }
    items
}

#[cfg(test)]
mod tests;
