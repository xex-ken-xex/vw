//! 確認の小窓。縁の色で重さを分け、ユーザーが操作した所の近くに出す。
//!
//! 高精細のモニタを何枚も並べていると、画面の中央に出した小窓は目に入らず、
//! 「何も起きない」と受け取られる。そこで次のようにする。
//! - 操作した所（ドロップした所など）の近くに出す
//! - 操作したのが別のウインドウ（別のモニタに出したペイン）なら、小窓もそのそばに別のウインドウで出す
//! - ファイルが動く・消える確認は黄色の縁にし、出たときに一度揺らす
//! - 出ている間は、タスクバーでも知らせる

use egui::{Color32, Id, Pos2, Ui, ViewportId, vec2};

/// 縁の色を決める、確認の重さ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    /// ふつうの確認（青）。
    #[default]
    Normal,
    /// ファイルが動く・消えるなど、気を付けて答えてほしい確認（黄色）。
    Caution,
}

impl Tone {
    pub fn color(self) -> Color32 {
        match self {
            Tone::Normal => Color32::from_rgb(0x4a, 0x90, 0xe2),
            Tone::Caution => Color32::from_rgb(0xf0, 0xc0, 0x30),
        }
    }
}

/// 揺らす時間（秒）。
const SHAKE_SECS: f64 = 0.3;
/// 縁の太さ。
const BORDER: f32 = 2.0;
/// 別のウインドウで出すときの、最初の大きさ。次のフレームから中身に合わせる。
const FIRST_SIZE: [f32; 2] = [480.0, 160.0];

/// 確認の小窓。[`Dialog::show`] を、答えが出るまで毎フレーム呼ぶ。
pub struct Dialog {
    id: Id,
    tone: Tone,
    title: String,
    near: Option<(ViewportId, Pos2)>,
    max_width: f32,
}

/// [`Dialog::show`] の結果。
pub struct DialogResponse<R> {
    pub inner: R,
    /// Esc や小窓の外のクリック、ウインドウを閉じるボタンで閉じようとした。
    pub should_close: bool,
}

/// 出し始めた時刻と、最後に出したフレーム。
#[derive(Clone, Copy)]
struct Shown {
    since: f64,
    frame: u64,
}

