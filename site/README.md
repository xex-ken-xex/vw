# site/ — 紹介ページと使い方

GitHub Pages（https://xex-ken-xex.github.io/vw/）に出すページの元です。
main に push すると `.github/workflows/pages.yml` が `scripts/build-site.sh` で組み立てて公開します。

| 場所 | 中身 |
| --- | --- |
| `index.html`、`en/index.html` | 紹介ページ（日本語・英語）。HTML をそのまま出します |
| `privacy.html`、`en/privacy.html` | プライバシー ポリシー |
| `manual/*.md` | 使い方の各章（日本語）。Markdown で書き、組み立てのときに HTML にします |
| `manual/template.html` | 使い方の章の HTML の型（見た目・上の帯・左の章の一覧） |
| `en/manual/*.md`、`en/manual/template.html` | 使い方の英語版。章のファイル名は日本語版と同じにします |

## scripts/build-site.sh

### 前提条件

- `sh` が動く環境（Linux、macOS、Windows では WSL か Git Bash）
- [pandoc](https://pandoc.org/) 3.x（Ubuntu: `sudo apt install pandoc`、Windows: `winget install JohnMacFarlane.Pandoc`）
- リポジトリ全体を取得していること（`docs/screenshot.png`、`docs/images/`、`packaging/icons/vw.png` を使います）

### 出来ること

1. 出力先のフォルダを空にして作り直します（既定は `_site`）
2. `site/` の HTML をそのままコピーします（`manual/` と `en/manual/` の Markdown・型と、この README は除きます）
3. スクリーンショットとアイコンを `docs/` と `packaging/` から `images/` へコピーします。README と同じ画像を使い回すので、`site/` に画像の複製は置きません。英語表示の画面（`docs/images/en/`）は `images/en/` へコピーします
4. `manual/*.md` と `en/manual/*.md` を 1 章ずつ `manual/*.html`、`en/manual/*.html` にします。上の帯の「English」「日本語」は、もう一方の言語の同じ章へ移ります。見出しの id は GitHub と同じ付け方なので、`sort.html#ai-タグと顔` のような日本語の見出しへのリンクがそのまま効きます。目次の章（`index.md`）のほかは、左にその章の見出しの一覧が付きます

### 使い方

```sh
./scripts/build-site.sh            # _site に組み立てる
./scripts/build-site.sh /tmp/site  # 出力先を変える
```

組み立てたら、`_site/index.html`、`_site/manual/index.html`、`_site/en/manual/index.html` をブラウザで開いて確かめます。
出力先のフォルダは毎回消して作り直すので、ほかのファイルを置いているフォルダを指定しないでください。

## 使い方の章を書くときの決まり

- 1 つの章は 1 つのユースケース（やりたいこと）にします。機能の名前ではなく、使う人がしたいことで分けます
- 先頭に `title` を書き、続けて 1 行でその章で出来ることを書きます

  ```markdown
  ---
  title: "写真を選り分ける"
  ---

  キー 1 つでフラグとタグを付け、良い写真を残し、要らない写真を片付ける。
  ```

- ほかの章へのリンクは `sort.html#見出し` のように `.html` で書きます。画像は `../images/名前.png`（元の画像は `docs/images/` に置きます）
- 章を足したら、`manual/index.md` の一覧と、`manual/template.html` の左の章の一覧にも足します
- 画面の名前は日本語表示のときのものを使い、ほかのアプリの名前は出しません
- 英語版（`en/manual/`）も同じ章・同じ見出しの並びにそろえます。画面の名前は英語表示のとき（`apps/vw/src/i18n.rs` の英語）のものを使い、画像は英語表示で撮って `docs/images/en/` に置き、`../../images/en/名前.png` で参照します（日本語版と同じ名前にします）。日本語版を直したら、英語版の同じ所も直します
- 他人の写真を写した画面には、画像のすぐ下に `<p class="credit">Photo: 出典</p>` で出典を書きます（今は RAW の例の NASA の写真 iss030e122639。パブリックドメインで義務ではありませんが、出どころを明らかにするため）
