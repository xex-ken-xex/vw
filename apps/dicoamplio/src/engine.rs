//! 水増し本体。元の各ファイルを 1 回だけ読み込み、(患者, 検査, シリーズ, 画像) の組ごとに
//! 複製して UID と識別タグを振り直し、rayon で並列に書き出す。

use crate::charset::{is_multi_iso2022, scs_values, Charset, Iso2022};
use crate::config::{Config, CsvEncoding, Level, RuleSpec, UidMode};
use crate::store;
use crate::template::{Template, Vars};
use crate::tree::{self, Place, SourceTree};
use anyhow::{bail, Context, Result};
use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry, VirtualVr};
use dicom_core::header::Header;
use dicom_core::value::Value;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::{tags, StandardDataDictionary};
use dicom_encoding::text::{SpecificCharacterSet, TextCodec};
use dicom_object::{open_file, InMemDicomObject};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use walkdir::WalkDir;

type Row = HashMap<String, String>;

pub struct Table {
    columns: Vec<String>,
    rows: Vec<Row>,
}

/// 読んだ CSV の置き場。画面は設定を変えるたびに Engine を作り直すので、CSV を読み直さずに済むよう
/// 画面が持ち、更新日時が変わったときだけ読み直す。
pub type TableCache = HashMap<(String, CsvEncoding), (Option<std::time::SystemTime>, Arc<Table>)>;

#[derive(Clone, Copy)]
enum UidLevel {
    Study,
    Series,
    Instance,
    Frame,
}

/// 置き換える UID のタグ(シーケンス内も含む)と、その階層。
const UID_TAGS: &[(Tag, UidLevel)] = &[
    (tags::STUDY_INSTANCE_UID, UidLevel::Study),
    (tags::SERIES_INSTANCE_UID, UidLevel::Series),
    (tags::SOP_INSTANCE_UID, UidLevel::Instance),
    (tags::REFERENCED_SOP_INSTANCE_UID, UidLevel::Instance),
    (tags::FRAME_OF_REFERENCE_UID, UidLevel::Frame),
    (Tag(0x3006, 0x0024), UidLevel::Frame), // ReferencedFrameOfReferenceUID
    (Tag(0x0020, 0x0200), UidLevel::Frame), // SynchronizationFrameOfReferenceUID
];

/// 規則の値に使える整数の変数。小文字は複製の番号、大文字は元の木での番号。
pub const BUILTIN_VARS: [&str; 14] = [
    "p", "p1", "P", "s", "s1", "S", "r", "r1", "R", "i", "i1", "I", "n", "inst",
];

/// 元ファイルの値をそのまま使う文字列の変数(レベルの粗い順)。
pub const SRC_VARS: [&str; 9] = [
    "src_patient_id",
    "src_patient_name",
    "src_patient_birth_date",
    "src_patient_sex",
    "src_study_id",
    "src_study_description",
    "src_series_number",
    "src_series_description",
    "src_modality",
];

/// 生成を実行した日時の変数。どのレベルでも使える。整数は年・月・日・時・分・秒、文字列は
/// 日付 `YYYYMMDD` と時刻 `HHMMSS`(DICOM の DA・TM の形)。
pub const NOW_INT_VARS: [&str; 6] = [
    "now_year",
    "now_month",
    "now_day",
    "now_hour",
    "now_minute",
    "now_second",
];
pub const NOW_STR_VARS: [&str; 2] = ["now_date", "now_time"];

/// 生成を実行した日時。PC の現地時刻で、実行の始めに 1 回だけ取る。
#[derive(Clone, Debug, PartialEq)]
pub struct RunTime {
    pub at: chrono::NaiveDateTime,
    pub parts: [i64; 6],
    pub date: String,
    pub time: String,
}

impl RunTime {
    pub fn now() -> RunTime {
        Self::from(chrono::Local::now().naive_local())
    }

    /// 変数の値(画面の吹き出し用)。日時の変数でなければ None。
    pub fn value(&self, name: &str) -> Option<String> {
        if let Some(k) = NOW_INT_VARS.iter().position(|v| *v == name) {
            return Some(self.parts[k].to_string());
        }
        match name {
            "now_date" => Some(self.date.clone()),
            "now_time" => Some(self.time.clone()),
            _ => None,
        }
    }
}

impl From<chrono::NaiveDateTime> for RunTime {
    fn from(t: chrono::NaiveDateTime) -> RunTime {
        use chrono::{Datelike, Timelike};
        RunTime {
            at: t,
            parts: [
                t.year() as i64,
                t.month() as i64,
                t.day() as i64,
                t.hour() as i64,
                t.minute() as i64,
                t.second() as i64,
            ],
            date: t.format("%Y%m%d").to_string(),
            time: t.format("%H%M%S").to_string(),
        }
    }
}

/// レベルごとに使える整数の変数。細かいレベルの番号は、粗いレベルのタグには使えない。
pub fn vars_for(level: Level) -> Vec<&'static str> {
    let upto = match level {
        Level::Patient => 3,
        Level::Study => 6,
        Level::Series => 9,
        Level::Image => 14,
    };
    // BUILTIN_VARS は患者 → 検査 → シリーズ → 画像の順に並べてある
    BUILTIN_VARS[..upto].to_vec()
}

/// レベルごとに使える、元ファイルの値の変数。
pub fn src_vars_for(level: Level) -> Vec<&'static str> {
    let upto = match level {
        Level::Patient => 4,
        Level::Study => 6,
        Level::Series | Level::Image => 9,
    };
    SRC_VARS[..upto].to_vec()
}

/// DICOM の標準でのタグのレベル(規則のレベルを決める根拠ではなく、食い違いを警告するために使う)。
/// 患者の属性のうち年齢・身長・体重は、標準では検査ごとの属性。
pub fn standard_level(tag: Tag) -> Level {
    const STUDY: &[Tag] = &[
        Tag(0x0008, 0x0020), // StudyDate
        Tag(0x0008, 0x0030), // StudyTime
        Tag(0x0008, 0x0050), // AccessionNumber
        Tag(0x0008, 0x0080), // InstitutionName
        Tag(0x0008, 0x0090), // ReferringPhysicianName
        Tag(0x0008, 0x1030), // StudyDescription
        Tag(0x0008, 0x1048), // PhysiciansOfRecord
        Tag(0x0008, 0x1060), // NameOfPhysiciansReadingStudy
        Tag(0x0020, 0x0010), // StudyID
    ];
    const SERIES: &[Tag] = &[
        Tag(0x0008, 0x0021), // SeriesDate
        Tag(0x0008, 0x0031), // SeriesTime
        Tag(0x0008, 0x0060), // Modality
        Tag(0x0008, 0x0070), // Manufacturer
        Tag(0x0008, 0x1010), // StationName
        Tag(0x0008, 0x103E), // SeriesDescription
        Tag(0x0008, 0x1070), // OperatorsName
        Tag(0x0008, 0x1090), // ManufacturerModelName
        Tag(0x0018, 0x0015), // BodyPartExamined
        Tag(0x0018, 0x1030), // ProtocolName
        Tag(0x0020, 0x0011), // SeriesNumber
    ];
    if matches!(
        tag,
        Tag(0x0010, 0x1010) | Tag(0x0010, 0x1020) | Tag(0x0010, 0x1030)
    ) {
        Level::Study
    } else if tag.group() == 0x0010 {
        Level::Patient
    } else if STUDY.contains(&tag) {
        Level::Study
    } else if SERIES.contains(&tag) {
        Level::Series
    } else {
        Level::Image
    }
}

/// 日本語などを文字コードで符号化する文字列の VR。AE・CS などは ASCII 限りなので含めない。
fn is_text_vr(vr: VR) -> bool {
    matches!(
        vr,
        VR::PN | VR::LO | VR::SH | VR::LT | VR::ST | VR::UT | VR::UC
    )
}

pub const CANCELLED: &str = "中断した";

struct Rule {
    tag: Tag,
    name: String,
    /// 辞書で決まらないタグで、元ファイルにもないときは None
    vr: Option<VR>,
    tpl: Template,
    /// テンプレートの元の文字列(既定のままかを画面が見るため)
    value: String,
    /// このタグが読む CSV(タグごとの指定がなければ全体の CSV)と、1 行を進める単位
    table: Option<Arc<Table>>,
    level: Level,
}

impl Rule {
    /// 値の欄が空。元ファイルにない要素も空で作る
    fn creates_empty(&self) -> bool {
        self.value.is_empty()
    }
}

/// 読んで、複製ごとに同じ結果になる書き換えを済ませた元ファイル。
struct Prepared {
    src: dicom_object::DefaultDicomObject,
    /// 日本語などを自前で符号化するときの文字セットの組
    flavor: Option<Iso2022>,
    /// 触れない(Keep)ときに、新しい値が元ファイルの文字セットで書けるかを調べるための符号化器
    keep_check: Option<(String, Option<SpecificCharacterSet>)>,
    src_inst_no: i64,
    path: PathBuf,
}

/// 1 つのアソシエーションで送るまとまり。
struct Group {
    label: String,
    /// 提案する (SOP Class UID, 転送構文の候補)
    contexts: Vec<(String, Vec<String>)>,
    /// (元ファイル, 木での位置, 送る複製の番号)
    items: Vec<(PathBuf, Place, Vec<Idx>)>,
    count: u64,
}

/// 複製の番号。UID はこれと元の UID から決定的に作る。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Idx {
    pub p: u32,
    pub s: u32,
    pub r: u32,
    pub i: u32,
}

/// 送信の接続 1 本の、いまの状態。画面の「接続の状態」用。
#[derive(Clone, Debug, Default)]
pub struct WorkerStatus {
    /// いま送っているまとまり。何もしていなければ None
    pub group: Option<String>,
    pub done: u64,
    pub total: u64,
    pub started: Option<std::time::Instant>,
}

/// 送り終えたまとまり 1 つの結果。
#[derive(Clone, Debug)]
pub struct GroupResult {
    pub label: String,
    pub count: u64,
    pub sent: u64,
    pub rejected: u64,
    pub secs: f64,
    pub error: Option<String>,
}

/// 実行の進み具合。別スレッドの画面から読む。
#[derive(Default)]
pub struct Progress {
    pub total: AtomicU64,
    pub done: AtomicU64,
    pub cancel: AtomicBool,
    pub log: Mutex<Vec<String>>,
    /// 標準エラーにも出す(コマンドライン用)
    pub echo: bool,
    /// 送信の接続ごとの状態(接続の数だけ)
    pub workers: Mutex<Vec<WorkerStatus>>,
    /// 送り終えたまとまりの結果
    pub results: Mutex<Vec<GroupResult>>,
    /// 接続ごとの端末風のログ(`[  12.345] …`。実行の始まりからの秒)
    pub worker_logs: Mutex<Vec<Vec<String>>>,
    pub started: Mutex<Option<std::time::Instant>>,
}

/// 接続のログに残す行数の上限。1 万件を送ると 1 万行になるので、古いものから捨てる
const WORKER_LOG_LINES: usize = 3000;

impl Progress {
    /// 接続 `k` の端末風のログに 1 行足す。
    pub fn wlog(&self, k: usize, msg: impl Into<String>) {
        let secs = self
            .started
            .lock()
            .unwrap()
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let line = format!("[{secs:8.3}] {}", msg.into());
        let mut logs = self.worker_logs.lock().unwrap();
        if let Some(l) = logs.get_mut(k) {
            if l.len() >= WORKER_LOG_LINES {
                l.drain(..WORKER_LOG_LINES / 10);
            }
            l.push(line);
        }
    }

    pub fn say(&self, msg: impl Into<String>) {
        let msg = msg.into();
        if self.echo {
            eprintln!("{msg}");
        }
        self.log.lock().unwrap().push(msg);
    }
}

pub struct Engine {
    cfg: Config,
    ns: Uuid,
    /// 生成を実行した日時。ファイルごとに取ると、秒をまたいだときに同じ実行の中で値がばらつく
    pub now: RunTime,
    rules: Vec<Rule>,
    /// 全体の CSV の列と行数(画面の表示用)
    pub csv_columns: Vec<String>,
    pub csv_rows: usize,
    tables: HashMap<(String, CsvEncoding), Arc<Table>>,
}

/// CSV の 1 行と、そこを読む複製。画面の割り当て表の 1 行。
#[derive(Clone, Debug, PartialEq)]
pub struct CsvUse {
    /// 列名の行を 1 とした行番号
    pub line: usize,
    /// 先頭の列の値(見分ける手がかり)
    pub head: String,
    /// この行を読む複製の名札(先頭から ASSIGN_LABELS 個まで)
    pub units: Vec<String>,
    /// この行を読む複製の数(数えた範囲の中で)
    pub count: u64,
}

/// 規則 1 本の、CSV の割り当て表。
#[derive(Clone, Debug, PartialEq)]
pub struct CsvAssignment {
    pub level: Level,
    pub rows: Vec<CsvUse>,
    /// そのレベルの単位(複製後の患者など)の総数
    pub units: u64,
    /// 数えた単位の数。総数が ASSIGN_UNIT_LIMIT を超えると、先頭からその数まで
    pub counted: u64,
}

/// 割り当て表で数える単位の上限。画面を作るたびに数えるので、全単位を回すと 100 万画像で固まる。
/// 行は単位の順に 1 つずつ進むので、先頭を数えれば並びは分かる
pub const ASSIGN_UNIT_LIMIT: u64 = 5000;
/// 割り当て表の 1 行に並べる名札の数。
pub const ASSIGN_LABELS: usize = 6;

/// 画面に出す、規則 1 本ぶんの見本。
pub struct PreviewRow {
    pub key: Tag,
    pub tag: String,
    pub name: String,
    pub level: Level,
    pub vr: String,
    pub value: Result<String, String>,
    /// 文字コードで符号化したバイト列(日本語を ISO 2022 にしたときだけ)
    pub encoded: Option<Vec<u8>>,
    /// 値の欄が空の規則(元にない要素も空で作る)
    pub creates_empty: bool,
    pub warnings: Vec<String>,
    /// CSV を使う規則なら、この複製で読む行(列名の行を 1 とした行番号)
    pub csv_line: Option<usize>,
}

impl Engine {
    pub fn new(cfg: &Config) -> Result<Engine> {
        Engine::with_cache(cfg, &mut TableCache::new())
    }

    pub fn with_cache(cfg: &Config, cache: &mut TableCache) -> Result<Engine> {
        if [cfg.patients, cfg.studies, cfg.series, cfg.images].contains(&0) {
            bail!("患者・検査・シリーズ・画像の数は 1 以上にしてほしい");
        }
        let mut tables = HashMap::new();
        let mut table = |path: &str, enc: CsvEncoding| -> Result<Arc<Table>> {
            let t = cached_table(cache, path, enc)?;
            tables.insert((path.to_string(), enc), t.clone());
            Ok(t)
        };
        let global = match cfg.csv.trim() {
            "" => None,
            path => Some(table(path, cfg.csv_encoding)?),
        };

        // 同じタグは後のものが勝つ
        let specs = cfg
            .rules
            .iter()
            .filter(|r| !(r.tag.trim().is_empty() && r.value.is_empty()));
        let mut rules: Vec<Rule> = Vec::new();
        for s in specs {
            let own = match s.csv.trim() {
                "" => None,
                path => Some(
                    table(path, s.csv_encoding.unwrap_or(cfg.csv_encoding))
                        .with_context(|| s.tag.clone())?,
                ),
            };
            let r = parse_rule(s, own.or_else(|| global.clone()))?;
            rules.retain(|x| x.tag != r.tag);
            rules.push(r);
        }
        let (csv_columns, csv_rows) = global
            .map(|t| (t.columns.clone(), t.rows.len()))
            .unwrap_or_default();
        Ok(Engine {
            cfg: cfg.clone(),
            // ランダムでも、名前空間だけを実行ごとに引き、UID は元の UID と番号から引いた名前空間で作る。
            // UID を 1 つずつ引くと、別のファイルから同じ UID を指す参照(Study・Series の UID、
            // ReferencedSOPInstanceUID)がばらばらになり、検査やシリーズのまとまりが崩れるため
            ns: match cfg.uid_mode {
                UidMode::Derived => Uuid::NAMESPACE_OID,
                UidMode::Random => Uuid::new_v4(),
            },
            now: RunTime::now(),
            rules,
            csv_columns,
            csv_rows,
            tables,
        })
    }

