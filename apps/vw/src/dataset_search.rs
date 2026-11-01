//! データセットを探す。データセットのフォルダ（[`crate::settings::Dataset::roots`]）以下から
//! 検索カタログを集め、名前・メモ・タグ・足した項目・メタデータの条件で絞る。
//!
//! 絞り込みはデータセットの単位で、カタログの重複のない組の一覧を先に調べてから、ファイルが
//! 持つ組の番号をたどる。ファイルを 1 つずつ開き直さないので、数千ファイルのデータセットでも
//! 入力のたびに結果を出し直せる。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rayon::prelude::*;

use crate::catalog::{self, Catalog};
use crate::settings::SavedCondition;

/// 探している途中の数。別のスレッドから数え、画面が毎フレーム読む。
#[derive(Default)]
pub struct Progress {
    /// 中を見たフォルダの数。
    pub folders: AtomicUsize,
    /// 見つけたカタログの数。
    pub found: AtomicUsize,
}

/// 見つけたデータセット 1 つ。
pub struct Entry {
    pub dir: PathBuf,
    pub catalog: Arc<Catalog>,
    /// メタデータの組を小文字にしたもの。入力のたびに全部の値を小文字にし直さないため。
    lower: Arc<Lower>,
}

impl Entry {
    pub fn new(dir: PathBuf, catalog: Catalog) -> Self {
        let lower = Arc::new(Lower::of(&catalog));
        Self {
            dir,
            catalog: Arc::new(catalog),
            lower,
        }
    }
}

/// [`Catalog::meta`] と同じ並びの、小文字の種類・キー・値。キーには DICOM のタグの名前も入れる
/// （番号でも名前でも指せるように）。
#[derive(Default)]
struct Lower {
    kinds: Vec<String>,
    keys: Vec<(String, Option<String>)>,
    values: Vec<String>,
    /// 種類とキーの組（元の綴り、重複なし）。候補の一覧を作るたびに全部の組をたどらないため。
    pairs: Vec<(String, String)>,
}

impl Lower {
    fn of(c: &Catalog) -> Self {
        let mut l = Lower::default();
        for (kind, key, value) in &c.meta {
            l.kinds.push(kind.to_lowercase());
            l.keys.push((
                key.to_lowercase(),
                vw_dicom::tag_name(key).map(|n| n.to_lowercase()),
            ));
            l.values.push(value.to_lowercase());
        }
        let pairs: BTreeSet<(&str, &str)> = c
            .meta
            .iter()
            .map(|(k, key, _)| (k.as_str(), key.as_str()))
            .collect();
        l.pairs = pairs
            .into_iter()
            .map(|(k, key)| (k.to_owned(), key.to_owned()))
            .collect();
        l
    }
}

/// 読んだカタログ。ファイルの更新時刻と大きさが同じなら、画面を開き直しても読み直さない。
/// 数十 MB のカタログでは、フォルダをたどるのと同じくらい JSON の読み込みに時間がかかるため。
struct Loaded {
    modified: SystemTime,
    len: u64,
    catalog: Arc<Catalog>,
    lower: Arc<Lower>,
}

static LOADED: Mutex<Option<HashMap<PathBuf, Loaded>>> = Mutex::new(None);

/// `dir` のカタログを、前に読んだものが変わっていなければそれを使って読む。`force` なら必ず読み直す
/// （vw が書き換えた直後。更新時刻の細かさが粗いドライブでは、同じ大きさの書き換えを見分けられないため）。
fn load_cached(dir: &Path, force: bool) -> Result<Entry, String> {
    let path = catalog::path_in(dir);
    let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (modified, len) = (meta.modified().ok(), meta.len());
    let entry = |l: &Loaded| Entry {
        dir: dir.to_path_buf(),
        catalog: l.catalog.clone(),
        lower: l.lower.clone(),
    };
    {
        let cache = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = cache.as_ref().and_then(|c| c.get(dir)).filter(|_| !force)
            && Some(l.modified) == modified
            && l.len == len
        {
            return Ok(entry(l));
        }
    }
    let fresh = Entry::new(dir.to_path_buf(), catalog::load(dir)?);
    if let Some(modified) = modified {
        let mut cache = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        cache.get_or_insert_with(HashMap::new).insert(
            dir.to_path_buf(),
            Loaded {
                modified,
                len,
                catalog: fresh.catalog.clone(),
                lower: fresh.lower.clone(),
            },
        );
    }
    Ok(fresh)
}

