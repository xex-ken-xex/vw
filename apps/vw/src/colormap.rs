//! 色の表の段。灰色の画像に、種類ごとの設定（`[pipeline]`）かスロットで選んだ色の表を当てる。
//!
//! 当てるのは読み込みのスレッドで、灰色の表示を作った直後。縮小画像にも同じ色が付き、
//! 画面の側で描き直さずに済むため。画素の値はそのまま持つので、十字や ROI の値は変わらない。

use std::sync::Arc;

use vw_core::{ColorModel, Curve, DisplayParams, Metadata, Palette, VwImage, display};

use crate::quantity::Quantity;
use crate::settings::PipelineSettings;

/// 設定の分け方での画像の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fits,
    Netcdf,
    /// DICOM の PT（PET）。
    DicomPt,
    Dicom,
    /// ほかの形式の灰色の画像。
    Gray,
}

impl Kind {
    pub fn of(plugin: &str, meta: &Metadata) -> Self {
        match plugin {
            "FITS" => Kind::Fits,
            vw_netcdf::NAME => Kind::Netcdf,
            "DICOM" if meta.get("DICOM", "Modality").map(str::trim) == Some("PT") => Kind::DicomPt,
            "DICOM" => Kind::Dicom,
            _ => Kind::Gray,
        }
    }

    /// 設定画面での名前。
    pub fn title(self) -> &'static str {
        match self {
            Kind::Fits => "FITS",
            Kind::Netcdf => "NetCDF",
            Kind::DicomPt => "DICOM PT",
            Kind::Dicom => "DICOM",
            Kind::Gray => "Other gray images",
        }
    }
}

/// 設定から作る、種類ごとの既定と使える表。読み込みのスレッドに渡す。
#[derive(Debug)]
pub struct Rules {
    palettes: Vec<Arc<Palette>>,
    /// 種類ごとに初めから入れる表。`None` は入れない。
    defaults: [(Kind, Option<String>); 5],
}

impl Rules {
    pub fn new(s: &PipelineSettings) -> Self {
        let palettes = vw_core::palette::BUILTIN
            .iter()
            .filter_map(|n| Palette::builtin(n))
            .chain(s.palettes.iter().filter_map(|c| c.palette().ok()))
            .map(Arc::new)
            .collect();
        let node = |on: bool, palette: &str| (s.color_node && on).then(|| palette.to_owned());
        Self {
            palettes,
            defaults: [
                (Kind::Fits, node(s.fits.color_node, &s.fits.palette)),
                (Kind::Netcdf, node(s.netcdf.color_node, &s.netcdf.palette)),
                (
                    Kind::DicomPt,
                    node(s.dicom.pt.color_node, &s.dicom.pt.palette),
                ),
                (Kind::Dicom, node(s.dicom.color_node, &s.dicom.palette)),
                (Kind::Gray, node(s.gray.color_node, &s.gray.palette)),
            ],
        }
    }

    pub fn palette(&self, name: &str) -> Option<Arc<Palette>> {
        self.palettes.iter().find(|p| p.name == name).cloned()
    }

    /// メニューに出す順の名前。
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.palettes.iter().map(|p| p.name.as_str())
    }

    /// 設定で `kind` に初めから入れる表の名前。
    pub fn default_for(&self, kind: Kind) -> Option<&str> {
        self.defaults
            .iter()
            .find(|(k, _)| *k == kind)
            .and_then(|(_, p)| p.as_deref())
    }

    /// `current` の次の表（最後の次は最初）。
    pub fn next_after(&self, current: Option<&str>) -> Option<&str> {
        let names: Vec<&str> = self.names().collect();
        let i = current.and_then(|c| names.iter().position(|n| *n == c));
        let next = match i {
            Some(i) => (i + 1) % names.len(),
            // 灰色のままの画像は、灰色の次から始める。
            None => 1 % names.len(),
        };
        names.get(next).copied()
    }
}

/// スロットで選んだ色の表。
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Pick {
    /// 設定の種類ごとの既定に従う。
    #[default]
    Auto,
    /// 色の表の段を外す（灰色）。
    Off,
    Palette(String),
}

