//! ライトにダーク・フラット・バイアスのマスターを当てる。
//!
//! 補正後 = (ライト − ダーク) ÷ 正規化したフラット。フラットは先にダークフラット
//! （なければバイアス、なければ定数のオフセット）を引き、中央値か平均で割って 1 前後にする。
//! デモザイクの前に、モザイクのままの値に当てる。

mod frame;
mod search;
mod select;
mod stack;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use vw_core::{Metadata, PixelData};

pub use frame::{Frame, Kind};
use search::{Index, Places};
use select::{Choice, number};

/// メタデータの見出し。
pub const GROUP: &str = "Calibration";
/// [`GROUP`] の、当てたかどうかの項目。
pub const STATE: &str = "State";
/// [`STATE`] の値: 当てた。
pub const APPLIED: &str = "Applied";
/// [`STATE`] の値: 当てられるが、この画像では切ってある。
pub const TURNED_OFF: &str = "Off for this image";
/// [`STATE`] の値: 合うマスターがない。
pub const NOT_APPLIED: &str = "Not applied";

/// 当てるマスターの役目。手で選び直すときの単位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// ライトから引くダーク。
    Dark,
    /// ライトを割るフラット。
    Flat,
    /// フラットから引くダークフラットかバイアス。
    FlatDark,
    /// ダークがないときなどに、ライトから引くバイアス。
    Bias,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Dark, Role::Flat, Role::FlatDark, Role::Bias];

    /// メタデータの項目名。
    pub fn key(self) -> &'static str {
        match self {
            Role::Dark => "Dark",
            Role::Flat => "Flat",
            Role::FlatDark => "Flat dark",
            Role::Bias => "Bias",
        }
    }

    /// 候補を並べるメタデータの項目名。
    pub fn candidate_key(self) -> &'static str {
        match self {
            Role::Dark => "Dark candidate",
            Role::Flat => "Flat candidate",
            Role::FlatDark => "Flat dark candidate",
            Role::Bias => "Bias candidate",
        }
    }
}

/// メタデータの、手で選んだマスターの役目を並べる項目名（値は [`Role::key`]）。
pub const CHOSEN: &str = "Chosen by hand";
/// [`GROUP`] の、ホットピクセルを埋めたかの項目。
pub const HOT_PIXELS: &str = "Hot pixels";
/// [`HOT_PIXELS`] の値: この画像では切ってある。
pub const HOT_PIXELS_OFF: &str = "off for this image";
/// 作ったマスターを置く、設定のフォルダの下のフォルダの名前。
pub const CACHE_FOLDER: &str = "calibration-cache";
/// [`GROUP`] の、1 枚ずつの画像からマスターを作っている（作った）ことの項目。
pub const BUILT: &str = "Built master";

/// フォルダごとに手で選んだマスター。
type Picks = HashMap<Role, PathBuf>;

/// 露出が違うダークをどうするか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DarkScaling {
    /// 露出が合うダークだけを使う。CMOS のアンプグローは露出に比例しないため、これが慣習。
    #[default]
    None,
    /// バイアスを引いたダークを、露出の比で伸ばす。
    Exposure,
}

/// フラットをどう選ぶか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlatMatch {
    #[default]
    Filter,
    /// フィルターが同じで、同じ夜に撮ったものだけ。
    FilterAndNight,
}

/// フラットを 1 前後にするときの割る数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlatNormalize {
    #[default]
    Median,
    Mean,
}

