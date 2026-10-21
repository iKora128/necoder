//! system OpenSSH の 1 本の接続（`docs/REMOTE-CHANNEL.md` §1・§3）。
//!
//! 1 回の接続 = 1 つの `ssh` プロセス。リモートコマンドは POSIX sh の bootstrap で、使える
//! remote-server を探し、無ければ**同じ接続の中で**受け取って置き、`proxy` へ exec する。以後その
//! 標準入出力が RPC になり、ファイル操作・プロセス・端末・TCP が全部その上を通る。
//!
//! OpenSSH の ControlMaster は使わない（Windows の OpenSSH には無い。使い回しの代わりに、自前の RPC が
//! 多重化する）。認証は接続のたびに 1 回で、GUI から起こす ssh には TTY が無いので askpass で訊く。

use crate::{
    paths_safe_for_shell, verify_hello, Connector, EventSink, ProcessOwner, RpcClient, SshProject,
    PROTOCOL_VERSION, SERVER_VERSION,
};
use anyhow::{anyhow, bail, Context as _, Result};
use std::io::{Read, Write};
#[cfg(debug_assertions)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

/// 全部の ssh に付ける接続オプション。
const SSH_OPTIONS: [&str; 5] = [
    // 死んだ/到達不能なホストで GUI が無限にハングしないよう接続打ち切りを入れる
    // （既定は OS の TCP タイムアウト任せ＝分単位。sleep/VPN 断からの復帰性に効く）。
    "ConnectTimeout=10",
    "ServerAliveInterval=15",
    "ServerAliveCountMax=3",
    // 利用者の config が多重化を有効にしていても使わない。Windows の OpenSSH は ControlMaster を持たず、
    // ControlPath があるだけで `getsockname failed: Not a socket` で落ちる（2026-10-21 実機）。
    // 多重化は自前の RPC がやるので、二重にもしない。
    "ControlMaster=no",
    "ControlPath=none",
];

/// 認証が終わって bootstrap が最初の行を出すまでの上限。人がパスワードを打つ時間（askpass の 3 分）を含む。
const AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(200);
/// 最初の行から RPC が始まるまでの上限（初回・版の更新では remote-server の転送を含む）。
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(120);
/// bootstrap の 1 行の上限（`.bashrc` が長い行を出しても溜め込まない）。
const BOOTSTRAP_LINE_LIMIT: usize = 4096;
/// `NECODER-` 以外の行を何行まで覚えておくか（失敗した時に見せる）。
const BOOTSTRAP_NOISE_LINES: usize = 8;

/// OpenSSH が接続に失敗した理由の種類（接続のトーストで案内を出し分ける・文言は GUI 側で i18n）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshFailure {
    /// 指紋を確かめていないホスト（初めて・確認を断った）。
    HostKeyUnknown,
    /// 覚えている指紋と違う（作り直した・なりすましの恐れ）。
    HostKeyChanged,
    /// 鍵・パスワードが通らない。
    Authentication,
    /// 鍵を試しすぎて切られた（agent に鍵が多い）。
    TooManyKeys,
    /// ホスト名が引けない。
    UnknownHost,
    /// 接続を断られた（sshd が居ない・ポート違い）。
    Refused,
    /// 応答が無い（ネットワーク・VPN・ファイアウォール）。
    TimedOut,
    /// `~/.ssh` や鍵の権限が広すぎて OpenSSH が読まない。
    BadPermissions,
    /// 鍵交換・暗号の方式が合わない（古い sshd）。
    Negotiation,
}

impl SshFailure {
    /// OpenSSH のログから種類を当てる（知らない書き方なら `None`＝ログをそのまま見せる）。
    /// 順番が意味を持つ: 指紋の変化は「Host key verification failed」も伴い、権限の広い鍵は
    /// 続けて「Permission denied」も出すので、より具体的な方を先に見る。
    pub fn classify(log: &str) -> Option<Self> {
        let has = |needle: &str| log.contains(needle);
        if has("REMOTE HOST IDENTIFICATION HAS CHANGED") {
            Some(Self::HostKeyChanged)
        } else if has("Bad owner or permissions") || has("UNPROTECTED PRIVATE KEY FILE") {
            Some(Self::BadPermissions)
        } else if has("Host key verification failed") || has("host key is known for") {
            Some(Self::HostKeyUnknown)
        } else if has("Too many authentication failures") {
            Some(Self::TooManyKeys)
        } else if has("Permission denied") {
            Some(Self::Authentication)
        } else if has("Could not resolve hostname") {
            Some(Self::UnknownHost)
        } else if has("Connection refused") {
            Some(Self::Refused)
        } else if has("timed out") {
            Some(Self::TimedOut)
        } else if has("Unable to negotiate") {
            Some(Self::Negotiation)
        } else {
            None
        }
    }
}

/// ssh が繋がらなかった。OpenSSH が理由を書いていればそれと、分かれば種類を持つ
/// （GUI は `anyhow::Error::downcast_ref` で取り出して案内を出す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConnectError {
    pub failure: Option<SshFailure>,
    /// OpenSSH が書いた理由（空行と前後の空白を落とした行）。
    pub reason: String,
    status: String,
}

