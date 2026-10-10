# vw アーキテクチャ

## 目標

- Linux / Windows で動く、起動と表示が速い画像ビューワ
- 対応予定形式: BMP, JPEG, PNG, DICOM, Nikon RAW (NEF), Sony RAW (ARW)
- 形式ごとの対応はプラグインとして追加する（最初は PNG のみ）

## 技術選定

| 項目 | 選択 | 理由 |
| --- | --- | --- |
| 言語 | Rust | ネイティブ速度、GC なし、Linux/Windows を同じコードでビルドできる |
| GUI | egui (eframe, glow バックエンド) | 即時モードで軽量、起動が速い、パネル構成が簡単。wgpu ではなく OpenGL (glow) を使い起動コストを抑える |
| PNG | `png` クレート | 純 Rust、16bit をそのまま取り出せる |

Electron 系（VSCode そのもの）は起動が重いので採用しない。見た目のレイアウトだけ VSCode に寄せる。

## データの流れ

```
ファイル (bytes)
   │  DecoderPlugin::decode        … 形式ごとのプラグイン（別スレッド）
   ▼
VwImage                            … vw が扱う共通メモリ形式
   │  display::to_display          … 階調変換・縮小（別スレッド）
   ▼
DisplayImage (RGBA8)
   │  egui テクスチャへ転送         … UI スレッド
   ▼
画面
```

- `VwImage`（`crates/vw-core/src/image.rs`）
  - 幅・高さ・チャンネル構成（Gray / GrayAlpha / RGB / RGBA）
  - 画素は `U8` / `U16` / `F32` のいずれか。16bit PNG・DICOM・RAW を 8bit に落とさず保持する
  - `significant_bits`（DICOM の 12bit、RAW の 14bit など）
  - メタデータ（グループ・キー・値の並び）
- `DisplayImage`（`crates/vw-core/src/display.rs`）
  - 描画用の RGBA8。ウィンドウ幅/レベル（`DisplayParams::window`）で階調を決める。DICOM のウィンドウ調整はここを使う
  - GPU のテクスチャ上限を超える画像は縮小する（今は 8192px。将来はタイル分割）

## プラグイン

`vw_core::DecoderPlugin` トレイト:

- `name()` / `extensions()` / `probe(header)` / `decode(bytes)` / `read_metadata(bytes)`
- `Registry` が中身（マジックナンバー）→ 拡張子の順でプラグインを選ぶ

現在は静的リンク（`crates/vw-app/src/main.rs` の `registry()` で登録）。
トレイトはジェネリクスなしのオブジェクト安全な形にしてあり、後で C ABI を被せれば
`.dll` / `.so` の動的プラグインにできる。

予定しているプラグイン（クレート候補）:

| 形式 | クレート案 |
| --- | --- |
| BMP | 自作（単純）または `image` の bmp |
| JPEG | `zune-jpeg`（高速） |
| DICOM | `dicom-object` + `dicom-pixeldata` |
| NEF / ARW | `rawler` または `rawloader` + デモザイク |

## 画面構成（SDI・シングルページ）

```
┌ File  View ──────────────────────────────────────┐
│ Folders │        Images (n 枚グリッド)   │ Metadata │
│         │  ┌────────┐┌────────┐          │          │
│         │  │ slot 1 ││ slot 2 │          │          │
│         │  └────────┘└────────┘          │          │
├─────────┴────────────────────────────────┴──────────┤
│ Log                                                 │
└─────────────────────────────────────────────────────┘
```

- 4 フィールド（フォルダツリー / メタデータ / 画像 / ログ）は View メニューで表示・非表示
- 画像フィールドは n 個のスロットを自動グリッドで並べる
  - ツリーでクリック: アクティブなスロットに表示
  - Ctrl+クリック: 新しいスロットを追加して表示
  - ホイール: カーソル位置を中心にズーム、ドラッグ: 移動、ダブルクリック: 全体表示に戻す
  - ← / → / PageUp / PageDown / Space: 同じフォルダの前後の画像
- メタデータはアクティブなスロットの画像を表示
- ログには読み込み・デコード・変換・テクスチャ転送の時間と、起動から最初の描画までの時間を出す

## 速さのための方針

- デコードと表示用変換は UI スレッド外で行う
- 日本語フォント（システムフォント）はバックグラウンドで読み込み、起動を待たせない
- フォルダツリーは開いたときに初めて読む
- リリースビルドは thin LTO・codegen-units=1・panic=abort
