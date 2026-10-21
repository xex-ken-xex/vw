//! 画面。見た目は vw と同じ `vw-ui`(配色・メニュー・パネル見出し)で、
//! パネルの並びも vw に合わせる: メニュー → 道具の帯 → ログ(下)→ 設定(左)→ 構成とプレビュー(右)→ タグの規則(中央)。

use crate::charset::Charset;
use crate::config::{
    self, Config, CsvEncoding, Destination, Level, RuleSpec, UidMode, DEFAULT_RULES,
};
use crate::engine::{
    self, src_vars_for, tag_of, vars_for, Engine, GroupResult, Idx, OutputSummary, PreviewRow,
    Progress, TableCache, WorkerStatus, NOW_INT_VARS, NOW_STR_VARS,
};
use crate::tree::{self, Counts, Place, SourceTree};
use anyhow::Result;
use dicom_dictionary_std::tags;
use eframe::egui::{self, Color32, RichText, ThemePreference};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;
use vw_tree::picker::{self, Picker};

/// 「タグを選んで足す」に出す、そのレベルの典型的なタグ。辞書にあるどの名前でも書ける。
fn typical_tags(level: Level) -> &'static [&'static str] {
    match level {
        Level::Patient => &[
            "PatientName",
            "PatientID",
            "PatientSex",
            "PatientBirthDate",
            "PatientAge",
            "PatientWeight",
            "PatientSize",
            "OtherPatientIDs",
            "PatientComments",
        ],
        Level::Study => &[
            "StudyDescription",
            "StudyDate",
            "StudyTime",
            "StudyID",
            "AccessionNumber",
            "ReferringPhysicianName",
            "InstitutionName",
            "PatientAge",
            "PatientWeight",
            "PatientSize",
        ],
        Level::Series => &[
            "SeriesDescription",
            "SeriesNumber",
            "SeriesDate",
            "SeriesTime",
            "Modality",
            "BodyPartExamined",
            "ProtocolName",
            "OperatorsName",
            "StationName",
            "Manufacturer",
        ],
        Level::Image => &[
            "InstanceNumber",
            "ImageComments",
            "AcquisitionDate",
            "AcquisitionTime",
            "ContentDate",
            "ContentTime",
        ],
    }
}

/// レベルの説明(規則の節の見出しの下)。
fn level_note(level: Level) -> &'static str {
    match level {
        Level::Patient => "患者ごとに 1 つの値。同じ患者の全画像で同じになる",
        Level::Study => "検査ごとに 1 つの値。検査日・検査の説明・受付番号など",
        Level::Series => "シリーズごとに 1 つの値。シリーズの説明・部位・プロトコルなど",
        Level::Image => "画像ごとに 1 つの値。画像番号など",
    }
}

/// 既定の規則にあるタグなら、その値(「タグを選んで足す」で最初から入れておく)。
fn default_value(tag: &str) -> &'static str {
    DEFAULT_RULES
        .iter()
        .find(|(t, _, _)| *t == tag)
        .map_or("", |(_, _, v)| v)
}

/// そのレベルの既定の規則のうち、いまの規則にないタグ。
/// 規則は (0010,0010) や /VR つきでも書けるので、文字ではなくタグで比べる
fn missing_defaults(rules: &[RuleSpec], level: Level) -> Vec<&'static str> {
    DEFAULT_RULES
        .iter()
        .filter(|(t, l, _)| *l == level && !rules.iter().any(|r| tag_of(&r.tag) == tag_of(t)))
        .map(|(t, _, _)| *t)
        .collect()
}

/// 変数の説明(変数名に乗せたときの吹き出し)。
fn var_help(name: &str) -> &'static str {
    match name {
        "p" => "患者の複製番号(0 始まり)。元の各患者を「患者」の数だけ複製した、その何回目か",
        "p1" => "患者の複製番号(1 始まり)",
        "P" => "元の木での患者の番号(0 始まり)。元の患者が複数いるとき、どの患者から複製したかを区別する",
        "s" => "検査の複製番号(0 始まり)。元の各検査を「患者あたりの検査」の数だけ複製した、その何回目か",
        "s1" => "検査の複製番号(1 始まり)",
        "S" => "元の木での検査の番号(その患者の中で 0 始まり)",
        "r" => "シリーズの複製番号(0 始まり)",
        "r1" => "シリーズの複製番号(1 始まり)",
        "R" => "元の木でのシリーズの番号(その検査の中で 0 始まり)",
        "i" => "画像の複製番号(0 始まり)。元の各画像を「シリーズあたりの画像」の数だけ複製した、その何回目か。元の何枚目かではない",
        "i1" => "画像の複製番号(1 始まり)",
        "I" => "元の木での画像の番号(そのシリーズの中で InstanceNumber 順に 0 始まり)",
        "n" => "通し番号(1 始まり)。元の画像 → 患者 → 検査 → シリーズ → 画像の複製の順に振った、全出力で一意な番号",
        "inst" => "元の InstanceNumber + 画像の複製番号 × 10000。元の画像どうしの区別はこれが引き受ける",
        "src_patient_id" => "元ファイルの PatientID",
        "src_patient_name" => "元ファイルの PatientName",
        "src_patient_birth_date" => "元ファイルの PatientBirthDate",
        "src_patient_sex" => "元ファイルの PatientSex",
        "src_study_id" => "元ファイルの StudyID",
        "src_study_description" => "元ファイルの StudyDescription",
        "src_series_number" => "元ファイルの SeriesNumber",
        "src_series_description" => "元ファイルの SeriesDescription",
        "src_modality" => "元ファイルの Modality",
        "now_year" => "生成を実行した年(例 2026)。実行の始めに 1 回取り、全ファイルで同じ",
        "now_month" => "生成を実行した月(1〜12)。2 桁にするなら %(now_month)02d",
        "now_day" => "生成を実行した日(1〜31)。2 桁にするなら %(now_day)02d",
        "now_hour" => "生成を実行した時(0〜23)",
        "now_minute" => "生成を実行した分(0〜59)",
        "now_second" => "生成を実行した秒(0〜59)",
        "now_date" => "生成を実行した日付 YYYYMMDD(例 20261021)。StudyDate などに",
        "now_time" => "生成を実行した時刻 HHMMSS(例 140509)。StudyTime などに",
        _ => "CSV の列。タグのレベルの単位で 1 行ずつ進む(行数を超えたら先頭へ戻る)",
    }
}

const WARN: Color32 = Color32::from_rgb(0xe0, 0xa0, 0x40);
const ERROR: Color32 = Color32::from_rgb(0xe0, 0x60, 0x60);
const OK: Color32 = Color32::from_rgb(0x6a, 0xc0, 0x7a);

/// 規則の行のタグの欄の幅。既定の規則の行とそろえるため、1 か所で決める。
const TAG_W: f32 = 200.0;

/// 木に出す患者・複製の上限。それより多い分は「…ほか N」にまとめる。
const TREE_LIMIT: usize = 3;
const SOURCE_LIMIT: usize = 50;

pub fn run(dir: PathBuf, initial: Option<Config>, preset: Option<String>) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DicoAmplio")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "DicoAmplio",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, dir, initial, preset)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

struct Job {
    progress: Arc<Progress>,
    done: Receiver<Result<u64, String>>,
    logged: usize,
    started: std::time::Instant,
}

/// 元データの木を裏で読んでいる最中。
struct Scan {
    rx: Receiver<SourceTree>,
    done: Arc<AtomicU64>,
    total: usize,
    cancel: Arc<AtomicBool>,
}

/// 出力の前に、出力先の中身を見せて尋ねる確認の段。
enum CleanDialog {
    /// 出力先が空(ないときも)。そう見せて、出力するか尋ねる
    Empty,
    /// 中身がある。消すか尋ねる。消してはいけない出力先なら、その理由
    Ask(Result<OutputSummary, String>),
    /// 「消す」を選んだあと。消す中身を一覧で見せて、もう一度確かめる
    Confirm(OutputSummary),
}

/// 保存の確認。同じ名前があるときは、もう一度押してもらう。
struct SaveDialog {
    name: String,
}

/// 右のパネルのタブ。
#[derive(Clone, Copy, PartialEq, Eq)]
enum RightTab {
    Preview,
    Send,
}

/// 複製後の木で選んでいる元のノード(患者・検査・シリーズの番号)。
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Selection {
    patient: usize,
    study: usize,
    series: usize,
}

struct App {
    cfg: Config,
    preset_dir: PathBuf,
    presets: Vec<String>,
    current_preset: Option<String>,
    save_dialog: Option<SaveDialog>,
    /// 出力の前の確認。Some のあいだ、出力先の中身を見せて尋ねている
    clean_dialog: Option<CleanDialog>,
    theme: ThemePreference,

    /// Arc なのは、実行のスレッドに渡すときに作り直さずに済ませるため
    engine: Result<Arc<Engine>, String>,
    /// engine を作ったときの設定(入出力先など、engine に関わらない欄は空にしてある)
    engine_cfg: Config,
    table_cache: TableCache,
    /// 元のディレクトリのファイル数(歩いて数えたもの。木を読む前から分かる)
    sources: Option<usize>,
    counted_input: String,
    tree: Option<Arc<SourceTree>>,
    scan: Option<Scan>,
    sample: Idx,
    selection: Selection,
    /// CSV の欄を開いている規則(cfg.rules の番号)
    csv_open: HashSet<usize>,

    job: Option<Job>,
    /// C-ECHO を裏で試している最中
    echo: Option<Receiver<Result<String, String>>>,
    log: Vec<String>,

    tab: RightTab,
    /// 送り先の帳簿(presets/destinations.toml)
    destinations: Vec<Destination>,
    /// 帳簿に保存するときの名前
    dest_name: String,
    /// 開いている「選ぶ窓」と、選んだ結果の入れ先
    picker: Option<(PickTarget, Picker)>,
    about: crate::about::About,
}

