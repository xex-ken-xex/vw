//! 終了時の状態（ウインドウの位置と大きさ、フォルダツリーの開閉）の保存と、起動時の復元。
//!
//! 設定（settings.toml）とは別のファイル（state.toml）にする。設定は利用者が書くもの、
//! 状態はアプリが毎回書き換えるもので、混ぜると利用者が編集中の設定ファイルを
//! 終了のたびに書き換えてしまうため。
//!
//! 位置と大きさは物理ピクセルで持つ。論理ピクセル（DPI で割った値）で持つと、
//! 拡大率の違うモニタをまたいだときに、どのモニタの拡大率で割った値か曖昧になるため。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::sub_window;
use crate::toml_reader::Reader;

/// モニタ 1 台の範囲（物理ピクセル）と拡大率（%）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale_percent: i32,
}

/// 最小化・最大化していないときのウインドウ。
#[derive(Debug, Clone, PartialEq)]
pub struct WindowState {
    /// 外枠の左上（物理ピクセル）。Wayland のように位置を取得できない環境では `None`。
    pub position: Option<[i32; 2]>,
    /// 内側（描画領域）の大きさ（物理ピクセル）。
    pub size: [u32; 2],
    /// 保存時にウインドウがあったモニタの拡大率。物理ピクセルの大きさを論理に戻すのに使う。
    pub scale: f32,
    pub maximized: bool,
    /// Windows で OS が持つ通常の矩形 `[left, top, right, bottom]`（外枠、ワークスペース座標）。
    /// あれば `position` と `size` より優先する（[`crate::window_placement`]）。
    pub normal: Option<[i32; 4]>,
    /// 保存時のモニタ構成。起動時と違えば位置を復元しない。
    pub monitors: Vec<Monitor>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppState {
    pub window: Option<WindowState>,
    /// 開いていたフォルダ。
    pub open_folders: Vec<PathBuf>,
    /// cd していたフォルダ。
    pub current: Option<PathBuf>,
    /// 最後にデータセットを取り出した出力先の親フォルダ。
    pub extract_dir: Option<PathBuf>,
    /// 最後に全画面スライドショーを始めた起点のフォルダ。
    pub fullscreen_root: Option<PathBuf>,
    /// 画素の変換の図で動かしたノードの位置。鍵は `形式.ノード`（例 `gray.voi`）。
    pub pipeline_nodes: BTreeMap<String, [f32; 2]>,
    /// ヒストグラム・ズームマップなどの別ウインドウの位置と「常に最前面」。
    /// 鍵はウインドウの名前（`histogram`、`zoom_map`）。
    pub sub_windows: BTreeMap<String, sub_window::Saved>,
    /// メタデータ欄で閉じていたグループ（英語の名前）。
    pub metadata_closed: Vec<String>,
    /// ペイン（フォルダ・画像・メタデータ・ログ）の並びと、別ウインドウに出したもの。
    pub dock: Option<vw_dock::Layout>,
    /// 画像の左ドラッグを矢印（画像を運ぶ）にしていた。既定は手のひら（表示位置を動かす）。
    pub arrow_tool: bool,
}

/// `[sub_windows.名前]` を読む。読めない項目だけを捨てる（なければ既定の大きさで開くだけ）。
fn read_sub_windows(r: &mut Reader<'_>) -> BTreeMap<String, sub_window::Saved> {
    let Some(section) = r.section("sub_windows") else {
        return BTreeMap::new();
    };
    let mut inner = Reader::new(section);
    let windows = section
        .keys()
        .filter_map(|name| {
            inner.section(name)?;
            let saved = sub_window::Saved {
                rect: inner.get::<[f32; 4]>(name, "rect"),
                on_top: inner.get::<bool>(name, "on_top").unwrap_or(false),
            };
            Some((name.clone(), saved))
        })
        .collect();
    r.problems.extend(
        inner
            .problems
            .into_iter()
            .map(|p| format!("sub_windows.{p}")),
    );
    windows
}

/// `[pipeline_nodes]` を読む。読めない項目だけを捨てる（位置はなくても既定の並びに戻るだけ）。
fn read_pipeline_nodes(r: &mut Reader<'_>) -> BTreeMap<String, [f32; 2]> {
    let Some(section) = r.section("pipeline_nodes") else {
        return BTreeMap::new();
    };
    section
        .keys()
        .filter_map(|key| Some((key.clone(), r.get::<[f32; 2]>("pipeline_nodes", key)?)))
        .collect()
}

/// `[dock]` を読む。読めなければ既定の並びに戻すので、捨てて説明を残す。
fn read_dock(table: &toml::Table, problems: &mut Vec<String>) -> Option<vw_dock::Layout> {
    let value = table.get("dock")?.clone();
    match value.try_into::<vw_dock::Layout>() {
        Ok(layout) => Some(layout),
        Err(e) => {
            problems.push(format!("dock: {e} (the default window layout is used)"));
            None
        }
    }
}

/// 起動時にウインドウをどう置くか。
#[derive(Debug, Clone, PartialEq)]
pub enum Placement {
    /// OS に任せる。
    Default,
    Restore(WindowState),
}

/// 保存されたウインドウを今のモニタ構成で復元してよいかを決める。
/// モニタの数・配置・解像度・拡大率のどれかが変わっていれば OS に任せる。
/// 前回の位置が今は画面外になっていることがあり、それを確かめるより確実なため。
pub fn placement(saved: Option<&WindowState>, current: &[Monitor]) -> Placement {
    let Some(saved) = saved else {
        return Placement::Default;
    };
    let mut a = saved.monitors.clone();
    let mut b = current.to_vec();
    a.sort();
    b.sort();
    if a != b {
        return Placement::Default;
    }
    Placement::Restore(saved.clone())
}

/// 小さすぎるウインドウは操作できないので、これ未満は保存も復元もしない。
const MIN_SIZE: u32 = 200;

impl AppState {
    /// 読めない部分は既定（ウインドウは OS 任せ、ツリーは全部閉じる）にして、その説明を返す。
    /// ウインドウの項目は組で意味を持つので、1 つでも読めなければウインドウ全体を既定にする。
    pub fn from_toml(text: &str) -> (Self, Vec<String>) {
        let table: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(e) => return (Self::default(), vec![format!("{e} (using defaults)")]),
        };
        let mut r = Reader::new(&table);
        let window = read_window(&mut r);
        let open_folders = r.get::<Vec<PathBuf>>("tree", "open").unwrap_or_default();
        let current = r.get::<PathBuf>("tree", "current");
        let extract_dir = r.get::<PathBuf>("dataset", "extract_dir");
        let fullscreen_root = r.get::<PathBuf>("fullscreen", "root");
        let pipeline_nodes = read_pipeline_nodes(&mut r);
        let sub_windows = read_sub_windows(&mut r);
        let metadata_closed = r
            .get::<Vec<String>>("metadata", "closed")
            .unwrap_or_default();
        let dock = read_dock(&table, &mut r.problems);
        let arrow_tool = r.get::<bool>("viewer", "arrow_tool").unwrap_or(false);
        (
            Self {
                window,
                open_folders,
                current,
                extract_dir,
                fullscreen_root,
                pipeline_nodes,
                sub_windows,
                metadata_closed,
                dock,
                arrow_tool,
            },
            r.problems,
        )
    }

