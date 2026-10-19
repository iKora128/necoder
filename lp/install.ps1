# necoder installer for Windows.
#
#   irm https://necoder.com/install.ps1 | iex
#
# Downloads the latest necoder-windows-x64-setup.exe from GitHub Releases, checks its SHA-256
# against the digest GitHub publishes for the asset, and runs it for the current user
# (no admin rights). Adds a Start menu entry, a desktop shortcut, "Open with necoder" in
# Explorer and `necoder` on PATH.
#
# Options (set before running):
#   $env:NECODER_VERSION = '0.1.31'          # install a specific version instead of the latest
#   $env:NECODER_TASKS   = 'contextmenu,addtopath'   # pick the installer tasks (default: all three)
#
# This file is kept ASCII-only on purpose: `irm` decodes the response by its Content-Type, and
# Windows PowerShell 5.1 falls back to a non-UTF-8 code page, which would garble other text.
# Source: https://github.com/iKora128/necoder/blob/main/lp/install.ps1

& {
    $ErrorActionPreference = 'Stop'
    # Windows PowerShell 5.1 on older Windows 10 may still default to TLS 1.0, which GitHub refuses.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    # The progress bar makes Invoke-WebRequest many times slower on Windows PowerShell 5.1.
    $ProgressPreference = 'SilentlyContinue'

    if (-not [Environment]::Is64BitOperatingSystem) {
        throw 'necoder needs 64-bit Windows (x64).'
    }

    $repository = 'iKora128/necoder'
    $assetName = 'necoder-windows-x64-setup.exe'
    $release = if ($env:NECODER_VERSION) {
        "https://api.github.com/repos/$repository/releases/tags/v$($env:NECODER_VERSION.TrimStart('v'))"
    } else {
        "https://api.github.com/repos/$repository/releases/latest"
    }
    Write-Host 'necoder: looking up the release...'
    $info = Invoke-RestMethod -Uri $release -Headers @{ 'User-Agent' = 'necoder-install' }
    $asset = $info.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
    if (-not $asset) {
        throw "necoder: $($info.tag_name) has no $assetName. See https://github.com/$repository/releases"
    }

    $work = Join-Path ([IO.Path]::GetTempPath()) ("necoder-install-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        $setup = Join-Path $work $assetName
        Write-Host "necoder: downloading $($info.tag_name) ($([math]::Round($asset.size / 1MB, 1)) MB)..."
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $setup -UseBasicParsing `
            -Headers @{ 'User-Agent' = 'necoder-install' }

        # GitHub publishes "sha256:<hex>" for every release asset. Refuse to run anything else.
        if ($asset.digest -and $asset.digest -like 'sha256:*') {
            $expected = $asset.digest.Substring(7).ToLowerInvariant()
            $actual = (Get-FileHash -Algorithm SHA256 -Path $setup).Hash.ToLowerInvariant()
            if ($actual -ne $expected) {
                throw "necoder: checksum mismatch (expected $expected, got $actual). Nothing was installed."
            }
            Write-Host 'necoder: checksum OK'
        } else {
            Write-Warning 'necoder: GitHub did not report a checksum for this file; skipping verification.'
        }

        $tasks = if ($env:NECODER_TASKS) { $env:NECODER_TASKS } else { 'desktopicon,contextmenu,addtopath' }
        Write-Host 'necoder: installing for the current user...'
        $arguments = @('/SILENT', '/CURRENTUSER', '/NORESTART', '/CLOSEAPPLICATIONS', "/TASKS=`"$tasks`"")
        $process = Start-Process -FilePath $setup -ArgumentList $arguments -Wait -PassThru
        if ($process.ExitCode -ne 0) {
            throw "necoder: the installer exited with code $($process.ExitCode)."
        }
    } finally {
        Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
    }

    $exe = Join-Path $env:LOCALAPPDATA 'Programs\necoder\necoder.exe'
    Write-Host ''
    Write-Host "necoder $($info.tag_name) is installed: $exe"
    Write-Host 'Start it from the Start menu or the desktop shortcut.'
    Write-Host 'Open a new terminal to use the `necoder` command (PATH changes apply to new windows).'
}
