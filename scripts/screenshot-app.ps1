# necoder の UI 検証用スクリーンショット（Windows 版・WINDOWS-PORT.md §W5）
#
# ## なぜ mac 版と方式が違うのか
#
# WINDOWS-PORT.md は当初「ヘッドレススクショ（`--features screenshot` + `NECODER_SCREENSHOT`）は
# OS 非依存だから Windows でもそのまま回る」と書いていたが、**これは誤りだった**。
# 2026-08-22 に実機で試すと `render_to_image not implemented for this platform` で落ちる
# ＝ gpui の `render_to_image` は **macOS にしか実装が無い**。
#
# そこで Windows では**実ウィンドウを Win32 の `PrintWindow` で捕捉する**。
# `PW_RENDERFULLCONTENT`（flags=2）を使うと DirectX で描かれた中身も取れる。
# 画面全体を撮る mac 版（`screencapture`）と違い**necoder の窓だけ**が撮れるので、
# 他のウィンドウが写り込まない・座標に依存しないという利点がある。
#
# 使い方:
#   scripts/screenshot-app.ps1
#   scripts/screenshot-app.ps1 -Out shot.png -ProjectPath C:\path\to\repo -WaitSeconds 5
#
# 出た PNG は **Read して目視で**検証すること（レイアウト崩れ・色・文字化け）。

[CmdletBinding()]
param(
    [string]$Out,
    [string]$ProjectPath,
    # ウィンドウが出てから撮影するまでの待ち（初回描画とフォント読み込みの完了を待つ）
    [int]$WaitSeconds = 4,
    # 撮影後もアプリを起動したままにする（対話で触りたいとき）
    [switch]$KeepRunning
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

# WSL/mac の target/ と奪い合わないよう分ける（WINDOWS-PORT.md §1）。
if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path (Get-Location) 'target-win'
}

if (-not $Out) {
    $stamp = Get-Date -Format 'HHmmss'
    $Out = Join-Path ([System.IO.Path]::GetTempPath()) "necoder-ui-$stamp.png"
}
$Out = [System.IO.Path]::GetFullPath($Out)

Write-Host "ビルド中..."
& cargo build -p necoder
if ($LASTEXITCODE -ne 0) { throw "ビルドに失敗しました" }

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class NecoderWindowCapture {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

# **DPI 認識の宣言は必須**（2026-08-23 に実際に踏んだ）。
#
# このプロセスが DPI 非対応のままだと Windows が座標を仮想化する:
#   - `GetWindowRect` は**論理**サイズ（例 1295x807）を返す
#   - `PrintWindow` は**物理**サイズ（150% なら 1942x1211）で描画する
# ＝ ビットマップが足りず **ウィンドウの左上 2/3 しか写らない**。
# 下ドック（ターミナル）や右ドック（Agent パネル）が丸ごと画面外になり、
# 「機能が動いていない」という誤診に直結する（実際に一度誤診した）。
[void][NecoderWindowCapture]::SetProcessDPIAware()

$binary = Join-Path $env:CARGO_TARGET_DIR 'debug\necoder.exe'
if (-not (Test-Path $binary)) { throw "バイナリが見つかりません: $binary" }

# 更新確認が撮影に混ざらないようにする。
$env:NECODER_NO_UPDATE_CHECK = '1'
$arguments = if ($ProjectPath) { @($ProjectPath) } else { @() }
$process = Start-Process -FilePath $binary -ArgumentList $arguments -PassThru

try {
    # ウィンドウハンドルが生えるまで待つ（最大 30s）
    $handle = [IntPtr]::Zero
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        Start-Sleep -Milliseconds 500
        $process.Refresh()
        if ($process.HasExited) {
            throw "necoder が起動直後に終了しました（exit $($process.ExitCode)）"
        }
        if ($process.MainWindowHandle -ne [IntPtr]::Zero) {
            $handle = $process.MainWindowHandle
            break
        }
    }
    if ($handle -eq [IntPtr]::Zero) { throw "ウィンドウが出ませんでした（30s 待機）" }

    # 初回描画・フォント読み込みの完了を待つ
    Start-Sleep -Seconds $WaitSeconds

    $rect = New-Object NecoderWindowCapture+RECT
    [void][NecoderWindowCapture]::GetWindowRect($handle, [ref]$rect)
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    if ($width -le 0 -or $height -le 0) { throw "ウィンドウのサイズが取れません（${width}x${height}）" }

    $bitmap = New-Object System.Drawing.Bitmap $width, $height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $deviceContext = $graphics.GetHdc()
    # flags=2 = PW_RENDERFULLCONTENT。DirectX で描かれた中身を取るために必要。
    $captured = [NecoderWindowCapture]::PrintWindow($handle, $deviceContext, 2)
    $graphics.ReleaseHdc($deviceContext)
    $graphics.Dispose()
    if (-not $captured) { $bitmap.Dispose(); throw "PrintWindow に失敗しました" }

    $bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()

    $sizeKb = [math]::Round((Get-Item $Out).Length / 1KB, 1)
    Write-Host "saved: $Out (${width}x${height}, $sizeKb KB)"
    Write-Host "→ この PNG を Read して目視で検証すること（レイアウト崩れ・色・文字化け）"
} finally {
    if ($KeepRunning) {
        Write-Host "アプリは起動したまま（PID $($process.Id)）— 終わったら: Stop-Process -Id $($process.Id)"
    } elseif (-not $process.HasExited) {
        $process.Kill()
    }
    Remove-Item Env:\NECODER_NO_UPDATE_CHECK -ErrorAction SilentlyContinue
}