/// スロットで変えたこと。スロットを閉じるまで残し、同じスロットで次の画像に送っても続けて当てる。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Choice {
    pub pick: Pick,
    /// ゲージで変えた黒と白の値（ファイルの値の単位）。`None` はファイル（と設定）のまま。
    pub window: Option<(f32, f32)>,
    /// 色を重ねた合成で、赤・緑・青ごとに変えた黒と白。
    pub channels: [Option<(f32, f32)>; 3],
    /// 図の「黒と白」の段で変えた曲線。`None` はファイル（と設定）のまま。
    pub curve: Option<Curve>,
    /// 保存したパイプラインの見た目で選んだ、黒と白の決め方。`window` があればそちらを使う。
    pub black_white: vw_pipeline::look::BlackWhite,
}

/// 読み込みに渡す、色の表の当て方。
#[derive(Debug, Clone)]
pub struct Look {
    pub rules: Arc<Rules>,
    pub choice: Choice,
}

/// ゲージの目盛りに書く値の倍率と単位。書く値は「ファイルの値 × 倍率」。
#[derive(Debug, Clone, PartialEq)]
pub struct Scale {
    pub factor: f64,
    pub unit: String,
}

impl Scale {
    pub fn of(meta: &Metadata) -> Self {
        let (factor, unit) = match Quantity::of(meta) {
            Quantity::Pet(Ok(f)) => (f, "SUVbw".to_owned()),
            Quantity::Pet(Err(_)) => (1.0, "Bq/ml".to_owned()),
            Quantity::Unit(u) => (1.0, u.to_owned()),
            Quantity::Named(u) => (1.0, u),
            Quantity::Raw => (1.0, String::new()),
        };
        Self { factor, unit }
    }
}

/// 灰色の画像に当てた結果。ゲージとメニューに使う。
#[derive(Debug, Clone)]
pub struct Applied {
    pub kind: Kind,
    /// 当てた表。`None` は段を入れていない（灰色）。
    pub palette: Option<Arc<Palette>>,
    pub pick: Pick,
    /// 黒と白の値（ファイルの値の単位）。
    pub window: (f32, f32),
    /// ゲージで変える前の黒と白の値。
    pub file_window: (f32, f32),
    /// 画像の値の最小と最大。黒と白の外の値があるかをゲージの端の三角で示す。
    pub range: Option<(f32, f32)>,
    pub curve: Curve,
    /// スロットで変える前の曲線。
    pub file_curve: Curve,
    pub scale: Scale,
    /// 保存したパイプラインで決めた黒と白の決め方。`File` は読み込んだときの決め方のまま。
    pub black_white: vw_pipeline::look::BlackWhite,
}

impl Applied {
    pub fn palette_name(&self) -> Option<&str> {
        self.palette.as_deref().map(|p| p.name.as_str())
    }

    /// スロットで変えた黒と白。ファイルの値のままなら `None`。
    pub fn slot_window(&self) -> Option<(f32, f32)> {
        (self.window != self.file_window).then_some(self.window)
    }

    /// スロットで変えた曲線。ファイルのままなら `None`。
    pub fn slot_curve(&self) -> Option<Curve> {
        (self.curve != self.file_curve).then_some(self.curve)
    }
}

