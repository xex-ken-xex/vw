//! フォルダの画像をまとめてスロットに読み込む（画像自動展開モード）。
//!
//! サブフォルダを辿るのは別スレッドで行い、見つけた画像のパスを順に UI スレッドへ渡す。
//! ホームやドライブ全体を辿ると数分かかることがあり、その間ウインドウが固まらないようにするため。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::Arc;

use vw_core::Registry;

use crate::sort::SortOrder;
use crate::tree::list_folder;

/// 辿るスレッドが先に見つけておけるパスの数。UI が確認で止まっている間に、
/// 辿るスレッドがドライブ全体を走査し続けないよう、ここで待たせる。
const LOOKAHEAD: usize = 256;

/// 1 回の読み込み（フォルダをダブルクリックしてから終わるまで）。
pub struct FolderLoad {
    pub root: PathBuf,
    rx: Receiver<PathBuf>,
    /// サブフォルダも読むかの答えを送る。答えるまで `Some`。
    subfolders: Option<Sender<bool>>,
    cancel: Arc<AtomicBool>,
    /// スロットに渡した枚数。
    pub loaded: usize,
    /// この枚数に達したら続けるか確認する。確認しない設定なら `None`。
    pause_at: Option<usize>,
    chunk: usize,
    /// 確認で止めたときに受け取っていた次のパス。
    held: Option<PathBuf>,
    /// 辿り終えた。
    done: bool,
}

/// 確認で止めているときの問い。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Question {
    /// サブフォルダも読むか。
    Subfolders,
    /// 次の区切りまで読み続けるか。
    More,
}

impl FolderLoad {
    /// `root` の読み込みを始める。直下の画像は確認せずに渡し、サブフォルダがあれば
    /// [`Question::Subfolders`] の答えを待ってから辿る。`chunk` 枚ごとに確認するなら `confirm`。
    /// `wake` は新しいパスを渡せるようになったときに呼ぶ（画面の再描画を頼む）。
    pub fn start(
        root: PathBuf,
        registry: Registry,
        order: SortOrder,
        confirm: bool,
        chunk: usize,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (dirs, files) = list_folder(&root, &registry, order);
        let (tx, rx) = sync_channel(LOOKAHEAD);
        let cancel = Arc::new(AtomicBool::new(false));
        let (subfolders, answer) = if dirs.is_empty() {
            (None, None)
        } else {
            let (s, a) = channel();
            (Some(s), Some(a))
        };
        let walker = Walker {
            registry,
            order,
            tx,
            cancel: cancel.clone(),
            wake: Box::new(wake),
        };
        let first = root.clone();
        std::thread::spawn(move || walker.run(&first, files, dirs, answer));
        Self {
            root,
            rx,
            subfolders,
            cancel,
            loaded: 0,
            pause_at: confirm.then_some(chunk.max(1)),
            chunk: chunk.max(1),
            held: None,
            done: false,
        }
    }

    /// 今答えを待っている問い。サブフォルダの問いを先に出す。
    pub fn question(&self) -> Option<Question> {
        if self.subfolders.is_some() {
            Some(Question::Subfolders)
        } else if self.held.is_some() && self.pause_at == Some(self.loaded) {
            Some(Question::More)
        } else {
            None
        }
    }

    /// 今スロットに渡せるパス。区切りに達したら、続けると答えるまで渡さない。
    pub fn take_ready(&mut self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        loop {
            let next = match self
                .held
                .take()
                .map(Ok)
                .unwrap_or_else(|| self.rx.try_recv())
            {
                Ok(p) => p,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.done = true;
                    break;
                }
            };
            // 区切りちょうどで画像が尽きたときに確認を出さないよう、次の 1 枚が届いてから止める。
            if self.pause_at == Some(self.loaded) {
                self.held = Some(next);
                break;
            }
            self.loaded += 1;
            out.push(next);
        }
        out
    }

    pub fn answer_subfolders(&mut self, yes: bool) {
        if let Some(s) = self.subfolders.take() {
            // 辿るスレッドが先に終わっていれば届かないが、そのときは答えも要らない。
            let _ = s.send(yes);
        }
    }

    /// 区切りの確認に答える。続けないなら false を返し、この読み込みは終わる。
    pub fn answer_more(&mut self, yes: bool) -> bool {
        if yes {
            self.pause_at = self.pause_at.map(|n| n + self.chunk);
        }
        yes
    }

    /// 全部渡し終えた。
    pub fn finished(&self) -> bool {
        self.done && self.held.is_none() && self.subfolders.is_none()
    }
}

impl Drop for FolderLoad {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// フォルダを辿るスレッドの持ち物。
struct Walker {
    registry: Registry,
    order: SortOrder,
    tx: SyncSender<PathBuf>,
    cancel: Arc<AtomicBool>,
    wake: Box<dyn Fn() + Send>,
}

impl Walker {
    fn run(
        self,
        root: &Path,
        files: Vec<PathBuf>,
        dirs: Vec<PathBuf>,
        answer: Option<Receiver<bool>>,
    ) {
        if !self.send_all(files) {
            return;
        }
        let Some(answer) = answer else {
            return;
        };
        if answer.recv() != Ok(true) {
            return;
        }
        self.walk(root, dirs);
    }

    /// 送れなくなったら（読み込みが取り消されたら）false。
    fn send_all(&self, files: Vec<PathBuf>) -> bool {
        for f in files {
            if self.cancel.load(Ordering::Relaxed) || self.tx.send(f).is_err() {
                return false;
            }
            (self.wake)();
        }
        true
    }

