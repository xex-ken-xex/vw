<#
.SYNOPSIS
  Microsoft Store に出す vw の MSIX を作る。使い方は同じフォルダの README.md。
#>
param(
    # vw.exe・vwbatch.exe と同梱物が入ったフォルダ（CI の dist/vw）
    [Parameter(Mandatory)] [string] $Source,
    [Parameter(Mandatory)] [string] $OutFile,
    # Cargo.toml の版（0.8.2 の形）。Store は 4 つ目を 0 にする決まりなので、末尾に .0 を足す
    [Parameter(Mandatory)] [string] $Version,
    # 以下 4 つは Partner Center の「製品 ID」ページの値。登録前は仮の値のまま作れる（Store には出せない）
    [string] $IdentityName = 'vw.unregistered',
    [string] $Publisher = 'CN=vw-unregistered',
    [string] $PublisherDisplayName = 'xex',
    [string] $DisplayName = 'vw'
)
$ErrorActionPreference = 'Stop'

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "版は 0.8.2 の形で渡してください（$Version）" }

# makeappx は Windows SDK に入っている。版ごとのフォルダに分かれるので、いちばん新しいものを使う
$makeappx = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" -ErrorAction SilentlyContinue |
    Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
    Select-Object -First 1
if (-not $makeappx) { throw 'makeappx.exe が見つかりません。Windows SDK を入れてください' }

$here = $PSScriptRoot
$stage = Join-Path ([IO.Path]::GetTempPath()) "vw-msix-$PID"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory $stage | Out-Null
try {
    Copy-Item -Recurse (Join-Path $Source '*') $stage
    Copy-Item -Recurse (Join-Path $here 'Assets') $stage

    $escape = { param($s) [Security.SecurityElement]::Escape($s) }
    $manifest = (Get-Content -Raw -Encoding UTF8 (Join-Path $here 'AppxManifest.xml')).
        Replace('{{IDENTITY_NAME}}', (& $escape $IdentityName)).
        Replace('{{PUBLISHER}}', (& $escape $Publisher)).
        Replace('{{PUBLISHER_DISPLAY_NAME}}', (& $escape $PublisherDisplayName)).
        Replace('{{DISPLAY_NAME}}', (& $escape $DisplayName)).
        Replace('{{VERSION}}', "$Version.0")
    if ($manifest -match '\{\{\w+\}\}') { throw "埋めていない値が残っています: $($Matches[0])" }
    [IO.File]::WriteAllText((Join-Path $stage 'AppxManifest.xml'), $manifest, [Text.UTF8Encoding]::new($false))

    & $makeappx.FullName pack /o /d $stage /p $OutFile
    if ($LASTEXITCODE -ne 0) { throw "makeappx が失敗しました（終了コード $LASTEXITCODE）" }
    Write-Host "作成: $OutFile（$makeappx）"
}
finally {
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}
