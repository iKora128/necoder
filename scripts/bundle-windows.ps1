# necoder の Windows 配布物を作る（WINDOWS-PORT.md §W6 / §W7）。
#
#   dist/necoder-windows-x64-setup.exe   インストーラ（ふつうの人はこちら）
#   dist/necoder-windows-x64.zip         ポータブル版（解凍してそのまま動く）
#
# 2 つは**同じ中身のフォルダ（stage）から作る**＝インストーラで入る物と zip の中身は常に一致する。
# release.yml の windows ジョブもこのスクリプトを呼ぶ（手元と CI で並べ方が分かれないように）。
#
# ## なぜ静的 CRT なのか（**ここが本質**）
#
# 素の `cargo build --release` で作った `necoder.exe` は **`VCRUNTIME140.dll` /
# `VCRUNTIME140_1.dll` に依存する**（2026-08-24 に `dumpbin /DEPENDENTS` で確認）。
# これは **Visual C++ 再頒布可能パッケージ**に入っている DLL で、**まっさらな Windows には無い**。
# そのまま配ると「VCRUNTIME140.dll が見つかりません」で起動できない人が出る。
#
# `-C target-feature=+crt-static` で CRT を静的リンクすると、**exe 1 個で完結**する。
# （`api-ms-win-crt-*` は Universal CRT ＝ Windows 10 以降に標準搭載なので問題ない。）
#
# ## 使い方
#
#   scripts/bundle-windows.ps1                    # ビルドから全部
#   scripts/bundle-windows.ps1 -SkipBuild         # ビルド済みを使って並べ直すだけ
#   scripts/bundle-windows.ps1 -SkipBuild -SkipRelay -TargetDir target   # CI（release.yml）
#
# インストーラには Inno Setup 6 が要る（`winget install -e --id JRSoftware.InnoSetup`）。
# 見つからなければ zip だけ作って警告する（-RequireInstaller で失敗にする・CI はこちら）。
#
# ## まだやっていないこと
#
# - **コード署名なし**。ブラウザで落とした exe は初回に SmartScreen の青い画面が出る（README 参照）。
#   `irm https://necoder.com/install.ps1 | iex` で入れた場合は出ない（ダウンロードの印が付かないため）