/// 校正の決め方。設定画面で変える。
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// 見つかれば当てる。
    pub apply: bool,
    /// ライトの近くの種類別のフォルダを探す。
    pub search_nearby: bool,
    /// マスターを貯めておくフォルダ。下の階層も探す。
    pub library: Vec<PathBuf>,
    pub use_dark: bool,
    pub use_flat: bool,
    pub use_bias: bool,
    pub exposure_tolerance_secs: f64,
    /// ℃。
    pub temperature_tolerance: f64,
    pub match_gain_offset: bool,
    pub dark_scaling: DarkScaling,
    pub flat_match: FlatMatch,
    pub flat_normalize: FlatNormalize,
    /// カラーのフラットを、モザイクの 4 つの位置ごとに 1 前後にする（色かぶりを消す）。
    pub equalize_cfa: bool,
    /// バイアスのマスターがないときに引く定数。
    pub synthetic_bias: Option<f64>,
    /// 1 枚ずつの校正画像しかないとき、重ねてマスターを作る。
    pub build_masters: bool,
    /// マスターを作るときに、中央値から何 σ 外れた値を除くか。
    pub master_rejection_sigma: f64,
    /// マスターダークで中央値から外れて明るい画素を、周りの値で埋める。
    pub hot_pixels: bool,
    /// ホットピクセルとみなす、中央値からの外れ（σ の倍数）。
    pub hot_pixel_sigma: f64,
    /// 作ったマスターを置くフォルダ。`None` なら作らない。設定ファイルには書かない（アプリが決める）。
    pub cache_dir: Option<PathBuf>,
    /// マスターができるまで待ってから当てる（一括の書き出し用）。`false` なら、作っている間は
    /// 素のまま出し、できたら [`Calibration::generation`] を進める（画面を待たせないため）。
    pub wait_for_masters: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            apply: true,
            search_nearby: true,
            library: Vec::new(),
            use_dark: true,
            use_flat: true,
            use_bias: true,
            exposure_tolerance_secs: 0.5,
            temperature_tolerance: 2.0,
            match_gain_offset: true,
            dark_scaling: DarkScaling::None,
            flat_match: FlatMatch::Filter,
            flat_normalize: FlatNormalize::Median,
            equalize_cfa: false,
            synthetic_bias: None,
            build_masters: true,
            master_rejection_sigma: 3.0,
            hot_pixels: false,
            hot_pixel_sigma: 3.0,
            cache_dir: None,
            wait_for_masters: false,
        }
    }
}

/// プラグインとアプリで共有する、校正の決め方と、探した結果・読んだマスターの覚え。
#[derive(Clone, Default)]
pub struct Calibration(Arc<Shared>);

#[derive(Default)]
struct Shared {
    options: RwLock<Options>,
    /// 「校正なし」にしたファイル。
    off: RwLock<HashSet<PathBuf>>,
    /// ライトのフォルダごとに、手で選んだマスター。同じ夜のライトは同じマスターで見たいので、
    /// 1 枚ごとではなくフォルダごとに持つ。
    picks: RwLock<HashMap<PathBuf, Picks>>,
    index: Mutex<Index>,
    masters: Mutex<Masters>,
    /// 「ホットピクセルを埋めない」にしたファイル。
    hot_off: RwLock<HashSet<PathBuf>>,
    /// 裏で作っている・作れなかったマスター（キャッシュのファイルのパス）。
    building: Mutex<HashSet<PathBuf>>,
    failed: Mutex<HashMap<PathBuf, String>>,
    /// マスターができるたびに 1 つ進む。アプリは変わったら FITS を読み直す。
    generation: AtomicU64,
}

impl Calibration {
    pub fn options(&self) -> Options {
        self.0
            .options
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_options(&self, options: Options) {
        *self.0.options.write().unwrap_or_else(|e| e.into_inner()) = options;
    }

    /// `file` を「校正なし」で見ているか。
    pub fn is_off(&self, file: &Path) -> bool {
        self.0
            .off
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(file)
    }

    /// `file` の校正を切り替える。切ったら `true`。
    pub fn toggle(&self, file: &Path) -> bool {
        let mut off = self.0.off.write().unwrap_or_else(|e| e.into_inner());
        if off.remove(file) {
            false
        } else {
            off.insert(file.to_path_buf());
            true
        }
    }

    /// `file` で、ホットピクセルを埋めるのを切っているか。
    pub fn is_hot_pixels_off(&self, file: &Path) -> bool {
        self.0
            .hot_off
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(file)
    }

    /// `file` の、ホットピクセルを埋めるかを切り替える。切ったら `true`。
    pub fn toggle_hot_pixels(&self, file: &Path) -> bool {
        let mut off = self.0.hot_off.write().unwrap_or_else(|e| e.into_inner());
        if off.remove(file) {
            false
        } else {
            off.insert(file.to_path_buf());
            true
        }
    }

    /// 裏でマスターを作っているか。
    pub fn is_building(&self) -> bool {
        !self
            .0
            .building
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// 裏でマスターができるたびに進む数。
    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::Acquire)
    }

