//! ターミナル風のログ欄の上に出す、常に書き換わる状態の行。
//!
//! 文字だけで棒や波形を描くのは、ログと同じ等幅の文字の並びの中に収め、
//! そのまま選んでコピーしても読める形にするため。

use std::collections::VecDeque;
use std::time::Instant;

/// 行の中の 1 区切りの役目。色は描く側が決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Label,
    Value,
    Accent,
    Dim,
}

pub type Segment = (String, Tone);

/// 進めている処理 1 つ。
pub struct Job {
    pub name: &'static str,
    pub done: usize,
    pub total: usize,
}

/// 状態の行を作るための、その時点の値。
pub struct Snapshot {
    /// 読み込みを待っている数。
    pub waiting: usize,
    /// 読み込んでいる最中の数。
    pub running: usize,
    pub workers: usize,
    /// 直近の 1 秒ごとの読み込み枚数の波形と、1 秒あたりの枚数。
    pub sparkline: String,
    pub rate: f32,
    /// 持っているフル解像度の画像のバイト数と、その上限。
    pub resident: usize,
    pub limit: usize,
    pub jobs: Vec<Job>,
    /// 回るしるし。何か動いているときだけ回す。
    pub spinner: char,
}

/// 状態の行の文言。言語で変わるもの。
pub struct Words<'a> {
    pub load: &'a str,
    pub waiting: &'a str,
    pub per_second: &'a str,
    pub full: &'a str,
    pub idle: &'a str,
}

/// 1 秒ごとの完了数を数え、直近の流れを波形にする。
pub struct Throughput {
    origin: Instant,
    /// (起動からの秒, その秒に終わった数)。古い順。
    counts: VecDeque<(u64, u32)>,
}

/// 波形に出す秒数。
pub const SPARK_SECONDS: usize = 24;
/// 1 秒あたりの枚数を平均する秒数。直前の秒は数え途中なので含めない。
const RATE_SECONDS: u64 = 3;

impl Throughput {
    pub fn new(origin: Instant) -> Self {
        Self {
            origin,
            counts: VecDeque::new(),
        }
    }

    pub fn record(&mut self, now: Instant) {
        let second = self.second(now);
        match self.counts.back_mut() {
            Some((s, n)) if *s == second => *n += 1,
            _ => self.counts.push_back((second, 1)),
        }
        while self
            .counts
            .front()
            .is_some_and(|(s, _)| *s + (SPARK_SECONDS as u64) < second)
        {
            self.counts.pop_front();
        }
    }

    fn second(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.origin).as_secs()
    }

    fn count_at(&self, second: u64) -> u32 {
        self.counts
            .iter()
            .find(|(s, _)| *s == second)
            .map_or(0, |(_, n)| *n)
    }

    /// 直近 [`SPARK_SECONDS`] 秒の 1 秒ごとの数を、古い方を左に 8 段の棒で。
    /// 高さは窓の中の一番多い秒に合わせる。
    pub fn sparkline(&self, now: Instant) -> String {
        const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let last = self.second(now);
        let counts: Vec<u32> = (0..SPARK_SECONDS as u64)
            .rev()
            .map(|back| last.checked_sub(back).map_or(0, |s| self.count_at(s)))
            .collect();
        let max = counts.iter().copied().max().unwrap_or(0);
        counts
            .iter()
            .map(|&n| {
                if n == 0 || max == 0 {
                    ' '
                } else {
                    LEVELS[((n as usize * LEVELS.len()).div_ceil(max as usize) - 1)
                        .min(LEVELS.len() - 1)]
                }
            })
            .collect()
    }

    /// 直前の数秒の 1 秒あたりの枚数。
    pub fn rate(&self, now: Instant) -> f32 {
        let last = self.second(now);
        let total: u32 = (1..=RATE_SECONDS)
            .filter_map(|back| last.checked_sub(back))
            .map(|s| self.count_at(s))
            .sum();
        total as f32 / RATE_SECONDS as f32
    }
}

/// `width` 文字の棒。埋まった分を █、残りを ░ で。端数は 1/8 刻みの棒で表す。
pub fn bar(fraction: f32, width: usize) -> String {
    const PARTS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let eighths = (fraction.clamp(0.0, 1.0) * (width * 8) as f32).round() as usize;
    let full = eighths / 8;
    let part = eighths % 8;
    let mut s = "█".repeat(full);
    if part > 0 {
        s.push(PARTS[part - 1]);
    }
    let used = full + usize::from(part > 0);
    s.push_str(&"░".repeat(width - used));
    s
}

/// 回るしるしの、起動からの秒に応じた 1 コマ。
pub fn spinner(seconds: f64) -> char {
    const FRAMES: [char; 4] = ['|', '/', '-', '\\'];
    FRAMES[(seconds * 8.0) as usize % FRAMES.len()]
}

/// 1.2 GB・820 MB のような短い形。
pub fn bytes(n: usize) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = n as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

