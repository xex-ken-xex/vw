//! 色の表の選び方（メニュー）と、スロットに重ねるゲージ（色と値の目盛りの帯）。

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Sense, Stroke};

use crate::colormap::{self, Applied, Pick, Rules};
use crate::i18n::tr;
use crate::settings::{GaugeSide, Language};

/// 色の表を選ぶメニューの中身。選んだら返す。
pub fn pick_menu(
    ui: &mut egui::Ui,
    lang: Language,
    applied: &Applied,
    rules: &Rules,
) -> Option<Pick> {
    let mut chosen = None;
    // 名前の右に色の帯を描くので、長い名前と重ならない幅を取る。
    ui.set_min_width(240.0);
    let auto = palette_title(
        lang,
        rules
            .default_for(applied.kind)
            .unwrap_or(vw_core::palette::GRAY),
    );
    let auto_text = format!("{} ({auto})", tr(lang, "As in settings"));
    if ui
        .add(vw_ui::menu::item(applied.pick == Pick::Auto, auto_text, ""))
        .on_hover_text(tr(
            lang,
            "Settings > Pixel pipeline decides the color map for each kind of image.",
        ))
        .clicked()
    {
        chosen = Some(Pick::Auto);
    }
    if ui
        .add(vw_ui::menu::item(
            applied.pick == Pick::Off,
            tr(lang, "Off (gray)"),
            "",
        ))
        .clicked()
    {
        chosen = Some(Pick::Off);
    }
    ui.separator();
    for name in rules.names() {
        let checked = matches!(&applied.pick, Pick::Palette(p) if p == name);
        let title = palette_title(lang, name);
        let response = ui.add(vw_ui::menu::item(checked, title, ""));
        if let Some(p) = rules.palette(name) {
            let r = response.rect;
            let strip = Rect::from_min_max(
                pos2(r.right() - 64.0, r.center().y - 4.0),
                pos2(r.right() - 6.0, r.center().y + 4.0),
            );
            paint_strip(ui.painter(), strip, &p, false);
        }
        if response.clicked() {
            chosen = Some(Pick::Palette(name.to_owned()));
        }
    }
    if chosen.is_some() {
        ui.close();
    }
    chosen
}

/// 色の表の、画面に出す名前。自分で足した表は設定に書いた名前のまま。
pub fn palette_title(lang: Language, name: &str) -> String {
    crate::i18n::tr_text(lang, vw_core::palette::title(name))
}

/// 横（`vertical` なら下から上）に、表の左端から右端の色を並べる。
pub fn paint_strip(
    painter: &egui::Painter,
    rect: Rect,
    palette: &vw_core::Palette,
    vertical: bool,
) {
    let steps = 64;
    for i in 0..steps {
        let (a, b) = (i as f32 / steps as f32, (i + 1) as f32 / steps as f32);
        let [r, g, bl] = palette.at((a + b) / 2.0);
        let part = if vertical {
            Rect::from_min_max(
                pos2(rect.left(), rect.bottom() - rect.height() * b),
                pos2(rect.right(), rect.bottom() - rect.height() * a),
            )
        } else {
            Rect::from_min_max(
                pos2(rect.left() + rect.width() * a, rect.top()),
                pos2(rect.left() + rect.width() * b, rect.bottom()),
            )
        };
        // 隣と隙間ができないよう、半画素ずつ重ねる。
        painter.rect_filled(part.expand(0.5), 0.0, Color32::from_rgb(r, g, bl));
    }
}

/// ゲージでの操作。
pub enum GaugeEvent {
    /// 黒と白の値を変えた（`None` はファイルの値に戻す）。
    Window(Option<(f32, f32)>),
    Pick(Pick),
}

/// ドラッグしているところ。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    Low,
    High,
    Both,
}

const BAND: f32 = 14.0;
const MARGIN: f32 = 10.0;
/// 黒と白の外の値があることを示す、帯の端の三角の長さ。
const ARROW: f32 = 9.0;
/// 下に出すときに、スロットの下端に重なる色導出の帯の分だけ空ける高さ。
const BOTTOM_ROOM: f32 = 24.0;
/// ドラッグの間に読み直しを頼む間隔（秒）。
const PREVIEW_INTERVAL: f64 = 0.15;