/// 「参照」で選んだパスの入れ先。
#[derive(Clone, Copy, PartialEq, Eq)]
enum PickTarget {
    Input,
    Output,
    AsIsDir,
    Csv,
    RuleCsv(usize),
    OpenPreset,
    SavePreset,
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        preset_dir: PathBuf,
        initial: Option<Config>,
        preset: Option<String>,
    ) -> Self {
        crate::fonts::load_cjk_in_background(&cc.egui_ctx);
        let theme = ThemePreference::System;
        vw_ui::theme::apply(&cc.egui_ctx, theme, true, 1.0);
        let last = config::preset_path(&preset_dir, config::LAST_NAME)
            .ok()
            .and_then(|p| Config::load(&p).ok());
        let mut cfg = initial.or(last).unwrap_or_default();
        for n in [
            &mut cfg.patients,
            &mut cfg.studies,
            &mut cfg.series,
            &mut cfg.images,
        ] {
            *n = (*n).max(1);
        }
        let mut table_cache = TableCache::new();
        let mut app = App {
            engine: Engine::with_cache(&cfg, &mut table_cache)
                .map_err(|e| format!("{e:#}"))
                .map(Arc::new),
            engine_cfg: Self::engine_key(&cfg),
            table_cache,
            cfg,
            presets: config::list_presets(&preset_dir),
            preset_dir,
            current_preset: preset,
            save_dialog: None,
            clean_dialog: None,
            theme,
            sources: None,
            counted_input: String::new(),
            tree: None,
            scan: None,
            sample: Idx {
                p: 0,
                s: 0,
                r: 0,
                i: 0,
            },
            selection: Selection::default(),
            csv_open: HashSet::new(),
            job: None,
            echo: None,
            log: vec![
                "起動した。元のディレクトリは、窓にフォルダをドロップしても指定できる".into(),
            ],
            tab: RightTab::Preview,
            destinations: vec![],
            dest_name: String::new(),
            picker: None,
            about: Default::default(),
        };
        match config::load_destinations(&app.preset_dir) {
            Ok(d) => app.destinations = d,
            Err(e) => app.log.push(format!("エラー: {e:#}")),
        }
        if app.cfg.store.enabled {
            app.tab = RightTab::Send;
        }
        app.count_sources(&cc.egui_ctx);
        app
    }

    fn note(&mut self, msg: impl Into<String>) {
        self.log.push(msg.into());
    }

    /// engine の中身を変える欄だけを残した設定。出力先や UID の接頭辞、送り先を打つたびに作り直さないため。
    fn engine_key(cfg: &Config) -> Config {
        Config {
            input: String::new(),
            output: String::new(),
            uid_root: String::new(),
            store: Default::default(),
            ..cfg.clone()
        }
    }

    fn writes_files(&self) -> bool {
        !self.cfg.store.enabled || self.cfg.store.also_write
    }

    fn refresh_engine(&mut self) {
        let key = Self::engine_key(&self.cfg);
        if key != self.engine_cfg {
            self.engine = Engine::with_cache(&self.cfg, &mut self.table_cache)
                .map_err(|e| format!("{e:#}"))
                .map(Arc::new);
            self.engine_cfg = key;
        }
    }

    /// 入力欄を打っている間に歩き回らないよう、確定したときにだけ数える。数えたら木も裏で読み始める。
    fn count_sources(&mut self, ctx: &egui::Context) {
        self.counted_input = self.cfg.input.clone();
        if let Some(s) = &self.scan {
            s.cancel.store(true, Ordering::Relaxed);
        }
        self.scan = None;
        self.tree = None;
        self.selection = Selection::default();
        let Ok(files) = engine::list_sources(&self.cfg.input) else {
            self.sources = None;
            return;
        };
        self.sources = Some(files.len());
        let done = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (d, c, ctx) = (done.clone(), cancel.clone(), ctx.clone());
        let total = files.len();
        std::thread::spawn(move || {
            let t = tree::scan(&files, &c, &d);
            if !c.load(Ordering::Relaxed) {
                let _ = tx.send(t);
            }
            ctx.request_repaint();
        });
        self.scan = Some(Scan {
            rx,
            done,
            total,
            cancel,
        });
    }

    fn poll_echo(&mut self) {
        let Some(rx) = &self.echo else { return };
        match rx.try_recv() {
            Ok(Ok(msg)) => {
                self.log.push(msg);
                self.echo = None;
            }
            Ok(Err(e)) => {
                self.log.push(format!("エラー: {e}"));
                self.echo = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.echo = None,
        }
    }

    fn poll_scan(&mut self) {
        let Some(s) = &self.scan else { return };
        match s.rx.try_recv() {
            Ok(t) => {
                tree::log_skipped(&t.skipped, |m| self.log.push(m));
                tree::log_duplicates(&t.duplicates, |m| self.log.push(m));
                // 「元 N 個」は読めたファイルの数にする(DICOM でないものや DICOMDIR を除く)
                self.sources = Some(t.files);
                self.tree = Some(Arc::new(t));
                self.scan = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.scan = None,
        }
    }

    fn total(&self) -> Option<u64> {
        let c = &self.cfg;
        self.sources.map(|n| {
            n as u64 * c.patients as u64 * c.studies as u64 * c.series as u64 * c.images as u64
        })
    }

    /// 選んでいる元のノード(シリーズ)の位置。木をまだ読めていなければ先頭。
    fn current_place(&self) -> Place {
        let s = self.selection;
        self.tree
            .as_ref()
            .and_then(|t| t.place_of(s.patient, s.study, s.series))
            .unwrap_or_default()
    }

    fn save_last(&self) {
        if let Ok(p) = config::preset_path(&self.preset_dir, config::LAST_NAME) {
            let _ = self.cfg.save(&p);
        }
    }

    fn load_preset(&mut self, name: &str, ctx: &egui::Context) {
        let result = config::preset_path(&self.preset_dir, name).and_then(|p| Config::load(&p));
        match result {
            Ok(c) => {
                self.cfg = c;
                self.current_preset = Some(name.to_string());
                self.refresh_engine();
                self.count_sources(ctx);
                self.note(format!("プリセット「{name}」を読んだ"));
            }
            Err(e) => self.note(format!("エラー: {e:#}")),
        }
    }

    fn save_preset(&mut self, name: &str) {
        let result =
            config::preset_path(&self.preset_dir, name).and_then(|p| self.cfg.save(&p).map(|_| p));
        match result {
            Ok(p) => {
                self.current_preset = Some(name.trim().to_string());
                self.presets = config::list_presets(&self.preset_dir);
                self.note(format!("プリセットを保存した: {}", p.display()));
            }
            Err(e) => self.note(format!("エラー: {e:#}")),
        }
    }

    fn delete_preset(&mut self, name: &str) {
        let result =
            config::preset_path(&self.preset_dir, name).and_then(|p| Ok(std::fs::remove_file(p)?));
        match result {
            Ok(()) => {
                if self.current_preset.as_deref() == Some(name) {
                    self.current_preset = None;
                }
                self.presets = config::list_presets(&self.preset_dir);
                self.note(format!("プリセット「{name}」を消した"));
            }
            Err(e) => self.note(format!("エラー: {e:#}")),
        }
    }

    /// 実行の前に分かる問題。実行ボタンを押せなくする理由にもなる。
    fn blocking_problem(&self) -> Option<String> {
        let as_is = self.cfg.store.enabled && self.cfg.store.as_is;
        if as_is {
            if self.cfg.store.as_is_dir.trim().is_empty() {
                return Some("そのまま送るディレクトリを指定してほしい".into());
            }
            if !Path::new(self.cfg.store.as_is_dir.trim()).is_dir() {
                return Some("そのまま送るディレクトリがない".into());
            }
        } else {
            if self.cfg.input.trim().is_empty() {
                return Some("元のディレクトリを指定してほしい".into());
            }
            if !Path::new(self.cfg.input.trim()).is_dir() {
                return Some("元のディレクトリがない".into());
            }
        }
        if !as_is && self.writes_files() {
            if self.cfg.output.trim().is_empty() {
                return Some("出力先を指定してほしい".into());
            }
            if Path::new(self.cfg.output.trim()) == Path::new(self.cfg.input.trim()) {
                return Some("出力先が元のディレクトリと同じ".into());
            }
        }
        if self.cfg.store.enabled {
            let s = &self.cfg.store;
            if s.host.trim().is_empty() || s.port == 0 {
                return Some("送り先のホストとポートを指定してほしい".into());
            }
            if s.called_ae.trim().is_empty()
                || s.called_ae.trim().len() > 16
                || s.calling_ae.trim().len() > 16
            {
                return Some("AE タイトルは 1〜16 文字".into());
            }
        }
        if let Err(e) = &self.engine {
            return Some(e.clone());
        }
        None
    }

    fn start(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        self.refresh_engine();
        if let Some(p) = self.blocking_problem() {
            self.note(format!("エラー: {p}"));
            return;
        }
        let as_is = self.cfg.store.enabled && self.cfg.store.as_is;
        if !as_is && self.writes_files() {
            let output = &self.cfg.output;
            self.clean_dialog = Some(if engine::output_has_entries(output) {
                CleanDialog::Ask(
                    engine::output_summary(&self.cfg.input, output).map_err(|e| format!("{e:#}")),
                )
            } else {
                CleanDialog::Empty
            });
            return;
        }
        self.launch(ctx, false);
    }

    /// 実行を裏のスレッドで始める。`clean` なら、先に出力先の中身を消す。
    fn launch(&mut self, ctx: &egui::Context, clean: bool) {
        // 元のディレクトリと出力先は engine_key に入れていないので、ここで最新の値で作り直す
        let engine = match Engine::with_cache(&self.cfg, &mut self.table_cache) {
            Ok(e) => Arc::new(e),
            Err(e) => {
                self.note(format!("エラー: {e:#}"));
                return;
            }
        };
        self.engine = Ok(engine.clone());
        self.save_last();
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let p = progress.clone();
        let ctx = ctx.clone();
        let as_is = self.cfg.store.enabled && self.cfg.store.as_is;
        let (input, output) = (self.cfg.input.clone(), self.cfg.output.clone());
        std::thread::spawn(move || {
            // 消すのも裏で。数万ファイルを消すあいだ画面を止めないため
            if clean {
                match engine::clean_output(&input, &output) {
                    Ok(n) => p.say(format!("出力先の前のファイル {n} 個を消した")),
                    Err(e) => {
                        let _ = tx.send(Err(format!("{e:#}")));
                        ctx.request_repaint();
                        return;
                    }
                }
            }
            let r = if as_is {
                engine.run_send_as_is(&p)
            } else {
                engine.run(&p)
            }
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        if self.cfg.store.enabled {
            self.tab = RightTab::Send;
        }
        self.note(if as_is {
            "そのまま送り始めた"
        } else {
            "実行を始めた"
        });
        self.job = Some(Job {
            progress,
            done: rx,
            logged: 0,
            started: std::time::Instant::now(),
        });
    }

    fn cancel(&self) {
        if let Some(j) = &self.job {
            j.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// 走っている実行の進み具合とログを取り込む。終わっていれば片づける。
    fn poll_job(&mut self) {
        let Some(job) = &mut self.job else { return };
        let lines: Vec<String> = job.progress.log.lock().unwrap()[job.logged..].to_vec();
        job.logged += lines.len();
        // スレッドがパニックで消えると送り手も消えるので、Disconnected は「失敗して終わった」として扱う
        let finished = match job.done.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("実行のスレッドが途中で止まった".to_string()))
            }
        };
        let elapsed = job.started.elapsed();
        self.log.extend(lines);
        if let Some(r) = finished {
            let verb = if self.cfg.store.enabled {
                "送信"
            } else {
                "書き出し"
            };
            match r {
                Ok(n) => self.log.push(format!(
                    "{verb} {n} 個({:.1} 秒、{:.0} 個/秒)",
                    elapsed.as_secs_f64(),
                    n as f64 / elapsed.as_secs_f64().max(0.001)
                )),
                Err(e) => self.log.push(format!("エラー: {e}")),
            }
            self.job = None;
        }
    }

    /// 窓にドロップされたフォルダ(ファイルならその親)を、元のディレクトリにする。
    fn take_dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        let Some(p) = dropped.into_iter().next() else {
            return;
        };
        let dir = if p.is_dir() {
            p
        } else {
            p.parent().map(Path::to_path_buf).unwrap_or(p)
        };
        self.cfg.input = dir.display().to_string();
        self.refresh_engine();
        self.count_sources(ctx);
        self.note(format!("元のディレクトリ: {}", dir.display()));
    }

    // ---- メニュー --------------------------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut load = None;
        let mut delete = None;
        let mut start = false;
        let mut help = false;
        vw_ui::menu::bar(ui, |ui| {
            vw_ui::menu::bar_menu(ui, "ファイル", |ui| {
                if ui
                    .add(vw_ui::menu::item(false, "設定ファイルを開く…", ""))
                    .clicked()
                {
                    ui.close();
                    let near = self.preset_dir.display().to_string();
                    self.picker = Some((
                        PickTarget::OpenPreset,
                        Self::file_picker(
                            "設定ファイルを開く",
                            picker::Mode::OpenFile {
                                extensions: vec!["toml".into()],
                            },
                            &near,
                        ),
                    ));
                }
                if ui
                    .add(vw_ui::menu::item(false, "設定ファイルに保存…", ""))
                    .clicked()
                {
                    ui.close();
                    let near = self.preset_dir.display().to_string();
                    self.picker = Some((
                        PickTarget::SavePreset,
                        Self::file_picker(
                            "設定ファイルに保存",
                            picker::Mode::SaveFile {
                                extensions: vec!["toml".into()],
                                name: "preset.toml".into(),
                            },
                            &near,
                        ),
                    ));
                }
                ui.separator();
                if ui.add(vw_ui::menu::item(false, "終了", "")).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            vw_ui::menu::bar_menu(ui, "プリセット", |ui| {
                if self.presets.is_empty() {
                    ui.add_enabled(false, egui::Button::new("(保存したプリセットはない)"));
                }
                for name in &self.presets {
                    let current = self.current_preset.as_deref() == Some(name.as_str());
                    if ui
                        .add(vw_ui::menu::item(current, name.as_str(), ""))
                        .clicked()
                    {
                        load = Some(name.clone());
                        ui.close();
                    }
                }
                ui.separator();
                if ui
                    .add(vw_ui::menu::item(false, "いまの設定を保存…", "Ctrl+S"))
                    .clicked()
                {
                    self.save_dialog = Some(SaveDialog {
                        name: self.current_preset.clone().unwrap_or_default(),
                    });
                    ui.close();
                }
                if !self.presets.is_empty() {
                    vw_ui::menu::submenu(ui, "消す", |ui| {
                        for name in &self.presets {
                            if ui
                                .add(vw_ui::menu::item(false, name.as_str(), ""))
                                .clicked()
                            {
                                delete = Some(name.clone());
                                ui.close();
                            }
                        }
                    });
                }
                ui.separator();
                ui.weak(
                    RichText::new(self.preset_dir.display().to_string())
                        .small()
                        .monospace(),
                );
            });
            vw_ui::menu::bar_menu(ui, "実行", |ui| {
                let idle = self.job.is_none();
                if ui
                    .add_enabled(idle, vw_ui::menu::item(false, "水増しを実行", "Ctrl+R"))
                    .clicked()
                {
                    start = true;
                    ui.close();
                }
                if ui
                    .add_enabled(!idle, vw_ui::menu::item(false, "中断", ""))
                    .clicked()
                {
                    self.cancel();
                    ui.close();
                }
                ui.separator();
                if ui
                    .add(vw_ui::menu::item(false, "元のディレクトリを読み直す", ""))
                    .clicked()
                {
                    self.count_sources(&ctx);
                    ui.close();
                }
                if ui.add(vw_ui::menu::item(false, "ログを消す", "")).clicked() {
                    self.log.clear();
                    ui.close();
                }
            });
            vw_ui::menu::bar_menu(ui, "表示", |ui| {
                if ui
                    .add(vw_ui::menu::item(
                        self.tab == RightTab::Preview,
                        "構成とプレビュー",
                        "",
                    ))
                    .clicked()
                {
                    self.tab = RightTab::Preview;
                    ui.close();
                }
                if ui
                    .add(vw_ui::menu::item(self.tab == RightTab::Send, "送信", ""))
                    .clicked()
                {
                    self.tab = RightTab::Send;
                    ui.close();
                }
                ui.separator();
                for (pref, label) in [
                    (ThemePreference::System, "テーマ: システムに合わせる"),
                    (ThemePreference::Dark, "テーマ: ダーク"),
                    (ThemePreference::Light, "テーマ: ライト"),
                ] {
                    if ui
                        .add(vw_ui::menu::item(self.theme == pref, label, ""))
                        .clicked()
                    {
                        self.theme = pref;
                        vw_ui::theme::apply(&ctx, pref, true, 1.0);
                        ui.close();
                    }
                }
            });
            vw_ui::menu::bar_menu(ui, "ヘルプ", |ui| {
                if ui
                    .add(vw_ui::menu::item(
                        false,
                        "Readme.html を開く(変数の一覧と例)",
                        "",
                    ))
                    .clicked()
                {
                    help = true;
                    ui.close();
                }
                if ui
                    .add(vw_ui::menu::item(false, "バージョン情報", ""))
                    .clicked()
                {
                    self.about.open = true;
                    ui.close();
                }
            });
        });
        if help {
            self.open_readme();
        }
        if let Some(n) = load {
            self.load_preset(&n, &ctx);
        }
        if let Some(n) = delete {
            self.delete_preset(&n);
        }
        let (run_key, save_key) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::R),
                i.modifiers.command && i.key_pressed(egui::Key::S),
            )
        });
        if start || run_key {
            self.start(&ctx);
        }
        if save_key && self.save_dialog.is_none() {
            self.save_dialog = Some(SaveDialog {
                name: self.current_preset.clone().unwrap_or_default(),
            });
        }
    }

    /// メニューの下の帯。実行のボタンと、いま何が起きるかの一行。
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut start = false;
        let mut cancel = false;
        let mut help = false;
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            match &self.job {
                Some(j) => {
                    cancel = ui.button("■ 中断").clicked();
                    let total = j.progress.total.load(Ordering::Relaxed);
                    let done = j.progress.done.load(Ordering::Relaxed);
                    let f = if total == 0 {
                        0.0
                    } else {
                        done as f32 / total as f32
                    };
                    ui.add(
                        egui::ProgressBar::new(f)
                            .desired_width(260.0)
                            .text(format!("{done} / {total}")),
                    );
                    let secs = j.started.elapsed().as_secs_f64();
                    if done > 0 && secs > 0.5 {
                        let left = (total.saturating_sub(done)) as f64 / (done as f64 / secs);
                        ui.weak(format!(
                            "{:.0} 個/秒、残り {:.0} 秒",
                            done as f64 / secs,
                            left
                        ));
                    }
                }
                None => {
                    let problem = self.blocking_problem();
                    let button = egui::Button::new(RichText::new("▶ 水増しを実行").strong());
                    let r = ui.add_enabled(problem.is_none(), button);
                    start = r.clicked();
                    let total = if self.cfg.store.enabled && self.cfg.store.as_is {
                        Some(0)
                    } else {
                        self.total()
                    };
                    match (problem, total) {
                        (Some(p), _) => {
                            ui.colored_label(WARN, format!("⚠ {p}"));
                        }
                        (None, Some(0)) => {
                            ui.label("そのまま送る");
                            ui.weak(format!(
                                "{} → C-STORE {}({} 接続、{})",
                                self.cfg.store.as_is_dir.trim(),
                                self.cfg.store.target(),
                                self.cfg.store.connections.max(1),
                                self.cfg.store.unit.label()
                            ));
                        }
                        (None, Some(total)) => {
                            let verb = if self.cfg.store.enabled {
                                "送信"
                            } else {
                                "出力"
                            };
                            ui.label(format!(
                                "元 {} 個 → {verb} {total} 個",
                                self.sources.unwrap_or(0)
                            ));
                            let s = &self.cfg.store;
                            let dest = match (s.enabled, s.also_write) {
                                (true, true) => format!(
                                    "→ C-STORE {}({} 接続、{})と {}",
                                    s.target(),
                                    s.connections.max(1),
                                    s.unit.label(),
                                    self.cfg.output.trim()
                                ),
                                (true, false) => format!(
                                    "→ C-STORE {}({} 接続、{})",
                                    s.target(),
                                    s.connections.max(1),
                                    s.unit.label()
                                ),
                                (false, _) => format!("→ {}", self.cfg.output.trim()),
                            };
                            ui.weak(dest);
                        }
                        (None, None) => {}
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                help = ui
                    .button(RichText::new("ヘルプ！わかんないよ！").strong())
                    .on_hover_text(
                        "同梱の Readme.html をブラウザで開く(変数の一覧・例・動きの説明)",
                    )
                    .clicked();
                ui.add_space(8.0);
                match &self.current_preset {
                    Some(n) => {
                        ui.label(RichText::new(n).strong());
                        ui.weak("プリセット:");
                    }
                    None => {
                        ui.weak("プリセット: (なし)");
                    }
                }
            });
        });
        if cancel {
            self.cancel();
        }
        if start {
            self.start(&ctx);
        }
        if help {
            self.open_readme();
        }
    }

    // ---- 設定 ----------------------------------------------------------------------------

    /// 設定の節の見出し。中身の字より少し大きい字と、下の細線で区切る。
    fn section(ui: &mut egui::Ui, title: &str) {
        ui.add_space(10.0);
        ui.label(RichText::new(title).strong());
        ui.add_space(2.0);
        ui.separator();
        ui.add_space(4.0);
    }

    /// パスの欄。ラベル、入力欄(残りの幅いっぱい)と「参照」。入力欄から離れたら true。
    fn path_row(
        ui: &mut egui::Ui,
        label: &str,
        value: &mut String,
        target: PickTarget,
        picker: &mut Option<(PickTarget, Picker)>,
    ) -> bool {
        ui.label(label);
        let mut committed = false;
        ui.horizontal(|ui| {
            let browse_w = 52.0;
            let r = ui.add(
                egui::TextEdit::singleline(value)
                    .desired_width(ui.available_width() - browse_w)
                    .hint_text("パス"),
            );
            committed |= r.lost_focus();
            if ui.button("参照").clicked() {
                *picker = Some((target, Self::folder_picker("フォルダを選ぶ", value)));
            }
        });
        committed
    }

    /// 窓の文言(日本語)。
    fn picker_labels() -> picker::Labels {
        picker::Labels {
            choose: "選ぶ".into(),
            cancel: "やめる".into(),
            name: "名前".into(),
            nothing: "(フォルダを選んでいない)".into(),
        }
    }

    fn folder_picker(title: &str, current: &str) -> Picker {
        Picker::folder(
            title,
            (!current.trim().is_empty()).then_some(Path::new(current)),
            Self::picker_labels(),
        )
    }

    fn file_picker(title: &str, mode: picker::Mode, near: &str) -> Picker {
        let start = Path::new(near);
        Picker::new(
            title,
            mode,
            (!near.trim().is_empty()).then_some(start),
            Self::picker_labels(),
        )
    }

    /// 選ぶ窓を描き、決まったら入れ先に入れる。
    fn picker_ui(&mut self, ctx: &egui::Context) {
        let Some((target, picker)) = &mut self.picker else {
            return;
        };
        let target = *target;
        let chosen = match picker.show(ctx) {
            picker::Outcome::Open => return,
            picker::Outcome::Cancelled => None,
            picker::Outcome::Chosen(p) => Some(p),
        };
        self.picker = None;
        let Some(p) = chosen else {
            return;
        };
        let text = p.display().to_string();
        match target {
            PickTarget::Input => {
                self.cfg.input = text;
                self.refresh_engine();
                self.count_sources(ctx);
            }
            PickTarget::Output => self.cfg.output = text,
            PickTarget::AsIsDir => self.cfg.store.as_is_dir = text,
            PickTarget::Csv => self.cfg.csv = text,
            PickTarget::RuleCsv(i) => {
                if let Some(r) = self.cfg.rules.get_mut(i) {
                    r.csv = text;
                }
            }
            PickTarget::OpenPreset => match Config::load(&p) {
                Ok(c) => {
                    self.cfg = c;
                    self.current_preset = None;
                    self.note(format!("設定を読んだ: {}", p.display()));
                }
                Err(e) => self.note(format!("エラー: {e:#}")),
            },
            PickTarget::SavePreset => match self.cfg.save(&p) {
                Ok(()) => self.note(format!("設定を保存した: {}", p.display())),
                Err(e) => self.note(format!("エラー: {e:#}")),
            },
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        vw_ui::panel::header(ui, "設定");
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            Self::section(ui, "入出力");
            let recount = Self::path_row(ui, "元のディレクトリ(再帰的に探す)", &mut self.cfg.input, PickTarget::Input, &mut self.picker);
            match (self.sources, &self.scan, &self.tree) {
                (Some(n), Some(s), _) => {
                    ui.colored_label(OK, format!("✓ ファイル {n} 個。構成を読んでいる… {} / {}", s.done.load(Ordering::Relaxed), s.total));
                }
                (Some(n), None, Some(t)) => {
                    let c = t.counts();
                    ui.colored_label(OK, format!("✓ ファイル {n} 個: {} 患者 / {} 検査 / {} シリーズ / {} 画像", c.patients, c.studies, c.series, c.images));
                    if !t.duplicates.is_empty() {
                        ui.colored_label(WARN, format!("⚠ SOPInstanceUID が重なる元ファイル {} 個(複製では別の UID を振る。ログに一覧)", t.duplicates.len()));
                    }
                }
                (Some(n), None, None) => {
                    ui.colored_label(OK, format!("✓ ファイル {n} 個"));
                }
                (None, _, _) if !self.cfg.input.trim().is_empty() => {
                    ui.colored_label(WARN, "⚠ ディレクトリがない");
                }
                _ => {
                    ui.weak("フォルダを窓にドロップしても指定できる");
                }
            }
            if recount || self.counted_input != self.cfg.input && !ui.memory(|m| m.focused().is_some()) {
                self.refresh_engine();
                self.count_sources(&ctx);
            }

            Self::section(ui, "送り先");
            let store = &mut self.cfg.store;
            let mode = match (store.enabled, store.also_write) {
                (false, _) => 0,
                (true, false) => 1,
                (true, true) => 2,
            };
            let labels = ["ファイルに書く", "C-STORE で送る(ディスクに書かない)", "送って、ファイルにも書く"];
            let mut chosen = mode;
            vw_ui::menu::combo("dest_mode").selected_text(labels[mode]).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
                for (k, l) in labels.iter().enumerate() {
                    ui.selectable_value(&mut chosen, k, *l);
                }
            });
            store.enabled = chosen > 0;
            store.also_write = chosen == 2;
            if store.enabled {
                ui.horizontal(|ui| {
                    ui.weak(format!("→ {}", store.target()));
                    if ui.small_button("送信の画面へ").clicked() {
                        self.tab = RightTab::Send;
                    }
                });
            }
            if self.writes_files() {
                ui.add_space(6.0);
                Self::path_row(ui, "出力先", &mut self.cfg.output, PickTarget::Output, &mut self.picker);
                ui.weak("出力は 出力先/P患者/S検査/Rシリーズ/SOPInstanceUID.dcm");
            }

            Self::section(ui, "件数(元の 1 組を何倍にするか)");
            egui::Grid::new("counts").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                for (label, v) in [
                    ("患者", &mut self.cfg.patients),
                    ("患者あたりの検査", &mut self.cfg.studies),
                    ("検査あたりのシリーズ", &mut self.cfg.series),
                    ("シリーズあたりの画像", &mut self.cfg.images),
                ] {
                    ui.label(label);
                    ui.add(egui::DragValue::new(v).range(1..=1_000_000).speed(0.2));
                    ui.end_row();
                }
            });
            let c = &self.cfg;
            let k = c.patients as u64 * c.studies as u64 * c.series as u64 * c.images as u64;
            match self.total() {
                Some(total) => {
                    ui.label(RichText::new(format!("{k} 倍 → 出力 {total} 個")).strong());
                }
                None => {
                    ui.weak(format!("{k} 倍(元のディレクトリを指定すると出力数が出る)"));
                }
            }

            Self::section(ui, "文字コード(日本語)");
            vw_ui::menu::combo("charset")
                .selected_text(charset_label(self.cfg.charset))
                .width(ui.available_width() - 8.0)
                .show_ui(ui, |ui| {
                    for cs in [Charset::Utf8, Charset::Iso2022jp, Charset::Keep] {
                        ui.selectable_value(&mut self.cfg.charset, cs, charset_label(cs));
                    }
                });
            ui.weak(charset_note(self.cfg.charset));

            Self::section(ui, "CSV の差し込み(全体)");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.cfg.csv).hint_text("(使わない)").desired_width(ui.available_width() - 104.0));
                if ui.button("参照").clicked() {
                    self.picker = Some((PickTarget::Csv, Self::file_picker("CSV を選ぶ", csv_mode(), &self.cfg.csv)));
                }
                if ui.button("消す").clicked() {
                    self.cfg.csv.clear();
                }
            });
            ui.horizontal(|ui| {
                ui.label("文字コード");
                vw_ui::menu::combo("csv_enc").selected_text(self.cfg.csv_encoding.label()).show_ui(ui, |ui| {
                    for e in CsvEncoding::ALL {
                        ui.selectable_value(&mut self.cfg.csv_encoding, e, e.label());
                    }
                });
            });
            match &self.engine {
                Ok(e) if !e.csv_columns.is_empty() => {
                    ui.colored_label(OK, format!("✓ {} 行", e.csv_rows));
                    ui.horizontal_wrapped(|ui| {
                        ui.weak("列:");
                        for c in &e.csv_columns {
                            ui.label(RichText::new(c).monospace()).on_hover_text(var_help(c));
                        }
                    });
                }
                Err(e) if !self.cfg.csv.trim().is_empty() && e.contains("CSV") => {
                    ui.colored_label(ERROR, format!("✗ {e}"));
                }
                _ => {
                    ui.weak("1 行目が列名。列名がそのまま変数になり、タグのレベルの単位で 1 行ずつ進む");
                }
            }

            Self::section(ui, "UID");
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.cfg.uid_mode, UidMode::Derived, "元の UID から決める");
                ui.radio_value(&mut self.cfg.uid_mode, UidMode::Random, "毎回ランダム");
            });
            ui.weak(match self.cfg.uid_mode {
                UidMode::Derived => "元の UID と複製の番号から決まる。同じ設定でやり直すと同じ UID になる",
                UidMode::Random => "実行ごとに別の UID になる。同じ実行の中では、同じ元の UID は同じ新しい UID になる(参照は保たれる)",
            });
            ui.label("接頭辞");
            ui.add(egui::TextEdit::singleline(&mut self.cfg.uid_root).desired_width(ui.available_width() - 8.0));
            ui.weak("UID は 接頭辞.<UUID を 10 進にした数> になる");
            ui.add_space(8.0);
        });
    }

    // ---- タグの規則 ----------------------------------------------------------------------

    fn rules_ui(&mut self, ui: &mut egui::Ui) {
        vw_ui::panel::header(ui, "タグの規則");
        ui.add_space(2.0);
        ui.weak("値は printf 風(EVAL%(p1)05d、%(weight).1f、%(name)-12s)。タグは辞書の名前か (gggg,eeee)。同じタグは下のものが勝つ。変数名に乗せると説明が出る。");
        ui.add_space(4.0);

        let mut remove = None;
        let mut add: Option<RuleSpec> = None;
        let mut restore_level: Option<Level> = None;
        let csv_columns = self
            .engine
            .as_ref()
            .ok()
            .map(|e| e.csv_columns.clone())
            .unwrap_or_default();
        let sample = self.sample;
        let place = self.current_place();
        let serial = self
            .engine
            .as_ref()
            .ok()
            .map(|e| e.serial_number(sample, &place))
            .unwrap_or(1);
        let run_time = self.engine.as_ref().ok().map(|e| e.now.clone());
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for level in Level::ALL {
                    let frame = egui::Frame::group(ui.style())
                        .fill(ui.visuals().faint_bg_color)
                        .inner_margin(8.0);
                    frame.show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{}レベル", level.label())).strong());
                            ui.weak(level_note(level));
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(RichText::new("変数:").small());
                            let vars = vars_for(level)
                                .into_iter()
                                .chain(src_vars_for(level))
                                .chain(NOW_INT_VARS)
                                .chain(NOW_STR_VARS)
                                .chain(csv_columns.iter().map(String::as_str));
                            for v in vars {
                                let now = run_time
                                    .as_ref()
                                    .and_then(|t| t.value(v))
                                    .map(|t| format!("{t}(画面ではこの設定を読んだ時刻。出力では実行の始めの時刻)"))
                                    .or_else(|| sample_value(v, sample, &place, serial));
                                let hover = match now {
                                    Some(n) => {
                                        format!("{}\nいま選んでいる複製では {n}", var_help(v))
                                    }
                                    None => var_help(v).to_string(),
                                };
                                ui.label(RichText::new(v).small().monospace())
                                    .on_hover_text(hover);
                            }
                        });
                        ui.add_space(4.0);

                        for i in 0..self.cfg.rules.len() {
                            if self.cfg.rules[i].level == level {
                                self.rule_ui(ui, i, &mut remove);
                            }
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("＋ 規則を足す").clicked() {
                                add = Some(RuleSpec::new("", level, ""));
                            }
                            vw_ui::menu::button(ui, "タグを選んで足す ▼", |ui| {
                                for name in typical_tags(level) {
                                    if ui.button(*name).clicked() {
                                        add = Some(RuleSpec::new(name, level, default_value(name)));
                                        ui.close();
                                    }
                                }
                            });
                            let missing = missing_defaults(&self.cfg.rules, level);
                            if !missing.is_empty()
                                && ui
                                    .button("既定を足し直す")
                                    .on_hover_text(format!(
                                        "消した既定の規則を、既定の値で足し直す: {}",
                                        missing.join(" ")
                                    ))
                                    .clicked()
                            {
                                restore_level = Some(level);
                            }
                        });
                    });
                    ui.add_space(6.0);
                }
                if let Err(e) = &self.engine {
                    ui.add_space(4.0);
                    ui.colored_label(ERROR, format!("✗ {e}"));
                }
            });
        if let Some(i) = remove {
            self.cfg.rules.remove(i);
            self.csv_open.clear();
        }
        if let Some(r) = add {
            self.cfg.rules.push(r);
        }
        if let Some(level) = restore_level {
            for tag in missing_defaults(&self.cfg.rules, level) {
                self.cfg
                    .rules
                    .push(RuleSpec::new(tag, level, default_value(tag)));
            }
        }
    }

    // ---- ヘルプ ----------------------------------------------------------------------------

    /// 同梱の Readme.html。配布では実行ファイルの隣、開発中はソースの apps/dicoamplio/ にある。
    fn readme_path() -> Option<PathBuf> {
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("Readme.html")));
        let in_source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Readme.html");
        [beside_exe, Some(in_source)]
            .into_iter()
            .flatten()
            .find(|p| p.is_file())
    }

    fn open_readme(&mut self) {
        match Self::readme_path() {
            Some(p) => {
                if let Err(e) = vw_launch::open_file(&p) {
                    self.note(format!("エラー: Readme.html を開けない: {e}"));
                }
            }
            None => self.note("エラー: Readme.html が見つからない(実行ファイルの隣に置いてほしい)"),
        }
    }

    /// 規則 1 本。タグ・値・CSV の開閉・削除の行と、開いていれば、そのタグ専用の CSV の行。
    fn rule_ui(&mut self, ui: &mut egui::Ui, i: usize, remove: &mut Option<usize>) {
        let open = self.csv_open.contains(&i) || !self.cfg.rules[i].csv.is_empty();
        let mut toggle = false;
        ui.horizontal(|ui| {
            let r = &mut self.cfg.rules[i];
            ui.add(
                egui::TextEdit::singleline(&mut r.tag)
                    .hint_text("PatientName")
                    .desired_width(TAG_W - 8.0),
            );
            let value_width = (ui.available_width() - 96.0).max(160.0);
            ui.add(
                egui::TextEdit::singleline(&mut r.value)
                    .hint_text("%(name)s")
                    .desired_width(value_width)
                    .font(egui::TextStyle::Monospace),
            );
            toggle = ui
                .selectable_label(!r.csv.is_empty(), "CSV")
                .on_hover_text("このタグだけが読む CSV を指定する")
                .clicked();
            if ui.button("×").on_hover_text("この規則を消す").clicked() {
                *remove = Some(i);
            }
        });
        if toggle && !self.csv_open.insert(i) {
            self.csv_open.remove(&i);
        }
        if !open {
            return;
        }
        ui.horizontal(|ui| {
            ui.add_space(TAG_W);
            let r = &mut self.cfg.rules[i];
            ui.label("CSV");
            ui.add(
                egui::TextEdit::singleline(&mut r.csv)
                    .hint_text("(全体の CSV を使う)")
                    .desired_width((ui.available_width() - 260.0).max(120.0)),
            );
            if ui.button("参照").clicked() {
                self.picker = Some((
                    PickTarget::RuleCsv(i),
                    Self::file_picker("CSV を選ぶ", csv_mode(), &r.csv),
                ));
            }
            if ui.button("消す").clicked() {
                r.csv.clear();
            }
            let label = r
                .csv_encoding
                .map(CsvEncoding::label)
                .unwrap_or("文字コード: 全体に従う");
            vw_ui::menu::combo(("rule_csv_enc", i))
                .selected_text(label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut r.csv_encoding, None, "全体に従う");
                    for e in CsvEncoding::ALL {
                        ui.selectable_value(&mut r.csv_encoding, Some(e), e.label());
                    }
                });
        });
        let r = &self.cfg.rules[i];
        if !r.csv.is_empty() {
            let enc = r.csv_encoding.unwrap_or(self.cfg.csv_encoding);
            if let Some((cols, rows)) = self
                .engine
                .as_ref()
                .ok()
                .and_then(|e| e.table_info(&r.csv, enc))
            {
                ui.horizontal(|ui| {
                    ui.add_space(TAG_W);
                    ui.weak(RichText::new(format!("{rows} 行 / 列: {}", cols.join(" "))).small());
                });
            }
        }
    }

    // ---- 構成(元 → 複製後)とプレビュー ------------------------------------------------------

    fn right_panel_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, label) in [
                (RightTab::Preview, "構成とプレビュー"),
                (RightTab::Send, "送信"),
            ] {
                if ui
                    .selectable_label(self.tab == tab, RichText::new(label).strong())
                    .clicked()
                {
                    self.tab = tab;
                }
            }
        });
        ui.separator();
        match self.tab {
            RightTab::Preview => self.preview_ui(ui),
            RightTab::Send => self.send_ui(ui),
        }
    }

    // ---- 送信 ----------------------------------------------------------------------------

    fn send_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical()
            .id_salt("send")
            .auto_shrink(false)
            .show(ui, |ui| {
                self.send_destination_ui(ui, &ctx);
                self.send_what_ui(ui);
                self.send_status_ui(ui);
                self.send_results_ui(ui);
            });
    }

    /// 送り先: 帳簿から選ぶか、入れて保存する。C-ECHO で試せる。
    fn send_destination_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        Self::section(ui, "送り先");
        let mut pick: Option<usize> = None;
        let mut delete: Option<usize> = None;
        ui.horizontal(|ui| {
            let current = self.destinations.iter().position(|d| {
                d.host == self.cfg.store.host
                    && d.port == self.cfg.store.port
                    && d.called_ae == self.cfg.store.called_ae
            });
            let text = current
                .map(|i| self.destinations[i].name.clone())
                .unwrap_or_else(|| "(帳簿から選ぶ)".into());
            vw_ui::menu::combo("dest_book")
                .selected_text(text)
                .width(240.0)
                .show_ui(ui, |ui| {
                    if self.destinations.is_empty() {
                        ui.weak("(まだない。下で入れて保存する)");
                    }
                    for (i, d) in self.destinations.iter().enumerate() {
                        if ui
                            .selectable_label(
                                current == Some(i),
                                format!("{}  {}@{}:{}", d.name, d.called_ae, d.host, d.port),
                            )
                            .clicked()
                        {
                            pick = Some(i);
                        }
                    }
                });
            if let Some(i) = current {
                if ui.small_button("帳簿から消す").clicked() {
                    delete = Some(i);
                }
            }
        });
        if let Some(i) = pick {
            let d = self.destinations[i].clone();
            self.cfg.store.host = d.host;
            self.cfg.store.port = d.port;
            self.cfg.store.called_ae = d.called_ae;
            self.dest_name = d.name;
        }
        if let Some(i) = delete {
            let d = self.destinations.remove(i);
            match config::save_destinations(&self.preset_dir, &self.destinations) {
                Ok(()) => self.note(format!("送り先「{}」を帳簿から消した", d.name)),
                Err(e) => self.note(format!("エラー: {e:#}")),
            }
        }
        let store = &mut self.cfg.store;
        egui::Grid::new("store")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("ホスト");
                ui.add(egui::TextEdit::singleline(&mut store.host).desired_width(240.0));
                ui.end_row();
                ui.label("ポート");
                ui.add(
                    egui::DragValue::new(&mut store.port)
                        .range(1..=65535)
                        .speed(1.0),
                );
                ui.end_row();
                ui.label("相手の AE タイトル");
                ui.add(
                    egui::TextEdit::singleline(&mut store.called_ae)
                        .desired_width(240.0)
                        .char_limit(16),
                );
                ui.end_row();
                ui.label("自分の AE タイトル");
                ui.add(
                    egui::TextEdit::singleline(&mut store.calling_ae)
                        .desired_width(240.0)
                        .char_limit(16),
                );
                ui.end_row();
            });
        ui.horizontal(|ui| {
            ui.label("名前");
            ui.add(
                egui::TextEdit::singleline(&mut self.dest_name)
                    .desired_width(150.0)
                    .hint_text("評価 PACS"),
            );
            let exists = self
                .destinations
                .iter()
                .any(|d| d.name == self.dest_name.trim());
            let ok = !self.dest_name.trim().is_empty() && !self.cfg.store.host.trim().is_empty();
            if ui
                .add_enabled(
                    ok,
                    egui::Button::new(if exists {
                        "帳簿を上書き"
                    } else {
                        "帳簿に保存"
                    }),
                )
                .clicked()
            {
                let d = Destination {
                    name: self.dest_name.trim().to_string(),
                    host: self.cfg.store.host.trim().to_string(),
                    port: self.cfg.store.port,
                    called_ae: self.cfg.store.called_ae.trim().to_string(),
                };
                match self.destinations.iter_mut().find(|x| x.name == d.name) {
                    Some(x) => *x = d.clone(),
                    None => self.destinations.push(d.clone()),
                }
                match config::save_destinations(&self.preset_dir, &self.destinations) {
                    Ok(()) => self.note(format!("送り先「{}」を帳簿に保存した", d.name)),
                    Err(e) => self.note(format!("エラー: {e:#}")),
                }
            }
            let busy = self.echo.is_some();
            if ui
                .add_enabled(!busy, egui::Button::new("C-ECHO"))
                .on_hover_text("接続を試す")
                .clicked()
            {
                let cfg = self.cfg.store.clone();
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let r = crate::store::echo(&cfg).map_err(|e| format!("{e:#}"));
                    let _ = tx.send(r);
                    ctx.request_repaint();
                });
                self.echo = Some(rx);
            }
            if busy {
                ui.weak("試している…");
            }
        });
    }

    /// 送るもの(水増し / そのまま)と送り方(単位・接続数)。
    fn send_what_ui(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "送るもの");
        let store = &mut self.cfg.store;
        let mut as_is = store.as_is;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut as_is, false, "水増しして送る(設定と規則を使う)");
            ui.selectable_value(&mut as_is, true, "フォルダの DICOM をそのまま送る");
        });
        store.as_is = as_is;
        if as_is {
            Self::path_row(
                ui,
                "そのまま送るディレクトリ(再帰的に探す)",
                &mut store.as_is_dir,
                PickTarget::AsIsDir,
                &mut self.picker,
            );
            ui.weak("複製も書き換えもしない。UID も元のまま");
        } else if !store.enabled {
            ui.colored_label(WARN, "⚠ 設定の「送り先」が「ファイルに書く」になっている。送るには「C-STORE で送る」にする");
        } else {
            ui.weak(if store.also_write {
                "送って、出力先にも書く"
            } else {
                "ディスクには書かない"
            });
        }
        store.enabled |= as_is;
        egui::Grid::new("send_how")
            .num_columns(4)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("アソシエーションの単位");
                vw_ui::menu::combo("assoc_unit")
                    .selected_text(store.unit.label())
                    .show_ui(ui, |ui| {
                        for u in config::AssocUnit::ALL {
                            ui.selectable_value(&mut store.unit, u, u.label());
                        }
                    });
                ui.label("同時の接続数");
                ui.add(
                    egui::DragValue::new(&mut store.connections)
                        .range(1..=64)
                        .speed(0.2),
                );
                ui.end_row();
            });
        ui.weak("単位ごとにアソシエーションを 1 本張り、そのまとまりに要る SOP Class だけを提案して、送り切ったら解放する");
    }

    /// 接続ごとの状態と、端末風のログ。
    fn send_status_ui(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "接続の状態");
        let Some(job) = &self.job else {
            ui.weak("実行中の送信はない");
            return;
        };
        let workers: Vec<WorkerStatus> = job.progress.workers.lock().unwrap().clone();
        let logs: Vec<Vec<String>> = job
            .progress
            .worker_logs
            .lock()
            .unwrap()
            .iter()
            .map(|l| l.iter().rev().take(200).rev().cloned().collect())
            .collect();
        let term_bg = ui.visuals().extreme_bg_color;
        for (k, w) in workers.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("接続 {}", k + 1)).strong());
                match (&w.group, w.started) {
                    (Some(g), Some(t)) => {
                        let secs = t.elapsed().as_secs_f64();
                        let rate = if secs > 0.2 {
                            format!("{:.0} 個/秒", w.done as f64 / secs)
                        } else {
                            String::new()
                        };
                        ui.label(g);
                        ui.add(
                            egui::ProgressBar::new(if w.total == 0 {
                                0.0
                            } else {
                                w.done as f32 / w.total as f32
                            })
                            .desired_width(140.0)
                            .text(format!("{} / {}", w.done, w.total)),
                        );
                        ui.weak(rate);
                    }
                    _ => {
                        ui.weak("待機");
                    }
                }
            });
            egui::Frame::NONE
                .fill(term_bg)
                .inner_margin(egui::Margin::symmetric(6, 4))
                .corner_radius(2)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .id_salt(("wlog", k))
                        .max_height(96.0)
                        .stick_to_bottom(true)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for line in logs.get(k).map(|l| l.as_slice()).unwrap_or(&[]) {
                                let text = RichText::new(line).small().monospace();
                                // 「拒否 0」は失敗ではない
                                let bad = line.contains("失敗")
                                    || (line.contains("拒否") && !line.ends_with("拒否 0"));
                                if bad {
                                    ui.colored_label(ERROR, text);
                                } else {
                                    ui.label(text);
                                }
                            }
                        });
                });
            ui.add_space(4.0);
        }
    }

    /// 送り終えたまとまりの表。
    fn send_results_ui(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "結果(まとまりごと)");
        let Some(job) = &self.job else {
            ui.weak("(次の送信の結果がここに出る)");
            return;
        };
        let results: Vec<GroupResult> = job.progress.results.lock().unwrap().clone();
        if results.is_empty() {
            ui.weak("(まだない)");
            return;
        }
        egui::Grid::new("results")
            .num_columns(5)
            .striped(true)
            .spacing([12.0, 2.0])
            .show(ui, |ui| {
                for h in ["まとまり", "個数", "成功", "拒否", "秒"] {
                    ui.label(RichText::new(h).weak().small());
                }
                ui.end_row();
                for r in &results {
                    ui.label(&r.label);
                    ui.label(r.count.to_string());
                    ui.label(r.sent.to_string());
                    if r.rejected > 0 {
                        ui.colored_label(ERROR, r.rejected.to_string());
                    } else {
                        ui.label("0");
                    }
                    ui.label(format!("{:.1}", r.secs));
                    ui.end_row();
                    if let Some(e) = &r.error {
                        ui.colored_label(ERROR, format!("✗ {e}"));
                        ui.end_row();
                    }
                }
            });
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui) {
        let c = &self.cfg;
        self.sample.p = self.sample.p.min(c.patients.max(1) - 1);
        self.sample.s = self.sample.s.min(c.studies.max(1) - 1);
        self.sample.r = self.sample.r.min(c.series.max(1) - 1);
        self.sample.i = self.sample.i.min(c.images.max(1) - 1);

        let tree_h = (ui.available_height() * 0.45).max(160.0);
        ui.columns(2, |cols| {
            self.source_tree_ui(&mut cols[0], tree_h);
            self.planned_tree_ui(&mut cols[1], tree_h);
        });
        ui.add_space(4.0);
        ui.separator();
        self.values_ui(ui);
    }

    fn source_tree_ui(&self, ui: &mut egui::Ui, height: f32) {
        ui.label(RichText::new("元").strong());
        let Some(t) = &self.tree else {
            match &self.scan {
                Some(s) => {
                    ui.weak(format!(
                        "読んでいる… {} / {}",
                        s.done.load(Ordering::Relaxed),
                        s.total
                    ));
                }
                None => {
                    ui.weak("元のディレクトリを指定すると、患者 → 検査 → シリーズの構成が出る");
                }
            }
            return;
        };
        let c = t.counts();
        ui.weak(counts_line(c));
        // 横にもスクロールさせる。長い患者 ID・名前で列が広がると、右のパネルごと広がって規則の欄を狭めるため
        egui::ScrollArea::both()
            .id_salt("src_tree")
            .max_height(height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                for (pi, p) in t.patients.iter().enumerate().take(SOURCE_LIMIT) {
                    let label = format!("患者 {} {}", p.id, p.name);
                    egui::CollapsingHeader::new(label)
                        .id_salt(("src_p", pi))
                        .default_open(pi == 0)
                        .show(ui, |ui| {
                            for (si, s) in p.studies.iter().enumerate() {
                                let label = format!("検査 {} {} {}", s.id, s.date, s.description);
                                egui::CollapsingHeader::new(label)
                                    .id_salt(("src_s", pi, si))
                                    .default_open(si == 0)
                                    .show(ui, |ui| {
                                        for r in &s.series {
                                            let inst = r
                                                .inst
                                                .map(|(lo, hi)| format!(" Inst {lo}〜{hi}"))
                                                .unwrap_or_default();
                                            ui.label(format!(
                                                "シリーズ {} {} {} — {} 画像{inst}",
                                                r.number, r.modality, r.description, r.images
                                            ));
                                        }
                                    });
                            }
                        });
                }
                if t.patients.len() > SOURCE_LIMIT {
                    ui.weak(format!("…ほか {} 患者", t.patients.len() - SOURCE_LIMIT));
                }
            });
    }

    /// 複製後の木。ノードを押すと、その複製の番号を「選んでいる複製」にする。
    fn planned_tree_ui(&mut self, ui: &mut egui::Ui, height: f32) {
        ui.label(RichText::new("複製後").strong());
        let (Some(t), Ok(engine)) = (self.tree.clone(), self.engine.clone()) else {
            if let Err(e) = &self.engine {
                ui.colored_label(ERROR, format!("✗ {e}"));
            } else {
                ui.weak("(元を読んでから)");
            }
            return;
        };
        let c = &self.cfg;
        let (np, ns, nr, ni) = (c.patients, c.studies, c.series, c.images);
        ui.weak(counts_line(t.planned(np, ns, nr, ni)));
        let mut pick: Option<(Selection, Idx)> = None;
        let sel = self.selection;
        let cur = self.sample;
        let value = |tag, ix: Idx, place: &Place, fallback: &str| -> String {
            match engine.render_rule(tag, ix, place) {
                Some(Ok(v)) => v,
                Some(Err(_)) => "?".into(),
                None => fallback.to_string(),
            }
        };
        egui::ScrollArea::both().id_salt("plan_tree").max_height(height).auto_shrink([false, true]).show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for (pi, p) in t.patients.iter().enumerate().take(SOURCE_LIMIT) {
                let p_place = t.place_of(pi, 0, 0).unwrap_or_default();
                for pk in 0..np.min(TREE_LIMIT as u32) {
                    let ix = Idx { p: pk, s: 0, r: 0, i: 0 };
                    let label = format!("患者 {} {}  (P={pi} p={pk})", value(tags::PATIENT_ID, ix, &p_place, &p.id), value(tags::PATIENT_NAME, ix, &p_place, &p.name));
                    let selected = sel.patient == pi && cur.p == pk;
                    let header = egui::CollapsingHeader::new(RichText::new(label).color(if selected { ui.visuals().strong_text_color() } else { ui.visuals().text_color() }))
                        .id_salt(("plan_p", pi, pk))
                        .default_open(pi == 0 && pk == 0);
                    let r = header.show(ui, |ui| {
                        for (si, s) in p.studies.iter().enumerate() {
                            let s_place = t.place_of(pi, si, 0).unwrap_or_default();
                            for sk in 0..ns.min(TREE_LIMIT as u32) {
                                let ix = Idx { p: pk, s: sk, r: 0, i: 0 };
                                let label = format!(
                                    "検査 {} {} {}  (S={si} s={sk})",
                                    value(tags::STUDY_ID, ix, &s_place, &s.id),
                                    value(tags::STUDY_DATE, ix, &s_place, &s.date),
                                    value(tags::STUDY_DESCRIPTION, ix, &s_place, &s.description)
                                );
                                let r = egui::CollapsingHeader::new(label).id_salt(("plan_s", pi, pk, si, sk)).default_open(si == 0 && sk == 0).show(ui, |ui| {
                                    for (ri, ser) in s.series.iter().enumerate() {
                                        let r_place = t.place_of(pi, si, ri).unwrap_or_default();
                                        for rk in 0..nr.min(TREE_LIMIT as u32) {
                                            let ix = Idx { p: pk, s: sk, r: rk, i: cur.i.min(ni - 1) };
                                            let inst = match (ser.inst, engine.instance_number_is_default()) {
                                                (Some((lo, hi)), true) => {
                                                    let k = cur.i as i64 * 10000;
                                                    format!(" Inst {}〜{}", lo + k, hi + k)
                                                }
                                                _ => String::new(),
                                            };
                                            let label = format!(
                                                "シリーズ {} {} {}  (R={ri} r={rk}) — {} 画像 × {ni}{inst}",
                                                value(tags::SERIES_NUMBER, ix, &r_place, &ser.number),
                                                value(tags::MODALITY, ix, &r_place, &ser.modality),
                                                value(tags::SERIES_DESCRIPTION, ix, &r_place, &ser.description),
                                                ser.images
                                            );
                                            let selected = sel == Selection { patient: pi, study: si, series: ri } && cur.p == pk && cur.s == sk && cur.r == rk;
                                            if ui.selectable_label(selected, label).clicked() {
                                                pick = Some((Selection { patient: pi, study: si, series: ri }, ix));
                                            }
                                        }
                                        if nr as usize > TREE_LIMIT {
                                            ui.weak(format!("…ほか {} シリーズ(r={}〜{})", nr as usize - TREE_LIMIT, TREE_LIMIT, nr - 1));
                                        }
                                    }
                                });
                                if r.header_response.clicked() {
                                    pick = Some((Selection { patient: pi, study: si, series: 0 }, Idx { p: pk, s: sk, r: 0, i: cur.i }));
                                }
                            }
                            if ns as usize > TREE_LIMIT {
                                ui.weak(format!("…ほか {} 検査(s={}〜{})", ns as usize - TREE_LIMIT, TREE_LIMIT, ns - 1));
                            }
                        }
                    });
                    if r.header_response.clicked() {
                        pick = Some((Selection { patient: pi, study: 0, series: 0 }, Idx { p: pk, s: 0, r: 0, i: cur.i }));
                    }
                }
                if np as usize > TREE_LIMIT {
                    ui.weak(format!("…ほか {} 患者(p={}〜{})", np as usize - TREE_LIMIT, TREE_LIMIT, np - 1));
                }
            }
            if t.patients.len() > SOURCE_LIMIT {
                ui.weak(format!("…ほか {} 元の患者", t.patients.len() - SOURCE_LIMIT));
            }
        });
        if let Some((s, ix)) = pick {
            self.selection = s;
            self.sample = ix;
        }
    }

    /// 選んでいる複製の番号と、そこでの各タグの値。
    fn values_ui(&mut self, ui: &mut egui::Ui) {
        let place = self.current_place();
        let c = &self.cfg;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("選んでいる複製").strong());
            for (label, v, max) in [
                ("患者 p", &mut self.sample.p, c.patients),
                ("検査 s", &mut self.sample.s, c.studies),
                ("シリーズ r", &mut self.sample.r, c.series),
                ("画像 i", &mut self.sample.i, c.images),
            ] {
                ui.weak(label);
                ui.add(egui::DragValue::new(v).range(0..=max.max(1) - 1).speed(0.2));
            }
            if let Ok(e) = &self.engine {
                ui.weak(format!(
                    "→ 通し番号 n = {}",
                    e.serial_number(self.sample, &place)
                ));
            }
        });
        if let Some(t) = &self.tree {
            let s = self.selection;
            if let Some(p) = t.patients.get(s.patient) {
                let study = p.studies.get(s.study);
                let series = study.and_then(|st| st.series.get(s.series));
                ui.weak(format!(
                    "元: 患者 {} {} (P={}) / 検査 {} (S={}) / シリーズ {} (R={}) の、患者 {} 回目・検査 {} 回目・シリーズ {} 回目の複製",
                    p.id,
                    p.name,
                    s.patient,
                    study.map(|st| st.id.as_str()).unwrap_or("-"),
                    s.study,
                    series.map(|r| r.number.as_str()).unwrap_or("-"),
                    s.series,
                    self.sample.p + 1,
                    self.sample.s + 1,
                    self.sample.r + 1
                ));
            }
        } else {
            ui.weak("元ファイルは読まず、規則だけを当てた値");
        }
        ui.add_space(4.0);
        let rows: Vec<PreviewRow> = self
            .engine
            .as_ref()
            .map(|e| e.preview(self.sample, &place))
            .unwrap_or_default();
        let field_bg = ui.visuals().extreme_bg_color;
        egui::ScrollArea::vertical()
            .id_salt("values")
            .auto_shrink(false)
            .show(ui, |ui| {
                for r in &rows {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&r.name).strong());
                        ui.weak(RichText::new(format!("{} {}", r.tag, r.vr)).small());
                        ui.weak(RichText::new(format!("{}レベル", r.level.label())).small());
                        if let Some(line) = r.csv_line {
                            ui.weak(RichText::new(format!("CSV {line} 行目")).small());
                        }
                    });
                    egui::Frame::NONE
                        .fill(field_bg)
                        .inner_margin(egui::Margin::symmetric(6, 3))
                        .corner_radius(2)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            match &r.value {
                                Ok(v) if v.is_empty() => {
                                    ui.weak(RichText::new("(空)").monospace());
                                }
                                Ok(v) => {
                                    ui.label(RichText::new(v).monospace());
                                }
                                Err(e) => {
                                    ui.colored_label(ERROR, format!("✗ {e}"));
                                }
                            }
                            if let Some(b) = &r.encoded {
                                let hex: String = b.iter().map(|x| format!("{x:02X} ")).collect();
                                ui.weak(RichText::new(hex).small().monospace());
                            }
                        });
                    for w in &r.warnings {
                        ui.colored_label(WARN, format!("⚠ {w}"));
                    }
                    ui.add_space(6.0);
                }
            });
    }

    // ---- ログ ----------------------------------------------------------------------------

    fn log_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            vw_ui::panel::header(ui, "ログ");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("消す").clicked() {
                    self.log.clear();
                }
            });
        });
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink(false)
            .show(ui, |ui| {
                for l in &self.log {
                    let text = RichText::new(l).monospace();
                    if l.starts_with("エラー") {
                        ui.colored_label(ERROR, text);
                    } else {
                        ui.label(text);
                    }
                }
            });
    }

    // ---- 出力先の前のファイル ------------------------------------------------------------

    fn clean_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.clean_dialog else {
            return;
        };
        /// 押されたもの。None は「まだ」
        enum Choice {
            Run { clean: bool },
            ConfirmClean(OutputSummary),
            Back(Result<OutputSummary, String>),
            Cancel,
        }
        let mut choice = None;
        let output = self.cfg.output.trim().to_string();
        let title = match dialog {
            CleanDialog::Confirm(_) => "本当に消す?",
            _ => "出力先の確認",
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                match dialog {
                    CleanDialog::Empty => {
                        ui.label(RichText::new(&output).monospace());
                        ui.label("出力先フォルダには、何もありません。");
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("出力する").clicked() {
                                choice = Some(Choice::Run { clean: false });
                            }
                            if ui.button("やめる").clicked() || esc {
                                choice = Some(Choice::Cancel);
                            }
                        });
                    }
                    CleanDialog::Ask(summary) => {
                        ui.label(RichText::new(&output).monospace());
                        match summary {
                            Ok(s) => {
                                ui.label(format!(
                                    "出力先フォルダに {} 個の項目(ファイル {} 個)がある。消してから出力する?",
                                    s.entries.len(),
                                    s.files
                                ));
                            }
                            Err(e) => {
                                ui.label("出力先フォルダに、ファイルかフォルダがある。");
                                ui.colored_label(ERROR, format!("✗ {e}"));
                            }
                        }
                        ui.add_space(4.0);
                        ui.colored_label(
                            WARN,
                            "⚠ 消さないと、前のファイルと今回のファイルが混ざる。同じ名前のものは上書きされる",
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if let Ok(s) = summary {
                                if ui.button("はい(中身を見て消す)").clicked() {
                                    choice = Some(Choice::ConfirmClean(s.clone()));
                                }
                            }
                            if ui.button("いいえ(消さずに出力する)").clicked() {
                                choice = Some(Choice::Run { clean: false });
                            }
                            if ui.button("やめる").clicked() || esc {
                                choice = Some(Choice::Cancel);
                            }
                        });
                    }
                    CleanDialog::Confirm(s) => {
                        ui.label("次のフォルダの中身を全部消してから出力する。フォルダそのものは残る。");
                        ui.label(RichText::new(s.path.display().to_string()).monospace().strong());
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(260.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for e in &s.entries {
                                    let line = match (e.dir, e.files) {
                                        (true, 0) => format!("📁 {}/  (空のフォルダ)", e.name),
                                        (true, n) => format!("📁 {}/  ファイル {n} 個", e.name),
                                        (false, _) => format!("📄 {}", e.name),
                                    };
                                    let text = RichText::new(line).monospace();
                                    if e.ours {
                                        ui.label(text);
                                    } else {
                                        ui.colored_label(WARN, text);
                                    }
                                }
                            });
                        ui.add_space(4.0);
                        ui.label(format!("合わせて {} 項目、ファイル {} 個", s.entries.len(), s.files));
                        if s.foreign() > 0 {
                            ui.colored_label(
                                WARN,
                                format!(
                                    "⚠ DicoAmplio が書いた形(P000000 のようなフォルダ)でないものが {} 個ある(黄色)。出力先を間違えていないか確かめてほしい",
                                    s.foreign()
                                ),
                            );
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let kill = egui::Button::new(RichText::new("本当に消して出力する").color(ERROR));
                            if ui.add(kill).clicked() {
                                choice = Some(Choice::Run { clean: true });
                            }
                            if ui.button("戻る").clicked() || esc {
                                choice = Some(Choice::Back(Ok(s.clone())));
                            }
                        });
                    }
                }
            });
        let Some(choice) = choice else { return };
        match choice {
            Choice::ConfirmClean(s) => self.clean_dialog = Some(CleanDialog::Confirm(s)),
            Choice::Back(s) => self.clean_dialog = Some(CleanDialog::Ask(s)),
            Choice::Cancel => {
                self.clean_dialog = None;
                self.note("出力をやめた");
            }
            Choice::Run { clean } => {
                let had_entries = matches!(self.clean_dialog, Some(CleanDialog::Ask(_)));
                self.clean_dialog = None;
                if !clean && had_entries {
                    self.note("⚠ 出力先の前のファイルを消さずに出力する。前のファイルと混ざる");
                }
                self.launch(ctx, clean);
            }
        }
    }

    // ---- プリセットの保存 ----------------------------------------------------------------

    fn save_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.save_dialog else {
            return;
        };
        let mut action = None;
        let exists = self.presets.iter().any(|p| p == dialog.name.trim());
        let is_current = self.current_preset.as_deref() == Some(dialog.name.trim());
        egui::Window::new("プリセットを保存")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("名前");
                let r = ui.add(egui::TextEdit::singleline(&mut dialog.name).desired_width(280.0));
                if !r.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                    r.request_focus();
                }
                let ok = !dialog.name.trim().is_empty();
                if exists && !is_current {
                    ui.colored_label(WARN, "⚠ 同じ名前のプリセットがある。保存すると上書きする");
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let label = if exists { "上書き保存" } else { "保存" };
                    let enter =
                        ok && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add_enabled(ok, egui::Button::new(label)).clicked() || enter {
                        action = Some(true);
                    }
                    if ui.button("やめる").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        action = Some(false);
                    }
                });
            });
        match action {
            Some(true) => {
                let n = self.save_dialog.take().map(|d| d.name).unwrap_or_default();
                self.save_preset(&n);
            }
            Some(false) => self.save_dialog = None,
            None => {}
        }
    }
}

