//! ファイルのスロットの並び順。フォルダの並び（[`crate::sort`]、ツリーの設定）とは別に持ち、
//! ファイルの種類（写真・DICOM・FITS・そのほか）ごとに基準を決められる。
//!
//! 種類が混ざったフォルダは、既定では 1 列に並べ、基準の型（名前／日時など）だけを
//! そろえる（[`Mix::OneLine`]）。日時は種類ごとに意味が違い、写真は撮影日時、DICOM は
//! 検査の日時、FITS は観測の日時（`DATE-OBS`）、そのほかは更新日時を使う。
//! [`Mix::ByKind`] にすると種類ごとにまとめ、写真 → DICOM → FITS → そのほか の順につなぐ。

use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use vw_core::Registry;

use crate::sort::{SortKey, SortOrder, natural_cmp, string_cmp};

/// ファイルを並べる基準。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKey {
    /// ファイル名。数字の並びは数値として比べる。
    #[default]
    NameNatural,
    /// ファイル名を 1 文字ずつ比べる。
    NameString,
    /// 更新日時。
    Modified,
    /// データの日時（写真は撮影日時、DICOM は検査の日時、FITS は観測の日時）。
    /// 読めないファイルは更新日時で代える。
    Date,
    /// ファイルの大きさ。
    Size,
}

impl FileKey {
    pub const ALL: [FileKey; 5] = [
        FileKey::NameNatural,
        FileKey::NameString,
        FileKey::Modified,
        FileKey::Date,
        FileKey::Size,
    ];

    pub fn title(self) -> &'static str {
        match self {
            FileKey::NameNatural => "Name (numbers by value)",
            FileKey::NameString => "Name (character by character)",
            FileKey::Modified => "Modified date",
            FileKey::Date => "Date of the data (taken, study, observed)",
            FileKey::Size => "File size",
        }
    }

    /// 切り替えのキーで次に選ぶ基準。[`FileKey::ALL`] の順に回る。
    pub fn next(self) -> FileKey {
        let at = FileKey::ALL.iter().position(|&k| k == self).unwrap_or(0);
        FileKey::ALL[(at + 1) % FileKey::ALL.len()]
    }

    /// フォルダの並びの基準に当たるもの。ファイルの並びをまだ書いていない設定ファイルで、
    /// 今までの並び（フォルダとファイルが 1 つの設定だった）を引き継ぐのに使う。
    pub fn from_sort_key(key: SortKey) -> FileKey {
        match key {
            SortKey::NameNatural => FileKey::NameNatural,
            SortKey::NameString => FileKey::NameString,
            SortKey::Modified => FileKey::Modified,
        }
    }
}

/// ファイルの種類。並べ方を種類ごとに持つ単位で、[`Mix::ByKind`] ではこの順につなぐ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// JPEG・WebP・カメラの RAW。
    Photo,
    Dicom,
    Fits,
    /// PNG・BMP・TIFF・GIF など。
    Other,
}

impl Kind {
    /// 拡張子から種類と、同じ名前の組（`IMG_0001.JPG` と `IMG_0001.ARW`）を並べる順を決める。
    /// 中身は読まない。拡張子で分からず中身で調べるファイル（`IM0001` など）は DICOM。
    fn of(registry: &Registry, path: &Path) -> (Kind, u8) {
        match registry.plugin_name_by_extension(path) {
            Some("JPEG" | "WebP" | "HEIC") => (Kind::Photo, 0),
            Some("RAW") => (Kind::Photo, 1),
            Some("DICOM") => (Kind::Dicom, 0),
            Some(vw_fits::NAME) => (Kind::Fits, 0),
            None if registry.needs_probe(path) => (Kind::Dicom, 0),
            _ => (Kind::Other, 0),
        }
    }
}

/// 種類が混ざったフォルダの並べ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mix {
    /// 1 列に並べ、基準（[`FileOrder::key`]）の型をそろえる。同じ名前の組が隣に並ぶ。
    #[default]
    OneLine,
    /// 種類ごとに、その種類の設定で並べ、写真 → DICOM → FITS → そのほか の順につなぐ。
    ByKind,
}

impl Mix {
    pub const ALL: [Mix; 2] = [Mix::OneLine, Mix::ByKind];

    pub fn title(self) -> &'static str {
        match self {
            Mix::OneLine => "Mixed in one line",
            Mix::ByKind => "Grouped by type",
        }
    }

    pub fn toggled(self) -> Mix {
        match self {
            Mix::OneLine => Mix::ByKind,
            Mix::ByKind => Mix::OneLine,
        }
    }
}

