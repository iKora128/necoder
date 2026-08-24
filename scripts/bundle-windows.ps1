# necoder-windows-x64.zip を作る（WINDOWS-PORT.md §W6・配布用）。
#
# ## なぜ静的 CRT なのか（**ここが本質**）
#
# 素の `cargo build --release` で作った `necoder.exe` は **`VCRUNTIME140.dll` /
# `VCRUNTIME140_1.dll` に依存する**（2026-08-24 に `dumpbin /DEPENDENTS` で確認）。
# これは **Visual C++ 再頒布可能パッケージ**に入っている DLL で、**まっさらな Windows には無い**。
# そのまま配ると「VCRUNTIME140.dll が見つかりません」で起動できない人が出る。
#
# `-C target-feature=+crt-static` で CRT を静的リンクすると、**exe 1 個で完結**する。
# zip を解凍して起動するだけ、という配布形態にはこれが正しい。
# （`api-ms-win-crt-*` は Universal CRT ＝ Windows 10 以降に標準搭載なので問題ない。）
#
# ## 使い方
#
#   scripts/bundle-windows.ps1              # zip まで作る
#   scripts/bundle-windows.ps1 -SkipBuild   # ビルド済みを使って zip だけ作り直す
#
# 出力: dist/necoder-windows-x64.zip
#
# ## まだやっていないこと
#
# - **コード署名なし**。初回起動で SmartScreen の警告が出る（README に説明が要る・§D7）。
#   Authenticode 署名は証明書を買ってから。OV でも評判は時間で育つ

[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [string]$OutDir
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

# 静的 CRT はフラグが変わる＝全再ビルドになるので、通常の target とは別に持つ。
$targetDir = Join-Path (Get-Location) 'target-win-dist'
if (-not $OutDir) { $OutDir = Join-Path (Get-Location) 'dist' }

# バージョンの唯一の出所 = workspace の Cargo.toml（bundle-mac.sh と同じ流儀）。
$version = (Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "necoder $version の Windows 配布物を作ります"

if (-not $SkipBuild) {
    Write-Host "静的 CRT でリリースビルド中（全再ビルドなので時間がかかります）..."
    $env:CARGO_TARGET_DIR = $targetDir
    # `2>&1` は付けない。Windows PowerShell 5.1 が exe の stderr を ErrorRecord に包んで
    # 終了コード 0 でも落ちるため（WINDOWS-PORT.md §4）。
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    & cargo build --release -p necoder
    $buildExit = $LASTEXITCODE
    Remove-Item Env:\RUSTFLAGS -ErrorAction SilentlyContinue
    if ($buildExit -ne 0) { throw "リリースビルドに失敗しました" }
}

$exe = Join-Path $targetDir 'release\necoder.exe'
if (-not (Test-Path $exe)) { throw "バイナリが見つかりません: $exe" }

# **配る前に依存を検証する。** VCRUNTIME が残っていたら静的リンクが効いていない＝
# 配布先で起動できない。ここで気づけないと「手元では動くのに」で終わる。
$dumpbin = Get-ChildItem 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC' `
    -Recurse -Filter dumpbin.exe -ErrorAction SilentlyContinue | Select-Object -First 1
if ($dumpbin) {
    $deps = & $dumpbin.FullName /DEPENDENTS $exe |
        Select-String -Pattern '^\s+(\S+\.dll)' |
        ForEach-Object { $_.Matches[0].Groups[1].Value }
    $vcruntime = $deps | Where-Object { $_ -match '^(VCRUNTIME|MSVCP)' }
    if ($vcruntime) {
        throw "静的 CRT が効いていません（$($vcruntime -join ', ') に依存）。RUSTFLAGS を確認すること"
    }
    Write-Host "依存 DLL: $($deps.Count) 個 — VCRUNTIME 依存なし（OK）"
} else {
    Write-Warning "dumpbin が見つからないため依存 DLL を検証できませんでした"
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$stage = Join-Path $OutDir "necoder-$version-windows-x64"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

Copy-Item $exe (Join-Path $stage 'necoder.exe')
Copy-Item 'LICENSE' $stage -ErrorAction SilentlyContinue
Copy-Item 'README.md' $stage -ErrorAction SilentlyContinue

$zip = Join-Path $OutDir 'necoder-windows-x64.zip'
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip

$sizeMb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
$exeMb = [math]::Round((Get-Item $exe).Length / 1MB, 1)
Write-Host ""
Write-Host "できました: $zip ($sizeMb MB / exe $exeMb MB)"
Write-Host "  ※ 未署名なので初回起動で SmartScreen の警告が出ます（README に説明が要る）"
