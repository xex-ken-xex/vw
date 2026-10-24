# check-vw: vw が起動しない Windows を調べるスクリプト

vw.exe をダブルクリックしても窓が出ない PC で、原因になりやすいところを順に調べ、
結果を `vw-check-report.txt` にまとめます。何もインストールせず、設定も書き換えません。

## 前提条件

- Windows 10 または 11 の PC（vw が起動しない PC そのもので動かします）
- Windows に最初から入っている Windows PowerShell 5.1 だけで動きます。Python などは要りません
- 管理者権限は要りません
- vw のリリース zip（`vw-windows-x64.zip`）を展開してあること。このスクリプトは zip の中の `check` フォルダに入っています
  （`vw.exe` がなくても、起動テスト以外は調べられます）

## 入っているファイル

| ファイル | 役割 |
| --- | --- |
| `check-vw.ps1` | 調べる本体。ユーザー名・PC 名・フォルダの場所を伏せて報告する |
| `check-vw.bat` | ダブルクリック用の入口。PowerShell のスクリプト実行が禁止された PC でも、この 1 回だけ許して動かします |
| `README.md` | この説明 |

## できること

1. **Windows の版**: ビルド番号、32/64 ビット、ARM 版か、仮想マシンか、リモートデスクトップ接続中か
2. **Visual C++ ランタイム**: vw.exe が読み込む `vcruntime140.dll` / `vcruntime140_1.dll` / `msvcp140.dll` / `ucrtbase.dll`
   が System32 にあるかと、その版（`msvcp140.dll` は 14.40 以上が要ります）
3. **グラフィックス**: GPU の名前とドライバ。「Microsoft 基本ディスプレイ アダプター」なら警告します
4. **OpenGL と DirectX 12**: 実際に OpenGL を開いて、版・レンダラ名を読みます。vw は OpenGL 2.0 以上で描画し、足りなければ DirectX 12 で起動し直すので、DirectX 12 (d3d12.dll) があるかも見ます。
   `1.1.0 / GDI Generic` のようにソフトウェアの 1.1 しかないときは、DirectX 12 で起動し直します
5. **vw.exe の起動テスト**: 起動して 10 秒待ち、すぐ終わったら終了コードから原因を推します。
   vw.exe の横に置かれた古い DLL や、インターネットから来た印（SmartScreen の対象）も見ます
6. **ログ**: 今回の起動で `%APPDATA%\vw\logs` にログができたか、できたなら末尾 30 行
7. **イベントログ**: 直近 3 日の vw.exe のアプリケーション エラー

最後に「まとめ」として、見つかった問題と直し方を番号付きで出します。

## 使い方

1. 展開した vw のフォルダの中の `check` フォルダを開きます
2. `check-vw.bat` をダブルクリックします
3. 黒い窓に結果が出て、同じフォルダに `vw-check-report.txt` ができます。中身をそのまま貼って送ってください

報告は人に送れるよう伏せ字にしてあります。

- ユーザー名は `<ユーザー>`、PC 名は `<PC>` に置き換えます
- 個人のフォルダは `%USERPROFILE%` や `%APPDATA%` のように環境変数の名前で書きます
- ログとイベントログに出るファイルやフォルダの場所は、まるごと `<パス>` にします（フォルダ名に人の名前などが入りうるため）

伏せないのは、Windows の版、PC の機種名、GPU とドライバの名前、DLL の版、vw.exe の版と終了コードです。

PowerShell から直接動かすときは:

```powershell
powershell -ExecutionPolicy Bypass -File check-vw.ps1                       # vw.exe を自動で探す
powershell -ExecutionPolicy Bypass -File check-vw.ps1 -VwExe D:\tools\vw\vw.exe
powershell -ExecutionPolicy Bypass -File check-vw.ps1 -NoLaunch             # 起動テストをしない
powershell -ExecutionPolicy Bypass -File check-vw.ps1 -WaitSeconds 20       # 起動を待つ秒数
```

## 終了コードの読み方

| 終了コード | 意味 |
| --- | --- |
| `1` | 描画の準備に失敗した。vw は OpenGL がだめなら wgpu（DirectX 12）で起動し直し、それもだめなら理由を窓に出して終わる |
| `0xC0000135` | 必要な DLL がない（VC++ ランタイム x64 が入っていない） |
| `0xC0000139` / `0xC000007B` | 古い DLL や 32 ビットの DLL が先に読まれた |
| `0xC0000409` | vw の中で異常が起きた。ログに理由が残る |
| `0xC0000005` | 不正なメモリアクセス。GPU ドライバか古い `msvcp140.dll` |

## OpenGL が足りなかったとき

- GPU メーカー（Intel / AMD / NVIDIA）のサイトか Windows Update から、その GPU のドライバを入れます
- リモートデスクトップ中なら、PC の前で直接起動します
- 仮想マシンなら、3D アクセラレーションを有効にします

### Mesa で試す（ドライバを入れられないとき）

vw は OpenGL が足りないと自動で DirectX 12 で起動し直すので、ふつうはこの手順は要りません。
DirectX 12 も使えない環境でだけ試してください。

ソフトウェアで OpenGL を描く Mesa の DLL を vw.exe の横に置くと、GPU がなくても動くはずです（遅くはなります）。
原因が OpenGL かどうかの切り分けにも使えます。

1. https://github.com/pal1000/mesa-dist-win/releases から `mesa3d-…-release-msvc.7z` を取ってきます（展開に 7-Zip が要ります）
2. 中の `x64` フォルダから `opengl32.dll` と `libgallium_wgl.dll`（あれば `libglapi.dll` も）を vw.exe と同じフォルダへ置きます
3. vw.exe を起動します。動いたら原因は OpenGL です。試し終わったらこれらの DLL は消してかまいません
