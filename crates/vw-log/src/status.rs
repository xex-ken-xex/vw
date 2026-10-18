//! ログ欄の下に出す、常に書き換わる状態の行。今動いている処理と、読み込みの集計を出す。
//!
//! 動いている処理は uv の画面のように、全体の数の行の下に 1 件 1 行で名前・棒・量を並べる。
//! 済んだものはこの行から消え、上のログに「済み」の行として積まれる（[`crate::log::Log::done`]）。
//!
//! 1 枚ごとの読み込みの記録はログファイルにだけ書き、ここには平均と数だけを出す。
//! ログ欄を 1 枚ごとの行で埋めると、何が起きたかの話題（フォルダを開いた、失敗した）が流れてしまうため。
//!
//! 文字だけで棒や波形を描くのは、ログと同じ等幅の文字の並びの中に収め、
//! そのまま選んでコピーしても読める形にするため。

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// 行の中の 1 区切りの役目。色は描く側が決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Label,
    Value,
    Accent,
    Dim,
    Bad,
    /// 進み具合の棒。
    Bar,
}

pub type Segment = (String, Tone);

/// 進めている処理 1 件（ZIP 1 つ、コピー 1 つなど）。
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    /// 何をしているか（訳した短い名前。「ZIP の展開」）。
    pub kind: String,
    /// 何に対してか（ファイルやフォルダの名前）。
    pub name: String,
    pub done: usize,
    /// 全部の量。まだ分からない（ZIP の目次を読んでいる、RAW を現像しているなど）ときは 0。
    pub total: usize,
    /// 量がバイト数か（そうでなければ件数）。
    pub bytes: bool,
    /// 全部の量が分からないときに、「…」の後ろに出す途中の数（「1,234 フォルダ」）。
    pub note: String,
}

/// 動き始めてから済んだ処理の数と、全部の数（「(164/179)」）。何も動かなくなったら数え直す。
///
/// 処理の側に数を持たせないのは、ZIP・コピー・送信などがそれぞれ別の仕組みで動いていて、
/// 見えている行の出入りから数えるのが一番どれにも手を入れずに済むため。
#[derive(Default)]
pub struct Tally {
    seen: HashSet<(String, String)>,
}

impl Tally {
    /// 今動いている処理を渡し、（済んだ数, 全部の数）を返す。
    pub fn update(&mut self, running: &[Task]) -> (usize, usize) {
        if running.is_empty() {
            self.seen.clear();
            return (0, 0);
        }
        let now: HashSet<(String, String)> = running
            .iter()
            .map(|t| (t.kind.clone(), t.name.clone()))
            .collect();
        let finished = self.seen.difference(&now).count();
        self.seen.extend(now);
        (finished, self.seen.len())
    }
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
    pub tasks: Vec<Task>,
    /// 動き始めてから済んだ処理の数と、全部の数（[`Tally`]）。
    pub tally: (usize, usize),
    /// 回るしるし。何か動いているときだけ回す。
    pub spinner: char,
    pub files: usize,
    pub failed: usize,
    /// 直近の読み込みの、読む・デコード・変換・テクスチャの平均（ミリ秒）。
    pub averages: Option<[f32; 4]>,
    /// 最後に読み込んだファイルの名前。
    pub last: Option<String>,
    pub last_failed: Option<String>,
}

/// 平均をとる、直近の読み込みの数。起動からの全部で平均すると、
/// 重い RAW を読んだ後でも小さな PNG の速さがいつまでも見えないため。
const AVERAGE_FILES: usize = 100;

/// 読み込みの集計。
#[derive(Default)]
pub struct LoadStats {
    pub files: usize,
    pub failed: usize,
    recent: VecDeque<[Duration; 4]>,
    pub last: Option<String>,
    pub last_failed: Option<String>,
}

impl LoadStats {
    /// 読み込めた 1 枚。`times` は読む・デコード・変換・テクスチャ。
    pub fn record(&mut self, name: String, times: [Duration; 4]) {
        self.files += 1;
        self.recent.push_back(times);
        if self.recent.len() > AVERAGE_FILES {
            self.recent.pop_front();
        }
        self.last = Some(name);
    }

    pub fn fail(&mut self, name: String) {
        self.failed += 1;
        self.last_failed = Some(name.clone());
        self.last = Some(name);
    }

