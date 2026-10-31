# vw が作るファイル

[README](../../README.md) › [使い方](README.md)


vw は元の画像ファイルを書き換えません。代わりに、次のファイルを作ります。

| ファイル | 場所 | 中身 |
| --- | --- | --- |
| `settings.toml` | 設定フォルダ（Windows: `%APPDATA%\vw`、Linux: `~/.config/vw`） | 設定 |
| `state.toml` | 設定フォルダ | 終了時の状態（ウインドウの位置と大きさ、ツリーの開閉、いたフォルダ、取り出し先など）。vw が自動で書き換える |
| `.vw-labels.json` | ラベルを付けた画像のあるフォルダ | そのフォルダの画像のフラグとタグ |
| `labels/` | 設定フォルダ | 書き込めないフォルダ（読み取り専用のメディアなど）の画像のラベル |
| `vw-search-catalog.json` | データセットのフォルダの直下 | 検索カタログ |
| `map-tiles/` | 設定フォルダ | 地図の絵（タイル）。消しても、次に地図を開いたとき取り直す |

## .vw-labels.json（ラベル）

```json
{
  "version": 1,
  "files": {
    "IMG_0001.jpg": { "flag": "good", "tags": ["山"] },
    "IMG_0003.jpg": { "flag": "bad" },
    "IMG_0006.jpg": { "tags": ["花"] },
    "IMG_0007.png": { "position": { "latitude": 43.0621, "longitude": 141.3544 } }
  }
}
```

- ドットで始まるので、エクスプローラや Linux のファイルマネージャーでは隠しファイルとして扱われます
- 画像を vw で移動・コピーすると、ラベルも移す先のフォルダのファイルへ移ります
- `position` は地図で置いた撮影地点です（画像の EXIF は書き換えません）

## .vw-ai-tags.json（AI タグと顔の枠）

```json
{
  "version": 1,
  "files": {
    "IMG_0001.jpg": {
      "model": "wd-vit-tagger-v3",
      "general": [{ "name": "outdoors", "score": 0.81 }],
      "rating": { "name": "general", "score": 0.97 },
      "faces": [{ "rect": [0.35, 0.12, 0.18, 0.22], "score": 0.94, "person": "Eileen", "similarity": 0.96 }]
    }
  }
}
```

- 顔の枠は画像の幅・高さに対する割合です。顔の特徴はここには入りません

## vw-search-catalog.json（検索カタログ）

JSON のファイルで、主に次のものが入っています。

| キー | 中身 |
| --- | --- |
| `name` / `memo` / `tags` / `fields` | データセットの名前・メモ・タグ・項目 |
| `created` / `updated` | 作った日時と、更新した日時 |
| `source` | 取り出して作ったデータセットなら、元のデータセット |
| `meta` | 「種類・キー・値」の組の一覧。同じ組は 1 回だけ持つ |
| `files` | ファイルごとのパス（データセットのフォルダからの相対パス。ZIP の中は `data.zip!/day1/Z001.jpg` のように `!` で区切る）・大きさ・更新日時と、`meta` の何番目の組を持つか |
| `skipped` | 読めなかったファイルと理由 |

手で書き換える必要はありません。名前・メモ・タグ・項目は vw のダイアログで直せます。