impl SshConnectError {
    /// ssh の終わり方（`exit status: 255`）と、OpenSSH が stderr へ書いた文から作る。
    pub fn new(status: impl Into<String>, log: &str) -> Self {
        let reason = log
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            failure: SshFailure::classify(&reason),
            reason,
            status: status.into(),
        }
    }
}

impl std::fmt::Display for SshConnectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "OpenSSH の接続に失敗: {}", self.status)?;
        if !self.reason.is_empty() {
            write!(formatter, ": {}", self.reason)?;
        }
        Ok(())
    }
}

impl std::error::Error for SshConnectError {}

/// 接続を確かめる（O37・G01）。server の配備はせず、`ssh <destination> true` を 1 回だけ走らせる
/// （鍵のパスフレーズやホスト鍵の確認は askpass で訊く・10 秒で打ち切る）。
/// 成功なら掛かった時間、失敗なら理由つきの [`SshConnectError`]（種類は [`SshFailure`]）。
/// blocking なので背景で呼ぶ。
pub fn test_ssh_connection(project: &SshProject) -> Result<Duration> {
    let started = Instant::now();
    let mut command = ssh_command();
    for option in SSH_OPTIONS {
        command.args(["-o", option]);
    }
    if let Some(port) = project.port {
        command.args(["-p", &port.to_string()]);
    }
    let output = command
        .arg(project.destination())
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("ssh を起動できない")?;
    let elapsed = started.elapsed();
    let log = String::from_utf8_lossy(&output.stderr);
    if !log.trim().is_empty() {
        eprint!("{log}");
    }
    if !output.status.success() {
        return Err(anyhow::Error::new(SshConnectError::new(
            output.status.to_string(),
            &log,
        )));
    }
    Ok(elapsed)
}

/// `ssh` サブプロセスの土台。`NECODER_SSH_CONFIG` が指定されていれば `-F <config>` を先頭に
/// 前置きする。テスト（Docker / WSL の隔離ホスト）やサンドボックス運用で、ユーザーの `~/.ssh/config`
/// や `known_hosts` を汚さずに remote を検証するための seam。未指定なら system ssh の既定どおり。
///
/// GUI から起こす ssh には TTY が無いので、パスワード / passphrase / host key 確認は
/// `SSH_ASKPASS` 経由で GUI に訊く（[`install_askpass`]）。
pub(crate) fn ssh_command() -> Command {
    let mut command = child_process::command("ssh");
    if let Some(config) = std::env::var_os("NECODER_SSH_CONFIG") {
        command.arg("-F").arg(config);
    }
    install_askpass(&mut command);
    command
}

