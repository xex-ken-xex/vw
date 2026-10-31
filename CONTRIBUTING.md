# vw への協力のしかた

vw が受け付けるのは、**不具合の報告** と、その **修正の提案（プルリクエスト）** です。

1. **Issue（チケット）を起票する**: [Issues](https://github.com/xex-ken-xex/vw/issues/new/choose) で「不具合の報告」を選び、テンプレートに沿って書きます
2. **修正を提案する**: 直し方が分かる場合は、その Issue を元にプルリクエスト（PR）を出します。Issue の番号を PR の説明に書いてください（例: `Closes #123`）

コードを書かなくても、Issue を起票するだけで十分な協力です。

脆弱性は公開の Issue にせず、[SECURITY.md](SECURITY.md) の方法で非公開で知らせてください。

## 不具合の報告の書き方

- **vw の版**（「ヘルプ」→「vw Image Viewer について」）、**OS**、**手順**、**期待した結果と実際の結果** を書いてください
- 下のログウインドウのエラー（赤字）や、ログのファイル（ログウインドウの見出しのフォルダのボタンで開けます）を貼ってもらえると原因が早く分かります
- 特定のファイルでだけ起きる場合は、再現するファイルがあると助かります。**個人情報や人に見せられない画像は貼らないでください**。代わりに形式・大きさ・作ったカメラやソフトを書いてください
- Windows で起動しないときは、配布物の `check` フォルダの `check-vw.bat` が作る `vw-check-report.txt` を貼ってください（ユーザー名やフォルダの場所は伏せてあります）
- 同じ Issue がないか、先に検索してください

## 修正の提案の出し方

1. リポジトリを fork し、main からブランチを作ります
2. 直し、次のチェックがすべて通ることを確かめます

   ```sh
   cargo fmt --all --check                      # 書式
   cargo clippy --all-targets -- -D warnings    # 静的解析（警告もエラー扱い）
   cargo test                                   # 自動テスト
   cargo build --release
   ```

3. PR を出します。テンプレートに沿って、直す前と後で何が変わるかを書いてください

CI（GitHub Actions）が Linux と Windows で同じチェックを行います。緑になってからレビューします。
CI は Cargo.lock のままビルドする（`--locked`）ので、依存を変えたら Cargo.lock もコミットしてください。

### 書き方の決まり

情報ごとに書く場所を決めています（詳しくは [CLAUDE.md](CLAUDE.md)）。

| 場所 | 書くこと |
| --- | --- |
| コード | どう実現するか。名前と構造で表す |
| テストコード | 何を保証するか。テスト名（例: `decodes_rgb8_with_text`）と内容で仕様を表す |
| コミットログ | なぜこの変更をしたか。変更内容の列挙ではなく理由を書く |
| コードコメント | なぜ他のやり方を選ばなかったか |

- 直した不具合が二度と起きないことを表すテストを付けてください
- 画面の文言を変えたら、日本語の訳も `apps/vw/src/i18n.rs` で直してください
- 依存クレートを足すときは、ライセンスが [packaging/about.toml](packaging/about.toml) の一覧にあるか確かめてください。ないとビルドが失敗します
- 設計の全体像は [docs/architecture.md](docs/architecture.md) にあります

## ビルドの準備

| もの | 用途 |
| --- | --- |
| Rust（stable、rustup で導入） | コンパイラとビルドツール（cargo） |
| Windows: Visual Studio 2022 Build Tools の「C++ によるデスクトップ開発」 | Rust がリンカとして使います |
| Linux: build-essential | C コンパイラとリンカ |
| Git | ソースの取得 |

リポジトリを取得してから OS に合ったスクリプトを実行すると、これらを入れたうえで、リリースビルドとテストまで行います。何度実行しても問題ありません。

```sh
scripts\setup-windows.bat     # Windows（winget を使うので途中で管理者権限の確認が出ます。Build Tools は数 GB・10〜30 分）
./scripts/setup-ubuntu.sh     # Ubuntu（apt のために sudo のパスワードを聞かれます）
```

環境だけ揃えてビルドしない場合は `-NoBuild`（Windows）/ `--no-build`（Ubuntu）を付けます。

```sh
cargo run --release -- [フォルダ または 画像ファイル ...]   # ビルドして起動
```

実行ファイルは `target/release/vw`（Windows は `target\release\vw.exe`）にできます。
開発中は `--release` なしの `cargo run` でも動きます。依存ライブラリは最適化してビルドするので、デバッグビルドでも画像の読み込みはそれほど遅くなりません。

使い方のページ（`site/`）を直すときは、[site/README.md](site/README.md) の手順で組み立てて確かめられます。

## ライセンス

送ってもらった変更は、vw と同じ `MIT OR Apache-2.0` のデュアルライセンスで公開します（Apache License 2.0 の第 5 条のとおり）。
