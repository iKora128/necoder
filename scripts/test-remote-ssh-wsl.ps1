<#
.SYNOPSIS
  Remote SSH の実 SSH 試験を Windows の client から回す（docs/REMOTE-CHANNEL.md §5 の R3）。

.DESCRIPTION
  WSL のディストリに**ユーザー権限の sshd**（sudo 不要・自分のユーザーだけが入れる）を立て、Docker 版
  （scripts/test-remote-ssh-docker.sh）と同じサンプルのプロジェクトを作り、Windows の ssh.exe から
  `crates/host/tests/remote_ssh_live.rs` を流す。remote-server は WSL で musl 向けにビルドして、
  接続の bootstrap が同じ接続の中で送り込む（NECODER_REMOTE_SERVER_BINARY）。

  前提: WSL のディストリに sshd・git・rustup の cargo（x86_64-unknown-linux-musl の target）がある。

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\test-remote-ssh-wsl.ps1
  powershell -ExecutionPolicy Bypass -File scripts\test-remote-ssh-wsl.ps1 -Filter terminal -KeepRunning
#>
param(
    [string]$Distro = 'Ubuntu',
    [int]$Port = 2222,
    # 走らせる試験の名前の一部（空なら全部）。
    [string]$Filter = '',
    # 試験の後も sshd を残す（GUI で手で繋いで確かめる時）。
    [switch]$KeepRunning
)
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$scratch = Join-Path $env:TEMP 'necoder-remote-wsl'
if (Test-Path $scratch) { Remove-Item -Recurse -Force $scratch }
New-Item -ItemType Directory -Force $scratch | Out-Null

function ConvertTo-WslPath([string]$path) {
    $converted = & wsl.exe -d $Distro -- wslpath -u ($path -replace '\\', '/')
    if ($LASTEXITCODE -ne 0) { throw "wslpath に失敗: $path" }
    return $converted.Trim()
}

# bash へ渡すスクリプトは LF で書く（CRLF だと bash が `\r` を命令の一部として読む）。
function Write-LfFile([string]$path, [string]$text) {
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($path, ($text -replace "`r`n", "`n"), $utf8)
}

$wslRepo = ConvertTo-WslPath $repo
$wslScratch = ConvertTo-WslPath $scratch
$user = (& wsl.exe -d $Distro -- whoami).Trim()

Write-Host '==> WSL で necoder-remote-server（x86_64-unknown-linux-musl）をビルド'
Write-LfFile (Join-Path $scratch 'build.sh') @"
set -eu
cargo=`$(command -v cargo || echo "`$HOME/.cargo/bin/cargo")
cd '$wslRepo'
CARGO_TARGET_DIR="`$HOME/necoder-target" "`$cargo" build --release -p host --bin necoder-remote-server --target x86_64-unknown-linux-musl
cp "`$HOME/necoder-target/x86_64-unknown-linux-musl/release/necoder-remote-server" '$wslScratch/necoder-remote-server'
"@
& wsl.exe -d $Distro -- bash "$wslScratch/build.sh"
if ($LASTEXITCODE -ne 0) { throw 'remote-server をビルドできない' }

Write-Host '==> 手元の鍵と ssh_config'
$key = Join-Path $scratch 'id_ed25519'
# PowerShell 5.1 は空の引数を落とすので、空のパスフレーズは '""' で渡す。
& ssh-keygen.exe -q -t ed25519 -N '""' -C 'necoder-remote-wsl' -f $key
if ($LASTEXITCODE -ne 0) { throw 'ssh-keygen に失敗' }
$knownHosts = Join-Path $scratch 'known_hosts'
$config = Join-Path $scratch 'ssh_config'
$forward = { param($path) $path -replace '\\', '/' }
Write-LfFile $config @"
Host necoder-wsl
    HostName 127.0.0.1
    Port $Port
    User $user
    IdentityFile $(& $forward $key)
    IdentitiesOnly yes
    UserKnownHostsFile $(& $forward $knownHosts)
    StrictHostKeyChecking accept-new
    BatchMode yes
    LogLevel ERROR
"@

