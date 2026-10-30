//! OS にたずねる、画面のマウスの位置（物理ピクセル）と、主ボタンを押しているか。
//! OS のウインドウバーでウインドウを動かしている間は、マウスの出来事がアプリに届かないので、
//! その間だけ使う。分からない環境（macOS など）では `None`。
//! Wayland ではウインドウの位置がそもそも分からず、動いたことに気づけないので、ここまで来ない。

use egui::Pos2;

#[cfg(windows)]
pub fn global() -> Option<(Pos2, bool)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetSystemMetrics, SM_SWAPBUTTON,
    };
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: GetCursorPos は渡した POINT に書くだけで、ほかの引数を取らない。
    if unsafe { GetCursorPos(&mut p) } == 0 {
        return None;
    }
    // GetAsyncKeyState は物理的なボタンを返すので、左右を入れ替えている人には右が主ボタン。
    // SAFETY: どちらも引数の値だけを読む。
    let primary = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 {
        VK_RBUTTON
    } else {
        VK_LBUTTON
    };
    let down = unsafe { GetAsyncKeyState(primary.into()) } < 0;
    Some((Pos2::new(p.x as f32, p.y as f32), down))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
pub fn global() -> Option<(Pos2, bool)> {
    use std::sync::{Mutex, OnceLock};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, KeyButMask, Window};
    use x11rb::rust_connection::RustConnection;

    static X: OnceLock<Option<Mutex<(RustConnection, Window)>>> = OnceLock::new();
    let x = X.get_or_init(|| {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        Some(Mutex::new((conn, root)))
    });
    let guard = x.as_ref()?.lock().ok()?;
    let (conn, root) = &*guard;
    let reply = conn.query_pointer(*root).ok()?.reply().ok()?;
    let down = reply.mask.contains(KeyButMask::BUTTON1);
    Some((
        Pos2::new(f32::from(reply.root_x), f32::from(reply.root_y)),
        down,
    ))
}

#[cfg(not(any(
    windows,
    all(unix, not(any(target_os = "macos", target_os = "android")))
)))]
pub fn global() -> Option<(Pos2, bool)> {
    None
}
