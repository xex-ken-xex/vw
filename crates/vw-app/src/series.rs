//! フォルダの DICOM を 検査 → シリーズ にまとめる。
//!
//! ファイルのヘッダ（画素の手前まで）を別スレッドで読み、Study / Series Instance UID で
//! 分ける。CT の 1 シリーズは数百ファイルあり、全部を UI スレッドで読むと固まるため。

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use vw_core::{Metadata, Registry};

/// ヘッダを読むときに最初に読むバイト数。画素データの手前までがたいていこれに収まり、
/// 1 ファイル数百 KB の CT でも全体を読まずに済む。収まらなければ全体を読み直す。
const HEADER_BYTES: u64 = 64 * 1024;

/// 1 つのシリーズ。
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub uid: String,
    pub number: Option<i64>,
    pub description: String,
    pub modality: String,
    /// Instance Number の順（同じならファイル名の順）に並べたファイル。
    pub files: Vec<PathBuf>,
}

impl Series {
    /// ツリーに出す名前。例: `#2 Axial 1mm (CT, 24)`。
    pub fn title(&self) -> String {
        let number = self.number.map(|n| format!("#{n} ")).unwrap_or_default();
        let description = if self.description.is_empty() {
            "(no description)"
        } else {
            &self.description
        };
        let modality = if self.modality.is_empty() {
            String::new()
        } else {
            format!("{}, ", self.modality)
        };
        format!("{number}{description} ({modality}{})", self.files.len())
    }
}

/// 1 つの検査とそのシリーズ。
#[derive(Debug, Clone, PartialEq)]
pub struct Study {
    pub uid: String,
    pub description: String,
    /// `YYYYMMDD`。
    pub date: String,
    /// Series Number の順。
    pub series: Vec<Series>,
}

impl Study {
    /// ツリーに出す名前。例: `Head 2026-10-01`。
    pub fn title(&self) -> String {
        let description = if self.description.is_empty() {
            "(no description)"
        } else {
            &self.description
        };
        let d = &self.date;
        if d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()) {
            format!("{description} {}-{}-{}", &d[..4], &d[4..6], &d[6..])
        } else {
            description.to_owned()
        }
    }
}

/// ヘッダを読んだファイルを 検査 → シリーズ に分ける。DICOM でないファイル（Series UID が
/// ないもの）は含めない。検査は日付順、シリーズは番号順、ファイルは Instance Number 順。
pub fn group(entries: Vec<(PathBuf, Metadata)>) -> Vec<Study> {
    let mut studies: Vec<Study> = Vec::new();
    let mut instance: HashMap<PathBuf, Option<i64>> = HashMap::new();
    for (path, m) in entries {
        let get = |k| m.get("DICOM", k).unwrap_or_default().to_owned();
        let series_uid = get("Series UID");
        if series_uid.is_empty() {
            continue;
        }
        let study_uid = get("Study UID");
        let study = match studies.iter().position(|s| s.uid == study_uid) {
            Some(i) => &mut studies[i],
            None => {
                studies.push(Study {
                    uid: study_uid,
                    description: get("Study"),
                    date: get("Study date"),
                    series: Vec::new(),
                });
                studies.last_mut().expect("just pushed")
            }
        };
        let series = match study.series.iter().position(|s| s.uid == series_uid) {
            Some(i) => &mut study.series[i],
            None => {
                study.series.push(Series {
                    uid: series_uid,
                    number: get("Series number").trim().parse().ok(),
                    description: get("Series"),
                    modality: get("Modality"),
                    files: Vec::new(),
                });
                study.series.last_mut().expect("just pushed")
            }
        };
        instance.insert(path.clone(), get("Instance number").trim().parse().ok());
        series.files.push(path);
    }
    studies.sort_by(|a, b| (&a.date, &a.description).cmp(&(&b.date, &b.description)));
    for study in &mut studies {
        study
            .series
            .sort_by_key(|s| (s.number.is_none(), s.number, s.description.clone()));
        for series in &mut study.series {
            // 番号のないファイルは後ろに回す。
            series
                .files
                .sort_by_key(|p| (instance[p].is_none(), instance[p], p.clone()));
        }
    }
    studies
}

