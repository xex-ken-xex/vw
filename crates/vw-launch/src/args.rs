//! 送り先のアプリに渡す引数を、設定の書き方から組み立てる。
//!
//! 書き方は `--open {path}` のように、空白で区切った並びに差し込み文字を混ぜる。
//! 差し込み文字は `{path}`（1 件。1 件ずつ起動する）、`{paths}`（全件を並べて 1 回だけ起動する）、
//! `{dir}`（入っているフォルダ）、`{name}`（ファイル名）。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// 引数の書き方の誤り。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgsError {
    /// `"` が閉じていない。
    UnclosedQuote,
}

impl std::fmt::Display for ArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgsError::UnclosedQuote => f.write_str("a \" is not closed"),
        }
    }
}

const PATH: &str = "{path}";
const PATHS: &str = "{paths}";
const DIR: &str = "{dir}";
const NAME: &str = "{name}";

/// 空白で区切る。`"` で囲んだところは空白を含めて 1 つにする。
///
/// `\` を逃がしの印に使わないのは、Windows のパス（`C:\Program Files\...`）を
/// そのまま書けるようにするため。シェルは通さないので、`&` や `^` もただの文字になる。
pub fn split(text: &str) -> Result<Vec<String>, ArgsError> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return Err(ArgsError::UnclosedQuote);
    }
    if started {
        out.push(current);
    }
    Ok(out)
}

/// [`split`] の逆。空白や `"` を含むもの、空のものは `"` で囲む。
pub fn join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty() || a.chars().any(char::is_whitespace) {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 書き方 `template` で `paths` を送るときの、起動 1 回ごとの引数。
pub fn launches(template: &[String], paths: &[PathBuf]) -> Vec<Vec<OsString>> {
    if paths.is_empty() {
        return Vec::new();
    }
    let mut template = template.to_vec();
    let has = |template: &[String], p: &str| template.iter().any(|t| t.contains(p));
    if ![PATH, PATHS, DIR, NAME].iter().any(|p| has(&template, p)) {
        template.push(PATHS.to_owned());
    }
    if has(&template, PATHS) {
        // `{dir}` と `{name}` は 1 件目のもの。全件を並べるときに 1 件ごとの値は決められない。
        let first = &paths[0];
        let args = template
            .iter()
            .flat_map(|token| {
                if token.contains(PATHS) {
                    paths
                        .iter()
                        .map(|p| fill(token, first, Some(p)))
                        .collect::<Vec<_>>()
                } else {
                    vec![fill(token, first, None)]
                }
            })
            .collect();
        vec![args]
    } else {
        paths
            .iter()
            .map(|p| template.iter().map(|t| fill(t, p, None)).collect())
            .collect()
    }
}

/// 起動する回数。確かめを出すかどうかを、引数を作らずに決めるため。
pub fn launch_count(template: &[String], paths: usize) -> usize {
    let one_by_one = [PATH, DIR, NAME]
        .iter()
        .any(|p| template.iter().any(|t| t.contains(p)))
        && !template.iter().any(|t| t.contains(PATHS));
    if one_by_one { paths } else { paths.min(1) }
}

/// `token` の差し込み文字を埋める。`each` は `{paths}` に入れる 1 件。
fn fill(token: &str, path: &Path, each: Option<&Path>) -> OsString {
    let dir = path.parent().unwrap_or(Path::new(""));
    let name = path.file_name().unwrap_or(path.as_os_str());
    let values: [(&str, &OsStr); 4] = [
        (PATHS, each.unwrap_or(path).as_os_str()),
        (PATH, path.as_os_str()),
        (DIR, dir.as_os_str()),
        (NAME, name),
    ];
    let mut out = OsString::new();
    let mut rest = token;
    'scan: while !rest.is_empty() {
        for (key, value) in values {
            if let Some(after) = rest.strip_prefix(key) {
                out.push(value);
                rest = after;
                continue 'scan;
            }
        }
        let next = rest
            .char_indices()
            .skip(1)
            .find(|&(_, c)| c == '{')
            .map_or(rest.len(), |(i, _)| i);
        out.push(&rest[..next]);
        rest = &rest[next..];
    }
    out
}

/// Windows の関連付けのコマンド（`"C:\app.exe" "%1"`）の、プログラムより後ろを
/// この書き方に直す。`%1` `%L` `%V` は `{path}`、ほかの `%` の印は捨てる。
pub fn from_windows_command(command: &str) -> String {
    let Ok(tokens) = split(command) else {
        return PATH.to_owned();
    };
    let args: Vec<String> = tokens
        .into_iter()
        .skip(1)
        .filter_map(|t| {
            replace_codes(&t, |c| match c {
                '1' | 'L' | 'l' | 'V' | 'v' => Some(PATH),
                _ => None,
            })
        })
        .collect();
    join(&args)
}

/// Linux の .desktop の `Exec` を、プログラムと、この書き方の引数に分ける。
/// `%f` `%u` は `{path}`、`%F` `%U` は `{paths}`、ほかの印（`%i` `%c` `%k` など）は捨てる。
pub fn from_desktop_exec(exec: &str) -> Option<(String, String)> {
    let mut tokens = split(exec).ok()?.into_iter();
    let program = tokens.next()?;
    let args: Vec<String> = tokens
        .filter_map(|t| {
            replace_codes(&t, |c| match c {
                'f' | 'u' => Some(PATH),
                'F' | 'U' => Some(PATHS),
                _ => None,
            })
        })
        .collect();
    Some((program, join(&args)))
}