/// `image` が灰色なら、黒と白の値をスロットで変えた値にし、当てる表を決める。
/// 灰色でない画像は `None`（色の表を当てない）。
pub fn prepare(image: &mut VwImage, plugin: &str, look: &Look) -> Option<Applied> {
    if !matches!(image.color, ColorModel::Gray | ColorModel::GrayAlpha) {
        return None;
    }
    let kind = Kind::of(plugin, &image.metadata);
    let file_window = display::window_of(image, &DisplayParams::default());
    let file_curve = image.curve;
    if let Some(w) = look
        .choice
        .window
        .or_else(|| look.choice.black_white.window(&image.data))
    {
        image.window = Some(w);
    }
    if let Some(c) = look.choice.curve {
        image.curve = c;
    }
    let name = match &look.choice.pick {
        // 偏差の変数は、色の表の段を入れる設定のときだけ、0 を真ん中にした表を勧めのとおり使う。
        Pick::Auto => look.rules.default_for(kind).map(|d| {
            image
                .metadata
                .get(vw_netcdf::GROUP, vw_netcdf::COLOR_MAP)
                .filter(|_| kind == Kind::Netcdf)
                .unwrap_or(d)
        }),
        Pick::Off => None,
        Pick::Palette(name) => Some(name.as_str()),
    };
    let palette = name
        .and_then(|n| look.rules.palette(n))
        .filter(|p| !p.is_gray());
    Some(Applied {
        kind,
        palette,
        pick: look.choice.pick.clone(),
        window: display::window_of(image, &DisplayParams::default()),
        file_window,
        range: display::gray_range(image),
        curve: image.curve,
        file_curve,
        scale: Scale::of(&image.metadata),
        black_white: match look.choice.window {
            Some(_) => vw_pipeline::look::BlackWhite::File,
            None => look.choice.black_white,
        },
    })
}

/// 明るさを反転したときのゲージ用に、色の並びを逆にしたもの（表がなければ灰色を逆に）。
pub fn flipped(applied: &Applied) -> Applied {
    let gray = vw_core::Palette::builtin(vw_core::palette::GRAY).expect("gray is built in");
    Applied {
        palette: Some(Arc::new(
            applied.palette.as_deref().unwrap_or(&gray).reversed(),
        )),
        ..applied.clone()
    }
}

/// ゲージの目盛り。`(帯の位置 0〜1, 書く値)` を、黒から白の向きに並べる。
/// 目盛りは書く値（倍率を掛けた値）で切りのよい数にする。2 つ目は目盛りの刻み（[`tick_text`] に渡す）。
pub fn ticks(applied: &Applied, count: usize) -> (Vec<(f32, f64)>, f64) {
    let (lo, hi) = applied.window;
    if hi <= lo || count == 0 {
        return (Vec::new(), 1.0);
    }
    let f = applied.scale.factor;
    let (a, b) = (lo as f64 * f, hi as f64 * f);
    let (a, b) = (a.min(b), a.max(b));
    let step = nice_step((b - a) / count as f64);
    let first = (a / step).ceil() as i64;
    let last = (b / step + 1e-6).floor() as i64;
    let out = (first..=last)
        .map(|i| {
            let v = i as f64 * step;
            let t = ((v / f - lo as f64) / (hi - lo) as f64) as f32;
            (applied.curve.apply(t.clamp(0.0, 1.0)), v)
        })
        .collect();
    (out, step)
}

/// `rough` に近い 1・2・5 × 10 の累乗。
fn nice_step(rough: f64) -> f64 {
    if rough <= 0.0 || !rough.is_finite() {
        return 1.0;
    }
    let p = 10f64.powf(rough.log10().floor());
    let m = rough / p;
    let m = if m < 1.5 {
        1.0
    } else if m < 3.5 {
        2.0
    } else if m < 7.5 {
        5.0
    } else {
        10.0
    };
    m * p
}

