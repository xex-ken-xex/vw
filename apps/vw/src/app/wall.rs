//! スロットの領域の下の外側の「外壁の帯」（[`crate::wall_bar`]）を描く。

use eframe::egui;
use egui::{Rect, vec2};
use egui_phosphor::regular as icon;

use crate::file_order::{Direction, FileKey, Mix};
use crate::i18n::tr;
use crate::keymap::Action;
use crate::layout::{self, SlotLayout};
use crate::settings_ui::layout_title;
use crate::wall_bar::{PartWidth, Room, WallPart, fit, grid_at};

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

/// 外壁の帯の部品の、前のフレームで測った幅。詰め方を決めるのに使う。
#[derive(Default)]
pub(super) struct WallMeasure {
    widths: HashMap<(WallPart, bool), f32>,
    separator: f32,
    more: f32,
}

impl WallMeasure {
    /// まだ描いていない形の幅は小さめに見積もる。大きめにすると、入るのに「…」にしまったまま
    /// その形を描かず、幅を測れないので。小さめなら一度描いて測り、次のフレームで決め直す。
    fn of(&self, part: WallPart) -> PartWidth {
        let full = self.widths.get(&(part, false)).copied();
        let icons = self.widths.get(&(part, true)).copied();
        let guess = ICON_GUESS;
        PartWidth {
            full: full.unwrap_or(guess),
            icons: icons.unwrap_or(guess).min(full.unwrap_or(f32::INFINITY)),
        }
    }

    /// 幅が変わったら true（次のフレームで詰め方を決め直す）。
    fn record(&mut self, key: (WallPart, bool), width: f32) -> bool {
        let old = self.widths.insert(key, width);
        old.is_none_or(|w| (w - width).abs() > 0.5)
    }
}

/// まだ描いていない形の部品の幅の見積もり。
const ICON_GUESS: f32 = 24.0;

/// 左寄せと右寄せの間の空き。
const WALL_GAP: f32 = 8.0;

