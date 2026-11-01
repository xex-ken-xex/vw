//! `restrict_dll_search` の前後で、名前だけの DLL の読み込みがどこを探すかを確かめる。
//!
//! `SetDefaultDllDirectories` はプロセス全体に効いて元に戻せないので、「制限する前」の確認も
//! 同じテスト関数の中で先に済ませる。テスト関数を分けると実行順が決まらず、別のテストが先に
//! 制限をかけてしまうため、この binary にはテストを 1 つだけ置く。
#![cfg(windows)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut core::ffi::c_void;
    fn FreeLibrary(module: *mut core::ffi::c_void) -> i32;
    fn GetModuleFileNameW(module: *mut core::ffi::c_void, buf: *mut u16, len: u32) -> u32;
    fn GetLastError() -> u32;
}

const ERROR_MOD_NOT_FOUND: u32 = 126;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// 名前だけで読み込み、読めたらそのファイルのパス、読めなければエラーコードを返す。
fn load_by_name(name: &str) -> Result<PathBuf, u32> {
    let name = wide(OsStr::new(name));
    // SAFETY: NUL で終わる UTF-16 で、呼び出しの間は生きている。
    let module = unsafe { LoadLibraryW(name.as_ptr()) };
    if module.is_null() {
        // SAFETY: 引数なし。
        return Err(unsafe { GetLastError() });
    }
    let mut buf = vec![0u16; 32768];
    // SAFETY: buf は len 要素ぶん確保してあり、module は今読み込んだ生きているハンドル。
    let len = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    // SAFETY: LoadLibraryW が返したハンドルを 1 回だけ解放する。
    unsafe { FreeLibrary(module) };
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

/// テストの exe と同じフォルダに、DLL の名前をした中身のでたらめなファイルを置く。
/// 読み込みの探す先にそのフォルダが入っていれば、見つかって「形式が不正」で失敗し、
/// 入っていなければ「見つからない」（ERROR_MOD_NOT_FOUND）で失敗する。この違いで見分ける。
struct BogusDll {
    path: PathBuf,
}

impl BogusDll {
    fn beside_exe(name: &str) -> Self {
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let path = dir.join(name);
        std::fs::write(&path, b"this is not a DLL").unwrap();
        Self { path }
    }
}

impl Drop for BogusDll {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn is_under_system32(path: &Path) -> bool {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let system32 = Path::new(&system_root).join("System32");
    let lower = |p: &Path| p.to_string_lossy().to_ascii_lowercase();
    lower(path).starts_with(&lower(&system32))
}

#[test]
fn after_restriction_dll_beside_exe_is_not_found_but_system32_dll_still_loads() {
    let name = "vw-renderer-dll-search-test.dll";
    let bogus = BogusDll::beside_exe(name);

    let before = load_by_name(name).unwrap_err();
    assert_ne!(
        before, ERROR_MOD_NOT_FOUND,
        "制限する前は exe のフォルダを探すので、置いたファイルが見つかって別の理由で失敗するはず"
    );

    vw_renderer::restrict_dll_search();

    let after = load_by_name(name).unwrap_err();
    assert_eq!(
        after, ERROR_MOD_NOT_FOUND,
        "制限した後は exe のフォルダを探さないので、見つからないはず"
    );

    for system_dll in ["dxgi.dll", "d3d12.dll", "opengl32.dll"] {
        let path = load_by_name(system_dll)
            .unwrap_or_else(|err| panic!("{system_dll} が読めない: エラー {err}"));
        assert!(
            is_under_system32(&path),
            "{system_dll} は System32 のものが読まれるはず: {}",
            path.display()
        );
    }

    drop(bogus);
}
