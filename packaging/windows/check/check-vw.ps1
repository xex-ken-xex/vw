# vw.exe が起動しない Windows で、原因になりやすいところを順に調べて報告にまとめる。
#
#   powershell -ExecutionPolicy Bypass -File check-vw.ps1                     # 同じフォルダか「ダウンロード」の vw.exe を探して試す
#   powershell -ExecutionPolicy Bypass -File check-vw.ps1 -VwExe D:\vw\vw.exe # vw.exe の場所を指定する
#   powershell -ExecutionPolicy Bypass -File check-vw.ps1 -NoLaunch           # vw.exe を起動せずに調べるだけ
#
# 同じフォルダの check-vw.bat をダブルクリックしても同じことができる。
# 何も入れず、何も書き換えない。報告はこのスクリプトと同じフォルダの vw-check-report.txt に書く。
param(
    [string]$VwExe = '',
    [switch]$NoLaunch,
    [int]$WaitSeconds = 10
)

$ErrorActionPreference = 'Continue'
$report = New-Object System.Collections.Generic.List[string]
$findings = New-Object System.Collections.Generic.List[string]

# 報告は人に送るものなので、ユーザー名・PC 名・個人のフォルダの場所を伏せる。
$masks = @()
foreach ($name in 'LOCALAPPDATA', 'APPDATA', 'USERPROFILE') {
    $value = [Environment]::GetEnvironmentVariable($name)
    if ($value) { $masks += , @($value, "%$name%") }
}
function Anonymize([string]$line) {
    foreach ($m in $masks) { $line = $line -replace [regex]::Escape($m[0]), $m[1] }
    foreach ($pair in @(@($env:USERNAME, '<ユーザー>'), @($env:COMPUTERNAME, '<PC>'))) {
        # 短い名前を伏せると、関係ない単語まで消えて報告が読めなくなる。
        if ($pair[0] -and $pair[0].Length -ge 3) {
            $line = $line -replace "\b$([regex]::Escape($pair[0]))\b", $pair[1]
        }
    }
    return $line
}
# ログやイベントログには開いた画像の場所が出る。フォルダ名に患者名などが入りうるので、場所はまるごと伏せる。
function MaskPaths([string]$line) {
    # フォルダ名には空白が入りうるので、最後の \ までは空白ごと伏せる（伏せすぎるのはかまわない）。
    $line = $line -replace '(?i)\b[a-z]:\\(?:[^\\\r\n"<>|]*\\)*[^\\\s"<>|]*', '<パス>'
    return $line -replace '\\\\(?:[^\\\r\n"<>|]*\\)*[^\\\s"<>|]*', '<パス>'
}
function Say([string]$line = '') {
    $line = Anonymize $line
    Write-Host $line
    $report.Add($line)
}
function Finding([string]$line) {
    $line = Anonymize $line
    $findings.Add($line)
    Write-Host "  !! $line" -ForegroundColor Yellow
    $report.Add("  !! $line")
}
function Section([string]$title) {
    Say ''
    Say "== $title =="
}
function FileVersionOf([string]$path) {
    if (-not (Test-Path $path)) { return $null }
    $v = (Get-Item $path).VersionInfo
    return [version]("{0}.{1}.{2}.{3}" -f $v.FileMajorPart, $v.FileMinorPart, $v.FileBuildPart, $v.FilePrivatePart)
}

Say "vw 起動チェック  $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"

