//! 設定「再起動するまでファイル読み出しエラーの詳細をログに出力する」。
//!
//! オンのとき、メタデータで読めなかったタグ（[`vw_core::Metadata::problems`]）をログに書く。
//! 設定ファイルには保存せず、起動するたびにオフに戻る。問い合わせのときだけ詳しく見るためのもので、
//! 付けたままにすると壊れたファイルの多いフォルダでログが流れ続けるため。
//!
//! オンかどうかを [`crate::settings::Settings`] に持たせないのは、設定ファイルを読み直す
//! （外で書き換えた、並びを変えた）たびに Settings が作り直され、オンにしたのが消えてしまうため。

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::log::{DetailWriter, Level};

static ON: AtomicBool = AtomicBool::new(false);

/// 検索カタログの登録で、読めないタグがあったファイルの数。登録が終わったときに要約の 1 行にする。
///
/// 登録ごとに数を分けないのは、読むのは登録のスレッドの奥（[`crate::catalog`]）で、そこまで
/// 数える口を通すと、ほとんどの人が使わない設定のために登録の作りを大きく変えることになるため。
/// 同時に 2 つ登録すると、先に終わった方の要約に両方の数が入る。
static CATALOG_FILES: AtomicUsize = AtomicUsize::new(0);

/// 登録のスレッドからログファイルに書く口。
static WRITER: Mutex<Option<DetailWriter>> = Mutex::new(None);

pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// ログファイルに書いているなら、その口を覚える。起動のときに 1 度だけ呼ぶ。
pub fn set_writer(writer: Option<DetailWriter>) {
    if let Ok(mut w) = WRITER.lock() {
        *w = writer;
    }
}

/// `path` のメタデータで読めなかった所を、1 か所 1 行の英語の文にする。
pub fn lines(path: &Path, metadata: &vw_core::Metadata) -> Vec<String> {
    metadata
        .problems
        .iter()
        .map(|p| format!("Metadata problem in {}: {p}", path.display()))
        .collect()
}

/// 検索カタログの登録で読んだファイルの、読めなかった所をログファイルにだけ書く。
/// 何千ファイルも読むので、ログ欄には登録の終わりに [`take_catalog_files`] の数だけを出す。
pub fn record_catalog(path: &Path, metadata: &vw_core::Metadata) {
    if !enabled() || metadata.problems.is_empty() {
        return;
    }
    CATALOG_FILES.fetch_add(1, Ordering::Relaxed);
    let Ok(writer) = WRITER.lock() else {
        return;
    };
    if let Some(w) = writer.as_ref() {
        for line in lines(path, metadata) {
            w.detail(Level::Warn, line);
        }
    }
}

/// [`record_catalog`] で数えたファイルの数を返し、0 に戻す。
pub fn take_catalog_files() -> usize {
    CATALOG_FILES.swap(0, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_problem_is_one_line_naming_the_file() {
        let mut m = vw_core::Metadata::default();
        m.problem("EXIF", "IFD0 (0x010F) Make", "unknown value type 99");
        m.problem("XMP", "", "not UTF-8 at byte 3 of 9");
        assert_eq!(
            lines(Path::new("a/b.jpg"), &m),
            [
                "Metadata problem in a/b.jpg: EXIF IFD0 (0x010F) Make: unknown value type 99",
                "Metadata problem in a/b.jpg: XMP: not UTF-8 at byte 3 of 9",
            ]
        );
    }
}