fn counts_line(c: Counts) -> String {
    format!(
        "{} 患者 / {} 検査 / {} シリーズ / {} 画像",
        c.patients, c.studies, c.series, c.images
    )
}

/// 変数の、いま選んでいる複製での値(吹き出し用)。CSV の列なら None。
fn sample_value(name: &str, ix: Idx, place: &Place, serial: u64) -> Option<String> {
    let n = match name {
        "p" => ix.p as i64,
        "p1" => ix.p as i64 + 1,
        "P" => place.patient as i64,
        "s" => ix.s as i64,
        "s1" => ix.s as i64 + 1,
        "S" => place.study as i64,
        "r" => ix.r as i64,
        "r1" => ix.r as i64 + 1,
        "R" => place.series as i64,
        "i" => ix.i as i64,
        "i1" => ix.i as i64 + 1,
        "I" => place.image as i64,
        "n" => serial as i64,
        "inst" => 1 + ix.i as i64 * 10000,
        "src_patient_id" => return Some(format!("`{}`", place.patient_id)),
        "src_patient_name" => return Some(format!("`{}`", place.patient_name)),
        "src_patient_birth_date" => return Some(format!("`{}`", place.patient_birth_date)),
        "src_patient_sex" => return Some(format!("`{}`", place.patient_sex)),
        "src_study_id" => return Some(format!("`{}`", place.study_id)),
        "src_study_description" => return Some(format!("`{}`", place.study_description)),
        "src_series_number" => return Some(format!("`{}`", place.series_number)),
        "src_series_description" => return Some(format!("`{}`", place.series_description)),
        "src_modality" => return Some(format!("`{}`", place.modality)),
        _ => return None,
    };
    Some(n.to_string())
}