/// 接続先のログインシェルに渡す、remote-server を探して（無ければ受け取って）`proxy` へ exec する
/// スクリプト。§3 の手順をそのまま 1 行にしたもの。
///
/// **`'` `\\` `!` と改行を入れない**: sshd はコマンドを利用者のログインシェルの `-c` に渡す。
/// 外側の `/bin/sh -c '…'` を fish（単引用符の中で `\\` と `\'` を解く）や csh（単引用符の中でも
/// `!` を履歴展開する）にも同じ意味で読ませるため（[`bootstrap_script_is_safe_for_any_login_shell`]）。
/// 1 行を読むのは sh の組み込みの `read` だけ（パイプから 1 byte ずつ読む＝続く RPC を食わない）。
fn bootstrap_script(preferred_server: &str, session: &str) -> String {
    let release = format!("{SERVER_VERSION}-p{PROTOCOL_VERSION}");
    let steps = [
        format!("v=\"{SERVER_VERSION} protocol={PROTOCOL_VERSION}\""),
        format!("d=\"$HOME/.local/share/necoder/remote/servers/{release}\""),
        "b=\"$d/necoder-remote-server\"".to_string(),
        "t=".to_string(),
        "command -v gzip >/dev/null 2>&1 && t=\"${t}gzip,\"".to_string(),
        "printf \"NECODER-HELLO %s %s %s %s\\n\" \"$(uname -s)\" \"$(uname -m)\" \"$t\" \"$HOME\""
            .to_string(),
        "ok() { [ -x \"$1\" ] && [ \"$(\"$1\" version 2>/dev/null)\" = \"$v\" ]; }".to_string(),
        format!("p=$(command -v \"{preferred_server}\" 2>/dev/null)"),
        format!(
            "for c in \"$p\" \"$b\"; do if [ -n \"$c\" ] && ok \"$c\"; then \
             printf \"NECODER-READY %s\\n\" \"$c\"; exec \"$c\" proxy --session {session}; fi; done"
        ),
        "printf \"NECODER-NEED\\n\"".to_string(),
        "IFS=\" \" read -r m n h z || exit 1".to_string(),
        "[ \"$m\" = NECODER-UPLOAD ] || exit 1".to_string(),
        "umask 077".to_string(),
        "mkdir -p \"$d\" || exit 1".to_string(),
        "f=\"$d/.upload-$$\"".to_string(),
        "if [ \"$z\" = 1 ]; then g=\"$f.gz\"; else g=\"$f\"; fi".to_string(),
        // GNU の head はちょうど n バイトだけ読む。無ければ dd を 1 バイト単位の読みで使う（遅いが正確）。
        "if head --version 2>/dev/null | grep GNU >/dev/null 2>&1; then head -c \"$n\" > \"$g\"; \
         else dd of=\"$g\" ibs=1 obs=65536 count=\"$n\" 2>/dev/null; fi"
            .to_string(),
        "if [ \"$z\" = 1 ]; then gzip -dc < \"$g\" > \"$f\" || { rm -f \"$g\" \"$f\"; \
         printf \"NECODER-FAIL gzip\\n\"; exit 1; }; rm -f \"$g\"; fi"
            .to_string(),
        "a=".to_string(),
        "if command -v sha256sum >/dev/null 2>&1; then a=$(sha256sum \"$f\" | cut -d \" \" -f 1); \
         elif command -v shasum >/dev/null 2>&1; then a=$(shasum -a 256 \"$f\" | cut -d \" \" -f 1); fi"
            .to_string(),
        "if [ -n \"$a\" ]; then [ \"$a\" = \"$h\" ] || { rm -f \"$f\"; \
         printf \"NECODER-FAIL checksum %s\\n\" \"$a\"; exit 1; }; fi"
            .to_string(),
        "chmod 700 \"$f\" && mv -f \"$f\" \"$b\" || { printf \"NECODER-FAIL install\\n\"; exit 1; }"
            .to_string(),
        "ok \"$b\" || { printf \"NECODER-FAIL version\\n\"; exit 1; }".to_string(),
        // 検証に通ってから、今の版以外の古い server を消す（版が上がるたびに溜まって容量を食わない）。
        format!(
            "(cd \"$d/..\" && for e in */; do [ \"$e\" = \"{release}/\" ] || rm -rf -- \"$e\"; done) \
             >/dev/null 2>&1"
        ),
        format!(
            "printf \"NECODER-READY %s\\n\" \"$b\"; exec \"$b\" proxy --session {session}"
        ),
    ];
    format!("/bin/sh -c '{}'", steps.join("; "))
}

/// bootstrap の最初の行（`NECODER-HELLO`）で分かる接続先の様子。
#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteHello {
    os: String,
    arch: String,
    gzip: bool,
    home: String,
}

impl RemoteHello {
    fn parse(rest: &str) -> Option<Self> {
        let mut fields = rest.splitn(4, ' ');
        let os = fields.next()?.to_string();
        let arch = fields.next()?.to_string();
        let tools = fields.next()?;
        let home = fields.next()?.to_string();
        (!os.is_empty() && !arch.is_empty() && !home.is_empty()).then(|| Self {
            os,
            arch,
            gzip: tools.split(',').any(|tool| tool == "gzip"),
            home,
        })
    }
}

/// 1 つの接続先へ繋ぐ側（[`crate::ReconnectingClient`] が切れるたびに呼ぶ）。
pub(crate) struct SshConnector {
    project: SshProject,
    preferred_server: String,
    session: String,
    /// 最初の接続で分かった接続先の `$HOME`（path 無しの URI の root）。
    home: Mutex<Option<PathBuf>>,
    /// 最後に起こした ssh（障害注入テストで落とす）。
    current: Mutex<Option<Weak<ProcessOwner>>>,
}

impl SshConnector {
    pub(crate) fn new(project: &SshProject, preferred_server: &str) -> Result<Self> {
        anyhow::ensure!(
            !preferred_server.is_empty() && paths_safe_for_shell(preferred_server),
            "remote server のコマンドに使えない文字がある: {preferred_server}"
        );
        Ok(Self {
            project: project.clone(),
            preferred_server: preferred_server.to_string(),
            session: random_session_id()?,
            home: Mutex::new(None),
            current: Mutex::new(None),
        })
    }

    pub(crate) fn home(&self) -> Option<PathBuf> {
        self.home.lock().ok().and_then(|home| home.clone())
    }

    /// 障害注入テスト用: いまの ssh を落とす（スリープ・VPN 断で接続が切れたのと同じ）。
    pub(crate) fn drop_connection(&self) {
        let owner = self
            .current
            .lock()
            .ok()
            .and_then(|current| current.as_ref().and_then(Weak::upgrade));
        if let Some(owner) = owner {
            owner.kill();
        }
    }

    fn command(&self) -> Command {
        let mut command = ssh_command();
        command.arg("-T");
        for option in SSH_OPTIONS {
            command.args(["-o", option]);
        }
        if let Some(port) = self.project.port {
            command.args(["-p", &port.to_string()]);
        }
        command
            .arg(self.project.destination())
            .arg(bootstrap_script(&self.preferred_server, &self.session));
        command
    }
}

