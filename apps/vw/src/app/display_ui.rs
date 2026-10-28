//! モニタごとの深い画像の描き方（[`crate::deep`]）と、それを確かめるための
//! システム情報のウインドウ・表示の階調テスト。

use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Color32, Rect, TextureHandle, TextureOptions};
use vw_core::DisplayImage16;

use super::VwApp;
use crate::deep::{self, Layer, Mode, Surface};
use crate::displays::{self, Display};
use crate::i18n::{tr, tr1};
use crate::layout;
use crate::settings::Language;

/// 描き終わるまでの間、描き直しを頼む間隔。
const POLL: Duration = Duration::from_millis(50);

pub(super) struct DisplayState {
    pub surface: Surface,
    /// 起動したときの描画の方法（ログの「Renderer:」と同じ文）。
    pub renderer: String,
    /// ログに出した、画像の欄のモニタと描き方。変わったときだけ出す。
    logged: Option<(String, Option<Mode>)>,
    pub info_open: bool,
    pub test: GradationTest,
}

impl DisplayState {
    pub fn new(surface: Surface, renderer: String) -> Self {
        Self {
            surface,
            renderer,
            logged: None,
            info_open: false,
            test: GradationTest::default(),
        }
    }
}

/// `ctx` のウインドウの真ん中があるモニタ。Wayland のように位置が分からなければ `None`。
fn display_of(ctx: &egui::Context) -> Option<Display> {
    let (rect, ppp) = ctx.input(|i| {
        (
            i.viewport().outer_rect,
            i.viewport().native_pixels_per_point.unwrap_or(1.0),
        )
    });
    let center = rect?.center();
    let point = [(center.x * ppp) as i32, (center.y * ppp) as i32];
    displays::at(&displays::cached().displays, point).cloned()
}

impl VwApp {
    /// `ctx` のウインドウがあるモニタでの、深い画像の描き方。
    fn deep_mode(&self, display: Option<&Display>) -> Option<Mode> {
        let monitor = self
            .settings()
            .display
            .monitor(display.map_or("", |d| &d.id));
        deep::mode(&monitor, self.display.surface)
    }

    /// `ctx` のウインドウがあるモニタでの、深い画像の描き方（全画面スライドショー用）。
    pub(super) fn deep_mode_here(&self, ctx: &egui::Context) -> Option<Mode> {
        self.deep_mode(display_of(ctx).as_ref())
    }

    /// 1 枚を大きく出しているスロットに、深い画像の描き方を用意する。ほかのスロットからは外す。
    /// 用意したスロットの添字を返す（そのスロットにはフル解像度の画像が要る）。
    pub(super) fn update_deep(
        &mut self,
        ctx: &egui::Context,
        view: &layout::View,
    ) -> Option<usize> {
        let display = display_of(ctx);
        let mode = self.deep_mode(display.as_ref());
        let target = (view.cols * view.rows == 1).then_some(view.range.start);
        let surface = self.display.surface;
        let options = self.texture_options;
        let max_side = max_side(ctx);
        let mut drawn = None;
        for (i, s) in self.slots.iter_mut().enumerate() {
            let wanted = mode.filter(|_| Some(i) == target).and_then(|mode| {
                let shown = s.shown.as_ref()?;
                if s.derive.is_some() || !deep::is_deep(shown.info.bits_per_sample) {
                    return None;
                }
                let full = shown.full.as_ref()?;
                let palette = shown.info.look.as_ref().and_then(|a| a.palette.clone());
                Some((Arc::clone(&full.image), palette, mode))
            });
            let Some((image, palette, mode)) = wanted else {
                s.deep = None;
                continue;
            };
            if !s
                .deep
                .as_ref()
                .is_some_and(|d| d.is_for(&image, palette.as_ref(), max_side, mode))
            {
                s.deep = Some(Layer::image(
                    &image,
                    palette.as_ref(),
                    max_side,
                    mode,
                    surface,
                ));
            }
            if let Some(layer) = &mut s.deep {
                if layer.poll(ctx, options) {
                    drawn = Some(mode);
                } else {
                    ctx.request_repaint_after(POLL);
                }
            }
        }
        let label = display.as_ref().map_or_else(String::new, Display::label);
        let now = (label, drawn);
        if self.display.logged.as_ref() != Some(&now) {
            if drawn.is_some() || self.display.logged.as_ref().is_some_and(|l| l.1.is_some()) {
                let how = drawn.map_or("8-bit", Mode::title);
                self.log
                    .info(format!("Display: {} - image drawn {how}", now.0));
            }
            self.display.logged = Some(now);
        }
        // 1 枚を大きく出していれば、深く描けない画像でもフル解像度は読む（いつもどおり）。
        // 深く描く画像は、縮小画像で足りる小さな画像でもフル解像度から作る。
        mode.and(target)
    }