    /// タグごとの CSV の列と行数(画面の表示用)。読み込めていなければ None。
    pub fn table_info(&self, path: &str, enc: CsvEncoding) -> Option<(Vec<String>, usize)> {
        self.tables
            .get(&(path.trim().to_string(), enc))
            .map(|t| (t.columns.clone(), t.rows.len()))
    }

    fn uid(&self, orig: &str, level: UidLevel, ix: Idx) -> String {
        let key = match level {
            UidLevel::Study | UidLevel::Frame => format!("{orig}|{}|{}", ix.p, ix.s),
            UidLevel::Series => format!("{orig}|{}|{}|{}", ix.p, ix.s, ix.r),
            UidLevel::Instance => format!("{orig}|{}|{}|{}|{}", ix.p, ix.s, ix.r, ix.i),
        };
        format!(
            "{}.{}",
            self.cfg.uid_root.trim(),
            Uuid::new_v5(&self.ns, key.as_bytes()).as_u128()
        )
    }

    /// 通し番号(0 始まり)。元の画像の通し番号 → 患者 → 検査 → シリーズ → 画像の複製の順で、全出力で一意。
    fn serial(&self, ix: Idx, ordinal: u64) -> u64 {
        let c = &self.cfg;
        (((ordinal * c.patients as u64 + ix.p as u64) * c.studies as u64 + ix.s as u64)
            * c.series as u64
            + ix.r as u64)
            * c.images as u64
            + ix.i as u64
    }

