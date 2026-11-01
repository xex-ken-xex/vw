//! 8bit より深い画像（16bit の PNG・TIFF、FITS、現像した RAW など）を、8bit に落とさずに描く。
//!
//! - 深く描く: 描画先の面が 10bit（Rgb10a2Unorm）か 16bit の浮動小数点（Rgba16Float）のとき、画像を
//!   16bit のテクスチャに入れ、egui の絵の間に自前のシェーダーで描く。egui のテクスチャは 8bit（RGBA8）
//!   しかないため。
//! - ディザ: 8bit に落とすときに、場所ごとに決まった小さなずれ（Bayer の 8x8 の表）を足してから丸める。
//!   なめらかなグラデーションの縞が、細かい模様に変わって目立たなくなる。値・計測・書き出しは変えない。
//!
//! どちらも、1 枚を大きく出しているスロット（1x1・最大化）と全画面スライドショー、表示の階調テストだけで
//! 使う。16bit のテクスチャは 8bit の 2 倍の GPU のメモリが要り、並べた縮小画像では差が見えないため。

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, Weak};

use eframe::egui::{self, ColorImage, Rect, TextureHandle, TextureOptions};
use eframe::egui_wgpu::{self, wgpu};
use vw_core::display::{self, DisplayImage16, DisplayParams};
use vw_core::{Palette, VwImage};

use crate::settings::{MonitorSettings, OutputDepth};

/// 深い画像の描き方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 16bit のテクスチャで、面の深さ（10bit か 16bit の浮動小数点）のまま描く。
    Deep,
    /// 8bit に落とすときにディザをかける。
    Dither,
}

/// 描き方の名前（ログと画面に出す）。
pub fn title(mode: Option<Mode>, surface: Surface) -> &'static str {
    match mode {
        Some(Mode::Deep) if surface.linear() => "16-bit float",
        Some(Mode::Deep) => "10-bit",
        Some(Mode::Dither) => "8-bit with dithering",
        None => "8-bit",
    }
}

/// 描画先の面と GPU の、深い画像を描く用意。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Surface {
    /// 描画先の面の 1 色あたりの bit 数。
    pub bits: u8,
    /// 自前のシェーダーで描く用意ができた（wgpu で、10bit 以上の面が取れた）。
    pub gpu: bool,
    /// 16bit の整数のテクスチャ（Rgba16Unorm）を使える。使えなければ 16bit の浮動小数点
    /// （Rgba16Float）にする。こちらは 1 に近い値で 11bit ほどの細かさしかない。
    pub norm16: bool,
}

impl Surface {
    /// 16bit の浮動小数点の面。値を線形の scRGB として読むので、ガンマを外して書く。
    pub fn linear(self) -> bool {
        self.bits >= 16
    }

    /// この面に深く描いたときの、1 色あたりのおおよその細かさ（明るい所で）。
    ///
    /// 16bit の浮動小数点は仮数が 10bit で、線形の値の白に近い所では 2^-11 刻み。ガンマのかかった
    /// 明るさに直すと刻みはほぼ半分になるので 12bit とみなす（暗い所はもっと細かい）。
    /// テクスチャも浮動小数点（Rgba16Float）のときは、ガンマのかかった値の白に近い所で 11bit ほど。
    pub fn drawn_bits(self) -> u32 {
        let surface = match self.bits {
            16.. => 12,
            bits => u32::from(bits),
        };
        if self.norm16 {
            surface
        } else {
            surface.min(11)
        }
    }

    /// この面で描ける深さ。
    pub fn depth(self) -> OutputDepth {
        match self.bits {
            16.. if self.gpu => OutputDepth::Float16,
            10.. if self.gpu => OutputDepth::TenBit,
            _ => OutputDepth::Standard,
        }
    }
}

/// そのモニタで深い画像をどう描くか。深く描くのは面と GPU の用意ができたときだけ。
/// 頼んだより深い面（別のモニタが 12bit 以上を頼んだときなど）なら、その深さで描く。
pub fn mode(monitor: &MonitorSettings, surface: Surface) -> Option<Mode> {
    if monitor.depth != OutputDepth::Standard && surface.depth() != OutputDepth::Standard {
        Some(Mode::Deep)
    } else if monitor.dither {
        Some(Mode::Dither)
    } else {
        None
    }
}