/// 2 行。1 行目は読み込みとフル解像度のメモリ、2 行目は進めている処理。
pub fn lines(s: &Snapshot, w: &Words) -> [Vec<Segment>; 2] {
    let seg = |text: String, tone| (text, tone);
    let busy = s.waiting + s.running > 0;
    let first = vec![
        seg(
            format!("{} ", if busy { s.spinner } else { ' ' }),
            Tone::Accent,
        ),
        seg(format!("{} ", w.load), Tone::Label),
        seg(format!("{}/{}", s.running, s.workers), Tone::Value),
        seg(format!("  {} ", w.waiting), Tone::Label),
        seg(format!("{:<4}", s.waiting), Tone::Value),
        seg(format!(" {} ", s.sparkline), Tone::Accent),
        seg(format!("{:5.1}", s.rate), Tone::Value),
        seg(format!(" {}", w.per_second), Tone::Label),
        seg(format!("   {} ", w.full), Tone::Label),
        seg(
            bar(s.resident as f32 / s.limit.max(1) as f32, 12),
            Tone::Accent,
        ),
        seg(
            format!(" {} / {}", bytes(s.resident), bytes(s.limit)),
            Tone::Value,
        ),
    ];
    let mut second = Vec::new();
    if s.jobs.is_empty() {
        second.push(seg(format!("  {}", w.idle), Tone::Dim));
    }
    for (i, job) in s.jobs.iter().enumerate() {
        let fraction = job.done as f32 / job.total.max(1) as f32;
        second.push(seg(
            format!("{}{} ", if i == 0 { "  " } else { "   " }, job.name),
            Tone::Label,
        ));
        second.push(seg(bar(fraction, 16), Tone::Accent));
        second.push(seg(
            format!(" {:3.0}% {}/{}", fraction * 100.0, job.done, job.total),
            Tone::Value,
        ));
    }
    [first, second]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn text(line: &[Segment]) -> String {
        line.iter().map(|(t, _)| t.as_str()).collect()
    }

    const WORDS: Words = Words {
        load: "load",
        waiting: "waiting",
        per_second: "/s",
        full: "full",
        idle: "idle",
    };

    fn snapshot() -> Snapshot {
        Snapshot {
            waiting: 0,
            running: 0,
            workers: 8,
            sparkline: " ".repeat(SPARK_SECONDS),
            rate: 0.0,
            resident: 0,
            limit: 4 << 30,
            jobs: Vec::new(),
            spinner: '/',
        }
    }

    #[test]
    fn bar_fills_from_the_left_in_eighths() {
        assert_eq!(bar(0.0, 4), "░░░░");
        assert_eq!(bar(0.5, 4), "██░░");
        assert_eq!(bar(1.0, 4), "████");
        assert_eq!(bar(1.0 / 32.0, 4), "▏░░░");
        assert_eq!(bar(2.0, 4), "████");
        assert_eq!(bar(0.3, 10).chars().count(), 10);
    }

    #[test]
    fn sparkline_shows_each_second_relative_to_the_busiest() {
        let origin = Instant::now();
        let mut t = Throughput::new(origin);
        let at = |s: u64| origin + Duration::from_millis(s * 1000 + 500);
        for _ in 0..8 {
            t.record(at(10));
        }
        for _ in 0..4 {
            t.record(at(11));
        }
        let line: Vec<char> = t.sparkline(at(12)).chars().collect();
        assert_eq!(line.len(), SPARK_SECONDS);
        assert_eq!(line[SPARK_SECONDS - 3], '█');
        assert_eq!(line[SPARK_SECONDS - 2], '▄');
        assert_eq!(line[SPARK_SECONDS - 1], ' ');
        assert!(line[..SPARK_SECONDS - 3].iter().all(|&c| c == ' '));
    }

    #[test]
    fn rate_averages_the_last_finished_seconds() {
        let origin = Instant::now();
        let mut t = Throughput::new(origin);
        let at = |s: u64| origin + Duration::from_millis(s * 1000 + 500);
        for s in [1, 2, 3] {
            for _ in 0..6 {
                t.record(at(s));
            }
        }
        t.record(at(4));
        assert_eq!(t.rate(at(4)), 6.0);
    }

    #[test]
    fn old_seconds_drop_out_of_the_window() {
        let origin = Instant::now();
        let mut t = Throughput::new(origin);
        t.record(origin);
        let later = origin + Duration::from_secs(SPARK_SECONDS as u64 + 5);
        t.record(later);
        assert_eq!(t.counts.len(), 1);
    }

    #[test]
    fn the_spinner_turns_only_while_loading() {
        let idle = lines(&snapshot(), &WORDS);
        assert!(text(&idle[0]).starts_with("  load 0/8"));
        let busy = Snapshot {
            waiting: 3,
            running: 2,
            ..snapshot()
        };
        let line = text(&lines(&busy, &WORDS)[0]);
        assert!(line.starts_with("/ load 2/8  waiting 3"), "{line}");
    }

    #[test]
    fn full_resolution_memory_is_shown_against_its_limit() {
        let s = Snapshot {
            resident: 1 << 30,
            ..snapshot()
        };
        let line = text(&lines(&s, &WORDS)[0]);
        assert!(line.ends_with("███░░░░░░░░░ 1.0 GB / 4.0 GB"), "{line}");
    }

    #[test]
    fn second_line_lists_each_running_job_with_its_progress() {
        assert_eq!(text(&lines(&snapshot(), &WORDS)[1]), "  idle");
        let s = Snapshot {
            jobs: vec![
                Job {
                    name: "ZIP",
                    done: 35,
                    total: 70,
                },
                Job {
                    name: "Send",
                    done: 0,
                    total: 0,
                },
            ],
            ..snapshot()
        };
        let line = text(&lines(&s, &WORDS)[1]);
        assert_eq!(
            line,
            "  ZIP ████████░░░░░░░░  50% 35/70   Send ░░░░░░░░░░░░░░░░   0% 0/0"
        );
    }
}
