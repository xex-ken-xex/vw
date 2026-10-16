//! ヒストグラムやズームマップのような別ウインドウの、位置と大きさの記憶。

use eframe::egui;

/// 位置と大きさ（論理ピクセルの `[左, 上, 幅, 高さ]`）を起動をまたいで覚える。
///
/// 開くときに渡す値は前回の終了時のものに固定し、動かした後の値は渡し直さない。
/// egui は渡した値が前のフレームと変わるとウインドウを動かすので、毎回今の値を渡すと
/// 利用者が動かしている最中のウインドウと取り合いになるため。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubWindow {
    initial: Option<[f32; 4]>,
    current: Option<[f32; 4]>,
}

impl SubWindow {
    pub fn restored(saved: Option<[f32; 4]>) -> Self {
        Self {
            initial: saved,
            current: saved,
        }
    }

    /// 終了時に残す値。
    pub fn rect(&self) -> Option<[f32; 4]> {
        self.current
    }

    pub fn builder(&self, title: String, default_size: [f32; 2]) -> egui::ViewportBuilder {
        let b = egui::ViewportBuilder::default().with_title(title);
        match self.initial {
            Some([x, y, w, h]) => b.with_position([x, y]).with_inner_size([w, h]),
            None => b.with_inner_size(default_size),
        }
    }

    /// ウインドウの中（そのビューポートの `ctx`）で毎フレーム呼ぶ。
    pub fn track(&mut self, ctx: &egui::Context) {
        let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
        if let (Some(outer), Some(inner)) = (outer, inner) {
            self.current = Some([outer.min.x, outer.min.y, inner.width(), inner.height()]);
        }
    }
}
