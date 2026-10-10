# vw の開発環境を Windows に揃える。
#
#   powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1           # 環境を揃えてビルドとテストまで
#   powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1 -NoBuild  # 環境だけ揃える
#
# 同じフォルダの setup-windows.bat をダブルクリックしても同じことができる。
# winget（Windows 10 1809 以降、Windows 11 に標準）で次を入れる。入っているものは飛ばす。
#   - Visual Studio 2022 Build Tools（C++ ビルドツールと Windows SDK）: Rust の MSVC 版がリンカとして使う
#   - Git
#   - rustup（Rust 本体）
param([switch]$NoBuild)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

function Test-Command($name) { [bool](Get-Command $name -ErrorAction SilentlyContinue) }

function Update-Path {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
                [Environment]::GetEnvironmentVariable('Path', 'User') + ';' +
                (Join-Path $env:USERPROFILE '.cargo\bin')
}

function Install-WithWinget($id, $extra = @()) {
    $listed = winget list --id $id --exact --accept-source-agreements 2>$null | Out-String
    if ($listed -match [regex]::Escape($id)) {
        Write-Host "    $id は導入済み"
        return
    }
    Write-Host "    $id を導入します"
    winget install --id $id --exact --silent --accept-package-agreements --accept-source-agreements @extra
    if ($LASTEXITCODE -ne 0) { throw "$id の導入に失敗しました (winget 終了コード $LASTEXITCODE)" }
}

if (-not (Test-Command winget)) {
    throw 'winget が見つかりません。Microsoft Store で「アプリ インストーラー」を入れてから再実行してください。'
}

Write-Host '==> Visual Studio Build Tools (C++)'
# GNU 版 (mingw) の Rust にしないのは、Windows 向けの依存クレートの多くが MSVC 版を前提に
# 検証されており、CI でも MSVC 版でビルドしているため。
Install-WithWinget 'Microsoft.VisualStudio.2022.BuildTools' @(
    '--override',
    '--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
)

Write-Host '==> Git'
Install-WithWinget 'Git.Git'

Write-Host '==> Rust'
Install-WithWinget 'Rustlang.Rustup'
Update-Path
rustup toolchain install stable-msvc --profile minimal --component clippy rustfmt
rustup default stable-msvc
rustc --version

if (-not $NoBuild) {
    Write-Host '==> ビルドとテスト'
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'ビルドに失敗しました' }
    cargo test
    if ($LASTEXITCODE -ne 0) { throw 'テストに失敗しました' }
    Write-Host ''
    Write-Host '完了: target\release\vw.exe で起動できます'
} else {
    Write-Host ''
    Write-Host '完了: cargo build --release でビルドできます'
}
Write-Host '新しく開いた端末でないと cargo が見つからないことがあります。その場合は端末を開き直してください。'
