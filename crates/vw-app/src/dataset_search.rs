//! データセットを探す。データセットのフォルダ（[`crate::settings::Dataset::roots`]）以下から
//! 検索カタログを集め、名前・メモ・タグ・足した項目・メタデータの条件で絞る。
//!
//! 絞り込みはデータセットの単位で、カタログの重複のない組の一覧を先に調べてから、ファイルが
//! 持つ組の番号をたどる。ファイルを 1 つずつ開き直さないので、数千ファイルのデータセットでも
//! 入力のたびに結果を出し直せる。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::catalog::{self, Catalog};

/// 見つけたデータセット 1 つ。
pub struct Entry {
    pub dir: PathBuf,
    pub catalog: Catalog,
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
        let mut index = Index::default();
        let mut seen = BTreeSet::new();
        for root in roots {
            index.walk(root, &mut seen);
        }
        index.entries.sort_by(|a, b| a.dir.cmp(&b.dir));
        index.collect_keys();
        index
    }

    fn walk(&mut self, dir: &Path, seen: &mut BTreeSet<PathBuf>) {
        if catalog::exists_in(dir) && seen.insert(dir.to_path_buf()) {
            match catalog::load(dir) {
                Ok(c) => self.entries.push(Entry {
                    dir: dir.to_path_buf(),
                    catalog: c,
                }),
                Err(e) => self.problems.push(e),
            }
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for e in read.flatten() {
            let bin = crate::tree::is_recycle_bin(&e.file_name().to_string_lossy());
            if !bin && e.file_type().is_ok_and(|t| t.is_dir()) {
                self.walk(&e.path(), seen);
            }
        }
    }

    fn collect_keys(&mut self) {
        let keys: BTreeSet<(String, String)> = self
            .entries
            .iter()
            .flat_map(|e| {
                e.catalog
                    .meta
                    .iter()
                    .map(|(k, key, _)| (k.clone(), key.clone()))
            })
            .collect();
        self.meta_keys = keys.into_iter().collect();
    }

    /// 1 つのデータセットを読み直す（更新や編集の後）。なくなっていれば外す。
    pub fn reload(&mut self, dir: &Path) {
        self.entries.retain(|e| e.dir != dir);
        if let Ok(c) = catalog::load(dir) {
            self.entries.push(Entry {
                dir: dir.to_path_buf(),
                catalog: c,
            });
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
            .flat_map(|e| &e.catalog.meta)
            .filter(|(k, kk, v)| {
                (kind.is_empty() || k.to_lowercase() == kind)
                    && key_matches(kk, &key)
                    && !v.is_empty()
            })
            .map(|(_, _, v)| v)
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

/// 簡易検索の 1 行と、詳細検索の条件。すべて AND で組み合わせる。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub text: String,
    pub conditions: Vec<Condition>,
}

/// 当たったデータセットと、条件に合うファイルの数。
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub entry: usize,
    pub files: usize,
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
    Dataset(Box<dyn Fn(&Catalog) -> bool>),
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
    for c in &query.conditions {
        checks.push(condition_check(c)?);
    }
    let mut hits = Vec::new();
    for (i, e) in index.entries.iter().enumerate() {
        if let Some(files) = evaluate(&e.catalog, &checks) {
            hits.push(Hit { entry: i, files });
        }
    }
    Ok(hits)
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

/// 当たればファイルの数を返す。メタデータの条件があれば、そのすべてに合うファイルの数。
///
fn evaluate(c: &Catalog, checks: &[Check]) -> Option<usize> {
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
                    let ok: Vec<bool> = c.meta.iter().map(|(_, _, v)| has(v)).collect();
                    if !ok.iter().any(|&b| b) {
                        return None;
                    }
                    wanted.push(ok);
                }
            }
            Check::Meta { kind, key, matcher } => {
                let ok: Vec<bool> = c
                    .meta
                    .iter()
                    .map(|(k, kk, v)| {
                        (kind.is_empty() || k.to_lowercase() == *kind)
                            && (key.is_empty() || key_matches(kk, key))
                            && matcher.matches(v)
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
        return Some(c.files.len());
    }
    let files = c
        .files
        .iter()
        .filter(|f| {
            wanted.iter().all(|ok| {
                f.meta
                    .iter()
                    .any(|&i| ok.get(i as usize).copied().unwrap_or(false))
            })
        })
        .count();
    (files > 0).then_some(files)
}

/// キーの一致。DICOM のタグは番号（`0010,0020`）のほか名前（`PatientID`）でも指せる。
fn key_matches(key: &str, wanted: &str) -> bool {
    key.to_lowercase() == wanted
        || vw_dicom::tag_name(key).is_some_and(|n| n.to_lowercase() == wanted)
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
                .map(|c| Entry {
                    dir: PathBuf::from(format!("/d/{}", c.name)),
                    catalog: c,
                })
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
            .map(|h| (index.entries[h.entry].catalog.name.clone(), h.files))
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
}