    /// 直近の平均（ミリ秒）。
    pub fn averages(&self) -> Option<[f32; 4]> {
        if self.recent.is_empty() {
            return None;
        }
        let mut sum = [0.0f32; 4];
        for t in &self.recent {
            for (s, d) in sum.iter_mut().zip(t) {
                *s += d.as_secs_f32() * 1000.0;
            }
        }
        Some(sum.map(|s| s / self.recent.len() as f32))
    }
}

/// 状態の行の文言。言語で変わるもの。
pub struct Words<'a> {
    pub load: &'a str,
    pub waiting: &'a str,
    pub per_second: &'a str,
    pub full: &'a str,
    /// 処理がいくつかの種類にまたがるときの、全体の数の行の名前。
    pub working: &'a str,
    /// 行に出しきれない処理の数に添える語（「ほか 3 件」の「ほか」）。
    pub more: &'a str,
}

/// 1 件 1 行で出す処理の数の上限。コピーを何十も重ねたときに、ログが押し出されないように。
pub const TASK_LINES: usize = 8;
/// 処理の名前の欄の幅（文字）。長い名前は頭を「…」で縮める（ファイル名は末尾のほうが見分けやすい）。
const NAME_WIDTH: usize = 32;
/// 進み具合の棒の幅（文字）。
const BAR_WIDTH: usize = 30;

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

/// `width` 文字の棒。埋まった分を █、残りを細い線 ─ で。端数は 1/8 刻みの棒で表す。
///
/// 残りを ░ にしないのは、字体によって網点が粗い点線に見え、棒の地として目にうるさいため。
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
    s.push_str(&"─".repeat(width - used));
    s
}

/// 回るしるしの、起動からの秒に応じた 1 コマ。
pub fn spinner(seconds: f64) -> char {
    const FRAMES: [char; 4] = ['|', '/', '-', '\\'];
    FRAMES[(seconds * 8.0) as usize % FRAMES.len()]
}

/// 1.2 GB・820 MB・36.1 MB・512 KB のような短い形。
pub fn bytes(n: usize) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = n as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 100.0 {
        format!("{mb:.0} MB")
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", n as f64 / 1024.0)
    }
}

/// `text` を `width` 文字にそろえる。長ければ頭を「…」にして末尾を残す。
fn fit(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return format!("{text}{}", " ".repeat(width - count));
    }
    let tail: String = text.chars().skip(count - (width - 1)).collect();
    format!("…{tail}")
}

/// 状態の行。動いている処理があれば、全体の数の行と 1 件 1 行の進み具合を先に。
/// そのあとに読み込みの集計、最後に読んだファイル、フル解像度のメモリ。
pub fn lines(s: &Snapshot, w: &Words) -> Vec<Vec<Segment>> {
    let mut out = task_lines(s, w);
    out.extend(load_lines(s, w));
    out
}

fn task_lines(s: &Snapshot, w: &Words) -> Vec<Vec<Segment>> {
    let seg = |text: String, tone| (text, tone);
    let Some(first) = s.tasks.first() else {
        return Vec::new();
    };
    let title = if s.tasks.iter().all(|t| t.kind == first.kind) {
        first.kind.as_str()
    } else {
        w.working
    };
    let (finished, all) = s.tally;
    let mut out = vec![vec![
        seg(format!("{} ", s.spinner), Tone::Accent),
        seg(format!("{title}... "), Tone::Label),
        seg(format!("({finished}/{all})"), Tone::Value),
    ]];
    for task in s.tasks.iter().take(TASK_LINES) {
        let name = fit(&format!("{} {}", task.kind, task.name), NAME_WIDTH);
        let mut line = vec![seg(format!("{name} "), Tone::Value)];
        if task.total == 0 {
            line.push(seg("…".into(), Tone::Dim));
            if !task.note.is_empty() {
                line.push(seg(format!(" {}", task.note), Tone::Value));
            }
        } else {
            let amount = if task.bytes {
                format!("{}/{}", bytes(task.done), bytes(task.total))
            } else {
                format!("{}/{}", task.done, task.total)
            };
            line.push(seg(
                bar(task.done as f32 / task.total as f32, BAR_WIDTH),
                Tone::Bar,
            ));
            line.push(seg(format!(" {amount}"), Tone::Value));
        }
        out.push(line);
    }
    if s.tasks.len() > TASK_LINES {
        out.push(vec![seg(
            format!("{} {}", w.more, s.tasks.len() - TASK_LINES),
            Tone::Dim,
        )]);
    }
    out
}

