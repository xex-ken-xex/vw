//! ファイルやフォルダを、OS の既定のアプリや、設定で決めたアプリで開く。

pub mod args;
pub mod assoc;

use std::ffi::OsString;
use std::path::Path;

/// `program` を `args` を付けて起動する。終わるのは待たない。
///
/// シェル（cmd や sh）を通さないのは、パスに `&` や `;` があると、そこで別のコマンドとして
/// 読まれてしまうため。
pub fn run(program: &Path, args: &[OsString]) -> std::io::Result<()> {
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .spawn()
        .map(drop)
}

/// [`run_captured`] で受け取った、アプリの出力と終わり方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    /// 終了コード。シグナルで止められたときは `None`。
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// [`run`] と同じに起動し、標準出力・標準エラーを受け取る。終わったら別スレッドで `done` を呼ぶ。
///
/// Windows ではコンソールの窓を出さない。出力は vw が受け取るので、窓は一瞬開いて閉じるだけになるため。
/// 出力は UTF-8 として読み、読めないバイトは置き換える（コンソールの文字コードで書く道具の
/// 日本語は化けるが、ログで終わり方と英語の出力が分かれば足りる）。
pub fn run_captured(
    program: &Path,
    args: &[OsString],
    done: impl FnOnce(std::io::Result<Captured>) + Send + 'static,
) -> std::io::Result<()> {
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let child = command.spawn()?;
    std::thread::spawn(move || {
        let result = child.wait_with_output().map(|out| Captured {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
        done(result);
    });
    Ok(())
}

/// ファイルを、拡張子に関連付けられたアプリで開く。
#[cfg(windows)]
pub fn open_file(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SE_ERR_NOASSOC, ShellExecuteW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    // cmd の start を使わないのは、cmd が引数を独自に読み直すため、起動はしても
    // 何も開かないことがあり、失敗も返ってこないから。ShellExecuteW なら結果の値で失敗が分かる。
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: wide は NUL で終わり、呼び出しの間生きている。ほかは null か定数。
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result > 32 {
        return Ok(());
    }
    if result == SE_ERR_NOASSOC as isize {
        // 関連付けが無いときは「アプリを選択」を出す。ShellExecuteW の "openas" は
        // Windows 10 以降で黙って失敗することがあるため、rundll32 から出す。
        return std::process::Command::new("rundll32")
            .arg("shell32.dll,OpenAs_RunDLL")
            .arg(path)
            .spawn()
            .map(drop);
    }
    Err(std::io::Error::other(format!(
        "ShellExecuteW failed ({result})"
    )))
}

#[cfg(not(windows))]
pub fn open_file(path: &Path) -> std::io::Result<()> {
    xdg_open(path)
}

/// Web のページを既定のブラウザで開く。ファイルと同じ口（Windows は ShellExecuteW、
/// Linux は xdg-open）がそのまま URL も受けるので、ブラウザを探す依存は足さない。
pub fn open_url(url: &str) -> std::io::Result<()> {
    open_file(Path::new(url))
}

/// フォルダをエクスプローラ（Linux では既定のファイラ）で開く。
#[cfg(windows)]
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    // ファイルと同じ cmd の start にしないのは、cmd がパスの & や ^ を区切りとして読み、
    // そうした文字を含むフォルダを開けなくなるため。explorer に直接渡せば解釈されない。
    std::process::Command::new("explorer")
        .arg(path)
        .spawn()
        .map(drop)
}

#[cfg(not(windows))]
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    xdg_open(path)
}

#[cfg(not(windows))]
fn xdg_open(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(drop)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn run_captured_hands_back_the_output_and_exit_code() {
        let (tx, rx) = channel();
        let args = ["-c", "echo out; echo err >&2; exit 3"].map(OsString::from);
        run_captured(Path::new("/bin/sh"), &args, move |r| tx.send(r).unwrap()).unwrap();
        let got = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!(
            got,
            Captured {
                code: Some(3),
                stdout: "out\n".into(),
                stderr: "err\n".into(),
            }
        );
    }

    #[test]
    fn run_captured_fails_at_once_for_a_missing_program() {
        assert!(run_captured(Path::new("/no/such/program"), &[], |_| {}).is_err());
    }
}
