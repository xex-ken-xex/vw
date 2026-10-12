//! ファイルやフォルダを OS の既定のアプリで開く。

use std::path::Path;

/// ファイルを、拡張子に関連付けられたアプリで開く。
#[cfg(windows)]
pub fn open_file(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // cmd の start は拡張子に関連付けられたアプリで開く。CREATE_NO_WINDOW で
    // cmd 自身の黒い窓を出さない。関連付けがなければ Windows が「アプリを選択」を出す。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(drop)
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
