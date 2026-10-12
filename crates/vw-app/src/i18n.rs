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

/// 実行時に決まる文言（エラーなど）を訳す。訳がなければそのまま。
pub fn tr_text(lang: Language, en: &str) -> String {
    match lang {
        Language::English => en.to_owned(),
        Language::Japanese => JA
            .iter()
            .find(|(k, _)| *k == en)
            .map_or(en, |(_, ja)| *ja)
            .to_owned(),
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
    ("Frame", "フレーム"),
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
    ("(empty)", "（空）"),
    (
        "Click a folder in the tree to show its contents",
        "ツリーのフォルダをクリックすると中身を表示します",
    ),
    ("No images or folders here", "画像もフォルダもありません"),
    ("Open this folder", "このフォルダへ移動"),
    ("Double-click to show the images inside", "ダブルクリックで中の画像を表示"),
    ("Dropped frames period", "コマ落ちを数える期間"),
    ("The play bar shows how many frames were skipped (not decoded in time) during this many last seconds.", "再生バーには、直近のこの秒数の間に落ちた（読み込みが間に合わずに飛ばした）コマの数を出します。"),
    (" s", " 秒"),
    ("Keep zoom when stepping", "前後の画像で倍率と位置を保つ"),
    ("Keep the zoom and position when moving to the previous or next image in a slot that holds several images (a folder, frames or a series). DICOM images always keep them.", "複数枚あるスロット（フォルダ・フレーム・シリーズ）で前後の画像に送るとき、倍率と位置を引き継ぎます。DICOM は常に引き継ぎます。"),
    ("Develop RAW in the background", "RAW を裏で現像"),
    ("Camera RAW files first show the preview JPEG embedded by the camera, then are developed from the sensor data in the background and replaced (a film icon is shown on the slot meanwhile). When off, develop from the right-click menu.", "カメラ RAW は、まずカメラが埋め込んだプレビュー JPEG を表示し、裏でセンサーのデータを現像して差し替えます（その間、スロットにフィルムの印が出ます）。オフのときは右クリックメニューから現像します。"),
    ("Developing RAW...", "RAW を現像中..."),
    ("Move bad images to trash...", "悪い画像をごみ箱へ..."),
    ("Move {} bad images to the trash?", "悪いフラグの画像 {} 枚をごみ箱へ送りますか？"),
    ("...and {} more", "...ほか {} 枚"),
    ("… and {} more", "… ほか {} 件"),
    ("No values", "値がありません"),
    ("Move to trash", "ごみ箱へ送る"),
    ("{} is encrypted. Enter the password.", "{} は暗号化されています。パスワードを入力してください。"),
    ("Wrong password.", "パスワードが違います。"),
    ("Open", "開く"),
    ("Medical images (DICOM)", "医用画像 (DICOM)"),
    ("Extensions to check for DICOM", "DICOM か調べる拡張子"),
    ("Regular expression for the extensions (without the dot, empty for no extension, case-insensitive) of files whose first bytes are read to see if they are DICOM. .dcm, .dicom and .dic files are always shown as DICOM. The default checks files with no extension, only digits, dcm, dicom or ima.", "先頭を読んで DICOM か調べるファイルの拡張子の正規表現（. を除いた部分。拡張子がなければ空文字列。大文字小文字は区別しない）。.dcm / .dicom / .dic は常に DICOM として扱います。既定は、拡張子なし・数字のみ・dcm・dicom・ima を調べます。"),
    ("Not a valid regular expression", "正規表現として正しくありません"),
    ("Close ZIP", "ZIP を閉じる"),
    ("Images inside a ZIP cannot be labeled", "ZIP の中の画像にはラベルを付けられません"),
    ("Reading DICOM headers…", "DICOM のヘッダを読んでいます…"),
    ("Close slot", "スロットを閉じる"),
    // コピー
    ("Copy", "コピー"),
    ("Copy all", "すべてコピー"),
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
        "Order of the folders in the tree and of the folders and images in the slots, also used for previous/next image. Folders always come first.",
        "ツリーのフォルダと、スロットに並べるフォルダ・画像の並び順。前後の画像への移動もこの順になります。フォルダは常に画像より前です。",
    ),
    ("Name (numbers by value)", "ファイル名（数字は数値として比較）"),
    ("Name (character by character)", "ファイル名（文字列として比較）"),
    ("Modified date", "更新日時"),
    ("Descending", "降順"),
    (
        "Reverse the sort order (Z to A, newest first).",
        "並び順を逆にします（Z から A、新しい順）。",
    ),
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
    ("Move", "移動"),
    ("Move \"{}\" to", "「{}」を次のフォルダへ移動します"),
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
    ("Previous image slot", "前の画像スロット"),
    ("Next image slot", "次の画像スロット"),
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
    (
        "Maximize/restore slot, or open folder",
        "スロットの最大化/元に戻す、またはフォルダへ移動",
    ),
    ("Up one folder", "1 つ上のフォルダへ"),
    ("Maximize slot", "スロットを最大化"),
    ("Fit image to slot", "スロットに合わせる"),
    ("Actual size (100%)", "実寸 (100%)"),
    ("Develop RAW from sensor data", "RAW をセンサーのデータから現像"),
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
    ("Frames skipped in the last {} seconds because images could not be decoded fast enough.", "直近 {} 秒の間に、デコードが間に合わずに飛ばしたコマの数です。"),
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
        "Put each image of the current folder into its own slot, or all of them into one slot that steps through them.",
        "今いるフォルダの画像を 1 枚ずつ別のスロットに入れるか、まとめて 1 つのスロットに入れて中の画像を送るか。",
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
        "How slots are arranged. Auto fits every slot on one page up to the Auto layout maximum; a fixed grid shows the rest on further pages (Page Up / Page Down).",
        "スロットの並べ方。自動は自動レイアウトの最大までなら全スロットを 1 ページに並べます。列 x 行を決めると、入りきらない分は次のページに回ります（Page Up / Page Down で送ります）。",
    ),
    ("Custom layout", "カスタムレイアウト"),
    (
        "Columns x rows used by the Custom layout in the View menu.",
        "表示メニューの「カスタム」で使う 列 x 行。",
    ),
    ("Auto layout maximum", "自動レイアウトの最大"),
    (
        "Largest grid (columns x rows) the Auto layout uses. More slots than this go on further pages, so slots do not get too small.",
        "自動レイアウトで使う最大の 列 x 行。これを超える数のスロットは次のページに回し、スロットが小さくなりすぎないようにします。",
    ),
    ("{} slots", "{} スロット"),
    // データセット
    ("Datasets", "データセット"),
    ("Register as dataset", "データセットとして登録"),
    ("Edit dataset", "データセットを編集"),
    ("Update dataset", "データセットを更新"),
    ("Register as dataset...", "データセットとして登録..."),
    ("Edit dataset...", "データセットを編集..."),
    ("Register", "登録"),
    ("Save", "保存"),
    ("Save and update", "保存して更新"),
    ("File speed", "ファイルの速さ"),
    (
        "Play at the speed written in the file. Changing the FPS turns this off.",
        "ファイルに書かれた速さで再生します。FPS を変えると外れます。",
    ),
    ("This file has no speed information.", "このファイルには速さの情報がありません。"),
    ("Extract dataset", "データセットの取り出し"),
    ("Destination", "出力先"),
    (
        "The files are copied with their folders. Files that already exist there are not overwritten.",
        "ファイルはフォルダ構成のまま写します。同じ名前のファイルがあれば上書きしません。",
    ),
    ("Register the destination as a dataset", "出力先をデータセットとして登録する"),
    ("The destination is already a dataset.", "出力先はすでにデータセットです。"),
    ("Extract", "取り出す"),
    ("Extract...", "取り出し..."),
    ("Show", "表示"),
    ("Later", "後で"),
    (
        "Stopped extracting. The files already copied remain in {}.",
        "取り出しを止めました。写したファイルは {} に残っています。",
    ),
    (
        "The files were extracted into the dataset \"{}\". Update it now?",
        "データセット「{}」の中に取り出しました。このデータセットを更新しますか？",
    ),
    (
        "Until it is updated, its search catalog does not include the extracted files.",
        "更新するまで、その検索カタログには取り出したファイルが入りません。",
    ),
    ("Enter the full path of the destination folder.", "出力先のフォルダをフルパスで入れてください。"),
    ("The destination is inside the dataset being extracted.", "出力先が、取り出すデータセットの中にあります。"),
    ("The destination is not a folder.", "出力先がフォルダではありません。"),
    (
        "Read all files again and rebuild the search catalog.",
        "全ファイルを読み直して検索カタログを作り直します。",
    ),
    ("Name", "名前"),
    ("Memo", "メモ"),
    ("Separate with commas", "カンマ区切り"),
    ("Stopped reading {}.", "{} の読み込みを止めました。"),
    ("Skip", "スキップ"),
    (
        "Finish the open dataset dialog first.",
        "開いているデータセットのダイアログを先に閉じてください。",
    ),
    ("Field names must be unique.", "項目名が重なっています。"),
    ("Dataset folders", "データセットのフォルダ"),
    (
        "Folders searched for datasets (folders with a search catalog), one per line. Subfolders are searched too.",
        "データセット（検索カタログのあるフォルダ）を探すフォルダ。1 行に 1 つ。下のフォルダも探します。",
    ),
    ("Dataset fields", "データセットの項目"),
    (
        "Fields you fill in for each dataset besides the name, memo and tags (such as where the data came from), separated by commas. They can be used as search conditions.",
        "名前・メモ・タグのほかに、データセットごとに書き込む項目（入手元など）。カンマ区切り。検索の条件に使えます。",
    ),
    ("Search names, memos, tags, fields and metadata", "名前・メモ・タグ・項目・メタデータを検索"),
    ("Advanced", "詳細"),
    ("Reload", "再読み込み"),
    ("Open in file manager", "フォルダを開く"),
    (
        "Look for search catalogs again, to show catalogs added or deleted outside vw.",
        "検索カタログを探し直します。vw の外でカタログを足したり消したりしたときに使います。",
    ),
    ("Update all", "すべて更新"),
    ("Read all files of the datasets shown again and rebuild their search catalogs.", "表示中のデータセットの全ファイルを読み直し、検索カタログを作り直します。"),
    ("Add dataset folders in Settings > Datasets.", "設定 > データセット でデータセットのフォルダを追加してください。"),
    ("Looking for datasets…", "データセットを探しています…"),
    ("{} datasets", "{} 件のデータセット"),
    ("{} search catalogs could not be read", "{} 個の検索カタログを読めませんでした"),
    ("kind", "種類"),
    ("key", "キー"),
    ("contains", "含む"),
    ("equals", "一致"),
    ("regex", "正規表現"),
    ("range", "数値の範囲"),
    ("exists", "存在する"),
    ("min", "下限"),
    ("max", "上限"),
    ("Condition", "条件"),
    ("Edit...", "編集..."),
    ("Update", "更新"),
    ("Files", "ファイル"),
    ("Updated", "更新日時"),
    ("Show the {} files found", "当たった {} 件を表示"),
    ("Show only the files that meet the search conditions, one slot each.", "検索の条件に合うファイルだけを、1 件ずつスロットに並べます。"),
    ("Files found ({})", "当たったファイル（{} 件）"),
    ("Extract only the files found by the search ({})", "検索で当たったファイルだけを取り出す（{}）"),
    ("Extracted from", "取り出し元"),
    ("Not read ({})", "読めなかったもの ({})"),
    ("Hits / files", "ヒット / ファイル"),
    ("Location", "場所"),
    ("Open/close datasets", "データセット画面を開く・閉じる"),
    // 全画面スライドショー
    ("Fullscreen slideshow...", "全画面スライドショー..."),
    ("Fullscreen slideshow", "全画面スライドショー"),
    ("Fullscreen: exit", "全画面: 終了"),
    ("Fullscreen: next image", "全画面: 次の画像"),
    ("Fullscreen: previous image", "全画面: 前の画像"),
    ("Fullscreen: next folder", "全画面: 次のフォルダ"),
    ("Fullscreen: previous folder", "全画面: 前のフォルダ"),
    ("Fullscreen: pause/resume", "全画面: 一時停止・再開"),
    ("Pixel probe on/off", "画素値の確認の切り替え"),
    ("Delete selected probe point", "選んだ十字を消す"),
    ("Copy point value", "十字の値をコピー"),
    ("Delete point", "十字を消す"),
    ("In order", "順送り"),
    ("Follow the newest file", "最新ファイルを追う"),
    ("Mode", "モード"),
    ("Show the images folder by folder, or keep showing the newest file under the folder and switch as soon as a new one arrives (for tethered shooting).", "画像をフォルダごとに順に見せるか、フォルダ以下で最も新しいファイルを見せ続け、新しいファイルが来たらすぐ切り替えるか（テザー撮影向け）。"),
    ("Sort by", "並べる基準"),
    ("Order of the folders and of the images in each folder.", "フォルダと、各フォルダの中の画像の並べ方。"),
    ("Folders descending", "フォルダを降順"),
    ("Go through the folders in reverse order (Z to A, newest first).", "フォルダを逆の順（Z から A、新しい順）に回ります。"),
    ("Images descending", "画像を降順"),
    ("Show the images in each folder in reverse order (Z to A, newest first).", "各フォルダの中の画像を逆の順（Z から A、新しい順）に見せます。"),
    ("Interval", "送りの間隔"),
    ("Seconds before moving on to the next image. 0 moves only with the keys. Animations and multi-frame images play at the file's speed before moving on; with 0 only their first frame is shown.", "次の画像へ送るまでの秒数。0 ならキーでだけ送ります。アニメーションやマルチフレームの画像はファイルの速さで再生してから次へ進みます。0 のときは先頭のフレームだけを見せます。"),
    ("Start over at the end", "最後まで行ったら最初に戻る"),
    ("After the last image, go back to the first one instead of stopping.", "最後の画像の次は、止まらずに最初の画像に戻ります。"),
    ("Show path", "パスを表示"),
    ("Show the path from the folder and the position in the folder at the bottom right.", "右下に、起点のフォルダからのパスとフォルダの中の位置を出します。"),
    ("Path color", "パスの文字色"),
    ("Color of the path text (#RRGGBB).", "パスの文字の色（#RRGGBB）。"),
    ("Path opacity", "パスの不透明度"),
    ("Opacity of the path text (%).", "パスの文字の不透明度（%）。"),
    ("Path text size", "パスの文字の大きさ"),
    ("Size of the path text.", "パスの文字の大きさ。"),
    ("Images in this folder and all folders below it are shown. Folders without images are skipped.", "このフォルダとその下のすべてのフォルダの画像を見せます。画像のないフォルダは飛ばします。"),
    ("Current folder", "今のフォルダ"),
    ("Folder not found", "フォルダが見つかりません"),
    ("Keys", "キー"),
    ("Mouse wheel", "マウスホイール"),
    ("Previous / next image", "前 / 次の画像"),
    ("Keys can be changed in Settings.", "キーは設定画面で変えられます。"),
    ("Start", "開始"),
    ("Paused", "一時停止中"),
    ("Looking for images...", "画像を探しています..."),
    ("Waiting for a new file...", "新しいファイルを待っています..."),
    ("No images under {}", "{} の下に画像がありません"),
    ("Cannot open this image: {}", "この画像を開けません: {}"),
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