/// 目盛りの値の書き方。刻みの桁に合わせて、要らない小数を出さない。
pub fn tick_text(v: f64, step: f64) -> String {
    let decimals = if step >= 1.0 {
        0
    } else {
        (-step.log10().floor()) as usize
    };
    format!("{v:.decimals$}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{CustomPalette, PipelineSettings};
    use vw_core::PixelData;

    fn gray(values: Vec<f32>) -> VwImage {
        VwImage::new(
            values.len() as u32,
            1,
            ColorModel::Gray,
            PixelData::F32(values),
        )
        .unwrap()
    }

    fn look(s: &PipelineSettings, choice: Choice) -> Look {
        Look {
            rules: Arc::new(Rules::new(s)),
            choice,
        }
    }

    fn dicom(modality: &str) -> Metadata {
        let mut m = Metadata::default();
        m.push("DICOM", "Modality", modality);
        m
    }

    #[test]
    fn kinds_follow_the_format_and_the_dicom_modality() {
        assert_eq!(Kind::of("FITS", &Metadata::default()), Kind::Fits);
        assert_eq!(Kind::of("DICOM", &dicom("PT")), Kind::DicomPt);
        assert_eq!(Kind::of("DICOM", &dicom("CT")), Kind::Dicom);
        assert_eq!(Kind::of("PNG", &Metadata::default()), Kind::Gray);
    }

    #[test]
    fn defaults_color_fits_and_pt_but_leave_other_images_gray() {
        let rules = Rules::new(&PipelineSettings::default());
        assert_eq!(rules.default_for(Kind::Fits), Some("inferno"));
        assert_eq!(rules.default_for(Kind::DicomPt), Some("hot_iron"));
        assert_eq!(rules.default_for(Kind::Dicom), None);
        assert_eq!(rules.default_for(Kind::Gray), None);
    }

    #[test]
    fn turning_the_stage_off_in_settings_leaves_every_kind_gray() {
        let s = PipelineSettings {
            color_node: false,
            ..Default::default()
        };
        let rules = Rules::new(&s);
        assert_eq!(rules.default_for(Kind::Fits), None);
        assert_eq!(rules.default_for(Kind::DicomPt), None);
    }

    #[test]
    fn fits_images_get_the_fits_palette_and_color_images_get_none() {
        let s = PipelineSettings::default();
        let mut img = gray(vec![0.0, 1.0]);
        let a = prepare(&mut img, "FITS", &look(&s, Choice::default())).unwrap();
        assert_eq!(a.palette_name(), Some("inferno"));
        let mut rgb = VwImage::new(1, 1, ColorModel::Rgb, PixelData::U8(vec![1, 2, 3])).unwrap();
        assert!(prepare(&mut rgb, "FITS", &look(&s, Choice::default())).is_none());
    }

    #[test]
    fn a_slot_can_turn_the_stage_off_or_pick_another_palette() {
        let s = PipelineSettings::default();
        let off = Choice {
            pick: Pick::Off,
            window: None,
            channels: [None; 3],
            curve: None,
            ..Choice::default()
        };
        let a = prepare(&mut gray(vec![0.0, 1.0]), "FITS", &look(&s, off)).unwrap();
        assert_eq!(a.palette_name(), None);
        let viridis = Choice {
            pick: Pick::Palette("viridis".into()),
            window: None,
            channels: [None; 3],
            curve: None,
            ..Choice::default()
        };
        let a = prepare(&mut gray(vec![0.0, 1.0]), "PNG", &look(&s, viridis)).unwrap();
        assert_eq!(a.palette_name(), Some("viridis"));
    }

    #[test]
    fn a_slot_can_change_the_curve_and_still_knows_the_file_curve() {
        let s = PipelineSettings::default();
        let mut img = gray(vec![0.0, 1.0]);
        let choice = Choice {
            curve: Some(Curve::Asinh),
            ..Choice::default()
        };
        let a = prepare(&mut img, "PNG", &look(&s, choice)).unwrap();
        assert_eq!(img.curve, Curve::Asinh);
        assert_eq!((a.curve, a.file_curve), (Curve::Asinh, Curve::Linear));
        assert_eq!(a.slot_curve(), Some(Curve::Asinh));
        let a = prepare(
            &mut gray(vec![0.0, 1.0]),
            "PNG",
            &look(&s, Choice::default()),
        )
        .unwrap();
        assert_eq!(a.slot_curve(), None);
    }

    #[test]
    fn picking_gray_counts_as_no_palette() {
        let s = PipelineSettings::default();
        let choice = Choice {
            pick: Pick::Palette("gray".into()),
            window: None,
            channels: [None; 3],
            curve: None,
            ..Choice::default()
        };
        let a = prepare(&mut gray(vec![0.0, 1.0]), "FITS", &look(&s, choice)).unwrap();
        assert_eq!(a.palette_name(), None);
    }

    #[test]
    fn custom_palettes_from_settings_can_be_picked() {
        let s = PipelineSettings {
            palettes: vec![CustomPalette {
                name: "blue".into(),
                colors: vec!["0 #000000".into(), "1 #0000ff".into()],
            }],
            ..Default::default()
        };
        let rules = Rules::new(&s);
        assert_eq!(rules.palette("blue").unwrap().colors[255], [0, 0, 255]);
        assert_eq!(rules.names().last(), Some("blue"));
    }

    #[test]
    fn the_slot_window_replaces_the_file_window_and_the_file_window_is_kept() {
        let s = PipelineSettings::default();
        let mut img = gray(vec![0.0, 50.0, 100.0]);
        img.window = Some((10.0, 90.0));
        let choice = Choice {
            pick: Pick::Auto,
            window: Some((20.0, 40.0)),
            channels: [None; 3],
            curve: None,
            ..Choice::default()
        };
        let a = prepare(&mut img, "FITS", &look(&s, choice)).unwrap();
        assert_eq!(img.window, Some((20.0, 40.0)));
        assert_eq!(a.window, (20.0, 40.0));
        assert_eq!(a.file_window, (10.0, 90.0));
        assert_eq!(a.range, Some((0.0, 100.0)));
    }

    #[test]
    fn next_palette_goes_round_and_starts_after_gray() {
        let rules = Rules::new(&PipelineSettings::default());
        assert_eq!(rules.next_after(None), Some("gray_inverted"));
        assert_eq!(rules.next_after(Some("inferno")), Some("magma"));
        assert_eq!(rules.next_after(Some("rainbow")), Some("coolwarm"));
        assert_eq!(rules.next_after(Some("blue_black_red")), Some("gray"));
    }

    #[test]
    fn pet_gauge_is_written_in_suv_when_the_factor_is_known() {
        let mut m = dicom("PT");
        m.push("DICOM", "Units", "BQML");
        assert_eq!(Scale::of(&m).unit, "Bq/ml");
        m.push("DICOM", vw_dicom::SUV_FACTOR, "0.0002");
        let scale = Scale::of(&m);
        assert_eq!((scale.factor, scale.unit.as_str()), (0.0002, "SUVbw"));
    }

    #[test]
    fn fits_gauge_unit_comes_from_bunit() {
        let mut m = Metadata::default();
        m.push("FITS", "BUNIT", "'Jy/beam '");
        assert_eq!(Scale::of(&m).unit, "Jy/beam");
        assert_eq!(Scale::of(&Metadata::default()).unit, "");
    }

    fn applied(window: (f32, f32), factor: f64, curve: Curve) -> Applied {
        Applied {
            kind: Kind::Fits,
            palette: None,
            pick: Pick::Auto,
            window,
            file_window: window,
            range: None,
            curve,
            file_curve: curve,
            scale: Scale {
                factor,
                unit: String::new(),
            },
            black_white: Default::default(),
        }
    }

    #[test]
    fn ticks_are_round_numbers_inside_the_window() {
        let (t, _) = ticks(&applied((3.0, 97.0), 1.0, Curve::Linear), 4);
        let values: Vec<f64> = t.iter().map(|(_, v)| *v).collect();
        assert_eq!(values, [20.0, 40.0, 60.0, 80.0]);
        assert!((t[0].0 - (17.0 / 94.0)).abs() < 1e-5);
    }

    #[test]
    fn ticks_are_written_in_scaled_units() {
        // 10000〜50000 Bq/ml を SUV（× 0.0001）で 1〜5。
        let (t, _) = ticks(&applied((10000.0, 50000.0), 0.0001, Curve::Linear), 4);
        let values: Vec<f64> = t.iter().map(|(_, v)| (v * 10.0).round() / 10.0).collect();
        assert_eq!(values, [1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!(t[0].0.abs() < 1e-5 && (t[4].0 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn ticks_follow_the_curve() {
        let (t, step) = ticks(&applied((0.0, 100.0), 1.0, Curve::Sqrt), 4);
        assert_eq!(step, 20.0);
        let (pos, _) = t.iter().find(|(_, v)| *v == 20.0).copied().unwrap();
        assert!((pos - 0.2f32.sqrt()).abs() < 1e-5);
    }

    #[test]
    fn tick_text_drops_digits_finer_than_the_step() {
        assert_eq!(tick_text(20.0, 20.0), "20");
        assert_eq!(tick_text(0.5, 0.5), "0.5");
        assert_eq!(tick_text(0.25, 0.05), "0.25");
    }
}
