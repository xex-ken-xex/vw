//! 1 次元のスペクトルを、スロットに折れ線のグラフで描く。

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Stroke};
use vw_core::spectrum::{AxisKind, Spectrum};

use crate::i18n::tr;
use crate::settings::Language;

/// 目盛りの字を置く余白。これより狭いスロットでは、目盛りを省いて線だけを描く。
const LEFT: f32 = 64.0;
const BOTTOM: f32 = 36.0;
const TOP: f32 = 10.0;
const RIGHT: f32 = 14.0;
const MIN_PLOT: f32 = 60.0;
const LINE: Color32 = Color32::from_rgb(96, 172, 255);

/// 横軸の値を見やすい単位に直す倍率と、その単位。
struct Unit {
    divide: f64,
    name: String,
}

fn unit_for(kind: &AxisKind, magnitude: f64) -> Unit {
    let pick = |steps: &[(f64, &str)]| {
        let (divide, name) = steps
            .iter()
            .find(|(d, _)| magnitude >= *d)
            .copied()
            .unwrap_or(*steps.last().expect("at least one unit"));
        Unit {
            divide,
            name: name.to_owned(),
        }
    };
    match kind {
        AxisKind::Frequency => pick(&[(1e9, "GHz"), (1e6, "MHz"), (1e3, "kHz"), (1.0, "Hz")]),
        AxisKind::Velocity => pick(&[(1e3, "km/s"), (1.0, "m/s")]),
        AxisKind::Wavelength => pick(&[
            (1.0, "m"),
            (1e-3, "mm"),
            (1e-6, "µm"),
            (1e-9, "nm"),
            (1e-10, "Å"),
        ]),
        AxisKind::Other { unit, .. } => Unit {
            divide: 1.0,
            name: unit.clone(),
        },
    }
}

fn axis_title(kind: &AxisKind, lang: Language) -> String {
    match kind {
        AxisKind::Frequency => tr(lang, "Frequency").to_owned(),
        AxisKind::Velocity => tr(lang, "Velocity").to_owned(),
        AxisKind::Wavelength => tr(lang, "Wavelength").to_owned(),
        AxisKind::Other { name, .. } if name.is_empty() => tr(lang, "Channel").to_owned(),
        AxisKind::Other { name, .. } => name.clone(),
    }
}

/// `lo`〜`hi` に多くても `most` 本入る、1・2・5 × 10 の累乗の刻みの目盛り。
pub fn ticks(lo: f64, hi: f64, most: usize) -> Vec<f64> {
    let span = hi - lo;
    if !(span.is_finite() && span > 0.0) {
        return vec![lo];
    }
    let most = most.max(2) as f64;
    let power = 10f64.powf((span / most).log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * power)
        .find(|s| span / s <= most)
        .unwrap_or(10.0 * power);
    let first = (lo / step).ceil() as i64;
    let last = (hi / step).floor() as i64;
    (first..=last).map(|k| k as f64 * step).collect()
}

/// 目盛りの字。刻みに合わせて小数の桁を決め、`-0` は `0` にする。
fn label(v: f64, step: f64) -> String {
    let decimals = if step >= 1.0 {
        0
    } else {
        (-step.log10().floor()) as usize
    };
    let text = format!("{v:.decimals$}");
    if text
        .trim_start_matches('-')
        .chars()
        .all(|c| c == '0' || c == '.')
    {
        "0".to_owned()
    } else {
        text
    }
}

