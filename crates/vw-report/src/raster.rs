//! 記録した命令を RGBA の画像にする（クリップボードと「画像だけ」の書き出し）。

use std::sync::Arc;

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};
use vello_cpu::kurbo::{Affine, BezPath, Circle, Rect, Shape, Stroke};
use vello_cpu::peniko::{Blob, FontData};
use vello_cpu::{Glyph, Pixmap, RenderContext, Resources, color::AlphaColor};

use crate::Rgba;
use crate::canvas::{Anchor, Measure, Op};
use crate::theme::{Color, Face};

/// 字体の並び。前の字体に無い字は次の字体で描く（欧文の字体に和文が無いときなど）。
#[derive(Clone, Default)]
pub struct Fonts {
    body: Vec<FontData>,
    mono: Vec<FontData>,
}

impl Fonts {
    /// `body`・`mono` は（ファイルの中身, 集合の中の番号）の並び。読めない字体は外す。
    pub fn new(body: Vec<(Arc<Vec<u8>>, u32)>, mono: Vec<(Arc<Vec<u8>>, u32)>) -> Self {
        let load = |list: Vec<(Arc<Vec<u8>>, u32)>| -> Vec<FontData> {
            list.into_iter()
                .filter(|(bytes, index)| FontRef::from_index(bytes, *index).is_ok())
                .map(|(bytes, index)| FontData::new(Blob::new(bytes), index))
                .collect()
        };
        let body = load(body);
        let mut mono = load(mono);
        // 等幅の字体に和文が無くても、本文の字体で描けるように。
        mono.extend(body.iter().cloned());
        Self { body, mono }
    }

    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }

    fn chain(&self, face: Face) -> &[FontData] {
        match face {
            Face::Body => &self.body,
            Face::Mono => &self.mono,
        }
    }

    /// 文字ごとの（字体の番号, 字の番号, 送り幅）。どの字体にも無い字は最初の字体の .notdef。
    fn shape(&self, text: &str, size: f32, face: Face) -> Vec<(usize, u32, f32)> {
        let chain = self.chain(face);
        let refs: Vec<FontRef<'_>> = chain
            .iter()
            .filter_map(|f| FontRef::from_index(f.data.data(), f.index).ok())
            .collect();
        text.chars()
            .filter_map(|c| {
                let (i, font, gid) = refs
                    .iter()
                    .enumerate()
                    .find_map(|(i, f)| f.charmap().map(c).map(|g| (i, f, g)))
                    .or_else(|| refs.first().map(|f| (0, f, skrifa::GlyphId::NOTDEF)))?;
                let advance = font
                    .glyph_metrics(Size::new(size), LocationRef::default())
                    .advance_width(gid)
                    .unwrap_or(size * 0.5);
                Some((i, gid.to_u32(), advance))
            })
            .collect()
    }
}

impl Measure for Fonts {
    fn width(&self, text: &str, size: f32, face: Face) -> f32 {
        if self.is_empty() {
            return crate::canvas::Estimate.width(text, size, face);
        }
        self.shape(text, size, face).iter().map(|g| g.2).sum()
    }
}

fn paint(c: Color) -> AlphaColor<vello_cpu::color::Srgb> {
    let [r, g, b, a] = c.0;
    AlphaColor::from_rgba8(r, g, b, a)
}

/// 命令を `scale` 倍の画素の画像にする。命令の順に重ね、画像の上に描いた線（計測線など）は
/// 画像より手前に出す。
pub fn render(ops: &[Op], width: f32, height: f32, scale: f32, fonts: &Fonts) -> Rgba {
    let w = ((width * scale).ceil() as u16).max(1);
    let h = ((height * scale).ceil() as u16).max(1);
    let mut out = Rgba {
        width: usize::from(w),
        height: usize::from(h),
        data: vec![0; usize::from(w) * usize::from(h) * 4],
    };
    let mut start = 0;
    for (i, op) in ops.iter().enumerate() {
        if let Op::Image { rect, image } = op {
            if start < i {
                over(&mut out, &vectors(&ops[start..i], w, h, scale, fonts));
            }
            blit(&mut out, image, rect.map(|v| v * scale));
            start = i + 1;
        }
    }
    if start < ops.len() {
        over(&mut out, &vectors(&ops[start..], w, h, scale, fonts));
    }
    out
}

