//! スライドショー（パラパラ表示）の時間の進め方。
//!
//! 再生開始からの経過時間と FPS からコマ番号（tick）を決め、そのコマで出す画像の位置を
//! 終端の扱い（[`EndMode`]）に従って求める。画像の読み込みと表示は [`crate::app`] が行う。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::TextureHandle;
use serde::{Deserialize, Serialize};

use crate::loader::ImageInfo;

/// 最後の画像まで進んだときの動き。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndMode {
    /// 先頭に戻って繰り返す。
    #[default]
    Loop,
    /// 最後の画像で止まる。
    Stop,
    /// 折り返して逆向きに進み、先頭でまた折り返す。
    PingPong,
}

impl EndMode {
    pub const ALL: [EndMode; 3] = [EndMode::Loop, EndMode::Stop, EndMode::PingPong];

    pub fn title(self) -> &'static str {
        match self {
            EndMode::Loop => "Loop",
            EndMode::Stop => "Stop at end",
            EndMode::PingPong => "Ping-pong",
        }
    }
}

pub const FPS_RANGE: RangeInclusive<f32> = 0.1..=120.0;
/// 再生バーに並べる FPS。
pub const FPS_PRESETS: [f32; 6] = [1.0, 5.0, 10.0, 15.0, 30.0, 60.0];

/// 開始位置 `start` から `tick` コマ進んだときの位置と、`Stop` で最後に着いたか。
pub fn frame_at(start: usize, tick: u64, n: usize, mode: EndMode) -> (usize, bool) {
    if n <= 1 {
        return (0, mode == EndMode::Stop);
    }
    let pos = start as u64 + tick;
    let n64 = n as u64;
    match mode {
        EndMode::Loop => ((pos % n64) as usize, false),
        EndMode::Stop => {
            let last = n64 - 1;
            (pos.min(last) as usize, pos >= last)
        }
        EndMode::PingPong => {
            let period = 2 * (n64 - 1);
            let p = pos % period;
            let index = if p < n64 { p } else { period - p };
            (index as usize, false)
        }
    }
}

/// `tick` の次から `ahead` コマ分で出す位置。重複は除き、出す順に並べる。
/// 先読みする画像を決めるのに使う。
pub fn upcoming(start: usize, tick: u64, n: usize, mode: EndMode, ahead: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(ahead);
    for t in tick + 1..=tick + ahead as u64 {
        let (index, finished) = frame_at(start, t, n, mode);
        if !out.contains(&index) {
            out.push(index);
        }
        if finished {
            break;
        }
    }
    out
}

/// 再生中の時計。コマが進んだときに、間に合わずに飛ばしたコマを数える。
#[derive(Debug, Clone)]
pub struct Clock {
    pub start: usize,
    pub n: usize,
    pub mode: EndMode,
    pub fps: f32,
    origin: Instant,
    tick: u64,
    /// 今のコマの画像を出せたか。
    shown: bool,
    /// 間に合わずに出せなかったコマの数。
    pub dropped: u64,
}

/// 時計を進めた結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// 今出すべき位置。
    pub index: usize,
    /// 前回からコマが進んだ。
    pub advanced: bool,
    /// `Stop` で最後のコマに着いた。
    pub finished: bool,
}

