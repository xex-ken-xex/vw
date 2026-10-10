//! ログウィンドウに出すメッセージの記録。
//!
//! `log` クレートなどを使わずに自前で持つのは、出力先が画面のログフィールドだけで、
//! 起動からの経過時間を付けたいだけだから。

use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// 起動時刻からの経過秒を付けてメッセージを溜める。
pub struct Log {
    origin: Instant,
    pub lines: Vec<(f64, Level, String)>,
}

impl Log {
    pub fn new(origin: Instant) -> Self {
        Self {
            origin,
            lines: Vec::new(),
        }
    }

    pub fn push(&mut self, level: Level, msg: impl Into<String>) {
        let t = self.origin.elapsed().as_secs_f64();
        self.lines.push((t, level, msg.into()));
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.push(Level::Info, msg);
    }

    pub fn warn(&mut self, msg: impl Into<String>) {
        self.push(Level::Warn, msg);
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.push(Level::Error, msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_messages_in_order_with_level_and_elapsed_time() {
        let mut log = Log::new(Instant::now());
        log.info("a");
        log.warn("b");
        log.error("c");
        let levels: Vec<_> = log.lines.iter().map(|(_, l, m)| (*l, m.as_str())).collect();
        assert!(levels == [(Level::Info, "a"), (Level::Warn, "b"), (Level::Error, "c")]);
        assert!(log.lines.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(log.lines[0].0 >= 0.0);
    }
}