    fn csv_row<'a>(&self, rule: &'a Rule, ix: Idx, place: &Place) -> Option<&'a Row> {
        let t = rule.table.as_deref()?;
        Some(&t.rows[self.csv_index(rule, ix, place) % t.rows.len()])
    }

    fn ints(&self, ix: Idx, place: &Place, src_inst_no: i64) -> Vec<(&'static str, i64)> {
        vec![
            ("p", ix.p as i64),
            ("p1", ix.p as i64 + 1),
            ("P", place.patient as i64),
            ("s", ix.s as i64),
            ("s1", ix.s as i64 + 1),
            ("S", place.study as i64),
            ("r", ix.r as i64),
            ("r1", ix.r as i64 + 1),
            ("R", place.series as i64),
            ("i", ix.i as i64),
            ("i1", ix.i as i64 + 1),
            ("I", place.image as i64),
            ("n", self.serial(ix, place.ordinal) as i64 + 1),
            ("inst", src_inst_no + ix.i as i64 * 10000),
        ]
        .into_iter()
        .chain(NOW_INT_VARS.into_iter().zip(self.now.parts))
        .collect()
    }

    fn strs<'a>(&'a self, place: &'a Place) -> [(&'static str, &'a str); 11] {
        [
            ("now_date", &self.now.date),
            ("now_time", &self.now.time),
            ("src_patient_id", &place.patient_id),
            ("src_patient_name", &place.patient_name),
            ("src_patient_birth_date", &place.patient_birth_date),
            ("src_patient_sex", &place.patient_sex),
            ("src_study_id", &place.study_id),
            ("src_study_description", &place.study_description),
            ("src_series_number", &place.series_number),
            ("src_series_description", &place.series_description),
            ("src_modality", &place.modality),
        ]
    }

    /// 規則を `ix` の複製に当てた値。`csv` は読む CSV の行。
    fn render(
        &self,
        rule: &Rule,
        ix: Idx,
        place: &Place,
        ints: &[(&'static str, i64)],
        strs: &[(&'static str, &str)],
        csv: Option<&Row>,
    ) -> Result<String> {
        let clock = |name: &str, secs: i64, jitter: i64| {
            self.shifted_now(name, secs, jitter, rule.level, ix, place)
        };
        rule.tpl.render(&Vars {
            ints,
            strs,
            csv,
            clock: Some(&clock),
        })
    }

    /// 実行の日時を `secs` 秒ずらし、さらに ±`jitter` 秒の中でランダムにずらした、日時の変数 `name` の値。
    /// ランダムの分は、規則のレベルの単位(検査なら複製後の検査 1 つ)と幅ごとに 1 回だけ決める。
    /// タグで分けないのは、StudyDate と StudyTime に同じ幅を書けば、同じ日時を指すようにするため
    fn shifted_now(
        &self,
        name: &str,
        secs: i64,
        jitter: i64,
        level: Level,
        ix: Idx,
        place: &Place,
    ) -> Result<String> {
        let random = if jitter > 0 {
            let key = format!(
                "now~{jitter}|{level:?}|{}",
                self.unit_index(level, ix, place)
            );
            let h = Uuid::new_v5(&self.ns, key.as_bytes()).as_u128();
            (h % (2 * jitter as u128 + 1)) as i128 - jitter as i128
        } else {
            0
        };
        let total = i64::try_from(secs as i128 + random)
            .ok()
            .and_then(chrono::TimeDelta::try_seconds)
            .context("日時のずらしが大きすぎる")?;
        let t = self
            .now
            .at
            .checked_add_signed(total)
            .filter(|t| (1..=9999).contains(&chrono::Datelike::year(t)))
            .context("ずらした日時が西暦 1〜9999 年の外になる")?;
        RunTime::from(t)
            .value(name)
            .with_context(|| format!("`{name}` はずらせない(ずらせるのは日時の変数だけ)"))
    }

    /// 元ファイルを読まずに、規則を `ix` の複製(元の木では `place`)に当てた値を見せる。
    pub fn preview(&self, ix: Idx, place: &Place) -> Vec<PreviewRow> {
        let ints = self.ints(ix, place, 1);
        let strs = self.strs(place);
        let japanese = Iso2022::japanese();
        self.rules
            .iter()
            .map(|r| {
                let value = self
                    .render(r, ix, place, &ints, &strs, self.csv_row(r, ix, place))
                    .map_err(|e| format!("{e:#}"));
                let vr = r.vr;
                let mut warnings = Vec::new();
                if r.level > standard_level(r.tag) {
                    warnings.push(format!(
                        "標準では{}レベルのタグ。{}レベルにすると、同じ{}の中で値がばらつく",
                        standard_level(r.tag).label(),
                        r.level.label(),
                        standard_level(r.tag).label()
                    ));
                }
                let mut encoded = None;
                if let (Ok(v), Some(vr)) = (&value, vr) {
                    warnings.extend(check_value(vr, v));
                    if self.cfg.charset == Charset::Iso2022jp && is_text_vr(vr) && !v.is_ascii() {
                        match japanese.encode(v) {
                            Ok(b) => encoded = Some(b),
                            Err(e) => warnings.push(format!("{e:#}")),
                        }
                    }
                }
                PreviewRow {
                    key: r.tag,
                    tag: r.tag.to_string(),
                    name: r.name.clone(),
                    level: r.level,
                    vr: vr
                        .map(|v| v.to_string().to_string())
                        .unwrap_or_else(|| "?".into()),
                    value,
                    encoded,
                    creates_empty: r.creates_empty(),
                    warnings,
                    csv_line: self.csv_line(r, ix, place),
                }
            })
            .collect()
    }

    pub fn sources(&self) -> Result<Vec<PathBuf>> {
        list_sources(&self.cfg.input)
    }

    /// タグに規則があれば、`ix` の複製(元の木では `place`)での値。なければ None(元ファイルの値のまま)。
    /// 画面が複製後の木のラベル(患者 ID・名前・検査の説明など)を出すのに使う。
    pub fn render_rule(&self, tag: Tag, ix: Idx, place: &Place) -> Option<Result<String, String>> {
        let r = self.rules.iter().find(|r| r.tag == tag)?;
        let ints = self.ints(ix, place, 1);
        let strs = self.strs(place);
        Some(
            self.render(r, ix, place, &ints, &strs, self.csv_row(r, ix, place))
                .map_err(|e| format!("{e:#}")),
        )
    }

    /// タグの規則が CSV を読むなら、CSV の各行をどの複製(そのレベルの単位)が読むか。
    pub fn csv_assignment(&self, tag: Tag, tree: &SourceTree) -> Option<CsvAssignment> {
        let rule = self.rules.iter().find(|r| r.tag == tag)?;
        let table = rule.table.as_deref()?;
        if !rule.tpl.uses_csv(&table.columns) || table.rows.is_empty() {
            return None;
        }
        let mut rows: Vec<CsvUse> = table
            .rows
            .iter()
            .enumerate()
            .map(|(k, row)| CsvUse {
                line: k + 2,
                head: table
                    .columns
                    .iter()
                    .take(3)
                    .map(|c| row.get(c).map(String::as_str).unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join(" / "),
                units: Vec::new(),
                count: 0,
            })
            .collect();
        let mut counted = 0;
        for (ix, place) in self
            .units(rule.level, tree)
            .take(ASSIGN_UNIT_LIMIT as usize)
        {
            let u = &mut rows[self.csv_index(rule, ix, &place) % table.rows.len()];
            u.count += 1;
            if u.units.len() < ASSIGN_LABELS {
                u.units.push(self.unit_label(rule.level, ix, &place));
            }
            counted += 1;
        }
        let c = tree.planned(
            self.cfg.patients,
            self.cfg.studies,
            self.cfg.series,
            self.cfg.images,
        );
        let units = match rule.level {
            Level::Patient => c.patients,
            Level::Study => c.studies,
            Level::Series => c.series,
            Level::Image => c.images,
        };
        Some(CsvAssignment {
            level: rule.level,
            rows,
            units,
            counted,
        })
    }

    /// `level` の単位(複製後の患者など)を、CSV の行が進む順(unit_index の順)に並べる。
    /// 単位より細かい番号は 0、場所はその単位の最初の画像。
    fn units<'a>(
        &'a self,
        level: Level,
        tree: &'a SourceTree,
    ) -> impl Iterator<Item = (Idx, Place)> + 'a {
        let c = &self.cfg;
        let (np, ns, nr, ni) = (c.patients, c.studies, c.series, c.images);
        let depth = level as usize;
        let copies = move |p: u32| {
            (0..if depth >= 1 { ns } else { 1 }).flat_map(move |s| {
                (0..if depth >= 2 { nr } else { 1 }).flat_map(move |r| {
                    (0..if depth >= 3 { ni } else { 1 }).map(move |i| Idx { p, s, r, i })
                })
            })
        };
        tree.patients.iter().enumerate().flat_map(move |(pi, pt)| {
            let sources: Vec<Place> = match level {
                Level::Patient => tree.place_of(pi, 0, 0).into_iter().collect(),
                _ => pt
                    .studies
                    .iter()
                    .enumerate()
                    .flat_map(|(si, st)| {
                        let series: Vec<usize> = match level {
                            Level::Study => vec![0],
                            _ => (0..st.series.len()).collect(),
                        };
                        series.into_iter().flat_map(move |ri| {
                            let images = match level {
                                Level::Image => st.series[ri].images,
                                _ => 1,
                            };
                            (0..images).filter_map(move |ii| {
                                tree.place_of(pi, si, ri).map(|p| Place {
                                    image: ii,
                                    ordinal: p.ordinal + ii as u64,
                                    ..p
                                })
                            })
                        })
                    })
                    .collect(),
            };
            sources.into_iter().flat_map(move |place| {
                (0..np).flat_map(move |p| {
                    let place = place.clone();
                    copies(p).map(move |ix| (ix, place.clone()))
                })
            })
        })
    }

    /// 割り当て表の名札。複製後の患者 ID(規則があれば)と、元(大文字)と複製(小文字)の番号。
    fn unit_label(&self, level: Level, ix: Idx, place: &Place) -> String {
        let id = match self.render_rule(tags::PATIENT_ID, ix, place) {
            Some(Ok(v)) => v,
            _ => place.patient_id.clone(),
        };
        let mut parts = vec![format!("P{} p{}", place.patient, ix.p)];
        if level >= Level::Study {
            parts.push(format!("S{} s{}", place.study, ix.s));
        }
        if level >= Level::Series {
            parts.push(format!("R{} r{}", place.series, ix.r));
        }
        if level >= Level::Image {
            parts.push(format!("I{} i{}", place.image, ix.i));
        }
        format!("{id}({})", parts.join(" · "))
    }

    /// InstanceNumber の規則が既定(`元の番号 + 複製番号 × 10000`)のままか。
    /// そのときだけ、画面が複製後の画像の番号の範囲を元の範囲から計算できる。
    pub fn instance_number_is_default(&self) -> bool {
        self.rules
            .iter()
            .find(|r| r.tag == tags::INSTANCE_NUMBER)
            .is_some_and(|r| r.value == "%(inst)d")
    }

    /// 規則が読む CSV の、`ix` の複製で使う行(1 行目の列名を含めた行番号)。
    fn csv_line(&self, rule: &Rule, ix: Idx, place: &Place) -> Option<usize> {
        let t = rule.table.as_deref()?;
        if !rule.tpl.uses_csv(&t.columns) {
            return None;
        }
        Some(self.csv_index(rule, ix, place) % t.rows.len() + 2)
    }

    /// CSV の行の番号(0 始まり、折り返す前)。そのレベルの単位で、隙間なく 1 つずつ進む。
    /// 元の木での通し番号(患者・検査・シリーズ・画像)を織り込むので、元が違えば必ず別の行になり、
    /// 同じ元の複製どうしも別の行になる。
    fn csv_index(&self, rule: &Rule, ix: Idx, place: &Place) -> usize {
        self.unit_index(rule.level, ix, place) as usize
    }

    /// `level` の単位(複製後の患者・検査・シリーズ・画像)の、全出力での通し番号(0 始まり)。
    fn unit_index(&self, level: Level, ix: Idx, place: &Place) -> u64 {
        let c = &self.cfg;
        let (np, ns, nr) = (c.patients as u64, c.studies as u64, c.series as u64);
        match level {
            Level::Patient => place.patient as u64 * np + ix.p as u64,
            Level::Study => (place.study_ordinal * np + ix.p as u64) * ns + ix.s as u64,
            Level::Series => {
                ((place.series_ordinal * np + ix.p as u64) * ns + ix.s as u64) * nr + ix.r as u64
            }
            Level::Image => self.serial(ix, place.ordinal),
        }
    }

    /// 通し番号(1 始まり)。画面の表示用。
    pub fn serial_number(&self, ix: Idx, place: &Place) -> u64 {
        self.serial(ix, place.ordinal) + 1
    }

    pub fn planned(&self, sources: usize) -> u64 {
        let c = &self.cfg;
        sources as u64 * c.patients as u64 * c.studies as u64 * c.series as u64 * c.images as u64
    }

    /// ディスクに書くか(送るだけのときは書かない)。
    fn writes_files(&self) -> bool {
        !self.cfg.store.enabled || self.cfg.store.also_write
    }

    /// 書き出した(送った)数を返す。中断したときも、そこまでの数を返す。
    pub fn run(&self, progress: &Progress) -> Result<u64> {
        if self.writes_files() && self.cfg.output.trim().is_empty() {
            bail!("出力先を指定してほしい");
        }
        self.preflight()?;
        let files = self.sources()?;
        // 元の木を先に読む。元の患者の番号などを規則の変数にし、送信では提示コンテキストを集めるため
        let tree = tree::scan(&files, &AtomicBool::new(false), &AtomicU64::new(0));
        let c = tree.counts();
        progress
            .total
            .store(self.planned(tree.files), Ordering::Relaxed);
        progress.say(format!(
            "元: {} 患者 / {} 検査 / {} シリーズ / {} 画像({} ファイル) → {} {} 個",
            c.patients,
            c.studies,
            c.series,
            c.images,
            tree.files,
            if self.cfg.store.enabled {
                "送信"
            } else {
                "出力"
            },
            progress.total.load(Ordering::Relaxed)
        ));
        tree::log_skipped(&tree.skipped, |m| progress.say(m));
        tree::log_duplicates(&tree.duplicates, |m| progress.say(m));
        if self.cfg.store.enabled {
            return self.run_store(&tree, &files, progress);
        }
        let result: Result<Vec<u64>> = files
            .par_iter()
            .filter_map(|f| tree.places.get(f).map(|place| (f, place)))
            .map(|(f, place)| self.process_file(f, place, progress))
            .collect();
        match result {
            Ok(v) => Ok(v.iter().sum()),
            Err(e) if e.to_string() == CANCELLED => {
                progress.say(CANCELLED);
                Ok(progress.done.load(Ordering::Relaxed))
            }
            Err(e) => Err(e),
        }
    }

    /// 1 つのアソシエーションで送るまとまり。
    fn groups(&self, tree: &tree::SourceTree, files: &[PathBuf]) -> Vec<Group> {
        use crate::config::AssocUnit;
        let c = &self.cfg;
        let n = c.store.connections.max(1) as u64;
        let mut listed: Vec<(&PathBuf, &Place)> = files
            .iter()
            .filter_map(|f| tree.places.get(f).map(|p| (f, p)))
            .collect();
        listed.sort_by_key(|(_, p)| p.ordinal);
        let mut map: std::collections::BTreeMap<String, Group> = std::collections::BTreeMap::new();
        for (f, place) in listed {
            let mut per_key: std::collections::BTreeMap<String, Vec<Idx>> =
                std::collections::BTreeMap::new();
            for ix in self.combos() {
                let key = match c.store.unit {
                    AssocUnit::Modality => format!(
                        "モダリティ {}",
                        if place.modality.is_empty() {
                            "(なし)"
                        } else {
                            &place.modality
                        }
                    ),
                    AssocUnit::Patient => format!("患者 P{} p{}", place.patient, ix.p),
                    AssocUnit::Study => format!(
                        "検査 P{} p{} S{} s{}",
                        place.patient, ix.p, place.study, ix.s
                    ),
                    AssocUnit::Series => format!(
                        "シリーズ P{} p{} S{} s{} R{} r{}",
                        place.patient, ix.p, place.study, ix.s, place.series, ix.r
                    ),
                    AssocUnit::All => format!("組 {}", self.serial(ix, place.ordinal) % n + 1),
                };
                per_key.entry(key).or_default().push(ix);
            }
            for (key, ixs) in per_key {
                let g = map.entry(key.clone()).or_insert_with(|| Group {
                    label: key,
                    contexts: vec![],
                    items: vec![],
                    count: 0,
                });
                let ctx = (
                    place.sop_class.clone(),
                    store::proposable(&place.transfer_syntax),
                );
                if !g.contexts.contains(&ctx) {
                    g.contexts.push(ctx);
                }
                g.count += ixs.len() as u64;
                g.items.push((f.clone(), place.clone(), ixs));
            }
        }
        map.into_values().collect()
    }

    /// C-STORE で送る。接続の数だけスレッドを立て、各スレッドがグループを 1 つずつ受け持って、
    /// アソシエーションを張り、複製を作りながら送り、送り切ったら解放する。
    fn run_store(
        &self,
        tree: &tree::SourceTree,
        files: &[PathBuf],
        progress: &Progress,
    ) -> Result<u64> {
        let groups = self.groups(tree, files);
        self.send_groups(groups, progress)
    }

    /// 水増しも書き換えもせず、ディレクトリの DICOM をそのまま送る(storescu と同じ)。
    pub fn run_send_as_is(&self, progress: &Progress) -> Result<u64> {
        let dir = self.cfg.store.as_is_dir.trim();
        let files = list_sources(dir).with_context(|| "そのまま送るディレクトリ")?;
        let tree = tree::scan(&files, &AtomicBool::new(false), &AtomicU64::new(0));
        let c = tree.counts();
        progress.total.store(tree.files as u64, Ordering::Relaxed);
        progress.say(format!(
            "そのまま送る: {} 患者 / {} 検査 / {} シリーズ / {} 画像({} ファイル)",
            c.patients, c.studies, c.series, c.images, tree.files
        ));
        tree::log_skipped(&tree.skipped, |m| progress.say(m));
        // 複製の番号は使わないので、全部 0 のまま。単位の分け方(モダリティ・検査…)はそのまま効く
        let one = Engine {
            cfg: Config {
                patients: 1,
                studies: 1,
                series: 1,
                images: 1,
                ..self.cfg.clone()
            },
            ..self.shallow()
        };
        let groups = one.groups(&tree, &files);
        one.send_groups_with(groups, progress, true)
    }

    /// 設定だけを引き継いだ Engine(規則・CSV は空)。そのまま送るときに、件数を 1 にしたものを作るため。
    fn shallow(&self) -> Engine {
        Engine {
            cfg: self.cfg.clone(),
            ns: self.ns,
            now: self.now.clone(),
            rules: vec![],
            csv_columns: vec![],
            csv_rows: 0,
            tables: HashMap::new(),
        }
    }

    fn send_groups(&self, groups: Vec<Group>, progress: &Progress) -> Result<u64> {
        self.send_groups_with(groups, progress, false)
    }

    fn send_groups_with(
        &self,
        groups: Vec<Group>,
        progress: &Progress,
        as_is: bool,
    ) -> Result<u64> {
        let cfg = &self.cfg.store;
        let n = cfg.connections.max(1) as usize;
        *progress.workers.lock().unwrap() = vec![WorkerStatus::default(); n];
        *progress.worker_logs.lock().unwrap() = vec![vec![]; n];
        *progress.started.lock().unwrap() = Some(std::time::Instant::now());
        progress.results.lock().unwrap().clear();
        progress.say(format!(
            "C-STORE {} へ、{} を単位に {} 本のアソシエーション({} 接続まで同時)で送る",
            cfg.target(),
            cfg.unit.label(),
            groups.len(),
            cfg.connections.max(1)
        ));
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results: Vec<Result<store::Stats>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..cfg.connections.max(1) as usize)
                .map(|k| {
                    let (next, groups) = (&next, &groups);
                    scope.spawn(move || {
                        let r = self.store_worker(k, groups, next, progress, as_is);
                        if let Err(e) = &r {
                            progress.say(format!("エラー: 接続 {}: {e:#}", k + 1));
                            // ほかの接続も止める。残りを送っても結果が片手落ちになるため
                            progress.cancel.store(true, Ordering::Relaxed);
                        }
                        r
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|w| {
                    w.join()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("送信のスレッドが途中で止まった")))
                })
                .collect()
        });
        let mut total = store::Stats::default();
        let mut first_err = None;
        let mut cancelled = false;
        for r in results {
            match r {
                Ok(s) => total += s,
                // ほかの接続の失敗を受けて止まった「中断した」より、本当の原因を先に伝える
                Err(e) if e.to_string() == CANCELLED => cancelled = true,
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        progress.say(format!(
            "送信 {} 個、受け側が拒否 {} 個",
            total.sent, total.rejected
        ));
        if let Some(e) = first_err {
            return Err(e);
        }
        if cancelled {
            progress.say(CANCELLED);
            return Ok(total.sent);
        }
        if total.rejected > 0 {
            bail!(
                "{} 個が受け側に拒否された(ログを見てほしい)",
                total.rejected
            );
        }
        Ok(total.sent)
    }

    fn store_worker(
        &self,
        k: usize,
        groups: &[Group],
        next: &std::sync::atomic::AtomicUsize,
        progress: &Progress,
        as_is: bool,
    ) -> Result<store::Stats> {
        let mut total = store::Stats::default();
        loop {
            if progress.cancel.load(Ordering::Relaxed) {
                bail!(CANCELLED);
            }
            let g = next.fetch_add(1, Ordering::Relaxed);
            let Some(group) = groups.get(g) else { break };
            let started = std::time::Instant::now();
            let set_status = |done: u64| {
                if let Some(w) = progress.workers.lock().unwrap().get_mut(k) {
                    *w = WorkerStatus {
                        group: Some(group.label.clone()),
                        done,
                        total: group.count,
                        started: Some(started),
                    };
                }
            };
            set_status(0);
            let r = self.send_group(k, group, progress, as_is, &set_status);
            if let Some(w) = progress.workers.lock().unwrap().get_mut(k) {
                *w = WorkerStatus::default();
            }
            let secs = started.elapsed().as_secs_f64();
            match r {
                Ok(st) => {
                    progress.results.lock().unwrap().push(GroupResult {
                        label: group.label.clone(),
                        count: group.count,
                        sent: st.sent,
                        rejected: st.rejected,
                        secs,
                        error: None,
                    });
                    total += st;
                }
                Err(e) => {
                    progress.results.lock().unwrap().push(GroupResult {
                        label: group.label.clone(),
                        count: group.count,
                        sent: 0,
                        rejected: 0,
                        secs,
                        error: Some(format!("{e:#}")),
                    });
                    return Err(e);
                }
            }
        }
        Ok(total)
    }

    /// 1 まとまりを、アソシエーションを 1 本張って送り切る。
    fn send_group(
        &self,
        k: usize,
        group: &Group,
        progress: &Progress,
        as_is: bool,
        set_status: &dyn Fn(u64),
    ) -> Result<store::Stats> {
        let cfg = &self.cfg.store;
        progress.wlog(
            k,
            format!(
                "A-ASSOCIATE-RQ {} ← {} : {}",
                cfg.target(),
                cfg.calling_ae.trim(),
                group.label
            ),
        );
        for (sop, ts) in &group.contexts {
            progress.wlog(k, format!("  提案 {sop} [{}]", ts.join(", ")));
        }
        let (mut conn, refused) = match store::Connection::open(cfg, &group.contexts) {
            Ok(c) => c,
            Err(e) => {
                progress.wlog(k, format!("A-ASSOCIATE 失敗: {e:#}"));
                return Err(e.context(group.label.clone()));
            }
        };
        progress.wlog(k, format!("A-ASSOCIATE-AC 受諾 ({} 個を送る)", group.count));
        progress.say(format!(
            "接続 {}: {} を張った({} 個、SOP Class {} 種)",
            k + 1,
            group.label,
            group.count,
            group.contexts.len()
        ));
        for r in &refused {
            progress.wlog(k, format!("  拒否された抽象構文 {r}"));
        }
        if !refused.is_empty() {
            progress.say(format!(
                "接続 {}: 受け側が受けつけない SOP Class(送れない): {}",
                k + 1,
                refused.join(", ")
            ));
        }
        let mut done = 0u64;
        for (path, place, ixs) in &group.items {
            // そのまま送るときは、読んだものを書き換えずに送る
            let prepared = if as_is {
                None
            } else {
                self.prepare(path, place, progress)?
            };
            if !as_is && prepared.is_none() {
                continue;
            }
            for &ix in ixs {
                if progress.cancel.load(Ordering::Relaxed) {
                    let _ = conn.close();
                    bail!(CANCELLED);
                }
                let (sop, ds) = match &prepared {
                    Some(p) => self.make(p, place, ix)?,
                    None => {
                        let ds = open_file(path).with_context(|| path.display().to_string())?;
                        let sop = ds
                            .meta()
                            .media_storage_sop_instance_uid()
                            .trim_end_matches(['\0', ' '])
                            .to_string();
                        (sop, ds)
                    }
                };
                if !as_is && self.writes_files() {
                    self.write(&ds, &sop, ix, path)?;
                }
                // ディスクを経由せず、データセットをそのまま送る
                let inst = store::Instance {
                    sop_class: place.sop_class.clone(),
                    sop_instance: sop,
                    object: ds.into_inner(),
                };
                let short = inst
                    .sop_instance
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .chars()
                    .rev()
                    .take(10)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>();
                let outcome = match conn.store(&inst) {
                    Ok(o) => o,
                    Err(e) => {
                        progress.wlog(k, format!("C-STORE-RQ …{short} 送信に失敗: {e:#}"));
                        return Err(e);
                    }
                };
                match outcome {
                    store::Outcome::Sent => {
                        progress.wlog(k, format!("C-STORE-RQ …{short}  → RSP 0000 (成功)"))
                    }
                    store::Outcome::Warned(st) => {
                        progress.wlog(k, format!("C-STORE-RQ …{short}  → RSP {st:04X} (警告)"));
                        progress.say(format!(
                            "警告つきで受けつけた(status {st:04X}): {}",
                            inst.sop_instance
                        ));
                    }
                    store::Outcome::Rejected(st) => {
                        progress.wlog(k, format!("C-STORE-RQ …{short}  → RSP {st:04X} (失敗)"));
                        progress.say(format!(
                            "受け側が拒否した(status {st:04X}): {}",
                            inst.sop_instance
                        ));
                    }
                    store::Outcome::Unsupported => {
                        progress.wlog(
                            k,
                            format!(
                                "C-STORE-RQ …{short}  送れない: 抽象構文 {} が未受諾",
                                inst.sop_class
                            ),
                        );
                        progress.say(format!(
                            "送れない: SOP Class {} を受け側が受けつけない({})",
                            inst.sop_class, inst.sop_instance
                        ));
                    }
                }
                done += 1;
                set_status(done);
                progress.done.fetch_add(1, Ordering::Relaxed);
            }
        }
        let st = conn.close().with_context(|| group.label.clone())?;
        progress.wlog(
            k,
            format!("A-RELEASE 完了: 成功 {} / 拒否 {}", st.sent, st.rejected),
        );
        Ok(st)
    }

    fn combos(&self) -> Vec<Idx> {
        let c = &self.cfg;
        let mut combos =
            Vec::with_capacity((c.patients * c.studies * c.series * c.images) as usize);
        for p in 0..c.patients {
            for s in 0..c.studies {
                for r in 0..c.series {
                    for i in 0..c.images {
                        combos.push(Idx { p, s, r, i });
                    }
                }
            }
        }
        combos
    }

    /// 1 複製を書く。`出力先/P患者/S検査/Rシリーズ/SOPInstanceUID.dcm`。
    fn write(
        &self,
        ds: &dicom_object::DefaultDicomObject,
        sop: &str,
        ix: Idx,
        path: &Path,
    ) -> Result<()> {
        if !is_uid(sop) {
            bail!(
                "SOPInstanceUID {sop:?} は数字と点だけの 1〜64 文字でないので、ファイル名にできない ({})",
                path.display()
            );
        }
        let dir = Path::new(self.cfg.output.trim())
            .join(format!("P{:06}", ix.p))
            .join(format!("S{:03}", ix.s))
            .join(format!("R{:03}", ix.r));
        std::fs::create_dir_all(&dir)?;
        // ファイルへ直接書くと、符号化に失敗したとき途中で切れた壊れたファイルが残る
        let mut buf = Vec::new();
        ds.write_all(&mut buf)
            .with_context(|| path.display().to_string())?;
        std::fs::write(dir.join(format!("{sop}.dcm")), buf)?;
        Ok(())
    }

    /// 書き出す前に、CSV の全行(CSV を使わない規則は 1 回)を規則に当てて、書けない値を見つける。
    /// 10 万ファイルを書いた途中で、CSV の 58 行目の字が書けないと分かるのでは遅いため。
    /// 元ファイルの文字セットに頼る「触れない」は、元ファイルを読むまで分からないので、ここでは調べない。
    fn preflight(&self) -> Result<()> {
        let flavor = (self.cfg.charset == Charset::Iso2022jp).then(Iso2022::japanese);
        let place = Place::default();
        let ints = self.ints(Idx::default(), &place, 1);
        let strs = self.strs(&place);
        for rule in &self.rules {
            let Some(vr) = rule.vr else { continue };
            let rows: Vec<Option<&Row>> = match &rule.table {
                Some(t) if rule.tpl.uses_csv(&t.columns) => t.rows.iter().map(Some).collect(),
                _ => vec![None],
            };
            for (k, row) in rows.into_iter().enumerate() {
                let origin = match (&rule.table, row) {
                    (Some(t), Some(_)) => {
                        format!("CSV の {} 行目(列: {})", k + 2, t.columns.join(" "))
                    }
                    _ => "テンプレート".to_string(),
                };
                let text = self
                    .render(rule, Idx::default(), &place, &ints, &strs, row)
                    .with_context(|| format!("{} {}: {origin}", rule.name, rule.tag))?;
                check_writable(vr, &text, flavor.as_ref())
                    .with_context(|| format!("{} {}: {origin}: `{text}`", rule.name, rule.tag))?;
            }
        }
        Ok(())
    }

    /// ファイルに書く経路。元を 1 回読み、複製を並列に作って書く。
    fn process_file(&self, path: &Path, place: &Place, progress: &Progress) -> Result<u64> {
        let Some(prepared) = self.prepare(path, place, progress)? else {
            return Ok(0);
        };
        let combos = self.combos();
        combos.par_iter().try_for_each(|&ix| -> Result<()> {
            if progress.cancel.load(Ordering::Relaxed) {
                bail!(CANCELLED);
            }
            let (sop, ds) = self.make(&prepared, place, ix)?;
            self.write(&ds, &sop, ix, path)?;
            progress.done.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })?;
        Ok(combos.len() as u64)
    }

    /// 元ファイルを読み、複製ごとに同じ結果になる書き換えを済ませておく。読めなければ None(飛ばす)。
    fn prepare(
        &self,
        path: &Path,
        _place: &Place,
        progress: &Progress,
    ) -> Result<Option<Prepared>> {
        let c = &self.cfg;
        // 木を読めたファイルだけが来るが、念のため読めなければ飛ばす
        let mut src = match open_file(path) {
            Ok(s) => s,
            Err(e) => {
                progress.say(format!("飛ばした: {} ({e})", path.display()));
                progress.total.fetch_sub(
                    c.patients as u64 * c.studies as u64 * c.series as u64 * c.images as u64,
                    Ordering::Relaxed,
                );
                return Ok(None);
            }
        };
        let src_scs = src
            .get(tags::SPECIFIC_CHARACTER_SET)
            .and_then(|e| e.to_str().ok())
            .map(|v| scs_values(&v))
            .unwrap_or_default();
        let multi = is_multi_iso2022(&src_scs);
        let spec: Option<Iso2022> = if multi {
            Iso2022::from_values(&src_scs)
        } else {
            None
        };
        // 日本語などを自前で符号化するときの文字セットの組
        let flavor: Option<Iso2022> = match c.charset {
            Charset::Iso2022jp => Some(Iso2022::japanese()),
            Charset::Keep if multi => Some(
                spec.clone()
                    .with_context(|| unsupported_scs(path, &src_scs))?,
            ),
            _ => None,
        };
        // 触れない(Keep)ときに、新しい値が元ファイルの文字セットで書けるかを調べるための符号化器
        let keep_check = (c.charset == Charset::Keep && !multi).then(|| {
            let code = src_scs
                .first()
                .map(String::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("ISO_IR 6");
            (code.to_string(), SpecificCharacterSet::from_code(code))
        });
        // dicom-rs は複数値の ISO 2022 を先頭の値だけで読み、ESC $ B などが文字列に残る。
        // 触れない(Keep)なら、そのバイト列のまま書き戻せば元どおりなので、読み直すのは書き換えるときだけ
        if multi && c.charset != Charset::Keep {
            let spec = spec
                .as_ref()
                .with_context(|| unsupported_scs(path, &src_scs))?;
            decode_source_text(&mut src, spec)
                .with_context(|| format!("元ファイルの文字列を読めない: {}", path.display()))?;
        }
        // 複製ごとに同じ結果になる書き換えは、複製する前に元に対して 1 回だけ行う。
        // 触れない(Keep)の元ファイルの文字列は、dicom-rs が読んだ文字列をそのバイト列に書き戻すだけで元どおり
        if let (Some(f), Charset::Iso2022jp) = (&flavor, c.charset) {
            reencode_existing(&mut src, f).with_context(|| path.display().to_string())?;
        }
        for rule in &self.rules {
            ensure_private_creator(&mut src, rule.tag);
        }
        let src_inst_no: i64 = src
            .get(tags::INSTANCE_NUMBER)
            .and_then(|e| e.to_str().ok())
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1);
        Ok(Some(Prepared {
            src,
            flavor,
            keep_check,
            src_inst_no,
            path: path.to_path_buf(),
        }))
    }

    /// `ix` の複製を作る。UID を振り直し、規則を当てる。(SOPInstanceUID, データセット) を返す。
    fn make(
        &self,
        prepared: &Prepared,
        place: &Place,
        ix: Idx,
    ) -> Result<(String, dicom_object::DefaultDicomObject)> {
        let c = &self.cfg;
        let path = &prepared.path;
        let mut ds = prepared.src.clone();
        self.rewrite_uids(&mut ds, ix);
        if place.duplicate > 0 {
            // 元で SOPInstanceUID が重なったファイルは、そのままだと複製でも同じ UID・同じファイル名に
            // なって上書きされる。自分の SOPInstanceUID だけを別にし、ほかのファイルからの参照
            // (ReferencedSOPInstanceUID)は、どちらを指していたか分からないので最初のファイルのままにする
            let orig = prepared
                .src
                .get(tags::SOP_INSTANCE_UID)
                .and_then(|e| e.to_str().ok())
                .map(|s| s.trim_end_matches('\0').trim().to_string())
                .unwrap_or_default();
            let key = format!("{orig}#{}", place.duplicate);
            ds.put_str(
                tags::SOP_INSTANCE_UID,
                VR::UI,
                self.uid(&key, UidLevel::Instance, ix),
            );
        }

        if let Some(code) = c.charset.code() {
            ds.put_str(tags::SPECIFIC_CHARACTER_SET, VR::CS, code);
        }
        let ints = self.ints(ix, place, prepared.src_inst_no);
        let strs = self.strs(place);
        for rule in &self.rules {
            let text = self
                .render(rule, ix, place, &ints, &strs, self.csv_row(rule, ix, place))
                .with_context(|| format!("{} ({})", rule.tag, path.display()))?;
            // 変数を当てて空になった値は、元ファイルにない要素なら作らない。既定の規則は元の値をそのまま
            // 使うもの(生年月日・説明など)を含むので、元にない要素を空で足すと、複製だけ中身が変わるため。
            // 値の欄そのものが空の規則は、空の要素を作れと書いたものとして作る
            if text.is_empty() && ds.get(rule.tag).is_none() && !rule.creates_empty() {
                continue;
            }
            // 元ファイルにある要素はその VR を、なければ辞書か規則の VR を使う。暗黙 VR のファイルでは
            // 辞書にないタグが UN で読まれるので、UN は「決まっていない」として扱う
            let source_vr = ds.get(rule.tag).map(|e| e.vr()).filter(|v| *v != VR::UN);
            let Some(vr) = source_vr.or(rule.vr) else {
                bail!("{} の VR が決まらない(元ファイルでは UN)。規則に /LO のように VR を添えてほしい", rule.tag)
            };
            if let Some((code, cs)) = &prepared.keep_check {
                check_keep(vr, &text, code, cs.as_ref())
                    .with_context(|| format!("{} {} ({})", rule.name, rule.tag, path.display()))?;
            }
            put_value(&mut ds, rule.tag, vr, &text, prepared.flavor.as_ref())
                .with_context(|| format!("{} {} ({})", rule.name, rule.tag, path.display()))?;
        }

        let sop = ds
            .get(tags::SOP_INSTANCE_UID)
            .map(|e| e.to_str().map(|s| s.trim_end_matches('\0').to_string()))
            .transpose()?
            .context("SOPInstanceUID がない")?;
        ds.meta_mut().media_storage_sop_instance_uid = sop.clone();
        ds.meta_mut().update_information_group_length();
        Ok((sop, ds))
    }

    /// シーケンスの中まで辿って、UID のタグを置き換える。
    fn rewrite_uids(&self, obj: &mut InMemDicomObject, ix: Idx) {
        let found: Vec<Tag> = obj.tags().collect();
        for tag in found {
            let level = UID_TAGS.iter().find(|(t, _)| *t == tag).map(|(_, l)| *l);
            obj.update_value(tag, |v| match v {
                Value::Primitive(p) => {
                    if let Some(level) = level {
                        let orig = p.to_str().trim_end_matches('\0').trim().to_string();
                        *p = PrimitiveValue::from(self.uid(&orig, level, ix));
                    }
                }
                Value::Sequence(seq) => {
                    for item in seq.items_mut() {
                        self.rewrite_uids(item, ix);
                    }
                }
                _ => {}
            });
        }
    }
}

/// DICOM の UID の形(数字と点だけ、1〜64 文字)か。出力のファイル名に使うので、`..` や `C:` や区切りを
/// 含む値をそのまま使うと出力先の外に書いてしまう。連番にせず UID のままにするのは、同じ UID の
/// 重なりがファイル名の衝突として見えるため。
fn is_uid(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

fn unsupported_scs(path: &Path, values: &[String]) -> String {
    format!(
        "元ファイルの文字セット `{}` は扱えない(先頭が空・IR 6・IR 13・IR 100/101/109/110/126/127/138/144/166 のどれかで、続きが IR 87 かそれらの別のセットの組だけ): {}",
        values.join("\\"),
        path.display()
    )
}

/// dicom-rs が読んだ複数値の ISO 2022 の文字列を、本来の文字列に直す(シーケンスの中も)。
fn decode_source_text(obj: &mut InMemDicomObject, spec: &Iso2022) -> Result<()> {
    let found: Vec<(Tag, VR)> = obj.iter().map(|e| (e.tag(), e.vr())).collect();
    for (tag, vr) in found {
        if vr != VR::SQ && !is_text_vr(vr) {
            continue;
        }
        let mut err: Option<anyhow::Error> = None;
        obj.update_value(tag, |v| match v {
            Value::Sequence(seq) => {
                for item in seq.items_mut() {
                    if let Err(e) = decode_source_text(item, spec) {
                        err = Some(e);
                    }
                }
            }
            Value::Primitive(p) => {
                let decoded: Result<Vec<String>> = p
                    .to_multi_str()
                    .iter()
                    .map(|t| match spec.raw_bytes_of(t) {
                        // ESC がなければ、dicom-rs が先頭の値で読んだとおりで正しい
                        Some(b) if b.contains(&0x1b) => {
                            spec.decode(&b).with_context(|| format!("{tag}"))
                        }
                        _ => Ok(t.clone()),
                    })
                    .collect();
                match decoded {
                    Ok(mut texts) if texts.len() == 1 => *p = PrimitiveValue::from(texts.remove(0)),
                    Ok(texts) => *p = PrimitiveValue::Strs(texts.into()),
                    Err(e) => err = Some(e),
                }
            }
            _ => {}
        });
        if let Some(e) = err {
            return Err(e);
        }
    }
    Ok(())
}

/// プライベートタグ(奇数グループ)は、その持ち主を示す要素(グループ, 0x00NN)が先にないと規格に合わない。
/// 元ファイルになければ足す。
fn ensure_private_creator(obj: &mut InMemDicomObject, tag: Tag) {
    if tag.group() % 2 == 1 && tag.element() >= 0x1000 {
        let creator = Tag(tag.group(), tag.element() >> 8);
        if obj.get(creator).is_none() {
            obj.put_str(creator, VR::LO, "DicoAmplio");
        }
    }
}

/// 元ファイルにすでにある日本語などの文字列を、ISO 2022 のバイト列に直す。
/// dicom-rs は複数値の文字セットを切り替えて書けないので、文字セットを替えたぶんは自分で符号化する。
fn reencode_existing(obj: &mut InMemDicomObject, flavor: &Iso2022) -> Result<()> {
    let found: Vec<(Tag, VR)> = obj.iter().map(|e| (e.tag(), e.vr())).collect();
    for (tag, vr) in found {
        if vr == VR::SQ {
            let mut err = None;
            obj.update_value(tag, |v| {
                if let Value::Sequence(seq) = v {
                    for item in seq.items_mut() {
                        if let Err(e) = reencode_existing(item, flavor) {
                            err = Some(e);
                        }
                    }
                }
            });
            if let Some(e) = err {
                return Err(e);
            }
        } else if is_text_vr(vr) {
            let Some(text) = obj.get(tag).and_then(|e| match e.value() {
                Value::Primitive(p) => Some(p.to_str().into_owned()),
                _ => None,
            }) else {
                continue;
            };
            if !text.is_ascii() {
                let bytes = flavor
                    .encode(&text)
                    .with_context(|| format!("元ファイルの {tag}: `{text}`"))?;
                obj.put(DataElement::new(tag, vr, PrimitiveValue::U8(bytes.into())));
            }
        }
    }
    Ok(())
}

/// 数値の VR は数として、日本語を ISO 2022 にするときは符号化したバイト列として、
/// それ以外は文字列として入れる。
fn put_value(
    obj: &mut InMemDicomObject,
    tag: Tag,
    vr: VR,
    v: &str,
    flavor: Option<&Iso2022>,
) -> Result<()> {
    let num = |v: &str| -> Result<f64> {
        v.trim()
            .parse::<f64>()
            .with_context(|| format!("{tag} は数値が要る: `{v}`"))
    };
    let el = |value: PrimitiveValue| DataElement::new(tag, vr, value);
    match vr {
        VR::US => obj.put(el(PrimitiveValue::from(num(v)? as u16))),
        VR::UL => obj.put(el(PrimitiveValue::from(num(v)? as u32))),
        VR::SS => obj.put(el(PrimitiveValue::from(num(v)? as i16))),
        VR::SL => obj.put(el(PrimitiveValue::from(num(v)? as i32))),
        VR::FL => obj.put(el(PrimitiveValue::from(num(v)? as f32))),
        VR::FD => obj.put(el(PrimitiveValue::from(num(v)?))),
        _ if flavor.is_some() && is_text_vr(vr) && !v.is_ascii() => {
            let bytes = flavor
                .unwrap()
                .encode(v)
                .with_context(|| format!("値 `{v}`"))?;
            obj.put(el(PrimitiveValue::U8(bytes.into())))
        }
        _ => obj.put_str(tag, vr, v),
    };
    Ok(())
}

fn parse_rule(spec: &RuleSpec, table: Option<Arc<Table>>) -> Result<Rule> {
    let csv_cols: &[String] = table.as_ref().map(|t| t.columns.as_slice()).unwrap_or(&[]);
    // `タグ` か `タグ/VR`。辞書にないタグ(プライベートなど)は VR が要る
    let (key, vr_text) = match spec.tag.trim().split_once('/') {
        Some((k, v)) => (k.trim(), Some(v.trim())),
        None => (spec.tag.trim(), None),
    };
    let given_vr = match vr_text {
        Some(v) => Some(
            <[u8; 2]>::try_from(v.as_bytes())
                .ok()
                .and_then(VR::from_binary)
                .with_context(|| format!("VR が読めない: `{v}`(LO や PN のような 2 文字)"))?,
        ),
        None => None,
    };
    let entry = StandardDataDictionary.by_expr(key);
    let tag = match entry {
        Some(e) => e.tag(),
        None => key
            .parse::<Tag>()
            .ok()
            .with_context(|| format!("タグ `{key}` が読めない(辞書の名前か (gggg,eeee))"))?,
    };
    if UID_TAGS.iter().any(|(t, _)| *t == tag) || tag == tags::MEDIA_STORAGE_SOP_INSTANCE_UID {
        bail!("{key} は UID として自動で振り直すので、規則では指定できない");
    }
    let vr = given_vr.or(match entry.map(|e| e.vr()) {
        Some(VirtualVr::Exact(vr)) => Some(vr),
        _ => None,
    });
    if entry.is_none() && vr.is_none() {
        bail!("辞書にないタグ {key} は VR が要る。例: {key}/LO");
    }
    let name = entry
        .map(|e| e.alias().to_string())
        .unwrap_or_else(|| tag.to_string());
    let level = spec.level;
    let tpl = Template::parse(&spec.value).with_context(|| key.to_string())?;
    let allowed: Vec<&str> = vars_for(level)
        .into_iter()
        .chain(src_vars_for(level))
        .collect();
    for n in tpl.names() {
        let in_csv = csv_cols.iter().any(|c| c == n);
        let builtin = BUILTIN_VARS.contains(&n) || SRC_VARS.contains(&n);
        let now = NOW_INT_VARS.contains(&n) || NOW_STR_VARS.contains(&n);
        if in_csv && now {
            bail!("{key}: CSV の列名 `{n}` は組み込みの日時の変数と同じで、どちらを指すか決まらない。CSV の列名を変えてほしい");
        }
        if now {
            continue;
        }
        if in_csv && builtin {
            // 書式によって組み込みの番号と CSV の列のどちらが勝つかが変わり、レベルの検査もすり抜ける
            bail!("{key}: CSV の列名 `{n}` は組み込みの変数と同じで、どちらを指すか決まらない。CSV の列名を変えてほしい");
        }
        if in_csv {
            continue;
        }
        if builtin && !allowed.contains(&n) {
            // 患者のタグが画像番号で変わると、同じ患者の画像で値がばらつく
            bail!(
                "{key} は{}レベルのタグなので、変数 `{n}` は使えない(使える変数: {})",
                level.label(),
                allowed.join(" ")
            );
        }
        if !builtin {
            bail!(
                "{key}: 変数 `{n}` がない(CSV の列名か {})",
                allowed.join(" ")
            );
        }
    }
    let ints = vars_for(level);
    for (n, shift) in tpl.shifts() {
        if !(NOW_INT_VARS.contains(&n) || NOW_STR_VARS.contains(&n)) {
            bail!("{key}: `{n}` はずらせない(ずらせるのは now_ で始まる日時の変数だけ)");
        }
        for b in shift.steps.iter().filter_map(|(_, b)| b.as_deref()) {
            if !ints.contains(&b) {
                // 検査のタグの日付が画像番号で進むと、同じ検査の画像で日付がばらつく
                bail!(
                    "{key} は{}レベルのタグなので、ずらしに変数 `{b}` は掛けられない(掛けられる変数: {})",
                    level.label(),
                    ints.join(" ")
                );
            }
        }
    }
    Ok(Rule {
        tag,
        name,
        vr,
        tpl,
        value: spec.value.clone(),
        table,
        level,
    })
}

/// 元のディレクトリにあるファイル(再帰)。DICOM でないものや DICOMDIR も入る。読むときに飛ばして、ログに出す。
/// 出力先にすでにあるファイルの数(出力先がなければ 0)。前の出力と混ざるのを、出力の前に知らせるため。
pub fn count_output_files(output: &str) -> usize {
    WalkDir::new(output.trim())
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .count()
}

/// 出力先の直下の 1 項目。消す前に、何が消えるかを見せるため。
#[derive(Clone, Debug, PartialEq)]
pub struct OutputEntry {
    pub name: String,
    pub dir: bool,
    /// フォルダなら、その中のファイルの数(0 なら空のフォルダ)
    pub files: usize,
    /// DicoAmplio が書いた形(`P` と 6 桁の数字のフォルダ)か
    pub ours: bool,
}

/// 消す前に見せる、出力先の中身。
#[derive(Clone, Debug, PartialEq)]
pub struct OutputSummary {
    /// 実際に消すディレクトリ(リンクなどを辿った先)
    pub path: PathBuf,
    pub entries: Vec<OutputEntry>,
    pub files: usize,
}

impl OutputSummary {
    /// DicoAmplio が書いた形でない項目の数。出力先の指定違いに気づくため
    pub fn foreign(&self) -> usize {
        self.entries.iter().filter(|e| !e.ours).count()
    }
}

/// 出力先の直下に、ファイルか(空でも)フォルダがあるか。
pub fn output_has_entries(output: &str) -> bool {
    std::fs::read_dir(output.trim()).is_ok_and(|mut d| d.next().is_some())
}

/// 消してよい出力先か確かめ、実際に消すディレクトリを返す。
fn cleanable(input: &str, output: &str) -> Result<PathBuf> {
    let out = Path::new(output.trim()).canonicalize()?;
    let input = Path::new(input.trim()).canonicalize().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent()?.canonicalize().ok());
    let home = dirs_home();
    let protected = Protected {
        input: input.as_deref(),
        home: home.as_deref(),
        exe_dir: exe_dir.as_deref(),
        system: &system_dirs(),
    };
    match protected.reason(&out) {
        Some(why) => bail!("{why}ので消さない: {}", out.display()),
        None => Ok(out),
    }
}

/// 消してはいけない場所。出力先の打ち間違いで、元・ホーム・システムを中身ごと消さないため。
struct Protected<'a> {
    input: Option<&'a Path>,
    home: Option<&'a Path>,
    exe_dir: Option<&'a Path>,
    system: &'a [PathBuf],
}

impl Protected<'_> {
    /// `out` を消してはいけない理由。どれも、その場所か、それを中に含む上の階層を止める
    fn reason(&self, out: &Path) -> Option<String> {
        let holds = |p: Option<&Path>| p.is_some_and(|p| p.starts_with(out));
        if out.parent().is_none() {
            return Some("出力先がドライブの直下".into());
        }
        if holds(self.input) {
            return Some("元のディレクトリが出力先の中にある".into());
        }
        if holds(self.home) {
            return Some("出力先がホームか、ホームを含む".into());
        }
        if holds(self.exe_dir) {
            return Some("出力先が DicoAmplio の置き場を含む".into());
        }
        // システムの場所は、その中も止める(C:\Windows\Temp なども消させない)
        self.system
            .iter()
            .find(|s| out.starts_with(s) || s.starts_with(out))
            .map(|s| format!("出力先がシステムの場所({})にかかる", s.display()))
    }
}