    pub fn to_toml(&self) -> String {
        let mut root = toml::Table::new();
        if let Some(w) = &self.window {
            let mut t = toml::Table::new();
            if let Some([x, y]) = w.position {
                t.insert("position".into(), toml::Value::try_from([x, y]).unwrap());
            }
            t.insert("size".into(), toml::Value::try_from(w.size).unwrap());
            t.insert("scale".into(), toml::Value::try_from(w.scale).unwrap());
            t.insert("maximized".into(), w.maximized.into());
            if let Some(normal) = w.normal {
                t.insert("normal".into(), toml::Value::try_from(normal).unwrap());
            }
            let monitors: Vec<[i32; 5]> = w
                .monitors
                .iter()
                .map(|m| [m.x, m.y, m.width, m.height, m.scale_percent])
                .collect();
            t.insert("monitors".into(), toml::Value::try_from(monitors).unwrap());
            root.insert("window".into(), t.into());
        }
        let mut tree = toml::Table::new();
        let open: Vec<String> = self
            .open_folders
            .iter()
            .filter_map(|p| p.to_str().map(str::to_owned))
            .collect();
        tree.insert("open".into(), toml::Value::try_from(open).unwrap());
        if let Some(current) = self.current.as_ref().and_then(|p| p.to_str()) {
            tree.insert("current".into(), current.into());
        }
        root.insert("tree".into(), tree.into());
        if let Some(dir) = self.extract_dir.as_ref().and_then(|p| p.to_str()) {
            let mut dataset = toml::Table::new();
            dataset.insert("extract_dir".into(), dir.into());
            root.insert("dataset".into(), dataset.into());
        }
        if let Some(dir) = self.fullscreen_root.as_ref().and_then(|p| p.to_str()) {
            let mut fullscreen = toml::Table::new();
            fullscreen.insert("root".into(), dir.into());
            root.insert("fullscreen".into(), fullscreen.into());
        }
        if self.arrow_tool {
            let mut viewer = toml::Table::new();
            viewer.insert("arrow_tool".into(), true.into());
            root.insert("viewer".into(), viewer.into());
        }
        if !self.metadata_closed.is_empty() {
            let mut metadata = toml::Table::new();
            metadata.insert(
                "closed".into(),
                toml::Value::try_from(&self.metadata_closed).unwrap(),
            );
            root.insert("metadata".into(), metadata.into());
        }
        if !self.pipeline_nodes.is_empty() {
            let nodes: toml::Table = self
                .pipeline_nodes
                .iter()
                .map(|(k, v)| (k.clone(), toml::Value::try_from(v).unwrap()))
                .collect();
            root.insert("pipeline_nodes".into(), nodes.into());
        }
        if !self.sub_windows.is_empty() {
            let windows: toml::Table = self
                .sub_windows
                .iter()
                .map(|(k, v)| {
                    let mut t = toml::Table::new();
                    if let Some(rect) = v.rect {
                        t.insert("rect".into(), toml::Value::try_from(rect).unwrap());
                    }
                    t.insert("on_top".into(), v.on_top.into());
                    (k.clone(), t.into())
                })
                .collect();
            root.insert("sub_windows".into(), windows.into());
        }
        if let Some(dock) = &self.dock {
            match toml::Value::try_from(dock) {
                Ok(v) => {
                    root.insert("dock".into(), v);
                }
                Err(e) => eprintln!("vw: cannot save the pane layout: {e}"),
            }
        }
        toml::to_string(&root).expect("state is always representable in TOML")
    }

