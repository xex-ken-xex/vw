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

use crate::pipelines::Preset;
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
    Gray(Box<Gray>),
    /// カラーの画像などで、色導出の対象ではない。標準のまま出す。
    NotGray,
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
    params: Params,
    size: [usize; 2],
}

struct Rendered {
    derived: ColorImage,
    difference: ColorImage,
    max: u8,
    checked: Result<(), Mismatch>,
}

pub struct Ready {
    key: Key,
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
    pub params: Params,
    /// 保存したパイプラインを選んだあとに、色導出のウインドウで値を変えた。
    pub modified: bool,
    pub view: View,
    /// 比べるときの仕切りの位置（画像部分の幅に対する割合）。
    pub split: f32,
    headers: HashMap<PathBuf, Header>,
    reading: Option<(PathBuf, Receiver<Result<Pipeline, String>>)>,
    rendering: Option<(Key, Receiver<Rendered>)>,
    ready: Option<Ready>,
    /// 一度でも色導出を出した。入るときの確かめで食い違ったら、モードに入らずにやめる。
    entered: bool,
}

impl Derive {
    pub fn new(preset: Option<String>, params: Params) -> Self {
        Self {
            preset,
            params,
            modified: false,
            view: View::Derived,
            split: 0.5,
            headers: HashMap::new(),
            reading: None,
            rendering: None,
            ready: None,
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
        let mut event = self.read_header(ctx, &shown.path);
        let full = shown.full.as_ref()?;
        if let Some((key, rx)) = &self.rendering {
            let Ok(r) = rx.try_recv() else {
                return event;
            };
            let key = key.clone();
            self.rendering = None;
            match r.checked {
                Ok(()) => {
                    self.entered = true;
                    self.show(ctx, key, r, options);
                }
                Err(mismatch) => {
                    self.headers.insert(key.path.clone(), Header::Refused);
                    event = Some(Event::Refused {
                        path: key.path,
                        mismatch,
                        left: !self.entered,
                    });
                }
            }
        }
        let Some(Header::Gray(info)) = self.headers.get(&shown.path) else {
            return event;
        };
        let key = Key {
            path: shown.path.clone(),
            image: Arc::as_ptr(&full.image) as usize,
            params: self.params,
            size: full.texture.size(),
        };
        if self.rendering.is_none() && self.ready.as_ref().is_none_or(|r| r.key != key) {
            let (image, info, k) = (full.image.clone(), info.clone(), key.clone());
            let rx = crate::background::compute(ctx, move || render(&image, &info, &k));
            self.rendering = Some((key, rx));
        }
        event
    }

    fn read_header(&mut self, ctx: &egui::Context, path: &Path) -> Option<Event> {
        if let Some((reading, rx)) = &self.reading {
            if let Ok(result) = rx.try_recv() {
                let reading = reading.clone();
                self.reading = None;
                let (header, event) = match result {
                    Ok(Pipeline::Gray(g)) => (Header::Gray(g), None),
                    Ok(Pipeline::Color(_)) => (Header::NotGray, None),
                    Err(e) => (Header::Failed, Some(Event::Failed(reading.clone(), e))),
                };
                self.headers.insert(reading, header);
                return event;
            }
        }
        if self.reading.is_none() && !self.headers.contains_key(path) {
            let rx = pixel_pipeline::read_in_background(ctx, path);
            self.reading = Some((path.to_path_buf(), rx));
        }
        None
    }

    fn show(&mut self, ctx: &egui::Context, key: Key, r: Rendered, options: TextureOptions) {
        match &mut self.ready {
            Some(ready) => {
                ready.derived.set(r.derived, options);
                ready.difference.set(r.difference, options);
                ready.max = r.max;
                ready.key = key;
            }
            None => {
                self.ready = Some(Ready {
                    derived: ctx.load_texture("derived", r.derived, options),
                    difference: ctx.load_texture("derived-difference", r.difference, options),
                    max: r.max,
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
                && matches!(self.headers.get(&shown.path), Some(Header::Gray(_)))
        })
    }

    /// `path` の画像に色導出を当てられないときの理由（i18n の鍵）。
    pub fn not_applied(&self, path: &Path) -> Option<&'static str> {
        match self.headers.get(path)? {
            Header::Gray(_) => None,
            Header::NotGray => Some("Standard: not a grayscale image"),
            Header::Refused => Some("Standard: file values do not reproduce the standard display"),
            Header::Failed => Some("Standard: could not read the pipeline"),
        }
    }

    /// 描いている途中（スロットに印を出すため）。
    pub fn busy(&self) -> bool {
        self.reading.is_some() || self.rendering.is_some()
    }

    /// 解析ウインドウの「変更後」に使う値と画像のデコードの仕方。
    pub fn edit_for(&self, path: &Path) -> Option<(Params, Decoded)> {
        match self.headers.get(path) {
            Some(Header::Gray(g)) => Some((self.params, Decoded::of(g))),
            _ => None,
        }
    }

    pub fn textures_mut(&mut self) -> impl Iterator<Item = &mut TextureHandle> {
        self.ready
            .iter_mut()
            .flat_map(|r| [&mut r.derived, &mut r.difference])
    }
}

/// 色導出のウインドウで値を変えた。`at_file` はファイルの値に戻した（標準と同じ）。
/// 保存したパイプラインを選んでいなければ、ファイルの値に戻すと標準の表示に戻る。
pub fn edit(derive: &mut Option<Derive>, params: Params, at_file: bool, presets: &[Preset]) {
    match derive {
        None if at_file => {}
        None => *derive = Some(Derive::new(None, params)),
        Some(d) if d.preset.is_none() && at_file => *derive = None,
        Some(d) => {
            d.params = params;
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
            d.params = p.params;
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
            Some(p) if !self.modified => self.params = p.params,
            Some(p) => self.modified = self.params != p.params,
        }
    }
}

fn render(image: &VwImage, info: &Gray, key: &Key) -> Rendered {
    let decoded = Decoded::of(info);
    let standard = pixel_pipeline::standard_gray(image);
    let file = pixel_pipeline::file_params(info, image, &decoded);
    let checked = pixel_pipeline::check_standard(image, &decoded, &file, &standard);
    let derived = pixel_pipeline::render(image, &decoded, &key.params);
    let (difference, max) = pixel_pipeline::difference(&derived, &standard);
    Rendered {
        derived: crate::loader::fit_color_image(derived, key.size),
        difference: crate::loader::fit_color_image(difference, key.size),
        max,
        checked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_pipeline::VoiFunction;

    fn window(center: f64, width: f64) -> Params {
        Params {
            slope: 1.0,
            intercept: -1024.0,
            center,
            width,
            function: VoiFunction::Linear,
            invert: false,
        }
    }

    fn saved(name: &str, params: Params) -> Preset {
        Preset {
            name: name.into(),
            params,
        }
    }

    #[test]
    fn editing_a_standard_slot_enters_unsaved_derivation_and_file_values_leave_it() {
        let mut d = None;
        edit(&mut d, window(40.0, 400.0), true, &[]);
        assert!(d.is_none());
        edit(&mut d, window(-600.0, 1500.0), false, &[]);
        let derived = d.as_ref().unwrap();
        assert_eq!(
            (derived.preset.as_deref(), derived.params),
            (None, window(-600.0, 1500.0))
        );
        edit(&mut d, window(40.0, 400.0), true, &[]);
        assert!(d.is_none());
    }

    #[test]
    fn editing_a_saved_pipeline_marks_it_modified_until_the_values_match_again() {
        let presets = [saved("lung", window(-600.0, 1500.0))];
        let mut d = Some(Derive::new(Some("lung".into()), window(-600.0, 1500.0)));
        edit(&mut d, window(-500.0, 1500.0), false, &presets);
        assert!(d.as_ref().unwrap().modified);
        edit(&mut d, window(-600.0, 1500.0), false, &presets);
        assert!(!d.as_ref().unwrap().modified);
        // ファイルの値に戻しても、保存したパイプラインを選んでいるので色導出のまま。
        edit(&mut d, window(40.0, 400.0), true, &presets);
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
        assert_eq!((d.params, d.modified), (window(-600.0, 1500.0), false));
    }

    #[test]
    fn slots_follow_overwritten_and_deleted_pipelines() {
        let mut follows = Derive::new(Some("lung".into()), window(-600.0, 1500.0));
        let mut changed = Derive::new(Some("lung".into()), window(-500.0, 1500.0));
        changed.modified = true;
        let overwritten = [saved("lung", window(-700.0, 1600.0))];
        follows.sync(&overwritten);
        changed.sync(&overwritten);
        assert_eq!(follows.params, window(-700.0, 1600.0));
        assert_eq!(
            (changed.params, changed.modified),
            (window(-500.0, 1500.0), true)
        );
        follows.sync(&[]);
        assert_eq!(
            (follows.preset.as_deref(), follows.params),
            (None, window(-700.0, 1600.0))
        );
    }
}