    /// システム情報のウインドウ。
    pub(super) fn system_info_ui(&mut self, ctx: &egui::Context) {
        if !self.display.info_open {
            return;
        }
        let lang = self.lang();
        let here = display_of(ctx);
        let lines = self.system_lines(here.as_ref(), lang);
        let mut open = true;
        egui::Window::new(tr(lang, "System information"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(480.0)
                    .show(ui, |ui| {
                        egui::Grid::new("system-info")
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                for (key, value) in &lines {
                                    if value.is_empty() {
                                        ui.strong(key);
                                        ui.label("");
                                    } else {
                                        ui.weak(key);
                                        ui.add(egui::Label::new(value).wrap());
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(6.0);
                if ui.button(tr(lang, "Copy")).clicked() {
                    ctx.copy_text(lines_text(&lines));
                    self.log.info("System information copied");
                }
            });
        self.display.info_open = open;
    }

    /// システム情報の行（項目と値）。値が空の行は見出し。
    fn system_lines(&self, here: Option<&Display>, lang: Language) -> Vec<(String, String)> {
        let system = displays::cached();
        let surface = self.display.surface;
        let t = |s: &'static str| tr(lang, s).to_owned();
        let mut lines = vec![
            ("vw".to_owned(), env!("CARGO_PKG_VERSION").to_owned()),
            (
                t("OS"),
                format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            ),
            (t("Renderer"), self.display.renderer.clone()),
            (
                t("Surface"),
                tr1(lang, "{}-bit per color", &surface.bits.to_string()),
            ),
            (
                t("16-bit textures"),
                if !surface.gpu {
                    t("not used (the surface is 8-bit)")
                } else if surface.norm16 {
                    "Rgba16Unorm".to_owned()
                } else {
                    t("Rgba16Float (about 11 bits near white)")
                },
            ),
        ];
        if let Some(depth) = system.x11_depth {
            lines.push((t("X11 screen depth"), depth.to_string()));
        }
        lines.push((
            t("This window is on"),
            here.map_or_else(|| t("unknown"), Display::label),
        ));
        for (n, d) in system.displays.iter().enumerate() {
            let monitor = self.settings().display.monitor(&d.id);
            let [x, y, w, h] = d.rect;
            lines.push((format!("{} {}", tr(lang, "Monitor"), n + 1), String::new()));
            lines.push((t("Name"), d.label()));
            let mut place = format!("{w}x{h} ({x}, {y}), {}%", d.scale_percent);
            if let Some(hz) = d.refresh_hz {
                place.push_str(&format!(", {hz:.2} Hz"));
            }
            if d.primary {
                place.push_str(&format!(", {}", tr(lang, "primary")));
            }
            lines.push((t("Size and position"), place));
            let signal = displays::signal_text(d, lang);
            if !signal.is_empty() {
                lines.push((t("Signal"), signal));
            }
            let on_off = |b: bool| t(if b { "on" } else { "off" });
            lines.push((
                t("Settings"),
                format!(
                    "{}: {}, {}: {}",
                    tr(lang, "10-bit"),
                    on_off(monitor.ten_bit),
                    tr(lang, "Dithering"),
                    on_off(monitor.dither)
                ),
            ));
            lines.push((
                t("Deep images are drawn"),
                deep::mode(&monitor, surface).map_or_else(|| t("8-bit"), |m| t(m.title())),
            ));
            lines.push((t("ID"), d.id.clone()));
        }
        if system.displays.is_empty() {
            lines.push((t("Monitors"), t("cannot be listed here")));
        }
        lines
    }

    /// 表示の階調テストの準備のウインドウと、全画面のテストの表示。
    pub(super) fn gradation_test_ui(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let surface = self.display.surface;
        if self.display.test.setup_open {
            let system = displays::cached();
            let here = display_of(ctx);
            let test = &mut self.display.test;
            if !system
                .displays
                .iter()
                .any(|d| Some(&d.id) == test.monitor.as_ref())
            {
                test.monitor = here
                    .as_ref()
                    .or(system.displays.first())
                    .map(|d| d.id.clone());
            }
            let mut open = true;
            let mut start = false;
            egui::Window::new(tr(lang, "Display gradation test"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(tr(lang, "Shows gray bands made inside vw on the whole screen of a monitor, so you can see how deep its gradation is (8, 10, 12 or 14 bits). No image files are needed."));
                    ui.add_space(6.0);
                    if system.displays.is_empty() {
                        ui.weak(tr(lang, "The monitors cannot be listed here. The test opens on the monitor of this window."));
                    }
                    for d in &system.displays {
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut test.monitor, Some(d.id.clone()), d.label());
                            ui.weak(displays::signal_text(d, lang));
                        });
                    }
                    ui.add_space(6.0);
                    start = ui.button(tr(lang, "Show")).clicked();
                });
            self.display.test.setup_open = open;
            if start {
                let chosen = system
                    .displays
                    .iter()
                    .find(|d| Some(&d.id) == self.display.test.monitor.as_ref())
                    .cloned()
                    .or(here);
                let monitor = self
                    .settings()
                    .display
                    .monitor(chosen.as_ref().map_or("", |d| &d.id));
                let mode = deep::mode(&monitor, surface);
                self.log.info(format!(
                    "Display gradation test: {}, drawn {}",
                    chosen.as_ref().map_or_else(String::new, Display::label),
                    mode.map_or("8-bit", Mode::title)
                ));
                self.display.test.setup_open = false;
                self.display.test.showing = Some(Showing {
                    display: chosen,
                    mode,
                    range: 1,
                    frames: 0,
                    drawn: None,
                    ten_bit_setting: monitor.ten_bit,
                });
            }
        }
        if let Some(showing) = &mut self.display.test.showing {
            let open = show_test(ctx, showing, surface, lang);
            if !open {
                self.display.test.showing = None;
            }
        }
    }
}

/// GPU のテクスチャの一辺の上限。標準の表示の上限（8192）と同じにする。
fn max_side(ctx: &egui::Context) -> u32 {
    ctx.input(|i| i.max_texture_side).min(8192) as u32
}

fn lines_text(lines: &[(String, String)]) -> String {
    lines
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                format!("[{k}]")
            } else {
                format!("{k}: {v}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 表示の階調テスト。
#[derive(Default)]
pub(super) struct GradationTest {
    setup_open: bool,
    /// 選んでいるモニタ（[`Display::id`]）。
    monitor: Option<String>,
    showing: Option<Showing>,
}

impl GradationTest {
    pub fn open_setup(&mut self) {
        self.setup_open = true;
    }
}

/// 全画面で出しているテスト。
struct Showing {
    display: Option<Display>,
    mode: Option<Mode>,
    /// [`RANGES`] の添字。
    range: usize,
    frames: u32,
    /// 作った帯（画面の大きさと明るさの範囲）。
    drawn: Option<(Drawn, [usize; 2], usize)>,
    /// そのモニタの 10bit の設定。入っているのに 10bit で描けないときに案内を出す。
    ten_bit_setting: bool,
}

enum Drawn {
    Deep(Layer),
    Plain(TextureHandle),
}

/// 階調テストの帯の 1 本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// その bit 数の段に丸めた帯。
    Steps(u32),
    /// 丸めない 16bit の帯。
    Smooth,
    /// 8bit にディザで落とした帯。
    Dither,
}

/// 上から順に並べる帯。12bit・14bit の帯は、医用モニタのように 10bit より深い信号を
/// 受けるモニタで、どこまで細かく出ているかを見分けるため。
pub const BANDS: [Band; 6] = [
    Band::Steps(8),
    Band::Steps(10),
    Band::Steps(12),
    Band::Steps(14),
    Band::Smooth,
    Band::Dither,
];

impl Band {
    fn name(self) -> &'static str {
        match self {
            Band::Steps(8) => "8-bit steps",
            Band::Steps(10) => "10-bit steps",
            Band::Steps(12) => "12-bit steps",
            Band::Steps(_) => "14-bit steps",
            Band::Smooth => "Smooth (16-bit)",
            Band::Dither => "8-bit with dithering",
        }
    }

    /// `expected` bit で出ているときに、この帯がどう見えるはずか。8bit の帯とディザの帯は
    /// どのモニタでも見え方が変わらないので何も言わない。`dithered` はディザで描いているとき。
    /// そのときは 8bit より深い帯もディザで 8bit に落ちる。
    pub fn expectation(self, expected: u32, dithered: bool) -> Option<Expect> {
        let bits = match self {
            Band::Steps(bits) if bits > 8 => bits,
            Band::Smooth => 16,
            _ => return None,
        };
        if dithered {
            Some(Expect::Dithered)
        } else if bits <= expected {
            Some(Expect::Finer)
        } else if expected > 8 {
            Some(Expect::SameAs(expected))
        } else {
            Some(Expect::SameAs(8))
        }
    }
}

/// 帯の見え方の見込み。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// 一つ上の帯より縞が細かく（なめらかに）見える。
    Finer,
    /// その bit 数の段の帯と同じ縞に見える。
    SameAs(u32),
    /// ディザの帯と同じ細かい模様に見える。
    Dithered,
}