/// 1 つの種類の並べ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindOrder {
    pub key: FileKey,
    pub descending: bool,
}

impl KindOrder {
    const fn ascending(key: FileKey) -> Self {
        Self {
            key,
            descending: false,
        }
    }
}

/// `[file_sort]`: ファイルのスロットと、前後の画像への移動の並び。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FileOrder {
    /// 1 列に並べるときの基準。
    pub key: FileKey,
    pub descending: bool,
    pub mix: Mix,
    /// 種類ごとにまとめるときの、それぞれの並べ方。
    pub photo: KindOrder,
    pub dicom: KindOrder,
    pub fits: KindOrder,
    pub other: KindOrder,
}

impl Default for FileOrder {
    /// 1 列の基準と向きはツリーの既定（`[tree]`）と同じ。設定ファイルに書いていないときに
    /// ツリーの並びを引き継ぐのと合わせるため。
    fn default() -> Self {
        Self {
            key: FileKey::NameNatural,
            descending: true,
            mix: Mix::OneLine,
            photo: KindOrder::ascending(FileKey::Date),
            dicom: KindOrder::ascending(FileKey::Date),
            fits: KindOrder::ascending(FileKey::Date),
            other: KindOrder::ascending(FileKey::NameNatural),
        }
    }
}

impl FileOrder {
    /// 種類を分けずに 1 つの基準で並べる（全画面のスライドショーなど、自分の並びを持つ画面用）。
    pub fn plain(order: SortOrder) -> Self {
        Self {
            key: FileKey::from_sort_key(order.key),
            descending: order.descending,
            ..Self::default()
        }
    }

    pub fn of_kind(&self, kind: Kind) -> KindOrder {
        match kind {
            Kind::Photo => self.photo,
            Kind::Dicom => self.dicom,
            Kind::Fits => self.fits,
            Kind::Other => self.other,
        }
    }

    pub fn of_kind_mut(&mut self, kind: Kind) -> &mut KindOrder {
        match kind {
            Kind::Photo => &mut self.photo,
            Kind::Dicom => &mut self.dicom,
            Kind::Fits => &mut self.fits,
            Kind::Other => &mut self.other,
        }
    }

    /// 種類 `kind` のファイルに使う基準と向き。
    fn for_kind(&self, kind: Kind) -> KindOrder {
        match self.mix {
            Mix::OneLine => KindOrder {
                key: self.key,
                descending: self.descending,
            },
            Mix::ByKind => self.of_kind(kind),
        }
    }

    /// ファイル名だけで比べるときの近い並び。消えたファイル（もう日時を読めない）の
    /// 位置を見積もるのに使う。
    pub fn by_name(&self) -> SortOrder {
        SortOrder {
            key: if self.key == FileKey::NameString {
                SortKey::NameString
            } else {
                SortKey::NameNatural
            },
            descending: self.descending,
        }
    }
}

/// フォルダとファイルの並び。フォルダを辿る一覧（[`crate::tree::list_folder`]）に渡す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ListOrder {
    pub folders: SortOrder,
    pub files: FileOrder,
}

impl ListOrder {
    /// フォルダとファイルを同じ基準で並べる（自分の並びを持つ画面用）。
    pub fn plain(folders: SortOrder, files: SortOrder) -> Self {
        Self {
            folders,
            files: FileOrder::plain(files),
        }
    }
}

/// データの日時。比べられるよう、数字だけを `YYYYMMDDhhmmss…` に並べた文字列にする。
/// 同じ日時の DICOM はシリーズ番号、Instance Number の順にする。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Stamp {
    when: String,
    series: i64,
    instance: i64,
}

impl Stamp {
    /// `YYYYMMDDhhmmss` とミリ秒の 3 桁。
    const DIGITS: usize = 17;

    fn new(text: &str) -> Option<Self> {
        let mut digits: String = text.chars().filter(char::is_ascii_digit).collect();
        if digits.len() < 8 {
            return None;
        }
        // 文字列のまま比べるので、時刻のない日付・小数秒のない時刻・ミリ秒より細かい時刻を
        // ミリ秒までの 17 桁にそろえる。桁が違うと同じ時刻でも同点にならない。
        digits.truncate(Stamp::DIGITS);
        while digits.len() < Stamp::DIGITS {
            digits.push('0');
        }
        Some(Self {
            when: digits,
            series: i64::MAX,
            instance: i64::MAX,
        })
    }