/// 8bit より深い値を持つ画像か。8bit の画像は 256 段しかないので、深く描いても変わらない。
pub fn is_deep(bits_per_sample: u32) -> bool {
    bits_per_sample > 8
}

/// 起動したときに決まった [`Surface`]。設定の画面にも出すので 1 か所に置く。
static SURFACE: std::sync::OnceLock<Surface> = std::sync::OnceLock::new();

/// 起動したときに決まった描画先の面。[`install`] の前は 8bit とみなす。
pub fn surface() -> Surface {
    SURFACE.get().copied().unwrap_or(Surface {
        bits: 8,
        ..Default::default()
    })
}

/// wgpu で起動して 10bit 以上の面が取れたら、自前のシェーダーを用意する。
pub fn install(cc: &eframe::CreationContext<'_>) -> Surface {
    let surface = prepare(cc);
    let _ = SURFACE.set(surface);
    surface
}

fn prepare(cc: &eframe::CreationContext<'_>) -> Surface {
    let Some(state) = &cc.wgpu_render_state else {
        return Surface {
            bits: 8,
            ..Default::default()
        };
    };
    let bits = vw_renderer::format_bits(state.target_format);
    let norm16 = state
        .device
        .features()
        .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM);
    if bits < 10 {
        return Surface {
            bits,
            gpu: false,
            norm16,
        };
    }
    let renderer = Renderer::new(&state.device, state.target_format);
    state.renderer.write().callback_resources.insert(renderer);
    Surface {
        bits,
        gpu: true,
        norm16,
    }
}

/// 作る元。
enum Source {
    Image {
        image: Arc<VwImage>,
        palette: Option<Arc<Palette>>,
        max_side: u32,
    },
    Pixels(DisplayImage16),
}

/// 何から作ったか。同じなら作り直さない。
#[derive(Debug, Clone, PartialEq)]
struct Key {
    image: usize,
    palette: usize,
    max_side: u32,
    mode: Mode,
}

/// GPU に送る画像。ミップマップの段も CPU で作っておく（wgpu はミップマップを自動では作らない）。
pub struct Upload {
    width: u32,
    height: u32,
    /// 段ごとの RGBA16。`norm16` でなければ値は 16bit の浮動小数点の bit。
    levels: Vec<Vec<u16>>,
    norm16: bool,
}

/// 作り終えたもの。
enum Built {
    Gpu(Upload),
    Dither(ColorImage),
}

enum State {
    Building(Receiver<Built>),
    Gpu {
        id: u64,
        /// まだ GPU に送っていない画像。最初に描くときに描画の側が取り出す。
        upload: Arc<Mutex<Option<Upload>>>,
        /// 描画の側は、これが生きている間だけテクスチャを持つ。
        alive: Arc<()>,
    },
    Dither(TextureHandle, TextureOptions),
    Failed,
}

/// 1 枚の深い画像の描き方と、描くもの。
pub struct Layer {
    key: Option<Key>,
    state: State,
}

impl Layer {
    /// `image` を表示の値にして（`palette` があれば色を付けて）作り始める。
    pub fn image(
        image: &Arc<VwImage>,
        palette: Option<&Arc<Palette>>,
        max_side: u32,
        mode: Mode,
        surface: Surface,
    ) -> Self {
        let key = key(image, palette, max_side, mode);
        let source = Source::Image {
            image: Arc::clone(image),
            palette: palette.cloned(),
            max_side,
        };
        Self {
            key: Some(key),
            state: State::Building(build(source, mode, surface.norm16)),
        }
    }

    /// 作ってある 16bit の画素（表示の階調テストの帯）から作り始める。
    pub fn pixels(pixels: DisplayImage16, mode: Mode, surface: Surface) -> Self {
        Self {
            key: None,
            state: State::Building(build(Source::Pixels(pixels), mode, surface.norm16)),
        }
    }

    /// [`Layer::image`] を同じ引数で作ったものか。
    pub fn is_for(
        &self,
        image: &Arc<VwImage>,
        palette: Option<&Arc<Palette>>,
        max_side: u32,
        mode: Mode,
    ) -> bool {
        self.key.as_ref() == Some(&key(image, palette, max_side, mode))
    }