/// 画像のほかの命令を、透明の地に描く。
fn vectors(ops: &[Op], w: u16, h: u16, scale: f32, fonts: &Fonts) -> Rgba {
    let mut ctx = RenderContext::new(w, h);
    let mut resources = Resources::new();
    let base = Affine::scale(f64::from(scale));
    for op in ops {
        ctx.set_transform(base);
        match op {
            Op::Line {
                points,
                color,
                width,
                dash,
            } => {
                let mut stroke =
                    Stroke::new(f64::from(*width)).with_join(vello_cpu::kurbo::Join::Round);
                if !dash.is_empty() {
                    stroke = stroke.with_dashes(0.0, dash.iter().map(|&d| f64::from(d)));
                }
                ctx.set_stroke(stroke);
                ctx.set_paint(paint(*color));
                ctx.stroke_path(&polyline(points, false));
            }
            Op::Rect { rect, fill, stroke } => {
                let [x, y, w, h] = rect.map(f64::from);
                let r = Rect::new(x, y, x + w, y + h);
                fill_and_stroke(&mut ctx, &r.to_path(0.1), *fill, *stroke);
            }
            Op::Polygon {
                points,
                fill,
                stroke,
            } => fill_and_stroke(&mut ctx, &polyline(points, true), *fill, *stroke),
            Op::Circle {
                center,
                radius,
                fill,
                stroke,
            } => {
                let c = Circle::new(
                    (f64::from(center[0]), f64::from(center[1])),
                    f64::from(*radius),
                );
                fill_and_stroke(&mut ctx, &c.to_path(0.05), *fill, *stroke);
            }
            Op::Text {
                text,
                at,
                size,
                color,
                anchor,
                face,
                vertical,
            } => {
                if fonts.is_empty() {
                    continue;
                }
                let glyphs = fonts.shape(text, *size, *face);
                let total: f32 = glyphs.iter().map(|g| g.2).sum();
                let start = match anchor {
                    Anchor::Start => 0.0,
                    Anchor::Middle => -total / 2.0,
                    Anchor::End => -total,
                };
                let place = Affine::translate((f64::from(at[0]), f64::from(at[1])));
                let turn = if *vertical {
                    Affine::rotate(-std::f64::consts::FRAC_PI_2)
                } else {
                    Affine::IDENTITY
                };
                ctx.set_transform(base * place * turn);
                ctx.set_paint(paint(*color));
                let chain = fonts.chain(*face);
                let mut x = start;
                let mut i = 0;
                while i < glyphs.len() {
                    let font = glyphs[i].0;
                    let mut run = Vec::new();
                    while i < glyphs.len() && glyphs[i].0 == font {
                        run.push(Glyph {
                            id: glyphs[i].1,
                            x,
                            y: 0.0,
                        });
                        x += glyphs[i].2;
                        i += 1;
                    }
                    ctx.glyph_run(&mut resources, &chain[font])
                        .font_size(*size)
                        .hint(false)
                        .fill_glyphs(run.into_iter());
                }
            }
            Op::Image { .. } => {}
        }
    }
    ctx.flush();
    let mut pixmap = Pixmap::new(w, h);
    ctx.render(&mut pixmap, &mut resources);
    let data: Vec<u8> = pixmap
        .take_unpremultiplied()
        .into_iter()
        .flat_map(|p| [p.r, p.g, p.b, p.a])
        .collect();
    Rgba {
        width: usize::from(w),
        height: usize::from(h),
        data,
    }
}

