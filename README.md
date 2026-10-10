# vw

Linux / Windows 向けの高速な画像ビューワ（Rust + egui）。

現在対応している形式: PNG（8/16bit、グレー・RGB・アルファ・パレット）

## ビルドと実行

```sh
cargo run --release -- [フォルダ または 画像ファイル ...]
```

Linux では X11/Wayland と OpenGL のライブラリ（`libxkbcommon-x11-0`, `libgl1` など）が必要です。

## 操作

| 操作 | 動作 |
| --- | --- |
| ツリーでクリック | アクティブなスロットに表示 |
| ツリーで Ctrl+クリック | スロットを追加して表示 |
| ホイール / ドラッグ / ダブルクリック | ズーム / 移動 / 全体表示 |
| ← → PageUp PageDown Space | 同じフォルダの前後の画像 |
| View メニュー | 4 つのフィールドの表示・非表示 |

設計は [docs/architecture.md](docs/architecture.md) を参照。