impl Connector for SshConnector {
    fn connect(&self, event_sink: Arc<EventSink>) -> Result<Arc<RpcClient>> {
        let destination = self.project.destination();
        let started = Instant::now();
        ssh_log(&destination, "接続開始", None);
        let mut child = self
            .command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("ssh を起動できない")?;
        let mut stdin = child.stdin.take().context("ssh の stdin が無い")?;
        let mut stdout = child.stdout.take().context("ssh の stdout が無い")?;
        let stderr = child.stderr.take().context("ssh の stderr が無い")?;
        let log = SshLog::capture(stderr);
        let owner = Arc::new(ProcessOwner::new(child));
        if let Ok(mut current) = self.current.lock() {
            *current = Some(Arc::downgrade(&owner));
        }
        let watchdog = Watchdog::start(&owner, AUTHENTICATION_TIMEOUT);

        let mut noise = Vec::new();
        let failure = |noise: &[String], what: &str| -> anyhow::Error {
            // ssh が先に終わったなら、それが理由（認証・ホスト鍵・名前・拒否…）。
            let status = owner.wait_for_exit(Duration::from_secs(2));
            let written = log.text();
            if let Some(status) = status.filter(|status| !status.success()) {
                return anyhow::Error::new(SshConnectError::new(status.to_string(), &written));
            }
            let mut message = format!("接続先の準備に失敗（{what}）");
            if !noise.is_empty() {
                message.push_str(&format!("。接続先が出した行: {}", noise.join(" / ")));
            }
            if !written.trim().is_empty() {
                message.push_str(&format!("。ssh: {}", written.trim()));
            }
            anyhow!(message)
        };

        let hello = loop {
            let Some(line) = read_bootstrap_line(&mut stdout)? else {
                return Err(failure(&noise, "最初の行が届く前に閉じた"));
            };
            if let Some(rest) = line.strip_prefix("NECODER-HELLO ") {
                break RemoteHello::parse(rest)
                    .ok_or_else(|| failure(&noise, "接続先の様子を読めない"))?;
            }
            remember_noise(&mut noise, line);
        };
        watchdog.extend(BOOTSTRAP_TIMEOUT);
        ssh_log(&destination, "認証済み", Some(started));
        if let Ok(mut home) = self.home.lock() {
            *home = Some(PathBuf::from(&hello.home));
        }

        let server = loop {
            let Some(line) = read_bootstrap_line(&mut stdout)? else {
                return Err(failure(&noise, "remote-server を起こす前に閉じた"));
            };
            if let Some(path) = line.strip_prefix("NECODER-READY ") {
                break path.to_string();
            }
            if line == "NECODER-NEED" {
                let phase = Instant::now();
                send_remote_server(&mut stdin, &hello)?;
                ssh_log(&destination, "remote-server を配備", Some(phase));
                continue;
            }
            if let Some(reason) = line.strip_prefix("NECODER-FAIL ") {
                return Err(failure(
                    &noise,
                    &format!("remote-server を置けない: {reason}"),
                ));
            }
            remember_noise(&mut noise, line);
        };
        watchdog.extend(crate::HELLO_TIMEOUT);
        remote_ssh_trace(&format!("remote-server: {server}"));
        let client = RpcClient::new(Box::new(stdout), Box::new(stdin), Some(owner), event_sink);
        verify_hello(&client).context("session proxy 接続 / protocol handshake に失敗")?;
        watchdog.cancel();
        // 以後の stderr（remote-server の知らせ）も、これまでどおり手元の stderr へ流す。
        log.forward_to_stderr();
        ssh_log(&destination, "session 確立（接続完了）", Some(started));
        Ok(client)
    }
}

/// 接続先に合う remote-server を探して、bootstrap が待っている stdin へ送る。
fn send_remote_server(stdin: &mut impl Write, hello: &RemoteHello) -> Result<()> {
    let explicit = std::env::var_os("NECODER_REMOTE_SERVER_BINARY").map(PathBuf::from);
    let Some(artifact) = explicit.or_else(|| find_remote_server_for(&hello.os, &hello.arch)) else {
        // bootstrap を正常に終わらせる（`read` が EOF を待ち続けない）。
        if let Err(error) = stdin.write_all(b"NECODER-ABORT\n") {
            remote_ssh_trace(&format!("配備の取りやめを伝えられない: {error}"));
        }
        bail!(
            "接続先（{}/{}）用の necoder-remote-server が見つからない。\
             CI 生成物を ~/.local/share/necoder/remote/artifacts/<target>/ に置くか、\
             NECODER_REMOTE_SERVER_BINARY=<接続先向けの artifact> を指定してください",
            hello.os,
            hello.arch
        );
    };
    let binary = std::fs::read(&artifact)
        .with_context(|| format!("remote server artifact を読めない: {}", artifact.display()))?;
    let checksum = sha256_hex(&binary);
    let (payload, gzip) = if hello.gzip {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&binary)?;
        (encoder.finish()?, true)
    } else {
        (binary, false)
    };
    writeln!(
        stdin,
        "NECODER-UPLOAD {} {checksum} {}",
        payload.len(),
        u8::from(gzip)
    )
    .context("remote server の転送を始められない")?;
    stdin
        .write_all(&payload)
        .context("remote server artifact の転送に失敗")?;
    stdin.flush()?;
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    let digest = sha2::Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // String への書き込みは失敗しない。
        let _infallible = write!(hex, "{byte:02x}");
    }
    hex
}

