//! フォルダの DICOM を 検査 → シリーズ にまとめる。
//!
//! ファイルのヘッダ（画素の手前まで）を別スレッドで読み、Study / Series Instance UID で
//! 分ける。CT の 1 シリーズは数百ファイルあり、全部を UI スレッドで読むと固まるため。

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use vw_core::{Metadata, Registry};

/// ヘッダを読むときに最初に読むバイト数。画素データの手前までがたいていこれに収まり、
/// 1 ファイル数百 KB の CT でも全体を読まずに済む。収まらなければ画素データの手前まで読み足す。
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
    /// `files` のうち、フレームが 2 枚以上あるもの（同じ順）。
    pub multiframe: Vec<PathBuf>,
}

/// シリーズをスロットに並べるときの 1 つ分。
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    /// 1 枚ずつのファイルをまとめた並び。ホイールでファイルを送る。
    Files(Vec<PathBuf>),
    /// 1 ファイル。マルチフレームなら、スロットの中でフレームを送る。
    File(PathBuf),
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

    /// スロットに並べる形。1 枚ずつのファイルが 2 つ以上あれば 1 つの並びにまとめ、その後ろに
    /// マルチフレームのファイル（と、まとめなかった 1 枚のファイル）を Instance Number の順に置く。
    /// マルチフレームを並びに入れないのは、並びはファイルを送るだけで、各ファイルの 2 フレーム目
    /// 以降に行けないため（トモシンセシスは左右・向きごとのファイルが 1 シリーズに入る）。
    pub fn parts(&self) -> Vec<Part> {
        let singles: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|f| !self.multiframe.contains(f))
            .cloned()
            .collect();
        if singles.len() < 2 {
            return self.files.iter().cloned().map(Part::File).collect();
        }
        std::iter::once(Part::Files(singles))
            .chain(self.multiframe.iter().cloned().map(Part::File))
            .collect()
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
                    multiframe: Vec::new(),
                });
                study.series.last_mut().expect("just pushed")
            }
        };
        instance.insert(path.clone(), get("Instance number").trim().parse().ok());
        if get("Number of frames")
            .trim()
            .parse::<usize>()
            .is_ok_and(|n| n > 1)
        {
            series.multiframe.push(path.clone());
        }
        series.files.push(path);
    }
    studies.sort_by(|a, b| (&a.date, &a.description).cmp(&(&b.date, &b.description)));
    for study in &mut studies {
        study
            .series
            .sort_by_key(|s| (s.number.is_none(), s.number, s.description.clone()));
        for series in &mut study.series {
            // 番号のないファイルは後ろに回す。
            let order = |p: &PathBuf| (instance[p].is_none(), instance[p], p.clone());
            series.files.sort_by_key(order);
            series.multiframe.sort_by_key(order);
        }
    }
    studies
}

/// 1 シリーズだけのフォルダを並べる鍵。検査の日付 → 検査の説明 → シリーズ番号の順に並ぶ
/// （[`group`] と同じ順）。番号のないシリーズは後ろに回す。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FolderSeries {
    study_date: String,
    study_description: String,
    study_uid: String,
    no_number: bool,
    number: Option<i64>,
    description: String,
}

impl FolderSeries {
    /// ヘッダから鍵と、シリーズの UID を作る。DICOM でなければ `None`。
    fn of(m: &Metadata) -> Option<(String, Self)> {
        let get = |k| m.get("DICOM", k).unwrap_or_default().to_owned();
        let uid = get("Series UID");
        if uid.is_empty() {
            return None;
        }
        let number: Option<i64> = get("Series number").trim().parse().ok();
        Some((
            uid,
            Self {
                study_date: get("Study date"),
                study_description: get("Study"),
                study_uid: get("Study UID"),
                no_number: number.is_none(),
                number,
                description: get("Series"),
            },
        ))
    }
}

/// フォルダ `dir` が DICOM の 1 シリーズだけを入れているなら、並べる鍵を返す。
///
/// 読むのは名前の順で先頭と末尾のファイルのヘッダだけ。全部を読めば確実だが、CT の
/// シリーズのフォルダが並ぶと検査 1 つで数百 MB を読むことになり、ネットワークドライブで
/// ツリーの先読みが詰まるため。サブフォルダや ZIP、DICOM でない画像があれば 1 シリーズの
/// フォルダとはみなさない。
pub fn folder_series(dir: &Path, registry: &Registry) -> Option<FolderSeries> {
    let extensions = crate::tree::dicom_extensions(registry);
    let mut first: Option<PathBuf> = None;
    let mut last: Option<PathBuf> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name();
        if crate::tree::is_hidden_name(&name.to_string_lossy()) {
            continue;
        }
        let path = e.path();
        if crate::tree::entry_is_dir(&e) || crate::archive::is_archive(&path) {
            return None;
        }
        if !registry.supports_file(&path) {
            continue;
        }
        if !crate::tree::may_be_dicom(registry, extensions, &path) {
            return None;
        }
        let by_name = |a: &PathBuf, b: &PathBuf| {
            crate::sort::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy())
        };
        if first.as_ref().is_none_or(|f| by_name(&path, f).is_lt()) {
            first = Some(path.clone());
        }
        if last.as_ref().is_none_or(|l| by_name(&path, l).is_gt()) {
            last = Some(path);
        }
    }
    let (first_uid, key) = FolderSeries::of(&read_header(registry, &first?)?)?;
    let last = last.expect("set together with first");
    let (last_uid, _) = FolderSeries::of(&read_header(registry, &last)?)?;
    (first_uid == last_uid).then_some(key)
}