# ---------------------------------------------------------------- OS
Section 'Windows の版'
$os = Get-CimInstance Win32_OperatingSystem
$cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$display = $cv.DisplayVersion
if (-not $display) { $display = $cv.ReleaseId }
Say "  $($os.Caption)  版 $display  ビルド $($cv.CurrentBuildNumber).$($cv.UBR)"
Say "  OS のビット数: $($os.OSArchitecture)   CPU: $env:PROCESSOR_ARCHITECTURE"
if (-not [Environment]::Is64BitOperatingSystem) {
    Finding '32 ビット版の Windows です。vw.exe は 64 ビット (x64) 専用なので、この PC では動きません。'
}
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') {
    Finding 'ARM 版の Windows です。Windows 10 の ARM 版は x64 のアプリを動かせません（Windows 11 なら動きます）。'
}
if ([int]$cv.CurrentBuildNumber -lt 14393) {
    Finding "ビルド $($cv.CurrentBuildNumber) は Windows 10 の最初期 (1507/1511) です。1607 (14393) 以降を勧めます。"
}

$cs = Get-CimInstance Win32_ComputerSystem
Say "  機種: $($cs.Manufacturer) $($cs.Model)"
if ("$($cs.Manufacturer) $($cs.Model)" -match 'VMware|VirtualBox|Virtual Machine|QEMU|KVM|Hyper-V|Parallels|Xen') {
    Finding '仮想マシンの中です。3D 描画（OpenGL）が使えない設定のことが多いので、下の OpenGL の結果を見てください。'
}
if ($env:SESSIONNAME -like 'RDP*') {
    Finding 'リモートデスクトップで接続しています。古い Windows 10 では、リモートデスクトップ中は OpenGL 1.1（ソフトウェア）しか使えず vw が起動できないことがあります。PC の前で直接試してください。'
}

# ---------------------------------------------------------------- VC ランタイム
Section 'Visual C++ ランタイム (64 ビット)'
# vw.exe が読み込む DLL は VCRUNTIME140.dll / VCRUNTIME140_1.dll / MSVCP140.dll と、Windows 10 に標準の UCRT。
$sys = Join-Path $env:WINDIR 'System32'
$need = @(
    @{ Name = 'vcruntime140.dll';   Min = [version]'14.20.0.0' },
    @{ Name = 'vcruntime140_1.dll'; Min = [version]'14.20.0.0' },
    @{ Name = 'msvcp140.dll';       Min = [version]'14.40.0.0' },
    @{ Name = 'ucrtbase.dll';       Min = [version]'10.0.0.0' }
)
foreach ($d in $need) {
    $v = FileVersionOf (Join-Path $sys $d.Name)
    if ($null -eq $v) {
        Say ("  {0,-20} なし" -f $d.Name)
        Finding "$($d.Name) が System32 にありません。64 ビット (x64) 版の VC++ 再頒布可能パッケージ（最新）を入れてください。"
    } else {
        Say ("  {0,-20} {1}" -f $d.Name, $v)
        if ($v -lt $d.Min) {
            Finding "$($d.Name) が古い ($v)。$($d.Min) 以上が要ります。64 ビット (x64) 版の VC++ 再頒布可能パッケージ（最新）を入れ直してください。"
        }
    }
}
$rt = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64' -ErrorAction SilentlyContinue
if ($rt) {
    Say "  登録されている x64 ランタイム: $($rt.Version)"
} else {
    Say '  登録されている x64 ランタイム: なし（x86 版だけ入れた場合もここが「なし」になる）'
}

# ---------------------------------------------------------------- GPU と OpenGL
Section 'グラフィックス'
foreach ($gpu in Get-CimInstance Win32_VideoController) {
    $date = ''
    if ($gpu.DriverDate) { $date = $gpu.DriverDate.ToString('yyyy-MM-dd') }
    Say "  $($gpu.Name)  ドライバ $($gpu.DriverVersion) ($date)"
    if ($gpu.Name -match 'Basic Display|基本ディスプレイ|Basic Render') {
        Finding "「$($gpu.Name)」は Windows の仮のドライバで、OpenGL 2.0 以上がありません。vw は DirectX 12 のソフトウェア描画で起動し直しますが遅くなります。GPU メーカー（Intel / AMD / NVIDIA）のドライバを入れると速くなります。"
    }
}