[CmdletBinding()]
param(
    [switch]$SkipBuild,
    # relay（Remote control の host.mjs）のビルドを飛ばす。CI は前のステップで済ませている。
    [switch]$SkipRelay,
    # インストーラを作れなかったら失敗にする（CI 用）。
    [switch]$RequireInstaller,
    # cargo の target ディレクトリ（既定は静的 CRT 専用の target-win-dist）。
    [string]$TargetDir,
    [string]$OutDir
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$root = (Get-Location).Path

# 相対パスはリポジトリの根から読む（絶対パスはそのまま）。
function Resolve-FromRoot([string]$Path) {
    if ([System.IO.Path]::IsPathRooted($Path)) { return [System.IO.Path]::GetFullPath($Path) }
    return [System.IO.Path]::GetFullPath((Join-Path $root $Path))
}

# 静的 CRT はフラグが変わる＝全再ビルドになるので、通常の target とは別に持つ。
if (-not $TargetDir) { $TargetDir = 'target-win-dist' }
$TargetDir = Resolve-FromRoot $TargetDir
if (-not $OutDir) { $OutDir = 'dist' }
$OutDir = Resolve-FromRoot $OutDir

# バージョンの唯一の出所 = workspace の Cargo.toml（bundle-mac.sh と同じ流儀）。
$version = (Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "necoder $version の Windows 配布物を作ります"

if (-not $SkipBuild) {
    Write-Host "静的 CRT でリリースビルド中（全再ビルドなので時間がかかります）..."
    $env:CARGO_TARGET_DIR = $TargetDir
    # `2>&1` は付けない。Windows PowerShell 5.1 が exe の stderr を ErrorRecord に包んで
    # 終了コード 0 でも落ちるため（WINDOWS-PORT.md §4）。
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    & cargo build --release -p necoder
    $buildExit = $LASTEXITCODE
    Remove-Item Env:\RUSTFLAGS -ErrorAction SilentlyContinue
    if ($buildExit -ne 0) { throw "リリースビルドに失敗しました" }
}

$exe = Join-Path $TargetDir 'release\necoder.exe'
if (-not (Test-Path $exe)) { throw "バイナリが見つかりません: $exe" }

# **配る前に依存を検証する。** VCRUNTIME が残っていたら静的リンクが効いていない＝配布先で起動できない。
# PE の import ディレクトリには DLL 名が ASCII のまま載るので、外部ツール無しでバイト走査できる
# （dumpbin は見つからない・古い版を掴んで黙って素通りする、を CI で二度踏んだ・release.yml 参照）。
$text = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($exe))
if ($text -notmatch 'KERNEL32\.dll') { throw "PE から KERNEL32.dll すら見つからない＝走査が機能していない" }
$vcruntime = @([regex]::Matches($text, '(VCRUNTIME|MSVCP)\d*[^\x00]*\.dll') |
    ForEach-Object { $_.Value } | Sort-Object -Unique)
if ($vcruntime) {
    throw "静的 CRT が効いていません（$($vcruntime -join ', ') に依存）。RUSTFLAGS を確認すること"
}
Write-Host "VCRUNTIME / MSVCP 依存なし（OK）"

if (-not $SkipRelay) {
    Push-Location 'relay'
    try {
        & npm.cmd ci
        if ($LASTEXITCODE -ne 0) { throw 'Remote host の依存インストールに失敗しました' }
        & npm.cmd run build
        if ($LASTEXITCODE -ne 0) { throw 'Remote host のビルドに失敗しました' }
    } finally { Pop-Location }
}
foreach ($built in 'relay/dist/host.mjs', 'relay/dist/THIRD_PARTY_LICENSES.txt') {
    if (-not (Test-Path $built)) { throw "$built がありません（relay をビルドすること）" }
}

# ── stage: zip とインストーラの共通の中身 ──
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$stage = Join-Path $OutDir 'stage'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

Copy-Item $exe (Join-Path $stage 'necoder.exe')
Copy-Item 'scripts/windows/README-Windows.txt' $stage
Copy-Item 'LICENSE', 'THIRD_PARTY_NOTICES.md' $stage
Copy-Item 'third_party/licenses' (Join-Path $stage 'licenses') -Recurse
New-Item -ItemType Directory -Path (Join-Path $stage 'control') -Force | Out-Null
Copy-Item 'relay/dist/host.mjs', 'relay/dist/THIRD_PARTY_LICENSES.txt' (Join-Path $stage 'control')

# ── ポータブル版 zip ──
$zip = Join-Path $OutDir 'necoder-windows-x64.zip'
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
Write-Host "zip: $zip ($([math]::Round((Get-Item $zip).Length / 1MB, 1)) MB)"

# ── インストーラ ──
$iscc = @(
    (Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1),
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
$setup = Join-Path $OutDir 'necoder-windows-x64-setup.exe'
if ($iscc) {
    Write-Host "インストーラを作成中（$iscc）..."
    & $iscc /Q "/DAppVersion=$version" "/DStageDir=$stage" "/DOutputDir=$OutDir" 'scripts\windows\necoder.iss'
    if ($LASTEXITCODE -ne 0) { throw "インストーラの作成に失敗しました（ISCC exit $LASTEXITCODE）" }
    Write-Host "setup: $setup ($([math]::Round((Get-Item $setup).Length / 1MB, 1)) MB)"
} elseif ($RequireInstaller) {
    throw "Inno Setup 6（ISCC.exe）が見つかりません"
} else {
    Write-Warning "Inno Setup 6 が見つからないためインストーラは作りませんでした（winget install -e --id JRSoftware.InnoSetup）"
}

Write-Host ""
Write-Host "できました（necoder $version）"
Write-Host "  ※ 未署名なので、ブラウザで落とした exe は初回に SmartScreen の警告が出ます"