    /// 作ったマスターのキャッシュを消す。消したファイルの数。作れなかった覚えも消すので、
    /// 次に開いたときに作り直す。
    pub fn clear_cache(&self) -> Result<usize, String> {
        self.0
            .failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        *self.0.masters.lock().unwrap_or_else(|e| e.into_inner()) = Masters::default();
        let Some(dir) = self.options().cache_dir else {
            return Ok(0);
        };
        let mut removed = 0;
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            // 自分で作ったものだけを消す（設定フォルダに人が置いたファイルを消さない）。
            if name.to_string_lossy().starts_with(stack::PREFIX) {
                std::fs::remove_file(entry.path())
                    .map_err(|e| format!("{}: {e}", entry.path().display()))?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// 1 枚ずつしかない校正画像の組の、作ったマスターを `frames` に足す。まだなければ作り始め、
    /// そのことを書いた (項目, 値) を返す。
    fn add_built_masters(
        &self,
        frames: &mut Vec<Frame>,
        o: &Options,
    ) -> Vec<(&'static str, String)> {
        let mut notes = Vec::new();
        let Some(dir) = o.cache_dir.as_ref().filter(|_| o.build_masters) else {
            return notes;
        };
        let groups: Vec<(PathBuf, Kind, Vec<Frame>)> = stack::groups(frames)
            .into_iter()
            .map(|g| {
                let path = dir.join(g.file_name(o.master_rejection_sigma));
                let owned = g.frames.iter().map(|f| (*f).clone()).collect();
                (path, g.kind, owned)
            })
            .collect();
        for (path, kind, singles) in groups {
            let what = format!(
                "master {} from {} single frames",
                kind.name(),
                singles.len()
            );
            if !path.exists() {
                if let Some(e) = self.failed(&path) {
                    notes.push((BUILT, format!("could not build the {what}: {e}")));
                    continue;
                }
                if o.wait_for_masters {
                    if let Err(e) = build(kind, &singles, o.master_rejection_sigma, &path) {
                        self.0
                            .failed
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(path.clone(), e.clone());
                        notes.push((BUILT, format!("could not build the {what}: {e}")));
                        continue;
                    }
                } else {
                    self.build_in_background(kind, singles, o.master_rejection_sigma, path);
                    notes.push((BUILT, format!("building a {what}")));
                    continue;
                }
            }
            let frame = self
                .0
                .index
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .frame(&path);
            if let Some(frame) = frame {
                notes.push((BUILT, format!("{what}: {}", path.display())));
                frames.push(frame);
            }
        }
        notes
    }

    fn failed(&self, path: &Path) -> Option<String> {
        self.0
            .failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .cloned()
    }

    fn build_in_background(&self, kind: Kind, singles: Vec<Frame>, sigma: f64, path: PathBuf) {
        if !self
            .0
            .building
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.clone())
        {
            return;
        }
        let shared = self.0.clone();
        std::thread::spawn(move || {
            let result = build(kind, &singles, sigma, &path);
            if let Err(e) = result {
                shared
                    .failed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(path.clone(), e);
            }
            shared
                .building
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&path);
            shared.generation.fetch_add(1, Ordering::AcqRel);
        });
    }

    /// `light` のフォルダのライトに、`role` のマスターとして `file` を使う。`None` なら自動に戻す。
    pub fn choose(&self, light: &Path, role: Role, file: Option<PathBuf>) {
        let Some(dir) = light.parent() else {
            return;
        };
        let mut picks = self.0.picks.write().unwrap_or_else(|e| e.into_inner());
        let folder = picks.entry(dir.to_path_buf()).or_default();
        match file {
            Some(f) => {
                folder.insert(role, f);
            }
            None => {
                folder.remove(&role);
            }
        }
    }

    fn picks(&self, light: &Path) -> Picks {
        let picks = self.0.picks.read().unwrap_or_else(|e| e.into_inner());
        light
            .parent()
            .and_then(|d| picks.get(d))
            .cloned()
            .unwrap_or_default()
    }