Section 'OpenGL（vw は OpenGL 2.0 以上で描画し、足りなければ DirectX 12 で起動し直す）'
$probe = @'
using System;
using System.Runtime.InteropServices;
public static class VwGlProbe {
    [StructLayout(LayoutKind.Sequential)]
    public struct PFD {
        public ushort nSize, nVersion; public uint dwFlags;
        public byte iPixelType, cColorBits, cRedBits, cRedShift, cGreenBits, cGreenShift, cBlueBits, cBlueShift,
                    cAlphaBits, cAlphaShift, cAccumBits, cAccumRedBits, cAccumGreenBits, cAccumBlueBits, cAccumAlphaBits,
                    cDepthBits, cStencilBits, cAuxBuffers, iLayerType, bReserved;
        public uint dwLayerMask, dwVisibleMask, dwDamageMask;
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr CreateWindowExW(uint ex, string cls, string name, uint style, int x, int y, int w, int h,
                                         IntPtr parent, IntPtr menu, IntPtr inst, IntPtr param);
    [DllImport("user32.dll")] static extern bool DestroyWindow(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("gdi32.dll")] static extern int ChoosePixelFormat(IntPtr dc, ref PFD pfd);
    [DllImport("gdi32.dll")] static extern bool SetPixelFormat(IntPtr dc, int fmt, ref PFD pfd);
    [DllImport("gdi32.dll")] static extern int DescribePixelFormat(IntPtr dc, int fmt, uint size, ref PFD pfd);
    [DllImport("opengl32.dll")] static extern IntPtr wglCreateContext(IntPtr dc);
    [DllImport("opengl32.dll")] static extern bool wglMakeCurrent(IntPtr dc, IntPtr rc);
    [DllImport("opengl32.dll")] static extern bool wglDeleteContext(IntPtr rc);
    [DllImport("opengl32.dll")] static extern IntPtr glGetString(uint name);

