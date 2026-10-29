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

/// 失敗しうる `work` を別スレッドで走らせる。`work` が panic しても、その説明を `Err` で返す。
/// 受け口を待つ画面が、結果の来ないまま待ち続けないようにするため。
pub fn try_compute<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Receiver<Result<T, String>> {
    compute(ctx, move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|panic| {
            Err(format!(
                "internal error: {}",
                crate::loader::panic_text(&panic)
            ))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_in_the_work_arrives_as_an_error() {
        let ctx = egui::Context::default();
        let rx = try_compute(&ctx, || -> Result<(), String> { panic!("broken date") });
        let got = rx.recv().unwrap();
        assert_eq!(got, Err("internal error: broken date".to_owned()));
    }
}
