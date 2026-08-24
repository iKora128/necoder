# idle メモリ実測の Windows 版（WINDOWS-PORT.md §W5）。
#
# release バイナリでファイルを 1 枚開いて idle 数秒 → ワーキングセットを読む。
# 用途は mac 版と同じ（terminal-stack-2026 §4「RAM で買う層への武器」）。
#
# 使い方: scripts/memory-usage.ps1 [-WaitSeconds 6]
#
# **mac の RSS と直接は比べないこと**。Windows のワーキングセット（WorkingSet64）は
# unix の RSS と定義が近いが同一ではなく、DirectX ランタイムが載る分の下駄もある。
# PrivateMemorySize64（そのプロセス専用のコミット量）も併記するので、共有分を除いた
# 比較はそちらを見る。

[CmdletBinding()]
param(
    [int]$WaitSeconds = 6
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path (Get-Location) 'target-win'
}

$binary = Join-Path $env:CARGO_TARGET_DIR 'release\necoder.exe'
if (-not (Test-Path $binary)) {
    throw "先に: cargo build --release -p necoder（見つからない: $binary）"
}

$probe = Join-Path ([System.IO.Path]::GetTempPath()) 'necoder-memory-probe.rs'
Set-Content -Path $probe -Value "fn main() {`n    println!(`"hello`");`n}" -Encoding utf8

# `Start-Process -Environment` は PowerShell 7.4 以降にしか無い（5.1 ではパラメータエラー）。
# 呼び出し側のプロセス環境に置いて子へ継承させる。
$env:NECODER_NO_UPDATE_CHECK = '1'
$process = Start-Process -FilePath $binary -ArgumentList $probe -PassThru
try {
    Start-Sleep -Seconds $WaitSeconds
    $process.Refresh()
    if ($process.HasExited) { throw "necoder が計測前に終了しました（exit $($process.ExitCode)）" }
    $workingSetMb = [math]::Round($process.WorkingSet64 / 1MB, 1)
    $privateMb = [math]::Round($process.PrivateMemorySize64 / 1MB, 1)
    Write-Host ("idle ワーキングセット: {0} MB / private commit: {1} MB（起動 {2}s 後・{3} を 1 枚表示）" `
        -f $workingSetMb, $privateMb, $WaitSeconds, $probe)
} finally {
    if (-not $process.HasExited) { $process.Kill() }
    Remove-Item Env:\NECODER_NO_UPDATE_CHECK -ErrorAction SilentlyContinue
}
