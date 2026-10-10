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
    ("(no subfolders)", "（サブフォルダなし）"),
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
    ("Full-resolution memory limit", "フル解像度のメモリ上限"),
    (
        "Memory used for full-resolution images (MB). Beyond this, images are shown from 512 px thumbnails until memory frees up.",
        "フル解像度の画像に使うメモリの上限（MB）。超えた分は、空きができるまで 512px の縮小画像で表示します。",
    ),
    ("Sort order", "並び順"),
    (
        "Order of files and folders in the tree, also used for previous/next image. Folders always come first.",
        "ツリーのファイルとフォルダの並び順。前後の画像への移動もこの順になります。フォルダは常にファイルより前です。",
    ),
    ("Name (numbers by value)", "ファイル名（数字は数値として比較）"),
    ("Name (character by character)", "ファイル名（文字列として比較）"),
    ("Modified date", "更新日時"),
    ("Descending", "降順"),
    (
        "Reverse the sort order (Z to A, newest first).",
        "並び順を逆にします（Z から A、新しい順）。",
    ),
    ("Tree mode", "ツリーのモード"),
    (
        "Normal shows folders and images; click an image to open it. Auto-load shows folders only and loads all images of a folder into slots.",
        "ノーマルはフォルダと画像を表示し、画像をクリックして開きます。画像自動展開はフォルダだけを表示し、フォルダの画像をまとめてスロットに読み込みます。",
    ),
    ("Normal", "ノーマル"),
    ("Auto-load images", "画像自動展開"),
    ("Folder click (auto-load)", "フォルダのクリック（画像自動展開）"),
    (
        "How a folder is expanded/collapsed and how it is loaded in auto-load mode. Images directly in the folder load at once; subfolders are asked about first.",
        "画像自動展開モードでの、フォルダの開閉と読み込みの操作。フォルダ直下の画像はすぐ読み込み、サブフォルダは先に確認します。",
    ),
    (
        "Click: expand after the double-click time / Double-click: load",
        "クリック: ダブルクリック時間の後に開閉 / ダブルクリック: 読み込み",
    ),
    (
        "Click: expand at once / Double-click: undo and load",
        "クリック: すぐ開閉 / ダブルクリック: 開閉を戻して読み込み",
    ),
    (
        "Click: expand / Shift+click: load",
        "クリック: 開閉 / Shift+クリック: 読み込み",
    ),
    ("Double-click time", "ダブルクリック時間"),
    (
        "Two clicks within this time (ms) count as a double-click.",
        "この時間（ミリ秒）以内の 2 回のクリックをダブルクリックとみなします。",
    ),
    ("Loading another folder", "別のフォルダを読み込むとき"),
    (
        "What happens to the open slots when another folder is loaded. Expanding or collapsing a folder never closes slots.",
        "別のフォルダを読み込むときに、表示中のスロットをどうするか。フォルダを開閉するだけではスロットは閉じません。",
    ),
    ("Replace the slots", "スロットを置き換える"),
    ("Add after the slots", "スロットの後ろに追加する"),
    ("Confirm large loads", "大量の読み込みを確認"),
    (
        "Ask before continuing each time the number of loaded images reaches the chunk size.",
        "読み込んだ枚数が区切りの枚数に達するたびに、続けるか確認します。",
    ),
    ("Chunk size", "区切りの枚数"),
    (
        "Number of images to load before asking again.",
        "次に確認するまでに読み込む枚数。",
    ),
    (" images", " 枚"),
    (
        "\"{}\" has subfolders. Load the images in all of them too?",
        "「{}」にはサブフォルダがあります。すべてのサブフォルダの画像も読み込みますか？",
    ),
    ("Load subfolders", "サブフォルダも読み込む"),
    ("This folder only", "このフォルダだけ"),
    (
        "{} images loaded. Load the next {}?",
        "{} 枚読み込みました。次の {} 枚も読み込みますか？",
    ),
    ("Load more", "続けて読み込む"),
    ("Stop", "やめる"),
    (
        "Double-click a folder in the tree to load its images",
        "ツリーのフォルダをダブルクリックすると画像を読み込みます",
    ),
    (
        "Shift+click a folder in the tree to load its images",
        "ツリーのフォルダを Shift+クリックすると画像を読み込みます",
    ),
    ("Behavior", "動作"),
    ("Group heading click", "グループ見出しのクリック"),
    (
        "What clicking a group heading (such as File in the metadata panel) does. The arrow always expands or collapses.",
        "グループの見出し（メタデータ欄の「ファイル」など）をクリックしたときの動作。▶ は常に開閉します。",
    ),
    ("Auto reload", "自動リロード"),
    (
        "Watch open folders. New images appear in the tree and folder slots, a slot showing the last image moves to the newest one, and overwritten images are reloaded.",
        "開いているフォルダを監視します。新しい画像をツリーとフォルダスロットに加え、最後の画像を表示しているスロットは最新の画像に移り、上書きされた画像は読み直します。",
    ),
    ("Flags and tags", "フラグとタグ"),
    ("Tagging mode", "タグ付けモード"),
    ("Show all images", "すべての画像を表示"),
    ("Show only good images", "良い画像だけを表示"),
    ("Show only bad images", "悪い画像だけを表示"),
    ("Show only unflagged images", "フラグのない画像だけを表示"),
    ("Export images...", "画像を書き出す..."),
    ("Labels", "ラベル"),
    ("Show only tag", "タグで絞り込み"),
    ("Export images", "画像の書き出し"),
    ("Folder", "フォルダ"),
    ("Include subfolders", "サブフォルダも含める"),
    ("Images shown", "対象の画像"),
    ("Copy files to a folder", "フォルダへコピー"),
    ("CSV list", "CSV の一覧"),
    ("JSONL list", "JSONL の一覧"),
    ("Destination folder", "コピー先のフォルダ"),
    ("Keep folder structure", "フォルダ構成を保つ"),
    ("Output file", "書き出すファイル"),
    ("Export", "書き出す"),
    ("Cancel", "キャンセル"),
    ("Close", "閉じる"),
    ("Select a folder in the tree first.", "先にツリーでフォルダを選んでください。"),
    ("Filter", "絞り込み"),
    ("Click to change", "クリックで切り替え"),
    ("No images match the filter", "絞り込みに合う画像がありません"),
    ("all", "すべて"),
    ("good", "良い"),
    ("bad", "悪い"),
    ("unflagged", "フラグなし"),
    ("Good", "良い"),
    ("Bad", "悪い"),
    ("Add tags in Settings > Flags and tags", "タグは 設定 > フラグとタグ で追加できます"),
    ("Tags", "タグ"),
    (
        "Tags you can put on images, separated by commas. The first nine are put on with the keys 1 to 9 in tagging mode.",
        "画像に付けられるタグ（カンマ区切り）。先頭から 9 個は、タグ付けモードで 1〜9 のキーで付けられます。",
    ),
    ("Tag names must be unique.", "タグの名前は重ならないようにしてください。"),
    ("Next image after labeling", "付けたら次の画像へ"),
    (
        "Move to the next image after putting a flag or tag on an image in tagging mode.",
        "タグ付けモードで画像にフラグやタグを付けたら、次の画像へ進みます。",
    ),
    ("Tagging mode on/off", "タグ付けモードの切り替え"),
    ("Flag as good", "良いフラグ"),
    ("Flag as bad", "悪いフラグ"),
    ("Tag 1", "タグ 1"),
    ("Tag 2", "タグ 2"),
    ("Tag 3", "タグ 3"),
    ("Tag 4", "タグ 4"),
    ("Tag 5", "タグ 5"),
    ("Tag 6", "タグ 6"),
    ("Tag 7", "タグ 7"),
    ("Tag 8", "タグ 8"),
    ("Tag 9", "タグ 9"),
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
    // 画像の操作とレイアウト
    ("Image", "画像"),
    ("Layout", "レイアウト"),
    ("Auto", "自動"),
    ("Custom", "カスタム"),
    ("Previous slot", "前のスロット"),
    ("Next slot", "次のスロット"),
    ("Previous page", "前のページ"),
    ("Next page", "次のページ"),
    ("Maximize/restore slot", "スロットの最大化/元に戻す"),
    ("Maximize slot", "スロットを最大化"),
    ("Fit image to slot", "スロットに合わせる"),
    ("Actual size (100%)", "実寸 (100%)"),
    ("Zoom in", "拡大"),
    ("Zoom out", "縮小"),
    ("Layout: auto", "レイアウト: 自動"),
    ("Layout: 1 x 1", "レイアウト: 1 x 1"),
    ("Layout: 1 x 2", "レイアウト: 1 x 2"),
    ("Layout: 2 x 1", "レイアウト: 2 x 1"),
    ("Layout: 2 x 2", "レイアウト: 2 x 2"),
    ("Layout: 3 x 3", "レイアウト: 3 x 3"),
    ("Layout: 4 x 4", "レイアウト: 4 x 4"),
    ("Layout: custom", "レイアウト: カスタム"),
    // スライドショーとフォルダスロット
    ("Play/pause slideshow", "スライドショーの再生/一時停止"),
    ("Slideshow", "スライドショー"),
    ("Play", "再生"),
    ("Pause", "一時停止"),
    ("Loop", "ループ"),
    ("Stop at end", "最後で止める"),
    ("Ping-pong", "往復"),
    ("dropped {}", "コマ落ち {}"),
    ("Images could not be decoded fast enough; some frames were skipped.", "デコードが間に合わず、一部のコマを飛ばしました。"),
    ("Slideshow speed", "スライドショーの速さ"),
    (
        "Frames per second of the slideshow. The play bar of a maximized slot changes and remembers it.",
        "スライドショーの 1 秒あたりのコマ数。最大化したスロットの再生バーで変えると、ここにも残ります。",
    ),
    ("Slideshow end", "スライドショーの終わり"),
    (
        "What happens after the last image: start over, stop, or play backwards.",
        "最後の画像の後の動き。先頭に戻る、止まる、逆向きに戻る。",
    ),
    ("Load folders into", "フォルダの読み込み先"),
    (
        "Auto-load mode: put each image into its own slot, or each folder into one slot that steps through its images.",
        "画像自動展開モードで、画像を 1 枚ずつ別のスロットに入れるか、フォルダごとに 1 つのスロットに入れて中の画像を送るか。",
    ),
    ("One slot per image", "画像ごとに 1 スロット"),
    ("One slot per folder", "フォルダごとに 1 スロット"),
    ("Key for the other way", "逆の入れ方にするキー"),
    (
        "Hold this key while loading a folder to load it the other way.",
        "このキーを押しながらフォルダを読み込むと、逆の入れ方で読み込みます。",
    ),
    ("None", "なし"),
    ("Slot layout", "スロットのレイアウト"),
    (
        "How slots are arranged. Auto fits every slot on one page; a fixed grid shows the rest on further pages (Page Up / Page Down).",
        "スロットの並べ方。自動はすべてのスロットを 1 ページに並べます。列 x 行を決めると、入りきらない分は次のページに回ります（Page Up / Page Down で送ります）。",
    ),
    ("Custom layout", "カスタムレイアウト"),
    (
        "Columns x rows used by the Custom layout in the View menu.",
        "表示メニューの「カスタム」で使う 列 x 行。",
    ),
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
