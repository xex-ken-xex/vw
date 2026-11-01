//! スロットごとの色導出。保存したパイプライン（または色導出のウインドウで変えた値）で
//! スロットの画像を描き直し、標準の表示と並べたり、差を色で見せたりする。
//!
//! 描くのは裏のスレッドで、1 スロットにつき 1 つずつ。値を変えている間も前の結果を出したままにして、
//! 描き終わるまで標準の表示に戻らないようにする（戻すと値を動かすたびにちらつくため）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use vw_core::VwImage;
use vw_dicom::pipeline::{Gray, Pipeline};

use crate::nodes;
use crate::pipelines::{Preset, Values};
use crate::pixel_pipeline::{self, Decoded, Mismatch, Params};
use crate::viewer::Shown;

/// スロットの画像部分に何を出すか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Derived,
    /// 左が標準、右が色導出。
    Compare,
    /// |色導出 − 標準| のヒートマップ。
    Difference,
}

/// ファイルの段（ヘッダ）を読んだ結果。画像ごとに 1 回だけ読む。
enum Header {
    /// DICOM のグレースケール。ファイルの段の値も変えられる。
    Gray(Box<Gray>),
    /// それ以外（DICOM 以外の形式、カラーの DICOM）。標準の表示の後ろにノードだけ当てる。
    Plain,
    /// ファイルの値で描くと標準の表示と食い違う。標準のまま出す。
    Refused,
    Failed,
}

/// 描いた結果が、どの画像のどの値のものか。
#[derive(Debug, Clone, PartialEq)]
struct Key {
    path: PathBuf,
    /// フル解像度の画像（捨てて読み直されたら別のものになる）。
    image: usize,
    values: Values,
    size: [usize; 2],
    /// 色の表の段の表。ノードの後ろで、標準の表示と同じ表を引く。
    palette: Option<Arc<vw_core::Palette>>,
    /// スロットのゲージで変えた黒と白。ファイルの VOI の代わりに使う。
    window: Option<(f32, f32)>,
    /// 値を動かしている間の、表示の大きさに縮めた画像での下描き。
    preview: bool,
}

struct Rendered {
    /// フル解像度の色導出（解析ウインドウ用）。
    full: Arc<ColorImage>,
    /// `full` の明るさを 16bit の値から求めたもの（定量評価用、[`crate::quality::luma_of_derived`]）。
    luma: Arc<Vec<f32>>,
    derived: ColorImage,
    difference: ColorImage,
    max: u8,
    checked: Result<(), Mismatch>,
    /// ノードが当てられなかった（式の誤りなど）。
    error: Option<(usize, String)>,
    notes: Vec<(usize, vw_pipeline::nodes::Note)>,
}

pub struct Ready {
    key: Key,
    /// フル解像度の色導出。下描きのときは表示の大きさしかない（[`Ready::preview`]）。
    pub full: Arc<ColorImage>,
    /// `full` の明るさ（16bit の値から）。定量評価に使う。
    pub luma: Arc<Vec<f32>>,
    /// 値を動かしている間の、縮めた画像での下描き。コピーや定量評価には使わない。
    pub preview: bool,
    pub derived: TextureHandle,
    pub difference: TextureHandle,
    /// 差の最大（0..=255）。
    pub max: u8,
    /// ノードが知らせること（星から測った PSF など）。ノードの番号と知らせ。
    pub notes: Vec<(usize, vw_pipeline::nodes::Note)>,
}

/// スロットに知らせること。ログに出す。
pub enum Event {
    /// ファイルの値で描いても標準と一致しないので、この画像は標準のまま出す。
    /// `left` はまだ一度も色導出を出していなかったので、色導出をやめた。
    Refused {
        path: PathBuf,
        mismatch: Mismatch,
        left: bool,
    },
    Failed(PathBuf, String),
}

pub struct Derive {
    /// 保存したパイプラインの名前。`None` は色導出のウインドウで変えただけでまだ保存していない値。
    pub preset: Option<String>,
    pub values: Values,
    /// 保存したパイプラインを選んだあとに、色導出のウインドウで値を変えた。
    pub modified: bool,
    pub view: View,
    /// 比べるときの仕切りの位置（画像部分の幅に対する割合）。
    pub split: f32,
    headers: HashMap<PathBuf, Header>,
    reading: Option<(PathBuf, Receiver<Result<Pipeline, String>>)>,
    rendering: Option<(Key, Receiver<Rendered>)>,
    ready: Option<Ready>,
    /// ノードが当てられなかった値の組。同じ組で描き直し続けないために覚える。
    failed: Option<(Key, (usize, String))>,
    /// 一度でも色導出を出した。入るときの確かめで食い違ったら、モードに入らずにやめる。
    entered: bool,
    /// 色導出のウインドウで最後に値を変えた時刻。少しの間は重いノードを縮めた画像で下描きする。
    edited_at: Option<std::time::Instant>,
}

