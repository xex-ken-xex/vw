//! 拡張子に OS が関連付けているアプリを調べる。
//!
//! 送り先の一覧の最初の中身にする。Windows はレジストリの関連付けを `AssocQueryStringW` で、
//! Linux は `xdg-mime` と .desktop ファイルで調べる。

use std::path::PathBuf;

/// 関連付けられているアプリ 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociatedApp {
    /// 表示名（「フォト ビューアー」など）。
    pub name: String,
    pub program: PathBuf,
    /// [`crate::args`] の書き方の引数。
    pub args: String,
    /// このアプリが既定になっている拡張子（小文字、`.` なし）。
    pub extensions: Vec<String>,
}

/// `extensions` のそれぞれに関連付けられた既定のアプリ。同じプログラムは 1 つにまとめる。
/// 時間がかかることがある（Linux では拡張子ごとに `xdg-mime` を起動する）ので、裏のスレッドで呼ぶ。
pub fn associated_apps(extensions: &[&str]) -> Vec<AssociatedApp> {
    let mut out: Vec<AssociatedApp> = Vec::new();
    for ext in extensions {
        let Some(app) = default_app(ext) else {
            continue;
        };
        match out.iter_mut().find(|a| a.program == app.program) {
            Some(known) => known.extensions.push((*ext).to_owned()),
            None => out.push(AssociatedApp {
                extensions: vec![(*ext).to_owned()],
                ..app
            }),
        }
    }
    out
}

#[cfg(windows)]
fn default_app(ext: &str) -> Option<AssociatedApp> {
    use windows_sys::Win32::UI::Shell::{
        ASSOCSTR_COMMAND, ASSOCSTR_EXECUTABLE, ASSOCSTR_FRIENDLYAPPNAME,
    };
    let program = PathBuf::from(query(ext, ASSOCSTR_EXECUTABLE)?);
    // ストアアプリ（「フォト」など）はプログラムのファイルが無く、引数を付けて起動できない。
    // それは「既定のアプリで開く」で開けるので、一覧には入れない。
    if !program.is_file() {
        return None;
    }
    let name = query(ext, ASSOCSTR_FRIENDLYAPPNAME)
        .filter(|n| !n.trim().is_empty())
        .or_else(|| Some(program.file_stem()?.to_string_lossy().into_owned()))?;
    let args = query(ext, ASSOCSTR_COMMAND)
        .map(|c| crate::args::from_windows_command(&c))
        .unwrap_or_else(|| "{path}".to_owned());
    Some(AssociatedApp {
        name,
        program,
        args,
        extensions: Vec::new(),
    })
}

#[cfg(windows)]
fn query(ext: &str, what: windows_sys::Win32::UI::Shell::ASSOCSTR) -> Option<String> {
    use windows_sys::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_INIT_IGNOREUNKNOWN};
    let assoc: Vec<u16> = format!(".{ext}").encode_utf16().chain([0]).collect();
    let mut buf = vec![0u16; 2048];
    let mut len = buf.len() as u32;
    // IGNOREUNKNOWN が無いと、関連付けの無い拡張子でも「アプリを選ぶ」の項目が返ってくる。
    // SAFETY: assoc は NUL で終わり、buf とその長さ len を書き込み先として渡している。
    let hr = unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            what,
            assoc.as_ptr(),
            std::ptr::null(),
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    if hr != 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
    (!text.is_empty()).then_some(text)
}

#[cfg(not(windows))]
fn default_app(ext: &str) -> Option<AssociatedApp> {
    let mime = mime_type(ext)?;
    let output = std::process::Command::new("xdg-mime")
        .args(["query", "default", mime])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let id = String::from_utf8(output.stdout).ok()?;
    let entry = desktop_entry(id.trim())?;
    let (program, args) = crate::args::from_desktop_exec(&entry.exec)?;
    Some(AssociatedApp {
        name: entry.name,
        program: PathBuf::from(program),
        args,
        extensions: Vec::new(),
    })
}

/// 拡張子の MIME の型。shared-mime-info の名前に合わせる。
#[cfg(not(windows))]
fn mime_type(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "nef" => "image/x-nikon-nef",
        "nrw" => "image/x-nikon-nrw",
        "arw" => "image/x-sony-arw",
        "srf" => "image/x-sony-srf",
        "sr2" => "image/x-sony-sr2",
        "raf" => "image/x-fuji-raf",
        "cr2" => "image/x-canon-cr2",
        "cr3" => "image/x-canon-cr3",
        "dng" => "image/x-adobe-dng",
        "orf" => "image/x-olympus-orf",
        "rw2" => "image/x-panasonic-rw2",
        "pef" => "image/x-pentax-pef",
        _ => return None,
    })
}

#[cfg(not(windows))]
struct DesktopEntry {
    name: String,
    exec: String,
}

/// `id`（`org.gnome.eog.desktop` など）の .desktop ファイルを XDG のデータの場所から探して読む。
#[cfg(not(windows))]
fn desktop_entry(id: &str) -> Option<DesktopEntry> {
    if id.is_empty() || id.contains('/') {
        return None;
    }
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    home.into_iter()
        .chain(dirs.split(':').map(PathBuf::from))
        .map(|d| d.join("applications").join(id))
        .find_map(|p| parse_desktop(&std::fs::read_to_string(p).ok()?))
}

/// .desktop の `[Desktop Entry]` 節の `Name` と `Exec`。
#[cfg(not(windows))]
fn parse_desktop(text: &str) -> Option<DesktopEntry> {
    let mut in_entry = false;
    let mut name = None;
    let mut exec = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name.get_or_insert_with(|| v.to_owned());
        } else if let Some(v) = line.strip_prefix("Exec=") {
            exec.get_or_insert_with(|| v.to_owned());
        }
    }
    Some(DesktopEntry {
        name: name?,
        exec: exec?,
    })
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn reads_name_and_exec_of_the_desktop_entry_section() {
        let text = "[Desktop Entry]\nName=Image Viewer\nName[ja]=画像ビューアー\nExec=eog %U\n\
                    [Desktop Action new]\nName=New\nExec=eog --new\n";
        let entry = parse_desktop(text).unwrap();
        assert_eq!(entry.name, "Image Viewer");
        assert_eq!(entry.exec, "eog %U");
    }

    #[test]
    fn an_entry_without_exec_is_not_an_app() {
        assert!(parse_desktop("[Desktop Entry]\nName=Link\n").is_none());
    }

    #[test]
    fn raw_extensions_have_mime_types() {
        for ext in ["NEF", "arw", "raf", "jpg"] {
            assert!(mime_type(ext).is_some(), "{ext}");
        }
    }
}