/// 見つけたデータセットの一覧と、入力の候補。
#[derive(Default)]
pub struct Index {
    pub entries: Vec<Entry>,
    /// 読めなかったカタログ。
    pub problems: Vec<String>,
    /// メタデータの種類とキーの組（候補用）。
    pub meta_keys: Vec<(String, String)>,
}

impl Index {
    /// `roots` 以下の全フォルダからカタログを集める。カタログのあるフォルダの下も探す
    /// （データセットの中に別のデータセットがあってよいため）。
    pub fn discover(roots: &[PathBuf]) -> Self {
        let progress = Progress::default();
        Self::merge(
            roots
                .iter()
                .map(|r| Self::walk_root(r, &progress))
                .collect(),
        )
    }

    /// 1 つのデータセットのフォルダ以下を探す。データセットのフォルダごとに別のスレッドで
    /// 探し、[`Index::merge`] でまとめるのに使う。
    pub fn walk_root(root: &Path, progress: &Progress) -> Self {
        walk(root, progress)
    }

    /// フォルダごとに探した結果をまとめる。データセットのフォルダが入れ子になっていると
    /// 同じカタログを二度見つけるので、1 つにする。
    pub fn merge(parts: Vec<Index>) -> Self {
        let mut index = Index::default();
        let mut seen = BTreeSet::new();
        for part in parts {
            for e in part.entries {
                if seen.insert(e.dir.clone()) {
                    index.entries.push(e);
                }
            }
            for p in part.problems {
                if !index.problems.contains(&p) {
                    index.problems.push(p);
                }
            }
        }
        index.entries.sort_by(|a, b| a.dir.cmp(&b.dir));
        index.collect_keys();
        index
    }

    fn collect_keys(&mut self) {
        let keys: BTreeSet<&(String, String)> =
            self.entries.iter().flat_map(|e| &e.lower.pairs).collect();
        self.meta_keys = keys.into_iter().cloned().collect();
    }

    /// 1 つのデータセットを読み直す（更新や編集の後）。なくなっていれば外す。
    pub fn reload(&mut self, dir: &Path) {
        self.entries.retain(|e| e.dir != dir);
        if let Ok(e) = load_cached(dir, true) {
            self.entries.push(e);
            self.entries.sort_by(|a, b| a.dir.cmp(&b.dir));
        }
        self.collect_keys();
    }

    pub fn tags(&self) -> Vec<String> {
        let set: BTreeSet<&String> = self.entries.iter().flat_map(|e| &e.catalog.tags).collect();
        set.into_iter().cloned().collect()
    }

    /// 足した項目の名前。設定の順に並べ、カタログにしかないものを後ろに足す。
    pub fn field_names(&self, configured: &[String]) -> Vec<String> {
        let mut names = configured.to_vec();
        let extra: BTreeSet<&String> = self
            .entries
            .iter()
            .flat_map(|e| e.catalog.fields.keys())
            .filter(|k| !configured.contains(k))
            .collect();
        names.extend(extra.into_iter().cloned());
        names
    }

    /// メタデータ `kind` / `key` の、今ある値（重複なし、空は除く）。種類やキーが空なら
    /// 条件と同じくどれにでも合うとみなすが、値が多すぎて選べないので、キーがなければ出さない。
    pub fn meta_values(&self, kind: &str, key: &str) -> Vec<String> {
        let (kind, key) = (kind.trim().to_lowercase(), key.trim().to_lowercase());
        if key.is_empty() {
            return Vec::new();
        }
        let set: BTreeSet<&String> = self
            .entries
            .iter()
            .flat_map(|e| {
                let (l, kind, key) = (&e.lower, &kind, &key);
                e.catalog
                    .meta
                    .iter()
                    .enumerate()
                    .filter_map(move |(i, (_, _, v))| {
                        let (k, name) = &l.keys[i];
                        ((kind.is_empty() || l.kinds[i] == *kind)
                            && (k == key || name.as_ref() == Some(key))
                            && !v.is_empty())
                        .then_some(v)
                    })
            })
            .collect();
        set.into_iter().cloned().collect()
    }

    /// 項目ごとの、今ある値（空は除く）。
    pub fn field_values(&self) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for e in &self.entries {
            for (k, v) in &e.catalog.fields {
                if !v.is_empty() {
                    out.entry(k.clone()).or_default().insert(v.clone());
                }
            }
        }
        out.into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect()
    }
}