/// 値を変えてからこの間は、重いノードを縮めた画像で下描きする。手を止めたら全画素で描き直す。
const PREVIEW_FOR: std::time::Duration = std::time::Duration::from_millis(400);

/// 下描きの長辺の上限。スロットのテクスチャは大きな画像でも縮めないことがあるので、別に決める。
const PREVIEW_SIDE: usize = 1024;

impl Derive {
    pub fn new(preset: Option<String>, values: Values) -> Self {
        Self {
            preset,
            values,
            modified: false,
            view: View::Derived,
            split: 0.5,
            headers: HashMap::new(),
            reading: None,
            rendering: None,
            ready: None,
            failed: None,
            entered: false,
            edited_at: None,
        }
    }

    /// 1 フレーム分。ヘッダを読み、値か画像が変わっていれば描き直しを頼む。
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        shown: &Shown,
        options: TextureOptions,
    ) -> Option<Event> {
        let mut event = self.read_header(ctx, shown);
        let full = shown.full.as_ref()?;
        if let Some((key, rx)) = &self.rendering {
            let Ok(r) = rx.try_recv() else {
                return event;
            };
            let key = key.clone();
            self.rendering = None;
            match (r.checked, r.error.clone()) {
                (Err(mismatch), _) => {
                    self.headers.insert(key.path.clone(), Header::Refused);
                    event = Some(Event::Refused {
                        path: key.path,
                        mismatch,
                        left: !self.entered,
                    });
                }
                (Ok(()), Some(e)) => self.failed = Some((key, e)),
                (Ok(()), None) => {
                    self.entered = true;
                    self.failed = None;
                    self.show(ctx, key, r, options);
                }
            }
        }
        let info = match self.headers.get(&shown.path) {
            Some(Header::Gray(g)) => Some(g.clone()),
            Some(Header::Plain) => None,
            _ => return event,
        };
        let size = full.texture.size();
        let (w, h) = (full.image.width as usize, full.image.height as usize);
        let smaller = w.max(h) > PREVIEW_SIDE;
        let editing = self
            .edited_at
            .map(|at| at.elapsed())
            .filter(|&e| e < PREVIEW_FOR);
        let preview = smaller && editing.is_some() && self.values.chain.iter().any(|n| n.heavy());
        if let Some(elapsed) = editing.filter(|_| preview) {
            ctx.request_repaint_after(PREVIEW_FOR - elapsed);
        }
        let key = Key {
            path: shown.path.clone(),
            image: Arc::as_ptr(&full.image) as usize,
            values: self.values.clone(),
            size,
            palette: shown.info.look.as_ref().and_then(|a| a.palette.clone()),
            window: shown.info.look.as_ref().and_then(|a| a.slot_window()),
            preview,
        };
        let done = self.ready.as_ref().is_some_and(|r| r.key == key)
            || self.failed.as_ref().is_some_and(|(k, _)| *k == key);
        if self.rendering.is_none() && !done {
            let (image, k) = (full.image.clone(), key.clone());
            let rx = crate::background::compute(ctx, move || render(&image, info.as_deref(), &k));
            self.rendering = Some((key, rx));
        }
        event
    }

    fn read_header(&mut self, ctx: &egui::Context, shown: &Shown) -> Option<Event> {
        let path = &shown.path;
        if let Some((reading, rx)) = &self.reading
            && let Ok(result) = rx.try_recv() {
                let reading = reading.clone();
                self.reading = None;
                let (header, event) = match result {
                    Ok(Pipeline::Gray(g)) => (Header::Gray(g), None),
                    Ok(Pipeline::Color(_)) => (Header::Plain, None),
                    Err(e) => (Header::Failed, Some(Event::Failed(reading.clone(), e))),
                };
                self.headers.insert(reading, header);
                return event;
            }
        if self.headers.contains_key(path) {
            return None;
        }
        if shown.info.plugin == "DICOM" {
            if self.reading.is_none() {
                let rx = pixel_pipeline::read_in_background(ctx, path);
                self.reading = Some((path.clone(), rx));
            }
        } else {
            self.headers.insert(path.clone(), Header::Plain);
        }
        None
    }

    fn show(&mut self, ctx: &egui::Context, key: Key, r: Rendered, options: TextureOptions) {
        match &mut self.ready {
            Some(ready) => {
                ready.derived.set(r.derived, options);
                ready.difference.set(r.difference, options);
                ready.max = r.max;
                ready.full = r.full;
                ready.luma = r.luma;
                ready.preview = key.preview;
                ready.notes = r.notes;
                ready.key = key;
            }
            None => {
                self.ready = Some(Ready {
                    derived: ctx.load_texture("derived", r.derived, options),
                    difference: ctx.load_texture("derived-difference", r.difference, options),
                    max: r.max,
                    full: r.full,
                    luma: r.luma,
                    preview: key.preview,
                    notes: r.notes,
                    key,
                });
            }
        }
    }

    /// 表示中の画像 `shown` の色導出の結果。値を変えて描き直している間は前の値のものを返す。
    pub fn ready_for(&self, shown: &Shown) -> Option<&Ready> {
        let full = shown.full.as_ref()?;
        self.ready.as_ref().filter(|r| {
            r.key.path == shown.path
                && r.key.image == Arc::as_ptr(&full.image) as usize
                && matches!(
                    self.headers.get(&shown.path),
                    Some(Header::Gray(_) | Header::Plain)
                )
        })
    }

    /// `path` の画像に色導出を当てられないときの理由（i18n の鍵）。
    pub fn not_applied(&self, path: &Path) -> Option<&'static str> {
        match self.headers.get(path)? {
            Header::Gray(_) | Header::Plain => None,
            Header::Refused => Some("Standard: file values do not reproduce the standard display"),
            Header::Failed => Some("Standard: could not read the pipeline"),
        }
    }

    /// 色導出で、表示の明るさの向きが標準の表示と逆になっているか（ゲージの色の向きを合わせるため）。
    /// 「Presentation」の段の反転と、ノードの「反転」を数える。
    pub fn inverted(&self, path: &Path) -> bool {
        let presentation = match (self.headers.get(path), self.values.params) {
            (Some(Header::Gray(g)), Some(p)) => p.invert != g.presentation.invert,
            (Some(Header::Plain), _) => false,
            _ => return false,
        };
        let nodes = self
            .values
            .chain
            .iter()
            .filter(|n| matches!(n, nodes::Node::Invert))
            .count();
        presentation != (nodes % 2 == 1)
    }

    /// 描いている途中（スロットに印を出すため）。
    pub fn busy(&self) -> bool {
        self.reading.is_some()
            || self.rendering.is_some()
            || self.ready.as_ref().is_some_and(|r| r.preview)
    }

    /// 表示中の画像 `shown` の、全画素で描いた色導出（コピーや定量評価に使う）。下描きなら `None`。
    pub fn full_for(&self, shown: &Shown) -> Option<&Arc<ColorImage>> {
        self.ready_for(shown)
            .filter(|r| !r.preview)
            .map(|r| &r.full)
    }

    /// [`Derive::full_for`] の明るさ（定量評価に使う）。
    pub fn luma_for(&self, shown: &Shown) -> Option<&Arc<Vec<f32>>> {
        self.ready_for(shown)
            .filter(|r| !r.preview)
            .map(|r| &r.luma)
    }

    /// 今の値でノードが当てられなかった理由（ノードの番号と文）。
    pub fn error(&self) -> Option<&(usize, String)> {
        self.failed.as_ref().map(|(_, e)| e)
    }

    /// 解析ウインドウの物理量の軸に使う、ファイルの段の変えた値と画像のデコードの仕方。
    pub fn edit_for(&self, path: &Path) -> Option<(Params, Decoded)> {
        match self.headers.get(path) {
            Some(Header::Gray(g)) => Some((self.values.params?, Decoded::of(g))),
            _ => None,
        }
    }

    pub fn textures_mut(&mut self) -> impl Iterator<Item = &mut TextureHandle> {
        self.ready
            .iter_mut()
            .flat_map(|r| [&mut r.derived, &mut r.difference])
    }
}

