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

/// OS のごみ箱から元の場所へ戻せるか。macOS のごみ箱は、送った物の一覧を読む口が
/// trash の部品にないので戻せない。
pub const CAN_RESTORE: bool = cfg!(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
));

/// ごみ箱から元の場所へ戻す。ごみ箱の一覧を読むのは Windows では数秒かかることがあるので、
/// 別スレッドで行う。
pub struct RestoreJob {
    done: Receiver<Vec<Outcome>>,
}

impl RestoreJob {
    pub fn start(files: Vec<PathBuf>, repaint: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(restore(files));
            repaint();
        });
        Self { done: rx }
    }

    /// 終わっていれば結果。
    pub fn poll(&self) -> Option<Vec<Outcome>> {
        self.done.try_recv().ok()
    }
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn restore(files: Vec<PathBuf>) -> Vec<Outcome> {
    use trash::os_limited;
    let items = match os_limited::list() {
        Ok(items) => items,
        Err(e) => {
            let e = e.to_string();
            return files.into_iter().map(|f| (f, Err(e.clone()))).collect();
        }
    };
    files
        .into_iter()
        .map(|f| {
            // 同じ場所の同じ名前を何度も送っていることがあるので、最後に送ったものを戻す。
            let item = items
                .iter()
                .filter(|item| item.original_path() == f)
                .max_by_key(|item| item.time_deleted)
                .cloned();
            let result = match item {
                None => Err("not found in the trash".to_owned()),
                Some(_) if f.exists() => {
                    Err("a file with the same name is already there".to_owned())
                }
                Some(item) => os_limited::restore_all([item]).map_err(|e| e.to_string()),
            };
            (f, result)
        })
        .collect()
}

#[cfg(not(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
)))]
fn restore(files: Vec<PathBuf>) -> Vec<Outcome> {
    files
        .into_iter()
        .map(|f| {
            (
                f,
                Err("restoring from the trash is not supported on this OS".to_owned()),
            )
        })
        .collect()
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

    fn wait<T>(poll: impl Fn() -> Option<T>) -> T {
        let started = Instant::now();
        loop {
            if let Some(o) = poll() {
                break o;
            }
            assert!(started.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn restore_puts_a_trashed_file_back_in_place() {
        // ごみ箱はホームの下にあり、別の装置の一時フォルダからは送れないことがあるので、ホームの下で試す。
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        let dir = home.join(format!(".vw-test-restore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bad.png");
        std::fs::write(&file, b"png").unwrap();
        let job = TrashJob::start(vec![file.clone()], || {});
        let trashed = wait(|| job.poll());
        assert!(trashed[0].1.is_ok(), "{:?}", trashed[0].1);
        assert!(!file.exists());

        let job = RestoreJob::start(vec![file.clone()], || {});
        let restored = wait(|| job.poll());
        assert!(restored[0].1.is_ok(), "{:?}", restored[0].1);
        assert_eq!(std::fs::read(&file).unwrap(), b"png");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restore_reports_a_file_that_is_not_in_the_trash() {
        let missing = std::env::temp_dir().join("vw-test-never-trashed.png");
        let job = RestoreJob::start(vec![missing.clone()], || {});
        let outcomes = wait(|| job.poll());
        assert_eq!(outcomes[0].0, missing);
        assert!(outcomes[0].1.is_err());
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
