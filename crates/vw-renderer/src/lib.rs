//! eframe のアプリを OpenGL で起動し、OpenGL 2.0 のない環境では wgpu で起動し直す。
//!
//! リモートデスクトップで入った仮想マシンのように、Windows が OpenGL 1.1（ソフトウェア）しか
//! 持たない環境がある。Windows の wgpu は DirectX 12 で描き、GPU がなくても Windows に標準の
//! ソフトウェア描画（WARP）を使える。それでも普段を OpenGL にしておくのは、wgpu より起動が速いため。
//!
//! 10bit の表示を頼まれたときは逆に wgpu で起動し、起動できなければ OpenGL（8bit）で起動し直す。

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui_wgpu;
use eframe::Renderer;

/// 使う描画の方法（`glow` か `wgpu`）。起動し直した子プロセスに伝えるほか、手で `wgpu` を指定して試せる。
pub const RENDERER_ENV: &str = "VW_RENDERER";
/// OpenGL で起動できなかった理由。起動し直した子プロセスがログと失敗の窓に出す。
const GLOW_ERROR_ENV: &str = "VW_GLOW_ERROR";
/// 10bit の面を頼んで wgpu で起動できなかった理由。OpenGL で起動し直した子プロセスが出す。
const WGPU_ERROR_ENV: &str = "VW_WGPU_ERROR";

/// 描画先の面（ウインドウに描く画素の置き場）の 1 色あたりの深さ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// 8bit。OpenGL で起動する。
    Standard,
    /// 10bit を頼む。wgpu で起動し、10bit の形式（Rgb10a2Unorm）が使えればそれにする。
    ///
    /// OpenGL で 10bit の面を頼む方法は採らない。glutin で 10bit の設定を選んでも、Windows では
    /// 一部の業務用の GPU のドライバしか応えず、届かなくても分からないため。
    TenBit,
}

/// 環境変数の値から、使う描画の方法を決める。指定がないか読めない値なら、
/// 8bit の面では OpenGL、10bit の面では wgpu。
pub fn requested(value: Option<&str>, depth: Depth) -> Renderer {
    match (value.map(str::trim), depth) {
        (Some(v), _) if v.eq_ignore_ascii_case("wgpu") => Renderer::Wgpu,
        (Some(v), _) if v.eq_ignore_ascii_case("glow") => Renderer::Glow,
        (_, Depth::TenBit) => Renderer::Wgpu,
        (_, Depth::Standard) => Renderer::Glow,
    }
}

/// 起動に失敗したあとにすること。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// もう一方の描画の方法で起動し直す。
    Relaunch(Renderer),
    /// 理由を窓に出して終わる。
    Report,
}

/// `graphics` は、失敗が描画の準備（OpenGL や wgpu の初期化）によるものか。
/// アプリ自身の初期化の失敗は、描画の方法を変えても同じように失敗するので起動し直さない。
/// `relaunched` は、このプロセスがすでに起動し直したものか（行ったり来たりしないため）。
pub fn next_step(renderer: Renderer, depth: Depth, graphics: bool, relaunched: bool) -> Step {
    match (renderer, depth) {
        _ if !graphics || relaunched => Step::Report,
        (Renderer::Glow, _) => Step::Relaunch(Renderer::Wgpu),
        (Renderer::Wgpu, Depth::TenBit) => Step::Relaunch(Renderer::Glow),
        (Renderer::Wgpu, Depth::Standard) => Step::Report,
    }
}

fn is_graphics_failure(err: &eframe::Error) -> bool {
    matches!(
        err,
        eframe::Error::Glutin(_)
            | eframe::Error::NoGlutinConfigs(..)
            | eframe::Error::OpenGL(_)
            | eframe::Error::Wgpu(_)
    )
}

/// `eframe::run_native` の代わり。OpenGL で起動できなければ wgpu で起動し直し、
/// どちらでも起動できなければ理由を窓に出す（GUI のアプリには標準エラーがないため）。
pub fn run_native(
    app_name: &str,
    options: eframe::NativeOptions,
    app_creator: eframe::AppCreator<'_>,
) -> eframe::Result {
    run_native_with_depth(app_name, options, Depth::Standard, app_creator)
}

