//! 悪いフラグを付けた画像をまとめてごみ箱へ送る。
//!
//! 完全に消さずにごみ箱へ送るのは、フラグの付け間違いに後から気付いても戻せるようにするため。

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

/// ごみ箱へ送る前に確かめている画像。
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
}

impl Pending {
    /// 確認に並べるファイル名（先頭の `max` 個）と、並べきれなかった数。
    pub fn preview(&self, max: usize) -> (Vec<String>, usize) {
        let names = self
            .files
            .iter()
            .take(max)
            .map(|f| {
                f.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        (names, self.files.len().saturating_sub(max))
    }
}

/// 1 つの画像をごみ箱へ送った結果。
pub type Outcome = (PathBuf, Result<(), String>);

/// 別スレッドでごみ箱へ送る。Windows のごみ箱は 1 枚ごとに時間がかかり、
/// 何百枚もあると画面が止まるため。
pub struct TrashJob {
    done: Receiver<Vec<Outcome>>,
}

impl TrashJob {
    pub fn start(files: Vec<PathBuf>, repaint: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            // まとめて送る trash::delete_all は 1 つでも失敗すると、どれが送れたのか分からないので、
            // 1 つずつ送る。
            let outcomes = files
                .into_iter()
                .map(|f| {
                    let result = trash::delete(&f).map_err(|e| e.to_string());
                    (f, result)
                })
                .collect();
            let _ = tx.send(outcomes);
            repaint();
        });
        Self { done: rx }
    }

    /// 終わっていれば結果。
    pub fn poll(&self) -> Option<Vec<Outcome>> {
        self.done.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn pending(n: usize) -> Pending {
        Pending {
            dir: PathBuf::from("/photos"),
            files: (0..n)
                .map(|i| PathBuf::from(format!("/photos/{i}.png")))
                .collect(),
        }
    }

    #[test]
    fn preview_lists_every_name_when_few() {
        let (names, rest) = pending(2).preview(5);
        assert_eq!(names, ["0.png", "1.png"]);
        assert_eq!(rest, 0);
    }

    #[test]
    fn preview_counts_the_names_it_leaves_out() {
        let (names, rest) = pending(7).preview(5);
        assert_eq!(names.len(), 5);
        assert_eq!(rest, 2);
    }

    #[test]
    fn missing_file_is_reported_as_a_failure() {
        let missing = std::env::temp_dir().join("vw-test-no-such-file.png");
        let job = TrashJob::start(vec![missing.clone()], || {});
        let started = Instant::now();
        let outcomes = loop {
            if let Some(o) = job.poll() {
                break o;
            }
            assert!(started.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].0, missing);
        assert!(outcomes[0].1.is_err());
    }
}