fn charset_label(cs: Charset) -> &'static str {
    match cs {
        Charset::Utf8 => "UTF-8(ISO_IR 192)",
        Charset::Iso2022jp => "ISO 2022 IR 13 / IR 87(JIS)",
        Charset::Keep => "触れない(元ファイルのまま)",
    }
}

fn charset_note(cs: Charset) -> &'static str {
    match cs {
        Charset::Utf8 => "(0008,0005) = ISO_IR 192。新しい装置・ビューアなら読める。",
        Charset::Iso2022jp => "(0008,0005) = ISO 2022 IR 13\\ISO 2022 IR 87。JAHIS など日本の運用向け。JIS X 0208 外の字は書けない。",
        Charset::Keep => "元ファイルの文字セットのまま。その文字セットで書けない値はエラーで止まる。",
    }
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_last();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job();
        self.poll_scan();
        self.poll_echo();
        self.take_dropped(&ctx);
        self.refresh_engine();
        if self.job.is_some() || self.scan.is_some() || self.echo.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        // メニューと道具の帯は窓の地の色、パネルはその一段上にして、枠・パネルが線に頼らず見分けられるようにする(vw と同じ)
        let chrome = ui.visuals().extreme_bg_color;
        egui::Panel::top("menu")
            .frame(egui::Frame::side_top_panel(ui.style()).fill(chrome))
            .show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .fill(chrome)
                    .inner_margin(egui::Margin::symmetric(6, 6)),
            )
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(140.0)
            .show(ui, |ui| self.log_ui(ui));
        egui::Panel::left("settings")
            .resizable(true)
            .default_size(320.0)
            .show(ui, |ui| self.settings_ui(ui));
        egui::Panel::right("preview")
            .resizable(true)
            .default_size(520.0)
            .show(ui, |ui| self.right_panel_ui(ui));
        egui::CentralPanel::default().show(ui, |ui| self.rules_ui(ui));
        self.save_dialog_ui(&ctx);
        self.clean_dialog_ui(&ctx);
        self.picker_ui(&ctx);
        if let Some(e) = self.about.ui(&ctx) {
            self.note(format!("エラー: ライセンスの一覧を開けない: {e}"));
        }
    }
}

/// CSV の選ぶ窓。.txt も見せるのは、タブ区切りや別の拡張子で渡されることがあるため。
fn csv_mode() -> picker::Mode {
    picker::Mode::OpenFile {
        extensions: vec!["csv".into(), "txt".into()],
    }
}
