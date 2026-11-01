//! 計測線のグラフの欄。アクティブなスロットの画像に引いた線の値を、レポートのテーマで描く。
//! 欄に出すのは書き出す図と同じもの（題・元の情報・グラフ・数値の表・説明文）で、
//! コピーやファイルに書いたときと見た目が変わらない。

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use vw_report::canvas::{Anchor, Measure, Op, Recorder};
use vw_report::{Block, Face, Sheet, Theme};

use crate::i18n::tr;
use crate::line_profile::{self, Profile, Values};
use crate::settings::Language;

use super::VwApp;

/// 書き出す形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Export {
    /// グラフの画像だけ（PNG）。
    Image,
    /// 画像・線の位置・グラフ・値の表の HTML と、点ごとの値の CSV。
    Report,
}

#[derive(Default)]
pub(super) struct LineProfilePane {
    pub open: bool,
    picker: Option<(Box<vw_tree::picker::Picker>, Export)>,
    /// 前に書き出したフォルダ。
    dir: Option<PathBuf>,
}

/// 欄に描くために集めたもの。
struct Gathered {
    path: PathBuf,
    profiles: Vec<Profile>,
    values: Values,
    color: vw_core::ColorModel,
    width: u32,
    height: u32,
    bits: u32,
}

fn labels(lang: Language) -> line_profile::Labels<'static> {
    line_profile::Labels {
        line: tr(lang, "Line"),
        distance: tr(lang, "Distance from the start"),
        pixel_value: tr(lang, "Pixel value"),
        measured_value: tr(lang, "Measured value"),
        start: tr(lang, "Start (x, y)"),
        end: tr(lang, "End (x, y)"),
        length: tr(lang, "Length"),
        min: tr(lang, "Min"),
        max: tr(lang, "Max"),
        mean: tr(lang, "Mean"),
        sd: tr(lang, "SD"),
        kind: tr(lang, "Values"),
    }
}

/// egui の字で測る（欄に描くときに、凡例や表の幅を描く字に合わせる）。
struct PainterMeasure(egui::Painter);

impl Measure for PainterMeasure {
    fn width(&self, text: &str, size: f32, face: Face) -> f32 {
        let font = match face {
            Face::Body => egui::FontId::proportional(size),
            Face::Mono => egui::FontId::monospace(size),
        };
        self.0
            .layout_no_wrap(text.to_owned(), font, Color32::WHITE)
            .size()
            .x
    }
}

impl Gathered {
    fn file_name(&self) -> String {
        let file = crate::frames::file_of(&self.path);
        let name = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match crate::frames::split(&self.path) {
            Some((_, frame)) => format!("{name}  Frame {}", frame + 1),
            None => name,
        }
    }

    fn meta(&self, lang: Language) -> Vec<String> {
        let color = match self.color {
            vw_core::ColorModel::Gray | vw_core::ColorModel::GrayAlpha => tr(lang, "gray"),
            vw_core::ColorModel::Rgb | vw_core::ColorModel::Rgba => tr(lang, "color"),
        };
        vec![
            self.file_name(),
            format!("{} × {}", self.width, self.height),
            format!("{}bit {color}", self.bits),
            crate::report_fonts::now(),
        ]
    }

    fn caption(&self, lang: Language) -> String {
        let mut text = tr(
            lang,
            "Values along each measure line from its start to its end. The values are those of the file, before the conversion for display (black and white, color map).",
        )
        .to_owned();
        if self.values.two_kinds() {
            text.push(' ');
            text.push_str(tr(
                lang,
                "The upper graph shows the pixel values stored in the file, the lower graph the measured values converted with the formula in the file.",
            ));
        }
        text
    }

    /// 欄・コピー・画像の書き出しに使う 1 枚の図。
    fn sheet(&self, lang: Language, theme: Theme) -> Sheet {
        let l = labels(lang);
        Sheet {
            title: tr(lang, "Values along the measure lines").to_owned(),
            meta: self.meta(lang),
            blocks: vec![
                Block::Chart(line_profile::chart(
                    &self.profiles,
                    self.color,
                    &self.values,
                    &l,
                    theme,
                )),
                Block::Caption(self.caption(lang)),
                Block::Table(line_profile::table(
                    &self.profiles,
                    self.color,
                    &self.values,
                    &l,
                )),
            ],
        }
    }
}