fn load_lines(s: &Snapshot, w: &Words) -> Vec<Vec<Segment>> {
    let seg = |text: String, tone| (text, tone);
    let busy = s.waiting + s.running > 0;
    let mut load = vec![
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
        seg("   files ".into(), Tone::Label),
        seg(s.files.to_string(), Tone::Value),
    ];
    if let Some([read, decode, convert, texture]) = s.averages {
        load.push(seg("   avg ms  read ".into(), Tone::Label));
        load.push(seg(format!("{read:.1}"), Tone::Value));
        load.push(seg(" / decode ".into(), Tone::Label));
        load.push(seg(format!("{decode:.1}"), Tone::Value));
        load.push(seg(" / convert ".into(), Tone::Label));
        load.push(seg(format!("{convert:.1}"), Tone::Value));
        load.push(seg(" / texture ".into(), Tone::Label));
        load.push(seg(format!("{texture:.1}"), Tone::Value));
    }
    if s.failed > 0 {
        load.push(seg(format!("   {} failed", s.failed), Tone::Bad));
    }
    let mut last = vec![seg("  > ".into(), Tone::Dim)];
    match &s.last {
        Some(name) => last.push(seg(name.clone(), Tone::Value)),
        None => last.push(seg("-".into(), Tone::Dim)),
    }
    if let Some(name) = &s.last_failed {
        last.push(seg("   last failed ".into(), Tone::Label));
        last.push(seg(name.clone(), Tone::Bad));
    }
    let memory = vec![
        seg(format!("  {} ", w.full), Tone::Label),
        seg(
            bar(s.resident as f32 / s.limit.max(1) as f32, 12),
            Tone::Accent,
        ),
        seg(
            format!(" {} / {}", bytes(s.resident), bytes(s.limit)),
            Tone::Value,
        ),
    ];
    vec![load, last, memory]
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
        working: "Working",
        more: "more",
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
            tasks: Vec::new(),
            tally: (0, 0),
            spinner: '/',
            files: 0,
            failed: 0,
            averages: None,
            last: None,
            last_failed: None,
        }
    }

    #[test]
    fn bar_fills_from_the_left_in_eighths() {
        assert_eq!(bar(0.0, 4), "────");
        assert_eq!(bar(0.5, 4), "██──");
        assert_eq!(bar(1.0, 4), "████");
        assert_eq!(bar(1.0 / 32.0, 4), "▏───");
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
        let line = text(&lines(&s, &WORDS)[2]);
        assert!(line.ends_with("███───────── 1.0 GB / 4.0 GB"), "{line}");
    }

    fn task(kind: &str, name: &str, done: usize, total: usize, bytes: bool) -> Task {
        Task {
            kind: kind.into(),
            name: name.into(),
            done,
            total,
            bytes,
            note: String::new(),
        }
    }

    #[test]
    fn a_task_of_unknown_size_shows_how_far_it_has_got() {
        let s = Snapshot {
            tasks: vec![Task {
                note: "120 folders".into(),
                ..task("Find", "share", 0, 0, false)
            }],
            ..snapshot()
        };
        let line = text(&lines(&s, &WORDS)[1]);
        assert!(line.ends_with("… 120 folders"), "{line}");
    }

    #[test]
    fn nothing_running_shows_only_the_loading_lines() {
        let all = lines(&snapshot(), &WORDS);
        assert_eq!(all.len(), 3);
        assert!(text(&all[0]).starts_with("  load"));
    }

    #[test]
    fn each_running_task_gets_a_line_under_the_overall_count() {
        let s = Snapshot {
            tasks: vec![
                task("Send", "PACS", 35, 70, false),
                task("ZIP", "photos.zip", 1 << 30, 4 << 30, true),
                task("Copy", "a.png", 0, 0, true),
            ],
            tally: (2, 5),
            ..snapshot()
        };
        let all = lines(&s, &WORDS);
        assert_eq!(all.len(), 4 + 3);
        assert_eq!(text(&all[0]), "/ Working... (2/5)");
        assert_eq!(
            text(&all[1]),
            format!(
                "{:<32} {}{} 35/70",
                "Send PACS",
                "█".repeat(15),
                "─".repeat(15)
            )
        );
        assert!(
            text(&all[2]).ends_with(" 1.0 GB/4.0 GB"),
            "{}",
            text(&all[2])
        );
        assert_eq!(text(&all[3]), format!("{:<32} …", "Copy a.png"));
        assert!(text(&all[4]).starts_with("/ load") || text(&all[4]).starts_with("  load"));
    }

    #[test]
    fn tasks_of_one_kind_are_counted_under_that_kind() {
        let s = Snapshot {
            tasks: vec![
                task("Copy", "a.png", 1, 2, true),
                task("Copy", "b.png", 1, 2, true),
            ],
            tally: (0, 2),
            ..snapshot()
        };
        assert_eq!(text(&lines(&s, &WORDS)[0]), "/ Copy... (0/2)");
    }

    #[test]
    fn long_names_keep_their_end() {
        let name = format!("{}.zip", "x".repeat(40));
        let s = Snapshot {
            tasks: vec![task("ZIP", &name, 1, 2, false)],
            ..snapshot()
        };
        let line = text(&lines(&s, &WORDS)[1]);
        assert!(line.starts_with('…'), "{line}");
        assert!(line.contains("xxx.zip "), "{line}");
    }

    #[test]
    fn tasks_beyond_the_limit_are_only_counted() {
        let tasks: Vec<Task> = (0..TASK_LINES + 3)
            .map(|i| task("Copy", &format!("{i}.png"), 1, 2, false))
            .collect();
        let s = Snapshot {
            tasks,
            ..snapshot()
        };
        let all = lines(&s, &WORDS);
        assert_eq!(text(&all[TASK_LINES + 1]), "more 3");
    }

    #[test]
    fn the_tally_counts_tasks_that_have_left_until_everything_stops() {
        let mut tally = Tally::default();
        let a = task("Copy", "a.png", 0, 1, true);
        let b = task("Copy", "b.png", 0, 1, true);
        assert_eq!(tally.update(&[a.clone(), b.clone()]), (0, 2));
        assert_eq!(tally.update(std::slice::from_ref(&b)), (1, 2));
        let c = task("ZIP", "c.zip", 0, 0, true);
        assert_eq!(tally.update(&[b, c.clone()]), (1, 3));
        assert_eq!(tally.update(&[c]), (2, 3));
        assert_eq!(tally.update(&[]), (0, 0));
        assert_eq!(tally.update(&[a]), (0, 1));
    }

    #[test]
    fn sizes_are_short_with_the_unit_that_fits() {
        assert_eq!(bytes(512 * 1024), "512 KB");
        assert_eq!(bytes(36 * 1024 * 1024 + 150 * 1024), "36.1 MB");
        assert_eq!(bytes(674 * 1024 * 1024), "674 MB");
        assert_eq!(bytes(3 << 29), "1.5 GB");
    }

    #[test]
    fn loading_shows_averages_failures_and_the_last_file() {
        let mut stats = LoadStats::default();
        let ms = Duration::from_millis;
        stats.record("a.png".into(), [ms(1), ms(10), ms(4), ms(0)]);
        stats.record("b.png".into(), [ms(3), ms(20), ms(6), ms(2)]);
        stats.fail("c.jpg".into());
        let s = Snapshot {
            files: stats.files,
            failed: stats.failed,
            averages: stats.averages(),
            last: stats.last.clone(),
            last_failed: stats.last_failed.clone(),
            ..snapshot()
        };
        let all = lines(&s, &WORDS);
        let load = text(&all[0]);
        assert!(
            load.ends_with(
                "files 2   avg ms  read 2.0 / decode 15.0 / convert 5.0 / texture 1.0   1 failed"
            ),
            "{load}"
        );
        assert_eq!(text(&all[1]), "  > c.jpg   last failed c.jpg");
    }

    #[test]
    fn averages_cover_only_the_recent_files() {
        let mut stats = LoadStats::default();
        stats.record("slow".into(), [Duration::from_secs(1); 4]);
        for _ in 0..AVERAGE_FILES {
            stats.record("fast".into(), [Duration::ZERO; 4]);
        }
        assert_eq!(stats.averages(), Some([0.0; 4]));
        assert_eq!(stats.files, AVERAGE_FILES + 1);
    }
}