/// スロットの画像部分 `body` に、ゲージを置く場所があるか。
pub fn fits(body: Rect, side: GaugeSide) -> bool {
    match side {
        GaugeSide::Right => body.height() >= 120.0,
        GaugeSide::Top | GaugeSide::Bottom => body.width() >= 160.0,
    }
}

/// スロットの画像部分 `body` にゲージを重ねて描き、端のドラッグと右クリックを受ける。
/// `drag` はドラッグ中の黒と白の値（離すまで読み直さず、目盛りだけ動かす）。
#[allow(clippy::too_many_arguments)]
pub fn gauge(
    ui: &mut egui::Ui,
    id: egui::Id,
    body: Rect,
    side: GaugeSide,
    lang: Language,
    applied: &Applied,
    rules: &Rules,
    drag: &mut Option<(f32, f32)>,
) -> Option<GaugeEvent> {
    let vertical = side == GaugeSide::Right;
    let long = if vertical {
        (body.height() * 0.6).clamp(60.0, 360.0)
    } else {
        (body.width() * 0.6).clamp(80.0, 420.0)
    };
    let band = match side {
        GaugeSide::Right => Rect::from_center_size(
            pos2(body.right() - MARGIN - BAND / 2.0, body.center().y),
            vec2(BAND, long),
        ),
        GaugeSide::Top => Rect::from_center_size(
            pos2(body.center().x, body.top() + MARGIN + ARROW + BAND / 2.0),
            vec2(long, BAND),
        ),
        GaugeSide::Bottom => Rect::from_center_size(
            // 下端は、マウスを乗せると色導出の帯が重なるので、その分を空ける。
            pos2(
                body.center().x,
                body.bottom() - MARGIN - 18.0 - BAND / 2.0 - BOTTOM_ROOM,
            ),
            vec2(long, BAND),
        ),
    };
    let mut shown = applied.clone();
    if let Some(w) = *drag {
        shown.window = w;
    }
    let (lo, hi) = shown.window;
    // 帯の位置 t（0 が黒）の画面上の点。
    let at = |t: f32| -> Pos2 {
        if vertical {
            pos2(band.center().x, band.bottom() - band.height() * t)
        } else {
            pos2(band.left() + band.width() * t, band.center().y)
        }
    };

    let mut event = None;
    let grip_rect = |t: f32| {
        let p = at(t);
        if vertical {
            Rect::from_center_size(p, vec2(BAND + 10.0, 12.0))
        } else {
            Rect::from_center_size(p, vec2(12.0, BAND + 10.0))
        }
    };
    let whole = ui.interact(band, id.with("band"), Sense::click_and_drag());
    let low = ui.interact(grip_rect(0.0), id.with("low"), Sense::drag());
    let high = ui.interact(grip_rect(1.0), id.with("high"), Sense::drag());
    let cursor = if vertical {
        egui::CursorIcon::ResizeVertical
    } else {
        egui::CursorIcon::ResizeHorizontal
    };
    let (low, high) = (low.on_hover_cursor(cursor), high.on_hover_cursor(cursor));
    let whole = whole
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text(tr(
            lang,
            "Drag the ends to change the black and white values, the middle to move both. Right-click to type them or reset.",
        ));
    let grip = [(&low, Grip::Low), (&high, Grip::High), (&whole, Grip::Both)]
        .into_iter()
        .find(|(r, _)| r.dragged())
        .map(|(r, g)| (r.drag_delta(), g));
    if let Some((delta, grip)) = grip {
        let px = if vertical { -delta.y } else { delta.x };
        let length = if vertical {
            band.height()
        } else {
            band.width()
        };
        let dv = px / length.max(1.0) * (hi - lo);
        let (mut a, mut b) = drag.unwrap_or(applied.window);
        match grip {
            Grip::Low => a += dv,
            Grip::High => b += dv,
            Grip::Both => {
                a += dv;
                b += dv;
            }
        }
        let gap = ((b - a).abs()).max(f32::EPSILON) * 1e-3;
        match grip {
            Grip::Low => a = a.min(b - gap),
            Grip::High => b = b.max(a + gap),
            Grip::Both => {}
        }
        *drag = Some((a, b));
        // ドラッグの間も、間を空けて読み直しを頼み、画像の色を追いつかせる。毎フレーム頼むと、
        // 大きな画像では読み直しが溜まって離したあとまで遅れるため。
        let now = ui.input(|i| i.time);
        let sent = id.with("sent");
        let last: f64 = ui.data(|d| d.get_temp(sent)).unwrap_or(f64::MIN);
        if now - last >= PREVIEW_INTERVAL {
            ui.data_mut(|d| d.insert_temp(sent, now));
            event = Some(GaugeEvent::Window(Some((a, b))));
        }
    }
    if [&low, &high, &whole].iter().any(|r| r.drag_stopped()) {
        event = drag.take().map(|w| GaugeEvent::Window(Some(w)));
    }

    let painter = ui.painter_at(body);
    let backdrop = Color32::from_black_alpha(140);
    let text_color = Color32::from_gray(230);
    let font = egui::FontId::proportional(11.0);
    // 短い帯では目盛りの数字が重なるので、長さに合わせて数を減らす。
    let length = if vertical {
        band.height()
    } else {
        band.width()
    };
    let most = if vertical { 5.0 } else { 4.0 };
    let (ticks, step) = colormap::ticks(&shown, (length / 50.0).floor().clamp(2.0, most) as usize);
    let galleys: Vec<_> = ticks
        .iter()
        .map(|(t, v)| {
            let text = colormap::tick_text(*v, step);
            (*t, painter.layout_no_wrap(text, font.clone(), text_color))
        })
        .collect();
    let unit = (!shown.scale.unit.is_empty())
        .then(|| painter.layout_no_wrap(shown.scale.unit.clone(), font.clone(), text_color))
        // 横の帯の右に入りきらない単位は出さない（切れた字だけが残るため）。
        .filter(|u| vertical || band.right() + ARROW + 6.0 + u.size().x <= body.right());

    // 目盛りの文字の分も含めて、暗い下地を敷く（明るい画像の上でも読めるように）。
    let widest = galleys.iter().map(|(_, g)| g.size().x).fold(0.0, f32::max);
    let line = galleys.first().map_or(12.0, |(_, g)| g.size().y);
    let mut area = band.expand(ARROW + 2.0);
    if vertical {
        area.min.x -= widest + 8.0;
        if let Some(u) = &unit {
            area.min.y -= u.size().y + 2.0;
            area.min.x = area.min.x.min(band.right() - u.size().x - 2.0);
        }
    } else {
        area.max.y += line + 4.0;
        if let Some(u) = &unit {
            area.max.x += u.size().x + 6.0;
        }
    }
    painter.rect_filled(area, 3.0, backdrop);

    let gray = vw_core::Palette::builtin(vw_core::palette::GRAY).expect("gray is built in");
    let palette = shown.palette.as_deref().unwrap_or(&gray);
    paint_strip(&painter, band, palette, vertical);
    let outline = Stroke::new(1.0, Color32::from_gray(200));
    painter.rect_stroke(band, 0.0, outline, egui::StrokeKind::Outside);
    // 黒と白の外の値がある側の端は三角にする。
    let (below, above) = shown
        .range
        .map_or((false, false), |(min, max)| (min < lo, max > hi));
    for (end, has, color) in [(0.0, below, palette.at(0.0)), (1.0, above, palette.at(1.0))] {
        if !has {
            continue;
        }
        let p = at(end);
        let out = if end == 0.0 { -1.0 } else { 1.0 };
        let half = BAND / 2.0;
        let points = if vertical {
            let base = p.y;
            vec![
                pos2(band.left(), base),
                pos2(band.right(), base),
                pos2(band.center().x, base - out * ARROW),
            ]
        } else {
            let base = p.x;
            vec![
                pos2(base, band.center().y - half),
                pos2(base, band.center().y + half),
                pos2(base + out * ARROW, band.center().y),
            ]
        };
        painter.add(egui::Shape::convex_polygon(
            points,
            Color32::from_rgb(color[0], color[1], color[2]),
            outline,
        ));
    }
    // 切りのよい刻みは頼んだ数より多くなることがあるので、前の数字に重なる数字は書かない。
    let mut taken: Option<Rect> = None;
    for (t, galley) in galleys {
        let p = at(t);
        let pos = if vertical {
            painter.hline(band.left() - 4.0..=band.left(), p.y, outline);
            pos2(
                band.left() - 6.0 - galley.size().x,
                p.y - galley.size().y / 2.0,
            )
        } else {
            painter.vline(p.x, band.bottom()..=band.bottom() + 4.0, outline);
            pos2(p.x - galley.size().x / 2.0, band.bottom() + 5.0)
        };
        let rect = Rect::from_min_size(pos, galley.size()).expand(2.0);
        if taken.is_some_and(|r| r.intersects(rect)) {
            continue;
        }
        taken = Some(rect);
        painter.galley(pos, galley, text_color);
    }
    if let Some(u) = unit {
        let pos = if vertical {
            pos2(
                band.right() - u.size().x,
                band.top() - ARROW - 2.0 - u.size().y,
            )
        } else {
            pos2(
                band.right() + ARROW + 6.0,
                band.center().y - u.size().y / 2.0,
            )
        };
        painter.galley(pos, u, text_color);
    }

    vw_ui::menu::context_with_fields(&whole, |ui| {
        if let Some(e) = window_fields(ui, lang, id, applied) {
            event = Some(e);
        }
        ui.separator();
        vw_ui::menu::submenu(ui, tr(lang, "Color map"), |ui| {
            if let Some(p) = pick_menu(ui, lang, applied, rules) {
                event = Some(GaugeEvent::Pick(p));
                ui.close();
            }
        });
    });
    event
}

