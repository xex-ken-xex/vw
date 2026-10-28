//! つないでいるモニタの一覧と、10bit の表示に関わる情報（ディスプレイへの信号の bit 数など）。
//!
//! winit もモニタの一覧を持っているが、eframe 経由では取り出せず、bit 数も持っていないので
//! OS に直接問い合わせる（[`crate::state::current_monitors`] と同じ理由）。
//!
//! Windows は DisplayConfig（QueryDisplayConfig）で、モニタの名前・見分けるためのパス・
//! 信号の bit 数を読む。DXGI の `IDXGIOutput6::GetDesc1` でも bit 数は読めるが、COM の
//! 準備が要り、名前やパスは別に DisplayConfig で読むことになるので使わない。
//! Linux（X11）は RandR で、モニタの名前は EDID から、ドライバの上限は「max bpc」から読む。

use crate::i18n::{tr, tr1};
use crate::settings::Language;

/// 1 台のモニタ。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Display {
    /// 設定でモニタを見分ける文字列。Windows はデバイスのパス、Linux は端子と EDID の名前。
    pub id: String,
    /// モニタの名前（EDID の名前）。読めなければ端子の名前。
    pub name: String,
    /// 端子（Windows は `\\.\DISPLAY1`、Linux は `DP-1` など）。同じ機種が 2 台あるときの見分けに出す。
    pub connector: String,
    /// 物理ピクセルでの位置と大きさ `[x, y, 幅, 高さ]`。
    pub rect: [i32; 4],
    pub scale_percent: i32,
    pub primary: bool,
    pub refresh_hz: Option<f32>,
    /// GPU がディスプレイへ送っている 1 色あたりの bit 数（Windows）。
    pub signal_bits: Option<u32>,
    /// HDR（Windows の「HDR を使用する」）が入っているか。
    pub hdr: Option<bool>,
    /// EDID に書かれた、ディスプレイが受けられる 1 色あたりの bit 数（Linux）。
    pub edid_bits: Option<u32>,
    /// ドライバが送る bit 数の上限（Linux の「max bpc」）。
    pub max_bpc: Option<u32>,
}

impl Display {
    /// 画面に出す名前。同じ名前のモニタが並んでも分かるように端子を添える。
    pub fn label(&self) -> String {
        if self.connector.is_empty() || self.connector == self.name {
            self.name.clone()
        } else {
            format!(
                "{} ({})",
                self.name,
                self.connector.trim_start_matches(r"\\.\")
            )
        }
    }

    fn contains(&self, [x, y]: [i32; 2]) -> bool {
        let [left, top, w, h] = self.rect;
        (left..left + w).contains(&x) && (top..top + h).contains(&y)
    }

    fn distance(&self, [x, y]: [i32; 2]) -> i64 {
        let [left, top, w, h] = self.rect;
        let dx = (i64::from(left) - i64::from(x))
            .max(i64::from(x) - i64::from(left + w))
            .max(0);
        let dy = (i64::from(top) - i64::from(y))
            .max(i64::from(y) - i64::from(top + h))
            .max(0);
        dx * dx + dy * dy
    }
}

/// モニタの名前に添える、10bit の表示に関わる短い説明（「信号 10bit」など）。分からなければ空。
pub fn signal_text(d: &Display, lang: Language) -> String {
    let mut parts = Vec::new();
    if let Some(bits) = d.signal_bits {
        parts.push(tr1(lang, "signal {}-bit", &bits.to_string()));
    }
    if d.hdr == Some(true) {
        parts.push(tr(lang, "HDR on").to_owned());
    }
    if let Some(bits) = d.edid_bits {
        parts.push(tr1(lang, "accepts {}-bit (EDID)", &bits.to_string()));
    }
    if let Some(bits) = d.max_bpc {
        parts.push(tr1(lang, "max bpc {}", &bits.to_string()));
    }
    parts.join(", ")
}

/// 画面全体の情報（X11 の色の深さ）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct System {
    pub displays: Vec<Display>,
    /// X11 の画面の色の深さ（24 か 30）。10bit で出すには 30 にしておく必要がある。
    pub x11_depth: Option<u8>,
}

/// 今のモニタの一覧。読めなければ空（Wayland だけの環境など）。
pub fn system() -> System {
    let mut system = platform::system();
    system.displays.sort_by_key(|d| (d.rect[0], d.rect[1]));
    system
}