    /// `path` から読む。ファイルがなければ既定（初回起動）。
    pub fn load(path: &Path) -> (Self, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let (state, problems) = Self::from_toml(&text);
                let problems = problems
                    .into_iter()
                    .map(|p| format!("{}: {p}", path.display()))
                    .collect();
                (state, problems)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), Vec::new()),
            Err(e) => (Self::default(), vec![format!("{}: {e}", path.display())]),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_toml())
    }
}

fn read_window(r: &mut Reader<'_>) -> Option<WindowState> {
    r.section("window")?;
    let before = r.problems.len();
    let position = r.get::<[i32; 2]>("window", "position");
    let size = r.get_valid::<[u32; 2]>(
        "window",
        "size",
        |[w, h]| *w >= MIN_SIZE && *h >= MIN_SIZE,
        "must be at least 200 x 200",
    );
    let scale = r.get_valid::<f32>(
        "window",
        "scale",
        |s| s.is_finite() && *s > 0.0,
        "must be positive",
    );
    let maximized = r.get::<bool>("window", "maximized");
    let normal = r.get_valid::<[i32; 4]>(
        "window",
        "normal",
        |[l, t, r, b]| r - l >= MIN_SIZE as i32 && b - t >= MIN_SIZE as i32,
        "must be at least 200 x 200",
    );
    let monitors = r.get::<Vec<[i32; 5]>>("window", "monitors");
    if r.problems.len() > before {
        r.problems
            .push("window: position and size are not restored".into());
        return None;
    }
    Some(WindowState {
        position,
        size: size?,
        scale: scale?,
        maximized: maximized.unwrap_or(false),
        normal,
        monitors: monitors?
            .into_iter()
            .map(|[x, y, width, height, scale_percent]| Monitor {
                x,
                y,
                width,
                height,
                scale_percent,
            })
            .collect(),
    })
}

/// 状態ファイルの場所。設定ファイルと同じフォルダ。
pub fn state_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name("state.toml")
}

/// 今のモニタ構成。取得できなければ空（Wayland だけの環境など）。
/// 前回も空なら構成は同じとみなし、大きさだけを復元する（位置は Wayland では決められない）。
///
/// winit もモニタの一覧を持っているが、eframe 経由では取り出せないので OS に直接問い合わせる。
pub fn current_monitors() -> Vec<Monitor> {
    let mut list = platform::monitors();
    list.sort();
    list
}