Write-Host "==> WSL にユーザー権限の sshd（127.0.0.1:$Port）とサンプルのプロジェクト"
Write-LfFile (Join-Path $scratch 'sshd.sh') @"
set -eu
base="`$HOME/necoder-remote-test"
if [ -f "`$base/sshd.pid" ]; then kill "`$(cat "`$base/sshd.pid")" 2>/dev/null || true; fi
pkill -u "`$(id -u)" -f necoder-remote-server 2>/dev/null || true
# 毎回「配備」の経路も通す（版が同じだと前回置いた古いビルドがそのまま使われ、今のコードを試せない）。
rm -rf "`$HOME/.local/share/necoder/remote/servers"
rm -rf "`$base"
mkdir -p "`$base"
chmod 700 "`$base"
ssh-keygen -q -t ed25519 -N '' -f "`$base/host_key"
install -m 600 '$wslScratch/id_ed25519.pub' "`$base/authorized_keys"
cat > "`$base/sshd_config" <<EOF
Port $Port
ListenAddress 127.0.0.1
HostKey `$base/host_key
PidFile `$base/sshd.pid
AuthorizedKeysFile `$base/authorized_keys
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
LogLevel VERBOSE
EOF
project="`$base/work/sample"
mkdir -p "`$project/src" "`$project/docs"
printf '%s\n' '[package]' 'name = "remote-sample"' 'version = "0.1.0"' 'edition = "2021"' > "`$project/Cargo.toml"
printf '%s\n' '/// TODO: replace this sample function with real project code.' 'pub fn add(left: u64, right: u64) -> u64 {' '    left + right' '}' > "`$project/src/lib.rs"
printf '%s\n' 'fn main() {' '    // TODO: add a useful command-line interface.' '    println!("remote ssh sample");' '}' > "`$project/src/main.rs"
printf '%s\n' '# Remote SSH sample' '' 'TODO: document the sample project.' > "`$project/README.md"
printf '%s\n' '# Notes' '' 'The live test edits this file through a second SSH connection.' > "`$project/docs/notes.md"
git -C "`$project" init -q
git -C "`$project" config user.name 'necoder Test'
git -C "`$project" config user.email 'necoder-test@example.invalid'
git -C "`$project" add .
git -C "`$project" commit -qm 'Initial fixture'
/usr/sbin/sshd -f "`$base/sshd_config" -E "`$base/sshd.log"
echo "`$project"
"@
$project = (& wsl.exe -d $Distro -- bash "$wslScratch/sshd.sh" | Select-Object -Last 1).Trim()
if ($LASTEXITCODE -ne 0) { throw 'sshd を立てられない' }

# 立ち上がるまで（WSL の localhost の転送を含む）待つ。
$ready = $false
for ($attempt = 0; $attempt -lt 30; $attempt++) {
    & ssh.exe -F $config necoder-wsl true
    if ($LASTEXITCODE -eq 0) { $ready = $true; break }
    Start-Sleep -Milliseconds 500
}
if (-not $ready) { throw "WSL の sshd（127.0.0.1:$Port）に繋がらない" }

Write-Host '==> Windows の client から実 SSH の試験'
$env:NECODER_SSH_CONFIG = $config
$env:NECODER_REMOTE_TEST_URI = "ssh://necoder-wsl$project"
$env:NECODER_REMOTE_SERVER_BINARY = Join-Path $scratch 'necoder-remote-server'
$env:NECODER_ASKPASS_DISABLE = '1'
# 疎通を「黙って」止める（sshd を SIGSTOP = TCP は張ったまま応答だけ消える・スリープ復帰直後と同じ形）。
# ssh を落とす（EOF が届く）のでは再現できない「無言で死んだ回線」からの復帰を確かめる。
$env:NECODER_REMOTE_TEST_FREEZE = "wsl.exe -d $Distro -- pkill -STOP -u $user -x sshd"
$env:NECODER_REMOTE_TEST_UNFREEZE = "wsl.exe -d $Distro -- pkill -CONT -u $user -x sshd"
Push-Location $repo
try {
    $arguments = @('test', '-p', 'host', '--test', 'remote_ssh_live', '--')
    if ($Filter) { $arguments += $Filter }
    $arguments += @('--nocapture', '--test-threads=1')
    & cargo @arguments
    $status = $LASTEXITCODE
}
finally {
    Pop-Location
    if (-not $KeepRunning) {
        & wsl.exe -d $Distro -- bash -c 'kill "$(cat "$HOME/necoder-remote-test/sshd.pid")" 2>/dev/null; pkill -u "$(id -u)" -f necoder-remote-server 2>/dev/null; true'
    }
    else {
        Write-Host "sshd は残した（ssh -F $config necoder-wsl）"
    }
}
exit $status