    public static string[] Probe() {
        IntPtr wnd = CreateWindowExW(0, "STATIC", "", 0x80000000, 0, 0, 1, 1, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
        if (wnd == IntPtr.Zero) return new[] { "ERROR", "窓を作れませんでした" };
        IntPtr dc = GetDC(wnd);
        try {
            PFD pfd = new PFD();
            pfd.nSize = (ushort)Marshal.SizeOf(typeof(PFD)); pfd.nVersion = 1;
            pfd.dwFlags = 0x4 | 0x20 | 0x1; pfd.cColorBits = 32; pfd.cDepthBits = 24;
            int fmt = ChoosePixelFormat(dc, ref pfd);
            if (fmt == 0) return new[] { "ERROR", "OpenGL の使える画素形式がありません" };
            PFD got = new PFD(); got.nSize = pfd.nSize;
            DescribePixelFormat(dc, fmt, (uint)Marshal.SizeOf(typeof(PFD)), ref got);
            if (!SetPixelFormat(dc, fmt, ref got)) return new[] { "ERROR", "画素形式を設定できません" };
            IntPtr rc = wglCreateContext(dc);
            if (rc == IntPtr.Zero) return new[] { "ERROR", "OpenGL のコンテキストを作れません" };
            try {
                wglMakeCurrent(dc, rc);
                bool generic = (got.dwFlags & 0x40) != 0;
                return new[] {
                    Marshal.PtrToStringAnsi(glGetString(0x1F02)) ?? "",
                    Marshal.PtrToStringAnsi(glGetString(0x1F01)) ?? "",
                    Marshal.PtrToStringAnsi(glGetString(0x1F00)) ?? "",
                    generic ? "generic" : "hardware"
                };
            } finally { wglMakeCurrent(IntPtr.Zero, IntPtr.Zero); wglDeleteContext(rc); }
        } finally { ReleaseDC(wnd, dc); DestroyWindow(wnd); }
    }
}
'@
try {
    Add-Type -TypeDefinition $probe -ErrorAction Stop
    $gl = [VwGlProbe]::Probe()
    if ($gl[0] -eq 'ERROR') {
        Say "  失敗: $($gl[1])"
        Finding "OpenGL が使えません（$($gl[1])）。vw は DirectX 12 で起動し直します。下の DirectX 12 の結果を見てください。"
    } else {
        Say "  版:       $($gl[0])"
        Say "  レンダラ: $($gl[1])"
        Say "  ベンダ:   $($gl[2])"
        Say "  種類:     $($gl[3])"
        $m = [regex]::Match($gl[0], '^(\d+)\.(\d+)')
        $glVersion = [version]'0.0'
        if ($m.Success) { $glVersion = [version]("$($m.Groups[1].Value).$($m.Groups[2].Value)") }
        if ($glVersion -lt [version]'2.0' -or $gl[1] -match 'GDI Generic') {
            Finding "OpenGL $($gl[0]) ($($gl[1])) しかありません。vw は DirectX 12 で起動し直します。下の DirectX 12 の結果を見てください。"
        }
    }
} catch {
    Say "  調べられませんでした: $($_.Exception.Message)"
}

Section 'DirectX 12（OpenGL が足りないときに vw が使う）'
if (Test-Path (Join-Path $sys 'd3d12.dll')) {
    Say "  d3d12.dll: $(FileVersionOf (Join-Path $sys 'd3d12.dll'))"
} else {
    Say '  d3d12.dll: なし'
    Finding 'DirectX 12 (d3d12.dll) がありません。OpenGL も足りない場合、vw は起動できません。Windows Update で Windows を新しくしてください。'
}

# ---------------------------------------------------------------- vw.exe
Section 'vw.exe'
if (-not $VwExe) {
    # 配布の zip では、このスクリプトは vw.exe の隣の check フォルダに入っている。
    $candidates = @(
        (Join-Path $PSScriptRoot '..\vw.exe'),
        (Join-Path $PSScriptRoot 'vw.exe'),
        (Join-Path $PSScriptRoot 'vw\vw.exe')
    )
    $downloads = Join-Path $env:USERPROFILE 'Downloads'
    if (Test-Path $downloads) {
        $candidates += Get-ChildItem $downloads -Recurse -Filter vw.exe -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | ForEach-Object { $_.FullName }
    }
    $VwExe = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (-not $VwExe -or -not (Test-Path $VwExe)) {
    Say '  vw.exe が見つかりません。-VwExe で場所を指定してください。'
} else {
    $exe = Get-Item $VwExe
    Say "  場所: $($exe.FullName)"
    Say "  版:   $($exe.VersionInfo.ProductVersion)  更新 $($exe.LastWriteTime.ToString('yyyy-MM-dd HH:mm'))"
    if (Get-Item $exe.FullName -Stream Zone.Identifier -ErrorAction SilentlyContinue) {
        Say '  インターネットから来た印 (Zone.Identifier): あり'
        Say '    → 初回は SmartScreen の青い画面が出ることがある。「詳細情報」→「実行」で起動できる。'
    }
    # 「いろいろ入れた」ときに exe の横へ置いた DLL が、System32 の新しい DLL より先に読まれてしまう。
    $local = Get-ChildItem $exe.DirectoryName -Filter *.dll -ErrorAction SilentlyContinue
    if ($local) {
        Say "  vw.exe と同じフォルダの DLL: $(($local | ForEach-Object { $_.Name }) -join ', ')"
        $stray = $local | Where-Object { $_.Name -match '^(vcruntime|msvcp|ucrtbase|api-ms-win)' }
        if ($stray) {
            Finding "vw.exe の横に $(($stray | ForEach-Object { $_.Name }) -join ', ') があります。古い版や 32 ビット版だと起動を妨げるので、別の場所へ移してから試してください。"
        }
    }

    $logDir = Join-Path $env:APPDATA 'vw\logs'
    if (-not $NoLaunch) {
        Say ''
        Say "  vw.exe を起動して $WaitSeconds 秒待ちます..."
        $started = Get-Date
        $p = Start-Process -FilePath $exe.FullName -PassThru
        # Handle を一度読まないと、終わったあと ExitCode が空になる（.NET の Process の癖）。
        $null = $p.Handle
        $null = $p.WaitForExit($WaitSeconds * 1000)
        if (-not $p.HasExited) {
            Say "  $WaitSeconds 秒たっても動いています（起動できています）。窓が見えなければ画面の外に出ている可能性があります。"
        } else {
            $code = $p.ExitCode
            $hex = '0x{0:X8}' -f $code
            Say "  すぐに終わりました。終了コード $code ($hex)"
            switch ($hex) {
                '0x00000001' { Finding '終了コード 1: OpenGL でも DirectX 12 でも描画の準備ができませんでした。上の OpenGL と DirectX 12 の結果を見てください。' }
                '0xC0000135' { Finding '終了コード 0xC0000135: 必要な DLL が見つかりません。VC++ ランタイム (x64) を確認してください。' }
                '0xC0000139' { Finding '終了コード 0xC0000139: DLL はあるが中の関数が足りません。古い DLL が先に読まれている可能性があります。' }
                '0xC000007B' { Finding '終了コード 0xC000007B: 32 ビットの DLL を 64 ビットの vw が読もうとしました。vw.exe の横や PATH 上の DLL を確認してください。' }
                '0xC0000142' { Finding '終了コード 0xC0000142: DLL の初期化に失敗しました。VC++ ランタイム (x64) を入れ直してください。' }
                '0xC0000409' { Finding '終了コード 0xC0000409: vw の中で異常が起きて止まりました。下のログに理由が残っているはずです。' }
                '0xC0000005' { Finding '終了コード 0xC0000005: メモリの不正な読み書きで止まりました。GPU ドライバか古い MSVCP140.dll が疑わしいです。' }
                default { }
            }
        }
        $newLogs = Get-ChildItem $logDir -Filter 'vw-*.log' -ErrorAction SilentlyContinue |
            Where-Object { $_.LastWriteTime -ge $started.AddSeconds(-2) } |
            Sort-Object LastWriteTime | Select-Object -Last 1
        if ($newLogs) {
            Say "  今回のログ: $($newLogs.FullName)"
            Say '  ---- ログの末尾 ----'
            Get-Content $newLogs.FullName -Tail 30 -Encoding UTF8 | ForEach-Object { Say "  | $(MaskPaths $_)" }
        } else {
            Say '  今回のログはできませんでした（画面を作る前に止まった）。'
        }
    }
}

# ---------------------------------------------------------------- イベントログ
Section 'Windows のイベントログ（直近 3 日の vw.exe のエラー）'
$events = Get-WinEvent -FilterHashtable @{ LogName = 'Application'; Id = 1000, 1001, 1026; StartTime = (Get-Date).AddDays(-3) } -ErrorAction SilentlyContinue |
    Where-Object { $_.Message -match 'vw\.exe' } | Select-Object -First 3
if ($events) {
    foreach ($e in $events) {
        Say "  $($e.TimeCreated.ToString('yyyy-MM-dd HH:mm:ss'))  ID $($e.Id)"
        ($e.Message -split "`r?`n" | Select-Object -First 12) | ForEach-Object { Say "    $(MaskPaths $_)" }
    }
} else {
    Say '  なし'
}

# ---------------------------------------------------------------- まとめ
Section 'まとめ'
if ($findings.Count -eq 0) {
    Say '  怪しいところは見つかりませんでした。この報告をそのまま送ってください。'
} else {
    $i = 1
    foreach ($f in $findings) { Say "  $i. $f"; $i++ }
}

$out = Join-Path $PSScriptRoot 'vw-check-report.txt'
$report | Set-Content -Path $out -Encoding UTF8
Write-Host ''
Write-Host "報告を書きました: $out" -ForegroundColor Cyan
