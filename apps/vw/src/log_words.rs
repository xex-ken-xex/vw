//! ログ欄の、人が読む行（開いた・読み込んだ・フラグを付けた・できなかった理由）の訳。
//!
//! ログのファイルは英語のまま残し（問い合わせのときに英語で検索できるように）、画面にだけ訳を出す。
//! 行は書く側で `format!` された後の文なので、英語の型（`{}` の所に何が入ってもよい）と突き合わせて
//! 中身を取り出し、訳の型に入れ直す。
//!
//! 書く側で型と中身を分けて渡す口（`log.info_tr("Opened folder {}", &[path])` のような）にしないのは、
//! 200 近い書く所を書き換えずに、訳したい行だけを表に足せば済むようにするため。
//! エラーの中身（OS やデコーダの言葉）は訳さず、そのまま入れる。

use crate::settings::Language;

/// 英語の型と日本語の型。英語の `{}` は前から順に 0, 1, … と数え、日本語では `{}`（順に）か
/// `{0}`・`{1}`（番号で、順を入れ替えるとき）で使う。
const JA: &[(&str, &str)] = &[
    ("Opened folder {}", "フォルダ {} を開きました"),
    ("Closed {}", "{} を閉じました"),
    (
        "Loaded {} images from {}",
        "{1} から {0} 枚の画像を読みました",
    ),
    ("No images in {}", "{} に画像はありません"),
    ("No folder is open", "フォルダを開いていません"),
    (
        "Auto reload: {} new / {} removed image(s) in {}",
        "自動で読み直し: {2} に新しい画像 {0} 枚、なくなった画像 {1} 枚",
    ),
    (
        "Cleared the flags of {} images in {} (Ctrl+Z to undo)",
        "{1} の {0} 枚のフラグを外しました（Ctrl+Z で戻せます）",
    ),
    (
        "No flagged images to clear in {}",
        "{} にフラグの付いた画像はありません",
    ),
    (
        "Moved {} bad images to the trash (Ctrl+Z to undo)",
        "Bad の画像 {} 枚をごみ箱へ送りました（Ctrl+Z で戻せます）",
    ),
    ("No bad images in {}", "{} に Bad の画像はありません"),
    (
        "Did not move the bad images in {} to the trash",
        "{} の Bad の画像をごみ箱へ送りませんでした",
    ),
    ("Flags restored: {} images", "フラグを戻しました: {} 枚"),
    (
        "Flags and tags cannot be put on images inside {}",
        "{} の中の画像には、フラグとタグを付けられません",
    ),
    (
        "No tag for key {} (add tags in Settings > Flags and tags)",
        "キー {} のタグがありません（設定の「フラグとタグ」で足せます）",
    ),
    ("Nothing to undo", "元に戻すものはありません"),
    (
        "Still restoring images from the trash",
        "まだごみ箱から戻しています",
    ),
    (
        "Cannot restore {} from the trash: {}",
        "{} をごみ箱から戻せません: {}",
    ),
    ("Cannot move or copy: {}", "移動もコピーもできません: {}"),
    (
        "Not moved: drop {} on a folder in the tree or a folder slot",
        "動かしませんでした: {} は、ツリーのフォルダかフォルダのスロットの上で離してください",
    ),
    (
        "Not moved: {} is already in {}",
        "動かしませんでした: {0} はもう {1} にあります",
    ),
    ("Did not move {}", "{} を動かしませんでした"),
    ("Renamed {} to {}", "{0} の名前を {1} に変えました"),
    ("Deleted {}", "{} を消しました"),
    (
        "{}: files in a ZIP are read-only (Ctrl+drag copies them out)",
        "{}: ZIP の中のファイルは変えられません（Ctrl を押しながら引くと外へコピーします）",
    ),
    (
        "{}: could not develop the RAW; showing the embedded preview",
        "{}: RAW を現像できないので、中に入っているプレビューを出しています",
    ),
    (
        "{}: shown downsampled to {}x{} (texture size limit)",
        "{0}: 大きすぎるので {1}×{2} に縮めて出しています（テクスチャの大きさの上限）",
    ),
    (
        "{} is larger than the archive size limit",
        "{} は書庫の大きさの上限を超えています",
    ),
    (
        "The image has no latitude and longitude",
        "この画像には緯度・経度がありません",
    ),
    (
        "Slideshow: there is only one image to show",
        "スライドショー: 見せる画像が 1 枚しかありません",
    ),
    (
        "Slideshow: {} frames were skipped because images could not be decoded fast enough",
        "スライドショー: 読むのが間に合わず、{} コマを飛ばしました",
    ),
    (
        "Showing {} files found in the dataset {}",
        "データセット {1} で見つかった {0} 個のファイルを出しています",
    ),
    (
        "{} files found are inside ZIP / 7z files and are not shown. Extract them to see them.",
        "見つかったうち {} 個は ZIP・7z の中にあるので出していません。取り出すと見られます。",
    ),
    (
        "Stopped extracting. The files already copied remain in {}.",
        "取り出しを止めました。コピーし終えたファイルは {} に残っています。",
    ),
    (
        "Stopped reading {}. Register or update it again to continue from there.",
        "{} の読み込みを止めました。もう一度登録か更新をすると、続きから読みます。",
    ),
    ("Cancelled reading {}.", "{} の読み込みを取り消しました。"),
    (
        "No QR code found in {}",
        "{} に QR コードは見つかりませんでした",
    ),
    (
        "No image to read QR codes from",
        "QR コードを読む画像がありません",
    ),
    ("Help: opened {}", "ヘルプ: {} を開きました"),
    ("Help: cannot open {}: {}", "ヘルプ: {} を開けません: {}"),
    (
        "Settings reloaded from file",
        "設定をファイルから読み直しました",
    ),
    (
        "Settings not reloaded: {}",
        "設定を読み直せませんでした: {}",
    ),
    (
        "Full-resolution memory limit raised from {} MB to {} MB",
        "原寸の画像に使うメモリの上限を {} MB から {} MB に上げました",
    ),
    (
        "Window: the displays changed since last time, so the window opens where the OS puts it",
        "ウインドウ: 前回から画面の並びが変わったので、OS の決めた場所に開きます",
    ),
    (
        "Already sending. Wait for it to finish or cancel it first.",
        "送っている途中です。終わるのを待つか、先に取り消してください。",
    ),
    (
        "Map: the map is turned on in the settings",
        "地図: 設定で地図を使うようにしました",
    ),
];

