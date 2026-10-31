//! 元に戻す（Ctrl+Z）。地図で置いた位置の変更と、フラグをまとめて消したことを、新しい順に戻す。
//! 1 つの履歴にまとめるのは、どちらも Ctrl+Z で戻すので、押したときに直前の操作が戻ってほしいため。

use std::path::PathBuf;

use crate::i18n::tr;
use crate::labels::{Flag, Position};

use super::*;

/// 戻せる操作の数。
const UNDO_LIMIT: usize = 100;

/// 元に戻せる操作と、戻すのに要る変える前の値。
pub(super) enum Undo {
    Position(PathBuf, Option<Position>),
    /// まとめて消したフラグ。画像ごとの消す前のフラグ。
    Flags(Vec<(PathBuf, Flag)>),
}

impl VwApp {
    pub(super) fn remember_undo(&mut self, step: Undo) {
        self.undo.push(step);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }

    /// 最後の操作を戻す。
    pub(super) fn undo(&mut self) {
        match self.undo.pop() {
            Some(Undo::Position(path, previous)) => self.set_position(&path, previous, false),
            Some(Undo::Flags(flags)) => self.restore_flags(flags),
            None => self.log.info(tr(self.lang(), "Nothing to undo")),
        }
    }

    /// 地図のウインドウの「元に戻す」。地図を見ながら押すので、間にフラグの消去があっても
    /// 位置の変更だけを戻す。
    pub(super) fn undo_position(&mut self) {
        let last = self
            .undo
            .iter()
            .rposition(|step| matches!(step, Undo::Position(..)));
        match last.map(|i| self.undo.remove(i)) {
            Some(Undo::Position(path, previous)) => self.set_position(&path, previous, false),
            _ => self.log.info(tr(self.lang(), "Nothing to undo")),
        }
    }

    fn restore_flags(&mut self, flags: Vec<(PathBuf, Flag)>) {
        let mut restored = 0;
        for (file, flag) in &flags {
            match self.labels.set_flag(file, Some(*flag)) {
                Ok(()) => restored += 1,
                Err(e) => self.log.error(format!("{}: {e}", file.display())),
            }
        }
        self.log.info(format!("Flags restored: {restored} images"));
        self.apply_filter_to_slots();
    }
}