    /// `dirs` を順に、各フォルダの画像 → そのサブフォルダの順で深さ優先に辿る。
    ///
    /// 再帰呼び出しにしないのは、深い階層でスレッドのスタックが尽きないようにするため。
    /// 一度辿ったフォルダは実体のパスで覚えて飛ばす。シンボリックリンクが親を指していると
    /// 同じフォルダを無限に辿ってしまうため。
    fn walk(&self, root: &Path, dirs: Vec<PathBuf>) {
        let mut seen: HashSet<PathBuf> = root.canonicalize().into_iter().collect();
        let mut stack: Vec<PathBuf> = dirs.into_iter().rev().collect();
        while let Some(dir) = stack.pop() {
            if self.cancel.load(Ordering::Relaxed) {
                return;
            }
            let real = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            if !seen.insert(real) {
                continue;
            }
            let (subdirs, files) = list_folder(&dir, &self.registry, self.order);
            if !self.send_all(files) {
                return;
            }
            stack.extend(subdirs.into_iter().rev());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{png_registry, TempDir};
    use std::time::{Duration, Instant};

    fn start(root: &Path, confirm: bool, chunk: usize) -> FolderLoad {
        FolderLoad::start(
            root.to_path_buf(),
            png_registry(),
            SortOrder::default(),
            confirm,
            chunk,
            || {},
        )
    }

    /// 止まるか辿り終わるまで受け取る。
    fn drain(load: &mut FolderLoad) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut out = Vec::new();
        while !load.finished() && load.question().is_none() {
            assert!(Instant::now() < deadline, "walker did not finish");
            out.extend(load.take_ready());
            std::thread::sleep(Duration::from_millis(1));
        }
        out.iter()
            .map(|p| {
                p.strip_prefix(&load.root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn images_directly_in_the_folder_load_without_asking() {
        let dir = TempDir::new();
        dir.touch("b10.png");
        dir.touch("b2.png");
        dir.touch("notes.txt");
        let mut load = start(dir.path(), true, 500);
        assert_eq!(drain(&mut load), ["b2.png", "b10.png"]);
        assert!(load.finished());
    }

    #[test]
    fn subfolders_are_asked_about_after_the_direct_images() {
        let dir = TempDir::new();
        dir.touch("top.png");
        dir.mkdir("sub");
        dir.touch("sub/inner.png");
        let mut load = start(dir.path(), true, 500);
        assert_eq!(load.question(), Some(Question::Subfolders));
        // 答える前でも直下の画像は届く。
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        while got.is_empty() {
            assert!(Instant::now() < deadline);
            got.extend(load.take_ready());
        }
        assert_eq!(got, [dir.path().join("top.png")]);
        load.answer_subfolders(true);
        assert_eq!(drain(&mut load), ["sub/inner.png"]);
        assert!(load.finished());
    }

    #[test]
    fn declining_subfolders_loads_only_the_direct_images() {
        let dir = TempDir::new();
        dir.touch("top.png");
        dir.mkdir("sub");
        dir.touch("sub/inner.png");
        let mut load = start(dir.path(), true, 500);
        load.answer_subfolders(false);
        assert_eq!(drain(&mut load), ["top.png"]);
        assert!(load.finished());
    }

    #[test]
    fn subfolders_are_walked_depth_first_in_sort_order() {
        let dir = TempDir::new();
        dir.mkdir("a10");
        dir.mkdir("a2");
        dir.mkdir("a2/deep");
        dir.touch("a2/deep/3.png");
        dir.touch("a2/1.png");
        dir.touch("a10/2.png");
        let mut load = start(dir.path(), false, 500);
        load.answer_subfolders(true);
        assert_eq!(drain(&mut load), ["a2/1.png", "a2/deep/3.png", "a10/2.png"]);
    }

    #[test]
    fn loading_pauses_at_each_chunk_until_confirmed() {
        let dir = TempDir::new();
        for i in 1..=5 {
            dir.touch(&format!("{i}.png"));
        }
        let mut load = start(dir.path(), true, 2);
        assert_eq!(drain(&mut load), ["1.png", "2.png"]);
        assert_eq!(load.question(), Some(Question::More));
        assert!(load.answer_more(true));
        assert_eq!(drain(&mut load), ["3.png", "4.png"]);
        assert!(load.answer_more(true));
        assert_eq!(drain(&mut load), ["5.png"]);
        assert!(load.finished());
        assert_eq!(load.loaded, 5);
    }

    #[test]
    fn no_question_when_the_images_end_exactly_at_a_chunk() {
        let dir = TempDir::new();
        dir.touch("1.png");
        dir.touch("2.png");
        let mut load = start(dir.path(), true, 2);
        assert_eq!(drain(&mut load), ["1.png", "2.png"]);
        assert_eq!(load.question(), None);
        assert!(load.finished());
    }

    #[test]
    fn stopping_at_a_chunk_ends_the_load() {
        let dir = TempDir::new();
        for i in 1..=3 {
            dir.touch(&format!("{i}.png"));
        }
        let mut load = start(dir.path(), true, 2);
        drain(&mut load);
        assert!(!load.answer_more(false));
    }

    #[test]
    fn without_confirmation_everything_loads_at_once() {
        let dir = TempDir::new();
        for i in 1..=5 {
            dir.touch(&format!("{i}.png"));
        }
        let mut load = start(dir.path(), false, 2);
        assert_eq!(drain(&mut load).len(), 5);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_back_to_a_parent_is_not_walked_forever() {
        let dir = TempDir::new();
        dir.mkdir("sub");
        dir.touch("sub/a.png");
        std::os::unix::fs::symlink(dir.path(), dir.path().join("sub/loop")).unwrap();
        let mut load = start(dir.path(), false, 500);
        load.answer_subfolders(true);
        assert_eq!(drain(&mut load), ["sub/a.png"]);
    }
}
