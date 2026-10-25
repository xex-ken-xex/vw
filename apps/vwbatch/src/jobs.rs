//! 入力に並べたファイルとフォルダから、書き出す 1 つずつを作る。

use std::path::{Path, PathBuf};

use vw_core::Registry;

/// 1 つの入力ファイルと、出力先のフォルダからの場所（拡張子なし）。
#[derive(Debug, PartialEq)]
pub struct Job {
    pub source: PathBuf,
    pub relative: PathBuf,
}

impl Job {
    /// 出力のファイル。フレームの番号があれば、名前の後ろに 1 始まりの 4 桁で付ける。
    pub fn output(&self, dir: &Path, frame: Option<usize>, extension: &str) -> PathBuf {
        let mut name = self.relative.as_os_str().to_owned();
        if let Some(f) = frame {
            name.push(format!("-{:04}", f + 1));
        }
        name.push(".");
        name.push(extension);
        dir.join(name)
    }
}

pub struct Jobs {
    pub files: Vec<Job>,
    /// 読めなかった入力（ないパス、画像でないファイル）。
    pub errors: Vec<String>,
}

/// Windows のごみ箱。ビューアのツリーとカタログと同じく、中の画像は扱わない。
const RECYCLE_BINS: [&str; 3] = ["$RECYCLE.BIN", "RECYCLER", "RECYCLED"];

/// フォルダは中まで探し、そのフォルダからの並びを保つ。出力先が入力のフォルダの中にあっても、
/// 書き出したものを読み直さないよう、出力先の中は探さない。
pub fn collect(registry: &Registry, inputs: &[PathBuf], output: &Path) -> Jobs {
    let output = std::fs::canonicalize(output).unwrap_or_else(|_| output.to_path_buf());
    let mut jobs = Jobs {
        files: Vec::new(),
        errors: Vec::new(),
    };
    for input in inputs {
        if input.is_file() {
            match registry.supports_file(input) {
                true => jobs.files.push(Job {
                    source: input.clone(),
                    relative: stem(Path::new(input.file_name().unwrap_or_default())),
                }),
                false => jobs
                    .errors
                    .push(format!("{}: not an image vw can read", input.display())),
            }
            continue;
        }
        if !input.is_dir() {
            jobs.errors
                .push(format!("{}: no such file or folder", input.display()));
            continue;
        }
        let walk = walkdir::WalkDir::new(input)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                let inside_output = e.file_type().is_dir()
                    && std::fs::canonicalize(e.path()).is_ok_and(|p| p == output);
                !RECYCLE_BINS.iter().any(|b| name.eq_ignore_ascii_case(b)) && !inside_output
            });
        for entry in walk {
            match entry {
                Ok(e) if e.file_type().is_file() && registry.supports_file(e.path()) => {
                    let relative = e.path().strip_prefix(input).unwrap_or(e.path());
                    jobs.files.push(Job {
                        source: e.path().to_path_buf(),
                        relative: stem(relative),
                    });
                }
                Ok(_) => {}
                Err(e) => jobs.errors.push(e.to_string()),
            }
        }
    }
    jobs
}

/// 最後の拡張子だけを除く（`a.fits.gz` は `a.fits`）。拡張子のない名前（`IM0001`）はそのまま。
fn stem(path: &Path) -> PathBuf {
    match path.file_stem() {
        Some(s) => path.with_file_name(s),
        None => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("vwbatch-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }

        fn file(&self, rel: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn registry() -> Registry {
        let mut r = Registry::new();
        r.register(Arc::new(vw_png::PngPlugin));
        r
    }

    #[test]
    fn folders_keep_their_subfolders_and_skip_the_output_and_recycle_bins() {
        let d = Dir::new("collect");
        d.file("in/a.png");
        d.file("in/sub/b.PNG");
        d.file("in/notes.txt");
        d.file("in/$RECYCLE.BIN/c.png");
        d.file("in/out/old.png");
        let input = d.0.join("in");
        let out = input.join("out");
        let jobs = collect(&registry(), std::slice::from_ref(&input), &out);
        let relative: Vec<_> = jobs.files.iter().map(|j| j.relative.clone()).collect();
        assert_eq!(relative, [PathBuf::from("a"), Path::new("sub").join("b")]);
        assert!(jobs.errors.is_empty());
    }

    #[test]
    fn a_file_given_by_name_goes_at_the_top_and_frames_get_numbers() {
        let d = Dir::new("single");
        d.file("x/scan.png");
        d.file("x/readme.txt");
        let inputs = [
            d.0.join("x/scan.png"),
            d.0.join("x/readme.txt"),
            d.0.join("missing"),
        ];
        let jobs = collect(&registry(), &inputs, &d.0.join("out"));
        assert_eq!(jobs.files.len(), 1);
        assert_eq!(jobs.errors.len(), 2);
        let out = Path::new("o");
        assert_eq!(jobs.files[0].output(out, None, "png"), out.join("scan.png"));
        assert_eq!(
            jobs.files[0].output(out, Some(1), "tif"),
            out.join("scan-0002.tif")
        );
    }
}