    /// 校正の結果をメタデータに書き、当てた値を返す。当てないときは `pixels` のまま。
    /// `get` はライトのヘッダのカード、`bayer` はモザイクの画像か。
    pub(crate) fn calibrate(
        &self,
        path: &Path,
        get: &dyn Fn(&str) -> Option<String>,
        bayer: bool,
        pixels: PixelData,
        metadata: &mut Metadata,
    ) -> PixelData {
        let o = self.options();
        if !o.apply {
            return pixels;
        }
        let Some(light) = Frame::read(path, get) else {
            return pixels;
        };
        if light.kind != Kind::Light {
            metadata.push(GROUP, "Frame type", light.kind.name());
            return pixels;
        }
        let mut frames = {
            let mut index = self.0.index.lock().unwrap_or_else(|e| e.into_inner());
            index.frames_near(
                path,
                &Places {
                    nearby: o.search_nearby,
                    library: &o.library,
                },
            )
        };
        let built = self.add_built_masters(&mut frames, &o);
        let picks = self.picks(path);
        let plan = plan(&light, &frames, &o, &picks);
        let Some(plan) = plan else {
            metadata.push(GROUP, STATE, NOT_APPLIED);
            for (role, why) in reasons(&light, &frames, &o).into_iter().chain(built) {
                metadata.push(GROUP, role, why);
            }
            return pixels;
        };
        if self.is_off(path) {
            metadata.push(GROUP, STATE, TURNED_OFF);
            return pixels;
        }
        let mut values = to_f32(pixels);
        let mut masters = self.0.masters.lock().unwrap_or_else(|e| e.into_inner());
        let hot = if !o.hot_pixels {
            HotPixels::Unused
        } else if self.is_hot_pixels_off(path) {
            HotPixels::Off
        } else {
            HotPixels::Fill(o.hot_pixel_sigma)
        };
        match plan.apply(&mut values, &light, bayer, &o, hot, &mut masters) {
            Ok(notes) => {
                metadata.push(GROUP, STATE, APPLIED);
                // 当てなかった種類は、わけを添える（ダークだけ当たってフラットがない、など）。
                let missing: Vec<_> = reasons(&light, &frames, &o)
                    .into_iter()
                    .filter(|(role, _)| *role != "Note" && notes.iter().all(|(k, _)| k != role))
                    .collect();
                for (k, v) in notes.into_iter().chain(missing).chain(built) {
                    metadata.push(GROUP, k, v);
                }
            }
            Err(e) => {
                metadata.push(GROUP, STATE, NOT_APPLIED);
                metadata.push(GROUP, "Error", e);
            }
        }
        PixelData::F32(values)
    }
}

fn build(kind: Kind, singles: &[Frame], sigma: f64, path: &Path) -> Result<(), String> {
    stack::Group {
        kind,
        frames: singles.iter().collect(),
    }
    .build(sigma, path)
}

/// ホットピクセルを埋めるか。
#[derive(Debug, Clone, Copy, PartialEq)]
enum HotPixels {
    /// 設定で切ってある。
    Unused,
    /// この画像では切ってある。
    Off,
    /// この σ で探して埋める。
    Fill(f64),
}

fn to_f32(pixels: PixelData) -> Vec<f32> {
    match pixels {
        PixelData::U8(v) => v.into_iter().map(f32::from).collect(),
        PixelData::U16(v) => v.into_iter().map(f32::from).collect(),
        PixelData::F32(v) => v,
    }
}

/// 引くオフセット。
#[derive(Debug, Clone, PartialEq)]
enum Offset<'a> {
    None,
    Constant(f64),
    Frame(Choice<'a>),
}

/// 当てるもの。
#[derive(Debug, Clone, PartialEq)]
struct Plan<'a> {
    /// ライトから引くダーク。露出が違えば、露出の比と、その前に引くオフセット。
    dark: Option<(Choice<'a>, f64)>,
    /// ライトから引くオフセット（ダークがないとき、または伸ばすダークの土台）。
    offset: Offset<'a>,
    flat: Option<(Choice<'a>, Offset<'a>)>,
}

/// バイアスのマスターか、なければ定数のオフセット。
fn offset_for<'a>(of: &Frame, frames: &'a [Frame], o: &Options, pick: Option<&Path>) -> Offset<'a> {
    if o.use_bias
        && let Ok(b) = select::biases(of, frames, o)
    {
        return Offset::Frame(Choice::pick(b, pick));
    }
    o.synthetic_bias.map_or(Offset::None, Offset::Constant)
}

/// フラットから引くもの。ダークフラットを先に、バイアスを後に並べた候補から選ぶ。
fn flat_offset<'a>(
    flat: &Frame,
    frames: &'a [Frame],
    o: &Options,
    pick: Option<&Path>,
) -> Offset<'a> {
    let mut ranked = Vec::new();
    if o.use_dark {
        ranked.extend(select::dark_flats(flat, frames, o).unwrap_or_default());
    }
    if o.use_bias {
        ranked.extend(select::biases(flat, frames, o).unwrap_or_default());
    }
    if ranked.is_empty() {
        return o.synthetic_bias.map_or(Offset::None, Offset::Constant);
    }
    Offset::Frame(Choice::pick(ranked, pick))
}