/// モニタが受ける 1 色の bit 数。Windows は GPU が送っている信号、Linux は EDID の値と
/// ドライバの上限（max bpc）の浅い方。分からなければ `None`。
pub fn monitor_bits(display: &Display) -> Option<u32> {
    display
        .signal_bits
        .or_else(|| match (display.edid_bits, display.max_bpc) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        })
}

/// vw が描いている 1 色の bit 数。描画先が深くても、10bit の描き方でなければ 8bit に落とす。
pub fn drawn_bits(mode: Option<Mode>, surface: Surface) -> u32 {
    match mode {
        Some(Mode::TenBit) => u32::from(surface.bits),
        _ => 8,
    }
}

/// 帯が細かく見えるはずの深さ。vw の描く深さとモニタの受ける深さの浅い方。
pub fn expected_bits(drawn: u32, monitor: Option<u32>) -> u32 {
    monitor.map_or(drawn, |m| m.min(drawn))
}

/// 明るさの範囲の名前と、左端の明るさ（8bit の段で数える）。
pub const RANGES: [(&str, f32); 3] = [
    ("Dark part", 8.0),
    ("Middle part", 120.0),
    ("Bright part", 224.0),
];

/// 画面の幅いっぱいに広げる明るさの幅（8bit の段で数える）。8bit の帯で 1 段が画面の 1/16 になり、
/// 10bit の帯で 1/64、12bit の帯で 1/256、14bit の帯で 1/1024 になる。幅 1920 の画面でも
/// 14bit の段は 2 ピクセル近くあり、隣の段と描き分けられる。
pub const SPAN: f32 = 16.0;

