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

/// ファイルを、拡張子に関連付けられたアプリで開く。
#[cfg(windows)]
pub fn open_file(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{ShellExecuteW, SE_ERR_NOASSOC};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    // cmd の start を使わないのは、cmd が引数を独自に読み直すため、起動はしても
    // 何も開かないことがあり、失敗も返ってこないから。ShellExecuteW なら結果の値で失敗が分かる。
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
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
