use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

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