/// ログに渡す訳の口。英語なら訳さない。
pub fn translator(lang: Language) -> Option<vw_log::log::Translator> {
    (lang != Language::English)
        .then(|| Box::new(move |english: &str| translate(lang, english)) as vw_log::log::Translator)
}

/// `english` の行を `lang` の言葉にする。表にない行と英語のときは `None`。
pub fn translate(lang: Language, english: &str) -> Option<String> {
    if lang == Language::English {
        return None;
    }
    JA.iter().find_map(|(en, ja)| {
        let args = captures(en, english)?;
        Some(fill(ja, &args))
    })
}

/// `template` の `{}` に入った中身。形が合わなければ `None`。
fn captures<'a>(template: &str, text: &'a str) -> Option<Vec<&'a str>> {
    let pieces: Vec<&str> = template.split("{}").collect();
    let (first, rest) = pieces.split_first()?;
    let mut text = text.strip_prefix(first)?;
    if rest.is_empty() {
        return text.is_empty().then(Vec::new);
    }
    let mut args = Vec::with_capacity(rest.len());
    for (i, piece) in rest.iter().enumerate() {
        if i == rest.len() - 1 {
            args.push(text.strip_suffix(piece)?);
        } else {
            // 空の区切り（`{}{}`）は表に置かない決まりなので、ここでは見つかる最初の所で切る。
            let at = text.find(piece)?;
            args.push(&text[..at]);
            text = &text[at + piece.len()..];
        }
    }
    Some(args)
}

/// 訳の型に中身を入れる。`{}` は順に、`{n}` は n 番目を入れる。
fn fill(template: &str, args: &[&str]) -> String {
    let mut out =
        String::with_capacity(template.len() + args.iter().map(|a| a.len()).sum::<usize>());
    let mut next = 0;
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inside = &after[..close];
        let index = if inside.is_empty() {
            next += 1;
            Some(next - 1)
        } else {
            inside.parse::<usize>().ok()
        };
        match index.and_then(|i| args.get(i)) {
            Some(arg) => out.push_str(arg),
            None => out.push_str(&rest[open..open + close + 2]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ja(text: &str) -> Option<String> {
        translate(Language::Japanese, text)
    }

    #[test]
    fn a_line_in_the_table_is_translated_with_its_values() {
        assert_eq!(
            ja("Opened folder /home/ken/photos").as_deref(),
            Some("フォルダ /home/ken/photos を開きました")
        );
    }

    #[test]
    fn numbered_places_reorder_the_values() {
        assert_eq!(
            ja("Loaded 12 images from C:\\pics").as_deref(),
            Some("C:\\pics から 12 枚の画像を読みました")
        );
    }

    #[test]
    fn error_text_inside_a_line_stays_as_it_is() {
        assert_eq!(
            ja("Cannot move or copy: Permission denied (os error 13)").as_deref(),
            Some("移動もコピーもできません: Permission denied (os error 13)")
        );
    }

    #[test]
    fn lines_not_in_the_table_and_english_are_not_translated() {
        assert_eq!(ja("Renderer: glow"), None);
        assert_eq!(ja("Opened folder"), None);
        assert_eq!(translate(Language::English, "Opened folder /a"), None);
    }

    #[test]
    fn a_line_without_values_must_match_whole() {
        assert_eq!(
            ja("Nothing to undo").as_deref(),
            Some("元に戻すものはありません")
        );
        assert_eq!(ja("Nothing to undo now"), None);
    }

    #[test]
    fn every_translation_uses_each_value_of_its_english_line() {
        for (en, ja) in JA {
            let n = en.matches("{}").count();
            assert!(!en.contains("{}{}"), "{en}");
            let args: Vec<String> = (0..n).map(|i| format!("<{i}>")).collect();
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let filled = fill(ja, &refs);
            for a in &args {
                assert_eq!(filled.matches(a.as_str()).count(), 1, "{en} -> {ja}");
            }
            assert!(!filled.contains('{'), "{ja}");
        }
    }

    #[test]
    fn every_english_line_is_listed_once() {
        for (i, (en, _)) in JA.iter().enumerate() {
            assert!(JA[..i].iter().all(|(e, _)| e != en), "{en}");
        }
    }
}
