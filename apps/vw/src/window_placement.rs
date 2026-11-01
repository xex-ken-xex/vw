//! Windows でのメインウインドウの通常の位置と大きさ（最大化・最小化を解いたときの矩形）。
//!
//! egui の外枠の位置と内側の大きさを組み合わせて戻すと、Windows では起動のたびに右下へずれた
//! （ken の環境、2026-10-20）。Windows のアプリが位置を覚えるときの定番の
//! GetWindowPlacement / SetWindowPlacement で、OS の持つ通常の矩形をそのまま読み書きする。
//! 最大化・最小化中でも通常の矩形が取れるので、最大化前の大きさを別に覚えておく必要もない。
//! 座標はワークスペース座標（タスクバーを除いた作業領域が基準）で、読みと書きが同じ基準なので
//! 換算しない。

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// 通常の矩形 `[left, top, right, bottom]`（物理ピクセル）と、元に戻すと最大化になるか。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Normal {
    pub rect: [i32; 4],
    pub maximized: bool,
}

/// Windows 以外では `None`（egui の位置と大きさを使う）。
pub fn get(frame: &eframe::Frame) -> Option<Normal> {
    platform::get(hwnd(frame)?)
}

/// 通常の矩形を `rect` にする。`show` が偽なら隠したまま動かす（最初のフレームを描く前）。
pub fn set(frame: &eframe::Frame, rect: [i32; 4], show: bool) -> bool {
    hwnd(frame).is_some_and(|h| platform::set(h, rect, show))
}

fn hwnd(frame: &eframe::Frame) -> Option<isize> {
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

#[cfg(windows)]
mod platform {
    use super::Normal;
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, IsIconic, IsZoomed, SW_HIDE, SW_SHOWNORMAL, SetWindowPlacement,
        WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
    };

    pub fn get(hwnd: isize) -> Option<Normal> {
        let hwnd = hwnd as HWND;
        // SAFETY: 整数だけの C の構造体で、0 は正しい値。
        let mut p: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        p.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        // SAFETY: length を入れた構造体に書くだけ。hwnd が無効なら 0 が返る。
        if unsafe { GetWindowPlacement(hwnd, &mut p) } == 0 {
            return None;
        }
        let r = p.rcNormalPosition;
        // 最小化中は、最小化の前に最大化していたかを OS が覚えている。
        // SAFETY: どちらも hwnd を読むだけ。
        let maximized = if unsafe { IsIconic(hwnd) } != 0 {
            p.flags & WPF_RESTORETOMAXIMIZED != 0
        } else {
            // SAFETY: 同上。
            unsafe { IsZoomed(hwnd) != 0 }
        };
        Some(Normal {
            rect: [r.left, r.top, r.right, r.bottom],
            maximized,
        })
    }

    pub fn set(hwnd: isize, [left, top, right, bottom]: [i32; 4], show: bool) -> bool {
        let hwnd = hwnd as HWND;
        // SAFETY: 整数だけの C の構造体で、0 は正しい値。
        let mut p: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        p.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        p.showCmd = if show { SW_SHOWNORMAL } else { SW_HIDE } as u32;
        p.ptMinPosition = POINT { x: -1, y: -1 };
        p.ptMaxPosition = POINT { x: -1, y: -1 };
        p.rcNormalPosition = RECT {
            left,
            top,
            right,
            bottom,
        };
        // SAFETY: length を入れた構造体を読むだけ。hwnd が無効なら 0 が返る。
        unsafe { SetWindowPlacement(hwnd, &p) != 0 }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Normal;

    pub fn get(_hwnd: isize) -> Option<Normal> {
        None
    }

    pub fn set(_hwnd: isize, _rect: [i32; 4], _show: bool) -> bool {
        false
    }
}
