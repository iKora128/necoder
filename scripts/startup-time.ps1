# 起動時間計測（cold start → 初回描画）の Windows 版（WINDOWS-PORT.md §W5）。
#
# 予算: Zed 比 ~80%（docs/ARCHITECTURE §8 / CLAUDE.md 性能予算）。
# アプリは `NECODER_STARTUP_LOG=1` のとき初回 render で `startup_ms=<n>` を stdout に出す。
#
# 使い方: scripts/startup-time.ps1 [-Runs 5]
#
# **mac の数値と直接は比べないこと**。DirectX/DirectWrite 経路は Metal/font-kit 経路と
# 別物で、フォントのラスタライズ方式も違う。予算はプラットフォームごとに持つ。

[CmdletBinding()]
param(
    [int]$Runs = 5
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path (Get-Location) 'target-win'
}

Write-Host "リリースビルド中..."
# **`2>&1` を使ってはいけない**（2026-08-23 に実際に踏んだ）。
# Windows PowerShell 5.1 はネイティブ exe の stderr を 1 行ずつ ErrorRecord に包むため、
# cargo が終了コード 0 で成功していても `NativeCommandError` で落ちる。
# cargo の進捗は stderr に出るので、素通しでコンソールに流す。
& cargo build --release -p necoder | Out-Null
if ($LASTEXITCODE -ne 0) { throw "リリースビルドに失敗しました" }

$binary = Join-Path $env:CARGO_TARGET_DIR 'release\necoder.exe'
if (-not (Test-Path $binary)) { throw "バイナリが見つかりません: $binary" }

# 計測用のプローブファイル（毎回同じ内容＝計測条件を揃える）。
$probe = Join-Path ([System.IO.Path]::GetTempPath()) 'necoder-startup-probe.txt'
$body = (1..200) -join ' '
Set-Content -Path $probe -Value "necoder 起動計測用プローブ`n$body" -Encoding utf8

# `Start-Process -Environment` は PowerShell 7.4 以降にしか無い（Windows PowerShell 5.1 では
# パラメータエラーになる）。5.1 でも動くよう、呼び出し側のプロセス環境に置いて子へ継承させる。
$env:NECODER_STARTUP_LOG = '1'
$env:NECODER_NO_UPDATE_CHECK = '1'

function Measure-Once {
    $log = [System.IO.Path]::GetTempFileName()
    $process = Start-Process -FilePath $binary -ArgumentList $probe `
        -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
    try {
        # 初回描画のログ行が出るまで待つ（最大 ~10s）
        $milliseconds = $null
        for ($waited = 0; $waited -lt 400; $waited++) {
            $match = Select-String -Path $log -Pattern 'startup_ms=([0-9.]+)' -ErrorAction SilentlyContinue |
                Select-Object -First 1
            if ($match) {
                $milliseconds = $match.Matches[0].Groups[1].Value
                break
            }
            Start-Sleep -Milliseconds 25
        }
        return $milliseconds
    } finally {
        if (-not $process.HasExited) { $process.Kill() }
        Remove-Item $log -ErrorAction SilentlyContinue
    }
}

Write-Host "起動時間 (cold start -> first render, $Runs 回):"
$samples = New-Object System.Collections.Generic.List[double]
for ($run = 1; $run -le $Runs; $run++) {
    $milliseconds = Measure-Once
    if ($milliseconds) {
        Write-Host ("  run {0}: {1} ms" -f $run, $milliseconds)
        $samples.Add([double]$milliseconds)
    } else {
        Write-Host ("  run {0}: (計測失敗)" -f $run)
    }
}

Remove-Item Env:\NECODER_STARTUP_LOG -ErrorAction SilentlyContinue
Remove-Item Env:\NECODER_NO_UPDATE_CHECK -ErrorAction SilentlyContinue

if ($samples.Count -gt 0) {
    $average = ($samples | Measure-Object -Average).Average
    Write-Host ("平均: {0:N1} ms ({1}/{2} 回成功)" -f $average, $samples.Count, $Runs)
} else {
    Write-Host "全て計測に失敗しました（NECODER_STARTUP_LOG の出力先を確認すること）"
}