/// bootstrap の 1 行を読む（`\n` まで・1 byte ずつ）。**先読みしない**: この後ろはそのまま RPC の
/// frame になるので、`BufReader` で余分に取ると最初の応答を食ってしまう。`None` = 閉じた。
fn read_bootstrap_line(reader: &mut impl Read) -> Result<Option<String>> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => {
                return Ok((!line.is_empty())
                    .then(|| String::from_utf8_lossy(&line).trim_end().to_string()));
            }
            Ok(_) if byte[0] == b'\n' => {
                return Ok(Some(String::from_utf8_lossy(&line).trim_end().to_string()));
            }
            Ok(_) => {
                if line.len() < BOOTSTRAP_LINE_LIMIT {
                    line.push(byte[0]);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("ssh の出力を読めない"),
        }
    }
}

fn remember_noise(noise: &mut Vec<String>, line: String) {
    let line = line.trim().to_string();
    if line.is_empty() {
        return;
    }
    if noise.len() == BOOTSTRAP_NOISE_LINES {
        noise.remove(0);
    }
    noise.push(line);
}

/// ssh の stderr を読み続ける（OpenSSH の理由・remote-server の知らせ）。接続が確立するまでは
/// 手元に溜めて失敗の理由に使い、確立した後は手元の stderr へそのまま流す。
struct SshLog {
    shared: Arc<Mutex<SshLogState>>,
}

#[derive(Default)]
struct SshLogState {
    text: String,
    forward: bool,
}

/// 溜めておく stderr の上限（OpenSSH の理由は数行・`.bashrc` の大量の出力で膨らませない）。
const SSH_LOG_LIMIT: usize = 16 * 1024;