/// 条件をかける先。
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Name,
    Memo,
    Tags,
    Field(String),
    /// 種類とキー。空ならどれでもよい。
    Meta {
        kind: String,
        key: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Contains,
    Equals,
    Regex,
    /// 数値の範囲。`value` が下限、`max` が上限で、空ならその側は限らない。
    Range,
    Exists,
}

impl Op {
    pub const ALL: [Op; 5] = [Op::Contains, Op::Equals, Op::Regex, Op::Range, Op::Exists];

    pub fn title(self) -> &'static str {
        match self {
            Op::Contains => "contains",
            Op::Equals => "equals",
            Op::Regex => "regex",
            Op::Range => "range",
            Op::Exists => "exists",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub target: Target,
    pub op: Op,
    pub value: String,
    pub max: String,
}

impl Condition {
    /// 値をまだ入れていない。検索のグループで並べた条件は、値を入れたものから効かせる。
    ///
    /// 空の値で「含む」を全件、「一致」を 0 件として効かせないのは、グループを選んで
    /// 値を 1 つ入れた時点で、残りの空の行のせいで結果が消えたり変わったりしないため。
    /// キーがあるかどうかは「exists」で探せる。
    pub fn is_blank(&self) -> bool {
        let empty = |s: &str| s.trim().is_empty();
        match self.op {
            Op::Contains | Op::Equals | Op::Regex => empty(&self.value),
            Op::Range => empty(&self.value) && empty(&self.max),
            Op::Exists => false,
        }
    }
}

impl Condition {
    /// 設定に取っておいた形から戻す。
    pub fn from_saved(c: &SavedCondition) -> Result<Self, String> {
        let target = match c.on.as_str() {
            "name" => Target::Name,
            "memo" => Target::Memo,
            "tags" => Target::Tags,
            on => match on.split_once(':') {
                Some(("field", f)) => Target::Field(f.to_owned()),
                Some(("meta", rest)) => {
                    let (kind, key) = rest.split_once(':').unwrap_or(("", rest));
                    Target::Meta {
                        kind: kind.to_owned(),
                        key: key.to_owned(),
                    }
                }
                _ => return Err(format!("unknown target {on:?}")),
            },
        };
        let op = Op::ALL
            .into_iter()
            .find(|op| op.title() == c.op)
            .ok_or_else(|| format!("unknown operator {:?}", c.op))?;
        Ok(Self {
            target,
            op,
            value: c.value.clone(),
            max: c.max.clone(),
        })
    }

    pub fn to_saved(&self) -> SavedCondition {
        let on = match &self.target {
            Target::Name => "name".to_owned(),
            Target::Memo => "memo".to_owned(),
            Target::Tags => "tags".to_owned(),
            Target::Field(f) => format!("field:{f}"),
            Target::Meta { kind, key } => format!("meta:{kind}:{key}"),
        };
        SavedCondition {
            on,
            op: self.op.title().to_owned(),
            value: self.value.clone(),
            max: self.max.clone(),
        }
    }
}

/// 簡易検索の 1 行と、詳細検索の条件。すべて AND で組み合わせる。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub text: String,
    pub conditions: Vec<Condition>,
}

/// 当たったデータセットと、条件に合うファイル（カタログの `files` の添字）。
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub entry: usize,
    pub files: Vec<usize>,
    /// ファイルの条件（メタデータの条件か、データセットの情報に見つからなかった語）があって、
    /// ファイルを絞り込んだか。絞り込んでいなければ `files` はデータセットのすべてのファイル。
    pub narrowed: bool,
}

/// 値の比べ方を前もって用意したもの。正規表現を値ごとに作り直さないため。
enum Matcher {
    Contains(String),
    Equals(String),
    Regex(regex::Regex),
    Range(Option<f64>, Option<f64>),
    Exists,
}

impl Matcher {
    fn new(c: &Condition) -> Result<Self, String> {
        let number = |s: &str| -> Result<Option<f64>, String> {
            let s = s.trim();
            if s.is_empty() {
                return Ok(None);
            }
            s.parse()
                .map(Some)
                .map_err(|_| format!("not a number: {s}"))
        };
        Ok(match c.op {
            Op::Contains => Matcher::Contains(c.value.trim().to_lowercase()),
            Op::Equals => Matcher::Equals(c.value.trim().to_lowercase()),
            Op::Regex => Matcher::Regex(regex::Regex::new(&c.value).map_err(|e| e.to_string())?),
            Op::Range => Matcher::Range(number(&c.value)?, number(&c.max)?),
            Op::Exists => Matcher::Exists,
        })
    }