/// DICOM のヘッダだけを読む。DICOM でなければ `None`。
fn read_header(registry: &Registry, path: &Path) -> Option<Metadata> {
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(HEADER_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    let plugin = registry.find(path, &head)?;
    if plugin.name() != "DICOM" {
        return None;
    }
    plugin.read_metadata(&head).ok().or_else(|| {
        let all = std::fs::read(path).ok()?;
        plugin.read_metadata(&all).ok()
    })
}

/// フォルダごとの、検査とシリーズの一覧。
pub enum Scan<'a> {
    /// まだ読んでいない。
    NotStarted,
    Reading,
    Done(&'a [Study]),
}

/// フォルダの DICOM のヘッダを別スレッドで読んで、検査とシリーズの一覧を持つ。
#[derive(Default)]
pub struct SeriesIndex {
    reading: HashMap<PathBuf, Receiver<Vec<Study>>>,
    done: HashMap<PathBuf, Vec<Study>>,
}

impl SeriesIndex {
    pub fn scan(&self, dir: &Path) -> Scan<'_> {
        if let Some(studies) = self.done.get(dir) {
            Scan::Done(studies)
        } else if self.reading.contains_key(dir) {
            Scan::Reading
        } else {
            Scan::NotStarted
        }
    }

    /// `dir` の `files` のヘッダを読み始める。終わったら `wake` を呼ぶ。
    pub fn start(
        &mut self,
        dir: PathBuf,
        files: Vec<PathBuf>,
        registry: Registry,
        wake: impl Fn() + Send + 'static,
    ) {
        let (tx, rx) = channel();
        self.reading.insert(dir, rx);
        std::thread::spawn(move || {
            let entries = files
                .into_iter()
                .filter_map(|p| Some((p.clone(), read_header(&registry, &p)?)))
                .collect();
            let _ = tx.send(group(entries));
            wake();
        });
    }

    /// 読み終わったフォルダの結果を受け取る。何か受け取ったら `true`。
    pub fn poll(&mut self) -> bool {
        let mut finished = Vec::new();
        for (dir, rx) in &self.reading {
            match rx.try_recv() {
                Ok(studies) => finished.push((dir.clone(), Some(studies))),
                Err(TryRecvError::Disconnected) => finished.push((dir.clone(), None)),
                Err(TryRecvError::Empty) => {}
            }
        }
        let changed = !finished.is_empty();
        for (dir, studies) in finished {
            self.reading.remove(&dir);
            self.done.insert(dir, studies.unwrap_or_default());
        }
        changed
    }

    /// フォルダの中身が変わったので、次に要るときに読み直す。
    pub fn forget(&mut self, dir: &Path) {
        self.done.remove(dir);
        self.reading.remove(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(study: &str, date: &str, series: &str, number: &str, instance: &str) -> Metadata {
        let mut m = Metadata::default();
        m.push("DICOM", "Study UID", study);
        m.push("DICOM", "Study date", date);
        m.push("DICOM", "Series UID", series);
        m.push("DICOM", "Series number", number);
        m.push("DICOM", "Instance number", instance);
        m
    }

    #[test]
    fn files_are_grouped_by_study_and_series_in_instance_order() {
        let entries = vec![
            (PathBuf::from("c"), meta("s2", "20261005", "b", "1", "1")),
            (PathBuf::from("z"), meta("s1", "20261001", "a2", "2", "2")),
            (PathBuf::from("y"), meta("s1", "20261001", "a1", "1", "1")),
            (PathBuf::from("x"), meta("s1", "20261001", "a2", "2", "1")),
        ];
        let studies = group(entries);
        let shape: Vec<String> = studies
            .iter()
            .flat_map(|s| {
                s.series.iter().map(|q| {
                    let files: Vec<_> = q.files.iter().map(|p| p.to_str().unwrap()).collect();
                    format!("{}/{}: {}", s.uid, q.uid, files.join(" "))
                })
            })
            .collect();
        assert_eq!(shape, ["s1/a1: y", "s1/a2: x z", "s2/b: c"]);
    }

    #[test]
    fn files_without_a_series_uid_are_left_out() {
        let studies = group(vec![(PathBuf::from("a.png"), Metadata::default())]);
        assert!(studies.is_empty());
    }

    #[test]
    fn titles_show_number_description_modality_and_count() {
        let series = Series {
            uid: "1".into(),
            number: Some(2),
            description: "Axial".into(),
            modality: "CT".into(),
            files: vec![PathBuf::from("a"), PathBuf::from("b")],
        };
        assert_eq!(series.title(), "#2 Axial (CT, 2)");
        let study = Study {
            uid: "1".into(),
            description: "Head".into(),
            date: "20261001".into(),
            series: vec![],
        };
        assert_eq!(study.title(), "Head 2026-10-01");
    }

    #[test]
    fn index_reads_dicom_headers_in_the_background() {
        use crate::test_util::{all_registry, TempDir};
        use vw_dicom::synth::Synth;
        let dir = TempDir::new();
        let files: Vec<PathBuf> = (1..=2)
            .map(|i| {
                let p = dir.path().join(format!("{i}.dcm"));
                Synth::ct(1, 1)
                    .instance(&format!("1.2.{i}"), 3 - i)
                    .write(&p)
                    .unwrap();
                p
            })
            .collect();
        let mut index = SeriesIndex::default();
        index.start(
            dir.path().to_path_buf(),
            files.clone(),
            all_registry(),
            || {},
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !index.poll() {
            assert!(std::time::Instant::now() < deadline, "scan did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Scan::Done(studies) = index.scan(dir.path()) else {
            panic!("scan should be done");
        };
        assert_eq!(
            studies[0].series[0].files,
            [files[1].clone(), files[0].clone()]
        );
    }
}
