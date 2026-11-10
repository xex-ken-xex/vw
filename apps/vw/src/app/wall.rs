//! スロットの領域の下の外側の「外壁の帯」（[`crate::wall_bar`]）を描く。

use eframe::egui;
use egui::{Rect, vec2};
use egui_phosphor::regular as icon;

use crate::file_order::{FileKey, Mix};
use crate::i18n::tr;
use crate::keymap::Action;
use crate::layout::{self, SlotLayout};
use crate::settings_ui::layout_title;
use crate::wall_bar::{WallPart, grid_at};

use super::menu::SortChange;
use super::slots::PageMove;
use super::*;

/// 外壁の帯の高さ。スロットの見出しと同じくらいの薄さにする。
pub(super) const WALL_BAR: f32 = 26.0;

/// レイアウトを選ぶマス目の 1 マスの大きさ。
const GRID_CELL: f32 = 16.0;

/// 外壁の帯で押されたこと。
pub(super) enum WallClick {
    Bar(BarAction),
    Page(PageMove),
    Sort(SortChange),
    Run(Action),
    Layout(SlotLayout),
}

impl VwApp {
    /// 外壁の帯を `rect` に描き、押されたことを返す。左寄せの部品を左から、右寄せの部品を
    /// 右端に並べる。
    pub(super) fn wall_bar(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        view: &layout::View,
    ) -> Option<WallClick> {
        ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
        ui.painter().hline(
            rect.x_range(),
            rect.top(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        let bar = self.settings().wall_bar.clone();
        let inner = rect.shrink2(vec2(6.0, 0.0));
        let mut click = None;
        // 右寄せの部品の幅は描くまで分からないので、前のフレームの幅で置く。
        let right_width = self.wall_right_width.min(inner.width());
        let right_rect = Rect::from_min_max(
            egui::pos2(inner.right() - right_width, inner.top()),
            inner.right_bottom(),
        );
        let left_rect = Rect::from_min_max(
            inner.left_top(),
            egui::pos2(right_rect.left() - 8.0, inner.bottom()),
        );
        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(left_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        left.set_clip_rect(left_rect.intersect(ui.clip_rect()));
        for (n, &part) in bar.left.iter().enumerate() {
            if n > 0 {
                left.separator();
            }
            if let Some(c) = self.wall_part(&mut left, part, view) {
                click = Some(c);
            }
        }
        // 前のフレームより広くなった分は右へはみ出して描き、次のフレームで左へ寄せ直す。
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let start = right.cursor().min.x;
        for (n, &part) in bar.right.iter().enumerate() {
            if n > 0 {
                right.separator();
            }
            if let Some(c) = self.wall_part(&mut right, part, view) {
                click = Some(c);
            }
        }
        let width = right.min_rect().right() - start;
        let width = if bar.right.is_empty() {
            0.0
        } else {
            width.max(0.0)
        };
        if (width - self.wall_right_width).abs() > 0.5 {
            self.wall_right_width = width;
            ui.ctx().request_repaint();
        }
        click
    }

    fn wall_part(
        &mut self,
        ui: &mut egui::Ui,
        part: WallPart,
        view: &layout::View,
    ) -> Option<WallClick> {
        match part {
            WallPart::Play => self.play_controls(ui, true).map(WallClick::Bar),
            WallPart::Sort => self.wall_sort(ui),
            WallPart::Mix => self.wall_mix(ui),
            WallPart::Layout => self.wall_layout(ui),
            WallPart::Pages => self.page_controls(ui, view).map(WallClick::Page),
        }
    }

    /// 並び順: 基準の名前（押すと並び順のメニュー）と、昇順／降順の切り替え。
    fn wall_sort(&self, ui: &mut egui::Ui) -> Option<WallClick> {
        let lang = self.lang();
        let files = self.settings().file_sort;
        let mut click = None;
        let text = format!(
            "{} {}",
            icon::SORT_ASCENDING,
            tr(lang, files.key.short_title())
        );
        let response = ui
            .button(text)
            .on_hover_text(self.with_key(tr(lang, "Change file order"), Action::CycleSortKey));
        egui::Popup::menu(&response).show(|ui| {
            if let Some(change) = self.sort_menu(ui) {
                click = Some(WallClick::Sort(change));
            }
        });
        let (glyph, tip) = if files.descending {
            (icon::ARROW_DOWN, "Descending")
        } else {
            (icon::ARROW_UP, "Ascending")
        };
        let tip = format!(
            "{}\n{}",
            tr(lang, tip),
            self.with_key(
                tr(lang, "Files in descending order"),
                Action::ToggleSortDescending
            )
        );
        if ui.button(glyph).on_hover_text(tip).clicked() {
            click = Some(WallClick::Run(Action::ToggleSortDescending));
        }
        click
    }

    /// 混ざり方: 今の並べ方を出したトグル。
    fn wall_mix(&self, ui: &mut egui::Ui) -> Option<WallClick> {
        let lang = self.lang();
        let mix = self.settings().file_sort.mix;
        let glyph = match mix {
            Mix::OneLine => icon::LIST,
            Mix::ByKind => icon::SQUARES_FOUR,
        };
        let text = format!("{glyph} {}", tr(lang, mix.short_title()));
        let tip = format!(
            "{}\n{}",
            tr(lang, mix.title()),
            self.with_key(tr(lang, "Group files by type"), Action::ToggleSortMix)
        );
        ui.selectable_label(mix == Mix::ByKind, text)
            .on_hover_text(tip)
            .clicked()
            .then_some(WallClick::Run(Action::ToggleSortMix))
    }

    /// レイアウト: 今のレイアウトを出したボタン。押すと決まった形の一覧と、n×m のマス目。
    fn wall_layout(&mut self, ui: &mut egui::Ui) -> Option<WallClick> {
        let lang = self.lang();
        let current = self.settings().viewer.layout;
        let text = format!("{} {}", icon::GRID_FOUR, layout_title(current, lang));
        let response = ui
            .button(text)
            .on_hover_text(self.with_key(tr(lang, "Choose a layout..."), Action::PickLayout));
        let open = std::mem::take(&mut self.layout_picker);
        let mut click = None;
        egui::Popup::menu(&response)
            .open_memory((open || response.clicked()).then_some(egui::SetOpenCommand::Toggle))
            .show(|ui| click = self.layout_picker_ui(ui));
        click
    }

    /// 外壁の帯にレイアウトの部品がないときに「レイアウトを選ぶ」で、`anchor` の上に浮かべる一覧。
    pub(super) fn floating_layout_picker(
        &mut self,
        ctx: &egui::Context,
        anchor: egui::Pos2,
    ) -> Option<WallClick> {
        if !self.layout_picker {
            return None;
        }
        let shown = egui::Area::new(egui::Id::new("floating_layout_picker"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .show(ui, |ui| self.layout_picker_ui(ui))
                    .inner
            });
        let outside = ctx.input(|i| i.pointer.any_click())
            && !ctx
                .pointer_interact_pos()
                .is_some_and(|p| shown.response.rect.contains(p));
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let click = shown.inner;
        if click.is_some() || outside || escape {
            self.layout_picker = false;
        }
        click
    }

    /// レイアウトの一覧と n×m のマス目。
    fn layout_picker_ui(&self, ui: &mut egui::Ui) -> Option<WallClick> {
        let lang = self.lang();
        let mut click = self.layout_menu(ui).map(WallClick::Run);
        ui.separator();
        ui.label(
            egui::RichText::new(tr(lang, "Rows and columns"))
                .small()
                .weak(),
        );
        if let Some(layout) = layout_grid(ui, self.settings().viewer.layout) {
            click = Some(WallClick::Layout(layout));
            ui.close();
        }
        click
    }
}

/// n×m を選ぶマス目。指したマスまでを塗り、押したらそのマス目を返す。
fn layout_grid(ui: &mut egui::Ui, current: SlotLayout) -> Option<SlotLayout> {
    let side = SlotLayout::MAX_SIDE as f32 * GRID_CELL;
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), egui::Sense::click());
    let hovered = response
        .hover_pos()
        .map(|p| grid_at((p.x - rect.left(), p.y - rect.top()), GRID_CELL));
    let marked = hovered.or(match current {
        SlotLayout::Grid { .. } => Some(current),
        SlotLayout::Auto => None,
    });
    let (cols, rows) = match marked {
        Some(SlotLayout::Grid { cols, rows }) => (cols, rows),
        _ => (0, 0),
    };
    let visuals = ui.visuals();
    let on = if hovered.is_some() {
        visuals.selection.bg_fill
    } else {
        visuals.widgets.inactive.bg_fill
    };
    let off = visuals.extreme_bg_color;
    let stroke = visuals.widgets.noninteractive.bg_stroke;
    let painter = ui.painter();
    for r in 0..SlotLayout::MAX_SIDE {
        for c in 0..SlotLayout::MAX_SIDE {
            let min = rect.left_top() + vec2(c as f32 * GRID_CELL, r as f32 * GRID_CELL);
            let cell = Rect::from_min_size(min, vec2(GRID_CELL, GRID_CELL)).shrink(1.0);
            let fill = if c < cols && r < rows { on } else { off };
            painter.rect_filled(cell, 1.0, fill);
            painter.rect_stroke(cell, 1.0, stroke, egui::StrokeKind::Inside);
        }
    }
    let text = if cols > 0 {
        format!("{cols} x {rows}")
    } else {
        String::from(" ")
    };
    ui.label(egui::RichText::new(text).small());
    if response.clicked() {
        return hovered;
    }
    None
}

impl FileKey {
    /// 外壁の帯のボタンに出す短い名前。
    pub fn short_title(self) -> &'static str {
        match self {
            FileKey::NameNatural => "Name",
            FileKey::NameString => "Name (characters)",
            FileKey::Modified => "Modified",
            FileKey::Date => "Date of the data",
            FileKey::Size => "Size",
        }
    }
}

impl Mix {
    /// 外壁の帯のトグルに出す短い名前。
    pub fn short_title(self) -> &'static str {
        match self {
            Mix::OneLine => "One line",
            Mix::ByKind => "By type",
        }
    }
}