/// 値の短い書き方（マウスの位置の読み）。
fn short(v: f64) -> String {
    if v != 0.0 && (v.abs() >= 1e5 || v.abs() < 1e-3) {
        format!("{v:.4e}")
    } else {
        format!("{v:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

/// `body` に `s` を描く。`hover` がグラフの上なら、いちばん近い点の値を出す。
pub fn paint(
    painter: &Painter,
    body: Rect,
    s: &Spectrum,
    lang: Language,
    visuals: &egui::Visuals,
    hover: Option<Pos2>,
) {
    let n = s.values.len();
    if n < 2 {
        return;
    }
    let text = visuals.text_color();
    let faint = visuals.weak_text_color();
    let font = FontId::proportional(11.0);
    let with_axes =
        body.width() > LEFT + RIGHT + MIN_PLOT && body.height() > TOP + BOTTOM + MIN_PLOT;
    let plot = if with_axes {
        Rect::from_min_max(
            pos2(body.left() + LEFT, body.top() + TOP),
            pos2(body.right() - RIGHT, body.bottom() - BOTTOM),
        )
    } else {
        body.shrink(4.0)
    };

    let (a, b) = (s.axis.value(0.0), s.axis.value((n - 1) as f64));
    let (x_lo, x_hi) = (a.min(b), a.max(b));
    let unit = unit_for(&s.axis.kind, x_lo.abs().max(x_hi.abs()));
    let (y_lo, y_hi) = match s.value_range() {
        Some((lo, hi)) if hi > lo => (f64::from(lo), f64::from(hi)),
        Some((v, _)) => (f64::from(v) - 1.0, f64::from(v) + 1.0),
        None => (0.0, 1.0),
    };
    // 線が枠に張り付かないよう、上下に少しだけ余白を取る。
    let pad = (y_hi - y_lo) * 0.05;
    let (y_lo, y_hi) = (y_lo - pad, y_hi + pad);
    let to_x = |x: f64| {
        let t = if x_hi > x_lo {
            (x - x_lo) / (x_hi - x_lo)
        } else {
            0.5
        };
        plot.left() + plot.width() * t as f32
    };
    let to_y = |y: f64| plot.bottom() - plot.height() * ((y - y_lo) / (y_hi - y_lo)) as f32;

    if with_axes {
        painter.rect_stroke(plot, 0.0, Stroke::new(1.0, faint), egui::StrokeKind::Inside);
        // 字が重ならないよう、横の目盛りは幅 80 点に 1 本まで。
        let xs = ticks(
            x_lo / unit.divide,
            x_hi / unit.divide,
            (plot.width() / 80.0) as usize,
        );
        let x_step = xs.windows(2).next().map_or(1.0, |w| w[1] - w[0]);
        for &t in &xs {
            let x = to_x(t * unit.divide);
            painter.line_segment(
                [pos2(x, plot.bottom()), pos2(x, plot.bottom() + 4.0)],
                Stroke::new(1.0, faint),
            );
            painter.line_segment(
                [pos2(x, plot.top()), pos2(x, plot.bottom())],
                Stroke::new(1.0, faint.gamma_multiply(0.25)),
            );
            painter.text(
                pos2(x, plot.bottom() + 5.0),
                Align2::CENTER_TOP,
                label(t, x_step),
                font.clone(),
                text,
            );
        }
        let ys = ticks(y_lo, y_hi, (plot.height() / 40.0).min(8.0) as usize);
        let y_step = ys.windows(2).next().map_or(1.0, |w| w[1] - w[0]);
        for &t in &ys {
            let y = to_y(t);
            painter.line_segment(
                [pos2(plot.left() - 4.0, y), pos2(plot.left(), y)],
                Stroke::new(1.0, faint),
            );
            painter.line_segment(
                [pos2(plot.left(), y), pos2(plot.right(), y)],
                Stroke::new(1.0, faint.gamma_multiply(0.25)),
            );
            painter.text(
                pos2(plot.left() - 6.0, y),
                Align2::RIGHT_CENTER,
                label(t, y_step),
                font.clone(),
                text,
            );
        }
        let x_title = match unit.name.as_str() {
            "" => axis_title(&s.axis.kind, lang),
            u => format!("{} ({u})", axis_title(&s.axis.kind, lang)),
        };
        painter.text(
            pos2(plot.center().x, body.bottom() - 2.0),
            Align2::CENTER_BOTTOM,
            x_title,
            font.clone(),
            faint,
        );
        if !s.value_unit.is_empty() {
            painter.text(
                pos2(body.left() + 4.0, plot.top()),
                Align2::LEFT_TOP,
                &s.value_unit,
                font.clone(),
                faint,
            );
        }
    }

    // 値のない点（NaN）で線を切る。
    let mut run: Vec<Pos2> = Vec::new();
    let flush = |run: &mut Vec<Pos2>| {
        if run.len() >= 2 {
            painter.add(egui::Shape::line(
                std::mem::take(run),
                Stroke::new(1.2, LINE),
            ));
        }
        run.clear();
    };
    for (i, &v) in s.values.iter().enumerate() {
        if v.is_finite() {
            run.push(pos2(to_x(s.axis.value(i as f64)), to_y(f64::from(v))));
        } else {
            flush(&mut run);
        }
    }
    flush(&mut run);

    let Some(p) = hover.filter(|p| plot.contains(*p)) else {
        return;
    };
    let t = f64::from((p.x - plot.left()) / plot.width());
    let x = x_lo + (x_hi - x_lo) * t;
    let index = (((x - s.axis.reference) / s.axis.step) + s.axis.reference_pixel - 1.0)
        .round()
        .clamp(0.0, (n - 1) as f64) as usize;
    let at = s.axis.value(index as f64);
    let sx = to_x(at);
    painter.line_segment(
        [pos2(sx, plot.top()), pos2(sx, plot.bottom())],
        Stroke::new(1.0, faint),
    );
    let v = s.values[index];
    let mut lines = vec![format!("{} {}", short(at / unit.divide), unit.name)
        .trim_end()
        .to_owned()];
    if let Some(vel) = s.radio_velocity(at) {
        lines.push(format!("{} km/s", short(vel / 1e3)));
    }
    if v.is_finite() {
        painter.circle_filled(pos2(sx, to_y(f64::from(v))), 3.0, LINE);
        lines.push(
            format!("{} {}", short(f64::from(v)), s.value_unit)
                .trim_end()
                .to_owned(),
        );
    } else {
        lines.push(tr(lang, "No value").to_owned());
    }
    lines.push(format!("#{}", index + 1));
    let galley = painter.layout_no_wrap(lines.join("\n"), font, text);
    let size = galley.size() + vec2(8.0, 6.0);
    let mut corner = pos2(sx + 8.0, plot.top() + 4.0);
    if corner.x + size.x > plot.right() {
        corner.x = sx - 8.0 - size.x;
    }
    let frame = Rect::from_min_size(corner, size);
    painter.rect_filled(frame, 3.0, visuals.extreme_bg_color.gamma_multiply(0.9));
    painter.galley(corner + vec2(4.0, 3.0), galley, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_use_round_steps_inside_the_range() {
        assert_eq!(ticks(0.0, 10.0, 6), vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        let t = ticks(101.6, 102.2, 8);
        assert!(t.len() >= 4 && t.len() <= 8, "{t:?}");
        assert!(t.iter().all(|v| (101.6..=102.2).contains(v)));
    }

    #[test]
    fn ticks_of_an_empty_range_are_one_point() {
        assert_eq!(ticks(3.0, 3.0, 6), vec![3.0]);
    }

    #[test]
    fn narrow_graphs_get_fewer_ticks() {
        assert!(ticks(-2000.0, 900.0, 2).len() <= 3);
    }

    #[test]
    fn labels_have_as_many_decimals_as_the_step_needs() {
        assert_eq!(label(101.8, 0.1), "101.8");
        assert_eq!(label(20.0, 5.0), "20");
        assert_eq!(label(-0.0, 0.5), "0");
    }

    #[test]
    fn axis_units_follow_the_size_of_the_values() {
        assert_eq!(unit_for(&AxisKind::Frequency, 110e9).name, "GHz");
        assert_eq!(unit_for(&AxisKind::Velocity, 2.2e7).name, "km/s");
        assert_eq!(unit_for(&AxisKind::Wavelength, 2.9e-3).name, "mm");
    }
}