    /// `lower` は `value` を小文字にしたもの（前もって作ってある）。
    fn matches_lower(&self, value: &str, lower: &str) -> bool {
        match self {
            Matcher::Contains(s) => lower.contains(s),
            Matcher::Equals(s) => lower.trim() == *s,
            _ => self.matches(value),
        }
    }

    fn matches(&self, value: &str) -> bool {
        match self {
            Matcher::Contains(s) => value.to_lowercase().contains(s),
            Matcher::Equals(s) => value.trim().to_lowercase() == *s,
            Matcher::Regex(r) => r.is_match(value),
            // 複数の値（`0.5\0.5`）はどれか 1 つが範囲に入れば当たり。
            Matcher::Range(lo, hi) => value.split('\\').any(|v| {
                leading_number(v)
                    .is_some_and(|n| lo.is_none_or(|lo| n >= lo) && hi.is_none_or(|hi| n <= hi))
            }),
            Matcher::Exists => !value.is_empty(),
        }
    }
}

/// 値の先頭の数。単位の付いた値（EXIF の `50 mm` など）も範囲で探せるように、数の後ろは見ない。
fn leading_number(value: &str) -> Option<f64> {
    let v = value.trim();
    let end = v
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(v.len(), |(i, _)| i);
    v[..end].parse().ok()
}

/// データセットにかける条件。メタデータの条件はファイルの単位で数える。
enum Check {
    Dataset(Box<dyn Fn(&Catalog) -> bool + Send + Sync>),
    Meta {
        kind: String,
        key: String,
        matcher: Matcher,
    },
    /// 簡易検索の 1 語（小文字）。名前・メモ・タグ・項目のどれかに含まれればデータセットごと
    /// 当たり、そうでなければメタデータの値に含むファイルを数える。
    Term(String),
}

pub fn search(index: &Index, query: &Query) -> Result<Vec<Hit>, String> {
    let mut checks = Vec::new();
    for term in query.text.split_whitespace() {
        checks.push(term_check(term));
    }
    for c in query.conditions.iter().filter(|c| !c.is_blank()) {
        checks.push(condition_check(c)?);
    }
    Ok(index
        .entries
        .par_iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let (files, narrowed) = evaluate(&e.catalog, &e.lower, &checks)?;
            Some(Hit {
                entry: i,
                files,
                narrowed,
            })
        })
        .collect())
}

/// `dir` 以下を探す。サブフォルダは並列にたどる（ネットワークドライブでは、1 つずつ一覧を
/// 待つより、いくつも同時に頼んだほうがずっと速いため）。カタログはフォルダの一覧の名前で
/// 見つけ、フォルダごとにファイルがあるかを問い合わせ直さない。
fn walk(dir: &Path, progress: &Progress) -> Index {
    progress.folders.fetch_add(1, Ordering::Relaxed);
    let mut index = Index::default();
    let Ok(read) = std::fs::read_dir(dir) else {
        return index;
    };
    let mut subdirs = Vec::new();
    let mut has_catalog = false;
    for e in read.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = e.file_type() else {
            continue;
        };
        if kind.is_dir() && !crate::tree::is_recycle_bin(&name) {
            subdirs.push(e.path());
        } else if !kind.is_dir() && name == catalog::FILE_NAME {
            has_catalog = true;
        }
    }
    if has_catalog {
        match load_cached(dir, false) {
            Ok(e) => {
                progress.found.fetch_add(1, Ordering::Relaxed);
                index.entries.push(e);
            }
            Err(e) => index.problems.push(e),
        }
    }
    let parts: Vec<Index> = subdirs.par_iter().map(|d| walk(d, progress)).collect();
    for p in parts {
        index.entries.extend(p.entries);
        index.problems.extend(p.problems);
    }
    index
}

/// 簡易検索の 1 語。
fn term_check(term: &str) -> Check {
    Check::Term(term.to_lowercase())
}