    /// 作り終えたら受け取る。描けるようになっていれば真。
    pub fn poll(&mut self, ctx: &egui::Context, options: TextureOptions) -> bool {
        if let State::Building(rx) = &self.state {
            self.state = match rx.try_recv() {
                Ok(Built::Gpu(upload)) => State::Gpu {
                    id: next_id(),
                    upload: Arc::new(Mutex::new(Some(upload))),
                    alive: Arc::new(()),
                },
                Ok(Built::Dither(image)) => {
                    State::Dither(ctx.load_texture("deep-dither", image, options), options)
                }
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => State::Failed,
            };
        }
        if let State::Dither(texture, current) = &mut self.state
            && *current != options {
                crate::viewer::set_texture_options(texture, options);
                *current = options;
            }
        matches!(self.state, State::Gpu { .. } | State::Dither(..))
    }

    /// 描けるようになっていれば `rect` に描く（`uv` は画像の中の範囲）。描けなければ偽。
    /// `options` は拡大・縮小の補間（スロットの設定と同じもの）。
    pub fn paint(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        uv: Rect,
        options: TextureOptions,
    ) -> bool {
        match &self.state {
            State::Gpu { id, upload, alive } => {
                painter.add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    Paint {
                        id: *id,
                        rect,
                        uv,
                        upload: Arc::clone(upload),
                        alive: Arc::downgrade(alive),
                        linear_magnify: options.magnification == egui::TextureFilter::Linear,
                        linear_minify: options.minification == egui::TextureFilter::Linear,
                    },
                ));
                true
            }
            State::Dither(texture, _) => {
                painter.image(texture.id(), rect, uv, egui::Color32::WHITE);
                true
            }
            State::Building(_) | State::Failed => false,
        }
    }
}

fn key(image: &Arc<VwImage>, palette: Option<&Arc<Palette>>, max_side: u32, mode: Mode) -> Key {
    Key {
        image: Arc::as_ptr(image) as usize,
        palette: palette.map_or(0, |p| Arc::as_ptr(p) as usize),
        max_side,
        mode,
    }
}

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 裏のスレッドで作る。大きな画像では表示の値にするのとミップマップに時間がかかるため。
fn build(source: Source, mode: Mode, norm16: bool) -> Receiver<Built> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let pixels = match source {
            Source::Image {
                image,
                palette,
                max_side,
            } => {
                let mut d = display::to_display16(
                    &image,
                    &DisplayParams {
                        max_side,
                        ..Default::default()
                    },
                );
                if let Some(p) = palette {
                    p.recolor16(&mut d.rgba);
                }
                d
            }
            Source::Pixels(p) => p,
        };
        let built = match mode {
            Mode::Deep => Built::Gpu(upload(pixels, norm16)),
            Mode::Dither => Built::Dither(ColorImage::from_rgba_unmultiplied(
                [pixels.width as usize, pixels.height as usize],
                &dither(&pixels.rgba, pixels.width as usize),
            )),
        };
        let _ = tx.send(built);
    });
    rx
}

fn upload(pixels: DisplayImage16, norm16: bool) -> Upload {
    let (width, height) = (pixels.width, pixels.height);
    let mut levels = vec![pixels.rgba];
    let (mut w, mut h) = (width as usize, height as usize);
    while w > 1 || h > 1 {
        let (next, nw, nh) = halve(levels.last().expect("at least one level"), w, h);
        levels.push(next);
        (w, h) = (nw, nh);
    }
    if !norm16 {
        for level in &mut levels {
            for v in level.iter_mut() {
                *v = f16_bits(f32::from(*v) / 65535.0);
            }
        }
    }
    Upload {
        width,
        height,
        levels,
        norm16,
    }
}

