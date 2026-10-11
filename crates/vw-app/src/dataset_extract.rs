//! データセットの取り出しと、表示するときの欠けの確認。
//!
//! 取り出しは、検索カタログにあるファイルを、データセットのフォルダからの相対パスのまま
//! 出力先へ写す。ZIP / 7z の中のファイルは展開し、アーカイブと同じ名前のフォルダの下に置く。
//! 写し終えたら出力先を新しいデータセットとして読み直す。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;

use vw_core::Registry;

use crate::archive::{self, ArchiveError, EntryError};
use crate::catalog::{self, Catalog, Info, Progress, Report, Skipped, Source, IN_ARCHIVE};

/// 取り出した結果。`failed` は写せなかったファイル（元から消えていたものなど）。
#[derive(Debug)]
pub struct Extracted {
    pub report: Report,
    pub copied: usize,
    pub failed: Vec<Skipped>,
}

/// 出力先として使えるか。空のフォルダか、まだないフォルダだけを受け付ける。
///
/// 中身のあるフォルダを許さないのは、出力先を読み直して新しい検索カタログを作るので、
/// もとからあったファイルまで取り出したものとして入ってしまうため。
pub fn check_destination(source: &Path, dest: &Path) -> Result<(), String> {
    if dest.as_os_str().is_empty() || !dest.is_absolute() {
        return Err("Enter the full path of the destination folder.".into());
    }
    // 元のデータセットの中に写すと、次に元を更新したとき写したファイルも数えてしまう。
    if dest.starts_with(source) {
        return Err("The destination is inside the dataset being extracted.".into());
    }
    match std::fs::read_dir(dest) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err("The destination folder is not empty.".into());
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if dest.exists() {
                Err("The destination is not a folder.".into())
            } else {
                Ok(())
            }
        }
        Err(e) => Err(format!("{}: {e}", dest.display())),
    }
}

/// `source` のデータセット（検索カタログは `c`）を `dest` に取り出し、`dest` に新しい検索カタログを
/// 作る。名前などは `info`、元のデータセットは `source` に記録する。
pub fn extract(
    source: &Path,
    c: &Catalog,
    dest: &Path,
    info: Info,
    registry: &Registry,
    progress: &Progress,
    ask_password: &mut dyn FnMut(&Path, bool) -> Option<String>,
) -> Result<Extracted, String> {
    check_destination(source, dest)?;
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let (copied, failed) = copy_files(source, c, dest, progress, ask_password)?;
    progress.done.store(0, Ordering::Relaxed);
    let mut report = catalog::scan(dest, info, None, registry, progress, ask_password)?;
    report.catalog.source = Some(Source {
        name: c.name.clone(),
        path: source.display().to_string(),
    });
    catalog::save(dest, &report.catalog)?;
    Ok(Extracted {
        report,
        copied,
        failed,
    })
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
        let found =
            match result {
                Ok(found) => found,
                Err(reason) => {
                    failed.extend(wanted.iter().map(|inner| {
                        catalog::skip(&format!("{archive}{IN_ARCHIVE}{inner}"), &reason)
                    }));
                    progress.done.fetch_add(wanted.len(), Ordering::Relaxed);
                    continue;
                }
            };
        for inner in &wanted {
            if !found.contains(*inner) {
                failed.push(catalog::skip(
                    &format!("{archive}{IN_ARCHIVE}{inner}"),
                    "not found in the archive",
                ));
            }
        }
        copied += found.len();
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
    std::fs::copy(source.join(&relative), &target)
        .map(drop)
        .map_err(|e| e.to_string())
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
) -> Result<BTreeSet<String>, String> {
    let relative = enclosed(archive).ok_or("unsafe path in the search catalog")?;
    let path = source.join(&relative);
    let root = dest.join(&relative);
    let mut password: Option<String> = None;
    loop {
        let mut written: Vec<PathBuf> = Vec::new();
        let mut found = BTreeSet::new();
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
            let mut file =
                std::fs::File::create(&target).map_err(|e| EntryError::Other(e.to_string()))?;
            written.push(target);
            archive::copy_entry(data, &mut file)?;
            found.insert(name);
            Ok(())
        });
        match result {
            Ok(_) => return Ok(found),
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
fn enclosed(path: &str) -> Option<PathBuf> {
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
    use crate::test_util::{png_registry, write_png, TempDir};
    use std::io::Write as _;

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
            &Progress::default(),
            ask,
        )
        .unwrap();
        catalog::save(dir, &r.catalog).unwrap();
        r.catalog
    }

    fn png_bytes() -> Vec<u8> {
        let tmp = TempDir::new();
        let p = tmp.path().join("a.png");
        write_png(&p, 2, 2, &[0; 12]);
        std::fs::read(p).unwrap()
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])], password: Option<&str>) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in entries {
            let mut options = zip::write::SimpleFileOptions::default();
            if let Some(pw) = password {
                options = options.with_aes_encryption(zip::AesMode::Aes256, pw);
            }
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    fn run(
        source: &Path,
        c: &Catalog,
        dest: &Path,
        ask: &mut dyn FnMut(&Path, bool) -> Option<String>,
    ) -> Result<Extracted, String> {
        extract(
            source,
            c,
            dest,
            info("copy"),
            &png_registry(),
            &Progress::default(),
            ask,
        )
    }

    #[test]
    fn copies_files_keeping_relative_paths_and_writes_a_new_catalog() {
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
        let saved = catalog::load(&dest).unwrap();
        assert_eq!(saved.name, "copy");
        assert_eq!(saved.files.len(), 2);
        let source = saved.source.unwrap();
        assert_eq!(source.name, "orig");
        assert_eq!(source.path, src.display().to_string());
    }

    #[test]
    fn archive_members_go_under_a_folder_named_like_the_archive() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        let png = png_bytes();
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
        let saved = catalog::load(&dest).unwrap();
        let paths: Vec<&str> = saved.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["sub/data.zip/b.png", "sub/data.zip/s1/a.png"]);
    }

    #[test]
    fn encrypted_archives_ask_for_the_password_again_when_extracting() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_zip(&src.join("s.zip"), &[("a.png", &png_bytes())], Some("pw"));
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
        write_zip(&src.join("s.zip"), &[("a.png", &png_bytes())], Some("pw"));
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
    fn destination_must_be_empty_and_outside_the_source() {
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
        assert!(check_destination(tmp.path().join("other").as_path(), &src).is_err());
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
        write_zip(&dir.join("z.zip"), &[("c.png", &png_bytes())], None);
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