    /// 撮影日時は撮った場所の時刻で書かれているので、更新日時もその場の時刻（ローカル）にして比べる。
    fn of_modified(t: SystemTime) -> Option<Self> {
        let local: chrono::DateTime<chrono::Local> = t.into();
        Self::new(&local.format("%Y%m%d%H%M%S%.3f").to_string())
    }
}

/// 並べるのに使う、1 つのファイルの情報。基準に使うものだけを読む。
struct Facts {
    name: String,
    /// 拡張子を除いた名前。同じ名前の組を隣に置くため、名前の比べはこれで行う。
    stem: String,
    kind: Kind,
    sub: u8,
    order: KindOrder,
    modified: Option<SystemTime>,
    size: Option<u64>,
    date: Option<Stamp>,
}

impl Facts {
    fn read(path: &Path, registry: &Registry, files: &FileOrder) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());
        let (kind, sub) = Kind::of(registry, path);
        let order = files.for_kind(kind);
        let meta = matches!(order.key, FileKey::Modified | FileKey::Date | FileKey::Size)
            .then(|| std::fs::metadata(path).ok())
            .flatten();
        let modified = meta.as_ref().and_then(|m| m.modified().ok());
        let size = meta.as_ref().map(std::fs::Metadata::len);
        let date = (order.key == FileKey::Date)
            .then(|| {
                data_date(path, kind, registry, modified)
                    .or_else(|| modified.and_then(Stamp::of_modified))
            })
            .flatten();
        Self {
            name,
            stem,
            kind,
            sub,
            order,
            modified,
            size,
            date,
        }
    }
}

/// `a` と `b` を比べる。基準が同じなら、拡張子を除いた名前 → 種類（写真 → RAW など）→
/// ファイル名の順に決める。同じ名前で拡張子だけ違う組が、どの基準でも隣に並ぶようにするため。
fn compare(a: &Facts, b: &Facts, mix: Mix) -> Ordering {
    if mix == Mix::ByKind && a.kind != b.kind {
        return a.kind.cmp(&b.kind);
    }
    let order = a.order;
    let by_name = |cmp: fn(&str, &str) -> Ordering| cmp(&a.stem, &b.stem);
    let by_key = match order.key {
        FileKey::NameNatural => by_name(natural_cmp),
        FileKey::NameString => by_name(string_cmp),
        FileKey::Modified => a.modified.cmp(&b.modified),
        FileKey::Date => a.date.cmp(&b.date),
        FileKey::Size => a.size.cmp(&b.size),
    };
    let by_key = if order.descending {
        by_key.reverse()
    } else {
        by_key
    };
    by_key
        .then_with(|| natural_cmp(&a.stem, &b.stem))
        .then_with(|| (a.kind, a.sub).cmp(&(b.kind, b.sub)))
        .then_with(|| natural_cmp(&a.name, &b.name))
}

/// `paths`（同じフォルダのファイル）を `files` の順に並べる。
pub fn sort_files(paths: &mut Vec<PathBuf>, files: &FileOrder, registry: &Registry) {
    let mut keyed: Vec<(Facts, PathBuf)> = std::mem::take(paths)
        .into_iter()
        .map(|p| (Facts::read(&p, registry, files), p))
        .collect();
    keyed.sort_by(|a, b| compare(&a.0, &b.0, files.mix));
    *paths = keyed.into_iter().map(|(_, p)| p).collect();
}

/// 読んだデータの日時。フォルダを開き直すたびにヘッダを読み直さないよう、
/// 更新日時が変わるまで覚えておく。
type DateCache = Mutex<HashMap<PathBuf, (Option<SystemTime>, Option<Stamp>)>>;

fn cache() -> &'static DateCache {
    static CACHE: OnceLock<DateCache> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn data_date(
    path: &Path,
    kind: Kind,
    registry: &Registry,
    modified: Option<SystemTime>,
) -> Option<Stamp> {
    if let Some((when, stamp)) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(path)
        && *when == modified
    {
        return stamp.clone();
    }
    let stamp = match kind {
        Kind::Photo => vw_core::exif::taken_of_file(path).and_then(|t| Stamp::new(&t)),
        Kind::Dicom => dicom_date(path, registry),
        Kind::Fits => fits_date(path),
        Kind::Other => None,
    };
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.to_path_buf(), (modified, stamp.clone()));
    stamp
}

fn dicom_date(path: &Path, registry: &Registry) -> Option<Stamp> {
    let m = crate::series::read_header(registry, path)?;
    let get = |k| m.get("DICOM", k).unwrap_or_default().trim().to_owned();
    let number = |k| get(k).parse::<i64>().unwrap_or(i64::MAX);
    let mut stamp = Stamp::new(&format!("{}{}", get("Study date"), get("Study time")))?;
    stamp.series = number("Series number");
    stamp.instance = number("Instance number");
    Some(stamp)
}