/// ミップマップの次の段。大きさは GPU の決まりどおり縦横を半分に切り捨て（1 より小さくしない）、
/// 1 画素はそれに重なる元の画素の平均（奇数の幅の端の余りも、隣の画素に混ぜて捨てない）。
fn halve(rgba: &[u16], w: usize, h: usize) -> (Vec<u16>, usize, usize) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let span = |i: usize, n: usize, len: usize| (i * len / n)..((i + 1) * len / n);
    let mut out = vec![0u16; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0u32; 4];
            let mut count = 0;
            for sy in span(y, nh, h) {
                for sx in span(x, nw, w) {
                    let i = (sy * w + sx) * 4;
                    for c in 0..4 {
                        sum[c] += u32::from(rgba[i + c]);
                    }
                    count += 1;
                }
            }
            let o = (y * nw + x) * 4;
            for c in 0..4 {
                out[o + c] = ((sum[c] + count / 2) / count) as u16;
            }
        }
    }
    (out, nw, nh)
}

/// 0〜1 の値を 16bit の浮動小数点（IEEE 754 binary16）の bit にする。
fn f16_bits(x: f32) -> u16 {
    if x <= 0.0 {
        return 0;
    }
    let bits = x.min(1.0).to_bits();
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    if exponent <= 0 {
        // 非正規化数（2^-14 より小さい）は 2^-24 を単位に数える。
        return (x * 16_777_216.0).round() as u16;
    }
    // 仮数の下の 13bit を丸めて落とす。繰り上がって指数に入っても、そのまま正しい値になる。
    let mantissa = (bits & 0x007f_ffff) + 0x1000;
    (((exponent as u32) << 10) + (mantissa >> 13)) as u16
}

/// 8x8 の Bayer の表。どの 2x2・4x4 の中でも値が散らばるように並べてある。
const BAYER: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60,
    28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15,
    47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];

/// RGBA16 を、ディザをかけて RGBA8 にする。アルファにはかけない。
///
/// 8bit の 1 段より細かい端数を、表の値（0〜1）と比べて切り上げるか決める。8x8 の範囲で平均すると
/// 元の値に戻る。乱数のノイズにしないのは、画像を送ったり描き直したりするたびに模様がちらつかないため。
pub fn dither(rgba: &[u16], width: usize) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for (i, (px, o)) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<4>().0)
        .enumerate()
    {
        let (x, y) = (i % width.max(1), i / width.max(1));
        for c in 0..3 {
            o[c] = dither_value(px[c], x, y);
        }
        o[3] = ((u32::from(px[3]) + 128) / 257) as u8;
    }
    out
}

/// 16bit の値 `v` を、場所 `(x, y)` のディザで 8bit にする。
pub fn dither_value(v: u16, x: usize, y: usize) -> u8 {
    let threshold = (f32::from(BAYER[(y % 8) * 8 + x % 8]) + 0.5) / 64.0;
    (f32::from(v) / 257.0 + threshold).floor().clamp(0.0, 255.0) as u8
}

/// 1 枚を描く頼み。egui が描く順のこの場所で、自前のシェーダーで描く。
struct Paint {
    id: u64,
    /// 描く矩形（ポイント）。スロットの外にはみ出してよい（egui が切り抜く）。
    rect: Rect,
    uv: Rect,
    upload: Arc<Mutex<Option<Upload>>>,
    alive: Weak<()>,
    linear_magnify: bool,
    linear_minify: bool,
}

impl egui_wgpu::CallbackTrait for Paint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(renderer) = resources.get_mut::<Renderer>() else {
            return Vec::new();
        };
        renderer.sweep();
        if !renderer.entries.contains_key(&self.id) {
            let taken = self.upload.lock().ok().and_then(|mut u| u.take());
            if let Some(upload) = taken {
                renderer.create(device, queue, self.id, &upload, self.alive.clone());
            }
        }
        let filters = (self.linear_magnify, self.linear_minify);
        let Some(entry) = renderer.entries.get(&self.id) else {
            return Vec::new();
        };
        if entry.filters != filters {
            let bind = renderer.bind_group(device, &entry.view, &entry.uniform, filters);
            let entry = renderer.entries.get_mut(&self.id).expect("checked above");
            entry.bind = bind;
            entry.filters = filters;
        }
        let entry = &renderer.entries[&self.id];
        let [w, h] = screen.size_in_pixels.map(|v| v.max(1) as f32);
        let ppp = screen.pixels_per_point;
        let ndc = |p: egui::Pos2| [p.x * ppp / w * 2.0 - 1.0, 1.0 - p.y * ppp / h * 2.0];
        let [x0, y0] = ndc(self.rect.min);
        let [x1, y1] = ndc(self.rect.max);
        let uniform: [f32; 8] = [
            x0,
            y0,
            x1,
            y1,
            self.uv.min.x,
            self.uv.min.y,
            self.uv.max.x,
            self.uv.max.y,
        ];
        queue.write_buffer(&entry.uniform, 0, bytemuck::cast_slice(&uniform));
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(renderer) = resources.get::<Renderer>() else {
            return;
        };
        let Some(entry) = renderer.entries.get(&self.id) else {
            return;
        };
        // egui は描く矩形を画面に収めた範囲を viewport にする。はみ出した画像（拡大中）が
        // 縮んで見えないよう、画面全体を viewport にして、頂点の位置を自分で決める。
        let [w, h] = info.screen_size_px;
        pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);
        pass.set_pipeline(&renderer.pipeline);
        pass.set_bind_group(0, &entry.bind, &[]);
        pass.draw(0..4, 0..1);
    }
}