fn condition_check(c: &Condition) -> Result<Check, String> {
    let m = Matcher::new(c)?;
    Ok(match &c.target {
        Target::Name => Check::Dataset(Box::new(move |cat: &Catalog| m.matches(&cat.name))),
        Target::Memo => Check::Dataset(Box::new(move |cat: &Catalog| m.matches(&cat.memo))),
        Target::Tags => Check::Dataset(Box::new(move |cat: &Catalog| {
            cat.tags.iter().any(|t| m.matches(t))
        })),
        Target::Field(name) => {
            let name = name.clone();
            Check::Dataset(Box::new(move |cat: &Catalog| {
                cat.fields.get(&name).is_some_and(|v| m.matches(v))
            }))
        }
        Target::Meta { kind, key } => Check::Meta {
            kind: kind.trim().to_lowercase(),
            key: key.trim().to_lowercase(),
            matcher: m,
        },
    })
}

/// 当たれば、条件に合うファイルの添字と、ファイルで絞り込んだかを返す。メタデータの条件が
/// あれば、そのすべてに合うファイルだけ。
fn evaluate(c: &Catalog, lower: &Lower, checks: &[Check]) -> Option<(Vec<usize>, bool)> {
    let mut wanted: Vec<Vec<bool>> = Vec::new();
    for check in checks {
        match check {
            Check::Dataset(f) => {
                if !f(c) {
                    return None;
                }
            }
            Check::Term(needle) => {
                let has = |s: &str| s.to_lowercase().contains(needle.as_str());
                let in_dataset = has(&c.name)
                    || has(&c.memo)
                    || c.tags.iter().any(|t| has(t))
                    || c.fields.values().any(|v| has(v));
                if !in_dataset {
                    let ok: Vec<bool> = lower
                        .values
                        .iter()
                        .map(|v| v.contains(needle.as_str()))
                        .collect();
                    if !ok.iter().any(|&b| b) {
                        return None;
                    }
                    wanted.push(ok);
                }
            }
            Check::Meta { kind, key, matcher } => {
                let ok: Vec<bool> = (0..c.meta.len())
                    .map(|i| {
                        let (k, name) = &lower.keys[i];
                        (kind.is_empty() || lower.kinds[i] == *kind)
                            && (key.is_empty() || k == key || name.as_ref() == Some(key))
                            && matcher.matches_lower(&c.meta[i].2, &lower.values[i])
                    })
                    .collect();
                if !ok.iter().any(|&b| b) {
                    return None;
                }
                wanted.push(ok);
            }
        }
    }
    if wanted.is_empty() {
        return Some(((0..c.files.len()).collect(), false));
    }
    let files: Vec<usize> = c
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            wanted.iter().all(|ok| {
                f.meta
                    .iter()
                    .any(|&i| ok.get(i as usize).copied().unwrap_or(false))
            })
        })
        .map(|(i, _)| i)
        .collect();
    (!files.is_empty()).then_some((files, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::FileEntry;

    fn catalog(name: &str, files: &[&[(&str, &str, &str)]]) -> Catalog {
        let mut c = Catalog {
            name: name.into(),
            ..Catalog::default()
        };
        for (n, triples) in files.iter().enumerate() {
            let mut ids = Vec::new();
            for &(k, key, v) in *triples {
                let t = (k.to_owned(), key.to_owned(), v.to_owned());
                let i = match c.meta.iter().position(|x| *x == t) {
                    Some(i) => i,
                    None => {
                        c.meta.push(t);
                        c.meta.len() - 1
                    }
                };
                ids.push(i as u32);
            }
            c.files.push(FileEntry {
                path: format!("{n}.dcm"),
                size: 0,
                modified: String::new(),
                meta: ids,
            });
        }
        c
    }

    fn index(catalogs: Vec<Catalog>) -> Index {
        let mut i = Index {
            entries: catalogs
                .into_iter()
                .map(|c| Entry::new(PathBuf::from(format!("/d/{}", c.name)), c))
                .collect(),
            ..Index::default()
        };
        i.collect_keys();
        i
    }

    #[test]
    fn meta_values_lists_each_value_of_a_key_once() {
        let i = sample();
        assert_eq!(i.meta_values("dicom", "0008,0060"), ["CT", "PT"]);
        assert_eq!(i.meta_values("", "Modality"), ["CT", "PT"]);
        assert_eq!(i.meta_values("DICOM", "0010,0020"), ["A", "B"]);
        assert!(i.meta_values("dicom", "").is_empty());
    }

    fn sample() -> Index {
        let mut ct = catalog(
            "CT kernels",
            &[
                &[("dicom", "0010,0020", "A"), ("dicom", "0008,0060", "CT")],
                &[("dicom", "0010,0020", "B"), ("dicom", "0008,0060", "CT")],
            ],
        );
        ct.tags = vec!["phantom".into()];
        ct.fields.insert("Source".into(), "Hospital A".into());
        let mut mixed = catalog(
            "Mixed",
            &[
                &[("dicom", "0010,0020", "A"), ("dicom", "0008,0060", "CT")],
                &[("dicom", "0010,0020", "B"), ("dicom", "0008,0060", "PT")],
                &[("exif", "Model", "NIKON Z 9"), ("image", "width", "6048")],
            ],
        );
        mixed.memo = "from the March study".into();
        index(vec![ct, mixed])
    }

    fn names(index: &Index, query: &Query) -> Vec<(String, usize)> {
        search(index, query)
            .unwrap()
            .into_iter()
            .map(|h| (index.entries[h.entry].catalog.name.clone(), h.files.len()))
            .collect()
    }

    fn cond(target: Target, op: Op, value: &str) -> Condition {
        Condition {
            target,
            op,
            value: value.into(),
            max: String::new(),
        }
    }

    fn meta(kind: &str, key: &str) -> Target {
        Target::Meta {
            kind: kind.into(),
            key: key.into(),
        }
    }

    #[test]
    fn empty_query_lists_every_dataset_with_all_files() {
        let i = sample();
        assert_eq!(
            names(&i, &Query::default()),
            [("CT kernels".into(), 2), ("Mixed".into(), 3)]
        );
    }

    #[test]
    fn simple_words_must_all_be_found_somewhere() {
        let i = sample();
        let q = |t: &str| Query {
            text: t.into(),
            ..Query::default()
        };
        assert_eq!(names(&i, &q("phantom")), [("CT kernels".into(), 2)]);
        assert_eq!(names(&i, &q("march")), [("Mixed".into(), 3)]);
        assert_eq!(
            names(&i, &q("hospital kernels")),
            [("CT kernels".into(), 2)]
        );
        assert_eq!(names(&i, &q("nikon")), [("Mixed".into(), 1)]);
        assert!(names(&i, &q("phantom nikon")).is_empty());
    }

    #[test]
    fn metadata_conditions_count_the_files_that_meet_all_of_them() {
        let i = sample();
        let q = Query {
            conditions: vec![
                cond(meta("dicom", "0010,0020"), Op::Equals, "A"),
                cond(meta("", "0008,0060"), Op::Equals, "CT"),
            ],
            ..Query::default()
        };
        assert_eq!(
            names(&i, &q),
            [("CT kernels".into(), 1), ("Mixed".into(), 1)]
        );
    }

    #[test]
    fn hits_list_the_files_that_meet_the_metadata_conditions() {
        let i = sample();
        let q = Query {
            conditions: vec![cond(meta("dicom", "0008,0060"), Op::Equals, "CT")],
            ..Query::default()
        };
        let hits = search(&i, &q).unwrap();
        let paths = |h: &Hit| -> Vec<String> {
            let c = &i.entries[h.entry].catalog;
            h.files.iter().map(|&f| c.files[f].path.clone()).collect()
        };
        assert_eq!(paths(&hits[0]), ["0.dcm", "1.dcm"]);
        assert_eq!(paths(&hits[1]), ["0.dcm"]);
        assert!(hits.iter().all(|h| h.narrowed));
    }

    #[test]
    fn dataset_conditions_alone_keep_every_file() {
        let i = sample();
        let q = Query {
            text: "march".into(),
            conditions: vec![cond(Target::Name, Op::Contains, "mixed")],
        };
        let hits = search(&i, &q).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].files, [0, 1, 2]);
        assert!(!hits[0].narrowed);
    }

    #[test]
    fn conditions_must_meet_in_the_same_file() {
        // Mixed には患者 A も PT もあるが、患者 A のファイルは CT なので当たらない。
        let i = sample();
        let q = Query {
            conditions: vec![
                cond(meta("dicom", "0010,0020"), Op::Equals, "A"),
                cond(meta("dicom", "0008,0060"), Op::Equals, "PT"),
            ],
            ..Query::default()
        };
        assert!(names(&i, &q).is_empty());
    }

    #[test]
    fn conditions_without_a_value_are_left_out_until_filled() {
        let i = sample();
        let q = Query {
            conditions: vec![
                cond(meta("dicom", "0010,0020"), Op::Equals, "A"),
                cond(meta("dicom", "0008,0060"), Op::Equals, " "),
                cond(meta("dicom", "0008,0020"), Op::Contains, ""),
                cond(meta("image", "width"), Op::Range, ""),
            ],
            ..Query::default()
        };
        let only_patient = Query {
            conditions: q.conditions[..1].to_vec(),
            ..Query::default()
        };
        assert_eq!(names(&i, &q), names(&i, &only_patient));
        assert!(!names(&i, &q).is_empty());
    }

    #[test]
    fn dicom_keys_can_be_given_by_name() {
        let i = sample();
        let q = Query {
            conditions: vec![cond(meta("dicom", "Modality"), Op::Equals, "pt")],
            ..Query::default()
        };
        assert_eq!(names(&i, &q), [("Mixed".into(), 1)]);
    }

    #[test]
    fn numeric_range_and_exists() {
        let i = sample();
        let mut range = cond(meta("image", "width"), Op::Range, "4000");
        range.max = "7000".into();
        let q = Query {
            conditions: vec![range],
            ..Query::default()
        };
        assert_eq!(names(&i, &q), [("Mixed".into(), 1)]);
        let q = Query {
            conditions: vec![cond(meta("exif", "Model"), Op::Exists, "")],
            ..Query::default()
        };
        assert_eq!(names(&i, &q), [("Mixed".into(), 1)]);
    }

    #[test]
    fn multi_valued_numbers_match_if_any_value_is_in_range() {
        let i = index(vec![catalog("s", &[&[("dicom", "0028,0030", "0.5\\0.7")]])]);
        let mut range = cond(meta("dicom", "0028,0030"), Op::Range, "0.6");
        range.max = "0.8".into();
        let q = Query {
            conditions: vec![range],
            ..Query::default()
        };
        assert_eq!(names(&i, &q).len(), 1);
    }

    #[test]
    fn range_reads_the_number_before_a_unit() {
        let i = index(vec![
            catalog("wide", &[&[("exif", "FocalLength", "24 mm")]]),
            catalog("tele", &[&[("exif", "FocalLength", "200 mm")]]),
            catalog("text", &[&[("exif", "FocalLength", "unknown")]]),
        ]);
        let mut range = cond(meta("exif", "FocalLength"), Op::Range, "100");
        range.max = "300".into();
        let q = Query {
            conditions: vec![range],
            ..Query::default()
        };
        assert_eq!(names(&i, &q), [("tele".to_owned(), 1)]);
    }

    #[test]
    fn fields_tags_name_and_memo_conditions() {
        let i = sample();
        let q = |c| Query {
            conditions: vec![c],
            ..Query::default()
        };
        let source = Target::Field("Source".into());
        assert_eq!(
            names(&i, &q(cond(source, Op::Equals, "hospital a"))),
            [("CT kernels".into(), 2)]
        );
        assert_eq!(names(&i, &q(cond(Target::Tags, Op::Exists, ""))).len(), 1);
        assert_eq!(
            names(&i, &q(cond(Target::Name, Op::Regex, "^Mix"))),
            [("Mixed".into(), 3)]
        );
        assert_eq!(
            names(&i, &q(cond(Target::Memo, Op::Contains, "MARCH"))).len(),
            1
        );
    }

    #[test]
    fn bad_regex_or_number_is_an_error() {
        let i = sample();
        let q = |c| Query {
            conditions: vec![c],
            ..Query::default()
        };
        assert!(search(&i, &q(cond(Target::Name, Op::Regex, "("))).is_err());
        assert!(search(&i, &q(cond(meta("", "width"), Op::Range, "abc"))).is_err());
    }

    #[test]
    fn nested_dataset_folders_find_each_catalog_once_and_count_progress() {
        let tmp = crate::test_util::TempDir::new();
        catalog::save(&tmp.mkdir("a/b"), &Catalog::default()).unwrap();
        let progress = Progress::default();
        let parts = vec![
            Index::walk_root(tmp.path(), &progress),
            Index::walk_root(&tmp.path().join("a"), &progress),
        ];
        assert_eq!(progress.folders.load(Ordering::Relaxed), 3 + 2);
        assert_eq!(progress.found.load(Ordering::Relaxed), 2);
        let i = Index::merge(parts);
        let found: Vec<_> = i.entries.iter().map(|e| e.dir.clone()).collect();
        assert_eq!(found, [tmp.path().join("a/b")]);
    }

    #[test]
    fn discovery_skips_catalogs_in_the_recycle_bin() {
        let tmp = crate::test_util::TempDir::new();
        catalog::save(&tmp.mkdir("$RECYCLE.BIN/S-1-5/$R1"), &Catalog::default()).unwrap();
        catalog::save(&tmp.mkdir("kept"), &Catalog::default()).unwrap();
        let i = Index::discover(&[tmp.path().to_path_buf()]);
        let found: Vec<_> = i.entries.iter().map(|e| e.dir.clone()).collect();
        assert_eq!(found, [tmp.path().join("kept")]);
    }

    #[test]
    fn discovery_finds_nested_datasets_and_suggestions() {
        let tmp = crate::test_util::TempDir::new();
        let outer = tmp.mkdir("outer");
        let inner = tmp.mkdir("outer/inner");
        let mut c = Catalog {
            name: "outer".into(),
            tags: vec!["b".into(), "a".into()],
            ..Catalog::default()
        };
        c.fields.insert("Other".into(), "x".into());
        catalog::save(&outer, &c).unwrap();
        let c = Catalog {
            name: "inner".into(),
            tags: vec!["a".into()],
            ..Catalog::default()
        };
        catalog::save(&inner, &c).unwrap();
        std::fs::write(tmp.mkdir("broken").join(catalog::FILE_NAME), "{").unwrap();
        let i = Index::discover(&[tmp.path().to_path_buf()]);
        let found: Vec<_> = i.entries.iter().map(|e| e.catalog.name.as_str()).collect();
        assert_eq!(found, ["outer", "inner"]);
        assert_eq!(i.problems.len(), 1);
        assert_eq!(i.tags(), ["a", "b"]);
        assert_eq!(i.field_names(&["Source".into()]), ["Source", "Other"]);
        assert_eq!(i.field_values()["Other"], ["x"]);
    }

    #[test]
    fn reopening_reuses_unchanged_catalogs_and_rereads_changed_ones() {
        let tmp = crate::test_util::TempDir::new();
        let dir = tmp.mkdir("set");
        let named = |name: &str| Catalog {
            name: name.into(),
            ..Catalog::default()
        };
        catalog::save(&dir, &named("first")).unwrap();
        let roots = [tmp.path().to_path_buf()];
        let a = Index::discover(&roots);
        let b = Index::discover(&roots);
        assert!(Arc::ptr_eq(&a.entries[0].catalog, &b.entries[0].catalog));
        catalog::save(&dir, &named("second, longer")).unwrap();
        let c = Index::discover(&roots);
        assert_eq!(c.entries[0].catalog.name, "second, longer");
    }

    #[test]
    fn reloading_after_an_edit_rereads_even_when_the_size_and_time_match() {
        let tmp = crate::test_util::TempDir::new();
        let dir = tmp.mkdir("set");
        let named = |name: &str| Catalog {
            name: name.into(),
            ..Catalog::default()
        };
        catalog::save(&dir, &named("abc")).unwrap();
        let mut index = Index::discover(&[tmp.path().to_path_buf()]);
        let modified = std::fs::metadata(catalog::path_in(&dir))
            .and_then(|m| m.modified())
            .unwrap();
        catalog::save(&dir, &named("abd")).unwrap();
        std::fs::File::options()
            .write(true)
            .open(catalog::path_in(&dir))
            .and_then(|f| f.set_modified(modified))
            .unwrap();
        index.reload(&dir);
        assert_eq!(index.entries[0].catalog.name, "abd");
    }

    #[test]
    fn conditions_round_trip_through_the_saved_form() {
        let conditions = [
            Condition {
                target: Target::Meta {
                    kind: "fits".into(),
                    key: "OBJECT".into(),
                },
                op: Op::Equals,
                value: "M42".into(),
                max: String::new(),
            },
            Condition {
                target: Target::Meta {
                    kind: String::new(),
                    key: "a:b".into(),
                },
                op: Op::Range,
                value: "1".into(),
                max: "2".into(),
            },
            Condition {
                target: Target::Field("入手元".into()),
                op: Op::Regex,
                value: "^x".into(),
                max: String::new(),
            },
            Condition {
                target: Target::Tags,
                op: Op::Exists,
                value: String::new(),
                max: String::new(),
            },
        ];
        for c in conditions {
            assert_eq!(Condition::from_saved(&c.to_saved()), Ok(c));
        }
        let bad = SavedCondition {
            on: "size".into(),
            op: "equals".into(),
            ..Default::default()
        };
        assert!(Condition::from_saved(&bad).is_err());
    }
}