/// OS の大事な場所。ないものは除く。
fn system_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = if cfg!(windows) {
        [
            "SystemRoot",
            "windir",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "ProgramData",
        ]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect()
    } else {
        // macOS の /private は /tmp と /var の実体なので入れない(一時フォルダへ出力できなくなる)
        [
            "/bin",
            "/boot",
            "/dev",
            "/etc",
            "/lib",
            "/lib32",
            "/lib64",
            "/opt",
            "/proc",
            "/root",
            "/run",
            "/sbin",
            "/srv",
            "/sys",
            "/usr",
            "/var",
            "/System",
            "/Library",
            "/Applications",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    };
    dirs.retain_mut(|d| match d.canonicalize() {
        Ok(c) => {
            *d = c;
            true
        }
        Err(_) => false,
    });
    dirs
}

/// 出力先の直下の中身。消してよい出力先でなければエラー。
pub fn output_summary(input: &str, output: &str) -> Result<OutputSummary> {
    let path = cleanable(input, output)?;
    let mut entries: Vec<OutputEntry> = std::fs::read_dir(&path)?
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let dir = e.path().is_dir();
            let files = if dir {
                count_output_files(&e.path().to_string_lossy())
            } else {
                1
            };
            let ours = dir
                && name.len() == 7
                && name.starts_with('P')
                && name[1..].chars().all(|c| c.is_ascii_digit());
            OutputEntry {
                name,
                dir,
                files,
                ours,
            }
        })
        .collect();
    // 見慣れないものを先に見せる
    entries.sort_by(|a, b| (a.ours, &a.name).cmp(&(b.ours, &b.name)));
    let files = entries.iter().map(|e| e.files).sum();
    Ok(OutputSummary {
        path,
        entries,
        files,
    })
}

