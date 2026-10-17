//! 地図へのドロップで撮影地点を置く・消す・元に戻す。

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::frames;
use crate::labels::Position;
use crate::viewer::DraggedFile;

use super::*;

/// 戻せる位置の変更の数。
const UNDO_LIMIT: usize = 100;

/// 戻すときに使う、変える前の位置。
pub(super) type PositionUndo = Vec<(PathBuf, Option<Position>)>;

impl VwApp {
    /// ドラッグ中の画像が地図の上にあれば放す所に印を出し、そこで放したら位置を置く。
    ///
    /// ツリーやフォルダのスロットが受け取るより前に、ここで取る。地図のウインドウが
    /// メインのウインドウに重なっていると、その下のフォルダへの移動としても受けてしまうため。
    pub(super) fn drop_on_map(&mut self, ctx: &egui::Context) {
        let Some(file) = egui::DragAndDrop::payload::<DraggedFile>(ctx) else {
            self.map.drop_hover = None;
            return;
        };
        let released = ctx.input(|i| i.pointer.any_released());
        let global =
            ctx.input(|i| Some(i.viewport().inner_rect?.min + i.pointer.latest_pos()?.to_vec2()));
        let here = global.and_then(|g| self.map.drop_point(ctx, g));
        // 放したときは、マウスがメインのウインドウを出た扱いになって位置が消えることがある
        // （X11 で地図のウインドウが重なっているとき）ので、そのときは前の位置を使う。
        let target = here.or(self.map.drop_hover.filter(|_| released));
        self.map.drop_hover = target.filter(|_| !released);
        if let Some((_, position)) = target.filter(|_| released) {
            egui::DragAndDrop::clear_payload(ctx);
            self.set_position(&file.0, Some(position), true);
        }
    }

    /// 地図で置いた位置を `position` にする（`None` なら消す）。`remember` なら元に戻せるようにする。
    pub(super) fn set_position(&mut self, path: &Path, position: Option<Position>, remember: bool) {
        let file = frames::file_of(path);
        // 展開した ZIP は一時フォルダの写しなので、ラベルを書いても次に開いたときには消えている。
        if self.archives.mount_of(&file).is_some() {
            self.log
                .warn(format!("{}: files in a ZIP are read-only", file.display()));
            return;
        }
        match self.labels.set_position(&file, position) {
            Ok(previous) => {
                if remember {
                    self.position_undo.push((file.clone(), previous));
                    if self.position_undo.len() > UNDO_LIMIT {
                        self.position_undo.remove(0);
                    }
                }
                self.log.info(match position {
                    Some(p) => format!(
                        "Position: {} ({:.5}, {:.5})",
                        file.display(),
                        p.latitude,
                        p.longitude
                    ),
                    None => format!("Position removed: {}", file.display()),
                });
                self.map.position_changed(&file);
            }
            Err(e) => self.log.error(e),
        }
    }

    /// 最後の位置の変更を戻す。
    pub(super) fn undo_position(&mut self) {
        match self.position_undo.pop() {
            Some((path, previous)) => self.set_position(&path, previous, false),
            None => self.log.info(tr(self.lang(), "Nothing to undo")),
        }
    }
}
