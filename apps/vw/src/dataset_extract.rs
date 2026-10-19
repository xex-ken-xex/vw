//! データセットの取り出しと、表示するときの欠けの確認。
//!
//! 取り出しは、検索カタログにあるファイルを、データセットのフォルダからの相対パスのまま
//! 出力先へ写す。ZIP / 7z の中のファイルは展開し、アーカイブと同じ名前のフォルダの下に置く。
//! 出力先をデータセットとして登録するのは、頼まれたときだけ。取り出した先は評価などに使う
//! 作業用のフォルダであることが多く、検索カタログがあると検索の結果に紛れ込むため。

use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;

use vw_core::Registry;

use crate::ai_tags::AiTagStore;
use crate::archive::{self, ArchiveError, EntryError};
use crate::catalog::{self, Catalog, Info, Progress, Report, Skipped, Source, IN_ARCHIVE};
use crate::labels::LabelStore;

/// 取り出した結果。`failed` は写せなかったファイル（元から消えていたもの、出力先に同じ名前が
/// あったものなど）。
#[derive(Debug)]
pub struct Extracted {
    pub copied: usize,
    pub failed: Vec<Skipped>,
    /// 出力先をデータセットとして登録したときの結果。
    pub registered: Option<Report>,
}

/// 出力先として使えるか。中身のあるフォルダでもよい（同じ名前のファイルは上書きしない）。
pub fn check_destination(source: &Path, dest: &Path) -> Result<(), String> {
    if dest.as_os_str().is_empty() || !dest.is_absolute() {
        return Err("Enter the full path of the destination folder.".into());
    }
    // 元のデータセットの中に写すと、次に元を更新したとき写したファイルも数えてしまう。
    if dest.starts_with(source) {
        return Err("The destination is inside the dataset being extracted.".into());
    }
    if dest.exists() && !dest.is_dir() {
        return Err("The destination is not a folder.".into());
    }
    Ok(())
}