/// 何を当てるか。何も当てられなければ `None`。
fn plan<'a>(light: &Frame, frames: &'a [Frame], o: &Options, picks: &Picks) -> Option<Plan<'a>> {
    let pick = |role: Role| picks.get(&role).map(PathBuf::as_path);
    let mut plan = Plan {
        dark: None,
        offset: Offset::None,
        flat: None,
    };
    if o.use_dark
        && let Ok(d) = select::dark(light, frames, o, pick(Role::Dark))
    {
        let ratio = match (light.exposure, d.frame.exposure) {
            (Some(l), Some(e)) if e > 0.0 && o.dark_scaling == DarkScaling::Exposure => l / e,
            _ => 1.0,
        };
        if (ratio - 1.0).abs() < 1e-6 {
            plan.dark = Some((d, 1.0));
        } else {
            // 伸ばす前にバイアスを抜かないと、バイアスまで伸ばしてしまう。
            let offset = offset_for(light, frames, o, pick(Role::Bias));
            if offset != Offset::None {
                plan.dark = Some((d, ratio));
                plan.offset = offset;
            }
        }
    }
    if plan.dark.is_none() {
        plan.offset = match offset_for(light, frames, o, pick(Role::Bias)) {
            // ダークもバイアスもなければ、定数だけ引いてもフラットの補正にしかならない。
            Offset::Constant(_) if !o.use_flat => Offset::None,
            other => other,
        };
    }
    // ライトから何も引けないのにフラットで割ると、センサーの土台（バイアス）まで割られて、
    // 周辺減光が逆向きに付く。フラットはライトから何か引けるときだけ当てる。
    let subtracted = plan.dark.is_some() || plan.offset != Offset::None;
    if o.use_flat
        && subtracted
        && let Ok(f) = select::flat(light, frames, o, pick(Role::Flat))
    {
        let offset = flat_offset(f.frame, frames, o, pick(Role::FlatDark));
        plan.flat = Some((f, offset));
    }
    let nothing = plan.dark.is_none() && plan.flat.is_none() && plan.offset == Offset::None;
    (!nothing).then_some(plan)
}

/// ライトから引けるもの（ダークかバイアスか定数）があるか。
fn can_subtract(light: &Frame, frames: &[Frame], o: &Options) -> bool {
    (o.use_dark && select::dark(light, frames, o, None).is_ok())
        || offset_for(light, frames, o, None) != Offset::None
}

/// 当てられなかったわけ（種類ごと）。
fn reasons(light: &Frame, frames: &[Frame], o: &Options) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if o.use_dark
        && let Err(e) = select::dark(light, frames, o, None)
    {
        out.push(("Dark", e));
    }
    if o.use_flat {
        match select::flat(light, frames, o, None) {
            Err(e) => out.push(("Flat", e)),
            Ok(_) if !can_subtract(light, frames, o) => out.push((
                "Flat",
                "not used: no dark, bias or constant bias to subtract from the light first".into(),
            )),
            Ok(_) => {}
        }
    }
    if out.is_empty() {
        out.push(("Note", "nothing to apply with these settings".into()));
    }
    out
}