/// GPU に置いた 1 枚。
struct Entry {
    view: wgpu::TextureView,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    filters: (bool, bool),
    alive: Weak<()>,
    _texture: wgpu::Texture,
}

/// 自前のシェーダーと、GPU に置いた画像。egui の描画の側（CallbackResources）に 1 つ置く。
struct Renderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// 拡大・縮小それぞれ最近傍か線形かの 4 通り。
    samplers: HashMap<(bool, bool), wgpu::Sampler>,
    entries: HashMap<u64, Entry>,
}

const SHADER: &str = r#"
struct View {
    rect: vec4<f32>,
    uv: vec4<f32>,
};
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> Out {
    let corner = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    var out: Out;
    out.position = vec4<f32>(mix(view.rect.xy, view.rect.zw, corner), 0.0, 1.0);
    out.uv = mix(view.uv.xy, view.uv.zw, corner);
    return out;
}

// egui と同じく、乗算済みのアルファで重ねる。値は表示の値（ガンマのかかった値）のまま書く。
// egui も 8bit の画像の値をそのまま面に書いているので、同じ画像なら 8bit の表示と同じ明るさになる。
@fragment
fn fs_main(v: Out) -> @location(0) vec4<f32> {
    let c = textureSample(image, image_sampler, v.uv);
    return vec4<f32>(c.rgb * c.a, c.a);
}

// 16bit の浮動小数点の面（線形の scRGB）には、sRGB のガンマを外した値を書く。egui も
// この面には線形の値を書く（fs_main_linear_framebuffer）ので、明るさが 8bit の表示と揃う。
fn linear_from_srgb(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main_linear(v: Out) -> @location(0) vec4<f32> {
    let c = textureSample(image, image_sampler, v.uv);
    return vec4<f32>(linear_from_srgb(c.rgb) * c.a, c.a);
}
"#;

/// 面の形式に合う、フラグメントシェーダーの入口。
fn fragment_entry(target: wgpu::TextureFormat) -> &'static str {
    if target == wgpu::TextureFormat::Rgba16Float {
        "fs_main_linear"
    } else {
        "fs_main"
    }
}