/// [`run_native`] の、描画先の面の深さを選べるもの。10bit の面を頼んで wgpu で起動できなければ、
/// OpenGL（8bit）で起動し直す。
pub fn run_native_with_depth(
    app_name: &str,
    mut options: eframe::NativeOptions,
    depth: Depth,
    app_creator: eframe::AppCreator<'_>,
) -> eframe::Result {
    let renderer = requested(std::env::var(RENDERER_ENV).ok().as_deref(), depth);
    options.renderer = renderer;
    if renderer == Renderer::Wgpu && depth == Depth::TenBit {
        ask_for_ten_bit(&mut options);
    }
    let Err(err) = eframe::run_native(app_name, options, app_creator) else {
        return Ok(());
    };
    let relaunched = glow_error().is_some() || wgpu_error().is_some();
    let reason = match next_step(renderer, depth, is_graphics_failure(&err), relaunched) {
        // 同じプロセスの中で切り替えない。winit のイベントループは 1 つのプロセスで 1 度しか
        // 作れず、使い回すには run_and_return にするしかないが、それでは窓を閉じたときの終わり方が変わる。
        Step::Relaunch(next) => match relaunch_with(next, &err.to_string()) {
            Ok(()) => return Ok(()),
            Err(e) => format!(
                "{}: {err}\n{}: could not restart: {e}",
                name(renderer),
                name(next)
            ),
        },
        Step::Report => match (glow_error(), wgpu_error()) {
            (Some(glow), _) => format!("OpenGL: {glow}\nwgpu: {err}"),
            (_, Some(wgpu)) => format!("wgpu: {wgpu}\nOpenGL: {err}"),
            _ => err.to_string(),
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

/// 10bit の面を頼んで wgpu で起動できず、OpenGL で起動し直したなら、その理由。
pub fn wgpu_error() -> Option<String> {
    std::env::var(WGPU_ERROR_ENV).ok().filter(|e| !e.is_empty())
}

fn name(renderer: Renderer) -> &'static str {
    match renderer {
        Renderer::Glow => "OpenGL",
        Renderer::Wgpu => "wgpu",
    }
}

/// 描画先の面に 10bit の形式を頼み、16bit の画像のテクスチャ（Rgba16Unorm）を使えるようにする。
/// 16bit のテクスチャは wgpu の必須の機能ではないので、GPU が持っているときだけ頼む。
fn ask_for_ten_bit(options: &mut eframe::NativeOptions) {
    egui_wgpu::set_preferred_surface_formats(vec![wgpu::TextureFormat::Rgb10a2Unorm]);
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    let base = Arc::clone(&setup.device_descriptor);
    setup.device_descriptor = Arc::new(move |adapter| {
        let mut descriptor = base(adapter);
        descriptor.required_features |=
            adapter.features() & wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        descriptor
    });
    options.wgpu_options.wgpu_setup = egui_wgpu::WgpuSetup::CreateNew(setup);
}

/// 描画先の面の 1 色あたりの bit 数。OpenGL で描いているときは 8 とみなす
/// （glutin が 8bit の設定を選ぶため）。
pub fn surface_bits(frame: &eframe::Frame) -> u8 {
    frame
        .wgpu_render_state()
        .map_or(8, |s| format_bits(s.target_format))
}

/// 描画先の形式の 1 色あたりの bit 数。
pub fn format_bits(format: wgpu::TextureFormat) -> u8 {
    use wgpu::TextureFormat as F;
    match format {
        F::Rgb10a2Unorm => 10,
        F::Rgba16Float => 16,
        _ => 8,
    }
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
            "wgpu {:?} ({}, {:?}), surface {:?}",
            info.backend, info.name, info.device_type, state.target_format
        )
    } else {
        "unknown".to_owned()
    };
    if let Some(glow) = glow_error() {
        text.push_str(&format!(", because OpenGL failed: {glow}"));
    }
    if let Some(wgpu) = wgpu_error() {
        text.push_str(&format!(", because wgpu failed: {wgpu}"));
    }
    text
}

fn relaunch_with(renderer: Renderer, error: &str) -> std::io::Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (value, error_env) = match renderer {
        Renderer::Wgpu => ("wgpu", GLOW_ERROR_ENV),
        Renderer::Glow => ("glow", WGPU_ERROR_ENV),
    };
    std::process::Command::new(std::env::current_exe()?)
        .args(args)
        .env(RENDERER_ENV, value)
        .env(error_env, error)
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
        let standard = |v| requested(v, Depth::Standard);
        assert_eq!(standard(None), Renderer::Glow);
        assert_eq!(standard(Some("")), Renderer::Glow);
        assert_eq!(standard(Some("glow")), Renderer::Glow);
        assert_eq!(standard(Some("vulkan")), Renderer::Glow);
        assert_eq!(standard(Some("wgpu")), Renderer::Wgpu);
        assert_eq!(standard(Some(" WGPU ")), Renderer::Wgpu);
    }

    #[test]
    fn ten_bit_starts_with_wgpu_unless_opengl_is_requested() {
        let ten_bit = |v| requested(v, Depth::TenBit);
        assert_eq!(ten_bit(None), Renderer::Wgpu);
        assert_eq!(ten_bit(Some("vulkan")), Renderer::Wgpu);
        assert_eq!(ten_bit(Some("glow")), Renderer::Glow);
    }

    #[test]
    fn restarts_with_wgpu_only_when_opengl_setup_failed() {
        let step = |r, g| next_step(r, Depth::Standard, g, false);
        assert_eq!(step(Renderer::Glow, true), Step::Relaunch(Renderer::Wgpu));
        assert_eq!(step(Renderer::Glow, false), Step::Report);
        assert_eq!(step(Renderer::Wgpu, true), Step::Report);
        assert_eq!(step(Renderer::Wgpu, false), Step::Report);
    }

    #[test]
    fn ten_bit_restarts_with_opengl_when_wgpu_setup_failed() {
        let step = |r, g| next_step(r, Depth::TenBit, g, false);
        assert_eq!(step(Renderer::Wgpu, true), Step::Relaunch(Renderer::Glow));
        assert_eq!(step(Renderer::Wgpu, false), Step::Report);
    }

    #[test]
    fn a_restarted_process_does_not_restart_again() {
        assert_eq!(
            next_step(Renderer::Glow, Depth::TenBit, true, true),
            Step::Report
        );
        assert_eq!(
            next_step(Renderer::Glow, Depth::Standard, true, true),
            Step::Report
        );
    }

    #[test]
    fn only_rgb10a2_and_float16_surfaces_are_deeper_than_8_bits() {
        assert_eq!(format_bits(wgpu::TextureFormat::Rgb10a2Unorm), 10);
        assert_eq!(format_bits(wgpu::TextureFormat::Rgba16Float), 16);
        assert_eq!(format_bits(wgpu::TextureFormat::Bgra8Unorm), 8);
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
