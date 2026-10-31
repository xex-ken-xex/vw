//! スロットからフォルダへドラッグした画像の移動・コピー。ラベルも一緒に移す。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use crate::labels::{Label, LabelStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Move,
    Copy,
}

/// 画像 `file` をフォルダ `to_dir` へ移す・写す要求。
#[derive(Debug, Clone)]
pub struct Transfer {
    pub file: PathBuf,
    pub to_dir: PathBuf,
    pub op: Op,
}

impl Transfer {
    /// 移した後のパス。名前は変えない。
    pub fn destination(&self) -> PathBuf {
        self.to_dir.join(self.file.file_name().unwrap_or_default())
    }

    /// 元と同じフォルダへの移動・コピー。何もしない。
    pub fn is_noop(&self) -> bool {
        self.file.parent() == Some(self.to_dir.as_path())
    }

    /// ファイルを移し、移した先のパスを返す。写したバイト数を `done` に足していく。
    ///
    /// 同じ名前のファイルが先にあれば何もしない。上書きの確認を挟むより、移す側の名前を
    /// 変えてもらうほうが、画像を取り違えて消す心配がないため。
    pub fn carry_file(&self, done: &AtomicUsize) -> Result<PathBuf, String> {
        let dest = self.destination();
        if dest.exists() {
            return Err(format!("{}: already exists", dest.display()));
        }
        match self.op {
            Op::Copy => copy_file(&self.file, &dest, done)?,
            Op::Move => move_file(&self.file, &dest, done)?,
        }
        Ok(dest)
    }

    /// 元に付いていたラベルを移した先 `dest` に付ける。移動なら元のラベルは消す。
    /// 付けられなかったときはその理由を返す（ファイルは移したままにする）。
    pub fn carry_labels(&self, labels: &LabelStore, dest: &Path) -> Option<String> {
        let label = labels.get(&self.file);
        if label.is_empty() {
            return None;
        }
        if let Err(e) = labels.set(dest, label) {
            return Some(e);
        }
        if self.op == Op::Move {
            return labels.set(&self.file, Label::default()).err();
        }
        None
    }
}

/// 裏のスレッドで進める移動・コピー 1 つ。ラベルは画面のスレッドで持っているので、
/// ファイルを移し終えてから [`Transfer::carry_labels`] で移す。
pub struct Job {
    pub transfer: Transfer,
    done: Arc<AtomicUsize>,
    total: usize,
    result: Receiver<Result<PathBuf, String>>,
}

impl Job {
    /// 移し始める。終わったら `wake` を呼ぶ。
    pub fn start(transfer: Transfer, wake: impl Fn() + Send + 'static) -> Self {
        let total = std::fs::metadata(&transfer.file).map_or(0, |m| m.len() as usize);
        let done = Arc::new(AtomicUsize::new(0));
        let (tx, result) = channel();
        let (t, count) = (transfer.clone(), done.clone());
        std::thread::spawn(move || {
            let _ = tx.send(t.carry_file(&count));
            wake();
        });
        Self {
            transfer,
            done,
            total,
            result,
        }
    }

    /// （写したバイト数, ファイルの大きさ）。
    pub fn progress(&self) -> (usize, usize) {
        (self.done.load(Ordering::Relaxed), self.total)
    }

    /// 終わっていれば移した先のパスか失敗の理由。
    pub fn poll(&self) -> Option<Result<PathBuf, String>> {
        match self.result.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err(format!("{}: stopped", self.transfer.file.display())))
            }
        }
    }
}

/// `std::fs::copy` を使わないのは、写している途中の進み具合が分からず、数 GB のファイルで
/// 止まって見えるため。更新日時は写す（ツリーやスロットを更新日時で並べているので、
/// 写した側が一番新しいものとして先頭に来てしまわないように）。
fn copy_file(from: &Path, to: &Path, done: &AtomicUsize) -> Result<(), String> {
    let source_error = |e: std::io::Error| format!("{}: {e}", from.display());
    let target_error = |e: std::io::Error| format!("{}: {e}", to.display());
    let mut source = std::fs::File::open(from).map_err(source_error)?;
    let modified = source.metadata().and_then(|m| m.modified()).ok();
    // create_new で、確かめてから作るまでの間に同じ名前のファイルができても上書きしない。
    let mut target = std::fs::File::create_new(to).map_err(target_error)?;
    let copied = (|| {
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = source.read(&mut buf).map_err(source_error)?;
            if n == 0 {
                break;
            }
            target.write_all(&buf[..n]).map_err(target_error)?;
            done.fetch_add(n, Ordering::Relaxed);
        }
        if let Some(time) = modified {
            target.set_modified(time).map_err(target_error)?;
        }
        Ok(())
    })();
    if copied.is_err() {
        drop(target);
        let _ = std::fs::remove_file(to);
    }
    copied
}