/// 階調テストの帯の画像。帯を縦に [`BANDS`] の数だけ並べ、左から右へ `start` から
/// `start + SPAN`（8bit の段）まで明るくする。
pub fn pattern(width: usize, height: usize, start: f32) -> DisplayImage16 {
    let (width, height) = (width.max(2), height.max(BANDS.len()));
    let quantize = |v: f32, bits: u32| {
        let steps = ((1u32 << bits) - 1) as f32;
        ((v * steps).round() / steps * 65535.0).round() as u16
    };
    let mut rgba = vec![0u16; width * height * 4];
    for y in 0..height {
        let band = (y * BANDS.len() / height).min(BANDS.len() - 1);
        for x in 0..width {
            let v = (start + SPAN * x as f32 / (width - 1) as f32) / 255.0;
            let value = match BANDS[band] {
                Band::Steps(bits) => quantize(v, bits),
                Band::Smooth => (v * 65535.0).round() as u16,
                Band::Dither => {
                    u16::from(deep::dither_value((v * 65535.0).round() as u16, x, y)) * 257
                }
            };
            let i = (y * width + x) * 4;
            rgba[i..i + 4].copy_from_slice(&[value, value, value, 65535]);
        }
    }
    DisplayImage16 {
        width: width as u32,
        height: height as u32,
        rgba,
    }
}

const TEST_OPTIONS: TextureOptions = TextureOptions::NEAREST;

