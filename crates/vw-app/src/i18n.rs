//! 画面の文言の翻訳。英語の文言をそのまま鍵にして、日本語訳を引く。
//!
//! gettext や fluent などを使わないのは、言語が 2 つで文言も 100 程度のうちは、
//! 表 1 つで足り、起動時に翻訳ファイルを読む手間もかからないため。
//! 訳がない文言は英語のまま出す。
//!
//! 日本語の表示には日本語フォントが要る（[`crate::fonts`]）。

use crate::settings::Language;

/// `en` を `lang` の文言にする。
pub fn tr(lang: Language, en: &'static str) -> &'static str {
    match lang {
        Language::English => en,
        Language::Japanese => JA.iter().find(|(k, _)| *k == en).map_or(en, |(_, ja)| ja),
    }
}

/// 訳に `{}` を 1 つ含む文言を、`arg` を埋め込んで返す。
pub fn tr1(lang: Language, en: &'static str, arg: &str) -> String {
    tr(lang, en).replacen("{}", arg, 1)
}

const JA: &[(&str, &str)] = &[
    // メニュー
    ("File", "ファイル"),
    ("View", "表示"),
    ("New slot", "スロットを追加"),
    ("Close all slots", "すべてのスロットを閉じる"),
    ("Quit", "終了"),
    ("Folder tree", "フォルダツリー"),
    ("Metadata", "メタデータ"),
    ("Images", "画像"),
    ("Log", "ログ"),
    ("Settings", "設定"),
    // パネル
    ("Folders", "フォルダ"),
    ("No image", "画像なし"),
    ("Path", "パス"),
    ("Size", "サイズ"),
    ("Plugin", "プラグイン"),
    ("Pixels", "画素数"),
    ("Color", "色"),
    ("Sample", "サンプル"),
    ("Texture", "テクスチャ"),
    // 画像スロット
    ("Loading...", "読み込み中..."),
    (
        "Failed to load (see log)",
        "読み込めませんでした（ログを参照）",
    ),
    (
        "Select an image in the folder tree (Ctrl+click adds a slot)",
        "フォルダツリーで画像を選んでください（Ctrl+クリックでスロットを追加）",
    ),
    ("(empty)", "（空）"),
    ("Close slot", "スロットを閉じる"),
    // コピー
    ("Copy", "コピー"),
    ("Copied", "コピーしました"),
    // ツール列
    ("Browse", "ブラウズ"),
    ("Pan", "移動"),
    ("Zoom", "ズーム"),
    ("Pixel probe", "画素値の確認"),
    ("Measure", "計測"),
    ("Not available yet", "まだ使えません"),
    // 設定画面
    ("Back", "戻る"),
    ("Search settings", "設定を検索"),
    ("Open settings file", "設定ファイルを開く"),
    ("Not saved yet", "未保存"),
    ("No settings match", "一致する設定はありません"),
    ("Appearance", "外観"),
    ("Viewer", "ビューワ"),
    ("Keyboard shortcuts", "キーボードショートカット"),
    ("Theme", "テーマ"),
    (
        "Follow the OS dark/light setting, or always use one.",
        "OS のダーク／ライト設定に従うか、どちらかに固定します。",
    ),
    ("System", "OS に従う"),
    ("Dark", "ダーク"),
    ("Light", "ライト"),
    ("Language", "言語"),
    ("Language of menus and labels.", "メニューやラベルの言語。"),
    ("UI scale", "表示倍率"),
    ("Size of text and icons.", "文字とアイコンの大きさ。"),
    ("Compact panels", "パネルを詰めて表示"),
    (
        "Reduce padding to show more information.",
        "余白を減らして、より多くの情報を表示します。",
    ),
    ("Enabled", "有効"),
    ("Background color", "背景色"),
    (
        "Color behind images (#RRGGBB). Leave empty for the theme default.",
        "画像の背後の色（#RRGGBB）。空欄ならテーマの既定色。",
    ),
    (
        "Use the #RRGGBB format.",
        "#RRGGBB の形式で入力してください。",
    ),
    ("Theme default", "テーマの既定色"),
    ("Behavior", "動作"),
    ("Group heading click", "グループ見出しのクリック"),
    (
        "What clicking a group heading (such as File in the metadata panel) does. The arrow always expands or collapses.",
        "グループの見出し（メタデータ欄の「ファイル」など）をクリックしたときの動作。▶ は常に開閉します。",
    ),
    ("Select only", "選択のみ"),
    ("Select and expand/collapse", "選択して開閉"),
    ("Add key", "キーを追加"),
    (
        "Press a key... (Esc to cancel)",
        "キーを押してください（Esc で取り消し）",
    ),
    ("Click to remove", "クリックで削除"),
    ("Reset to default", "既定に戻す"),
    (
        "Already used by \"{}\". Remove it there first.",
        "「{}」で使用中です。先にそちらから外してください。",
    ),
    (
        "Already assigned to this action.",
        "この操作にはすでに割り当て済みです。",
    ),
    (
        "Assigned. Note: the OS usually uses this key for \"{}\", so it may not reach vw.",
        "割り当てました。ただし OS が「{}」に使うことが多く、vw に届かない場合があります。",
    ),
    (
        "Clipboard keys (Ctrl+C, Ctrl+X, Ctrl+V) are reserved by the OS.",
        "クリップボードのキー（Ctrl+C / Ctrl+X / Ctrl+V）は OS 用のため割り当てられません。",
    ),
    (
        "Also assigned to another action.",
        "別の操作にも割り当てられています。",
    ),
    ("Used by the OS for \"{}\".", "OS が「{}」に使っています。"),
    ("No keys", "キーなし"),
    // 操作
    ("Previous image", "前の画像"),
    ("Next image", "次の画像"),
    ("Show/hide folder tree", "フォルダツリーの表示切替"),
    ("Show/hide metadata", "メタデータの表示切替"),
    ("Show/hide images", "画像の表示切替"),
    ("Show/hide log", "ログの表示切替"),
    ("Open/close settings", "設定を開く／閉じる"),
    // OS が使うキーの用途
    ("Close window", "ウィンドウを閉じる"),
    ("Switch windows", "ウィンドウの切り替え"),
    ("Window menu", "ウィンドウメニュー"),
    ("System menu", "システムメニュー"),
    ("Select all", "すべて選択"),
    ("Undo", "元に戻す"),
    ("Redo", "やり直し"),
    ("Task Manager", "タスクマネージャー"),
    ("Start menu", "スタートメニュー"),
    ("Application menu", "アプリケーションメニュー"),
    ("Run command", "コマンドの実行"),
    ("Move window", "ウィンドウの移動"),
    ("Resize window", "ウィンドウのサイズ変更"),
    ("Maximize window", "ウィンドウの最大化"),
    ("Open terminal", "端末を開く"),
    ("Lock screen", "画面のロック"),
    ("Restart display server", "表示サーバの再起動"),
    ("Switch workspace", "ワークスペースの切り替え"),
    ("Switch virtual console", "仮想コンソールの切り替え"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{os_reserved, Action, KeyCombo, Os};
    use eframe::egui::Key;

    #[test]
    fn english_is_returned_unchanged() {
        assert_eq!(tr(Language::English, "Settings"), "Settings");
    }

    #[test]
    fn japanese_uses_the_table() {
        assert_eq!(tr(Language::Japanese, "Settings"), "設定");
    }

    #[test]
    fn untranslated_text_falls_back_to_english() {
        assert_eq!(
            tr(Language::Japanese, "zzz not in table"),
            "zzz not in table"
        );
    }

    #[test]
    fn placeholder_is_filled_in_both_languages() {
        let msg = "Used by the OS for \"{}\".";
        assert_eq!(
            tr1(Language::English, msg, "Undo"),
            "Used by the OS for \"Undo\"."
        );
        assert_eq!(
            tr1(Language::Japanese, msg, "元に戻す"),
            "OS が「元に戻す」に使っています。"
        );
    }

    #[test]
    fn each_english_text_appears_once() {
        for (i, (en, _)) in JA.iter().enumerate() {
            assert!(!JA[..i].iter().any(|(e, _)| e == en), "duplicate: {en}");
        }
    }

    #[test]
    fn placeholders_survive_translation() {
        for (en, ja) in JA {
            assert_eq!(en.matches("{}").count(), ja.matches("{}").count(), "{en}");
        }
    }

    #[test]
    fn every_action_has_a_japanese_name() {
        for a in Action::ALL {
            assert_ne!(tr(Language::Japanese, a.title()), a.title(), "{a:?}");
        }
    }

    #[test]
    fn every_os_key_reason_has_a_japanese_name() {
        let mods = [
            (false, false, true),
            (true, false, true),
            (true, false, false),
            (true, true, false),
            (false, true, true),
        ];
        for os in [Os::Windows, Os::Linux] {
            for key in Key::ALL {
                for (ctrl, shift, alt) in mods {
                    let combo = KeyCombo {
                        ctrl,
                        shift,
                        alt,
                        key: *key,
                    };
                    if let Some(reason) = os_reserved(os, combo) {
                        assert_ne!(tr(Language::Japanese, reason), reason, "{reason}");
                    }
                }
            }
        }
    }
}