/// 色導出のウインドウで値を変えた。保存したパイプラインを選んでいなければ、
/// 何も変えていない値（標準と同じ）に戻すと標準の表示に戻る。
pub fn edit(derive: &mut Option<Derive>, values: Values, presets: &[Preset]) {
    let at_file = values.is_standard();
    match derive {
        None if at_file => {}
        None => *derive = Some(Derive::new(None, values)),
        Some(d) if d.preset.is_none() && at_file => *derive = None,
        Some(d) => {
            d.values = values;
            d.modified = true;
            d.edited_at = Some(std::time::Instant::now());
            d.sync(presets);
        }
    }
}

/// 色導出のウインドウを閉じた。保存していない値を捨てる。
pub fn revert(derive: &mut Option<Derive>, presets: &[Preset]) {
    let Some(d) = derive else {
        return;
    };
    match d
        .preset
        .as_deref()
        .and_then(|n| presets.iter().find(|p| p.name == n))
    {
        Some(p) => {
            d.values = p.values.clone();
            d.modified = false;
        }
        None => *derive = None,
    }
}

impl Derive {
    /// 保存したパイプラインの一覧が変わった（保存・上書き・削除・ファイルの書き換え）ときに合わせる。
    /// 消えた名前は「未保存」にして値は残し、値を変えていなければ新しい値にする。
    pub fn sync(&mut self, presets: &[Preset]) {
        let Some(name) = &self.preset else {
            return;
        };
        match presets.iter().find(|p| &p.name == name) {
            None => {
                self.preset = None;
                self.modified = false;
            }
            Some(p) if !self.modified => self.values = p.values.clone(),
            Some(p) => self.modified = self.values != p.values,
        }
    }
}