/// 読み直す間隔。モニタをつなぎ替えても数秒で追いつけば足り、毎フレーム OS に問い合わせるほどではない
/// （X11 では問い合わせのたびに接続を作る）。
const REFRESH: std::time::Duration = std::time::Duration::from_secs(5);

/// [`system`] を、[`REFRESH`] の間は覚えておいて返す。設定の画面と画像の欄の両方から引くため、
/// 1 か所に覚える。
pub fn cached() -> System {
    use std::sync::Mutex;
    use std::time::Instant;
    static CACHE: Mutex<Option<(Instant, System)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match &*cache {
        Some((at, system)) if at.elapsed() < REFRESH => system.clone(),
        _ => {
            let system = system();
            *cache = Some((Instant::now(), system.clone()));
            system
        }
    }
}

/// 物理ピクセルの点 `point` があるモニタ。どのモニタにもなければ、いちばん近いモニタ。
pub fn at(displays: &[Display], point: [i32; 2]) -> Option<&Display> {
    displays
        .iter()
        .find(|d| d.contains(point))
        .or_else(|| displays.iter().min_by_key(|d| d.distance(point)))
}

/// EDID（ディスプレイが名乗る 128 バイトの情報）から、モニタの名前を読む。
#[cfg_attr(windows, allow(dead_code))]
pub fn edid_name(edid: &[u8]) -> Option<String> {
    // 18 バイトの記述子が 4 つ並び、頭が 0 0 0 0xFC のものが名前（最大 13 文字、改行で終わる）。
    edid.get(54..126)?
        .as_chunks::<18>()
        .0
        .iter()
        .find(|d| d[..4] == [0, 0, 0, 0xFC])
        .map(|d| {
            let text: String = d[5..18]
                .iter()
                .take_while(|&&b| b != b'\n')
                .map(|&b| b as char)
                .collect();
            text.trim().to_owned()
        })
        .filter(|name| !name.is_empty())
}

/// EDID 1.4 の、デジタル入力の 1 色あたりの bit 数。1.3 以前と、書いていないものは `None`。
#[cfg_attr(windows, allow(dead_code))]
pub fn edid_bits(edid: &[u8]) -> Option<u32> {
    let (&version, &revision, &input) = (edid.get(18)?, edid.get(19)?, edid.get(20)?);
    if version != 1 || revision < 4 || input & 0x80 == 0 {
        return None;
    }
    match (input >> 4) & 0x07 {
        n @ 1..=6 => Some(4 + 2 * u32::from(n)),
        _ => None,
    }
}

#[cfg(windows)]
mod platform {
    use super::{Display, System};
    use std::collections::HashMap;
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Devices::Display::{
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
        DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
        DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
    };
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

    const MONITORINFOF_PRIMARY: u32 = 1;

    fn text(wide: &[u16]) -> String {
        let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
        String::from_utf16_lossy(&wide[..end])
    }

    /// DisplayConfig から読む、1 つの出力の情報。
    struct Target {
        name: String,
        path: String,
        refresh_hz: Option<f32>,
        signal_bits: Option<u32>,
        hdr: Option<bool>,
    }

