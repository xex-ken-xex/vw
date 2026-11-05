//! Microsoft Store 版（MSIX パッケージ）として動いているときの一時フォルダ。
//!
//! パッケージのアプリが %TEMP%（%LOCALAPPDATA% の下）に書いたものは、Windows が
//! `%LOCALAPPDATA%\Packages\<パッケージ ファミリ名>\LocalCache\Local` の下へ振り替える。振り替えた先は
//! vw 自身には元の場所に見えるが、エクスプローラーなどパッケージの外のプロセスには見えない。
//! そこで起動直後に、振り替え先の本当の場所を %TEMP% と %TMP% へ入れ、ZIP を解凍した画像なども
//! 外から見える場所に置く。
//!
//! 振り替えをやめる制限付き機能（unvirtualizedResources）は、Store の認定で認められなかった
//! （2026-11-04）。一時フォルダを使う箇所ごとに直す案は、箇所が多く漏れやすいので採らない。
//! %APPDATA% は振り替えない。子のプロセス（設定ファイルを開くエディタなど）が自分の設定を
//! vw の場所に探してしまうため。設定の場所は `vw_pipeline::settings_file::config_path` が決める。

// Linux では振り替えがなく、パスを求める関数はテストからしか呼ばれない。
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

/// `local_appdata` は振り替え前の %LOCALAPPDATA%、`family` はパッケージ ファミリ名。
pub fn redirected_temp(local_appdata: &Path, family: &str) -> PathBuf {
    local_appdata
        .join("Packages")
        .join(family)
        .join("LocalCache")
        .join("Local")
        .join("Temp")
}

/// パッケージとして動いていれば、%TEMP% と %TMP% を振り替え先の本当の場所へ向ける。
/// スレッドを作る前（main の最初）に呼ぶこと。
#[cfg(windows)]
pub fn redirect_temp() {
    let Some(family) = family_name() else {
        return;
    };
    let Some(local_appdata) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return;
    };
    let temp = redirected_temp(&local_appdata, &family);
    if std::fs::create_dir_all(&temp).is_err() {
        return;
    }
    // SAFETY: main の最初で、ほかのスレッドがまだないときに呼ばれる。
    unsafe {
        std::env::set_var("TEMP", &temp);
        std::env::set_var("TMP", &temp);
    }
}

#[cfg(not(windows))]
pub fn redirect_temp() {}

#[cfg(windows)]
fn family_name() -> Option<String> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFamilyName;
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    // SAFETY: 長さを渡したバッファに書くだけ。パッケージでなければ APPMODEL_ERROR_NO_PACKAGE が返る。
    let err = unsafe { GetCurrentPackageFamilyName(&mut len, buf.as_mut_ptr()) };
    if err != ERROR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_temp_into_the_package_local_cache() {
        let local = Path::new(r"C:\Users\ken\AppData\Local");
        let family = vw_pipeline::settings_file::STORE_PACKAGE_FAMILY;
        assert_eq!(
            redirected_temp(local, family),
            vw_pipeline::settings_file::store_local_cache(local)
                .join("Local")
                .join("Temp")
        );
    }
}
