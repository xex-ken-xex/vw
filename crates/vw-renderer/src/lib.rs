//! eframe のアプリを OpenGL で起動し、OpenGL 2.0 のない環境では wgpu で起動し直す。
//!
//! リモートデスクトップで入った仮想マシンのように、Windows が OpenGL 1.1（ソフトウェア）しか
//! 持たない環境がある。Windows の wgpu は DirectX 12 で描き、GPU がなくても Windows に標準の
//! ソフトウェア描画（WARP）を使える。それでも普段を OpenGL にしておくのは、wgpu より起動が速いため。

use std::ffi::OsString;
use std::path::PathBuf;

use eframe::Renderer;

/// 使う描画の方法（`glow` か `wgpu`）。起動し直した子プロセスに伝えるほか、手で `wgpu` を指定して試せる。
pub const RENDERER_ENV: &str = "VW_RENDERER";
/// OpenGL で起動できなかった理由。起動し直した子プロセスがログと失敗の窓に出す。
const GLOW_ERROR_ENV: &str = "VW_GLOW_ERROR";

/// 環境変数の値から、使う描画の方法を決める。指定がないか読めない値なら OpenGL。
pub fn requested(value: Option<&str>) -> Renderer {
    match value.map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("wgpu") => Renderer::Wgpu,
        _ => Renderer::Glow,
    }
}

/// 起動に失敗したあとにすること。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// wgpu で起動し直す。
    Relaunch,
    /// 理由を窓に出して終わる。
    Report,
}

/// `graphics` は、失敗が描画の準備（OpenGL の初期化）によるものか。
/// アプリ自身の初期化の失敗は、wgpu にしても同じように失敗するので起動し直さない。
pub fn next_step(renderer: Renderer, graphics: bool) -> Step {
    if renderer == Renderer::Glow && graphics {
        Step::Relaunch
    } else {
        Step::Report
    }
}

fn is_graphics_failure(err: &eframe::Error) -> bool {
    matches!(
        err,
        eframe::Error::Glutin(_) | eframe::Error::NoGlutinConfigs(..) | eframe::Error::OpenGL(_)
    )
}

/// `eframe::run_native` の代わり。OpenGL で起動できなければ wgpu で起動し直し、
/// どちらでも起動できなければ理由を窓に出す（GUI のアプリには標準エラーがないため）。
pub fn run_native(
    app_name: &str,
    mut options: eframe::NativeOptions,
    app_creator: eframe::AppCreator<'_>,
) -> eframe::Result {
    let renderer = requested(std::env::var(RENDERER_ENV).ok().as_deref());
    options.renderer = renderer;
    let Err(err) = eframe::run_native(app_name, options, app_creator) else {
        return Ok(());
    };
    let reason = match next_step(renderer, is_graphics_failure(&err)) {
        // 同じプロセスの中で wgpu に切り替えない。winit のイベントループは 1 つのプロセスで 1 度しか
        // 作れず、使い回すには run_and_return にするしかないが、それでは窓を閉じたときの終わり方が変わる。
        Step::Relaunch => match relaunch_with_wgpu(&err.to_string()) {
            Ok(()) => return Ok(()),
            Err(e) => format!("OpenGL: {err}\nwgpu: could not restart: {e}"),
        },
        Step::Report => match glow_error() {
            Some(glow) => format!("OpenGL: {glow}\nwgpu: {err}"),
            None => err.to_string(),
        },
    };
    show_failure(
        app_name,
        &failure_message(app_name, &reason, check_script()),
    );
    Err(err)
}

/// 起動し直す前に OpenGL で失敗していれば、その理由。
pub fn glow_error() -> Option<String> {
    std::env::var(GLOW_ERROR_ENV).ok().filter(|e| !e.is_empty())
}

