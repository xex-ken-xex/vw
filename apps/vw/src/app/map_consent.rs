//! 地図を使うかを、初めて地図を開こうとしたときに聞く。
//!
//! 地図の絵は外のサーバーから取るので、既定は切っておく決まり（設定 `map.enabled`）。それでも
//! 表示メニューには「地図」をいつも出し、選んだときに聞いて、はいなら設定を入れて開く。
//! 設定を入れるまでメニューから消しておく作りをやめたのは、地図があることに気づけないため。

use eframe::egui;

use crate::i18n::{tr, tr1};

use super::*;

/// 聞いている途中の、はいのあとにすること。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MapAsk {
    /// 地図を開く。
    Open,
    /// 地図を開いて、このスロットのデータの範囲に合わせる。
    ShowSlot(usize),
}

/// 地図の絵の URL のホスト（`https://tile.example.org/{z}/…` なら `tile.example.org`）。
fn tile_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?']).next().unwrap_or(rest)
}

impl VwApp {
    /// 地図を開く。使う設定が切ってあれば、先に聞く。
    pub(super) fn toggle_map(&mut self) {
        if self.settings().map.enabled {
            self.map.open = !self.map.open;
        } else {
            self.map_ask = Some(MapAsk::Open);
        }
    }

    pub(super) fn map_ask_dialog(&mut self, ctx: &egui::Context) {
        let Some(ask) = self.map_ask else {
            return;
        };
        let lang = self.lang();
        let host = tile_host(&self.settings().map.tile_url).to_owned();
        let mut answer = None;
        let modal = vw_ui::dialog::Dialog::new("map_question")
            .max_width(480.0)
            .show(ctx, |ui| {
                ui.label(tr(lang, "Use the map?"));
                ui.label(tr1(
                    lang,
                    "The map pictures are downloaded from {} while the map is open.",
                    &host,
                ));
                ui.weak(tr(lang, "You can turn it off again in Settings > Map."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(tr(lang, "Use the map")).clicked() {
                        answer = Some(true);
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        answer = Some(false);
                    }
                });
            });
        if modal.should_close && answer.is_none() {
            answer = Some(false);
        }
        let Some(yes) = answer else {
            return;
        };
        self.map_ask = None;
        if !yes {
            return;
        }
        self.settings.settings.map.enabled = true;
        self.settings_changed(ctx);
        self.log.info("Map: the map is turned on in the settings");
        match ask {
            MapAsk::Open => self.map.open = true,
            MapAsk::ShowSlot(index) => self.show_slot_on_map(index),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_names_the_server_the_pictures_come_from() {
        assert_eq!(
            tile_host("https://tile.openstreetmap.org/{z}/{x}/{y}.png"),
            "tile.openstreetmap.org"
        );
        assert_eq!(tile_host("http://a.b:8080?z={z}"), "a.b:8080");
        assert_eq!(tile_host("tiles.local/{z}"), "tiles.local");
    }
}