impl VwApp {
    /// アクティブなスロットの画像の線と値。線がない・元の画像を読み込み中なら `Err` で案内。
    fn gather_profiles(&self) -> Result<Gathered, &'static str> {
        let slot = self
            .slots
            .get(self.active)
            .ok_or("Choose the measure tool (M) and drag on an image to draw a line.")?;
        let shown = slot
            .shown()
            .ok_or("Choose the measure tool (M) and drag on an image to draw a line.")?;
        let lines = slot.shown_lines();
        if lines.is_empty() {
            return Err("Choose the measure tool (M) and drag on an image to draw a line.");
        }
        let full = shown.full.as_ref().ok_or("Reading the image...")?;
        let meta = &shown.info.metadata;
        let values = Values::of(meta);
        let profiles = lines
            .iter()
            .map(|l| line_profile::profile(&full.image, meta, l, &values))
            .collect();
        Ok(Gathered {
            path: shown.path.clone(),
            profiles,
            values,
            color: full.image.color,
            width: shown.info.width,
            height: shown.info.height,
            bits: shown.info.bits_per_sample,
        })
    }

    pub(super) fn line_profile_pane(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let theme = self.settings.settings.report.theme.theme();
        let gathered = self.gather_profiles();
        let ready = gathered.is_ok();
        let mut copy_graph = false;
        let mut copy_values = false;
        let mut export = None;
        let mut clear = false;
        ui.horizontal_wrapped(|ui| {
            copy_graph = ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Copy graph")))
                .on_hover_text(tr(
                    lang,
                    "Copy the graph with its title, legend and table as an image.",
                ))
                .clicked();
            copy_values = ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Copy values")))
                .on_hover_text(tr(
                    lang,
                    "Copy the value of every point as a table to paste into a spreadsheet.",
                ))
                .clicked();
            if ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Write image...")))
                .on_hover_text(tr(lang, "Write the graph as a PNG image."))
                .clicked()
            {
                export = Some(Export::Image);
            }
            if ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Write report...")))
                .on_hover_text(tr(
                    lang,
                    "Write an HTML report with the image, the lines, the graph and the values, and a CSV of every point beside it.",
                ))
                .clicked()
            {
                export = Some(Export::Report);
            }
            ui.separator();
            clear = ui
                .add_enabled(ready, egui::Button::new(tr(lang, "Remove all lines")))
                .clicked();
        });
        ui.separator();
        let g = match gathered {
            Ok(g) => g,
            Err(hint) => {
                ui.centered_and_justified(|ui| ui.weak(tr(lang, hint)));
                return;
            }
        };
        let sheet = g.sheet(lang, theme);
        egui::ScrollArea::vertical()
            .id_salt("line_profile")
            .auto_shrink(false)
            .show(ui, |ui| paint_sheet(ui, &sheet, theme, lang));
        let ctx = ui.ctx().clone();
        if copy_graph {
            let img = self.sheet_image(&sheet, lang, theme);
            let what = format!("line profile graph ({}x{})", img.size[0], img.size[1]);
            ctx.copy_image(img);
            self.copied(&ctx, &what);
        }
        if copy_values {
            let text = line_profile::rows(&g.profiles, g.color, &g.values, '\t');
            self.copy_text(&ctx, text, "line profile values");
        }
        if clear && let Some(slot) = self.slots.get_mut(self.active) {
            slot.clear_shown_lines();
        }
        if let Some(kind) = export {
            self.start_line_export(&g, lang, kind);
        }
        self.line_export_picker(&ctx, lang, theme);
    }

    fn sheet_image(&self, sheet: &Sheet, lang: Language, theme: Theme) -> egui::ColorImage {
        let words = crate::report_fonts::words(lang);
        let img = sheet.image(theme, &words, crate::report_fonts::fonts(theme));
        egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.data)
    }

    fn start_line_export(&mut self, g: &Gathered, lang: Language, kind: Export) {
        let stem = crate::frames::file_of(&g.path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let (ext, title) = match kind {
            Export::Image => ("png", "Write the graph image"),
            Export::Report => ("html", "Write the line profile report"),
        };
        let start = self.line_profile.dir.clone().or_else(|| {
            crate::frames::file_of(&g.path)
                .parent()
                .map(Path::to_path_buf)
        });
        let picker = vw_tree::picker::Picker::new(
            tr(lang, title),
            vw_tree::picker::Mode::SaveFile {
                extensions: vec![ext.to_owned()],
                name: format!("{stem}-lines-{stamp}.{ext}"),
            },
            start.as_deref(),
            crate::settings_ui::picker_labels(lang),
        );
        self.line_profile.picker = Some((Box::new(picker), kind));
    }

    fn line_export_picker(&mut self, ctx: &egui::Context, lang: Language, theme: Theme) {
        let Some((picker, kind)) = self.line_profile.picker.as_mut() else {
            return;
        };
        let kind = *kind;
        match picker.show(ctx) {
            vw_tree::picker::Outcome::Open => {}
            vw_tree::picker::Outcome::Cancelled => self.line_profile.picker = None,
            vw_tree::picker::Outcome::Chosen(path) => {
                self.line_profile.picker = None;
                self.line_profile.dir = path.parent().map(Path::to_path_buf);
                let result = match self.gather_profiles() {
                    Ok(g) => match kind {
                        Export::Image => self.write_line_image(&g, lang, theme, &path),
                        Export::Report => self.write_line_report(&g, lang, theme, &path),
                    },
                    Err(e) => Err(e.to_owned()),
                };
                match result {
                    Ok(written) => self.log.info(format!("Line profile: wrote {written}")),
                    Err(e) => self.log.error(format!("Line profile: {e}")),
                }
            }
        }
    }

    fn write_line_image(
        &self,
        g: &Gathered,
        lang: Language,
        theme: Theme,
        path: &Path,
    ) -> Result<String, String> {
        let words = crate::report_fonts::words(lang);
        let img = g
            .sheet(lang, theme)
            .image(theme, &words, crate::report_fonts::fonts(theme));
        write_new(path, &vw_report::png(&img))?;
        Ok(path.display().to_string())
    }

    fn write_line_report(
        &self,
        g: &Gathered,
        lang: Language,
        theme: Theme,
        path: &Path,
    ) -> Result<String, String> {
        let words = crate::report_fonts::words(lang);
        let l = labels(lang);
        let mut page = vw_report::html::Page::new(
            theme,
            words,
            tr(lang, "Values along the measure lines"),
            &g.meta(lang),
        );
        if let Some(picture) = self.picture_with_lines(g, theme) {
            page.image(
                &picture,
                &[],
                tr(
                    lang,
                    "The measure lines on the image. The numbers match the graph and the table.",
                ),
            );
        }
        page.chart(
            &line_profile::chart(&g.profiles, g.color, &g.values, &l, theme),
            &g.caption(lang),
        );
        page.table(
            &line_profile::table(&g.profiles, g.color, &g.values, &l),
            Some(tr(lang, "Position, length and values of each line")),
        );
        let csv_path = path.with_extension("csv");
        page.paragraph(&format!(
            "{} {}",
            tr(lang, "The value of every point is in the CSV file:"),
            csv_path.file_name().unwrap_or_default().to_string_lossy()
        ));
        write_new(path, page.finish().as_bytes())?;
        write_new(
            &csv_path,
            line_profile::rows(&g.profiles, g.color, &g.values, ',').as_bytes(),
        )?;
        Ok(format!("{} and {}", path.display(), csv_path.display()))
    }

    /// スロットに見えている画像に計測線と番号を描いた図。
    fn picture_with_lines(&self, g: &Gathered, theme: Theme) -> Option<vw_report::Rgba> {
        let (_, image) = self.slot_copy_image(self.active)?;
        let image = vw_report::Rgba {
            width: image.size[0],
            height: image.size[1],
            data: image.as_raw().to_vec(),
        };
        let fonts = crate::report_fonts::fonts(theme);
        let width = vw_report::SHEET_WIDTH;
        let k = width / image.width.max(1) as f32;
        let height = image.height as f32 * k;
        let mut r = Recorder::new(fonts);
        r.ops.push(Op::Image {
            rect: [0.0, 0.0, width, height],
            image,
        });
        for p in &g.profiles {
            let [cr, cg, cb, _] = vw_report::Theme::Lab
                .line(p.number.saturating_sub(1))
                .color
                .0;
            let color = vw_report::Color([cr, cg, cb, 255]);
            let at = |(x, y): (u32, u32)| [(x as f32 + 0.5) * k, (y as f32 + 0.5) * k];
            let (a, b) = (at(p.start), at(p.end));
            r.line(vec![a, b], vw_report::Color([0, 0, 0, 150]), 4.0, &[]);
            r.line(vec![a, b], color, 2.0, &[]);
            let label = p.number.to_string();
            let w = r.width(&label, 12.0, Face::Body) + 8.0;
            let x = (b[0] + 4.0).min(width - w);
            let y = (b[1] - 8.0).clamp(0.0, (height - 16.0).max(0.0));
            r.rect([x, y, w, 16.0], Some(color), None);
            r.text(
                label,
                [x + 4.0, y + 12.0],
                12.0,
                vw_report::Color([255, 255, 255, 255]),
            );
        }
        Some(vw_report::raster::render(
            &r.ops,
            width,
            height,
            vw_report::IMAGE_SCALE,
            fonts,
        ))
    }
}