/// `%x` の印を `code` で置き換える。消えて空になった引数は `None`。
fn replace_codes(token: &str, code: impl Fn(char) -> Option<&'static str>) -> Option<String> {
    let mut out = String::new();
    let mut chars = token.chars();
    let mut had_code = false;
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some(c) => {
                had_code = true;
                if let Some(text) = code(c) {
                    out.push_str(text);
                }
            }
            None => out.push('%'),
        }
    }
    (!(had_code && out.is_empty())).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(text: &str) -> Vec<String> {
        split(text).unwrap()
    }

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    fn strings(launches: Vec<Vec<OsString>>) -> Vec<Vec<String>> {
        launches
            .into_iter()
            .map(|a| a.into_iter().map(|s| s.into_string().unwrap()).collect())
            .collect()
    }

    #[test]
    fn splits_on_spaces_and_keeps_quoted_spaces() {
        assert_eq!(
            template(r#"--open "C:\My Pictures\a b.jpg"  -x"#),
            ["--open", r"C:\My Pictures\a b.jpg", "-x"]
        );
    }

    #[test]
    fn keeps_shell_characters_as_text() {
        assert_eq!(template("a&b ^c |d"), ["a&b", "^c", "|d"]);
    }

    #[test]
    fn quoted_empty_text_is_an_empty_argument() {
        assert_eq!(template(r#"a "" b"#), ["a", "", "b"]);
    }

    #[test]
    fn rejects_an_unclosed_quote() {
        assert_eq!(split(r#"a "b"#), Err(ArgsError::UnclosedQuote));
    }

    #[test]
    fn join_quotes_what_split_would_break() {
        let args = vec!["a b".to_owned(), "".to_owned(), "c".to_owned()];
        assert_eq!(split(&join(&args)).unwrap(), args);
    }

    #[test]
    fn path_launches_once_per_item() {
        let got = launches(
            &template("--open {path}"),
            &paths(&["/a/1.jpg", "/b/2.jpg"]),
        );
        assert_eq!(
            strings(got),
            [vec!["--open", "/a/1.jpg"], vec!["--open", "/b/2.jpg"]]
        );
    }

    #[test]
    fn paths_launches_once_with_every_item() {
        let got = launches(&template("-n {paths}"), &paths(&["/a/1.jpg", "/b/2.jpg"]));
        assert_eq!(strings(got), [vec!["-n", "/a/1.jpg", "/b/2.jpg"]]);
    }

    #[test]
    fn paths_inside_a_longer_argument_repeats_that_argument() {
        let got = launches(&template("--file={paths}"), &paths(&["/1", "/2"]));
        assert_eq!(strings(got), [vec!["--file=/1", "--file=/2"]]);
    }

    #[test]
    fn no_placeholder_appends_every_path() {
        let got = launches(&template("--new-window"), &paths(&["/1", "/2"]));
        assert_eq!(strings(got), [vec!["--new-window", "/1", "/2"]]);
    }

    #[test]
    fn empty_template_passes_the_paths() {
        let got = launches(&[], &paths(&["/1"]));
        assert_eq!(strings(got), [vec!["/1"]]);
    }

    #[test]
    fn dir_and_name_come_from_each_path() {
        let got = launches(&template("{dir} {name}"), &paths(&["/a/1.jpg", "/b/2.jpg"]));
        assert_eq!(strings(got), [vec!["/a", "1.jpg"], vec!["/b", "2.jpg"]]);
    }

    #[test]
    fn dir_with_paths_uses_the_first_path() {
        let got = launches(&template("-C {dir} {paths}"), &paths(&["/a/1", "/b/2"]));
        assert_eq!(strings(got), [vec!["-C", "/a", "/a/1", "/b/2"]]);
    }

    #[test]
    fn unknown_braces_are_left_as_they_are() {
        let got = launches(&template("{x}{path}{"), &paths(&["/1"]));
        assert_eq!(strings(got), [vec!["{x}/1{"]]);
    }

    #[test]
    fn non_ascii_text_around_placeholders_is_kept() {
        let got = launches(&template("写真={path}"), &paths(&["/画像/1.jpg"]));
        assert_eq!(strings(got), [vec!["写真=/画像/1.jpg"]]);
    }

    #[test]
    fn nothing_to_send_launches_nothing() {
        assert!(launches(&template("{path}"), &[]).is_empty());
    }

    #[test]
    fn launch_count_matches_launches() {
        for text in ["{path}", "{paths}", "", "{dir}", "-x {name} {paths}"] {
            for n in 0..3 {
                let t = template(text);
                let p: Vec<PathBuf> = (0..n).map(|i| PathBuf::from(format!("/{i}"))).collect();
                assert_eq!(launch_count(&t, n), launches(&t, &p).len(), "{text} {n}");
            }
        }
    }

    #[test]
    fn windows_command_maps_percent_one_to_path() {
        assert_eq!(
            from_windows_command(r#""C:\Program Files\X\x.exe" /open "%1""#),
            "/open {path}"
        );
    }

    #[test]
    fn windows_command_drops_other_codes() {
        assert_eq!(from_windows_command(r#""C:\x.exe" "%1" %*"#), "{path}");
    }

    #[test]
    fn desktop_exec_maps_field_codes() {
        assert_eq!(
            from_desktop_exec("gimp-2.10 %U"),
            Some(("gimp-2.10".to_owned(), "{paths}".to_owned()))
        );
        assert_eq!(
            from_desktop_exec("eog --new-instance %f %i"),
            Some(("eog".to_owned(), "--new-instance {path}".to_owned()))
        );
    }

    #[test]
    fn desktop_exec_keeps_a_double_percent() {
        assert_eq!(
            from_desktop_exec("app --rate=50%% %F"),
            Some(("app".to_owned(), "--rate=50% {paths}".to_owned()))
        );
    }
}