/// `items` のうち鍵のあるもの（1 シリーズのフォルダ）だけを、それらが占めていた位置の中で
/// 鍵の順に並べ直す。ほかの項目の位置は動かさない。
///
/// 並び順の比べ方に混ぜないのは、ファイルの並びの設定（名前の降順など）とシリーズ番号の
/// 昇順を 1 つの比べ方にすると、鍵のあるものとないものの間で順序が矛盾するため。
/// 並べ替えたら `true`。
pub fn place_in_series_order<T>(
    items: &mut Vec<T>,
    key: impl Fn(&T) -> Option<FolderSeries>,
) -> bool {
    let mut keyed: Vec<(usize, FolderSeries)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| Some((i, key(item)?)))
        .collect();
    if keyed.is_sorted_by(|a, b| a.1 <= b.1) {
        return false;
    }
    keyed.sort_by(|a, b| a.1.cmp(&b.1));
    let mut slots: Vec<Option<T>> = std::mem::take(items).into_iter().map(Some).collect();
    let mut movers = keyed
        .iter()
        .map(|(i, _)| slots[*i].take().expect("each index is taken once"))
        .collect::<Vec<T>>()
        .into_iter();
    *items = slots
        .into_iter()
        .map(|s| s.unwrap_or_else(|| movers.next().expect("one mover per emptied place")))
        .collect();
    true
}