impl Renderer {
    fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vw deep image"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vw deep image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vw deep image"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vw deep image"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fragment_entry(target)),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let mut samplers = HashMap::new();
        for magnify in [false, true] {
            for minify in [false, true] {
                let filter = |linear| {
                    if linear {
                        wgpu::FilterMode::Linear
                    } else {
                        wgpu::FilterMode::Nearest
                    }
                };
                // 最近傍の縮小ではミップマップを使わない（viewer::texture_options と同じ理由）。
                let (mipmap, lod_max) = if minify {
                    (wgpu::MipmapFilterMode::Linear, 32.0)
                } else {
                    (wgpu::MipmapFilterMode::Nearest, 0.0)
                };
                samplers.insert(
                    (magnify, minify),
                    device.create_sampler(&wgpu::SamplerDescriptor {
                        label: Some("vw deep image"),
                        mag_filter: filter(magnify),
                        min_filter: filter(minify),
                        mipmap_filter: mipmap,
                        lod_max_clamp: lod_max,
                        ..Default::default()
                    }),
                );
            }
        }
        Self {
            pipeline,
            layout,
            samplers,
            entries: HashMap::new(),
        }
    }

    fn bind_group(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        uniform: &wgpu::Buffer,
        filters: (bool, bool),
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vw deep image"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.samplers[&filters]),
                },
            ],
        })
    }

    fn create(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: u64,
        upload: &Upload,
        alive: Weak<()>,
    ) {
        let format = if upload.norm16 {
            wgpu::TextureFormat::Rgba16Unorm
        } else {
            wgpu::TextureFormat::Rgba16Float
        };
        let size = wgpu::Extent3d {
            width: upload.width,
            height: upload.height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vw deep image"),
            size,
            mip_level_count: upload.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, data) in upload.levels.iter().enumerate() {
            let w = (upload.width >> level).max(1);
            let h = (upload.height >> level).max(1);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(data),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 8),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vw deep image"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let filters = (true, true);
        let bind = self.bind_group(device, &view, &uniform, filters);
        self.entries.insert(
            id,
            Entry {
                view,
                uniform,
                bind,
                filters,
                alive,
                _texture: texture,
            },
        );
    }

    /// 持ち主（[`Layer`]）がなくなった画像を GPU から捨てる。
    fn sweep(&mut self) {
        self.entries.retain(|_, e| e.alive.strong_count() > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(ten_bit: bool, dither: bool) -> MonitorSettings {
        MonitorSettings {
            id: "m".into(),
            depth: if ten_bit {
                OutputDepth::TenBit
            } else {
                OutputDepth::Standard
            },
            dither,
            ..Default::default()
        }
    }

    const TEN_BIT_GPU: Surface = Surface {
        bits: 10,
        gpu: true,
        norm16: true,
    };

    #[test]
    fn ten_bit_needs_the_setting_and_a_ten_bit_surface() {
        assert_eq!(mode(&monitor(true, false), TEN_BIT_GPU), Some(Mode::Deep));
        let eight = Surface {
            bits: 8,
            gpu: false,
            norm16: false,
        };
        assert_eq!(mode(&monitor(true, false), eight), None);
        assert_eq!(mode(&monitor(false, false), TEN_BIT_GPU), None);
    }

    #[test]
    fn dither_is_used_where_ten_bit_is_off_or_unavailable() {
        assert_eq!(mode(&monitor(false, true), TEN_BIT_GPU), Some(Mode::Dither));
        let eight = Surface {
            bits: 8,
            gpu: false,
            norm16: false,
        };
        assert_eq!(mode(&monitor(true, true), eight), Some(Mode::Dither));
        assert_eq!(mode(&monitor(true, true), TEN_BIT_GPU), Some(Mode::Deep));
    }

    #[test]
    fn a_monitor_asking_for_10_bit_is_drawn_deep_on_a_float_surface_too() {
        let float = Surface {
            bits: 16,
            gpu: true,
            norm16: true,
        };
        assert_eq!(mode(&monitor(true, false), float), Some(Mode::Deep));
        assert_eq!(title(Some(Mode::Deep), float), "16-bit float");
        assert_eq!(title(Some(Mode::Deep), TEN_BIT_GPU), "10-bit");
    }

    #[test]
    fn a_float_surface_draws_about_12_bits_and_a_float_texture_about_11() {
        let float = |norm16| Surface {
            bits: 16,
            gpu: true,
            norm16,
        };
        assert_eq!(float(true).drawn_bits(), 12);
        assert_eq!(float(false).drawn_bits(), 11);
        assert_eq!(TEN_BIT_GPU.drawn_bits(), 10);
        assert!(float(true).linear());
        assert!(!TEN_BIT_GPU.linear());
    }

    #[test]
    fn the_surface_tells_how_deep_it_can_draw() {
        assert_eq!(TEN_BIT_GPU.depth(), OutputDepth::TenBit);
        let no_shader = Surface {
            bits: 10,
            gpu: false,
            norm16: false,
        };
        assert_eq!(no_shader.depth(), OutputDepth::Standard);
    }

    #[test]
    fn only_the_float_surface_takes_the_linear_shader() {
        assert_eq!(
            fragment_entry(wgpu::TextureFormat::Rgba16Float),
            "fs_main_linear"
        );
        assert_eq!(fragment_entry(wgpu::TextureFormat::Rgb10a2Unorm), "fs_main");
    }

    #[test]
    fn the_shader_compiles_with_both_fragment_entries() {
        use wgpu::naga;
        let module = naga::front::wgsl::parse_str(SHADER).expect("WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("WGSL validates");
        let entries: Vec<_> = module
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        for entry in ["vs_main", "fs_main", "fs_main_linear"] {
            assert!(entries.contains(&entry), "{entries:?}");
        }
    }

    #[test]
    fn only_images_deeper_than_8_bits_are_drawn_deep() {
        assert!(!is_deep(8));
        assert!(is_deep(16));
        assert!(is_deep(32));
    }

    #[test]
    fn dither_keeps_values_that_are_already_8_bit_steps() {
        let rgba: Vec<u16> = (0..64)
            .flat_map(|_| [100 * 257, 0, 255 * 257, 65535])
            .collect();
        let out = dither(&rgba, 8);
        assert!(out.chunks(4).all(|p| p == [100, 0, 255, 255]));
    }

    #[test]
    fn dither_averages_to_the_value_between_two_steps() {
        // 8bit の 100 と 101 のちょうど 1/4 の所。
        let v = (100.25 * 257.0) as u16;
        let rgba: Vec<u16> = (0..64).flat_map(|_| [v, v, v, 65535]).collect();
        let out = dither(&rgba, 8);
        let reds: Vec<u8> = out.chunks(4).map(|p| p[0]).collect();
        assert!(reds.iter().all(|&r| r == 100 || r == 101));
        let ups = reds.iter().filter(|&&r| r == 101).count();
        assert_eq!(ups, 16, "a quarter of the 8x8 block rounds up");
    }

    #[test]
    fn mipmaps_halve_down_to_one_pixel_and_average() {
        let rgba: Vec<u16> = [0u16, 100, 200, 300]
            .iter()
            .flat_map(|&v| [v, v, v, 65535])
            .collect();
        let pixels = DisplayImage16 {
            width: 2,
            height: 2,
            rgba,
        };
        let up = upload(pixels, true);
        assert_eq!(up.levels.len(), 2);
        assert_eq!(up.levels[1], vec![150, 150, 150, 65535]);
    }

    #[test]
    fn odd_sizes_round_down_and_keep_the_edge_pixels() {
        let (out, w, h) = halve(&[10, 10, 10, 10, 20, 20, 20, 20, 60, 60, 60, 60], 3, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, vec![30, 30, 30, 30]);
    }

    #[test]
    fn mip_levels_follow_the_gpu_sizes() {
        // GPU の段の大きさは縦横を半分に切り捨てる: 320x200 は 9 段（… 5x3, 2x1, 1x1）。
        let pixels = DisplayImage16 {
            width: 320,
            height: 200,
            rgba: vec![0; 320 * 200 * 4],
        };
        let up = upload(pixels, true);
        assert_eq!(up.levels.len(), 9);
        for (level, data) in up.levels.iter().enumerate() {
            let w = (320usize >> level).max(1);
            let h = (200usize >> level).max(1);
            assert_eq!(data.len(), w * h * 4, "level {level}");
        }
    }

    #[test]
    fn half_floats_hold_unit_values() {
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(1.0), 0x3C00);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(0.25), 0x3400);
        // 非正規化数の最小。
        assert_eq!(f16_bits(1.0 / 16_777_216.0), 1);
    }

    #[test]
    fn a_layer_is_for_the_same_image_palette_size_and_mode_only() {
        let image = Arc::new(
            VwImage::new(
                1,
                1,
                vw_core::ColorModel::Gray,
                vw_core::PixelData::U16(vec![1]),
            )
            .unwrap(),
        );
        let other = Arc::new((*image).clone());
        let surface = Surface::default();
        let layer = Layer::image(&image, None, 8192, Mode::Dither, surface);
        assert!(layer.is_for(&image, None, 8192, Mode::Dither));
        assert!(!layer.is_for(&other, None, 8192, Mode::Dither));
        assert!(!layer.is_for(&image, None, 4096, Mode::Dither));
        assert!(!layer.is_for(&image, None, 8192, Mode::Deep));
    }
}