/// FITS の最初のヘッダから `DATE-OBS` を読む。画像を読まずに済むよう、先頭だけを見る。
/// gzip で固めたファイルは展開しないと読めないので、更新日時で代える。
fn fits_date(path: &Path) -> Option<Stamp> {
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(64 * 1024)
        .read_to_end(&mut head)
        .ok()?;
    fits_date_of(&head)
}

fn fits_date_of(head: &[u8]) -> Option<Stamp> {
    for card in head.chunks(80) {
        let card = String::from_utf8_lossy(card);
        if card.starts_with("END") && card[3..].trim().is_empty() {
            return None;
        }
        if let Some(value) = card.strip_prefix("DATE-OBS=") {
            let value = value.split('/').next().unwrap_or_default();
            return Stamp::new(value.trim().trim_matches('\''));
        }
    }
    None
}

/// `items` のうち `key` の付くものだけを、それらが占めていた位置の中で `key` の順に並べ直す。
/// ほかの項目の位置は動かさない。並べ替えたら `true`。
///
/// スロットをまとめて並べ直さないのは、利用者が足した空のスロットや、別のフォルダから
/// 持ってきたスロットの位置まで動かしてしまうため。
pub fn place_in_order<T, K: Ord>(items: &mut Vec<T>, key: impl Fn(&T) -> Option<K>) -> bool {
    let mut keyed: Vec<(usize, K)> = items
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{TempDir, all_registry};
    use std::time::Duration;

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn registry() -> Registry {
        let mut r = all_registry();
        r.register(std::sync::Arc::new(vw_fits::FitsPlugin::new(
            vw_fits::SharedOptions::default(),
            vw_fits::Calibration::default(),
        )));
        r
    }

    fn sorted(dir: &Path, files: &[&str], order: &FileOrder) -> Vec<String> {
        let mut paths: Vec<PathBuf> = files.iter().map(|f| dir.join(f)).collect();
        sort_files(&mut paths, order, &registry());
        names(&paths)
    }

    fn touch(dir: &Path, name: &str, secs: u64, bytes: usize) {
        let path = dir.join(name);
        std::fs::write(&path, vec![0u8; bytes]).unwrap();
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    fn one_line(key: FileKey) -> FileOrder {
        FileOrder {
            key,
            descending: false,
            ..FileOrder::default()
        }
    }

    #[test]
    fn photo_and_raw_with_the_same_name_sit_next_to_each_other() {
        let dir = TempDir::new();
        let files = [
            "IMG_0002.JPG",
            "IMG_0001.ARW",
            "IMG_0001_b.png",
            "IMG_0001.JPG",
        ];
        assert_eq!(
            sorted(dir.path(), &files, &one_line(FileKey::NameNatural)),
            [
                "IMG_0001.JPG",
                "IMG_0001.ARW",
                "IMG_0001_b.png",
                "IMG_0002.JPG"
            ]
        );
    }

    #[test]
    fn same_picture_in_several_formats_stays_together_when_sorted_by_date() {
        let dir = TempDir::new();
        touch(dir.path(), "a.png", 30, 1);
        touch(dir.path(), "a.bmp", 10, 1);
        touch(dir.path(), "b.png", 20, 1);
        let order = one_line(FileKey::Modified);
        // a.bmp が最も古いので a の組が先、組の中は種類と名前の順。
        assert_eq!(
            sorted(dir.path(), &["b.png", "a.png", "a.bmp"], &order),
            ["a.bmp", "b.png", "a.png"]
        );
    }

    #[test]
    fn one_line_mixes_kinds_by_the_common_key() {
        let dir = TempDir::new();
        let files = ["c.png", "b.fits", "a.jpg", "d.dcm"];
        assert_eq!(
            sorted(dir.path(), &files, &one_line(FileKey::NameNatural)),
            ["a.jpg", "b.fits", "c.png", "d.dcm"]
        );
    }

    #[test]
    fn grouped_by_type_puts_photos_dicom_fits_then_others() {
        let dir = TempDir::new();
        let files = ["a.png", "b.fits", "c.dcm", "d.jpg", "IM0001"];
        let order = FileOrder {
            mix: Mix::ByKind,
            photo: KindOrder::ascending(FileKey::NameNatural),
            dicom: KindOrder::ascending(FileKey::NameNatural),
            fits: KindOrder::ascending(FileKey::NameNatural),
            ..FileOrder::default()
        };
        assert_eq!(
            sorted(dir.path(), &files, &order),
            ["d.jpg", "c.dcm", "IM0001", "b.fits", "a.png"]
        );
    }

    #[test]
    fn grouped_by_type_uses_each_types_own_direction() {
        let dir = TempDir::new();
        let files = ["a.jpg", "b.jpg", "a.png", "b.png"];
        let order = FileOrder {
            mix: Mix::ByKind,
            photo: KindOrder {
                key: FileKey::NameNatural,
                descending: true,
            },
            ..FileOrder::default()
        };
        assert_eq!(
            sorted(dir.path(), &files, &order),
            ["b.jpg", "a.jpg", "a.png", "b.png"]
        );
    }

    #[test]
    fn descending_reverses_the_key_but_keeps_pairs_in_type_order() {
        let dir = TempDir::new();
        let order = FileOrder {
            descending: true,
            ..one_line(FileKey::NameNatural)
        };
        assert_eq!(
            sorted(dir.path(), &["a.jpg", "b.arw", "b.jpg"], &order),
            ["b.jpg", "b.arw", "a.jpg"]
        );
    }

    #[test]
    fn size_order_puts_smaller_files_first() {
        let dir = TempDir::new();
        touch(dir.path(), "big.png", 0, 300);
        touch(dir.path(), "small.png", 0, 10);
        assert_eq!(
            sorted(
                dir.path(),
                &["big.png", "small.png"],
                &one_line(FileKey::Size)
            ),
            ["small.png", "big.png"]
        );
    }

    #[test]
    fn date_falls_back_to_modified_for_files_without_a_date() {
        let dir = TempDir::new();
        touch(dir.path(), "new.png", 20, 1);
        touch(dir.path(), "old.png", 10, 1);
        assert_eq!(
            sorted(
                dir.path(),
                &["new.png", "old.png"],
                &one_line(FileKey::Date)
            ),
            ["old.png", "new.png"]
        );
    }

    fn fits_header(cards: &[&str]) -> Vec<u8> {
        cards
            .iter()
            .flat_map(|c| format!("{c:<80}").into_bytes())
            .collect()
    }

    #[test]
    fn fits_date_is_read_from_date_obs() {
        let head = fits_header(&[
            "SIMPLE  =                    T",
            "DATE-OBS= '2024-01-02T03:04:05' / start",
            "END",
        ]);
        assert_eq!(fits_date_of(&head).unwrap().when, "20240102030405000");
    }

    #[test]
    fn fits_without_date_obs_has_no_date() {
        let head = fits_header(&[
            "SIMPLE  =                    T",
            "END",
            "DATE-OBS= '2024-01-02'",
        ]);
        assert!(fits_date_of(&head).is_none());
    }

    #[test]
    fn dates_without_time_compare_with_dates_with_time() {
        let day = Stamp::new("2024-01-02").unwrap();
        let later = Stamp::new("2024:01:02 00:00:01").unwrap();
        assert!(day < later);
        assert_eq!(day.when, "20240102000000000");
    }

    #[test]
    fn same_time_compares_equal_whatever_the_precision() {
        let exif = Stamp::new("2024:05:01 10:00:00").unwrap();
        let modified = Stamp::new("20240501100000.000").unwrap();
        let dicom = Stamp::new("20240501100000.000000").unwrap();
        assert_eq!(exif, modified);
        assert_eq!(exif, dicom);
        assert!(Stamp::new("20240501100000.001").unwrap() > exif);
    }

    #[test]
    fn dicom_of_one_study_follows_series_then_instance_number() {
        let at = |series, instance| Stamp {
            series,
            instance,
            ..Stamp::new("20240102").unwrap()
        };
        assert!(at(1, 9) < at(2, 1));
        assert!(at(2, 1) < at(2, 2));
    }

    #[test]
    fn place_in_order_moves_only_keyed_items() {
        let mut items = vec![("x", None), ("c", Some(3)), ("y", None), ("a", Some(1))];
        assert!(place_in_order(&mut items, |(_, k)| *k));
        let order: Vec<&str> = items.iter().map(|(n, _)| *n).collect();
        assert_eq!(order, ["x", "a", "y", "c"]);
        assert!(!place_in_order(&mut items, |(_, k)| *k));
    }

    #[test]
    fn next_file_key_cycles_through_all_keys() {
        let mut key = FileKey::NameNatural;
        for expected in &FileKey::ALL[1..] {
            key = key.next();
            assert_eq!(key, *expected);
        }
        assert_eq!(key.next(), FileKey::NameNatural);
    }
}
