//! 裏のスレッドで 1 つの結果を計算し、終わったら画面を描き直してもらう。
//!
//! egui は入力がないと描き直さないので、結果を送るだけでは画面に出ない。
//! 送った直後に `request_repaint` を呼ぶ組を毎回書かずに済むようにする。

use std::sync::mpsc::{channel, Receiver};

use eframe::egui;

/// `work` を別スレッドで走らせ、結果の受け口を返す。受け口は `try_recv` で毎フレーム覗く。
pub fn compute<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(work());
        ctx.request_repaint();
    });
    rx
}
