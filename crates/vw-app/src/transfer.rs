//! スロットからフォルダへドラッグした画像の移動・コピー。ラベルも一緒に移す。

use std::path::{Path, PathBuf};

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

    /// ファイルを移し、付いていたラベルを移した先に付ける。移動なら元のラベルは消す。
    ///
    /// 同じ名前のファイルが先にあれば何もしない。上書きの確認を挟むより、移す側の名前を
    /// 変えてもらうほうが、画像を取り違えて消す心配がないため。
    /// ラベルを移せなかったときもファイルは移したままにし、`label_error` で知らせる。
    pub fn run(&self, labels: &mut LabelStore) -> Result<Done, String> {
        let dest = self.destination();
        if dest.exists() {
            return Err(format!("{}: already exists", dest.display()));
        }
        let label = labels.get(&self.file);
        match self.op {
            Op::Copy => copy_file(&self.file, &dest)?,
            Op::Move => move_file(&self.file, &dest)?,
        }
        let mut label_error = None;
        if !label.is_empty() {
            if let Err(e) = labels.set(&dest, label) {
                label_error = Some(e);
            } else if self.op == Op::Move {
                label_error = labels.set(&self.file, Label::default()).err();
            }
        }
        Ok(Done { dest, label_error })
    }
}

/// 移動・コピーの結果。
#[derive(Debug)]
pub struct Done {
    pub dest: PathBuf,
    pub label_error: Option<String>,
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", from.display()))
}

/// 名前の付け替えで移す。別のドライブへは付け替えられないので、そのときはコピーしてから消す。
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_file(from, to)?;
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
        let mut labels = LabelStore::new(None);
        labels.toggle_flag(&file, Flag::Good).unwrap();
        let t = Transfer {
            file: file.clone(),
            to_dir: to.clone(),
            op: Op::Move,
        };
        let done = t.run(&mut labels).unwrap();
        assert_eq!(done.dest, to.join("a.png"));
        assert!(done.label_error.is_none());
        assert!(!file.exists());
        assert_eq!(std::fs::read(&done.dest).unwrap(), b"png");
        assert_eq!(labels.get(&done.dest).flag, Some(Flag::Good));
        assert!(labels.get(&file).is_empty());
    }

    #[test]
    fn copying_keeps_the_original_and_its_label() {
        let (_dir, file, to) = setup();
        let mut labels = LabelStore::new(None);
        labels.toggle_tag(&file, "cat").unwrap();
        let t = Transfer {
            file: file.clone(),
            to_dir: to,
            op: Op::Copy,
        };
        let done = t.run(&mut labels).unwrap();
        assert!(file.exists() && done.dest.exists());
        assert!(labels.get(&file).tags.contains("cat"));
        assert!(labels.get(&done.dest).tags.contains("cat"));
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
        assert!(t.run(&mut LabelStore::new(None)).is_err());
        assert!(file.exists());
        assert_eq!(std::fs::read(to.join("a.png")).unwrap(), b"other");
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
