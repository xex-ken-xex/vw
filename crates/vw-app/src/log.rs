//! ログ欄に出すメッセージの記録と、同じ内容を書き残すログファイル。
//!
//! `log` クレートなどを使わずに自前で持つのは、出力先が画面のログ欄と 1 つのファイルだけで、
//! 起動からの経過時間を付けたいだけだから。

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, Local};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Info => "INFO ",
            Level::Warn => "WARN ",
            Level::Error => "ERROR",
        }
    }
}

pub struct Line {
    /// 起動からの経過秒。
    pub elapsed: f64,
    pub at: DateTime<Local>,
    pub level: Level,
    pub text: String,
}

impl Line {
    /// ターミナル風の表示とコピーの形。起動からの経過秒を付ける。
    pub fn terminal(&self) -> String {
        format!("[{:8.3}] {}", self.elapsed, self.text)
    }

    /// 普通の表示の頭に付ける時刻。
    pub fn clock(&self) -> String {
        self.at.format("%H:%M:%S").to_string()
    }

    /// ログファイルの 1 行。日時・経過秒・重さをすべて付ける。
    fn raw(&self) -> String {
        format!(
            "{} {} [{:8.3}] {}",
            self.at.format("%Y-%m-%d %H:%M:%S%.3f %:z"),
            self.level.tag(),
            self.elapsed,
            self.text
        )
    }
}

/// 起動時刻からの経過秒を付けてメッセージを溜め、ファイルにも書く。
pub struct Log {
    origin: Instant,
    pub lines: Vec<Line>,
    file: Option<LogFile>,
}

impl Log {
    pub fn new(origin: Instant) -> Self {
        Self {
            origin,
            lines: Vec::new(),
            file: None,
        }
    }

    /// ここから先の行を `file` にも書く。それまでに溜めた行も先に書く。
    pub fn write_to(&mut self, file: LogFile) {
        for line in &self.lines {
            file.write_line(&line.raw());
        }
        self.file = Some(file);
    }

    pub fn file_path(&self) -> Option<&Path> {
        self.file.as_ref().map(|f| f.path.as_path())
    }

    pub fn push(&mut self, level: Level, msg: impl Into<String>) {
        let line = Line {
            elapsed: self.origin.elapsed().as_secs_f64(),
            at: Local::now(),
            level,
            text: msg.into(),
        };
        if let Some(f) = &self.file {
            f.write_line(&line.raw());
        }
        self.lines.push(line);
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

    /// すべての行をターミナル風の形で。1 行ずつ改行で区切る。
    pub fn all_text(&self) -> String {
        self.lines
            .iter()
            .map(Line::terminal)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 起動 1 回分のログファイル。書くたびにフラッシュする。
///
/// 書き込みをまとめないのは、落ちたときの直前の行こそ要るため。ログは数秒に数行なので遅くならない。
/// `Arc<Mutex>` で持つのは、落ちたときの知らせ（panic hook）からも同じファイルに書くため。
#[derive(Clone)]
pub struct LogFile {
    path: PathBuf,
    file: Arc<Mutex<File>>,
}

impl LogFile {
    /// `dir` に `vw-日時.log` を作り、古いものは新しい `keep` 個を残して消す。
    pub fn create(dir: &Path, keep: usize, now: DateTime<Local>) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("vw-{}.log", now.format("%Y%m%d-%H%M%S")));
        let file = File::options().create(true).append(true).open(&path)?;
        prune(dir, keep);
        Ok(Self {
            path,
            file: Arc::new(Mutex::new(file)),
        })
    }

    fn write_line(&self, line: &str) {
        // ファイルに書けなくても画面のログは続ける。書けないことを画面に出すと、その行も書こうとして繰り返すため黙る。
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }

    /// 落ちたときに、その知らせ（場所と理由）をファイルの最後に書く。
    pub fn install_panic_hook(&self) {
        let file = self.clone();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let at = Local::now().format("%Y-%m-%d %H:%M:%S%.3f %:z");
            let backtrace = std::backtrace::Backtrace::force_capture();
            file.write_line(&format!("{at} PANIC {info}\n{backtrace}"));
            previous(info);
        }));
    }
}

/// `vw-*.log` を名前（= 作った日時）の新しい順に `keep` 個残す。
fn prune(dir: &Path, keep: usize) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("vw-") && n.ends_with(".log"))
        })
        .collect();
    logs.sort();
    let excess = logs.len().saturating_sub(keep);
    for old in &logs[..excess] {
        let _ = std::fs::remove_file(old);
    }
}

/// ログファイルを置くフォルダ。設定ファイルの隣の `logs`。
pub fn log_dir(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name("logs")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn at(s: &str) -> DateTime<Local> {
        let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap();
        naive.and_local_timezone(Local).unwrap()
    }

    #[test]
    fn records_messages_in_order_with_level_and_elapsed_time() {
        let mut log = Log::new(Instant::now());
        log.info("a");
        log.warn("b");
        log.error("c");
        let levels: Vec<_> = log
            .lines
            .iter()
            .map(|l| (l.level, l.text.as_str()))
            .collect();
        assert_eq!(
            levels,
            [(Level::Info, "a"), (Level::Warn, "b"), (Level::Error, "c")]
        );
        assert!(log.lines.windows(2).all(|w| w[0].elapsed <= w[1].elapsed));
    }

    #[test]
    fn copy_all_uses_the_terminal_form_one_line_each() {
        let mut log = Log::new(Instant::now());
        log.info("first");
        log.error("second");
        let all = log.all_text();
        assert_eq!(all.lines().count(), 2);
        assert!(all.starts_with('[') && all.contains("] first\n["));
    }

    #[test]
    fn the_log_file_gets_every_line_with_date_level_and_elapsed_time() {
        let tmp = TempDir::new();
        let mut log = Log::new(Instant::now());
        log.info("before the file");
        log.write_to(LogFile::create(tmp.path(), 5, at("2026-10-16 14:22:19")).unwrap());
        log.error("after\nwith a second line");
        let path = log.file_path().unwrap().to_path_buf();
        assert_eq!(path.file_name().unwrap(), "vw-20261016-142219.log");
        let text = std::fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].contains(" INFO  [") && lines[0].ends_with("] before the file"));
        assert!(lines[1].contains(" ERROR [") && lines[1].ends_with("] after"));
        assert_eq!(lines[2], "with a second line");
        assert!(lines[0].starts_with("20") && lines[0].as_bytes()[4] == b'-');
    }

    #[test]
    fn only_the_newest_log_files_are_kept() {
        let tmp = TempDir::new();
        for (i, t) in [
            "2026-10-01 00:00:00",
            "2026-10-02 00:00:00",
            "2026-10-03 00:00:00",
        ]
        .iter()
        .enumerate()
        {
            LogFile::create(tmp.path(), 2, at(t)).unwrap();
            let count = std::fs::read_dir(tmp.path()).unwrap().count();
            assert_eq!(count, (i + 1).min(2));
        }
        tmp.touch("notes.txt");
        LogFile::create(tmp.path(), 2, at("2026-10-04 00:00:00")).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "notes.txt",
                "vw-20261003-000000.log",
                "vw-20261004-000000.log"
            ]
        );
    }
}