impl Plan<'_> {
    /// 値に当てる。読めないマスターがあれば、値は変えずに `Err`（先に全部読んでから書き換える）。
    fn apply(
        &self,
        values: &mut [f32],
        light: &Frame,
        bayer: bool,
        o: &Options,
        hot: HotPixels,
        masters: &mut Masters,
    ) -> Result<Vec<(&'static str, String)>, String> {
        let n = light.width * light.height;
        let mut notes = Vec::new();
        let describe = |c: &Choice| {
            if c.note.is_empty() {
                c.frame.path.display().to_string()
            } else {
                format!("{} ({})", c.frame.path.display(), c.note)
            }
        };
        let offset = load_offset(&self.offset, n, masters)?;
        let dark = match &self.dark {
            Some((c, ratio)) => Some((masters.raw(&c.frame.path, n)?, *ratio)),
            None => None,
        };
        let flat = match &self.flat {
            Some((c, flat_offset)) => {
                let key = format!(
                    "{:?} {:?} {}",
                    flat_offset_key(flat_offset),
                    o.flat_normalize,
                    o.equalize_cfa && bayer
                );
                Some(masters.flat(&c.frame.path, &key, n, |raw| {
                    let sub = load_offset(flat_offset, n, &mut Masters::default())?;
                    Ok(normalize_flat(
                        &raw,
                        &sub,
                        light.width,
                        o.flat_normalize,
                        o.equalize_cfa && bayer,
                    ))
                })?)
            }
            None => None,
        };
        let base = |i: usize| -> f32 {
            match &offset {
                Sub::None => 0.0,
                Sub::Constant(c) => *c,
                Sub::Values(v) => v[i],
            }
        };
        let subtracted = |i: usize| -> f32 {
            match &dark {
                Some((d, ratio)) if *ratio == 1.0 => d[i],
                Some((d, ratio)) => {
                    let b = base(i);
                    b + (*ratio as f32) * (d[i] - b)
                }
                None => base(i),
            }
        };
        let before = sampled(n, |i| values[i]);
        let taken = sampled(n, subtracted);
        for (i, v) in values.iter_mut().enumerate().take(n) {
            let mut x = *v - subtracted(i);
            if let Some(f) = &flat {
                let f = f[i];
                if f > 1e-6 {
                    x /= f;
                }
            }
            *v = x;
        }
        match (hot, &self.dark) {
            (HotPixels::Unused, _) => {}
            (HotPixels::Off, _) => notes.push((HOT_PIXELS, HOT_PIXELS_OFF.into())),
            (HotPixels::Fill(_), None) => notes.push((HOT_PIXELS, "not filled: no dark".into())),
            (HotPixels::Fill(sigma), Some((c, _))) => {
                let d = masters.raw(&c.frame.path, n)?;
                let step = if bayer { 2 } else { 1 };
                let hot = masters.hot(&c.frame.path, sigma, &d, light.width, step);
                fill_hot_pixels(values, light.width, &hot, step);
                notes.push((HOT_PIXELS, format!("{} filled ({sigma} sigma)", hot.len())));
            }
        }
        if let Some((c, ratio)) = &self.dark {
            notes.push(("Dark", describe(c)));
            if *ratio != 1.0 {
                notes.push(("Dark scale", format!("x{:.3}", ratio)));
            }
        }
        if self.dark.as_ref().is_none_or(|(_, r)| *r != 1.0) {
            match &self.offset {
                Offset::Frame(c) => notes.push(("Bias", describe(c))),
                Offset::Constant(v) => notes.push(("Bias", format!("constant {}", number(*v)))),
                Offset::None => {}
            }
        }
        if let Some((c, flat_offset)) = &self.flat {
            notes.push(("Flat", describe(c)));
            notes.push((
                "Flat dark",
                match flat_offset {
                    Offset::Frame(c) => describe(c),
                    Offset::Constant(v) => format!("constant {}", number(*v)),
                    Offset::None => "none".into(),
                },
            ));
        }
        // 手で選び直せるように、候補と、手で選んだ役目を残す。
        let mut chosen = Vec::new();
        let mut list = |role: Role, c: &Choice| {
            if c.candidates.len() > 1 {
                for f in &c.candidates {
                    notes.push((role.candidate_key(), f.path.display().to_string()));
                }
            }
            if c.manual {
                chosen.push(role.key());
            }
        };
        if let Some((c, _)) = &self.dark {
            list(Role::Dark, c);
        }
        if let Offset::Frame(c) = &self.offset {
            list(Role::Bias, c);
        }
        if let Some((c, flat_offset)) = &self.flat {
            list(Role::Flat, c);
            if let Offset::Frame(c) = flat_offset {
                list(Role::FlatDark, c);
            }
        }
        if !chosen.is_empty() {
            notes.push((CHOSEN, chosen.join(", ")));
        }
        // 当てた効き目を数で見せる（画素の変換の図とメタデータ欄に出す）。
        let formula = match (
            self.dark.is_some() || self.offset != Offset::None,
            flat.is_some(),
        ) {
            (true, true) => "(light - dark) / flat",
            (true, false) => "light - dark",
            (false, _) => "light / flat",
        };
        notes.push(("Formula", formula.into()));
        if self.dark.is_some() || self.offset != Offset::None {
            notes.push(("Subtracted (median)", value_text(percentile(&taken, 0.5))));
        }
        if let Some(f) = &flat {
            let f = sampled(n, |i| f[i]);
            notes.push((
                "Flat (1-99 %)",
                format!("{:.3} - {:.3}", percentile(&f, 0.01), percentile(&f, 0.99)),
            ));
        }
        let after = sampled(n, |i| values[i]);
        notes.push((
            "Light median",
            format!(
                "{} -> {}",
                value_text(percentile(&before, 0.5)),
                value_text(percentile(&after, 0.5))
            ),
        ));
        Ok(notes)
    }
}

/// 数えるのに使う画素の上限。全画素を並べ替えると数千万画素で重いので、間引いて見る。
const SAMPLES: usize = 100_000;

/// `n` 画素から等間隔に取った値（NaN を除き、小さい順）。
fn sampled(n: usize, value: impl Fn(usize) -> f32) -> Vec<f32> {
    let step = n.div_ceil(SAMPLES).max(1);
    let mut v: Vec<f32> = (0..n)
        .step_by(step)
        .map(value)
        .filter(|v| v.is_finite())
        .collect();
    v.sort_by(f32::total_cmp);
    v
}