/// ログに書く、いま使っている描画の方法。
pub fn describe(cc: &eframe::CreationContext<'_>) -> String {
    let mut text = if let Some(gl) = &cc.gl {
        use eframe::glow::HasContext;
        // SAFETY: eframe が作って current にした GL のコンテキストから文字列を読むだけ
        let (version, renderer) = unsafe {
            (
                gl.get_parameter_string(eframe::glow::VERSION),
                gl.get_parameter_string(eframe::glow::RENDERER),
            )
        };
        format!("OpenGL {version} ({renderer})")
    } else if let Some(state) = &cc.wgpu_render_state {
        let info = state.adapter.get_info();
        format!(
            "wgpu {:?} ({}, {:?})",
            info.backend, info.name, info.device_type
        )
    } else {
        "unknown".to_owned()
    };
    if let Some(glow) = glow_error() {
        text.push_str(&format!(", because OpenGL failed: {glow}"));
    }
    text
}

fn relaunch_with_wgpu(glow_error: &str) -> std::io::Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    std::process::Command::new(std::env::current_exe()?)
        .args(args)
        .env(RENDERER_ENV, "wgpu")
        .env(GLOW_ERROR_ENV, glow_error)
        .spawn()
        .map(drop)
}

/// vw の配布の zip では、起動しない PC を調べるスクリプトが exe の隣の `check` フォルダにある。
fn check_script() -> Option<PathBuf> {
    let script = std::env::current_exe()
        .ok()?
        .parent()?
        .join("check")
        .join("check-vw.bat");
    script.is_file().then_some(script)
}

/// 失敗の窓の文面。設定を読む前なので UI の言語が分からず、英語と日本語を並べる。
pub fn failure_message(app_name: &str, reason: &str, check: Option<PathBuf>) -> String {
    let mut text = format!(
        "{app_name} could not start because the graphics could not be set up.\n\
         It needs OpenGL 2.0 or later, or DirectX 12.\n\n\
         描画の準備ができず、{app_name} を起動できませんでした。\n\
         OpenGL 2.0 以上か DirectX 12 が要ります。\n\n\
         {reason}"
    );
    if let Some(check) = check {
        text.push_str(&format!(
            "\n\nTo find the cause, run / 原因を調べるには次を実行してください:\n{}",
            check.display()
        ));
    }
    text
}

#[cfg(windows)]
fn show_failure(app_name: &str, message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (text, caption) = (wide(message), wide(app_name));
    // SAFETY: どちらも NUL で終わる UTF-16 で、呼び出しの間は生きている
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_failure(_app_name: &str, message: &str) {
    eprintln!("{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_opengl_unless_wgpu_is_requested() {
        assert_eq!(requested(None), Renderer::Glow);
        assert_eq!(requested(Some("")), Renderer::Glow);
        assert_eq!(requested(Some("glow")), Renderer::Glow);
        assert_eq!(requested(Some("vulkan")), Renderer::Glow);
        assert_eq!(requested(Some("wgpu")), Renderer::Wgpu);
        assert_eq!(requested(Some(" WGPU ")), Renderer::Wgpu);
    }

    #[test]
    fn restarts_with_wgpu_only_when_opengl_setup_failed() {
        assert_eq!(next_step(Renderer::Glow, true), Step::Relaunch);
        assert_eq!(next_step(Renderer::Glow, false), Step::Report);
        assert_eq!(next_step(Renderer::Wgpu, true), Step::Report);
        assert_eq!(next_step(Renderer::Wgpu, false), Step::Report);
    }

    #[test]
    fn app_creation_failure_is_not_a_graphics_failure() {
        let err = eframe::Error::AppCreation("settings".into());
        assert!(!is_graphics_failure(&err));
    }

    #[test]
    fn failure_message_names_the_app_the_reason_and_the_check_script() {
        let script = PathBuf::from("vw").join("check").join("check-vw.bat");
        let text = failure_message("vw", "OpenGL: requires 2.0", Some(script.clone()));
        assert!(text.contains("vw could not start"));
        assert!(text.contains("vw を起動できませんでした"));
        assert!(text.contains("OpenGL: requires 2.0"));
        assert!(text.contains(&script.display().to_string()));
    }

    #[test]
    fn failure_message_without_check_script_does_not_mention_it() {
        let text = failure_message("DicoAmplio", "wgpu: no adapter", None);
        assert!(!text.contains("check-vw"));
    }
}