/// 名前の付け替えで移す。別のドライブへは付け替えられないので、そのときはコピーしてから消す。
fn move_file(from: &Path, to: &Path, done: &AtomicUsize) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_file(from, to, done)?;
    std::fs::remove_file(from).map_err(|e| {
        // 元を消せなければ、2 つに増えたままにしないよう写した側を消す。
        let _ = std::fs::remove_file(to);
        format!("{}: {e}", from.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::Flag;
    use crate::test_util::TempDir;

    fn setup() -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new();
        let file = dir.mkdir("src").join("a.png");
        std::fs::write(&file, b"png").unwrap();
        let to = dir.mkdir("dst");
        (dir, file, to)
    }

    #[test]
    fn moving_takes_the_file_and_its_label_along() {
        let (_dir, file, to) = setup();
        let labels = LabelStore::new(None);
        labels.toggle_flag(&file, Flag::Good).unwrap();
        let t = Transfer {
            file: file.clone(),
            to_dir: to.clone(),
            op: Op::Move,
        };
        let dest = t.carry_file(&AtomicUsize::new(0)).unwrap();
        assert_eq!(dest, to.join("a.png"));
        assert!(t.carry_labels(&labels, &dest).is_none());
        assert!(!file.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"png");
        assert_eq!(labels.get(&dest).flag, Some(Flag::Good));
        assert!(labels.get(&file).is_empty());
    }

    #[test]
    fn copying_keeps_the_original_and_its_label() {
        let (_dir, file, to) = setup();
        let labels = LabelStore::new(None);
        labels.toggle_tag(&file, "cat").unwrap();
        let t = Transfer {
            file: file.clone(),
            to_dir: to,
            op: Op::Copy,
        };
        let dest = t.carry_file(&AtomicUsize::new(0)).unwrap();
        assert!(t.carry_labels(&labels, &dest).is_none());
        assert!(file.exists() && dest.exists());
        assert!(labels.get(&file).tags.contains("cat"));
        assert!(labels.get(&dest).tags.contains("cat"));
    }

    #[test]
    fn never_overwrites_a_file_of_the_same_name() {
        let (_dir, file, to) = setup();
        std::fs::write(to.join("a.png"), b"other").unwrap();
        let t = Transfer {
            file: file.clone(),
            to_dir: to.clone(),
            op: Op::Move,
        };
        assert!(t.carry_file(&AtomicUsize::new(0)).is_err());
        assert!(file.exists());
        assert_eq!(std::fs::read(to.join("a.png")).unwrap(), b"other");
    }

    #[test]
    fn copying_counts_the_bytes_and_keeps_the_modified_time() {
        let (_dir, file, to) = setup();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let t = Transfer {
            file,
            to_dir: to,
            op: Op::Copy,
        };
        let done = AtomicUsize::new(0);
        let dest = t.carry_file(&done).unwrap();
        assert_eq!(done.load(Ordering::Relaxed), 3);
        assert_eq!(std::fs::metadata(dest).unwrap().modified().unwrap(), old);
    }

    #[test]
    fn a_job_moves_the_file_on_another_thread() {
        let (_dir, file, to) = setup();
        let job = Job::start(
            Transfer {
                file: file.clone(),
                to_dir: to.clone(),
                op: Op::Move,
            },
            || {},
        );
        let result = loop {
            if let Some(r) = job.poll() {
                break r;
            }
            std::thread::yield_now();
        };
        assert_eq!(result.unwrap(), to.join("a.png"));
        assert!(!file.exists());
    }

    #[test]
    fn transfer_into_the_same_folder_is_a_noop() {
        let (_dir, file, _) = setup();
        let t = Transfer {
            to_dir: file.parent().unwrap().to_path_buf(),
            file,
            op: Op::Move,
        };
        assert!(t.is_noop());
    }
}