/// 全画面のテストを出す。閉じたら偽。
fn show_test(ctx: &egui::Context, s: &mut Showing, surface: Surface, lang: Language) -> bool {
    let id = egui::ViewportId::from_hash_of("vw-gradation-test");
    // 枠なしでモニタと同じ位置と大きさに開き、次のフレームで全画面にする。全画面はウインドウの
    // あるモニタで開くので、拡大率の換算で少しずれても選んだモニタで開く。全画面を受けない
    // 環境（ウインドウマネージャのない X11 など）でも、モニタを覆うので見られる。
    let mut builder = egui::ViewportBuilder::default()
        .with_title(tr(lang, "Display gradation test"))
        .with_decorations(false)
        .with_inner_size([640.0, 480.0]);
    if let Some(d) = &s.display {
        let [x, y, w, h] = d.rect;
        let scale = d.scale_percent.max(1) as f32 / 100.0;
        builder = builder
            .with_position([x as f32 / scale, y as f32 / scale])
            .with_inner_size([w as f32 / scale, h as f32 / scale]);
    }
    let mut open = true;
    ctx.show_viewport_immediate(id, builder, |root, _class| {
        let ctx = &root.ctx().clone();
        s.frames += 1;
        if s.frames == 2 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        let (escape, close, left, right) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.viewport().close_requested(),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
            )
        });
        if escape || close {
            open = false;
        }
        if right {
            s.range = (s.range + 1) % RANGES.len();
        }
        if left {
            s.range = (s.range + RANGES.len() - 1) % RANGES.len();
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(root, |ui| {
                let rect = ui.max_rect();
                let ppp = ctx.pixels_per_point();
                let size = [
                    (rect.width() * ppp).round() as usize,
                    (rect.height() * ppp).round() as usize,
                ];
                let fresh = s
                    .drawn
                    .as_ref()
                    .is_some_and(|(_, at, range)| *at == size && *range == s.range);
                if !fresh {
                    let pixels = pattern(size[0], size[1], RANGES[s.range].1);
                    let drawn = match s.mode {
                        Some(mode) => Drawn::Deep(Layer::pixels(pixels, mode, surface)),
                        None => Drawn::Plain(ctx.load_texture(
                            "gradation-test",
                            plain(&pixels),
                            TEST_OPTIONS,
                        )),
                    };
                    s.drawn = Some((drawn, size, s.range));
                }
                let whole = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                let painter = ui.painter_at(rect);
                if let Some((drawn, _, _)) = &mut s.drawn {
                    match drawn {
                        Drawn::Deep(layer) => {
                            if layer.poll(ctx, TEST_OPTIONS) {
                                layer.paint(&painter, rect, whole, TEST_OPTIONS);
                            } else {
                                ctx.request_repaint_after(POLL);
                            }
                        }
                        Drawn::Plain(texture) => {
                            painter.image(texture.id(), rect, whole, Color32::WHITE);
                        }
                    }
                }
                labels(&painter, rect, s, surface, lang);
            });
    });
    open
}

/// 8bit の面にそのまま出すときの帯（16bit を丸めるだけ）。
fn plain(pixels: &DisplayImage16) -> egui::ColorImage {
    let rgba: Vec<u8> = pixels
        .rgba
        .iter()
        .map(|&v| ((u32::from(v) + 128) / 257) as u8)
        .collect();
    egui::ColorImage::from_rgba_unmultiplied([pixels.width as usize, pixels.height as usize], &rgba)
}

