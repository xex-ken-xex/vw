# vw への協力のしかた

不具合の報告・改良の提案・機能追加の要望は、どれも歓迎します。
次の流れでお願いします。

1. **Issue（チケット）を起票する**: [Issues](https://github.com/xex-ken-xex/vw/issues/new/choose) で「不具合の報告」か「改良・機能追加の要望」を選び、テンプレートに沿って書きます
2. **話し合う**: 大きな変更は、作り始める前に Issue で進め方を決めます。小さな修正（誤字・明らかな不具合）はすぐ 3 に進んでかまいません
3. **プルリクエスト（PR）を出す**: Issue の番号を PR の説明に書きます（例: `Closes #123`）

コードを書かなくても、Issue を起票するだけで十分な協力です。

脆弱性は公開の Issue にせず、[SECURITY.md](SECURITY.md) の方法で非公開で知らせてください。

## Issue の書き方

- 不具合は、**vw の版**（「ヘルプ」→「vw について」）、**OS**、**手順**、**期待した結果と実際の結果** を書いてください
- 下の **ログ欄** のエラー（赤字）や、ログのファイル（ログ欄の見出しのフォルダのボタンで開けます）を貼ってもらえると原因が早く分かります
- 特定のファイルでだけ起きる場合は、再現するファイルがあると助かります。**個人情報や人に見せられない画像は貼らないでください**。代わりに形式・大きさ・作ったカメラやソフトを書いてください
- 同じ Issue がないか、先に検索してください

## プルリクエストの出し方

1. リポジトリを fork し、main からブランチを作ります
2. 変更し、次のチェックがすべて通ることを確かめます

   ```sh
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   cargo build --release
   ```

3. PR を出します。テンプレートに沿って、変更の前と後で何が変わるかを書いてください

CI（GitHub Actions）が Linux と Windows で同じチェックを行います。緑になってからレビューします。

### 書き方の決まり

情報ごとに書く場所を決めています（詳しくは [CLAUDE.md](CLAUDE.md)）。

| 場所 | 書くこと |
| --- | --- |
| コード | どう実現するか。名前と構造で表す |
| テストコード | 何を保証するか。テスト名（例: `decodes_rgb8_with_text`）と内容で仕様を表す |
| コミットログ | なぜこの変更をしたか。変更内容の列挙ではなく理由を書く |
| コードコメント | なぜ他のやり方を選ばなかったか |

- 新しい振る舞いには、それを表すテストを付けてください
- 画面の文言を足したら、日本語の訳も `apps/vw/src/i18n.rs` に足してください
- 新しい依存クレートを足すときは、ライセンスが [packaging/about.toml](packaging/about.toml) の一覧にあるか確かめてください。ないとビルドが失敗します
- 設計の全体像は [docs/architecture.md](docs/architecture.md) にあります

## 開発とビルド

### 必要なもの

| もの | 用途 |
| --- | --- |
| Rust（stable、rustup で導入） | コンパイラとビルドツール（cargo） |
| Windows: Visual Studio 2022 Build Tools の「C++ によるデスクトップ開発」 | Rust がリンカとして使います |
| Linux: build-essential | C コンパイラとリンカ |
| Git | ソースの取得 |

### 環境を自動で揃える

リポジトリを取得してから、OS に合ったスクリプトを実行します。
必要なものを入れたうえで、リリースビルドとテストまで行います。何度実行しても問題ありません。

**Windows**

```bat
git clone https://github.com/xex-ken-xex/vw.git
cd vw
scripts\setup-windows.bat
```

`scripts\setup-windows.bat` はエクスプローラーからダブルクリックしても動きます。
winget で Visual Studio Build Tools、Git、rustup を入れるため、途中で管理者権限の確認が出ます。
Build Tools の導入には数 GB のダウンロードと 10〜30 分ほどかかります。
Git がまだない場合は、先に [Git for Windows](https://git-scm.com/download/win) を入れるか、
GitHub の **Code → Download ZIP** でソースを取得してください。

**Ubuntu**

```sh
git clone https://github.com/xex-ken-xex/vw.git
cd vw
./scripts/setup-ubuntu.sh
```

`sudo` のパスワードを聞かれます。apt で必要なパッケージを入れ、rustup で Rust を入れます。

どちらも、環境だけ揃えてビルドしない場合は `-NoBuild`（Windows）/ `--no-build`（Ubuntu）を付けます。

### ビルドと実行

```sh
cargo run --release -- [フォルダ または 画像ファイル ...]   # ビルドして起動
cargo build --release                                       # 実行ファイルだけ作る
```

実行ファイルは `target/release/vw`（Windows は `target\release\vw.exe`）にできます。
開発中は `--release` なしの `cargo run` でも動きます。依存ライブラリは最適化してビルドするので、
デバッグビルドでも画像の読み込みはそれほど遅くなりません。

### テストとチェック

プルリクエストを出す前に、CI と同じ次のチェックを通してください。

```sh
cargo fmt --all --check                      # 書式
cargo clippy --all-targets -- -D warnings    # 静的解析（警告もエラー扱い）
cargo test                                   # 自動テスト
cargo deny check advisories sources          # 依存の既知の脆弱性・取り下げられた版・取得元（cargo install cargo-deny）
```

CI は Cargo.lock のままビルドする（`--locked`）ので、依存を変えたら Cargo.lock もコミットしてください。
依存の検査は週 1 回も自動で走り、コードを変えていなくても新しく見つかった脆弱性に気づけます。
GitHub Actions はコミット SHA で固定してあり、新しい版は Dependabot の PR で取り込みます。

### リポジトリの構成

```
crates/
  vw-core/   共通メモリ形式 (VwImage)、プラグインの定義 (DecoderPlugin)、描画用変換
  vw-png/    PNG プラグイン
  vw-jpeg/   JPEG プラグイン
  vw-bmp/    BMP プラグイン
  vw-webp/   WebP プラグイン
  vw-gif/    GIF プラグイン
  vw-tiff/   TIFF プラグイン
  vw-fits/   FITS（天体の画像）プラグイン
  vw-raw/    カメラ RAW プラグイン（rawler、LGPL-2.1）
  vw-dicom/  文字コードの変換
  vw-ui/     見た目（配色・メニュー・パネル・フォント）。apps/ のアプリで共有する
  vw-tree/   フォルダツリーの基底と、それでフォルダやファイルを選ぶ窓。apps/ のアプリで共有する
  vw-pipeline/ 画素の変換（ノード・伸縮・見た目）と、その保存の形式。vw と vwbatch で共有する
  vw-compare/ 似た画像の位置合わせと、違いを数で表す比較
  vw-quality/ 画質の値（PSNR・SSIM など）を測る関数
  vw-dock/    ウインドウのドッキング（左右・上下に分けて並べ、入れ直す）
  vw-launch/  ファイルやフォルダを、OS の既定のアプリや設定で決めたアプリで開く
  vw-log/     ログウインドウ（ログの行・ログファイル・状態の行）
  vw-renderer/ OpenGL で起動し、使えなければ wgpu で起動し直す
apps/
  vw/          ビューア本体（実行ファイル vw）
  vwbatch/     保存した画素の変換を画像にまとめてかけるコマンド（実行ファイル vwbatch）
  dicoamplio/  評価データを水増しするツール（実行ファイル dicoamplio。apps/dicoamplio/README.md）
docs/        設計資料、使い方（docs/manual/）、README の画像
scripts/     開発環境の構築スクリプト
packaging/   配布物に同梱するファイル
```

設計の詳細は [docs/architecture.md](docs/architecture.md)、
コーディングのルールは [CLAUDE.md](CLAUDE.md) を参照してください。

### 新しい形式を追加する

1. `crates/vw-<形式名>` を作り、`vw_core::DecoderPlugin` を実装します
   - `probe` でファイル先頭のマジックナンバーを判定し、`decode` で `VwImage` を返します
   - 階調は落とさずに返します（16bit なら `PixelData::U16`）
2. `apps/vw/src/main.rs` の `registry()` に 1 行追加して登録します

`crates/vw-png` が実装例です。

### リリース

CI（GitHub Actions）が Linux と Windows でチェック・テスト・ビルドを行い、次のときに Release へ載せます。

| きっかけ | Release |
| --- | --- |
| main ブランチへの push（プルリクエストのマージ） | 「最新ビルド (main)」を置き換える |
| `v` で始まるタグの push | そのタグ名の正式版を作る |
| Actions の画面から CI を手動実行（版番号を入力） | main の最新コミットにタグを付け、正式版を作る |

正式版を出すときは、`Cargo.toml` の `version` を上げてから、次のどちらかを行います。

- GitHub の **Actions** → **CI** → **Run workflow** で、Branch を `main` のまま版番号（例: `v0.3.0`）を入れて実行する
- 手元の main でタグを付けて push する

```sh
git tag v0.3.0
git push origin v0.3.0
```

## ライセンス

送ってもらった変更は、vw と同じ `MIT OR Apache-2.0` のデュアルライセンスで公開します（Apache License 2.0 の第 5 条のとおり）。
