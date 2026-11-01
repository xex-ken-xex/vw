//! 引き出した欄やヒストグラムなどの別ウインドウを、本体の持ち物（Windows の owned window、
//! X11 の WM_TRANSIENT_FOR）にして、本体より手前に保つ。
//!
//! 常に最前面（AlwaysOnTop）にしないのは、ほかのアプリの上にまで出てしまうから。
//! egui は別ウインドウを作るときに持ち主を渡す手段を持たないので、作られたあとで
//! 自分のプロセスのウインドウを探して持ち主を付ける。

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// 持ち主を付け終えた別ウインドウの数。egui のビューポートの数と合うまで、毎フレーム探し直す
/// （ウインドウは egui がビューポートを作ったフレームより後に現れることがあるため）。
#[derive(Default)]
pub struct KeepInFront {
    adopted: usize,
}

impl KeepInFront {
    pub fn update(&mut self, ctx: &eframe::egui::Context, frame: &eframe::Frame) {
        let others = ctx.input(|i| i.raw.viewports.len()).saturating_sub(1);
        if others == self.adopted {
            return;
        }
        let Ok(handle) = frame.window_handle() else {
            return;
        };
        // 探せない環境（Wayland、ウインドウマネージャの無い X11）では、探し直し続けないよう済んだことにする。
        self.adopted = platform::adopt(handle.as_raw())
            .unwrap_or(others)
            .min(others);
    }
}

#[cfg(windows)]
mod platform {
    use super::RawWindowHandle;
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, GetClassNameW, GetWindow, SetWindowLongPtrW, SetWindowPos,
        GWLP_HWNDPARENT, GW_OWNER, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };

    /// winit が作るウインドウのクラス名。winit が裏で作るメッセージ用のウインドウと見分けるのに使う。
    const WINIT_CLASS: &str = "Window Class";

    struct Search {
        main: HWND,
        found: usize,
    }

    /// 本体と同じスレッドの別ウインドウに本体を持ち主として付け、別ウインドウの数を返す。
    pub fn adopt(main: RawWindowHandle) -> Option<usize> {
        let RawWindowHandle::Win32(main) = main else {
            return None;
        };
        let mut search = Search {
            main: main.hwnd.get() as HWND,
            found: 0,
        };
        // SAFETY: visit は列挙の間だけ呼ばれ、渡した Search はこの呼び出しの間生きている。
        unsafe {
            EnumThreadWindows(
                GetCurrentThreadId(),
                Some(visit),
                &mut search as *mut Search as LPARAM,
            );
        }
        Some(search.found)
    }

    unsafe extern "system" fn visit(hwnd: HWND, search: LPARAM) -> BOOL {
        // SAFETY: search は adopt() が渡した Search へのポインタで、列挙の間だけ使う。
        let search = unsafe { &mut *(search as *mut Search) };
        if hwnd == search.main || !is_winit_window(hwnd) {
            return 1;
        }
        search.found += 1;
        // 作ったあとで持ち主を変えるのは、CreateWindowEx の hWndParent に渡す手段が
        // egui に無いから。GWLP_HWNDPARENT は最上位のウインドウでは持ち主を変える。
        // SAFETY: 列挙中の hwnd を読むだけ。
        if unsafe { GetWindow(hwnd, GW_OWNER) }.is_null() {
            // SAFETY: どちらも同じスレッドのウインドウの手がかりと整数を渡すだけ。
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, search.main as isize);
                // 持ち主を付けただけでは、次に本体を触るまで重なりの順は変わらない。
                // 本体を別ウインドウのすぐ下へ動かす（HWND_TOP で別ウインドウを上げると、
                // ほかのアプリの上にまで出てしまう）。
                SetWindowPos(
                    search.main,
                    hwnd,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
        1
    }

    fn is_winit_window(hwnd: HWND) -> bool {
        let mut name = [0u16; 32];
        // SAFETY: 書き込み先の配列と、その長さを渡している。
        let len = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
        len > 0 && String::from_utf16_lossy(&name[..len as usize]) == WINIT_CLASS
    }
}

#[cfg(not(windows))]
mod platform {
    use super::RawWindowHandle;
    use std::sync::OnceLock;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ConfigureWindowAux, ConnectionExt, PropMode, StackMode, Window,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    /// 本体と同じプロセスのウインドウに WM_TRANSIENT_FOR で本体を付け、別ウインドウの数を返す。
    /// Wayland では何もしない（ほかのウインドウを指す手段がプロトコルに無い）。
    pub fn adopt(main: RawWindowHandle) -> Option<usize> {
        let main: Window = match main {
            RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok()?,
            RawWindowHandle::Xcb(h) => h.window.get(),
            _ => return None,
        };
        let X11 {
            conn,
            root,
            client_list,
            pid_atom,
        } = X11::get()?;
        let pid = std::process::id();
        let windows: Vec<Window> = conn
            .get_property(false, *root, *client_list, AtomEnum::WINDOW, 0, u32::MAX)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .collect();
        let mut found = 0;
        for w in windows.into_iter().filter(|&w| w != main) {
            let owner = conn
                .get_property(false, w, *pid_atom, AtomEnum::CARDINAL, 0, 1)
                .ok()
                .and_then(|c| c.reply().ok())
                .and_then(|r| r.value32()?.next());
            if owner != Some(pid) {
                continue;
            }
            found += 1;
            let _ = conn.change_property32(
                PropMode::REPLACE,
                w,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                &[main],
            );
        }
        if found > 0 {
            // 付けただけでは、次に本体を触るまで重なりの順は変わらない。本体を上げると、
            // ウインドウマネージャが持ち物をその上に載せ直す（別ウインドウを本体の上へ
            // 動かすよう頼んでも、親の違うウインドウ同士なので openbox などは聞かない）。
            let _ = conn.configure_window(
                main,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            );
        }
        let _ = conn.flush();
        Some(found)
    }

    struct X11 {
        conn: RustConnection,
        root: Window,
        client_list: u32,
        pid_atom: u32,
    }

    impl X11 {
        fn get() -> Option<&'static X11> {
            static X11: OnceLock<Option<X11>> = OnceLock::new();
            X11.get_or_init(|| {
                let (conn, screen) = x11rb::connect(None).ok()?;
                let root = conn.setup().roots[screen].root;
                let atom = |name: &[u8]| -> Option<u32> {
                    Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
                };
                let client_list = atom(b"_NET_CLIENT_LIST")?;
                let pid_atom = atom(b"_NET_WM_PID")?;
                Some(X11 {
                    conn,
                    root,
                    client_list,
                    pid_atom,
                })
            })
            .as_ref()
        }
    }
}