impl Clock {
    pub fn new(start: usize, n: usize, mode: EndMode, fps: f32, now: Instant) -> Self {
        Self {
            start,
            n,
            mode,
            fps,
            origin: now,
            tick: 0,
            // 開始位置の画像は表示中なので、出せたことにする。
            shown: true,
            dropped: 0,
        }
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// `now` の時点まで進める。
    pub fn advance(&mut self, now: Instant) -> Step {
        let tick = ticks(now.saturating_duration_since(self.origin), self.fps);
        let advanced = tick > self.tick;
        if advanced {
            // 1 フレームの間に複数コマ進んだ分（FPS が画面の更新より速い）と、
            // 前のコマを出せないまま次に進んだ分は、どちらも見せられなかったコマ。
            self.dropped += tick - self.tick - 1 + u64::from(!self.shown);
            self.tick = tick;
            self.shown = false;
        }
        let (index, finished) = frame_at(self.start, self.tick, self.n, self.mode);
        Step {
            index,
            advanced,
            finished,
        }
    }

    /// 今のコマの画像を出したことを記録する。
    pub fn mark_shown(&mut self) {
        self.shown = true;
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// 次のコマまでの時間。再描画を予約するのに使う。
    pub fn until_next(&self, now: Instant) -> Duration {
        let next = Duration::from_secs_f64((self.tick + 1) as f64 / f64::from(self.fps));
        (self.origin + next).saturating_duration_since(now)
    }

    /// FPS や終端の扱いを変えたときに、今の位置から数え直す。
    /// 経過時間を新しい FPS で掛け直すと、位置が飛んでしまうため。
    pub fn restart(&mut self, fps: f32, mode: EndMode, now: Instant) {
        let (index, _) = frame_at(self.start, self.tick, self.n, self.mode);
        let dropped = self.dropped;
        *self = Clock::new(index, self.n, mode, fps, now);
        self.dropped = dropped;
    }
}

/// 先読みした 1 コマ。
#[derive(Clone)]
pub struct Frame {
    pub path: PathBuf,
    pub texture: TextureHandle,
    pub info: ImageInfo,
}

/// 再生中のスライドショー。
pub struct Playback {
    /// 再生しているスロットの番号。
    pub slot: u64,
    /// 先読みの読み込み要求に付けるスロット番号。表示中のスロットの番号を使わないのは、
    /// 先読みの結果を表示中の画像の読み込み結果と取り違えないため。
    pub loader_slot: u64,
    pub frames: Vec<PathBuf>,
    pub clock: Clock,
    /// 読み込み済みの画像（位置ごと）。表示した後も、この先また出すうちは持っておく。
    /// 短い並びをループすると同じ画像をすぐまた出すので、出すたびに読み直さないため。
    pub cache: BTreeMap<usize, Frame>,
    /// 読み込み中の要求番号と、その位置。
    pub pending: HashMap<u64, usize>,
    /// 読み込めなかった位置。読み直しを繰り返さないよう、そのコマは飛ばす。
    pub failed: HashSet<usize>,
    /// 1 コマのテクスチャのおおよそのバイト数。先読みの枚数をメモリの上限に収めるのに使う。
    pub frame_bytes: usize,
}

impl Playback {
    pub fn new(slot: u64, loader_slot: u64, frames: Vec<PathBuf>, clock: Clock) -> Self {
        Self {
            slot,
            loader_slot,
            frames,
            clock,
            cache: BTreeMap::new(),
            pending: HashMap::new(),
            failed: HashSet::new(),
            frame_bytes: 0,
        }
    }

    /// 先読みするコマ数。0.5 秒分を目安に、メモリの上限 `budget` に収まる数にする。
    /// 少なすぎるとデコードの揺らぎでコマが落ち、多すぎると再生の開始とシークが遅れる。
    pub fn ahead(&self, budget: usize) -> usize {
        let by_time = (self.clock.fps * 0.5).ceil() as usize;
        let by_memory = budget.checked_div(self.frame_bytes).unwrap_or(usize::MAX);
        by_time.clamp(2, 24).min(by_memory.max(1))
    }
}

fn ticks(elapsed: Duration, fps: f32) -> u64 {
    (elapsed.as_secs_f64() * f64::from(fps)).floor() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_wraps_to_the_first_image() {
        assert_eq!(frame_at(3, 1, 5, EndMode::Loop), (4, false));
        assert_eq!(frame_at(3, 2, 5, EndMode::Loop), (0, false));
        assert_eq!(frame_at(0, 12, 5, EndMode::Loop), (2, false));
    }

    #[test]
    fn stop_stays_on_the_last_image_and_reports_finished() {
        assert_eq!(frame_at(2, 1, 5, EndMode::Stop), (3, false));
        assert_eq!(frame_at(2, 2, 5, EndMode::Stop), (4, true));
        assert_eq!(frame_at(2, 9, 5, EndMode::Stop), (4, true));
    }

    #[test]
    fn ping_pong_turns_back_at_both_ends() {
        let seq: Vec<usize> = (0..10)
            .map(|t| frame_at(0, t, 4, EndMode::PingPong).0)
            .collect();
        assert_eq!(seq, [0, 1, 2, 3, 2, 1, 0, 1, 2, 3]);
    }

    #[test]
    fn single_image_never_moves() {
        for mode in EndMode::ALL {
            assert_eq!(frame_at(0, 7, 1, mode).0, 0);
        }
    }

    #[test]
    fn upcoming_lists_the_next_images_in_play_order() {
        assert_eq!(upcoming(0, 0, 10, EndMode::Loop, 3), [1, 2, 3]);
        assert_eq!(upcoming(0, 8, 10, EndMode::Loop, 3), [9, 0, 1]);
    }

    #[test]
    fn upcoming_stops_at_the_end_when_not_looping() {
        assert_eq!(upcoming(0, 7, 10, EndMode::Stop, 5), [8, 9]);
    }

    #[test]
    fn upcoming_has_no_duplicates_when_ping_ponging_a_short_series() {
        assert_eq!(upcoming(0, 0, 3, EndMode::PingPong, 6), [1, 2, 0]);
    }

    #[test]
    fn clock_advances_one_image_per_frame_period() {
        let t0 = Instant::now();
        let mut c = Clock::new(0, 10, EndMode::Loop, 10.0, t0);
        let s = c.advance(t0 + Duration::from_millis(50));
        assert_eq!((s.index, s.advanced), (0, false));
        let s = c.advance(t0 + Duration::from_millis(100));
        assert_eq!((s.index, s.advanced), (1, true));
    }

    #[test]
    fn frames_not_shown_in_time_are_counted_as_dropped() {
        let t0 = Instant::now();
        let mut c = Clock::new(0, 10, EndMode::Loop, 10.0, t0);
        c.advance(t0 + Duration::from_millis(100));
        // コマ 1 を出せないままコマ 2 へ。
        c.advance(t0 + Duration::from_millis(200));
        assert_eq!(c.dropped, 1);
        c.mark_shown();
        // 1 回の更新で 3 コマ進むと、間の 2 コマは見せられない。
        c.advance(t0 + Duration::from_millis(500));
        assert_eq!(c.dropped, 3);
    }

    #[test]
    fn image_shown_on_time_is_not_dropped() {
        let t0 = Instant::now();
        let mut c = Clock::new(0, 10, EndMode::Loop, 10.0, t0);
        for i in 1..=5 {
            c.advance(t0 + Duration::from_millis(100 * i));
            c.mark_shown();
        }
        assert_eq!(c.dropped, 0);
    }

    #[test]
    fn restart_keeps_the_current_image() {
        let t0 = Instant::now();
        let mut c = Clock::new(2, 10, EndMode::Loop, 10.0, t0);
        c.advance(t0 + Duration::from_millis(300));
        let t1 = t0 + Duration::from_millis(310);
        c.restart(60.0, EndMode::PingPong, t1);
        let s = c.advance(t1);
        assert_eq!((s.index, c.fps, c.mode), (5, 60.0, EndMode::PingPong));
    }

    fn playback(fps: f32, frame_bytes: usize) -> Playback {
        let clock = Clock::new(0, 100, EndMode::Loop, fps, Instant::now());
        let mut pb = Playback::new(1, 2, Vec::new(), clock);
        pb.frame_bytes = frame_bytes;
        pb
    }

    #[test]
    fn prefetch_covers_half_a_second() {
        assert_eq!(playback(30.0, 0).ahead(usize::MAX), 15);
    }

    #[test]
    fn prefetch_keeps_a_minimum_and_a_maximum() {
        assert_eq!(playback(1.0, 0).ahead(usize::MAX), 2);
        assert_eq!(playback(120.0, 0).ahead(usize::MAX), 24);
    }

    #[test]
    fn prefetch_fits_in_the_memory_budget() {
        assert_eq!(playback(60.0, 100).ahead(500), 5);
        // 1 コマも収まらなくても、次のコマだけは読む。
        assert_eq!(playback(60.0, 1000).ahead(500), 1);
    }

    #[test]
    fn next_frame_time_counts_down() {
        let t0 = Instant::now();
        let c = Clock::new(0, 10, EndMode::Loop, 4.0, t0);
        assert_eq!(c.until_next(t0), Duration::from_millis(250));
        assert_eq!(
            c.until_next(t0 + Duration::from_millis(100)),
            Duration::from_millis(150)
        );
    }
}