/// フォルダのスロットを開いて読んだ画像 `paths` を、DICOM のシリーズ（2 枚以上のもの）と
/// 残りに分ける。シリーズは、フォルダごとのヘッダを読んだ結果 `studies` の順
/// （検査 → シリーズ番号）で、中は Instance Number の順。残りは `paths` の順のまま。
pub fn split_into_series<'a>(
    paths: &[PathBuf],
    studies: impl IntoIterator<Item = &'a Study>,
) -> (Vec<Series>, Vec<PathBuf>) {
    let wanted: std::collections::HashSet<&PathBuf> = paths.iter().collect();
    let mut taken = std::collections::HashSet::new();
    let mut found = Vec::new();
    for study in studies {
        for series in &study.series {
            let files: Vec<PathBuf> = series
                .files
                .iter()
                .filter(|f| wanted.contains(f))
                .cloned()
                .collect();
            if files.len() < 2 {
                continue;
            }
            taken.extend(files.iter().cloned());
            found.push(Series {
                multiframe: series
                    .multiframe
                    .iter()
                    .filter(|f| wanted.contains(f))
                    .cloned()
                    .collect(),
                files,
                ..series.clone()
            });
        }
    }
    let rest = paths
        .iter()
        .filter(|p| !taken.contains(*p))
        .cloned()
        .collect();
    (found, rest)
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
        // 画素データの手前まで読み足す。全体を読み直すと、数 GB のマルチフレームを丸ごと読んでしまう。
        let head = vw_dicom::frame_reader::read_head_file(path).ok()?;
        plugin.read_metadata(&head).ok()
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

    fn frames(mut m: Metadata, n: &str) -> Metadata {
        m.push("DICOM", "Number of frames", n);
        m
    }

    #[test]
    fn multiframe_files_of_a_series_get_their_own_parts_after_the_single_files() {
        let entries = vec![
            (
                PathBuf::from("tomo-l"),
                frames(meta("s", "", "a", "1", "4"), "50"),
            ),
            (PathBuf::from("2d-r"), meta("s", "", "a", "1", "1")),
            (
                PathBuf::from("tomo-r"),
                frames(meta("s", "", "a", "1", "3"), "48"),
            ),
            (
                PathBuf::from("2d-l"),
                frames(meta("s", "", "a", "1", "2"), "1"),
            ),
        ];
        let series = &group(entries)[0].series[0];
        assert_eq!(
            series.parts(),
            [
                Part::Files(vec![PathBuf::from("2d-r"), PathBuf::from("2d-l")]),
                Part::File(PathBuf::from("tomo-r")),
                Part::File(PathBuf::from("tomo-l")),
            ]
        );
    }

    #[test]
    fn a_series_of_only_multiframe_files_is_one_part_per_file() {
        let entries = vec![
            (
                PathBuf::from("b"),
                frames(meta("s", "", "a", "1", "2"), "30"),
            ),
            (
                PathBuf::from("a"),
                frames(meta("s", "", "a", "1", "1"), "30"),
            ),
            (PathBuf::from("c"), meta("s", "", "a", "1", "3")),
        ];
        let series = &group(entries)[0].series[0];
        assert_eq!(
            series.parts(),
            [
                Part::File(PathBuf::from("a")),
                Part::File(PathBuf::from("b")),
                Part::File(PathBuf::from("c")),
            ]
        );
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
            multiframe: vec![],
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
        use crate::test_util::{TempDir, all_registry};
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

    /// フォルダ `dir` に、シリーズ `uid`（番号 `number`）の DICOM を `instances` の順で書く。
    /// ファイル名は UID 風のばらばらにし、書いた順（更新日時の順）も Instance Number と合わせない。
    fn write_series(dir: &Path, uid: &str, number: i32, instances: &[i32]) -> Vec<PathBuf> {
        use vw_dicom::synth::Synth;
        std::fs::create_dir_all(dir).unwrap();
        instances
            .iter()
            .map(|&n| {
                let p = dir.join(format!("1.2.{}.{}.dcm", 97 - n * 7 % 13, n));
                Synth::ct(1, 1)
                    .study("1.9", "Head", "20261105")
                    .series(uid, number, &format!("S{number}"))
                    .instance(&format!("{uid}.{n}"), n)
                    .write(&p)
                    .unwrap();
                p
            })
            .collect()
    }

    fn key(dir: &Path) -> Option<FolderSeries> {
        folder_series(dir, &crate::test_util::all_registry())
    }

    #[test]
    fn a_folder_of_one_series_gets_a_key_ordered_by_series_number() {
        let root = crate::test_util::TempDir::new();
        write_series(&root.path().join("a"), "1.3", 3, &[4, 1, 5, 2, 3]);
        write_series(&root.path().join("b"), "1.1", 1, &[2, 1]);
        write_series(&root.path().join("c"), "1.0", 10, &[1, 2]);
        let (a, b, c) = (
            key(&root.path().join("a")),
            key(&root.path().join("b")),
            key(&root.path().join("c")),
        );
        assert!(b.as_ref().unwrap() < a.as_ref().unwrap());
        assert!(a.as_ref().unwrap() < c.as_ref().unwrap());
    }

    #[test]
    fn a_folder_with_two_series_subfolders_or_other_images_gets_no_key() {
        let root = crate::test_util::TempDir::new();
        let two = root.path().join("two");
        write_series(&two, "1.1", 1, &[1]);
        // 名前の順で末尾に来るファイルを別のシリーズにする。
        vw_dicom::synth::Synth::ct(1, 1)
            .series("1.2", 2, "other")
            .write(&two.join("zz.dcm"))
            .unwrap();
        assert_eq!(key(&two), None);

        let nested = root.path().join("nested");
        write_series(&nested, "1.1", 1, &[1, 2]);
        std::fs::create_dir(nested.join("sub")).unwrap();
        assert_eq!(key(&nested), None);

        let mixed = root.path().join("mixed");
        write_series(&mixed, "1.1", 1, &[1, 2]);
        std::fs::write(mixed.join("a.png"), crate::test_util::png_bytes(1, 1)).unwrap();
        assert_eq!(key(&mixed), None);

        let plain = root.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        std::fs::write(plain.join("a.png"), crate::test_util::png_bytes(1, 1)).unwrap();
        assert_eq!(key(&plain), None);
    }

    #[test]
    fn series_folders_are_reordered_only_among_their_own_places() {
        let k = |n: i64| FolderSeries {
            study_date: "20261105".into(),
            study_description: String::new(),
            study_uid: "1".into(),
            no_number: false,
            number: Some(n),
            description: String::new(),
        };
        // 名前の降順で並んだフォルダ。x と y はシリーズのフォルダでない。
        let mut items = vec![
            ("x", None),
            ("S3", Some(3)),
            ("y", None),
            ("S2", Some(2)),
            ("S1", Some(1)),
        ];
        assert!(place_in_series_order(&mut items, |(_, n)| n.map(k)));
        let names: Vec<_> = items.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["x", "S1", "y", "S2", "S3"]);
        assert!(!place_in_series_order(&mut items, |(_, n)| n.map(k)));
    }

    #[test]
    fn opened_folder_images_split_into_series_in_instance_order_and_the_rest() {
        let root = crate::test_util::TempDir::new();
        let dir = root.path().join("S2");
        let files = write_series(&dir, "1.2", 2, &[4, 1, 5, 2, 3]);
        let png = dir.join("note.png");
        std::fs::write(&png, crate::test_util::png_bytes(1, 1)).unwrap();
        let mut index = SeriesIndex::default();
        index.start(
            dir.clone(),
            files.clone(),
            crate::test_util::all_registry(),
            || {},
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !index.poll() {
            assert!(std::time::Instant::now() < deadline, "scan did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Scan::Done(studies) = index.scan(&dir) else {
            panic!("scan should be done");
        };
        // 開いたスロットには、ファイルの並びの設定の順（ここでは書いた順）で入っている。
        let mut opened = files.clone();
        opened.push(png.clone());
        let (series, rest) = split_into_series(&opened, studies);
        assert_eq!(series.len(), 1);
        let by_instance = [1, 3, 4, 0, 2].map(|i| files[i].clone());
        assert_eq!(series[0].files, by_instance);
        assert_eq!(rest, [png]);
    }
}