/// `source` のデータセット（検索カタログは `c`）のファイルを `dest` に写す。`register` があれば、
/// 写した後に `dest` をその名前などでデータセットとして登録し、元のデータセットを記録する。
pub fn extract(
    source: &Path,
    c: &Catalog,
    dest: &Path,
    register: Option<(Info, &Registry, &AiTagStore)>,
    progress: &Progress,
    ask_password: &mut dyn FnMut(&Path, bool) -> Option<String>,
) -> Result<Extracted, String> {
    check_destination(source, dest)?;
    if register.is_some() && catalog::exists_in(dest) {
        return Err("The destination is already a dataset.".into());
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let (copied, failed) = copy_files(source, c, dest, progress, ask_password)?;
    let registered = match register {
        Some((info, registry, ai_tags)) => {
            progress.done.store(0, Ordering::Relaxed);
            // 取り出したばかりのフォルダにラベルはまだないので、設定フォルダの控えは見ない。
            let labels = LabelStore::new(None);
            let mut report = catalog::scan(
                dest,
                info,
                None,
                registry,
                ai_tags,
                &labels,
                progress,
                ask_password,
            )?;
            report.catalog.source = Some(Source {
                name: c.name.clone(),
                path: source.display().to_string(),
            });
            catalog::save(dest, &report.catalog)?;
            Some(report)
        }
        None => None,
    };
    Ok(Extracted {
        copied,
        failed,
        registered,
    })
}

/// カタログ `c` のうち、`paths`（カタログのパス）のファイルだけを残したもの。検索で当たった
/// ファイルだけを取り出すときに、[`extract`] に渡す。
pub fn only(c: &Catalog, paths: &[String]) -> Catalog {
    let paths: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
    let mut out = c.clone();
    out.files.retain(|f| paths.contains(f.path.as_str()));
    out
}

/// 検索カタログにあるファイルを写し、写せた数と写せなかったものを返す。
fn copy_files(
    source: &Path,
    c: &Catalog,
    dest: &Path,
    progress: &Progress,
    ask_password: &mut dyn FnMut(&Path, bool) -> Option<String>,
) -> Result<(usize, Vec<Skipped>), String> {
    progress.total.store(c.files.len(), Ordering::Relaxed);
    let mut failed = Vec::new();
    let mut copied = 0;
    let mut archives: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for f in &c.files {
        if progress.cancelled() {
            return Err("Cancelled".into());
        }
        if let Some((archive, inner)) = f.path.split_once(IN_ARCHIVE) {
            archives.entry(archive).or_default().insert(inner);
            continue;
        }
        progress.step(&f.path);
        match copy_file(source, dest, &f.path) {
            Ok(()) => copied += 1,
            Err(e) => failed.push(catalog::skip(&f.path, e)),
        }
        progress.done.fetch_add(1, Ordering::Relaxed);
    }
    for (archive, wanted) in archives {
        if progress.cancelled() {
            return Err("Cancelled".into());
        }
        progress.step(archive);
        let result = unpack(source, dest, archive, &wanted, progress, ask_password);
        let unpacked =
            match result {
                Ok(u) => u,
                Err(reason) => {
                    failed.extend(wanted.iter().map(|inner| {
                        catalog::skip(&format!("{archive}{IN_ARCHIVE}{inner}"), &reason)
                    }));
                    progress.done.fetch_add(wanted.len(), Ordering::Relaxed);
                    continue;
                }
            };
        for inner in &wanted {
            let reason = if unpacked.existing.contains(*inner) {
                ALREADY_EXISTS
            } else if !unpacked.copied.contains(*inner) {
                "not found in the archive"
            } else {
                continue;
            };
            failed.push(catalog::skip(
                &format!("{archive}{IN_ARCHIVE}{inner}"),
                reason,
            ));
        }
        copied += unpacked.copied.len();
        progress.done.fetch_add(wanted.len(), Ordering::Relaxed);
    }
    if progress.cancelled() {
        return Err("Cancelled".into());
    }
    Ok((copied, failed))
}

fn copy_file(source: &Path, dest: &Path, path: &str) -> Result<(), String> {
    let relative = enclosed(path).ok_or("unsafe path in the search catalog")?;
    let target = dest.join(&relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if target.exists() {
        return Err(ALREADY_EXISTS.into());
    }
    std::fs::copy(source.join(&relative), &target)
        .map(drop)
        .map_err(|e| e.to_string())
}

const ALREADY_EXISTS: &str = "already exists in the destination";

/// アーカイブから展開したもの（中のパス）。`existing` は出力先に同じ名前があって写さなかったもの。
#[derive(Default)]
struct Unpacked {
    copied: BTreeSet<String>,
    existing: BTreeSet<String>,
}

/// アーカイブの中の `wanted` を `dest/<アーカイブの相対パス>/` の下に展開し、展開できたものを返す。
/// パスワードが違って途中で止まったら、その回に書いたファイルを消してから聞き直す。
fn unpack(
    source: &Path,
    dest: &Path,
    archive: &str,
    wanted: &BTreeSet<&str>,
    progress: &Progress,
    ask_password: &mut dyn FnMut(&Path, bool) -> Option<String>,
) -> Result<Unpacked, String> {
    let relative = enclosed(archive).ok_or("unsafe path in the search catalog")?;
    let path = source.join(&relative);
    let root = dest.join(&relative);
    let mut password: Option<String> = None;
    loop {
        let mut written: Vec<PathBuf> = Vec::new();
        let mut out = Unpacked::default();
        let result = archive::for_each_entry(&path, password.as_deref(), |inner, data| {
            if progress.cancelled() {
                return Err(EntryError::Other("Cancelled".into()));
            }
            let Some(data) = data else {
                return Ok(());
            };
            let name = catalog::slash_path(inner);
            if !wanted.contains(name.as_str()) {
                return Ok(());
            }
            let target = root.join(inner);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| EntryError::Other(e.to_string()))?;
            }
            let mut file = match std::fs::File::create_new(&target) {
                Ok(f) => f,
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    out.existing.insert(name);
                    return Ok(());
                }
                Err(e) => return Err(EntryError::Other(e.to_string())),
            };
            written.push(target);
            archive::copy_entry(data, &mut file)?;
            out.copied.insert(name);
            Ok(())
        });
        match result {
            Ok(_) => return Ok(out),
            Err(e) => {
                for p in &written {
                    let _ = std::fs::remove_file(p);
                }
                match e {
                    ArchiveError::NeedsPassword | ArchiveError::WrongPassword
                        if !progress.cancelled() =>
                    {
                        match ask_password(&path, password.is_some()) {
                            Some(p) => password = Some(p),
                            None => return Err("skipped (no password)".into()),
                        }
                    }
                    ArchiveError::NeedsPassword | ArchiveError::WrongPassword => {
                        return Err("Cancelled".into())
                    }
                    ArchiveError::Other(e) => return Err(e),
                }
            }
        }
    }
}

