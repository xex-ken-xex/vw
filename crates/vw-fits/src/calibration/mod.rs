//! ライトにダーク・フラット・バイアスのマスターを当てる。
//!
//! 補正後 = (ライト − ダーク) ÷ 正規化したフラット。フラットは先にダークフラット
//! （なければバイアス、なければ定数のオフセット）を引き、中央値か平均で割って 1 前後にする。
//! デモザイクの前に、モザイクのままの値に当てる。

mod frame;
mod search;
mod select;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use vw_core::{Metadata, PixelData};

pub use frame::{Frame, Kind};
use search::{Index, Places};
use select::{number, Choice};

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
        let frames = {
            let mut index = self.0.index.lock().unwrap_or_else(|e| e.into_inner());
            index.frames_near(
                path,
                &Places {
                    nearby: o.search_nearby,
                    library: &o.library,
                },
            )
        };
        let picks = self.picks(path);
        let plan = plan(&light, &frames, &o, &picks);
        let Some(plan) = plan else {
            metadata.push(GROUP, STATE, NOT_APPLIED);
            for (role, why) in reasons(&light, &frames, &o) {
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
        match plan.apply(&mut values, &light, bayer, &o, &mut masters) {
            Ok(notes) => {
                metadata.push(GROUP, STATE, APPLIED);
                // 当てなかった種類は、わけを添える（ダークだけ当たってフラットがない、など）。
                let missing: Vec<_> = reasons(&light, &frames, &o)
                    .into_iter()
                    .filter(|(role, _)| *role != "Note" && notes.iter().all(|(k, _)| k != role))
                    .collect();
                for (k, v) in notes.into_iter().chain(missing) {
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
    if o.use_bias {
        if let Ok(b) = select::biases(of, frames, o) {
            return Offset::Frame(Choice::pick(b, pick));
        }
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
    if o.use_dark {
        if let Ok(d) = select::dark(light, frames, o, pick(Role::Dark)) {
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
    if o.use_flat && subtracted {
        if let Ok(f) = select::flat(light, frames, o, pick(Role::Flat)) {
            let offset = flat_offset(f.frame, frames, o, pick(Role::FlatDark));
            plan.flat = Some((f, offset));
        }
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
    if o.use_dark {
        if let Err(e) = select::dark(light, frames, o, None) {
            out.push(("Dark", e));
        }
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
}

struct Entry {
    path: PathBuf,
    /// フラットは、引いたオフセットと割り方も同じときだけ使い回す。
    key: String,
    modified: Option<SystemTime>,
    values: Arc<Vec<f32>>,
}

/// 覚えておくマスターの数（ダーク・バイアス・フラットとその予備）。1 枚数十 MB になるので多くしない。
const KEEP: usize = 4;

impl Masters {
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

fn read_master(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    crate::mosaic_values(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests;