impl Dialog {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug) -> Self {
        Self {
            id: Id::unique(id),
            tone: Tone::Normal,
            title: String::new(),
            near: None,
            max_width: 480.0,
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// 別のウインドウで出すときの、ウインドウの題。
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn max_width(mut self, width: f32) -> Self {
        self.max_width = width;
        self
    }

    /// ウインドウ `viewport` の `at`（そのウインドウの中の位置）の近くに出す。
    /// 指定しなければメインのウインドウの中央。
    pub fn near(mut self, viewport: ViewportId, at: Pos2) -> Self {
        self.near = Some((viewport, at));
        self
    }

    pub fn show<R>(
        self,
        ctx: &egui::Context,
        content: impl FnOnce(&mut Ui) -> R,
    ) -> DialogResponse<R> {
        let elapsed = self.shown_for(ctx);
        let shake = self.shake(ctx, elapsed);
        match self.near {
            Some((viewport, at)) if viewport != ViewportId::ROOT => {
                let global = ctx.input(|i| {
                    i.raw
                        .viewports
                        .get(&viewport)
                        .and_then(|v| v.inner_rect)
                        .map(|r| r.min + at.to_vec2())
                });
                match global {
                    Some(global) => self.show_window(ctx, global + vec2(shake, 0.0), content),
                    None => self.show_modal(ctx, None, shake, content),
                }
            }
            near => self.show_modal(ctx, near.map(|(_, at)| at), shake, content),
        }
    }

    /// 出し続けている秒数。出し始めたフレームでは、タスクバーでも知らせる。
    fn shown_for(&self, ctx: &egui::Context) -> f64 {
        let now = ctx.input(|i| i.time);
        let frame = ctx.cumulative_frame_nr();
        let key = self.id.with("shown");
        let previous = ctx.data(|d| d.get_temp::<Shown>(key));
        // 前のフレームにも出していれば続き。間が空いたら、新しく出したものとみる。
        let since = match previous {
            Some(p) if p.frame + 1 >= frame => p.since,
            _ => {
                let attention = match self.tone {
                    Tone::Normal => egui::UserAttentionType::Informational,
                    Tone::Caution => egui::UserAttentionType::Critical,
                };
                ctx.send_viewport_cmd_to(
                    ViewportId::ROOT,
                    egui::ViewportCommand::RequestUserAttention(attention),
                );
                now
            }
        };
        ctx.data_mut(|d| d.insert_temp(key, Shown { since, frame }));
        now - since
    }

    /// 黄色の小窓を、出したときに左右へ揺らす量。
    fn shake(&self, ctx: &egui::Context, elapsed: f64) -> f32 {
        if self.tone != Tone::Caution || elapsed >= SHAKE_SECS {
            return 0.0;
        }
        ctx.request_repaint();
        shake_offset(elapsed)
    }

    fn frame(&self, ctx: &egui::Context) -> egui::Frame {
        egui::Frame::popup(&ctx.global_style()).stroke(egui::Stroke::new(BORDER, self.tone.color()))
    }

    fn show_modal<R>(
        self,
        ctx: &egui::Context,
        at: Option<Pos2>,
        shake: f32,
        content: impl FnOnce(&mut Ui) -> R,
    ) -> DialogResponse<R> {
        let area = match at {
            // 既定の場所は中央に錨を下ろしていて、位置を足しても錨が勝つので、場所は作り直す。
            Some(at) => egui::Area::new(self.id)
                .kind(egui::UiKind::Modal)
                .sense(egui::Sense::hover())
                .order(egui::Order::Foreground)
                .interactable(true)
                .fixed_pos(at + vec2(8.0 + shake, 8.0)),
            None => egui::Modal::default_area(self.id)
                .anchor(egui::Align2::CENTER_CENTER, vec2(shake, 0.0)),
        };
        let max_width = self.max_width;
        let modal = egui::Modal::new(self.id)
            .area(area)
            .frame(self.frame(ctx))
            .show(ctx, |ui| {
                ui.set_max_width(max_width);
                content(ui)
            });
        DialogResponse {
            should_close: modal.should_close(),
            inner: modal.inner,
        }
    }

    /// 別のウインドウで、`global`（画面全体の位置）の近くに出す。
    fn show_window<R>(
        self,
        ctx: &egui::Context,
        global: Pos2,
        content: impl FnOnce(&mut Ui) -> R,
    ) -> DialogResponse<R> {
        let size_key = self.id.with("size");
        let size = ctx
            .data(|d| d.get_temp::<egui::Vec2>(size_key))
            .unwrap_or(FIRST_SIZE.into());
        let builder = egui::ViewportBuilder::default()
            .with_title(self.title.clone())
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_position(global + vec2(8.0, 8.0))
            .with_inner_size(size);
        let frame = self.frame(ctx);
        let max_width = self.max_width;
        let mut content = Some(content);
        let mut inner = None;
        let mut should_close = false;
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of(self.id),
            builder,
            |root, _class| {
                let vctx = root.ctx().clone();
                should_close |= vctx
                    .input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested());
                egui::CentralPanel::default().frame(frame).show(root, |ui| {
                    ui.set_max_width(max_width);
                    if let Some(content) = content.take() {
                        let r = ui.scope(content);
                        let wanted = r.response.rect.size() + frame.total_margin().sum();
                        if (wanted - size).length() > 1.0 {
                            vctx.data_mut(|d| d.insert_temp(size_key, wanted));
                            vctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(wanted));
                        }
                        inner = Some(r.inner);
                    }
                });
            },
        );
        match (inner, content) {
            (Some(inner), _) => DialogResponse {
                inner,
                should_close,
            },
            // 別のウインドウを開けない環境では中身が呼ばれないので、メインのウインドウに出す。
            (None, Some(content)) => self.show_modal(ctx, None, 0.0, content),
            (None, None) => unreachable!("the contents run or stay unused"),
        }
    }
}

/// 揺れの横のずれ（ポイント）。だんだん小さくなり、[`SHAKE_SECS`] で 0 になる。
fn shake_offset(elapsed: f64) -> f32 {
    let fade = 1.0 - (elapsed / SHAKE_SECS).clamp(0.0, 1.0);
    ((elapsed * std::f64::consts::TAU * 12.0).sin() * 8.0 * fade) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shake_fades_out_by_the_end() {
        assert!(shake_offset(SHAKE_SECS).abs() < 1e-3);
        let peak = (0..30)
            .map(|i| shake_offset(i as f64 / 100.0).abs())
            .fold(0.0, f32::max);
        assert!(peak > 4.0);
    }

    #[test]
    fn caution_is_yellow_and_normal_is_blue() {
        let y = Tone::Caution.color();
        assert!(y.r() > 200 && y.g() > 150 && y.b() < 100);
        let b = Tone::Normal.color();
        assert!(b.b() > 200 && b.r() < 100);
    }
}
