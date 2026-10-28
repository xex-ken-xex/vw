# Microsoft Store 用の MSIX を作る

vw を Microsoft Store に出すための MSIX パッケージを作ります。
DicoAmplio と vwbatch は Store に出しません。vwbatch は zip 版で使います。

vwbatch を入れない理由: コマンドだけの道具をスタートメニューに出さない設定（`AppListEntry="none"`）にすると、
Store は「画面のないアプリ」とみなし、Microsoft の個別の許可（HeadlessAppBypass）がないと受け付けません
（2026-10-28 に Partner Center の検証で弾かれました）。

Store に出すと、Store が Microsoft の証明書で署名し直します。そのため、署名のない zip 版を止める
Windows の「スマート アプリ コントロール」でも、Store 版の vw は起動できます。証明書を買う必要はありません。

## できること

- `make-msix.ps1` が、zip 版と同じ中身（`vw.exe`・README・ライセンス・`check\`）に
  アイコンと `AppxManifest.xml` を足して、`.msix` を 1 つ作ります。フォルダに `vwbatch.exe` があっても入れません。
- できた MSIX では、次のように動きます。
  - スタートメニューに vw が出ます。
  - コマンドプロンプトから `vw` で呼べます（実行エイリアス）。
  - 設定やログは zip 版と同じ `%APPDATA%\vw` に置かれます。zip 版の設定をそのまま使えます。
  - VC++ ランタイム（`Microsoft.VCLibs.140.00.UWPDesktop`）は Store が一緒に入れます。
- MSIX には署名をしません。そのため、**できた `.msix` はそのままでは PC に入れられません**。
  Partner Center に上げるためだけのファイルです。

## 前提条件

- Windows 10 / 11 と PowerShell（Windows PowerShell 5.1 か PowerShell 7）
- Windows SDK（`makeappx.exe` が入っているもの）。GitHub Actions の `windows-latest` には入っています。
  手元で作るときは、Visual Studio Installer か
  [Windows SDK](https://developer.microsoft.com/windows/downloads/windows-sdk/) から入れます。
- `-Source` に渡すフォルダに、ビルド済みの `vw.exe` があること

## 使い方

### CI で作る（ふだんはこちら）

`.github/workflows/ci.yml` の Windows のビルドが毎回作ります。Actions の実行結果の **Artifacts** にある
`vw-store-msix` をダウンロードして展開すると、`vw-windows-x64.msix` が出てきます。
Release には載せません（署名がなく、利用者の PC には入れられないためです）。

### 手元で作る

```powershell
cargo build --release
# zip 版と同じ中身を 1 つのフォルダに集める
New-Item -ItemType Directory -Force dist\vw | Out-Null
Copy-Item target\release\vw.exe, README.md, LICENSE-MIT, LICENSE-APACHE dist\vw\
Copy-Item -Recurse packaging\windows\check dist\vw\check
# 版は Cargo.toml の [workspace.package] の version
powershell -ExecutionPolicy Bypass -File packaging\windows\msix\make-msix.ps1 -Source dist\vw -OutFile dist\vw-windows-x64.msix -Version 0.8.2
```

| 引数 | 意味 |
| --- | --- |
| `-Source` | 中身を集めたフォルダ（必須） |
| `-OutFile` | 作る `.msix` のパス（必須） |
| `-Version` | `0.8.2` の形の版（必須）。Store の決まりで、末尾に `.0` を足した `0.8.2.0` になります |
| `-IdentityName` `-Publisher` `-PublisherDisplayName` `-DisplayName` | Partner Center の「製品 ID」の値。省略すると、スクリプトに書いてある値を使います |

既定値は Partner Center で予約した「vw Image Viewer」（Store ID `9PP7JP120MW5`）の値です。

## Store に出す手順（初回）

1. [Partner Center](https://partner.microsoft.com/dashboard) に、個人の開発者として登録します（無料。本人確認があります）。
2. **アプリ と ゲーム → 新しい製品 → MSIX または PWA アプリ** で、アプリの名前を予約します。
   名前は Store 全体で重ならないものが要ります。`vw` だけでは取れないことがあります。
3. 製品の **製品管理 → 製品 ID** にある次の 4 つを、`make-msix.ps1` の既定値に書き込みます。
   - `Package/Identity/Name` → `-IdentityName`
   - `Package/Identity/Publisher` → `-Publisher`
   - `Package/Properties/PublisherDisplayName` → `-PublisherDisplayName`
   - 予約した名前 → `-DisplayName`
4. CI が作った `vw-windows-x64.msix` を、提出の **パッケージ** に上げます。
5. 提出の **申請オプション** で、制限付きの機能の理由を書きます（下の文面）。
6. **ストア登録情報**（説明・スクリーンショット）と **年齢区分** を埋めて提出します。
   プライバシー ポリシーの URL には、サイトの `privacy.html` を使います。

### 制限付きの機能の理由（申請オプションに貼る文面）

> runFullTrust: vw is a Win32 desktop image viewer packaged with the Desktop Bridge.
>
> unvirtualizedResources: vw keeps its settings, logs and caches in %APPDATA%\vw, shared with the
> portable zip build. The app has a button that opens the settings file in the user's text editor and a
> menu that opens the log folder in File Explorer. Those processes run outside the package, so with
> file system write virtualization they would not see the files vw wrote. We therefore disable
> virtualization with desktop6:FileSystemWriteVirtualization.

## 2 回目からの更新

Cargo.toml の版を上げてから CI で作り直し、Partner Center で新しい提出を作ってパッケージを差し替えます。
Store は、前に出したものより大きい版しか受け付けません。

## ファイル

| ファイル | 中身 |
| --- | --- |
| `make-msix.ps1` | MSIX を作るスクリプト |
| `AppxManifest.xml` | パッケージの定義のひな形。`{{...}}` をスクリプトが埋める |
| `Assets/` | スタートメニューと Store のアイコン。`packaging/icons/vw.png` を縮めたもの（`convert vw.png -resize 150x150 Square150x150Logo.png` など。44・50 も同じ） |
