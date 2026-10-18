//! ログ欄の中身を描く。ログの行と、ターミナル風のときはその下の状態の行。

use egui::text::LayoutJob;
use egui::{Color32, FontId, Response, TextFormat, TextWrapMode, Ui};

use crate::log::{Level, Line, Log};
use crate::status::{Segment, Tone};

/// ターミナル風のログ欄の地の色。テーマによらず黒地にする。
pub const TERMINAL_BACKGROUND: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);
/// ログの行と状態の行の間のすき間。
const STATUS_GAP: f32 = 2.0;
/// 済んだ行の動詞と、進み具合の棒の色。uv の「Built」と同じ緑。
const DONE: Color32 = Color32::from_rgb(0x4e, 0xc9, 0x5a);

/// 描いた結果。
pub struct Shown {
    /// 右クリックのメニューを付ける、行と空いたところを合わせた範囲。
    pub menu_target: Option<Response>,
    /// 文字の範囲選択を始めた。
    pub selecting: bool,
}

/// ログの行を新しいものが下に来る順で出し、末尾に自動で追従する。ターミナル風（`terminal`）では
/// 黒地にして、`status` の行をその下に出す（uv のように、済んだ行が上に積まれ、動いているものが下に残る）。
///
/// 行ごとに別のラベルにして 1 つの文字列にまとめないのは、行ごとに色を変えるのと、
/// 新しい行が増えても選んだ範囲が行のラベルに付いたまま動かないようにするため。
pub fn show(ui: &mut Ui, log: &Log, status: &[Vec<Segment>], terminal: bool, wrap: bool) -> Shown {
    let frame = if terminal {
        egui::Frame::new()
            .fill(TERMINAL_BACKGROUND)
            .inner_margin(egui::Margin::symmetric(4, 2))
    } else {
        egui::Frame::new()
    };
    frame
        .show(ui, |ui| lines(ui, log, status, terminal, wrap))
        .inner
}

/// ログの行と状態の行を 1 つのスクロールに並べる。
///
/// 状態の行をスクロールの外の下に置かないのは、行の数が変わるたびにログの見える高さが変わり、
/// egui の末尾への追従が外れて最後のログの行が半分隠れるため。端末と同じく、上へスクロールすれば
/// 状態の行も一緒に流れる。
fn lines(ui: &mut Ui, log: &Log, status: &[Vec<Segment>], terminal: bool, wrap: bool) -> Shown {
    let mut selecting = false;
    let mut menu_target: Option<Response> = None;
    let scroll = if wrap {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    };
    scroll
        .auto_shrink(false)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_min_width(ui.available_width());
            for (i, line) in log.lines.iter().enumerate() {
                let label = if terminal {
                    terminal_line(ui, line, wrap)
                } else {
                    normal_line(ui, line, wrap)
                };
                let r = ui.push_id(i, |ui| ui.add(label)).inner;
                selecting |= r.drag_started();
                menu_target = Some(match menu_target.take() {
                    Some(m) => m.union(r),
                    None => r,
                });
            }
            if !status.is_empty() {
                ui.add_space(STATUS_GAP);
                let visuals = ui.visuals().clone();
                for line in status {
                    // 折り返さずに切り詰める。動くたびに折り返しが変わって高さが揺れないように。
                    ui.add(
                        egui::Label::new(status_job(line, terminal, &visuals))
                            .selectable(false)
                            .truncate(),
                    );
                }
            }
            let rest = ui.allocate_response(ui.available_size(), egui::Sense::click());
            menu_target = Some(match menu_target.take() {
                Some(m) => m.union(rest),
                None => rest,
            });
        });
    Shown {
        menu_target,
        selecting,
    }
}

fn status_font() -> FontId {
    FontId::monospace(12.0)
}

fn wrap_mode(wrap: bool) -> TextWrapMode {
    if wrap {
        TextWrapMode::Wrap
    } else {
        TextWrapMode::Extend
    }
}

/// 経過秒と本文を重さの色で。済んだ行は頭の動詞だけを緑にする。
fn terminal_line(ui: &Ui, line: &Line, wrap: bool) -> egui::Label {
    let color = match line.level {
        Level::Info | Level::Done => Color32::from_rgb(0xcc, 0xcc, 0xcc),
        Level::Warn => Color32::from_rgb(0xe5, 0xc0, 0x7b),
        Level::Error => Color32::from_rgb(0xf0, 0x71, 0x78),
    };
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let text = line.terminal();
    let body = text.len() - line.text.len();
    let mut job = LayoutJob::default();
    job.append(&text[..body], 0.0, TextFormat::simple(font.clone(), color));
    append_body(&mut job, line, font, color);
    egui::Label::new(job).wrap_mode(wrap_mode(wrap))
}

/// 時刻を薄く、本文を重さの色で。1 つのラベルにするのは、範囲選択でコピーしたときに時刻と本文が 1 行に並ぶようにするため。
fn normal_line(ui: &Ui, line: &Line, wrap: bool) -> egui::Label {
    let visuals = ui.visuals();
    let color = match line.level {
        Level::Info | Level::Done => visuals.text_color(),
        Level::Warn => visuals.warn_fg_color,
        Level::Error => visuals.error_fg_color,
    };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = LayoutJob::default();
    job.append(
        &format!("{}  ", line.clock()),
        0.0,
        TextFormat::simple(font.clone(), visuals.weak_text_color()),
    );
    append_body(&mut job, line, font, color);
    egui::Label::new(job).wrap_mode(wrap_mode(wrap))
}

fn append_body(job: &mut LayoutJob, line: &Line, font: FontId, color: Color32) {
    let (verb, rest) = line.text.split_at(line.verb);
    if !verb.is_empty() {
        job.append(verb, 0.0, TextFormat::simple(font.clone(), DONE));
    }
    job.append(rest, 0.0, TextFormat::simple(font, color));
}

/// 状態の 1 行。ターミナル風は黒地に決まった色、普通の見た目はテーマの色で。
pub fn status_job(line: &[Segment], terminal: bool, visuals: &egui::Visuals) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (text, tone) in line {
        let color = match (tone, terminal) {
            (Tone::Label, true) => Color32::from_rgb(0x8a, 0x8a, 0x8a),
            (Tone::Value, true) => Color32::from_rgb(0xe6, 0xe6, 0xe6),
            (Tone::Dim, true) => Color32::from_rgb(0x5c, 0x5c, 0x5c),
            (Tone::Bad, true) => Color32::from_rgb(0xf0, 0x71, 0x78),
            (Tone::Label | Tone::Dim, false) => visuals.weak_text_color(),
            (Tone::Value, false) => visuals.text_color(),
            (Tone::Bad, false) => visuals.error_fg_color,
            (Tone::Accent, _) => Color32::from_rgb(0xe0, 0x8a, 0x5a),
            (Tone::Bar, _) => DONE,
        };
        job.append(text, 0.0, TextFormat::simple(status_font(), color));
    }
    job
}