/// 出力先の中身を消す。出力先のディレクトリそのものは残す。消したファイルの数を返す。
pub fn clean_output(input: &str, output: &str) -> Result<u64> {
    if !Path::new(output.trim()).is_dir() {
        return Ok(0);
    }
    let out = cleanable(input, output)?;
    let mut removed = 0;
    for entry in std::fs::read_dir(&out)? {
        let path = entry?.path();
        if path.is_dir() {
            removed += count_output_files(&path.to_string_lossy()) as u64;
            std::fs::remove_dir_all(&path)
        } else {
            removed += 1;
            std::fs::remove_file(&path)
        }
        .with_context(|| format!("消せない: {}", path.display()))?;
    }
    Ok(removed)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .and_then(|p| p.canonicalize().ok())
}

pub fn list_sources(input: &str) -> Result<Vec<PathBuf>> {
    let input = input.trim();
    if input.is_empty() {
        bail!("元のディレクトリを指定してほしい");
    }
    if !Path::new(input).is_dir() {
        bail!("元のディレクトリがない: {input}");
    }
    Ok(WalkDir::new(input)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect())
}

fn cached_table(cache: &mut TableCache, path: &str, enc: CsvEncoding) -> Result<Arc<Table>> {
    let key = (path.to_string(), enc);
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let Some((cached_mtime, t)) = cache.get(&key) {
        if mtime.is_some() && *cached_mtime == mtime {
            return Ok(t.clone());
        }
    }
    let (columns, rows) = load_csv(Path::new(path), enc)?;
    let t = Arc::new(Table { columns, rows });
    cache.insert(key, (mtime, t.clone()));
    Ok(t)
}

/// 規則のタグの書き方(`PatientName`、`(0010,0010)`、`(0029,1010)/LO`)からタグだけを取り出す。
/// 同じタグの規則かを、書き方の違いによらず判定するために使う。
/// 規則の行 1 つの検査の結果。画面が、その行の下にエラーを出すのに使う。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuleCheck {
    pub error: Option<String>,
    /// 下の行が同じタグを持つので、この行は使われない
    pub overridden: bool,
}

/// 規則の行(`cfg.rules` と同じ順)を 1 本ずつ検査する。
/// Engine を作るのは最初のエラーで止まるので、どの行が悪いか、ほかにも悪い行があるかは分からないため。
pub fn check_rules(cfg: &Config, cache: &mut TableCache) -> Vec<RuleCheck> {
    let global = match cfg.csv.trim() {
        "" => Ok(None),
        path => cached_table(cache, path, cfg.csv_encoding).map(Some),
    };
    let blank = |r: &RuleSpec| r.tag.trim().is_empty() && r.value.is_empty();
    let tags: Vec<Option<Tag>> = cfg
        .rules
        .iter()
        .map(|r| (!blank(r)).then(|| tag_of(&r.tag)).flatten())
        .collect();
    cfg.rules
        .iter()
        .enumerate()
        .map(|(k, spec)| {
            if blank(spec) {
                return RuleCheck::default();
            }
            let overridden = tags[k].is_some() && tags[k + 1..].contains(&tags[k]);
            let own = match spec.csv.trim() {
                "" => Ok(None),
                path => cached_table(cache, path, spec.csv_encoding.unwrap_or(cfg.csv_encoding))
                    .map(Some),
            };
            let error = match (own, &global) {
                (Err(e), _) => Some(format!("{e:#}")),
                (Ok(Some(t)), _) => parse_rule(spec, Some(t)).err().map(|e| format!("{e:#}")),
                (Ok(None), Ok(g)) => parse_rule(spec, g.clone()).err().map(|e| format!("{e:#}")),
                // CSV の列が分からないので、列を使う規則を「変数がない」と誤って咎めてしまう
                (Ok(None), Err(_)) => parse_rule(spec, None)
                    .err()
                    .map(|_| "全体の CSV が読めないので、確かめられない".to_string()),
            };
            RuleCheck { error, overridden }
        })
        .collect()
}

pub fn tag_of(key: &str) -> Option<Tag> {
    let key = key.trim().split('/').next()?.trim();
    StandardDataDictionary
        .by_expr(key)
        .map(|e| e.tag())
        .or_else(|| key.parse::<Tag>().ok())
}

fn load_csv(path: &Path, enc: CsvEncoding) -> Result<(Vec<String>, Vec<Row>)> {
    let bytes =
        std::fs::read(path).with_context(|| format!("CSV を開けない: {}", path.display()))?;
    let text = match enc {
        CsvEncoding::Utf8 => String::from_utf8(bytes).context("CSV が UTF-8 として読めない")?,
        CsvEncoding::ShiftJis => encoding_rs::SHIFT_JIS
            .decode_without_bom_handling(&bytes)
            .0
            .into_owned(),
        // Excel の「CSV」は Shift_JIS。UTF-8 として読めればそちらを優先する
        CsvEncoding::Auto => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => encoding_rs::SHIFT_JIS
                .decode_without_bom_handling(e.as_bytes())
                .0
                .into_owned(),
        },
    };
    let text = text.trim_start_matches('\u{feff}');
    let mut rd = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let head: Vec<String> = rd.headers()?.iter().map(str::to_string).collect();
    let mut rows = Vec::new();
    for rec in rd.records() {
        let rec = rec?;
        rows.push(
            head.iter()
                .cloned()
                .zip(rec.iter().map(str::to_string))
                .collect(),
        );
    }
    if rows.is_empty() {
        bail!("CSV にデータ行がない: {}", path.display());
    }
    Ok((head, rows))
}

/// 触れない(Keep)で、値が元ファイルの文字セット(複数値でないもの)で書けるか。
/// 書けない値を dicom-rs に渡すと、書き込みの途中で失敗して「なぜ」が分からないため、先に調べる。
fn check_keep(vr: VR, text: &str, code: &str, cs: Option<&SpecificCharacterSet>) -> Result<()> {
    if !is_text_vr(vr) || text.is_ascii() {
        return Ok(());
    }
    let hint = "文字コードを utf8 か iso2022jp にすると書ける";
    let first_non_ascii = text.chars().find(|c| !c.is_ascii()).unwrap_or('?');
    // 指定なし(ISO_IR 6)は ASCII だけ。dicom-rs の既定の符号化器は ISO 8859-1 で ü などを通してしまうが、
    // 文字セットの指定がないファイルに 0x80 以上を書くのは規格にない
    if code == "ISO_IR 6" {
        bail!("元ファイルには文字セットの指定がなく(ASCII)、`{first_non_ascii}` (U+{:04X}) は書けない。{hint}", first_non_ascii as u32);
    }
    // ISO_IR 13 は JIS X 0201(半角カナまで)。dicom-rs の符号化器は Shift_JIS なので漢字も通してしまうが、規格にない
    if code == "ISO_IR 13" {
        if let Some(c) = text
            .chars()
            .find(|c| !c.is_ascii() && !('\u{FF61}'..='\u{FF9F}').contains(c))
        {
            bail!("元ファイルの文字セット `{code}` は半角カナまでで、`{c}` (U+{:04X}) は書けない。{hint}", c as u32);
        }
        return Ok(());
    }
    if let Some(cs) = cs {
        if cs.encode(text).is_err() {
            let c = text
                .chars()
                .find(|c| cs.encode(&c.to_string()).is_err())
                .unwrap_or('?');
            bail!(
                "元ファイルの文字セット `{code}` では `{c}` (U+{:04X}) が書けない。{hint}",
                c as u32
            );
        }
    }
    Ok(())
}

/// 値をその VR で書けるか。数値の VR は数として読めること、日本語を自前で符号化するときは符号化できること。
fn check_writable(vr: VR, text: &str, flavor: Option<&Iso2022>) -> Result<()> {
    if matches!(vr, VR::US | VR::UL | VR::SS | VR::SL | VR::FL | VR::FD) {
        text.trim()
            .parse::<f64>()
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("{vr} は数値が要る"))?;
    }
    if let Some(f) = flavor {
        if is_text_vr(vr) && !text.is_ascii() {
            f.encode(text)?;
        }
    }
    Ok(())
}