#[cfg(windows)]
mod platform {
    use super::Monitor;
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _dc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: data は monitors() が渡した Vec へのポインタで、列挙の間だけ使う。
        let out = unsafe { &mut *(data as *mut Vec<Monitor>) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            let r = info.rcMonitor;
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
            out.push(Monitor {
                x: r.left,
                y: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
                scale_percent: (dpi_x * 100 / 96) as i32,
            });
        }
        1
    }

    /// winit がプロセスを DPI 対応にした後（ウインドウ作成後）に呼ぶこと。
    /// それより前だと Windows が拡大率で割った座標を返し、保存時の値と比べられない。
    pub fn monitors() -> Vec<Monitor> {
        let mut out: Vec<Monitor> = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(collect),
                &mut out as *mut Vec<Monitor> as LPARAM,
            );
        }
        out
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Monitor;
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt;

    /// X11 では拡大率がモニタごとでなく画面全体で 1 つなので、100 として持つ。
    pub fn monitors() -> Vec<Monitor> {
        let Ok((conn, screen)) = x11rb::connect(None) else {
            return Vec::new();
        };
        let root = conn.setup().roots[screen].root;
        let Ok(reply) = conn
            .randr_get_monitors(root, true)
            .map_err(drop)
            .and_then(|c| c.reply().map_err(drop))
        else {
            return Vec::new();
        };
        reply
            .monitors
            .iter()
            .map(|m| Monitor {
                x: m.x.into(),
                y: m.y.into(),
                width: m.width.into(),
                height: m.height.into(),
                scale_percent: 100,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn monitor(x: i32, width: i32) -> Monitor {
        Monitor {
            x,
            y: 0,
            width,
            height: 1080,
            scale_percent: 100,
        }
    }

    fn window(monitors: Vec<Monitor>) -> WindowState {
        WindowState {
            position: Some([2000, 100]),
            size: [1280, 800],
            scale: 1.5,
            maximized: true,
            normal: Some([1900, 80, 3100, 900]),
            monitors,
        }
    }

    #[test]
    fn first_launch_uses_os_placement() {
        assert_eq!(placement(None, &[monitor(0, 1920)]), Placement::Default);
    }

    #[test]
    fn same_monitors_restore_the_window() {
        let saved = window(vec![monitor(0, 1920), monitor(1920, 2560)]);
        // 列挙の順は OS 次第なので、順番が違っても同じ構成とみなす。
        let now = [monitor(1920, 2560), monitor(0, 1920)];
        assert_eq!(
            placement(Some(&saved), &now),
            Placement::Restore(saved.clone())
        );
    }

    #[test]
    fn removed_monitor_falls_back_to_os_placement() {
        let saved = window(vec![monitor(0, 1920), monitor(1920, 2560)]);
        assert_eq!(
            placement(Some(&saved), &[monitor(0, 1920)]),
            Placement::Default
        );
    }

    #[test]
    fn changed_resolution_or_scale_falls_back_to_os_placement() {
        let saved = window(vec![monitor(0, 1920)]);
        assert_eq!(
            placement(Some(&saved), &[monitor(0, 2560)]),
            Placement::Default
        );
        let mut scaled = monitor(0, 1920);
        scaled.scale_percent = 150;
        assert_eq!(placement(Some(&saved), &[scaled]), Placement::Default);
    }

    #[test]
    fn state_round_trips_through_toml() {
        let state = AppState {
            window: Some(window(vec![monitor(0, 1920), monitor(1920, 2560)])),
            open_folders: vec![PathBuf::from("/home/ken"), PathBuf::from("/home/ken/写真")],
            current: Some(PathBuf::from("/home/ken/写真")),
            extract_dir: Some(PathBuf::from("/home/ken/評価")),
            fullscreen_root: Some(PathBuf::from("/home/ken/撮影")),
            pipeline_nodes: [("gray.voi".to_owned(), [560.0, -20.5])].into(),
            sub_windows: [
                (
                    "histogram".to_owned(),
                    sub_window::Saved {
                        rect: Some([10.0, 20.5, 700.0, 400.0]),
                        on_top: true,
                    },
                ),
                (
                    "zoom_map".to_owned(),
                    sub_window::Saved {
                        rect: None,
                        on_top: false,
                    },
                ),
            ]
            .into(),
            metadata_closed: vec!["EXIF".into(), "AI tags".into()],
            dock: Some(saved_dock()),
            arrow_tool: true,
        };
        assert_eq!(AppState::from_toml(&state.to_toml()), (state, vec![]));
    }

    /// ログを別ウインドウに出し、メタデータを隠した並び。
    fn saved_dock() -> vw_dock::Layout {
        use vw_dock::{Axis, Back, FloatSaved, Node, Side, Target};
        vw_dock::Layout {
            main: Some(Node::Split {
                axis: Axis::Row,
                ratio: 0.25,
                first: Box::new(Node::Pane("tree".into())),
                second: Box::new(Node::Pane("images".into())),
            }),
            floats: vec![FloatSaved {
                root: Node::Pane("log".into()),
                rect: [1930.0, 40.5, 600.0, 300.0],
            }],
            hidden: [(
                "metadata".to_owned(),
                Back::Main(Target::beside("images", Side::Right)),
            )]
            .into(),
            docked: [("log".to_owned(), Target::edge(Side::Bottom))].into(),
            floated: [("log".to_owned(), [1930.0, 40.5, 600.0, 300.0])].into(),
        }
    }

    #[test]
    fn broken_pane_layout_falls_back_to_the_default_and_says_so() {
        let (state, problems) = AppState::from_toml("[dock]\nmain = 3\n[tree]\nopen = [\"/a\"]\n");
        assert_eq!(state.dock, None);
        assert_eq!(state.open_folders, vec![PathBuf::from("/a")]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("dock:"), "{problems:?}");
    }

    #[test]
    fn broken_sub_window_is_dropped_alone() {
        let (state, problems) = AppState::from_toml(
            "[sub_windows]\nhistogram = { rect = [1.0, 2.0, 300.0, 200.0], on_top = \"yes\" }\nzoom_map = \"wide\"\n",
        );
        assert_eq!(
            state.sub_windows,
            [(
                "histogram".to_owned(),
                sub_window::Saved {
                    rect: Some([1.0, 2.0, 300.0, 200.0]),
                    on_top: false,
                }
            )]
            .into()
        );
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems.iter().all(|p| p.starts_with("sub_windows.")),
            "{problems:?}"
        );
    }

    #[test]
    fn window_without_position_round_trips() {
        let mut w = window(vec![]);
        w.position = None;
        let state = AppState {
            window: Some(w),
            open_folders: vec![],
            current: None,
            extract_dir: None,
            fullscreen_root: None,
            pipeline_nodes: BTreeMap::new(),
            sub_windows: BTreeMap::new(),
            metadata_closed: Vec::new(),
            arrow_tool: false,
            dock: None,
        };
        assert_eq!(AppState::from_toml(&state.to_toml()), (state, vec![]));
    }

    #[test]
    fn empty_file_means_defaults() {
        assert_eq!(AppState::from_toml(""), (AppState::default(), vec![]));
    }

    #[test]
    fn broken_window_entry_drops_the_whole_window_but_keeps_the_tree() {
        let text = "[window]\nsize = [\"wide\", 800]\nscale = 1.0\nmonitors = []\n[tree]\nopen = [\"/a\"]\n";
        let (state, problems) = AppState::from_toml(text);
        assert_eq!(state.window, None);
        assert_eq!(state.open_folders, [PathBuf::from("/a")]);
        assert!(!problems.is_empty());
    }

    #[test]
    fn tiny_window_is_not_restored() {
        let text = "[window]\nsize = [10, 10]\nscale = 1.0\nmonitors = []\n";
        let (state, problems) = AppState::from_toml(text);
        assert_eq!(state.window, None);
        assert!(!problems.is_empty());
    }

    #[test]
    fn incomplete_window_entry_is_not_restored() {
        let (state, _) = AppState::from_toml("[window]\nsize = [800, 600]\n");
        assert_eq!(state.window, None);
    }

    #[test]
    fn broken_tree_entry_starts_with_everything_closed() {
        let (state, problems) = AppState::from_toml("[tree]\nopen = \"/a\"\n");
        assert!(state.open_folders.is_empty());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn unparsable_file_means_defaults_with_a_note() {
        let (state, problems) = AppState::from_toml("[window");
        assert_eq!(state, AppState::default());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn saved_state_is_loaded_next_time() {
        let dir = TempDir::new();
        let path = dir.path().join("vw").join("state.toml");
        let state = AppState {
            window: None,
            open_folders: vec![dir.path().to_path_buf()],
            current: None,
            extract_dir: None,
            fullscreen_root: None,
            pipeline_nodes: BTreeMap::new(),
            sub_windows: BTreeMap::new(),
            metadata_closed: Vec::new(),
            arrow_tool: false,
            dock: None,
        };
        state.save(&path).unwrap();
        assert_eq!(AppState::load(&path), (state, vec![]));
    }

    #[test]
    fn missing_file_is_a_silent_first_launch() {
        let dir = TempDir::new();
        assert_eq!(
            AppState::load(&dir.path().join("state.toml")),
            (AppState::default(), vec![])
        );
    }

    #[test]
    fn state_file_sits_next_to_settings() {
        let p = state_path(Path::new("/cfg/vw/settings.toml"));
        assert_eq!(p, Path::new("/cfg/vw/state.toml"));
    }
}