/// 小さい順の `sorted` の `q`（0〜1）の位置の値。
fn percentile(sorted: &[f32], q: f64) -> f32 {
    if sorted.is_empty() {
        return f32::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn value_text(v: f32) -> String {
    format!("{v:.1}")
}

fn flat_offset_key(o: &Offset) -> String {
    match o {
        Offset::None => "none".into(),
        Offset::Constant(v) => format!("const {v}"),
        Offset::Frame(c) => c.frame.path.display().to_string(),
    }
}

/// 読み込んだオフセット。
enum Sub {
    None,
    Constant(f32),
    Values(Arc<Vec<f32>>),
}

fn load_offset(o: &Offset, n: usize, masters: &mut Masters) -> Result<Sub, String> {
    Ok(match o {
        Offset::None => Sub::None,
        Offset::Constant(v) => Sub::Constant(*v as f32),
        Offset::Frame(c) => Sub::Values(masters.raw(&c.frame.path, n)?),
    })
}

/// フラットからオフセットを引き、中央値か平均で割る。`by_cfa` ならモザイクの 4 つの位置ごとに。
fn normalize_flat(
    raw: &[f32],
    sub: &Sub,
    width: usize,
    how: FlatNormalize,
    by_cfa: bool,
) -> Vec<f32> {
    let mut flat: Vec<f32> = raw
        .iter()
        .enumerate()
        .map(|(i, &v)| match sub {
            Sub::None => v,
            Sub::Constant(c) => v - c,
            Sub::Values(s) => v - s[i],
        })
        .collect();
    let groups: usize = if by_cfa { 4 } else { 1 };
    let group = |i: usize| -> usize {
        if by_cfa {
            (i / width % 2) * 2 + (i % width % 2)
        } else {
            0
        }
    };
    let mut norms = vec![1.0f32; groups];
    for (g, norm) in norms.iter_mut().enumerate() {
        let mut values: Vec<f32> = flat
            .iter()
            .enumerate()
            .filter(|&(i, v)| group(i) == g && v.is_finite())
            .map(|(_, &v)| v)
            .collect();
        if values.is_empty() {
            continue;
        }
        let value = match how {
            FlatNormalize::Mean => {
                (values.iter().map(|&v| v as f64).sum::<f64>() / values.len() as f64) as f32
            }
            FlatNormalize::Median => {
                let mid = values.len() / 2;
                *values.select_nth_unstable_by(mid, f32::total_cmp).1
            }
        };
        if value > 0.0 {
            *norm = value;
        }
    }
    for (i, v) in flat.iter_mut().enumerate() {
        *v /= norms[group(i)];
    }
    flat
}

/// 読み込んだマスターの覚え。同じ夜のライトを次々に見るとき、毎回読み直さないように。
#[derive(Default)]
struct Masters {
    entries: Vec<Entry>,
    /// マスターダークごとの、ホットピクセルの位置（小さい順）。
    hot: Vec<(PathBuf, HotKey, Arc<Vec<usize>>)>,
}

struct Entry {
    path: PathBuf,
    /// フラットは、引いたオフセットと割り方も同じときだけ使い回す。
    key: String,
    modified: Option<SystemTime>,
    values: Arc<Vec<f32>>,
}

/// ホットピクセルを探したときの σ（ビット列）と、周りの画素の間隔。
type HotKey = (u64, usize);

/// 覚えておくマスターの数（ダーク・バイアス・フラットとその予備）。1 枚数十 MB になるので多くしない。
const KEEP: usize = 4;

impl Masters {
    /// `dark` のホットピクセルの位置。同じダークと σ なら、前に探したものを使う。
    fn hot(
        &mut self,
        path: &Path,
        sigma: f64,
        dark: &[f32],
        width: usize,
        step: usize,
    ) -> Arc<Vec<usize>> {
        let key = (sigma.to_bits(), step);
        if let Some((_, _, found)) = self.hot.iter().find(|(p, k, _)| p == path && *k == key) {
            return found.clone();
        }
        let found = Arc::new(hot_pixels(dark, width, step, sigma));
        if self.hot.len() >= 2 {
            self.hot.remove(0);
        }
        self.hot.push((path.to_path_buf(), key, found.clone()));
        found
    }

    fn raw(&mut self, path: &Path, n: usize) -> Result<Arc<Vec<f32>>, String> {
        self.get_or(path, "", n, Ok)
    }

    fn flat(
        &mut self,
        path: &Path,
        key: &str,
        n: usize,
        make: impl FnOnce(Vec<f32>) -> Result<Vec<f32>, String>,
    ) -> Result<Arc<Vec<f32>>, String> {
        self.get_or(path, &format!("flat {key}"), n, make)
    }

    fn get_or(
        &mut self,
        path: &Path,
        key: &str,
        n: usize,
        make: impl FnOnce(Vec<f32>) -> Result<Vec<f32>, String>,
    ) -> Result<Arc<Vec<f32>>, String> {
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some(at) = self
            .entries
            .iter()
            .position(|e| e.path == path && e.key == key && e.modified == modified)
        {
            let e = self.entries.remove(at);
            let values = e.values.clone();
            self.entries.push(e);
            return Ok(values);
        }
        let raw = read_master(path)?;
        if raw.len() != n {
            return Err(format!("{}: size differs from the light", path.display()));
        }
        let values = Arc::new(make(raw)?);
        if self.entries.len() >= KEEP {
            self.entries.remove(0);
        }
        self.entries.push(Entry {
            path: path.to_path_buf(),
            key: key.to_owned(),
            modified,
            values: values.clone(),
        });
        Ok(values)
    }
}

/// マスターダークで、`step` 画素離れた周りの 8 画素の中央値より、σ の `sigma` 倍より明るい画素の
/// 位置。σ は、周りとの差の、全体での外れの中央値 × 1.4826。暗い側は見ない（死んだ画素は
/// ダークでは見分けにくく、埋めると星を消しやすい）。
///
/// ダーク全体の中央値と比べないのは、アンプグローや列の模様で明るいところが、まとめて
/// ホットピクセルになってしまうため。
fn hot_pixels(dark: &[f32], width: usize, step: usize, sigma: f64) -> Vec<usize> {
    if width == 0 {
        return Vec::new();
    }
    let height = dark.len() / width;
    // 縁の画素は周りが片側にしかなく、明るさの傾きの分だけ外れて見えるので、探さない。
    let inside = |i: usize| {
        let (x, y) = (i % width, i / width);
        x >= step && y >= step && x + step < width && y + step < height
    };
    let excess: Vec<f32> = (0..dark.len())
        .map(|i| match neighbour_median(dark, width, step, i, &[]) {
            Some(m) if inside(i) => dark[i] - m,
            _ => f32::NAN,
        })
        .collect();
    let sorted = sampled(excess.len(), |i| excess[i]);
    if sorted.is_empty() {
        return Vec::new();
    }
    let median = percentile(&sorted, 0.5);
    let mut deviations: Vec<f32> = sorted.iter().map(|v| (v - median).abs()).collect();
    deviations.sort_by(f32::total_cmp);
    // 外れの中央値が 0（整数の値で、半分以上が同じ値）でも、1 段の差を外れとしないよう下限を置く。
    let spread = (1.4826 * percentile(&deviations, 0.5)).max(0.5);
    let limit = median + sigma as f32 * spread;
    excess
        .iter()
        .enumerate()
        .filter(|(_, v)| **v > limit)
        .map(|(i, _)| i)
        .collect()
}

/// `i` の画素から `step` 画素離れた周りの 8 画素のうち、`skip`（小さい順）に入らず値のあるものの中央値。
fn neighbour_median(
    values: &[f32],
    width: usize,
    step: usize,
    i: usize,
    skip: &[usize],
) -> Option<f32> {
    let height = values.len() / width;
    let (w, h, s) = (width as isize, height as isize, step as isize);
    let (x, y) = ((i % width) as isize, (i / width) as isize);
    let mut around = [0.0f32; 8];
    let mut n = 0;
    for (dx, dy) in [
        (-s, -s),
        (0, -s),
        (s, -s),
        (-s, 0),
        (s, 0),
        (-s, s),
        (0, s),
        (s, s),
    ] {
        let (nx, ny) = (x + dx, y + dy);
        if nx < 0 || ny < 0 || nx >= w || ny >= h {
            continue;
        }
        let j = (ny * w + nx) as usize;
        if values[j].is_finite() && skip.binary_search(&j).is_err() {
            around[n] = values[j];
            n += 1;
        }
    }
    let around = &mut around[..n];
    around.sort_by(f32::total_cmp);
    around.get(n / 2).copied()
}

/// `hot`（小さい順の位置）の画素を、`step` 画素離れた周りの 8 画素のうち、ホットピクセルで
/// ないものの中央値で埋める。カラーのモザイクでは `step` = 2 で、同じ色の画素から取る。
fn fill_hot_pixels(values: &mut [f32], width: usize, hot: &[usize], step: usize) {
    if width == 0 {
        return;
    }
    for &i in hot {
        if let Some(v) = neighbour_median(values, width, step, i, hot) {
            values[i] = v;
        }
    }
}

fn read_master(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    crate::mosaic_values(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests;