/// 検索カタログの相対パス（`/` 区切り）を、フォルダの中を指すパスにする。「..」や絶対パスで
/// 外を指すものは `None`。手で書き換えられた検索カタログで、出力先の外に書かないため。
pub fn enclosed(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split('/') {
        let mut c = Path::new(part).components();
        match (c.next(), c.next()) {
            (Some(Component::Normal(n)), None) => out.push(n),
            _ => return None,
        }
    }
    Some(out)
}

/// 検索カタログにあるのに見つからないファイル。ZIP / 7z の中はアーカイブがあるかだけを見る
/// （表示のたびに展開して確かめるのは重いため）。
pub fn missing(dir: &Path, c: &Catalog) -> Vec<String> {
    let mut checked: BTreeMap<&str, bool> = BTreeMap::new();
    let mut out = Vec::new();
    for f in &c.files {
        let on_disk = f
            .path
            .split_once(IN_ARCHIVE)
            .map_or(f.path.as_str(), |(a, _)| a);
        let exists = *checked
            .entry(on_disk)
            .or_insert_with(|| enclosed(on_disk).is_some_and(|p| dir.join(p).is_file()));
        if !exists {
            out.push(f.path.clone());
        }
    }
    out
}

/// `dir` を含む一番近いデータセット（`dir` 自身は除く）。
pub fn outer_dataset(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .skip(1)
        .find(|d| catalog::exists_in(d))
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{png_bytes, png_registry, write_png, write_zip, TempDir};

    fn no_password(_: &Path, _: bool) -> Option<String> {
        None
    }

    fn info(name: &str) -> Info {
        Info {
            name: name.into(),
            ..Info::default()
        }
    }

    fn register(dir: &Path, ask: &mut dyn FnMut(&Path, bool) -> Option<String>) -> Catalog {
        let r = catalog::scan(
            dir,
            info("orig"),
            None,
            &png_registry(),
            &AiTagStore::new(None),
            &LabelStore::new(None),
            &Progress::default(),
            ask,
        )
        .unwrap();
        catalog::save(dir, &r.catalog).unwrap();
        r.catalog
    }

    fn run(
        source: &Path,
        c: &Catalog,
        dest: &Path,
        ask: &mut dyn FnMut(&Path, bool) -> Option<String>,
    ) -> Result<Extracted, String> {
        extract(source, c, dest, None, &Progress::default(), ask)
    }

    #[test]
    fn copies_files_keeping_relative_paths_without_a_catalog() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("a/b")).unwrap();
        write_png(&src.join("top.png"), 2, 2, &[0; 12]);
        write_png(&src.join("a/b/deep.png"), 2, 2, &[0; 12]);
        let c = register(&src, &mut no_password);
        let dest = tmp.path().join("out");

        let x = run(&src, &c, &dest, &mut no_password).unwrap();

        assert!(dest.join("top.png").is_file());
        assert!(dest.join("a/b/deep.png").is_file());
        assert_eq!(x.copied, 2);
        assert!(x.failed.is_empty());
        assert!(!catalog::exists_in(&dest));
    }

    #[test]
    fn extracting_only_some_files_copies_just_those_including_inside_archives() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_png(&src.join("hit.png"), 2, 2, &[0; 12]);
        write_png(&src.join("miss.png"), 2, 2, &[0; 12]);
        let png = png_bytes(2, 2);
        write_zip(
            &src.join("z.zip"),
            &[("in.png", &png), ("out.png", &png)],
            None,
        );
        let c = register(&src, &mut no_password);
        let dest = tmp.path().join("out");

        let hits = ["hit.png".to_owned(), "z.zip!/in.png".to_owned()];
        let x = run(&src, &only(&c, &hits), &dest, &mut no_password).unwrap();

        assert_eq!(x.copied, 2);
        assert!(dest.join("hit.png").is_file());
        assert!(dest.join("z.zip/in.png").is_file());
        assert!(!dest.join("miss.png").exists());
        assert!(!dest.join("z.zip/out.png").exists());
    }

    #[test]
    fn registering_the_destination_writes_a_catalog_that_names_the_source() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_png(&src.join("a.png"), 2, 2, &[0; 12]);
        write_zip(&src.join("z.zip"), &[("b.png", &png_bytes(2, 2))], None);
        let c = register(&src, &mut no_password);
        let dest = tmp.path().join("out");
        let registry = png_registry();

        let x = extract(
            &src,
            &c,
            &dest,
            Some((info("copy"), &registry, &AiTagStore::new(None))),
            &Progress::default(),
            &mut no_password,
        )
        .unwrap();

        assert_eq!(x.registered.unwrap().catalog.files.len(), 2);
        let saved = catalog::load(&dest).unwrap();
        assert_eq!(saved.name, "copy");
        let paths: Vec<&str> = saved.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.png", "z.zip/b.png"]);
        let source = saved.source.unwrap();
        assert_eq!(source.name, "orig");
        assert_eq!(source.path, src.display().to_string());
    }

    #[test]
    fn registering_into_an_existing_dataset_folder_is_refused() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_png(&src.join("a.png"), 2, 2, &[0; 12]);
        let c = register(&src, &mut no_password);
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        catalog::save(&dest, &Catalog::default()).unwrap();
        let registry = png_registry();

        let r = extract(
            &src,
            &c,
            &dest,
            Some((info("copy"), &registry, &AiTagStore::new(None))),
            &Progress::default(),
            &mut no_password,
        );

        assert!(r.is_err());
        assert!(!dest.join("a.png").exists());
    }

    #[test]
    fn files_already_in_the_destination_are_not_overwritten() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_png(&src.join("a.png"), 2, 2, &[0; 12]);
        write_png(&src.join("b.png"), 2, 2, &[0; 12]);
        write_zip(&src.join("z.zip"), &[("c.png", &png_bytes(2, 2))], None);
        let c = register(&src, &mut no_password);
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(dest.join("z.zip")).unwrap();
        std::fs::write(dest.join("a.png"), b"mine").unwrap();
        std::fs::write(dest.join("z.zip/c.png"), b"mine").unwrap();

        let x = run(&src, &c, &dest, &mut no_password).unwrap();

        assert_eq!(x.copied, 1);
        let failed: Vec<&str> = x.failed.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(failed, ["a.png", "z.zip!/c.png"]);
        assert_eq!(std::fs::read(dest.join("a.png")).unwrap(), b"mine");
        assert_eq!(std::fs::read(dest.join("z.zip/c.png")).unwrap(), b"mine");
    }

    #[test]
    fn archive_members_go_under_a_folder_named_like_the_archive() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        let png = png_bytes(2, 2);
        write_zip(
            &src.join("sub/data.zip"),
            &[("s1/a.png", &png), ("b.png", &png)],
            None,
        );
        let c = register(&src, &mut no_password);
        let dest = tmp.path().join("out");

        run(&src, &c, &dest, &mut no_password).unwrap();

        assert!(dest.join("sub/data.zip/s1/a.png").is_file());
        assert!(dest.join("sub/data.zip/b.png").is_file());
    }

    #[test]
    fn encrypted_archives_ask_for_the_password_again_when_extracting() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_zip(
            &src.join("s.zip"),
            &[("a.png", &png_bytes(2, 2))],
            Some("pw"),
        );
        let c = register(&src, &mut |_, _| Some("pw".into()));
        let dest = tmp.path().join("out");
        let mut asked = 0;

        let x = run(&src, &c, &dest, &mut |_, _| {
            asked += 1;
            Some("pw".into())
        })
        .unwrap();

        assert_eq!(asked, 1);
        assert_eq!(x.copied, 1);
        assert!(dest.join("s.zip/a.png").is_file());
    }

    #[test]
    fn skipping_the_password_reports_the_members_as_not_copied() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_zip(
            &src.join("s.zip"),
            &[("a.png", &png_bytes(2, 2))],
            Some("pw"),
        );
        let c = register(&src, &mut |_, _| Some("pw".into()));
        let dest = tmp.path().join("out");

        let x = run(&src, &c, &dest, &mut no_password).unwrap();

        assert_eq!(x.copied, 0);
        assert_eq!(x.failed.len(), 1);
        assert_eq!(x.failed[0].path, "s.zip!/a.png");
        assert!(!dest.join("s.zip/a.png").exists());
    }

    #[test]
    fn files_missing_from_the_source_are_reported_and_the_rest_copied() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_png(&src.join("a.png"), 2, 2, &[0; 12]);
        write_png(&src.join("b.png"), 2, 2, &[0; 12]);
        let c = register(&src, &mut no_password);
        std::fs::remove_file(src.join("b.png")).unwrap();

        let x = run(&src, &c, &tmp.path().join("out"), &mut no_password).unwrap();

        assert_eq!(x.copied, 1);
        assert_eq!(x.failed.len(), 1);
        assert_eq!(x.failed[0].path, "b.png");
    }

    #[test]
    fn destination_must_be_a_folder_outside_the_source() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("x"), b"").unwrap();
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();

        assert!(check_destination(&src, &tmp.path().join("new")).is_ok());
        assert!(check_destination(&src, &empty).is_ok());
        assert!(check_destination(&src, &src.join("inner")).is_err());
        assert!(check_destination(&src, &src).is_err());
        assert!(check_destination(tmp.path().join("other").as_path(), &src).is_ok());
        assert!(check_destination(&empty, &src.join("x")).is_err());
        assert!(check_destination(&src, Path::new("relative")).is_err());
    }

    #[test]
    fn catalog_paths_that_leave_the_folder_are_refused() {
        assert!(enclosed("a/b.png").is_some());
        assert!(enclosed("../b.png").is_none());
        assert!(enclosed("a/../../b.png").is_none());
        assert!(enclosed("/etc/passwd").is_none());
        assert!(enclosed("").is_none());
    }

    #[test]
    fn missing_lists_catalog_files_that_are_gone() {
        let tmp = TempDir::new();
        let dir = tmp.path();
        write_png(&dir.join("a.png"), 2, 2, &[0; 12]);
        write_png(&dir.join("b.png"), 2, 2, &[0; 12]);
        write_zip(&dir.join("z.zip"), &[("c.png", &png_bytes(2, 2))], None);
        let c = register(dir, &mut no_password);
        std::fs::remove_file(dir.join("b.png")).unwrap();
        std::fs::remove_file(dir.join("z.zip")).unwrap();

        assert_eq!(missing(dir, &c), ["b.png", "z.zip!/c.png"]);
    }

    #[test]
    fn outer_dataset_is_the_nearest_enclosing_catalog() {
        let tmp = TempDir::new();
        let outer = tmp.path().join("outer");
        let inner = outer.join("x/inner");
        std::fs::create_dir_all(&inner).unwrap();
        catalog::save(&outer, &Catalog::default()).unwrap();
        catalog::save(&inner, &Catalog::default()).unwrap();

        assert_eq!(outer_dataset(&inner.join("new")), Some(inner.clone()));
        assert_eq!(outer_dataset(&inner), Some(outer.clone()));
        assert_eq!(outer_dataset(&outer), None);
    }
}