/// 右クリックの、黒と白の値を書き込む欄と「元に戻す」。値は目盛りと同じ単位で書く。
fn window_fields(
    ui: &mut egui::Ui,
    lang: Language,
    id: egui::Id,
    applied: &Applied,
) -> Option<GaugeEvent> {
    let f = applied.scale.factor;
    let (lo, hi) = applied.window;
    // 書きかけの文字は、黒と白の値が変わったら捨てる（鍵に値を含める）。
    let key = id.with(("fields", lo.to_bits(), hi.to_bits()));
    let mut texts: [String; 2] = ui.data_mut(|d| d.get_temp(key)).unwrap_or_else(|| {
        [
            crate::histogram::number(lo as f64 * f),
            crate::histogram::number(hi as f64 * f),
        ]
    });
    let unit = &applied.scale.unit;
    let mut event = None;
    egui::Grid::new(key.with("grid"))
        .num_columns(3)
        .show(ui, |ui| {
            for (i, label) in ["Black", "White"].into_iter().enumerate() {
                ui.label(tr(lang, label));
                ui.add(egui::TextEdit::singleline(&mut texts[i]).desired_width(90.0));
                ui.weak(unit);
                ui.end_row();
            }
        });
    let parsed = (
        texts[0].trim().parse::<f64>().ok(),
        texts[1].trim().parse::<f64>().ok(),
    );
    let valid = matches!(parsed, (Some(a), Some(b)) if a < b);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(valid, egui::Button::new(tr(lang, "Apply")))
            .clicked()
        {
            if let (Some(a), Some(b)) = parsed {
                event = Some(GaugeEvent::Window(Some(((a / f) as f32, (b / f) as f32))));
            }
            ui.close();
        }
        let changed = applied.window != applied.file_window;
        if ui
            .add_enabled(
                changed,
                egui::Button::new(tr(lang, "Reset black and white")),
            )
            .clicked()
        {
            event = Some(GaugeEvent::Window(None));
            ui.close();
        }
    });
    if event.is_some() {
        ui.data_mut(|d| d.remove::<[String; 2]>(key));
    } else {
        ui.data_mut(|d| d.insert_temp(key, texts));
    }
    event
}
