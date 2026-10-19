# necoder の Windows アイコン（.ico）を作る（WINDOWS-PORT.md §W7）。
#
# 原画は mac と同じ `crates/necoder/assets/icon/necoder.png`（make-icon.py が作る角丸の 1024px）。
# エクスプローラ・タスクバー・スタートメニュー・Alt+Tab が使う 7 サイズを 1 つの .ico に詰める。
#
# ## なぜ PowerShell なのか
#
# mac 版の make-icon.py は Pillow を使うが、まっさらな Windows に Python は無い
# （`python.exe` は Microsoft Store へ飛ばすだけのスタブ）。System.Drawing なら Windows に標準で在る。
#
# ## 縮め方（pixel art 前提・make-icon.py と同じ考え方）
#
# 原画のドットは 1024px で 4px 四方。1024 を割り切るサイズ（256 / 128 / 64 / 32 / 16）は
# **最近傍**で縮める＝ドットをそのまま拾うので輪郭が溶けない。割り切れない 48 / 24 だけは
# 高品質バイキュービック（最近傍だとドットの幅が揃わず歪む）。
#
# 各サイズは PNG のまま .ico に入れる（Windows Vista 以降はこれを読める・BMP より小さい）。
#
# 使い方:
#   scripts/make-icon-windows.ps1
# 出力: crates/necoder/assets/icon/necoder.ico（コミットする・build.rs が exe に埋め込む）

[CmdletBinding()]
param(
    [string]$Source,
    [string]$Out
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
if (-not $Source) { $Source = 'crates/necoder/assets/icon/necoder.png' }
if (-not $Out) { $Out = 'crates/necoder/assets/icon/necoder.ico' }
$Source = [System.IO.Path]::GetFullPath($Source)
$Out = [System.IO.Path]::GetFullPath($Out)

Add-Type -AssemblyName System.Drawing

$sizes = @(256, 128, 64, 48, 32, 24, 16)
$master = [System.Drawing.Image]::FromFile($Source)
try {
    $images = foreach ($size in $sizes) {
        $bitmap = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
        if ($master.Width % $size -eq 0) {
            $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
            # 既定の画素中心ずれで端の 1 列が欠けるのを防ぐ。
            $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
        } else {
            $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        }
        $graphics.DrawImage($master, 0, 0, $size, $size)
        $graphics.Dispose()
        $stream = New-Object System.IO.MemoryStream
        $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
        $bitmap.Dispose()
        , $stream.ToArray()
        $stream.Dispose()
    }
} finally {
    $master.Dispose()
}

# ICONDIR（6 バイト）+ ICONDIRENTRY（16 バイト × 枚数）+ 各 PNG。
$file = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter $file
$writer.Write([UInt16]0)               # 予約
$writer.Write([UInt16]1)               # 1 = アイコン
$writer.Write([UInt16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($index = 0; $index -lt $sizes.Count; $index++) {
    $size = $sizes[$index]
    $edge = if ($size -ge 256) { 0 } else { $size }   # 256 は 0 と書く決まり
    $writer.Write([byte]$edge)          # 幅
    $writer.Write([byte]$edge)          # 高さ
    $writer.Write([byte]0)              # パレット数
    $writer.Write([byte]0)              # 予約
    $writer.Write([UInt16]1)            # プレーン
    $writer.Write([UInt16]32)           # bpp
    $writer.Write([UInt32]$images[$index].Length)
    $writer.Write([UInt32]$offset)
    $offset += $images[$index].Length
}
foreach ($image in $images) { $writer.Write($image) }
$writer.Flush()
[System.IO.File]::WriteAllBytes($Out, $file.ToArray())
$writer.Dispose()

$sizeKb = [math]::Round((Get-Item $Out).Length / 1KB, 1)
Write-Host "生成: $Out（$($sizes -join ' / ') px・$sizeKb KB）"