/// 新しいファイルにだけ書く（上書きしない）。
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// 図を欄の幅に合わせて描く。狭い欄では縮める。
fn paint_sheet(ui: &mut egui::Ui, sheet: &Sheet, theme: Theme, lang: Language) {
    const MIN_WIDTH: f32 = 520.0;
    let available = ui.available_width().max(100.0);
    let logical = available.max(MIN_WIDTH);
    let scale = available / logical;
    let measure = PainterMeasure(ui.painter().clone());
    let mut r = Recorder::new(&measure);
    let words = crate::report_fonts::words(lang);
    let height = sheet.layout(&mut r, theme, &words, logical);
    let (rect, _) = ui.allocate_exact_size(vec2(available, height * scale), Sense::hover());
    paint_ops(&ui.painter_at(rect), &r.ops, rect.min, scale);
}

fn color32(c: vw_report::Color) -> Color32 {
    let [r, g, b, a] = c.0;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// 記録した命令を egui で描く。
fn paint_ops(painter: &egui::Painter, ops: &[Op], origin: Pos2, scale: f32) {
    let at = |p: [f32; 2]| origin + vec2(p[0], p[1]) * scale;
    for op in ops {
        match op {
            Op::Line {
                points,
                color,
                width,
                dash,
            } => {
                let pts: Vec<Pos2> = points.iter().map(|&p| at(p)).collect();
                let stroke = Stroke::new(width * scale.max(0.5), color32(*color));
                if dash.is_empty() {
                    painter.add(egui::Shape::line(pts, stroke));
                } else {
                    let on: Vec<f32> = dash.iter().step_by(2).map(|d| d * scale).collect();
                    let off: Vec<f32> = dash.iter().skip(1).step_by(2).map(|d| d * scale).collect();
                    painter.extend(egui::Shape::dashed_line_with_offset(
                        &pts, stroke, &on, &off, 0.0,
                    ));
                }
            }
            Op::Rect { rect, fill, stroke } => {
                let r = Rect::from_min_size(at([rect[0], rect[1]]), vec2(rect[2], rect[3]) * scale);
                if let Some(f) = fill {
                    painter.rect_filled(r, 0.0, color32(*f));
                }
                if let Some((c, w)) = stroke {
                    painter.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(w * scale, color32(*c)),
                        egui::StrokeKind::Middle,
                    );
                }
            }
            Op::Polygon {
                points,
                fill,
                stroke,
            } => {
                let pts: Vec<Pos2> = points.iter().map(|&p| at(p)).collect();
                let stroke =
                    stroke.map_or(Stroke::NONE, |(c, w)| Stroke::new(w * scale, color32(c)));
                painter.add(egui::Shape::convex_polygon(
                    pts,
                    fill.map_or(Color32::TRANSPARENT, color32),
                    stroke,
                ));
            }
            Op::Circle {
                center,
                radius,
                fill,
                stroke,
            } => {
                let stroke =
                    stroke.map_or(Stroke::NONE, |(c, w)| Stroke::new(w * scale, color32(c)));
                painter.circle(
                    at(*center),
                    radius * scale,
                    fill.map_or(Color32::TRANSPARENT, color32),
                    stroke,
                );
            }
            Op::Text {
                text,
                at: p,
                size,
                color,
                anchor,
                face,
                vertical,
            } => {
                let font = match face {
                    Face::Body => egui::FontId::proportional(size * scale),
                    Face::Mono => egui::FontId::monospace(size * scale),
                };
                let galley = painter.layout_no_wrap(text.clone(), font, color32(*color));
                let (w, h) = (galley.size().x, galley.size().y);
                let shift = match anchor {
                    Anchor::Start => 0.0,
                    Anchor::Middle => w / 2.0,
                    Anchor::End => w,
                };
                let base = at(*p);
                // SVG の文字の位置は字の基準線。egui は字の箱の左上なので、上にずらす。
                let ascent = h * 0.8;
                if *vertical {
                    let pos = pos2(base.x - ascent, base.y + shift);
                    painter.add(
                        egui::epaint::TextShape::new(pos, galley, color32(*color))
                            .with_angle(-std::f32::consts::FRAC_PI_2),
                    );
                } else {
                    painter.galley(
                        pos2(base.x - shift, base.y - ascent),
                        galley,
                        color32(*color),
                    );
                }
            }
            Op::Image { .. } => {}
        }
    }
}