/// 値が DICOM の VR の決まりに収まっているか。収まらなくても書き出しは止めず、見本で知らせる。
pub fn check_value(vr: VR, v: &str) -> Vec<String> {
    let mut w = Vec::new();
    let chars = v.chars().count();
    let mut limit = |name: &str, max: usize| {
        if chars > max {
            w.push(format!("{name} は {max} 文字まで(いま {chars})"));
        }
    };
    match vr {
        VR::LO => limit("LO", 64),
        VR::SH => limit("SH", 16),
        VR::CS => limit("CS", 16),
        VR::AE => limit("AE", 16),
        VR::DS => limit("DS", 16),
        VR::IS => limit("IS", 12),
        VR::UI => limit("UI", 64),
        VR::PN => {
            let groups: Vec<&str> = v.split('=').collect();
            if groups.len() > 3 {
                w.push("PN の成分グループは 3 つまで(字形=表意=表音)".into());
            }
            for g in groups {
                if g.chars().count() > 64 {
                    w.push("PN の 1 グループは 64 文字まで".into());
                }
                if g.split('^').count() > 5 {
                    w.push("PN の成分(^)は 5 つまで".into());
                }
            }
        }
        _ => {}
    }
    if vr == VR::CS
        && !v
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == ' ' || c == '_')
    {
        w.push("CS は大文字・数字・空白・_ だけ".into());
    }
    if vr == VR::DA && !(v.len() == 8 && v.chars().all(|c| c.is_ascii_digit())) {
        w.push("DA は YYYYMMDD".into());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_dicom(
        dir: &Path,
        name: &str,
        pid: &str,
        study: &str,
        series: &str,
        inst: i64,
    ) -> PathBuf {
        let mut obj = InMemDicomObject::new_empty();
        let sop = format!("1.2.3.{pid}.{study}.{series}.{inst}");
        for (tag, vr, v) in [
            (
                tags::SOP_CLASS_UID,
                VR::UI,
                "1.2.840.10008.5.1.4.1.1.7".to_string(),
            ),
            (tags::SOP_INSTANCE_UID, VR::UI, sop.clone()),
            (tags::PATIENT_ID, VR::LO, pid.to_string()),
            (tags::PATIENT_NAME, VR::PN, format!("{pid}^Src")),
            (tags::STUDY_INSTANCE_UID, VR::UI, format!("1.2.3.{study}")),
            (tags::STUDY_DESCRIPTION, VR::LO, format!("desc {study}")),
            (
                tags::SERIES_INSTANCE_UID,
                VR::UI,
                format!("1.2.3.{study}.{series}"),
            ),
            (tags::SERIES_NUMBER, VR::IS, "1".to_string()),
            (tags::MODALITY, VR::CS, "OT".to_string()),
            (tags::INSTANCE_NUMBER, VR::IS, inst.to_string()),
        ] {
            obj.put(DataElement::new(tag, vr, PrimitiveValue::from(v)));
        }
        let meta = dicom_object::FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1")
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.7")
            .media_storage_sop_instance_uid(sop);
        let path = dir.join(name);
        obj.with_meta(meta).unwrap().write_to_file(&path).unwrap();
        path
    }

    /// 元: 患者 A(検査 1 つ、シリーズ 1 つ、画像 2 枚)と患者 B(検査 1 つ、シリーズ 2 つ、画像 1 枚ずつ)。
    fn source_tree(name: &str) -> (PathBuf, SourceTree) {
        let dir = scratch(name);
        let files = vec![
            write_dicom(&dir, "a1.dcm", "A", "10", "1", 1),
            write_dicom(&dir, "a2.dcm", "A", "10", "1", 2),
            write_dicom(&dir, "b1.dcm", "B", "20", "1", 1),
            write_dicom(&dir, "b2.dcm", "B", "20", "2", 1),
        ];
        let t = tree::scan(&files, &AtomicBool::new(false), &AtomicU64::new(0));
        assert_eq!(t.files, 4);
        (dir, t)
    }

    fn write_csv(dir: &Path, rows: usize) -> String {
        let mut text = String::from("name,code\n");
        for k in 0..rows {
            text.push_str(&format!("N{k},C{k}\n"));
        }
        let p = dir.join("people.csv");
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn csv_engine(csv: &str, counts: [u32; 4], rules: &[(&str, Level, &str)]) -> Engine {
        let [patients, studies, series, images] = counts;
        Engine::new(&Config {
            patients,
            studies,
            series,
            images,
            csv: csv.to_string(),
            rules: rules
                .iter()
                .map(|(t, l, v)| RuleSpec::new(t, *l, v))
                .collect(),
            ..Config::default()
        })
        .unwrap()
    }

    /// 元 1 枚(StudyDescription はある、InstitutionName・PatientBirthDate はない)を規則で 1 回書き出す。
    fn write_once(name: &str, rules: &[(&str, Level, &str)]) -> InMemDicomObject {
        let dir = scratch(name);
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_dicom(&src, "a.dcm", "A", "10", "1", 1);
        let out = dir.join("out");
        let mut cfg = Config {
            input: src.to_string_lossy().into_owned(),
            output: out.to_string_lossy().into_owned(),
            ..Config::default()
        };
        cfg.rules
            .extend(rules.iter().map(|(t, l, v)| RuleSpec::new(t, *l, v)));
        let n = Engine::new(&cfg)
            .unwrap()
            .run(&Progress::default())
            .unwrap();
        assert_eq!(n, 1);
        let f = WalkDir::new(&out)
            .into_iter()
            .filter_map(|e| e.ok())
            .find(|e| e.file_type().is_file())
            .unwrap();
        open_file(f.path()).unwrap().into_inner()
    }

    #[test]
    fn only_digits_and_dots_make_a_file_name_from_a_uid() {
        assert!(is_uid("1.2.840.10008.5.1.4.1.1.2"));
        assert!(is_uid("1"));
        assert!(!is_uid(""));
        assert!(!is_uid(&"1".repeat(65)));
        assert!(!is_uid("../../etc/passwd"));
        assert!(!is_uid("C:\\Windows\\x"));
        assert!(!is_uid("1.2.3 "));
        assert!(!is_uid("1.2.3\0"));
    }

    #[test]
    fn rule_with_an_empty_value_creates_the_element_empty_even_when_the_source_lacks_it() {
        let ds = write_once(
            "empty-create",
            &[
                ("InstitutionName", Level::Study, ""),
                ("StudyDescription", Level::Study, ""),
            ],
        );
        let inst = ds.get(tags::INSTITUTION_NAME).expect("元になくても作る");
        assert_eq!(inst.vr(), VR::LO);
        assert_eq!(inst.to_str().unwrap().trim(), "");
        assert_eq!(
            ds.get(tags::STUDY_DESCRIPTION)
                .unwrap()
                .to_str()
                .unwrap()
                .trim(),
            "",
            "元の値は空にする"
        );
    }

    #[test]
    fn value_that_renders_empty_does_not_add_an_element_the_source_lacks() {
        let ds = write_once(
            "empty-render",
            &[(
                "InstitutionName",
                Level::Study,
                "%(src_patient_birth_date)s",
            )],
        );
        assert!(ds.get(tags::INSTITUTION_NAME).is_none());
        // 既定の PatientBirthDate(元の値のまま)も、元にないので作らない
        assert!(ds.get(tags::PATIENT_BIRTH_DATE).is_none());
    }

    #[test]
    fn preview_marks_rules_whose_value_field_is_empty() {
        let cfg = Config {
            rules: vec![
                RuleSpec::new("InstitutionName", Level::Study, ""),
                RuleSpec::new(
                    "StudyDescription",
                    Level::Study,
                    "%(src_study_description)s",
                ),
            ],
            ..Config::default()
        };
        let rows = Engine::new(&cfg)
            .unwrap()
            .preview(Idx::default(), &Place::default());
        assert!(rows[0].creates_empty);
        assert!(!rows[1].creates_empty, "空に描かれるだけの規則は違う");
    }

    #[test]
    fn rule_check_reports_each_bad_row_and_rows_hidden_by_a_later_one() {
        let cfg = Config {
            rules: vec![
                RuleSpec::new("PatientID", Level::Patient, "A%(p)d"),
                RuleSpec::new("NoSuchTag", Level::Patient, "x"),
                RuleSpec::new("StudyDescription", Level::Study, "%(i)d"),
                RuleSpec::new("", Level::Image, ""),
                RuleSpec::new("SeriesDescription", Level::Series, "%(zz)s"),
                RuleSpec::new("PatientID", Level::Patient, "B%(p)d"),
                RuleSpec::new("StudyDate", Level::Study, "%(now_date+1d*i)s"),
            ],
            ..Config::default()
        };
        let checks = check_rules(&cfg, &mut TableCache::new());
        assert_eq!(checks.len(), 7, "行の数と同じだけ返す");
        let err = |k: usize| checks[k].error.clone().unwrap_or_default();
        assert_eq!(checks[0].error, None);
        assert!(checks[0].overridden, "下の PatientID に負ける");
        assert!(err(1).contains("NoSuchTag"), "{}", err(1));
        assert!(err(2).contains("`i`"), "{}", err(2));
        assert_eq!(checks[3], RuleCheck::default(), "空の行は咎めない");
        assert!(err(4).contains("`zz`"), "{}", err(4));
        assert_eq!(
            checks[5],
            RuleCheck::default(),
            "下のものが勝つので、こちらは使われる"
        );
        assert!(err(6).contains("`i`"), "{}", err(6));
    }

    #[test]
    fn rule_check_matches_what_building_the_engine_reports() {
        let cfg = Config {
            rules: vec![RuleSpec::new("StudyDescription", Level::Study, "%(i)d")],
            ..Config::default()
        };
        let engine_err = Engine::new(&cfg).err().unwrap();
        let checks = check_rules(&cfg, &mut TableCache::new());
        assert_eq!(checks[0].error, Some(format!("{engine_err:#}")));
    }

    #[test]
    fn rule_check_blames_only_the_rows_whose_csv_cannot_be_read() {
        let dir = scratch("check-csv");
        let csv = write_csv(&dir, 2);
        let missing = dir.join("none.csv").to_string_lossy().into_owned();
        let mut own = RuleSpec::new("PatientName", Level::Patient, "%(name)s");
        own.csv = missing.clone();
        let cfg = Config {
            csv: csv.clone(),
            rules: vec![
                own,
                RuleSpec::new("PatientComments", Level::Patient, "%(code)s"),
            ],
            ..Config::default()
        };
        let checks = check_rules(&cfg, &mut TableCache::new());
        assert!(checks[0].error.as_deref().unwrap().contains("none.csv"));
        assert_eq!(checks[1].error, None, "全体の CSV の列は使える");

        // 全体の CSV が読めないときは、CSV の列を使う行だけ確かめられないと言う
        let cfg = Config {
            csv: missing,
            rules: vec![
                RuleSpec::new("PatientComments", Level::Patient, "%(code)s"),
                RuleSpec::new("PatientID", Level::Patient, "A%(p)d"),
            ],
            ..Config::default()
        };
        let checks = check_rules(&cfg, &mut TableCache::new());
        assert!(checks[0]
            .error
            .as_deref()
            .unwrap()
            .contains("確かめられない"));
        assert_eq!(checks[1].error, None);
    }

    #[test]
    fn source_values_are_read_from_the_selected_file() {
        let (_dir, t) = source_tree("values");
        let b = t.file_of(1, 0, 1, 0).unwrap();
        assert!(b.ends_with("b2.dcm"), "{}", b.display());
        let v = tree::read_values(b).unwrap();
        assert_eq!(v[&tags::PATIENT_ID], "B");
        assert_eq!(v[&tags::PATIENT_NAME], "B^Src");
        assert_eq!(v[&tags::STUDY_DESCRIPTION], "desc 20");
        assert!(
            !v.contains_key(&tags::PATIENT_BIRTH_DATE),
            "元にないタグは入らない"
        );
        assert!(t.file_of(1, 0, 5, 0).is_none());
    }

    #[test]
    fn preview_rows_carry_their_tag_for_comparing_with_the_source() {
        let eng = Engine::new(&Config::default()).unwrap();
        let rows = eng.preview(Idx::default(), &Place::default());
        let id = rows.iter().find(|r| r.name == "PatientID").unwrap();
        assert_eq!(id.key, tags::PATIENT_ID);
    }

    /// 全出力を回して、その規則で各 CSV の行を読む単位の数を数える(割り当て表と突き合わせる)。
    fn brute_force_counts(eng: &Engine, tag: Tag, t: &SourceTree, rows: usize) -> Vec<u64> {
        let rule = eng.rules.iter().find(|r| r.tag == tag).unwrap();
        let c = &eng.cfg;
        let mut seen = std::collections::HashSet::new();
        let mut counts = vec![0; rows];
        for place in t.places.values() {
            for p in 0..c.patients {
                for s in 0..c.studies {
                    for r in 0..c.series {
                        for i in 0..c.images {
                            let ix = Idx { p, s, r, i };
                            let unit = match rule.level {
                                Level::Patient => vec![place.patient, p as usize],
                                Level::Study => {
                                    vec![place.patient, place.study, p as usize, s as usize]
                                }
                                Level::Series => vec![
                                    place.patient,
                                    place.study,
                                    place.series,
                                    p as usize,
                                    s as usize,
                                    r as usize,
                                ],
                                Level::Image => vec![
                                    place.ordinal as usize,
                                    p as usize,
                                    s as usize,
                                    r as usize,
                                    i as usize,
                                ],
                            };
                            if seen.insert(unit) {
                                counts[eng.csv_line(rule, ix, place).unwrap() - 2] += 1;
                            }
                        }
                    }
                }
            }
        }
        counts
    }

    #[test]
    fn csv_assignment_counts_match_what_the_output_reads_at_every_level() {
        let (dir, t) = source_tree("assign-levels");
        let csv = write_csv(&dir, 5);
        let eng = csv_engine(
            &csv,
            [3, 2, 2, 2],
            &[
                ("PatientName", Level::Patient, "%(name)s"),
                ("StudyDescription", Level::Study, "%(name)s"),
                ("SeriesDescription", Level::Series, "%(name)s"),
                ("ImageComments", Level::Image, "%(code)s"),
            ],
        );
        for (tag, units) in [
            (tags::PATIENT_NAME, 2 * 3),
            (tags::STUDY_DESCRIPTION, 2 * 3 * 2),
            (tags::SERIES_DESCRIPTION, 3 * 3 * 2 * 2),
            (tags::IMAGE_COMMENTS, 4 * 3 * 2 * 2 * 2),
        ] {
            let a = eng.csv_assignment(tag, &t).unwrap();
            assert_eq!((a.units, a.counted), (units, units), "{tag}");
            let counts: Vec<u64> = a.rows.iter().map(|r| r.count).collect();
            assert_eq!(counts, brute_force_counts(&eng, tag, &t, 5), "{tag}");
            assert_eq!(counts.iter().sum::<u64>(), units);
        }
    }

    #[test]
    fn csv_assignment_lists_rows_with_their_values_and_the_copies_reading_them() {
        let (dir, t) = source_tree("assign-labels");
        let csv = write_csv(&dir, 4);
        let eng = csv_engine(
            &csv,
            [3, 1, 1, 1],
            &[
                ("PatientID", Level::Patient, "EVAL%(P)03d-%(p)04d"),
                ("PatientName", Level::Patient, "%(name)s"),
            ],
        );
        let a = eng.csv_assignment(tags::PATIENT_NAME, &t).unwrap();
        assert_eq!(a.level, Level::Patient);
        let lines: Vec<_> = a
            .rows
            .iter()
            .map(|r| (r.line, r.head.as_str(), r.count))
            .collect();
        // 元 A の複製 3 人が 2〜4 行目、元 B の 3 人が 5 行目と、戻って 2・3 行目
        assert_eq!(
            lines,
            [
                (2, "N0 / C0", 2),
                (3, "N1 / C1", 2),
                (4, "N2 / C2", 1),
                (5, "N3 / C3", 1)
            ]
        );
        assert_eq!(
            a.rows[0].units,
            ["EVAL000-0000(P0 p0)", "EVAL001-0001(P1 p1)"]
        );
        assert_eq!(a.rows[3].units, ["EVAL001-0000(P1 p0)"]);
    }

    #[test]
    fn csv_assignment_shows_rows_no_copy_reads() {
        let (dir, t) = source_tree("assign-unused");
        let csv = write_csv(&dir, 5);
        let eng = csv_engine(
            &csv,
            [1, 1, 1, 1],
            &[("PatientName", Level::Patient, "%(name)s")],
        );
        let counts: Vec<u64> = eng
            .csv_assignment(tags::PATIENT_NAME, &t)
            .unwrap()
            .rows
            .iter()
            .map(|r| r.count)
            .collect();
        assert_eq!(counts, [1, 1, 0, 0, 0]);
    }

    #[test]
    fn csv_assignment_labels_name_every_level_down_to_the_rules_own() {
        let (dir, t) = source_tree("assign-image");
        let csv = write_csv(&dir, 100);
        let eng = csv_engine(
            &csv,
            [1, 1, 1, 2],
            &[("ImageComments", Level::Image, "%(code)s")],
        );
        let a = eng.csv_assignment(tags::IMAGE_COMMENTS, &t).unwrap();
        // 元の画像の順 → 画像の複製の順。A の 2 枚目の複製 0 が 3 番目。
        // PatientID の規則がないので、名札は元の患者 ID
        assert_eq!(a.rows[2].units, ["A(P0 p0 · S0 s0 · R0 r0 · I1 i0)"]);
        assert_eq!(a.rows[6].units, ["B(P1 p0 · S0 s0 · R1 r0 · I0 i0)"]);
    }

    #[test]
    fn csv_assignment_keeps_a_few_labels_per_row_and_counts_the_rest() {
        let (dir, t) = source_tree("assign-many");
        let csv = write_csv(&dir, 1);
        let eng = csv_engine(
            &csv,
            [10, 1, 1, 1],
            &[("PatientName", Level::Patient, "%(name)s")],
        );
        let a = eng.csv_assignment(tags::PATIENT_NAME, &t).unwrap();
        assert_eq!(a.rows[0].count, 20);
        assert_eq!(a.rows[0].units.len(), ASSIGN_LABELS);
    }

    #[test]
    fn csv_assignment_counts_only_the_first_units_of_a_huge_output() {
        let (dir, t) = source_tree("assign-huge");
        let csv = write_csv(&dir, 7);
        let eng = csv_engine(
            &csv,
            [100, 10, 10, 10],
            &[("ImageComments", Level::Image, "%(code)s")],
        );
        let a = eng.csv_assignment(tags::IMAGE_COMMENTS, &t).unwrap();
        assert_eq!(a.units, 4 * 100 * 10 * 10 * 10);
        assert_eq!(a.counted, ASSIGN_UNIT_LIMIT);
        assert_eq!(
            a.rows.iter().map(|r| r.count).sum::<u64>(),
            ASSIGN_UNIT_LIMIT
        );
    }

    #[test]
    fn csv_assignment_is_none_without_csv_columns() {
        let (dir, t) = source_tree("assign-none");
        let csv = write_csv(&dir, 3);
        let eng = csv_engine(
            &csv,
            [2, 1, 1, 1],
            &[("PatientName", Level::Patient, "Fixed^%(p1)d")],
        );
        assert!(
            eng.csv_assignment(tags::PATIENT_NAME, &t).is_none(),
            "列を使わない規則"
        );
        assert!(
            eng.csv_assignment(tags::ACCESSION_NUMBER, &t).is_none(),
            "規則がないタグ"
        );
        let eng = csv_engine("", [2, 1, 1, 1], &[]);
        assert!(
            eng.csv_assignment(tags::PATIENT_ID, &t).is_none(),
            "CSV がない"
        );
    }

    #[test]
    fn standard_levels_follow_the_dicom_modules() {
        assert_eq!(standard_level(tags::PATIENT_NAME), Level::Patient);
        assert_eq!(standard_level(tags::PATIENT_WEIGHT), Level::Study);
        assert_eq!(standard_level(tags::STUDY_DESCRIPTION), Level::Study);
        assert_eq!(standard_level(tags::SERIES_NUMBER), Level::Series);
        assert_eq!(standard_level(tags::INSTANCE_NUMBER), Level::Image);
    }

    #[test]
    fn a_coarse_tag_cannot_use_a_finer_variable() {
        let spec = RuleSpec::new("PatientName", Level::Patient, "X%(i)d");
        let err = parse_rule(&spec, None).err().unwrap().to_string();
        assert!(err.contains("患者レベル") && err.contains('i'), "{err}");
        assert!(parse_rule(
            &RuleSpec::new("InstanceNumber", Level::Image, "%(i)d"),
            None
        )
        .is_ok());
    }

    #[test]
    fn csv_rows_advance_per_the_level_of_the_tag() {
        let table = Arc::new(Table {
            columns: vec!["v".into()],
            rows: (0..100)
                .map(|k| [("v".to_string(), k.to_string())].into())
                .collect(),
        });
        let cfg = Config {
            patients: 3,
            studies: 2,
            series: 1,
            images: 1,
            ..Config::default()
        };
        let rule = |level| {
            parse_rule(
                &RuleSpec::new("StudyDescription", level, "%(v)s"),
                Some(table.clone()),
            )
            .unwrap()
        };
        let eng = Engine::new(&cfg).unwrap();
        let at = |r: &Rule, p, s| {
            eng.csv_row(r, Idx { p, s, r: 0, i: 0 }, &Place::default())
                .unwrap()["v"]
                .clone()
        };
        assert_eq!(
            (
                at(&rule(Level::Patient), 2, 1),
                at(&rule(Level::Patient), 2, 0)
            ),
            ("2".into(), "2".into())
        );
        assert_eq!(
            (at(&rule(Level::Study), 2, 1), at(&rule(Level::Study), 0, 1)),
            ("5".into(), "1".into())
        );
        // 元の 2 人目の患者は、1 人目の複製と別の行を読む
        let second = Place {
            patient: 1,
            ..Place::default()
        };
        let at2 = |p| {
            eng.csv_row(
                &rule(Level::Patient),
                Idx {
                    p,
                    s: 0,
                    r: 0,
                    i: 0,
                },
                &second,
            )
            .unwrap()["v"]
                .clone()
        };
        assert_eq!((at2(0), at2(2)), ("3".into(), "5".into()));
    }

    #[test]
    fn source_patients_are_told_apart_by_the_default_rules() {
        let eng = Engine::new(&Config::default()).unwrap();
        let ix = Idx {
            p: 0,
            s: 0,
            r: 0,
            i: 0,
        };
        let first = Place::default();
        let second = Place {
            patient: 1,
            ordinal: 7,
            ..Place::default()
        };
        assert_eq!(
            eng.render_rule(tags::PATIENT_ID, ix, &first)
                .unwrap()
                .unwrap(),
            "EVAL000-0000"
        );
        assert_eq!(
            eng.render_rule(tags::PATIENT_ID, ix, &second)
                .unwrap()
                .unwrap(),
            "EVAL001-0000"
        );
        assert_eq!(
            (
                eng.serial_number(ix, &first),
                eng.serial_number(ix, &second)
            ),
            (1, 8)
        );
    }

    #[test]
    fn a_removed_default_rule_leaves_the_source_value_untouched() {
        let mut cfg = Config::default();
        // 辞書の名前でなくタグで書いても、同じタグなら外れる
        cfg.drop_rules_for("(0010,0020)");
        let eng = Engine::new(&cfg).unwrap();
        let ix = Idx {
            p: 1,
            s: 0,
            r: 0,
            i: 0,
        };
        assert!(eng
            .render_rule(tags::PATIENT_ID, ix, &Place::default())
            .is_none());
        assert!(eng
            .render_rule(tags::PATIENT_NAME, ix, &Place::default())
            .is_some());
    }

    #[test]
    fn source_values_are_available_as_string_variables() {
        let place = Place {
            patient_id: "1001".into(),
            modality: "CT".into(),
            ..Place::default()
        };
        let rule = parse_rule(
            &RuleSpec::new("PatientID", Level::Patient, "%(src_patient_id)s-%(p1)d"),
            None,
        )
        .unwrap();
        let eng = Engine::new(&Config::default()).unwrap();
        let ints = eng.ints(
            Idx {
                p: 1,
                s: 0,
                r: 0,
                i: 0,
            },
            &place,
            1,
        );
        let strs = eng.strs(&place);
        assert_eq!(
            rule.tpl
                .render(&Vars {
                    ints: &ints,
                    strs: &strs,
                    csv: None,
                    clock: None,
                })
                .unwrap(),
            "1001-2"
        );
        // シリーズの値は患者レベルでは使えない
        let err = parse_rule(
            &RuleSpec::new("PatientID", Level::Patient, "%(src_modality)s"),
            None,
        )
        .err()
        .unwrap()
        .to_string();
        assert!(err.contains("患者レベル"), "{err}");
    }

    #[test]
    fn a_csv_column_named_like_a_builtin_variable_is_rejected() {
        let table = Arc::new(Table {
            columns: vec!["n".into()],
            rows: vec![[("n".to_string(), "x".to_string())].into()],
        });
        let err = parse_rule(
            &RuleSpec::new("PatientID", Level::Patient, "%(n)d"),
            Some(table),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(err.contains("列名") && err.contains('n'), "{err}");
    }

    #[test]
    fn keep_rejects_non_ascii_when_the_source_declares_no_character_set() {
        let cs = SpecificCharacterSet::from_code("ISO_IR 6");
        let err = check_keep(VR::PN, "Müller", "ISO_IR 6", cs.as_ref())
            .unwrap_err()
            .to_string();
        assert!(err.contains("ASCII") && err.contains('ü'), "{err}");
        assert!(check_keep(VR::PN, "Mueller", "ISO_IR 6", cs.as_ref()).is_ok());
        let latin = SpecificCharacterSet::from_code("ISO_IR 100");
        assert!(check_keep(VR::PN, "Müller", "ISO_IR 100", latin.as_ref()).is_ok());
    }

    #[test]
    fn csv_line_numbers_count_the_header_and_wrap_around() {
        let table = Arc::new(Table {
            columns: vec!["v".into()],
            rows: (0..3)
                .map(|k| [("v".to_string(), k.to_string())].into())
                .collect(),
        });
        let cfg = Config {
            patients: 5,
            ..Config::default()
        };
        let eng = Engine::new(&cfg).unwrap();
        let rule = parse_rule(
            &RuleSpec::new("PatientName", Level::Patient, "%(v)s"),
            Some(table.clone()),
        )
        .unwrap();
        let line = |p| {
            eng.csv_line(
                &rule,
                Idx {
                    p,
                    s: 0,
                    r: 0,
                    i: 0,
                },
                &Place::default(),
            )
        };
        assert_eq!((line(0), line(2), line(3)), (Some(2), Some(4), Some(2)));
        let no_csv = parse_rule(
            &RuleSpec::new("PatientID", Level::Patient, "%(p)d"),
            Some(table),
        )
        .unwrap();
        assert_eq!(
            eng.csv_line(
                &no_csv,
                Idx {
                    p: 0,
                    s: 0,
                    r: 0,
                    i: 0
                },
                &Place::default()
            ),
            None
        );
    }

    #[test]
    fn a_zero_count_is_refused_before_anything_runs() {
        let err = Engine::new(&Config {
            patients: 0,
            ..Config::default()
        })
        .err()
        .unwrap()
        .to_string();
        assert!(err.contains("1 以上"), "{err}");
    }

    #[test]
    fn tag_of_accepts_every_spelling_of_a_rule_tag() {
        assert_eq!(tag_of("PatientName"), Some(tags::PATIENT_NAME));
        assert_eq!(tag_of("(0010,0010)"), Some(tags::PATIENT_NAME));
        assert_eq!(tag_of("(0029,1010)/LO"), Some(Tag(0x0029, 0x1010)));
        assert_eq!(tag_of("NoSuchTag"), None);
    }

    #[test]
    fn cs_must_be_uppercase() {
        assert!(check_value(VR::CS, "M").is_empty());
        assert!(!check_value(VR::CS, "male").is_empty());
    }

    #[test]
    fn long_names_are_flagged() {
        assert!(!check_value(VR::LO, &"あ".repeat(65)).is_empty());
        assert!(check_value(VR::LO, &"あ".repeat(64)).is_empty());
        assert!(!check_value(VR::PN, "a=b=c=d").is_empty());
    }

    fn uid_of(cfg: &Config, orig: &str, p: u32) -> String {
        let ix = Idx {
            p,
            s: 0,
            r: 0,
            i: 0,
        };
        Engine::new(cfg).unwrap().uid(orig, UidLevel::Instance, ix)
    }

    #[test]
    fn derived_uids_repeat_across_runs_and_random_ones_do_not() {
        let derived = Config {
            uid_mode: UidMode::Derived,
            ..Config::default()
        };
        assert_eq!(uid_of(&derived, "1.2.3", 0), uid_of(&derived, "1.2.3", 0));
        let random = Config::default();
        assert_eq!(random.uid_mode, UidMode::Random, "既定はランダム");
        assert_ne!(uid_of(&random, "1.2.3", 0), uid_of(&random, "1.2.3", 0));
    }

    #[test]
    fn random_uids_keep_references_within_one_run() {
        let eng = Engine::new(&Config::default()).unwrap();
        let ix = Idx {
            p: 1,
            s: 0,
            r: 0,
            i: 0,
        };
        let a = eng.uid("1.2.3", UidLevel::Study, ix);
        assert_eq!(
            a,
            eng.uid("1.2.3", UidLevel::Study, ix),
            "同じ元の UID は同じ新しい UID"
        );
        assert_ne!(a, eng.uid("1.2.4", UidLevel::Study, ix));
        assert!(a.starts_with("2.25.") && a.len() <= 64, "{a}");
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dicoamplio-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cleaning_the_output_removes_its_contents_but_keeps_the_folder() {
        let root = scratch("clean");
        let (input, output) = (root.join("in"), root.join("out"));
        std::fs::create_dir_all(output.join("P000000/S000")).unwrap();
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(output.join("P000000/S000/a.dcm"), b"x").unwrap();
        std::fs::write(output.join("note.txt"), b"x").unwrap();
        let (i, o) = (input.to_string_lossy(), output.to_string_lossy());
        assert_eq!(count_output_files(&o), 2);
        assert_eq!(clean_output(&i, &o).unwrap(), 2);
        assert!(output.is_dir());
        assert_eq!(count_output_files(&o), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_summary_lists_what_would_be_removed_with_foreign_entries_first() {
        let root = scratch("summary");
        let output = root.join("out");
        std::fs::create_dir_all(output.join("P000000/S000")).unwrap();
        std::fs::create_dir_all(output.join("photos")).unwrap();
        std::fs::write(output.join("P000000/S000/a.dcm"), b"x").unwrap();
        std::fs::write(output.join("P000000/S000/b.dcm"), b"x").unwrap();
        std::fs::write(output.join("memo.txt"), b"x").unwrap();
        let o = output.to_string_lossy();
        let s = output_summary("", &o).unwrap();
        let names: Vec<(&str, usize, bool)> = s
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.files, e.ours))
            .collect();
        assert_eq!(
            names,
            [
                ("memo.txt", 1, false),
                ("photos", 0, false),
                ("P000000", 2, true)
            ],
            "空のフォルダも出る"
        );
        assert_eq!((s.files, s.foreign()), (3, 2));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_output_with_only_empty_folders_still_counts_as_not_empty() {
        let root = scratch("empty-dirs");
        let o = root.to_string_lossy();
        assert!(!output_has_entries(&o));
        std::fs::create_dir_all(root.join("P000000")).unwrap();
        assert!(output_has_entries(&o));
        assert_eq!(count_output_files(&o), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn protected_places_and_folders_holding_them_are_never_cleaned() {
        let system = [PathBuf::from("/sys/w"), PathBuf::from("/apps")];
        let p = Protected {
            input: Some(Path::new("/data/src")),
            home: Some(Path::new("/users/ken")),
            exe_dir: Some(Path::new("/tools/dicoamplio")),
            system: &system,
        };
        for out in [
            "/",
            "/data",
            "/data/src",
            "/users",
            "/users/ken",
            "/tools",
            "/sys/w",
            "/sys/w/temp",
            "/sys",
            "/apps",
        ] {
            assert!(p.reason(Path::new(out)).is_some(), "{out} は消さない");
        }
        for out in ["/data/out", "/users/ken/out", "/sys/other"] {
            assert_eq!(p.reason(Path::new(out)), None, "{out} は消してよい");
        }
    }

    #[test]
    fn cleaning_refuses_an_output_that_holds_the_input() {
        let root = scratch("guard");
        let input = root.join("src");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("a.dcm"), b"x").unwrap();
        let err = clean_output(&input.to_string_lossy(), &root.to_string_lossy()).unwrap_err();
        assert!(err.to_string().contains("元のディレクトリ"), "{err}");
        assert!(input.join("a.dcm").exists(), "元は消えない");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_time_variables_give_parts_and_dicom_date_and_time() {
        let t = RunTime::from(
            chrono::NaiveDate::from_ymd_opt(2026, 10, 21)
                .unwrap()
                .and_hms_opt(9, 5, 7)
                .unwrap(),
        );
        assert_eq!(t.parts, [2026, 10, 21, 9, 5, 7]);
        assert_eq!((t.date.as_str(), t.time.as_str()), ("20261021", "090507"));
        let mut eng = Engine::new(&Config::default()).unwrap();
        eng.now = t;
        let place = Place::default();
        let ix = Idx {
            p: 0,
            s: 0,
            r: 0,
            i: 0,
        };
        let ints = eng.ints(ix, &place, 1);
        let strs = eng.strs(&place);
        let render = |v: &str| {
            Template::parse(v)
                .unwrap()
                .render(&Vars {
                    ints: &ints,
                    strs: &strs,
                    csv: None,
                    clock: None,
                })
                .unwrap()
        };
        assert_eq!(render("%(now_date)s-%(now_time)s"), "20261021-090507");
        assert_eq!(
            render("%(now_year)d/%(now_month)02d/%(now_day)02d %(now_hour)02d:%(now_minute)02d:%(now_second)02d"),
            "2026/10/21 09:05:07"
        );
    }

    #[test]
    fn run_time_variables_are_allowed_at_the_patient_level() {
        let spec = RuleSpec::new("PatientComments", Level::Patient, "made %(now_date)s");
        assert!(parse_rule(&spec, None).is_ok());
    }

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, sec: u32) -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, sec)
            .unwrap()
    }

    fn idx(p: u32, s: u32, r: u32, i: u32) -> Idx {
        Idx { p, s, r, i }
    }

    /// 規則 `rules` で、実行の日時を `now` に固めた Engine(UID は派生にして結果を決める)。
    fn clocked(now: chrono::NaiveDateTime, rules: &[(&str, Level, &str)]) -> Engine {
        let cfg = Config {
            patients: 3,
            studies: 30,
            series: 2,
            images: 4,
            uid_mode: UidMode::Derived,
            rules: rules
                .iter()
                .map(|(t, l, v)| RuleSpec::new(t, *l, v))
                .collect(),
            ..Config::default()
        };
        let mut eng = Engine::new(&cfg).unwrap();
        eng.now = RunTime::from(now);
        eng
    }

    fn value(eng: &Engine, tag: Tag, ix: Idx) -> String {
        eng.render_rule(tag, ix, &Place::default())
            .unwrap()
            .unwrap()
    }

    fn parse_dt(date: &str, time: &str) -> chrono::NaiveDateTime {
        chrono::NaiveDateTime::parse_from_str(&format!("{date}{time}"), "%Y%m%d%H%M%S").unwrap()
    }

    #[test]
    fn date_steps_one_day_per_study_across_month_and_year_ends() {
        let eng = clocked(
            at(2026, 12, 30, 12, 0, 0),
            &[("StudyDate", Level::Study, "%(now_date+1d*s)s")],
        );
        let days: Vec<String> = (0..4)
            .map(|s| value(&eng, tags::STUDY_DATE, idx(0, s, 0, 0)))
            .collect();
        assert_eq!(days, ["20261230", "20261231", "20270101", "20270102"]);
        // 患者を替えても、検査の番号が同じなら同じ日
        assert_eq!(value(&eng, tags::STUDY_DATE, idx(2, 2, 0, 0)), "20270101");
    }

    #[test]
    fn date_steps_backwards_and_knows_leap_days() {
        let rules = [("StudyDate", Level::Study, "%(now_date-1d*p)s")];
        let eng = clocked(at(2028, 3, 1, 8, 0, 0), &rules);
        assert_eq!(value(&eng, tags::STUDY_DATE, idx(1, 0, 0, 0)), "20280229");
        let eng = clocked(at(2026, 3, 1, 8, 0, 0), &rules);
        assert_eq!(value(&eng, tags::STUDY_DATE, idx(1, 0, 0, 0)), "20260228");
        assert_eq!(value(&eng, tags::STUDY_DATE, idx(2, 0, 0, 0)), "20260227");
    }

    #[test]
    fn time_stepping_past_midnight_moves_the_date_too() {
        let eng = clocked(
            at(2026, 12, 31, 23, 50, 0),
            &[
                ("ContentDate", Level::Image, "%(now_date+5m*i)s"),
                ("ContentTime", Level::Image, "%(now_time+5m*i)s"),
            ],
        );
        let pair = |i| {
            (
                value(&eng, tags::CONTENT_DATE, idx(0, 0, 0, i)),
                value(&eng, tags::CONTENT_TIME, idx(0, 0, 0, i)),
            )
        };
        assert_eq!(pair(0), ("20261231".into(), "235000".into()));
        assert_eq!(pair(1), ("20261231".into(), "235500".into()));
        assert_eq!(pair(2), ("20270101".into(), "000000".into()));
        assert_eq!(pair(3), ("20270101".into(), "000500".into()));
    }

    #[test]
    fn time_stepping_backwards_past_midnight_moves_the_date_back() {
        let eng = clocked(
            at(2026, 1, 1, 0, 10, 0),
            &[(
                "StudyDescription",
                Level::Image,
                "%(now_date-7m*i)s %(now_time-7m*i)s",
            )],
        );
        assert_eq!(
            value(&eng, tags::STUDY_DESCRIPTION, idx(0, 0, 0, 1)),
            "20260101 000300"
        );
        assert_eq!(
            value(&eng, tags::STUDY_DESCRIPTION, idx(0, 0, 0, 2)),
            "20251231 235600"
        );
    }

    #[test]
    fn shifted_parts_follow_the_shifted_date_and_time() {
        let eng = clocked(
            at(2026, 12, 31, 23, 59, 30),
            &[(
                "StudyDescription",
                Level::Study,
                "%(now_year+45s*s)d-%(now_month+45s*s)02d-%(now_day+45s*s)02d %(now_hour+45s*s)02d:%(now_minute+45s*s)02d:%(now_second+45s*s)02d",
            )],
        );
        assert_eq!(
            value(&eng, tags::STUDY_DESCRIPTION, idx(0, 1, 0, 0)),
            "2027-01-01 00:00:15"
        );
        // ずらさない変数は実行の日時のまま
        let eng = clocked(
            at(2026, 12, 31, 23, 59, 30),
            &[(
                "StudyDescription",
                Level::Study,
                "%(now_date+1d*s)s %(now_date)s %(now_year)d",
            )],
        );
        assert_eq!(
            value(&eng, tags::STUDY_DESCRIPTION, idx(0, 1, 0, 0)),
            "20270101 20261231 2026"
        );
    }

    #[test]
    fn steps_can_combine_several_loop_counts() {
        // 患者ごとに 1 週、検査ごとに 1 日、画像ごとに 30 秒
        let eng = clocked(
            at(2026, 10, 21, 9, 0, 0),
            &[(
                "StudyDescription",
                Level::Image,
                "%(now_date+7d*p+1d*s+30s*i)s %(now_time+7d*p+1d*s+30s*i)s",
            )],
        );
        assert_eq!(
            value(&eng, tags::STUDY_DESCRIPTION, idx(1, 2, 0, 3)),
            "20261030 090130"
        );
    }

    /// 検査レベルで ±12 時間のランダムにした、全検査・全画像の (検査の番号, 画像の番号, 日時)。
    fn random_study_times(eng: &Engine) -> Vec<(u32, (u32, u32, u32), chrono::NaiveDateTime)> {
        let mut out = Vec::new();
        for p in 0..3 {
            for s in 0..30 {
                for r in 0..2 {
                    for i in 0..4 {
                        let ix = idx(p, s, r, i);
                        let t = parse_dt(
                            &value(eng, tags::STUDY_DATE, ix),
                            &value(eng, tags::STUDY_TIME, ix),
                        );
                        out.push((p * 30 + s, (p, r, i), t));
                    }
                }
            }
        }
        out
    }

    const RANDOM_STUDY_RULES: [(&str, Level, &str); 3] = [
        ("StudyDate", Level::Study, "%(now_date~12h)s"),
        ("StudyTime", Level::Study, "%(now_time~12h)s"),
        (
            "StudyDescription",
            Level::Study,
            "%(now_date~12h)s %(now_hour~12h)02d%(now_minute~12h)02d%(now_second~12h)02d",
        ),
    ];

    #[test]
    fn random_shift_stays_within_its_range() {
        let now = at(2026, 10, 21, 23, 0, 0);
        let eng = clocked(now, &RANDOM_STUDY_RULES);
        let times = random_study_times(&eng);
        for (_, _, t) in &times {
            assert!((*t - now).abs() <= chrono::TimeDelta::hours(12), "{t}");
        }
        // 23 時 ± 12 時間なので、日付をまたいだ検査も、またがない検査もある
        let dates: std::collections::BTreeSet<_> = times.iter().map(|(_, _, t)| t.date()).collect();
        assert!(dates.len() >= 2, "{dates:?}");
    }

    #[test]
    fn random_shift_is_the_same_for_every_image_of_a_study() {
        let eng = clocked(at(2026, 10, 21, 9, 0, 0), &RANDOM_STUDY_RULES);
        let mut per_study: HashMap<u32, chrono::NaiveDateTime> = HashMap::new();
        for (study, ix, t) in random_study_times(&eng) {
            let first = *per_study.entry(study).or_insert(t);
            assert_eq!(first, t, "検査 {study} の画像 {ix:?}");
        }
        // 検査どうしでは違う
        let distinct: std::collections::BTreeSet<_> = per_study.values().collect();
        assert!(distinct.len() > 80, "90 検査で {} 通り", distinct.len());
    }

    #[test]
    fn random_shift_is_shared_by_date_and_time_tags_with_the_same_range() {
        let eng = clocked(at(2026, 10, 21, 23, 0, 0), &RANDOM_STUDY_RULES);
        for s in 0..30 {
            let ix = idx(1, s, 0, 0);
            assert_eq!(
                value(&eng, tags::STUDY_DESCRIPTION, ix),
                format!(
                    "{} {}",
                    value(&eng, tags::STUDY_DATE, ix),
                    value(&eng, tags::STUDY_TIME, ix)
                )
            );
        }
    }

    #[test]
    fn random_shift_reaches_both_ends_of_a_small_range() {
        // ±1 秒なら -1 0 +1 の 3 通りだけで、3 通りとも出る
        let now = at(2026, 10, 21, 9, 0, 0);
        let eng = clocked(now, &[("ContentTime", Level::Image, "%(now_time~1s)s")]);
        let mut seen = std::collections::BTreeSet::new();
        for s in 0..30 {
            for i in 0..4 {
                let time = value(&eng, tags::CONTENT_TIME, idx(0, s, 0, i));
                seen.insert((parse_dt("20261021", &time) - now).num_seconds());
            }
        }
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), [-1, 0, 1]);
    }

    #[test]
    fn random_shift_is_drawn_per_unit_of_the_tags_level() {
        // 画像レベルなら、同じ検査の中でも画像ごとに違う
        let eng = clocked(
            at(2026, 10, 21, 9, 0, 0),
            &[("ContentTime", Level::Image, "%(now_time~12h)s")],
        );
        let times: std::collections::BTreeSet<_> = (0..4)
            .map(|i| value(&eng, tags::CONTENT_TIME, idx(0, 0, 0, i)))
            .collect();
        assert!(times.len() > 1, "{times:?}");
        // 患者レベルなら、検査が違っても同じ患者なら同じ
        let eng = clocked(
            at(2026, 10, 21, 9, 0, 0),
            &[("PatientComments", Level::Patient, "%(now_time~12h)s")],
        );
        let comments = tags::PATIENT_COMMENTS;
        let first = value(&eng, comments, idx(1, 0, 0, 0));
        for s in 0..30 {
            assert_eq!(value(&eng, comments, idx(1, s, 1, 3)), first);
        }
        assert_ne!(value(&eng, comments, idx(0, 0, 0, 0)), first);
    }

    #[test]
    fn random_shift_differs_between_source_studies_copied_with_the_same_numbers() {
        let eng = clocked(at(2026, 10, 21, 9, 0, 0), &RANDOM_STUDY_RULES);
        let values: std::collections::BTreeSet<_> = (0..20)
            .map(|k| {
                let place = Place {
                    study_ordinal: k,
                    ..Place::default()
                };
                eng.render_rule(tags::STUDY_DESCRIPTION, idx(0, 0, 0, 0), &place)
                    .unwrap()
                    .unwrap()
            })
            .collect();
        assert!(values.len() > 15, "{values:?}");
    }

    #[test]
    fn random_shift_adds_to_fixed_steps() {
        let now = at(2026, 10, 21, 9, 0, 0);
        let eng = clocked(
            now,
            &[
                ("StudyDate", Level::Study, "%(now_date+1d*s~30m)s"),
                ("StudyTime", Level::Study, "%(now_time+1d*s~30m)s"),
            ],
        );
        for s in 0..30 {
            let ix = idx(0, s, 0, 0);
            let t = parse_dt(
                &value(&eng, tags::STUDY_DATE, ix),
                &value(&eng, tags::STUDY_TIME, ix),
            );
            let off = t - now - chrono::TimeDelta::days(s as i64);
            assert!(
                off.abs() <= chrono::TimeDelta::minutes(30),
                "検査 {s}: {off}"
            );
        }
    }

    #[test]
    fn random_shift_repeats_with_derived_uids_and_changes_with_random_ones() {
        let now = at(2026, 10, 21, 9, 0, 0);
        let all = |eng: &Engine| -> Vec<String> {
            (0..30)
                .map(|s| value(eng, tags::STUDY_DESCRIPTION, idx(0, s, 0, 0)))
                .collect()
        };
        let a = clocked(now, &RANDOM_STUDY_RULES);
        assert_eq!(all(&a), all(&a), "同じ実行の中では何度引いても同じ");
        assert_eq!(all(&a), all(&clocked(now, &RANDOM_STUDY_RULES)));
        let random = |eng: Engine| Engine {
            ns: Uuid::new_v4(),
            ..eng
        };
        let b = random(clocked(now, &RANDOM_STUDY_RULES));
        let c = random(clocked(now, &RANDOM_STUDY_RULES));
        assert_ne!(all(&b), all(&c));
    }

    #[test]
    fn zero_range_random_shift_does_nothing() {
        let eng = clocked(
            at(2026, 10, 21, 9, 5, 7),
            &[("StudyTime", Level::Study, "%(now_time~0m)s")],
        );
        assert_eq!(value(&eng, tags::STUDY_TIME, idx(2, 7, 1, 3)), "090507");
    }

    #[test]
    fn shift_outside_years_1_to_9999_is_an_error() {
        let eng = clocked(
            at(9999, 12, 31, 12, 0, 0),
            &[("StudyDate", Level::Study, "%(now_date+1d*s)s")],
        );
        assert_eq!(value(&eng, tags::STUDY_DATE, idx(0, 0, 0, 0)), "99991231");
        let err = eng
            .render_rule(tags::STUDY_DATE, idx(0, 1, 0, 0), &Place::default())
            .unwrap()
            .unwrap_err();
        assert!(err.contains("9999"), "{err}");
        let eng = clocked(
            at(2026, 1, 1, 0, 0, 0),
            &[("StudyDate", Level::Study, "%(now_date-1000000d*s)s")],
        );
        assert!(eng
            .render_rule(tags::STUDY_DATE, idx(0, 1, 0, 0), &Place::default())
            .unwrap()
            .is_err());
    }

    #[test]
    fn multiplier_must_be_a_loop_count_allowed_at_the_tags_level() {
        let ok = |l, v| parse_rule(&RuleSpec::new("StudyDescription", l, v), None);
        let err = |l, v| format!("{:#}", ok(l, v).err().unwrap());
        assert!(ok(Level::Patient, "%(now_date+1d*p)s").is_ok());
        assert!(ok(Level::Patient, "%(now_date+1d*P)s").is_ok());
        assert!(ok(Level::Study, "%(now_date+1d*s1)s").is_ok());
        assert!(ok(Level::Series, "%(now_time+5m*r)s").is_ok());
        assert!(ok(Level::Image, "%(now_time+5m*i+1s*n)s").is_ok());
        // 細かいレベルの番号は、粗いレベルのタグには掛けられない
        assert!(err(Level::Patient, "%(now_date+1d*s)s").contains("`s`"));
        assert!(err(Level::Study, "%(now_date+1d*i)s").contains("`i`"));
        assert!(err(Level::Series, "%(now_time+1m*n)s").contains("`n`"));
        // 文字列の変数や、日時の変数は掛けられない
        assert!(err(Level::Image, "%(now_date+1d*src_series_number)s").contains("掛けられない"));
        assert!(err(Level::Image, "%(now_date+1d*now_day)s").contains("掛けられない"));
        // ランダムだけなら、どのレベルでも使える
        assert!(ok(Level::Patient, "%(now_time~30m)s").is_ok());
    }

    #[test]
    fn multiplier_cannot_be_a_csv_column() {
        let table = Arc::new(Table {
            columns: vec!["days".into()],
            rows: vec![[("days".to_string(), "3".to_string())].into()],
        });
        let err = parse_rule(
            &RuleSpec::new("StudyDate", Level::Study, "%(now_date+1d*days)s"),
            Some(table),
        )
        .err()
        .unwrap();
        assert!(format!("{err:#}").contains("`days`"), "{err:#}");
    }

    #[test]
    fn only_run_time_variables_can_be_shifted() {
        let err = |v| {
            format!(
                "{:#}",
                parse_rule(&RuleSpec::new("StudyDate", Level::Study, v), None)
                    .err()
                    .unwrap()
            )
        };
        // now_ で始まる名前だけをずらしとして読むので、ほかは名前の一部になり、変数がない
        assert!(err("%(s+1d)s").contains("`s+1d`"));
        assert!(err("%(now_week+1d)s").contains("`now_week`"));
        assert!(err("%(now_date*s)s").contains("`now_date*s`"));
        assert!(err("%(now_date+1y)s").contains("単位"));
    }

    #[test]
    fn preflight_accepts_shifted_dates_for_date_and_time_tags() {
        let eng = clocked(
            at(2026, 10, 21, 9, 0, 0),
            &[
                ("StudyDate", Level::Study, "%(now_date+1d*s~30m)s"),
                ("StudyTime", Level::Study, "%(now_time+1d*s~30m)s"),
                ("ContentTime", Level::Image, "%(now_time-5m*i)s"),
            ],
        );
        eng.preflight().unwrap();
    }
}
