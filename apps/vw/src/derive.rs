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
}

struct Rendered {
    /// フル解像度の色導出（解析ウインドウ用）。
    full: Arc<ColorImage>,
    derived: ColorImage,
    difference: ColorImage,
    max: u8,
    checked: Result<(), Mismatch>,
    /// ノードが当てられなかった（式の誤りなど）。
    error: Option<(usize, String)>,
}

pub struct Ready {
    key: Key,
    pub full: Arc<ColorImage>,
    pub derived: TextureHandle,
    pub difference: TextureHandle,
    /// 差の最大（0..=255）。
    pub max: u8,
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
}

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
        let key = Key {
            path: shown.path.clone(),
            image: Arc::as_ptr(&full.image) as usize,
            values: self.values.clone(),
            size: full.texture.size(),
            palette: shown.info.look.as_ref().and_then(|a| a.palette.clone()),
            window: shown.info.look.as_ref().and_then(|a| a.slot_window()),
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
        if let Some((reading, rx)) = &self.reading {
            if let Ok(result) = rx.try_recv() {
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
                ready.key = key;
            }
            None => {
                self.ready = Some(Ready {
                    derived: ctx.load_texture("derived", r.derived, options),
                    difference: ctx.load_texture("derived-difference", r.difference, options),
                    max: r.max,
                    full: r.full,
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

    /// 描いている途中（スロットに印を出すため）。
    pub fn busy(&self) -> bool {
        self.reading.is_some() || self.rendering.is_some()
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

fn render(image: &VwImage, info: Option<&Gray>, key: &Key) -> Rendered {
    let standard = pixel_pipeline::standard_display(image);
    let (at_file, mut float) = match info {
        Some(g) => {
            let decoded = Decoded::of(g);
            let file = pixel_pipeline::file_params(g, image, &decoded);
            let file = match key.window {
                Some(w) => pixel_pipeline::with_window(file, image, &decoded, w),
                None => file,
            };
            let at_file = pixel_pipeline::float_of_params(image, &decoded, &file);
            let float = match key.values.params {
                Some(p) if p != file => pixel_pipeline::float_of_params(image, &decoded, &p),
                _ => at_file.clone(),
            };
            (at_file, float)
        }
        None => {
            let f = pixel_pipeline::float_of_display(image);
            (f.clone(), f)
        }
    };
    let checked = pixel_pipeline::check_standard(&at_file, &standard);
    drop(at_file);
    let error = nodes::apply(&key.values.chain, &mut float).err();
    let mut derived = pixel_pipeline::to_color_image(&float, &standard);
    // 色の表は灰色のまま出てきたときだけ引く（混合などで RGB になったら、その色を出す）。
    let gray = float.channels == 1;
    drop(float);
    let mut standard = standard;
    if let Some(p) = &key.palette {
        p.recolor(&mut standard);
        if gray {
            for c in &mut derived.pixels {
                let [r, g, b] = p.colors[c.r() as usize];
                *c = egui::Color32::from_rgba_premultiplied(r, g, b, c.a());
            }
        }
    }
    let (difference, max) = pixel_pipeline::difference(&derived, &standard);
    Rendered {
        derived: crate::loader::fit_color_image(derived.clone(), key.size),
        full: Arc::new(derived),
        difference: crate::loader::fit_color_image(difference, key.size),
        max,
        checked,
        error,
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
        }
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
}