/// 帯の名前と、モニタ・描き方・操作の説明を重ねる。
fn labels(painter: &egui::Painter, rect: Rect, s: &Showing, surface: Surface, lang: Language) {
    let font = egui::FontId::proportional(15.0);
    let wrap = (rect.width() * 0.6).max(200.0);
    let boxed = |pos: egui::Pos2, anchor: egui::Align2, text: String| {
        let galley = painter.layout(text, font.clone(), Color32::WHITE, wrap);
        let r = anchor.anchor_size(pos, galley.size()).expand(5.0);
        painter.rect_filled(r, 3.0, Color32::from_black_alpha(200));
        painter.galley(r.min + egui::vec2(5.0, 5.0), galley, Color32::WHITE);
    };
    let drawn_bits = drawn_bits(s.mode, surface);
    let monitor_bits = s.display.as_ref().and_then(monitor_bits);
    let expected = expected_bits(drawn_bits, monitor_bits);
    let band_h = rect.height() / BANDS.len() as f32;
    for (i, band) in BANDS.iter().enumerate() {
        let top = rect.top() + band_h * i as f32;
        let mut text = tr(lang, band.name()).to_owned();
        match band.expectation(expected, s.mode == Some(Mode::Dither)) {
            Some(Expect::Finer) => {
                text.push_str(" - ");
                text.push_str(tr(lang, "should look finer than the band above"));
            }
            Some(Expect::SameAs(bits)) => {
                text.push_str(" - ");
                text.push_str(&tr1(
                    lang,
                    "should look the same as the {}-bit band now",
                    &bits.to_string(),
                ));
            }
            Some(Expect::Dithered) => {
                text.push_str(" - ");
                text.push_str(tr(lang, "should look like the dithering band"));
            }
            None => {}
        }
        boxed(
            egui::pos2(rect.left() + 12.0, top + 10.0),
            egui::Align2::LEFT_TOP,
            text,
        );
    }
    let drawn = match s.mode {
        Some(mode) => tr(lang, mode.title()).to_owned(),
        None => tr(lang, "8-bit").to_owned(),
    };
    let mut info = vec![
        s.display
            .as_ref()
            .map_or_else(|| tr(lang, "unknown").to_owned(), |d| {
                let signal = displays::signal_text(d, lang);
                if signal.is_empty() {
                    d.label()
                } else {
                    format!("{} ({signal})", d.label())
                }
            }),
        format!(
            "{}: {}  {}: {}",
            tr(lang, "Surface"),
            tr1(lang, "{}-bit per color", &surface.bits.to_string()),
            tr(lang, "Drawn"),
            drawn
        ),
        format!(
            "{}: {}  ({})",
            tr(lang, "Brightness"),
            tr(lang, RANGES[s.range].0),
            tr(lang, "Left/Right: change, Esc: close")
        ),
        format!(
            "{}: {}  ({}: {}, {}: {})",
            tr(lang, "Expected depth"),
            tr1(lang, "{}-bit per color", &expected.to_string()),
            tr(lang, "vw draws"),
            tr1(lang, "{}-bit per color", &drawn_bits.to_string()),
            tr(lang, "Monitor"),
            monitor_bits.map_or_else(
                || tr(lang, "unknown").to_owned(),
                |m| tr1(lang, "{}-bit per color", &m.to_string())
            ),
        ),
        tr(lang, "With n-bit output, the bands down to the n-bit steps get finer one by one, and the deeper bands look the same as the n-bit band. With 8-bit output every band looks like the top one.").to_owned(),
    ];
    if s.ten_bit_setting && s.mode != Some(Mode::TenBit) {
        info.push(
            tr(lang, "10-bit is on for this monitor, but this start has an 8-bit surface. Restart vw, and see Help > System information.").to_owned(),
        );
    } else if !s.ten_bit_setting {
        info.push(tr(lang, "To draw 10-bit, turn on 10-bit for this monitor in Settings > Display and restart vw.").to_owned());
    }
    boxed(
        egui::pos2(rect.right() - 12.0, rect.bottom() - 12.0),
        egui::Align2::RIGHT_BOTTOM,
        info.join("\n"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(p: &DisplayImage16, band: usize) -> Vec<u16> {
        let (w, h) = (p.width as usize, p.height as usize);
        let y = (2 * band + 1) * h / (2 * BANDS.len());
        (0..w).map(|x| p.rgba[(y * w + x) * 4]).collect()
    }

    fn distinct(values: &[u16]) -> usize {
        let mut v = values.to_vec();
        v.dedup();
        v.len()
    }

    #[test]
    fn each_band_has_the_steps_of_its_depth() {
        let p = pattern(4096, 50, 120.0);
        // 8bit の 16 段の幅に、深さが 2bit 増えるごとにほぼ 4 倍の段が入る（16/255 の幅を
        // 2^n - 1 段で割るので、深いほど 4 倍より少し多い。両端が段の途中にかかると 1 段増える）。
        assert_eq!(distinct(&row(&p, 0)), 17);
        assert!((65..=66).contains(&distinct(&row(&p, 1))));
        assert!((257..=258).contains(&distinct(&row(&p, 2))));
        assert!((1028..=1029).contains(&distinct(&row(&p, 3))));
        assert!(distinct(&row(&p, 4)) > 2000);
    }

    #[test]
    fn bands_cover_the_same_brightness_from_left_to_right() {
        let p = pattern(1000, 50, 8.0);
        for band in 0..5 {
            let r = row(&p, band);
            let (first, last) = (f32::from(r[0]) / 257.0, f32::from(r[999]) / 257.0);
            assert!((first - 8.0).abs() < 0.6, "band {band}: {first}");
            assert!((last - 24.0).abs() < 0.6, "band {band}: {last}");
        }
    }

    #[test]
    fn the_dither_band_uses_only_8_bit_values() {
        let p = pattern(500, 50, 120.0);
        let r = row(&p, 5);
        assert!(r.iter().all(|v| v % 257 == 0));
        assert!(distinct(&r) > 17, "dithering mixes neighbouring steps");
    }

    fn display(signal: Option<u32>, edid: Option<u32>, max_bpc: Option<u32>) -> Display {
        Display {
            id: "m".into(),
            name: "m".into(),
            connector: String::new(),
            rect: [0, 0, 1920, 1080],
            scale_percent: 100,
            primary: true,
            refresh_hz: None,
            signal_bits: signal,
            hdr: None,
            edid_bits: edid,
            max_bpc,
        }
    }

    #[test]
    fn monitor_bits_prefer_the_signal_then_the_shallower_of_edid_and_max_bpc() {
        assert_eq!(monitor_bits(&display(Some(12), Some(10), None)), Some(12));
        assert_eq!(monitor_bits(&display(None, Some(12), Some(10))), Some(10));
        assert_eq!(monitor_bits(&display(None, Some(10), None)), Some(10));
        assert_eq!(monitor_bits(&display(None, None, None)), None);
    }

    #[test]
    fn expected_depth_is_the_shallower_of_vw_and_the_monitor() {
        assert_eq!(expected_bits(10, Some(12)), 10);
        assert_eq!(expected_bits(10, Some(8)), 8);
        assert_eq!(expected_bits(10, None), 10);
    }

    #[test]
    fn only_10_bit_drawing_uses_the_surface_depth() {
        let surface = Surface {
            bits: 10,
            gpu: true,
            norm16: true,
        };
        assert_eq!(drawn_bits(Some(Mode::TenBit), surface), 10);
        assert_eq!(drawn_bits(Some(Mode::Dither), surface), 8);
        assert_eq!(drawn_bits(None, surface), 8);
    }

    #[test]
    fn bands_deeper_than_the_output_look_like_the_output_band() {
        let at = |expected| {
            BANDS
                .iter()
                .map(|b| b.expectation(expected, false))
                .collect::<Vec<_>>()
        };
        use Expect::*;
        assert_eq!(
            at(10),
            [
                None,
                Some(Finer),
                Some(SameAs(10)),
                Some(SameAs(10)),
                Some(SameAs(10)),
                None
            ]
        );
        assert_eq!(
            at(12),
            [
                None,
                Some(Finer),
                Some(Finer),
                Some(SameAs(12)),
                Some(SameAs(12)),
                None
            ]
        );
        assert_eq!(
            at(8),
            [
                None,
                Some(SameAs(8)),
                Some(SameAs(8)),
                Some(SameAs(8)),
                Some(SameAs(8)),
                None
            ]
        );
        let dithered: Vec<_> = BANDS.iter().map(|b| b.expectation(8, true)).collect();
        assert_eq!(
            dithered,
            [
                None,
                Some(Dithered),
                Some(Dithered),
                Some(Dithered),
                Some(Dithered),
                None
            ]
        );
    }

    #[test]
    fn every_range_stays_inside_the_8_bit_scale() {
        for (name, start) in RANGES {
            assert!(start >= 0.0 && start + SPAN <= 255.0, "{name}");
        }
    }

    #[test]
    fn bands_are_opaque_gray() {
        let p = pattern(300, 60, 8.0);
        for px in p.rgba.chunks_exact(4) {
            assert_eq!((px[0], px[3]), (px[1], 65535));
            assert_eq!(px[1], px[2]);
        }
    }

    #[test]
    fn a_tiny_screen_still_gets_one_row_per_band() {
        let p = pattern(1, 1, 120.0);
        assert_eq!((p.width, p.height), (2, BANDS.len() as u32));
    }

    #[test]
    fn band_and_range_names_have_japanese() {
        let names = BANDS
            .iter()
            .map(|b| b.name())
            .chain(RANGES.iter().map(|(name, _)| *name));
        for name in names {
            assert_ne!(tr(Language::Japanese, name), name);
        }
    }

    #[test]
    fn copied_system_information_has_one_line_per_entry() {
        let lines = vec![
            ("vw".to_owned(), "0.8.1".to_owned()),
            ("Monitor 1".to_owned(), String::new()),
            ("Name".to_owned(), "RX660".to_owned()),
        ];
        assert_eq!(lines_text(&lines), "vw: 0.8.1\n[Monitor 1]\nName: RX660");
    }
}