/// `src` の RGBA（色は乗算していない）を `dst` に重ねる。
fn mix(dst: &mut [u8], src: [u8; 4]) {
    let sa = f32::from(src[3]) / 255.0;
    if sa <= 0.0 {
        return;
    }
    let da = f32::from(dst[3]) / 255.0;
    let a = sa + da * (1.0 - sa);
    for c in 0..3 {
        let v = (f32::from(src[c]) * sa + f32::from(dst[c]) * da * (1.0 - sa)) / a;
        dst[c] = v.round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (a * 255.0).round() as u8;
}

fn over(dst: &mut Rgba, src: &Rgba) {
    for (d, s) in dst.data.chunks_exact_mut(4).zip(src.data.chunks_exact(4)) {
        mix(d, [s[0], s[1], s[2], s[3]]);
    }
}

fn polyline(points: &[[f32; 2]], closed: bool) -> BezPath {
    let mut p = BezPath::new();
    for (i, q) in points.iter().enumerate() {
        let q = (f64::from(q[0]), f64::from(q[1]));
        if i == 0 {
            p.move_to(q);
        } else {
            p.line_to(q);
        }
    }
    if closed {
        p.close_path();
    }
    p
}

fn fill_and_stroke(
    ctx: &mut RenderContext,
    path: &BezPath,
    fill: Option<Color>,
    stroke: Option<(Color, f32)>,
) {
    if let Some(c) = fill {
        ctx.set_paint(paint(c));
        ctx.fill_path(path);
    }
    if let Some((c, w)) = stroke {
        ctx.set_stroke(Stroke::new(f64::from(w)));
        ctx.set_paint(paint(c));
        ctx.stroke_path(path);
    }
}

/// 画像を `rect`（画素）に、近い画素を取って縮めて置く。図の中の画像は元の画素の見た目を
/// 残したいので、補間でぼかさない。
fn blit(out: &mut Rgba, image: &Rgba, rect: [f32; 4]) {
    let [x0, y0, w, h] = rect;
    let (x0, y0) = (x0.round() as isize, y0.round() as isize);
    let (w, h) = (w.round().max(1.0) as usize, h.round().max(1.0) as usize);
    for y in 0..h {
        let ty = y0 + y as isize;
        if ty < 0 || ty as usize >= out.height {
            continue;
        }
        let sy = (y * image.height / h).min(image.height.saturating_sub(1));
        for x in 0..w {
            let tx = x0 + x as isize;
            if tx < 0 || tx as usize >= out.width {
                continue;
            }
            let sx = (x * image.width / w).min(image.width.saturating_sub(1));
            let s = (sy * image.width + sx) * 4;
            let d = (ty as usize * out.width + tx as usize) * 4;
            let src = [
                image.data[s],
                image.data[s + 1],
                image.data[s + 2],
                image.data[s + 3],
            ];
            mix(&mut out.data[d..d + 4], src);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Recorder;
    use crate::theme::Theme;

    fn pixel(img: &Rgba, x: usize, y: usize) -> [u8; 4] {
        let i = (y * img.width + x) * 4;
        img.data[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn shapes_are_drawn_at_the_given_scale() {
        let mut r = Recorder::new(&crate::canvas::Estimate);
        r.rect([0.0, 0.0, 10.0, 10.0], Some(Theme::Dark.background()), None);
        r.rect([2.0, 2.0, 4.0, 4.0], Some(Color::rgb(0xFF0000)), None);
        let img = render(&r.ops, 10.0, 10.0, 2.0, &Fonts::default());
        assert_eq!((img.width, img.height), (20, 20));
        assert_eq!(pixel(&img, 1, 1), [0x1E, 0x1F, 0x22, 255]);
        assert_eq!(pixel(&img, 8, 8), [255, 0, 0, 255]);
    }

    #[test]
    fn images_are_placed_inside_the_sheet() {
        let mut r = Recorder::new(&crate::canvas::Estimate);
        r.rect([0.0, 0.0, 8.0, 8.0], Some(Color::rgb(0xFFFFFF)), None);
        r.ops.push(Op::Image {
            rect: [4.0, 4.0, 2.0, 2.0],
            image: Rgba {
                width: 1,
                height: 1,
                data: vec![0, 0, 255, 255],
            },
        });
        let img = render(&r.ops, 8.0, 8.0, 1.0, &Fonts::default());
        assert_eq!(pixel(&img, 5, 5), [0, 0, 255, 255]);
        assert_eq!(pixel(&img, 1, 1), [255, 255, 255, 255]);
    }

    #[test]
    fn shapes_after_an_image_are_drawn_over_it() {
        let mut r = Recorder::new(&crate::canvas::Estimate);
        r.ops.push(Op::Image {
            rect: [0.0, 0.0, 8.0, 8.0],
            image: Rgba {
                width: 1,
                height: 1,
                data: vec![0, 0, 255, 255],
            },
        });
        r.rect([2.0, 2.0, 4.0, 4.0], Some(Color::rgb(0xFF0000)), None);
        let img = render(&r.ops, 8.0, 8.0, 1.0, &Fonts::default());
        assert_eq!(pixel(&img, 4, 4), [255, 0, 0, 255]);
        assert_eq!(pixel(&img, 0, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn missing_fonts_fall_back_to_estimated_widths() {
        let f = Fonts::new(vec![(Arc::new(b"not a font".to_vec()), 0)], Vec::new());
        assert!(f.is_empty());
        assert_eq!(f.width("あ", 10.0, Face::Body), 10.0);
    }
}