impl VwApp {
    /// 外壁の帯を `rect` に描き、押されたことを返す。左寄せの部品を左から、右寄せの部品を
    /// 右端に並べる。入りきらないときは [`fit`] のとおりに詰める。
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
        let measure = &self.wall_measure;
        let left_widths: Vec<PartWidth> = bar.left.iter().map(|&p| measure.of(p)).collect();
        let right_widths: Vec<PartWidth> = bar.right.iter().map(|&p| measure.of(p)).collect();
        let fit = fit(
            &left_widths,
            &right_widths,
            Room {
                available: inner.width(),
                separator: measure.separator,
                gap: WALL_GAP,
                more: measure.more,
            },
        );
        let icons = |n: usize| n < fit.icons;
        let right_width: f32 = right_widths
            .iter()
            .enumerate()
            .map(|(i, w)| {
                if icons(bar.left.len() + i) {
                    w.icons
                } else {
                    w.full
                }
            })
            .sum::<f32>()
            + bar.right.len().saturating_sub(1) as f32 * measure.separator;
        let right_width = right_width.min(inner.width());
        let right_rect = Rect::from_min_max(
            egui::pos2(inner.right() - right_width, inner.top()),
            inner.right_bottom(),
        );
        let left_rect = Rect::from_min_max(
            inner.left_top(),
            egui::pos2(right_rect.left() - WALL_GAP, inner.bottom()),
        );
        let mut click = None;
        let mut changed = false;
        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(left_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        left.set_clip_rect(left_rect.intersect(ui.clip_rect()));
        let shown = &bar.left[..fit.left_shown];
        for (n, &part) in shown.iter().enumerate() {
            if n > 0 {
                changed |= self.wall_separator(&mut left);
            }
            let (c, width) = self.measured_part(&mut left, part, icons(n), view);
            changed |= self.wall_measure.record((part, icons(n)), width);
            click = click.or(c);
        }
        let hidden = &bar.left[fit.left_shown..];
        self.wall_overflow = hidden.to_vec();
        if !hidden.is_empty() {
            if !shown.is_empty() {
                changed |= self.wall_separator(&mut left);
            }
            let x = left.cursor().min.x;
            let response = left
                .button(icon::DOTS_THREE)
                .on_hover_text(tr(self.lang(), "More"));
            let more = left.cursor().min.x - x;
            if (more - self.wall_measure.more).abs() > 0.5 {
                self.wall_measure.more = more;
                changed = true;
            }
            let inner = egui::Popup::menu(&response)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    let mut picked = None;
                    for &part in hidden {
                        let c = ui
                            .horizontal(|ui| self.wall_part(ui, part, false, true, view))
                            .inner;
                        picked = picked.or(c);
                    }
                    picked
                });
            click = click.or(inner.and_then(|r| r.inner));
        }
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        for (n, &part) in bar.right.iter().enumerate() {
            if n > 0 {
                changed |= self.wall_separator(&mut right);
            }
            let as_icons = icons(bar.left.len() + n);
            let (c, width) = self.measured_part(&mut right, part, as_icons, view);
            changed |= self.wall_measure.record((part, as_icons), width);
            click = click.or(c);
        }
        // 帯のどこでもホイールでページを送る。ページの部品の上だけにしないのは、細い部品を
        // 狙わなくても送れるようにするため。
        if view.pages > 1
            && (bar.right.contains(&WallPart::Pages) || shown.contains(&WallPart::Pages))
            && let Some(steps) = self.page_wheel_over(ui, rect)
        {
            click = click.or(Some(WallClick::Page(PageMove::Scroll(steps))));
        }
        if changed {
            ui.ctx().request_repaint();
        }
        click
    }

    /// 区切りを描き、幅が変わったら true。
    fn wall_separator(&mut self, ui: &mut egui::Ui) -> bool {
        let x = ui.cursor().min.x;
        ui.separator();
        let width = ui.cursor().min.x - x;
        let changed = (width - self.wall_measure.separator).abs() > 0.5;
        self.wall_measure.separator = width;
        changed
    }

    /// 部品を描き、押されたことと、あとの隙間を含めた幅を返す。
    fn measured_part(
        &mut self,
        ui: &mut egui::Ui,
        part: WallPart,
        icons: bool,
        view: &layout::View,
    ) -> (Option<WallClick>, f32) {
        let x = ui.cursor().min.x;
        let click = self.wall_part(ui, part, icons, false, view);
        (click, ui.cursor().min.x - x)
    }

    /// `in_menu` は「…」のメニューの中に描くとき。メニューの中では、部品の一覧を egui の
    /// 下の段（`menu_button`）で開く。浮かぶ窓（`Popup`）にしないのは、egui では同時に 1 つしか
    /// 開けず、開くと「…」のメニューが閉じてしまうため。
    fn wall_part(
        &mut self,
        ui: &mut egui::Ui,
        part: WallPart,
        icons: bool,
        in_menu: bool,
        view: &layout::View,
    ) -> Option<WallClick> {
        match part {
            WallPart::Play => self
                .play_controls(ui, true, icons, in_menu)
                .map(WallClick::Bar),
            WallPart::Sort => self.wall_sort(ui, icons, in_menu),
            WallPart::Mix => self.wall_mix(ui, icons),
            WallPart::Layout => self.wall_layout(ui, icons, in_menu),
            WallPart::Pages => self
                .page_controls(ui, view, icons, false)
                .map(WallClick::Page),
        }
    }

    /// 並び順: 基準の名前（押すと並び順のメニュー）と、昇順／降順の切り替え。
    fn wall_sort(&self, ui: &mut egui::Ui, icons: bool, in_menu: bool) -> Option<WallClick> {
        let lang = self.lang();
        let files = self.settings().file_sort;
        let mut click = None;
        let name = tr(lang, files.key.short_title());
        let text = if icons {
            icon::SORT_ASCENDING.to_owned()
        } else {
            format!("{} {name}", icon::SORT_ASCENDING)
        };
        let tip = format!(
            "{name}\n{}",
            self.with_key(tr(lang, "Change file order"), Action::CycleSortKey)
        );
        if in_menu {
            ui.menu_button(text, |ui| {
                if let Some(change) = self.sort_menu(ui) {
                    click = Some(WallClick::Sort(change));
                }
            })
            .response
            .on_hover_text(tip);
        } else {
            let response = ui.button(text).on_hover_text(tip);
            egui::Popup::menu(&response).show(|ui| {
                if let Some(change) = self.sort_menu(ui) {
                    click = Some(WallClick::Sort(change));
                }
            });
        }
        let (glyph, tip) = match files.direction() {
            Direction::Ascending => (icon::ARROW_UP, "Ascending"),
            Direction::Descending => (icon::ARROW_DOWN, "Descending"),
            Direction::Mixed => (icon::ARROWS_DOWN_UP, "Each type has its own direction"),
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
    fn wall_mix(&self, ui: &mut egui::Ui, icons: bool) -> Option<WallClick> {
        let lang = self.lang();
        let mix = self.settings().file_sort.mix;
        let glyph = match mix {
            Mix::OneLine => icon::LIST,
            Mix::ByKind => icon::SQUARES_FOUR,
        };
        let text = if icons {
            glyph.to_owned()
        } else {
            format!("{glyph} {}", tr(lang, mix.short_title()))
        };
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
    fn wall_layout(&mut self, ui: &mut egui::Ui, icons: bool, in_menu: bool) -> Option<WallClick> {
        let lang = self.lang();
        let current = layout_title(self.settings().viewer.layout, lang);
        let text = if icons {
            icon::GRID_FOUR.to_owned()
        } else {
            format!("{} {current}", icon::GRID_FOUR)
        };
        let tip = format!(
            "{current}\n{}",
            self.with_key(tr(lang, "Choose a layout..."), Action::PickLayout)
        );
        if in_menu {
            return ui
                .menu_button(text, |ui| self.layout_picker_ui(ui))
                .inner
                .flatten();
        }
        let response = ui.button(text).on_hover_text(tip);
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