/// スロットを持たずに（全画面のスライドショーで）、色導出の値 `values` で `image` を描く。
/// 当てられないとき（ファイルの値で標準の表示が再現できない、段が読めない、ノードの誤り）は
/// `None` で、そのときは標準の表示を出す。
pub fn render_once(
    path: &Path,
    image: &VwImage,
    dicom: bool,
    values: &Values,
    palette: Option<Arc<vw_core::Palette>>,
) -> Option<ColorImage> {
    let header = match dicom {
        true => match pixel_pipeline::read(path).ok()? {
            Pipeline::Gray(g) => Some(g),
            Pipeline::Color(_) => None,
        },
        false => None,
    };
    let key = Key {
        path: path.to_path_buf(),
        image: 0,
        values: values.clone(),
        size: [image.width as usize, image.height as usize],
        palette,
        window: None,
        preview: false,
    };
    let r = render(image, header.as_deref(), &key);
    (r.checked.is_ok() && r.error.is_none()).then_some(r.derived)
}

fn render(image: &VwImage, info: Option<&Gray>, key: &Key) -> Rendered {
    let vw_pipeline::render::Rendering {
        float,
        mut standard,
        checked,
        error,
        notes,
        missing,
    } = vw_pipeline::render::render(
        image,
        info,
        &key.values,
        key.window,
        key.preview.then_some(PREVIEW_SIDE),
    );
    let mut derived = pixel_pipeline::to_color_image(&float, &standard);
    // 色の表は灰色のまま出てきたときだけ引く（混合などで RGB になったら、その色を出す）。
    let gray = float.channels == 1;
    let luma = Arc::new(crate::quality::luma_of_derived(
        &float,
        key.palette.as_deref().filter(|_| gray),
    ));
    drop(float);
    if let Some(p) = &key.palette {
        p.recolor(&mut standard);
        if gray {
            for c in &mut derived.pixels {
                let [r, g, b] = p.colors[c.r() as usize];
                *c = egui::Color32::from_rgba_premultiplied(r, g, b, c.a());
            }
        }
    }
    if let Some(m) = &missing {
        let [r, g, b] = m.color;
        for (c, _) in derived
            .pixels
            .iter_mut()
            .zip(&m.pixels)
            .filter(|&(_, &on)| on)
        {
            *c = egui::Color32::from_rgb(r, g, b);
        }
    }
    let (difference, max) = pixel_pipeline::difference(&derived, &standard);
    Rendered {
        derived: crate::loader::fit_color_image(derived.clone(), key.size),
        full: Arc::new(derived),
        luma,
        difference: crate::loader::fit_color_image(difference, key.size),
        max,
        checked,
        error,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_pipeline::VoiFunction;

    fn window(center: f64, width: f64) -> Values {
        Values {
            params: Some(Params {
                slope: 1.0,
                intercept: -1024.0,
                center,
                width,
                function: VoiFunction::Linear,
                invert: false,
            }),
            chain: Vec::new(),
        }
    }

    fn saved(name: &str, values: Values) -> Preset {
        Preset {
            name: name.into(),
            values,
            look: None,
        }
    }

    fn render_gray(values: Vec<f32>, chain: Vec<crate::nodes::Node>) -> Rendered {
        use vw_core::{ColorModel, PixelData};
        let mut image = VwImage::new(
            values.len() as u32,
            1,
            ColorModel::Gray,
            PixelData::F32(values),
        )
        .unwrap();
        image.window = Some((0.0, 100.0));
        let key = Key {
            path: PathBuf::from("a.fits"),
            image: 0,
            values: Values {
                params: None,
                chain,
            },
            size: [4, 1],
            palette: None,
            window: None,
            preview: false,
        };
        render(&image, None, &key)
    }

    #[test]
    fn nodes_before_the_stretch_node_see_values_beyond_white() {
        use crate::nodes::Node;
        let half = || Node::Expression {
            text: "v / 2".into(),
        };
        let reds = |r: &Rendered| r.full.pixels.iter().map(|c| c.r()).collect::<Vec<_>>();
        let staged = render_gray(vec![0.0, 50.0, 100.0, 200.0], vec![half(), Node::Stretch]);
        assert_eq!(staged.error, None);
        assert_eq!(reds(&staged), [0, 64, 128, 255]);
        let after = render_gray(vec![0.0, 50.0, 100.0, 200.0], vec![half()]);
        assert_eq!(reds(&after), [0, 64, 128, 128]);
    }

    #[test]
    fn a_second_stretch_node_is_reported() {
        use crate::nodes::Node;
        let r = render_gray(vec![1.0], vec![Node::Stretch, Node::Stretch]);
        assert_eq!(r.error.map(|(i, _)| i), Some(1));
    }

    #[test]
    fn editing_a_standard_slot_enters_unsaved_derivation_and_file_values_leave_it() {
        let mut d = None;
        edit(&mut d, Values::default(), &[]);
        assert!(d.is_none());
        edit(&mut d, window(-600.0, 1500.0), &[]);
        let derived = d.as_ref().unwrap();
        assert_eq!(
            (derived.preset.as_deref(), &derived.values),
            (None, &window(-600.0, 1500.0))
        );
        let nodes_only = Values {
            params: None,
            chain: vec![crate::nodes::Node::Invert],
        };
        edit(&mut d, nodes_only.clone(), &[]);
        assert_eq!(d.as_ref().unwrap().values, nodes_only);
        edit(&mut d, Values::default(), &[]);
        assert!(d.is_none());
    }

    #[test]
    fn editing_a_saved_pipeline_marks_it_modified_until_the_values_match_again() {
        let presets = [saved("lung", window(-600.0, 1500.0))];
        let mut d = Some(Derive::new(Some("lung".into()), window(-600.0, 1500.0)));
        edit(&mut d, window(-500.0, 1500.0), &presets);
        assert!(d.as_ref().unwrap().modified);
        edit(&mut d, window(-600.0, 1500.0), &presets);
        assert!(!d.as_ref().unwrap().modified);
        // ファイルの値に戻しても、保存したパイプラインを選んでいるので色導出のまま。
        edit(&mut d, Values::default(), &presets);
        assert_eq!(d.as_ref().unwrap().preset.as_deref(), Some("lung"));
    }

    #[test]
    fn closing_the_window_drops_unsaved_values() {
        let presets = [saved("lung", window(-600.0, 1500.0))];
        let mut unsaved = Some(Derive::new(None, window(1.0, 2.0)));
        revert(&mut unsaved, &presets);
        assert!(unsaved.is_none());
        let mut changed = Some(Derive::new(Some("lung".into()), window(1.0, 2.0)));
        changed.as_mut().unwrap().modified = true;
        revert(&mut changed, &presets);
        let d = changed.unwrap();
        assert_eq!((d.values, d.modified), (window(-600.0, 1500.0), false));
    }

    #[test]
    fn slots_follow_overwritten_and_deleted_pipelines() {
        let mut follows = Derive::new(Some("lung".into()), window(-600.0, 1500.0));
        let mut changed = Derive::new(Some("lung".into()), window(-500.0, 1500.0));
        changed.modified = true;
        let overwritten = [saved("lung", window(-700.0, 1600.0))];
        follows.sync(&overwritten);
        changed.sync(&overwritten);
        assert_eq!(follows.values, window(-700.0, 1600.0));
        assert_eq!(
            (&changed.values, changed.modified),
            (&window(-500.0, 1500.0), true)
        );
        follows.sync(&[]);
        assert_eq!(
            (follows.preset.as_deref(), &follows.values),
            (None, &window(-700.0, 1600.0))
        );
    }

    #[test]
    fn an_odd_number_of_invert_nodes_flips_the_brightness() {
        let path = Path::new("a.png");
        let with = |n: usize| {
            let mut d = Derive::new(
                None,
                Values {
                    params: None,
                    chain: vec![crate::nodes::Node::Invert; n],
                },
            );
            d.headers.insert(path.to_path_buf(), Header::Plain);
            d.inverted(path)
        };
        assert!(!with(0));
        assert!(with(1));
        assert!(!with(2));
        // ヘッダを読む前（どの段を当てるか分からない）は反転していないものとして扱う。
        assert!(!Derive::new(None, Values::default()).inverted(path));
    }
}