impl SshLog {
    fn capture(mut stderr: impl Read + Send + 'static) -> Self {
        let shared = Arc::new(Mutex::new(SshLogState::default()));
        let writer = shared.clone();
        let spawned = thread::Builder::new()
            .name("necoder-ssh-stderr".to_string())
            .spawn(move || {
                let mut chunk = [0u8; 4096];
                loop {
                    let read = match stderr.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    };
                    let text = String::from_utf8_lossy(&chunk[..read]).to_string();
                    let Ok(mut state) = writer.lock() else {
                        break;
                    };
                    if state.forward {
                        eprint!("{text}");
                    } else if state.text.len() < SSH_LOG_LIMIT {
                        state.text.push_str(&text);
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("Remote SSH: ssh の stderr を読むスレッドを起動できない: {error}");
        }
        Self { shared }
    }

    fn text(&self) -> String {
        self.shared
            .lock()
            .map(|state| state.text.clone())
            .unwrap_or_default()
    }

    /// 確立した。溜めた分（known_hosts に足した等の警告）を出し、以後はそのまま流す。
    fn forward_to_stderr(&self) {
        if let Ok(mut state) = self.shared.lock() {
            if !state.text.trim().is_empty() {
                eprint!("{}", state.text);
            }
            state.text.clear();
            state.forward = true;
        }
    }
}

/// 期限が来たら ssh を落とす（読みが返らないまま固まらない）。延長・取り消しができる。
struct Watchdog {
    deadline: Arc<Mutex<Option<Instant>>>,
}

impl Watchdog {
    fn start(owner: &Arc<ProcessOwner>, timeout: Duration) -> Self {
        let deadline = Arc::new(Mutex::new(Some(Instant::now() + timeout)));
        let watched = deadline.clone();
        let owner = Arc::downgrade(owner);
        let spawned = thread::Builder::new()
            .name("necoder-ssh-watchdog".to_string())
            .spawn(move || loop {
                thread::sleep(Duration::from_millis(200));
                let Ok(deadline) = watched.lock().map(|deadline| *deadline) else {
                    return;
                };
                let Some(deadline) = deadline else {
                    return;
                };
                if Instant::now() >= deadline {
                    if let Some(owner) = owner.upgrade() {
                        eprintln!("Remote SSH: 接続の準備が時間内に終わらないので打ち切る");
                        owner.kill();
                    }
                    return;
                }
            });
        if let Err(error) = spawned {
            eprintln!("Remote SSH: 接続の見張りを起動できない: {error}");
        }
        Self { deadline }
    }

    fn extend(&self, timeout: Duration) {
        if let Ok(mut deadline) = self.deadline.lock() {
            if deadline.is_some() {
                *deadline = Some(Instant::now() + timeout);
            }
        }
    }

    fn cancel(&self) {
        if let Ok(mut deadline) = self.deadline.lock() {
            *deadline = None;
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// session id（64 桁の hex）。OS の乱数から作る（`/dev/urandom` を直接読まない＝Windows でも同じ）。
pub(crate) fn random_session_id() -> Result<String> {
    random_hex(32)
}

fn random_hex(length: usize) -> Result<String> {
    let mut bytes = vec![0u8; length];
    getrandom::fill(&mut bytes).map_err(|error| anyhow!("OS の乱数を読めない: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn ssh_log(destination: &str, phase: &str, elapsed_from: Option<Instant>) {
    match elapsed_from {
        Some(started) => {
            eprintln!(
                "[ssh {destination}] {phase} ({}ms)",
                started.elapsed().as_millis()
            );
        }
        None => eprintln!("[ssh {destination}] {phase}"),
    }
}

fn remote_ssh_trace(message: &str) {
    if std::env::var_os("NECODER_REMOTE_TRACE").is_some() {
        eprintln!("[remote-trace] {message}");
    }
}

/// remote の uname (os, arch) を Rust target triple へ（配備バイナリの探索キー・#1）。
/// Linux は static-musl（glibc 依存なし）、macOS は apple-darwin。未知は None。
pub(crate) fn remote_target_triple(remote_os: &str, remote_arch: &str) -> Option<String> {
    let os = match remote_os.to_ascii_lowercase().as_str() {
        "linux" => "unknown-linux-musl",
        "darwin" => "apple-darwin",
        _ => return None,
    };
    let arch = match remote_arch.to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        _ => return None,
    };
    Some(format!("{arch}-{os}"))
}

/// remote target 用の配備バイナリを探す（#1 の自動発見・env 指定なしで mac/Windows → Linux を通す）:
/// 1) per-target キャッシュ `~/.local/share/necoder/remote/artifacts/<triple>/necoder-remote-server`
/// 2) mac の .app 同梱 `<exe>/../Resources/remote/<triple>/necoder-remote-server`
/// 3) Windows / Linux の同梱 `<exe のフォルダ>/remote/<triple>/necoder-remote-server`（インストーラと zip）
/// 4) same-platform なら従来の sibling / dev ビルド（[`find_local_remote_server`]）
fn find_remote_server_for(remote_os: &str, remote_arch: &str) -> Option<PathBuf> {
    if let Some(triple) = remote_target_triple(remote_os, remote_arch) {
        if let Some(home) = paths::home_dir() {
            let cached = home
                .join(".local/share/necoder/remote/artifacts")
                .join(&triple)
                .join("necoder-remote-server");
            if cached.is_file() {
                return Some(cached);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for bundled in [dir.join("../Resources/remote"), dir.join("remote")] {
                    let bundled = bundled.join(&triple).join("necoder-remote-server");
                    if bundled.is_file() {
                        return Some(bundled);
                    }
                }
            }
        }
    }
    if same_platform(remote_os, remote_arch) {
        return find_local_remote_server();
    }
    None
}

fn find_local_remote_server() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let sibling = current.parent()?.join("necoder-remote-server");
    if sibling.is_file() {
        return Some(sibling);
    }
    // 開発ビルド限定の fallback。release では CARGO_MANIFEST_DIR（ビルドマシンの絶対パス）を
    // 配布バイナリに焼き込まない — release.yml のビルドパス焼き込みガードが 0 件を前提にする。
    #[cfg(debug_assertions)]
    {
        let development =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/necoder-remote-server");
        if development.is_file() {
            return Some(development);
        }
    }
    None
}

fn same_platform(remote_os: &str, remote_arch: &str) -> bool {
    let remote_os = remote_os.to_ascii_lowercase();
    let os = match remote_os.as_str() {
        "darwin" => "macos",
        "linux" => "linux",
        other => other,
    };
    let remote_arch = remote_arch.to_ascii_lowercase();
    let arch = match remote_arch.as_str() {
        "arm64" => "aarch64",
        "amd64" => "x86_64",
        other => other,
    };
    os == std::env::consts::OS && arch == std::env::consts::ARCH
}

// ---------------------------------------------------------------------------
// GUI askpass（ROADMAP「残: GUI askpass」）
// ---------------------------------------------------------------------------

/// 発行した askpass token の寿命。1 回の接続試行（再入力 3 回ぶん）を見込んだ上限で、
/// 切れた token は次の発行時に掃除される。
const ASKPASS_TOKEN_TTL: Duration = Duration::from_secs(180);

/// 1 回の接続試行に対して発行した askpass token と、その token で訊かれた回数。
///
/// GUI から秘密を引き出せるのは、**その ssh のために発行した token を持つ子プロセスだけ**。
/// token は `ssh` の環境変数にしか置かないので、GUI の制御口を開けるだけの別プロセスは
/// パスワード入力欄を出せない（制御口は本人だけが開けるが、秘密を返す口は更に絞る）。
///
/// `asked` は**同じ問い**が何回来たかを prompt 文字列ごとに数える。OpenSSH は入力を間違えると
/// 同じ prompt で askpass を呼び直す（既定 3 回）ので、2 回目以降は GUI が「失敗したので
/// もう一度」と言える。prompt ごとに分けるのは、passphrase → password のように**別の問い**へ
/// 進んだときに再入力と誤表示しないため。
struct AskpassIssue {
    token: String,
    issued: Instant,
    asked: Vec<(String, u32)>,
}

static ASKPASS_TOKENS: Mutex<Vec<AskpassIssue>> = Mutex::new(Vec::new());

/// token を 1 つ発行して登録する（期限切れはこの機会に捨てる）。乱数が読めなければ askpass を諦める
/// （推測可能な token を配るくらいなら、従来どおり「TTY が無いので失敗」の方が安全）。
fn issue_askpass_token() -> Option<String> {
    let token = random_hex(16).ok()?;
    let mut tokens = ASKPASS_TOKENS.lock().ok()?;
    let now = Instant::now();
    tokens.retain(|issue| now.duration_since(issue.issued) < ASKPASS_TOKEN_TTL);
    tokens.push(AskpassIssue {
        token: token.clone(),
        issued: now,
        asked: Vec::new(),
    });
    Some(token)
}

/// GUI 側の検証。「今このプロセスが出した、まだ生きている token か」だけを見る。
///
/// 使い捨てにはしない — OpenSSH は 1 回の接続で鍵の passphrase とパスワードを続けて訊くことが
/// あり、再入力（`NumberOfPasswordPrompts`）もあるため、寿命は TTL で切る。
pub fn askpass_token_is_live(token: &str) -> bool {
    let Ok(mut tokens) = ASKPASS_TOKENS.lock() else {
        return false;
    };
    let now = Instant::now();
    tokens.retain(|issue| now.duration_since(issue.issued) < ASKPASS_TOKEN_TTL);
    tokens.iter().any(|issue| issue.token == token)
}

/// token を検証しつつ「この問いは何回目か」を 1 始まりで返す（生きていなければ `None`）。
/// 2 以上 = 直前の入力が拒否されて OpenSSH が訊き直している、と GUI が判断できる。
pub fn askpass_attempt(token: &str, prompt: &str) -> Option<u32> {
    let mut tokens = ASKPASS_TOKENS.lock().ok()?;
    let now = Instant::now();
    tokens.retain(|issue| now.duration_since(issue.issued) < ASKPASS_TOKEN_TTL);
    let issue = tokens.iter_mut().find(|issue| issue.token == token)?;
    if let Some((_, count)) = issue.asked.iter_mut().find(|(seen, _)| seen == prompt) {
        *count += 1;
        return Some(*count);
    }
    issue.asked.push((prompt.to_string(), 1));
    Some(1)
}

/// この `ssh` に「訊く先」を持たせる。
///
/// - `SSH_ASKPASS` = necoder 自身（起動時に `NECODER_ASKPASS_TOKEN` を見て askpass モードに入る）
/// - `SSH_ASKPASS_REQUIRE=force` = TTY の有無に依らず askpass を使う（OpenSSH 8.4+ の公開仕様。
///   Windows の OpenSSH 9.5 も同じ）
///
/// 何もせず返る条件（いずれも「GUI に訊けない・訊くべきでない」場合）:
/// 利用者が自前の `SSH_ASKPASS` を持っている / `NECODER_ASKPASS_DISABLE` / GUI が居ない
/// （headless CLI・統合テスト・`necoder-remote-server`）。
fn install_askpass(command: &mut Command) {
    if std::env::var_os("SSH_ASKPASS").is_some()
        || std::env::var_os("NECODER_ASKPASS_DISABLE").is_some()
    {
        return;
    }
    // 自分自身が askpass として呼ばれている最中なら、その子に token を配り直さない。
    if std::env::var_os("NECODER_ASKPASS_TOKEN").is_some() {
        return;
    }
    if !gui_can_answer_askpass() {
        return;
    }
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(token) = issue_askpass_token() else {
        return;
    };
    command
        .env("SSH_ASKPASS", executable)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("NECODER_ASKPASS_TOKEN", token);
}

/// GUI の制御口が答えられる状態か。
///
/// unix は制御口（Unix socket）がファイルとして在るかで見る。Windows の制御口は名前付きパイプで、
/// 在るかを確かめるだけで接続を 1 つ消費してしまうので、**このプロセスが GUI か**（窓を開ける前に
/// [`crate::mark_main_thread`] を呼んでいるか）で見る。GUI の ssh だけが askpass を使えれば足りる。
fn gui_can_answer_askpass() -> bool {
    let Some(socket) = paths::runtime_socket() else {
        return false;
    };
    if cfg!(windows) {
        crate::MAIN_THREAD.get().is_some()
    } else {
        socket.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §3 / §4: 外側の `/bin/sh -c '…'` を、fish や csh のログインシェルでも同じ意味で読ませる。
    #[test]
    fn bootstrap_script_is_safe_for_any_login_shell() {
        let script = bootstrap_script("necoder-remote-server", &"a".repeat(64));
        let inner = script
            .strip_prefix("/bin/sh -c '")
            .and_then(|rest| rest.strip_suffix('\''))
            .expect("外側は /bin/sh -c '…'");
        assert!(
            !inner.contains('\''),
            "単引用符を入れると外側の引用が閉じる"
        );
        assert!(!inner.contains('!'), "csh は単引用符の中でも ! を展開する");
        assert!(
            !inner.contains("\\\\") && !inner.contains("\\'"),
            "fish が解く並び"
        );
        assert!(!inner.contains('\n'), "csh は引用の中の改行を受け付けない");
        assert!(inner.contains("NECODER-HELLO") && inner.contains("proxy --session aaaa"));
    }

    #[test]
    fn hello_line_keeps_a_home_with_spaces() {
        assert_eq!(
            RemoteHello::parse("Linux x86_64 gzip, /home/dev user"),
            Some(RemoteHello {
                os: "Linux".to_string(),
                arch: "x86_64".to_string(),
                gzip: true,
                home: "/home/dev user".to_string(),
            })
        );
        // 道具が 1 つも無い（空のまま区切りだけ残る）。
        assert_eq!(
            RemoteHello::parse("Darwin arm64  /Users/dev").map(|hello| (hello.gzip, hello.home)),
            Some((false, "/Users/dev".to_string()))
        );
        assert_eq!(RemoteHello::parse("Linux"), None);
    }

    /// bootstrap の行は 1 byte ずつ読み、後ろの RPC のバイトに手を付けない。
    #[test]
    fn bootstrap_lines_do_not_read_ahead() {
        let mut input: &[u8] = b"motd line\nNECODER-READY /srv/bin\nSHRS\x04\x00";
        assert_eq!(
            read_bootstrap_line(&mut input).unwrap().as_deref(),
            Some("motd line")
        );
        assert_eq!(
            read_bootstrap_line(&mut input).unwrap().as_deref(),
            Some("NECODER-READY /srv/bin")
        );
        assert_eq!(input, b"SHRS\x04\x00", "frame の先頭が残っている");
    }

    #[test]
    fn checksum_is_lower_hex_sha256() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// askpass token: 発行したものだけが生き、知らない値は通らない。
    /// （GUI 側 `control_ipc` の "askpass" はこの判定だけを根拠に秘密を返す。）
    #[test]
    fn askpass_token_only_matches_issued_value() {
        let token = issue_askpass_token().expect("OS の乱数から token を作れる");
        assert_eq!(token.len(), 32, "128bit を hex で = 32 文字");
        assert!(askpass_token_is_live(&token));
        assert!(
            !askpass_token_is_live("deadbeef"),
            "発行していない値は通さない"
        );
        assert!(!askpass_token_is_live(""), "空も通さない");
        let second = issue_askpass_token().expect("2 本目も作れる");
        assert_ne!(token, second, "接続ごとに別の token");
        assert!(
            askpass_token_is_live(&token),
            "TTL 内なら先の token も生きている"
        );
    }

    /// 同じ問いの回数だけが増え、別の問い・別の token では 1 に戻る
    /// （GUI の「入力が拒否されました」表示の根拠）。
    #[test]
    fn askpass_attempt_counts_repeats_per_prompt() {
        let token = issue_askpass_token().expect("token を作れる");
        let password = "user@example.internal's password:";
        let passphrase = "Enter passphrase for key '/home/user/.ssh/id_ed25519':";
        assert_eq!(askpass_attempt(&token, password), Some(1));
        assert_eq!(
            askpass_attempt(&token, password),
            Some(2),
            "同じ問い = 再入力"
        );
        assert_eq!(
            askpass_attempt(&token, passphrase),
            Some(1),
            "別の問いは 1 回目から"
        );
        let other = issue_askpass_token().expect("別の接続の token");
        assert_eq!(
            askpass_attempt(&other, password),
            Some(1),
            "token ごとに独立"
        );
        assert_eq!(askpass_attempt("deadbeef", password), None, "無効な token");
    }

    /// `install_askpass`: 無効化されていれば触らない（環境の尊重）。
    /// env は同一プロセス共有なので、判定に使う関数の入口だけを直接確かめる。
    #[test]
    fn install_askpass_respects_disable_switch() {
        // SAFETY: このテストは env を書き換える。並行実行される他テストは
        // NECODER_ASKPASS_DISABLE を読まないので影響しない。
        unsafe { std::env::set_var("NECODER_ASKPASS_DISABLE", "1") };
        let mut command = child_process::command("ssh");
        install_askpass(&mut command);
        assert!(
            command.get_envs().next().is_none(),
            "無効化されているときは env を足さない"
        );
        unsafe { std::env::remove_var("NECODER_ASKPASS_DISABLE") };
    }

    #[test]
    fn remote_target_triple_maps_uname_to_musl() {
        assert_eq!(
            remote_target_triple("Linux", "x86_64").as_deref(),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            remote_target_triple("Linux", "aarch64").as_deref(),
            Some("aarch64-unknown-linux-musl")
        );
        assert_eq!(
            remote_target_triple("Darwin", "arm64").as_deref(),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(remote_target_triple("FreeBSD", "x86_64"), None);
    }
}