    fn header<T>(
        kind: i32,
        path: &DISPLAYCONFIG_PATH_INFO,
        target: bool,
    ) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
        let (adapter, id) = if target {
            (path.targetInfo.adapterId, path.targetInfo.id)
        } else {
            (path.sourceInfo.adapterId, path.sourceInfo.id)
        };
        DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: kind,
            size: std::mem::size_of::<T>() as u32,
            adapterId: adapter,
            id,
        }
    }

    /// GDI のデバイス名（`\\.\DISPLAY1`）ごとの出力。
    fn targets() -> HashMap<String, Target> {
        let mut out = HashMap::new();
        let (mut paths, mut modes) = (0u32, 0u32);
        if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) }
            != ERROR_SUCCESS
        {
            return out;
        }
        let mut path_list = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
        let mut mode_list = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        let ok = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut paths,
                path_list.as_mut_ptr(),
                &mut modes,
                mode_list.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if ok != ERROR_SUCCESS {
            return out;
        }
        for path in &path_list[..paths as usize] {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: header::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(
                    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    path,
                    false,
                ),
                ..Default::default()
            };
            if unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } != 0 {
                continue;
            }
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                header: header::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(
                    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                    path,
                    true,
                ),
                ..Default::default()
            };
            let named = unsafe { DisplayConfigGetDeviceInfo(&mut target.header) } == 0;
            let mut color = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
                header: header::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>(
                    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                    path,
                    true,
                ),
                ..Default::default()
            };
            // Windows 10 1709 より前は読めない。
            let colored = unsafe { DisplayConfigGetDeviceInfo(&mut color.header) } == 0;
            let rate = path.targetInfo.refreshRate;
            let gdi = text(&source.viewGdiDeviceName);
            // 複製表示では 1 つの GDI の名前に出力が複数ある。最初の 1 つを使う。
            out.entry(gdi).or_insert_with(|| Target {
                name: if named {
                    text(&target.monitorFriendlyDeviceName)
                } else {
                    String::new()
                },
                path: if named {
                    text(&target.monitorDevicePath)
                } else {
                    String::new()
                },
                refresh_hz: (rate.Denominator != 0)
                    .then(|| rate.Numerator as f32 / rate.Denominator as f32),
                signal_bits: colored
                    .then_some(color.bitsPerColorChannel)
                    .filter(|&b| b > 0),
                // 2 番目の bit が「HDR が入っている」（advancedColorEnabled）。
                hdr: colored.then_some(unsafe { color.Anonymous.value } & 0b10 != 0),
            });
        }
        out
    }

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _dc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: data は system() が渡した Vec へのポインタで、列挙の間だけ使う。
        let out = unsafe { &mut *(data as *mut Vec<(String, Display)>) };
        let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe {
            GetMonitorInfoW(
                monitor,
                &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
            )
        } != 0
        {
            let r = info.monitorInfo.rcMonitor;
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
            let gdi = text(&info.szDevice);
            out.push((
                gdi.clone(),
                Display {
                    connector: gdi,
                    rect: [r.left, r.top, r.right - r.left, r.bottom - r.top],
                    scale_percent: (dpi_x * 100 / 96) as i32,
                    primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                    ..Default::default()
                },
            ));
        }
        1
    }

    pub fn system() -> System {
        let mut monitors: Vec<(String, Display)> = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(collect),
                &mut monitors as *mut Vec<(String, Display)> as LPARAM,
            );
        }
        let mut targets = targets();
        let displays = monitors
            .into_iter()
            .map(|(gdi, mut d)| {
                if let Some(t) = targets.remove(&gdi) {
                    d.refresh_hz = t.refresh_hz;
                    d.signal_bits = t.signal_bits;
                    d.hdr = t.hdr;
                    d.name = t.name;
                    d.id = t.path;
                }
                if d.name.is_empty() {
                    d.name = gdi.trim_start_matches(r"\\.\").to_owned();
                }
                if d.id.is_empty() {
                    d.id = gdi;
                }
                d
            })
            .collect();
        System {
            displays,
            x11_depth: None,
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{edid_bits, edid_name, Display, System};
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::{ConnectionExt as _, Output};
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

    pub fn system() -> System {
        let Ok((conn, screen)) = x11rb::connect(None) else {
            return System::default();
        };
        let root_screen = &conn.setup().roots[screen];
        let root = root_screen.root;
        let depth = root_screen.root_depth;
        let atom = |name: &str| {
            conn.intern_atom(true, name.as_bytes())
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.atom)
                .filter(|&a| a != 0)
        };
        let (edid_atom, bpc_atom) = (atom("EDID"), atom("max bpc"));
        let property = |output: Output, atom: Option<u32>| -> Option<Vec<u8>> {
            let reply = conn
                .randr_get_output_property(output, atom?, AtomEnum::ANY, 0, 256, false, false)
                .ok()?
                .reply()
                .ok()?;
            (!reply.data.is_empty()).then_some(reply.data)
        };
        let resources = conn
            .randr_get_screen_resources_current(root)
            .ok()
            .and_then(|c| c.reply().ok());
        let refresh = |output: Output| -> Option<f32> {
            let resources = resources.as_ref()?;
            let info = conn
                .randr_get_output_info(output, resources.config_timestamp)
                .ok()?
                .reply()
                .ok()?;
            let crtc = conn
                .randr_get_crtc_info(info.crtc, resources.config_timestamp)
                .ok()?
                .reply()
                .ok()?;
            let mode = resources.modes.iter().find(|m| m.id == crtc.mode)?;
            let dots = f32::from(mode.htotal) * f32::from(mode.vtotal);
            (dots > 0.0).then(|| mode.dot_clock as f32 / dots)
        };
        let Ok(reply) = conn
            .randr_get_monitors(root, true)
            .map_err(drop)
            .and_then(|c| c.reply().map_err(drop))
        else {
            return System {
                displays: Vec::new(),
                x11_depth: Some(depth),
            };
        };
        let displays = reply
            .monitors
            .iter()
            .map(|m| {
                let connector = conn
                    .get_atom_name(m.name)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                    .unwrap_or_default();
                let output = m.outputs.first().copied();
                let edid = output.and_then(|o| property(o, edid_atom));
                let name = edid
                    .as_deref()
                    .and_then(edid_name)
                    .unwrap_or_else(|| connector.clone());
                Display {
                    id: format!("{connector}/{name}"),
                    name,
                    connector,
                    rect: [m.x.into(), m.y.into(), m.width.into(), m.height.into()],
                    // X11 では拡大率がモニタごとでなく画面全体で 1 つ。
                    scale_percent: 100,
                    primary: m.primary,
                    refresh_hz: output.and_then(refresh),
                    signal_bits: None,
                    hdr: None,
                    edid_bits: edid.as_deref().and_then(edid_bits),
                    max_bpc: output
                        .and_then(|o| property(o, bpc_atom))
                        .and_then(|d| Some(u32::from_ne_bytes(d.get(..4)?.try_into().ok()?))),
                }
            })
            .collect();
        System {
            displays,
            x11_depth: Some(depth),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(x: i32, width: i32) -> Display {
        Display {
            id: format!("d{x}"),
            rect: [x, 0, width, 1080],
            ..Default::default()
        }
    }

    #[test]
    fn a_point_belongs_to_the_monitor_that_contains_it() {
        let list = [display(0, 1920), display(1920, 2560)];
        assert_eq!(at(&list, [100, 500]).unwrap().id, "d0");
        assert_eq!(at(&list, [1920, 0]).unwrap().id, "d1920");
    }

    #[test]
    fn a_point_outside_every_monitor_belongs_to_the_nearest() {
        let list = [display(0, 1920), display(1920, 2560)];
        assert_eq!(at(&list, [5000, 500]).unwrap().id, "d1920");
        assert_eq!(at(&list, [-30, -30]).unwrap().id, "d0");
        assert!(at(&[], [0, 0]).is_none());
    }

    #[test]
    fn two_monitors_of_the_same_model_are_told_apart_by_the_connector() {
        let d = Display {
            name: "RX660".into(),
            connector: r"\\.\DISPLAY2".into(),
            ..Default::default()
        };
        assert_eq!(d.label(), "RX660 (DISPLAY2)");
    }

    fn edid(version: (u8, u8), input: u8, name: Option<&str>) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
        (e[18], e[19], e[20]) = (version.0, version.1, input);
        if let Some(name) = name {
            let d = &mut e[72..90];
            d[..5].copy_from_slice(&[0, 0, 0, 0xFC, 0]);
            let mut text = name.as_bytes().to_vec();
            text.push(b'\n');
            text.resize(13, b' ');
            d[5..18].copy_from_slice(&text);
        }
        e
    }

    #[test]
    fn edid_names_the_monitor_up_to_the_line_break() {
        assert_eq!(
            edid_name(&edid((1, 4), 0xB5, Some("RX660"))).as_deref(),
            Some("RX660")
        );
        assert_eq!(edid_name(&edid((1, 4), 0xB5, None)), None);
        assert_eq!(edid_name(&[0; 20]), None);
    }

    #[test]
    fn signal_text_lists_only_what_is_known() {
        let d = Display {
            signal_bits: Some(10),
            hdr: Some(true),
            edid_bits: Some(12),
            max_bpc: Some(16),
            ..Default::default()
        };
        assert_eq!(
            signal_text(&d, Language::English),
            "signal 10-bit, HDR on, accepts 12-bit (EDID), max bpc 16"
        );
        let off = Display {
            hdr: Some(false),
            ..Default::default()
        };
        assert_eq!(signal_text(&off, Language::English), "");
    }

    #[test]
    fn edid_1_4_digital_input_tells_the_bits_per_color() {
        // 0b1011_0101: デジタル入力、10bit、DisplayPort。
        assert_eq!(edid_bits(&edid((1, 4), 0b1011_0101, None)), Some(10));
        assert_eq!(edid_bits(&edid((1, 4), 0b1100_0101, None)), Some(12));
        assert_eq!(edid_bits(&edid((1, 4), 0b1010_0101, None)), Some(8));
        // 書いていない、1.3、アナログ入力。
        assert_eq!(edid_bits(&edid((1, 4), 0b1000_0101, None)), None);
        assert_eq!(edid_bits(&edid((1, 3), 0b1011_0101, None)), None);
        assert_eq!(edid_bits(&edid((1, 4), 0b0011_0101, None)), None);
    }
}
