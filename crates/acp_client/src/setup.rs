//! setup — 設定の「インストール」1 つで、エージェントを裏で入れてログインまで進める（2026-10-19・#69）。
//!
//! 以前は「npm でパソコン全体に入れる」コマンドをターミナルで流していた。Windows には普通 Node.js が
//! 無く、winget・管理者の確認（UAC）・PowerShell の実行ポリシー・ターミナルの読み方まで、初めての人が
//! 越える物が多すぎた。VS Code の Claude Code / Codex 拡張や Zed は、本体を自分で持ち、ログインは
//! ボタン → ブラウザで済ませる。ここはそれと同じ形にする:
//!
//! 1. **Node.js** — パソコンに使える版（[`NODE_MIN_MAJOR`] 以上）があればそれ。無ければ公式の配布物
//!    （[`NODE_VERSION`]）を necoder の道具箱（[`paths::toolchain_dir`]）へ落とし、**このソースに
//!    書いた SHA-256 と照合してから**展開する
//! 2. **エージェントの CLI** — `npm install -g --prefix <道具箱>/npm`（necoder の中だけ・パソコン全体の
//!    npm にも PATH にも触らない。道具箱は起動時に PATH の末尾へ足してある＝`host::append_toolchain_to_path`）
//! 3. **ACP アダプタ** — 起動時に使う版（レジストリの版）を necoder の置き場へ先に入れる
//!    （最初のスレッドが数百 MB の取得を待たない・`install::preinstall`）
//! 4. **ログイン**（[`LoginSession`]）— CLI のログインを裏で起こす。CLI が自分でブラウザを開く。
//!    Codex は手元で受けて自動で終わる。Claude Code はブラウザに出たコードを貼る（necoder の入力欄から
//!    CLI の標準入力へ渡す）
//!
//! 何をしたかは `logs/agent-setup.log` に残し、失敗した時は最後の数行を画面に出す。

use super::*;
use std::io::{Read as _, Write as _};
use std::process::{Child, ChildStdin};
use std::sync::{Arc, Mutex};

/// necoder が落とす Node.js の版（LTS）。上げる時は下の SHA-256 も公式の `SHASUMS256.txt` から
/// 書き直すこと（https://nodejs.org/dist/v<版>/SHASUMS256.txt）。
pub const NODE_VERSION: &str = "24.22.0";

/// パソコンの Node.js をそのまま使う下限（これより古ければ necoder の道具箱に入れる）。
pub const NODE_MIN_MAJOR: u32 = 20;

/// プラットフォームごとの公式の配布物（`node-v<版>-<名前>`）と、その SHA-256。
const NODE_ARCHIVES: [(&str, &str, &str); 6] = [
    (
        "windows-x86_64",
        "win-x64.zip",
        "f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3",
    ),
    (
        "windows-aarch64",
        "win-arm64.zip",
        "3a5c209a5bc59e65b3bc0fa13ed5e166a26bd3d55332847f498b76b037408d61",
    ),
    (
        "macos-aarch64",
        "darwin-arm64.tar.gz",
        "6a9724d9b50a5f22b4816cded441b5a8d9ddae546191a5e5b7efa5a5e2ee26d5",
    ),
    (
        "macos-x86_64",
        "darwin-x64.tar.gz",
        "495e3f273c645f3c4677d438304cdfc701729ea5dec5728f5c8e76a2dece4b91",
    ),
    (
        "linux-x86_64",
        "linux-x64.tar.gz",
        "28ea0a438de9a13bd7891fd758eee6c7819ee8af5442dfd8d45bd65e56d4228c",
    ),
    (
        "linux-aarch64",
        "linux-arm64.tar.gz",
        "b7e7991d5407815adfa74a2a9cd4db683552c8ce32ff4a295c22e972849c3f5d",
    ),
];

/// npm で入れる時間の上限（初回は数百 MB を落とすので長めに持つ）。
const INSTALL_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// 失敗した時に画面へ出すログの行数。
const LOG_TAIL_LINES: usize = 6;

/// 今どの段か（画面の「インストール中…」の中身）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupStage {
    /// Node.js を確かめる・落とす。
    Node,
    /// エージェントの CLI を入れる。
    Agent,
    /// ACP アダプタを入れる。
    Adapter,
}

/// 入れられなかった理由（画面に出す 1 行と、ログの最後の数行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupError {
    pub message: String,
    pub log_tail: Vec<String>,
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Node.js の公式の配布物 1 つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeArchive {
    /// ファイル名（`node-v24.22.0-win-x64.zip`）。
    pub file: String,
    /// 中の最上位のフォルダ（`node-v24.22.0-win-x64`）。
    pub folder: String,
    pub sha256: &'static str,
}

impl NodeArchive {
    pub fn url(&self) -> String {
        format!("https://nodejs.org/dist/v{NODE_VERSION}/{}", self.file)
    }
}

/// このプラットフォームの Node.js の配布物（無い組み合わせは `None`）。
pub fn node_archive() -> Option<NodeArchive> {
    node_archive_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn node_archive_for(os: &str, arch: &str) -> Option<NodeArchive> {
    let key = format!("{os}-{arch}");
    let (_, suffix, sha256) = NODE_ARCHIVES
        .iter()
        .find(|(platform, _, _)| *platform == key)?;
    let file = format!("node-v{NODE_VERSION}-{suffix}");
    let folder = file
        .strip_suffix(".zip")
        .or_else(|| file.strip_suffix(".tar.gz"))?
        .to_string();
    Some(NodeArchive {
        file,
        folder,
        sha256,
    })
}

/// `node --version` の出力（`v24.22.0`）の大きい版。
fn node_major(version: &str) -> Option<u32> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// その node が使える版か（起こして `--version` を聞く・背景で呼ぶ）。
fn node_is_recent(node: &Path) -> bool {
    child_process::command(node)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| node_major(&String::from_utf8_lossy(&output.stdout)))
        .is_some_and(|major| major >= NODE_MIN_MAJOR)
}

/// necoder の道具箱の node（在るとは限らない）。
pub fn managed_node() -> Option<PathBuf> {
    let directory = host::toolchain_search_dirs().into_iter().next()?;
    Some(directory.join(if cfg!(windows) { "node.exe" } else { "node" }))
}

/// necoder の道具箱の npm の置き場（`npm install -g --prefix` の先）。
pub fn npm_prefix() -> Option<PathBuf> {
    Some(paths::toolchain_dir()?.join("npm"))
}

/// 使える node（パソコンの物 → necoder の道具箱の物の順・無ければ `None`）。背景で呼ぶ。
pub fn usable_node() -> Option<PathBuf> {
    host::refresh_search_path();
    find_in_path("node")
        .filter(|node| node_is_recent(node))
        .or_else(|| managed_node().filter(|node| node.is_file() && node_is_recent(node)))
}

/// 設定の「インストール」1 つで入れられる組み込みか（npm で入れる物）。uv（Kimi）や
/// `curl … | bash`（Grok）は従来どおり手順のコマンドをターミナルで流す。
pub fn can_install(kind: &AgentKind) -> bool {
    npm_packages(kind).is_some()
}

/// ログインをターミナル無しで裏で進められるか。**実機で確かめた CLI だけ**（2026-10-19・Windows 11）:
/// `codex login` は手元の受け口（localhost:1455）で自動で終わり、`claude auth login` はブラウザに出た
/// コードを標準入力で受け取る（どちらも端末が無くても動く）。Copilot・Qwen・OpenCode のログインは
/// 対話の画面（TUI）なので従来どおりターミナルで流す。
pub fn can_login_in_background(kind: &AgentKind) -> bool {
    matches!(kind.id, "claude" | "codex")
}

/// `install_cmd`（`npm i -g a b`）の a b。
fn npm_packages(kind: &AgentKind) -> Option<Vec<&'static str>> {
    let packages: Vec<&'static str> = kind
        .install_cmd
        .strip_prefix("npm i -g ")?
        .split_whitespace()
        .collect();
    (!packages.is_empty()).then_some(packages)
}

/// 入れる物の内訳: npm で入れる CLI と、先に置いておく ACP アダプタ（版まで固定・CLI と同じ物なら無し）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallPlan {
    pub cli_packages: Vec<String>,
    pub adapter: Option<(String, String)>,
}

/// 何を入れるかを決める。アダプタが CLI と別の物（Claude Code・Codex）なら、アダプタは起動時に使う
/// 版（レジストリの版・引けなければ組み込みの版）を necoder の置き場へ入れ、CLI の `-g` からは外す
/// （同じ数百 MB を 2 回落とさない）。
pub fn install_plan(kind: &AgentKind, registry: Option<&registry::Registry>) -> Option<InstallPlan> {
    let packages = npm_packages(kind)?;
    let separate_adapter = kind.bin != kind.cli_bin;
    let adapter = separate_adapter
        .then(|| {
            readiness::builtin_registry_npm(kind, registry)
                .map(|(name, version)| format!("{name}@{version}"))
                .or_else(|| kind.package.map(str::to_string))
        })
        .flatten();
    let adapter_name = adapter
        .as_deref()
        .map(|spec| registry::split_npm_spec(spec).0.to_string());
    let cli_packages = packages
        .into_iter()
        .filter(|package| {
            adapter_name.as_deref() != Some(registry::split_npm_spec(package).0)
        })
        .map(str::to_string)
        .collect();
    Some(InstallPlan {
        cli_packages,
        adapter: adapter.map(|spec| (spec, kind.bin.to_string())),
    })
}

/// エージェントを入れる（**blocking・ネットワーク**。背景で呼ぶ）。`stage` は段が変わるたびに呼ぶ。
pub fn install_agent(
    kind: &AgentKind,
    registry: Option<&registry::Registry>,
    stage: &dyn Fn(SetupStage),
) -> std::result::Result<(), SetupError> {
    let log_path = log_path();
    let mut log = open_log(log_path.as_deref());
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    write_log(&mut log, &format!("\n=== {} を入れる（{started}）===", kind.label));
    let result = (|| -> Result<()> {
        let plan = install_plan(kind, registry)
            .with_context(|| format!("{} は自動では入れられない", kind.label))?;
        stage(SetupStage::Node);
        let node = ensure_node(&mut log)?;
        write_log(&mut log, &format!("node: {}", node.display()));
        if !plan.cli_packages.is_empty() {
            stage(SetupStage::Agent);
            install_globally(&node, &plan.cli_packages, &mut log, log_path.as_deref())?;
        }
        if let Some((package, bin)) = &plan.adapter {
            stage(SetupStage::Adapter);
            write_log(&mut log, &format!("アダプタ: {package}"));
            install::preinstall(package, bin, &BTreeMap::new())?;
        }
        // Windows: 道具箱は起動時に PATH へ足してあるが、ついでにレジストリの PATH も読み直す。
        host::refresh_search_path();
        Ok(())
    })();
    match result {
        Ok(()) => {
            write_log(&mut log, "完了");
            Ok(())
        }
        Err(error) => {
            let message = format!("{error:#}");
            write_log(&mut log, &format!("失敗: {message}"));
            drop(log);
            Err(SetupError {
                message,
                log_tail: log_path.as_deref().map(read_tail).unwrap_or_default(),
            })
        }
    }
}

/// 使える node を返す（無ければ necoder の道具箱へ落としてから）。
fn ensure_node(log: &mut Option<std::fs::File>) -> Result<PathBuf> {
    if let Some(node) = usable_node() {
        return Ok(node);
    }
    write_log(log, &format!("Node.js {NODE_VERSION} を落とす"));
    download_node()?;
    let node = managed_node().context("道具箱の置き場が決まらない")?;
    anyhow::ensure!(
        node.is_file(),
        "Node.js を展開したが {} が無い",
        node.display()
    );
    Ok(node)
}

/// 公式の配布物を落とし、SHA-256 を照合してから道具箱の `node/` へ置く（前の物は置き換える）。
fn download_node() -> Result<()> {
    let archive = node_archive().with_context(|| {
        format!(
            "この環境（{}-{}）向けの Node.js を necoder は持っていない",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let toolchain = paths::toolchain_dir().context("道具箱の置き場が決まらない")?;
    std::fs::create_dir_all(&toolchain)
        .with_context(|| format!("{} を作れない", toolchain.display()))?;
    let work = tempfile::Builder::new()
        .prefix("node-")
        .tempdir_in(&toolchain)
        .context("作業用のフォルダを作れない")?;
    let file = work.path().join(&archive.file);
    let url = archive.url();
    deploy::download_with_curl(&url, &file).map_err(anyhow::Error::msg)?;
    let actual = deploy::sha256_file(&file).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        actual == archive.sha256,
        "落とした Node.js の SHA-256 が合わない（期待 {} / 実際 {actual}）。使わずに捨てた",
        archive.sha256
    );
    let unpacked = work.path().join("unpacked");
    std::fs::create_dir_all(&unpacked).context("展開先を作れない")?;
    deploy::extract(&file, &url, &unpacked, Path::new("node")).map_err(anyhow::Error::msg)?;
    let inner = unpacked.join(&archive.folder);
    anyhow::ensure!(
        inner.is_dir(),
        "Node.js の書庫に {} が無い",
        archive.folder
    );
    let target = toolchain.join("node");
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .with_context(|| format!("前の {} を消せない", target.display()))?;
    }
    std::fs::rename(&inner, &target)
        .with_context(|| format!("{} へ置けない", target.display()))?;
    Ok(())
}

/// `npm install -g --prefix <道具箱>/npm <packages>`。出力はログへ。
fn install_globally(
    node: &Path,
    packages: &[String],
    log: &mut Option<std::fs::File>,
    log_path: Option<&Path>,
) -> Result<()> {
    let npm = install::Npm::find(node).context("npm が見つからない")?;
    let prefix = npm_prefix().context("道具箱の置き場が決まらない")?;
    std::fs::create_dir_all(&prefix)
        .with_context(|| format!("{} を作れない", prefix.display()))?;
    write_log(log, &format!("npm install -g {}", packages.join(" ")));
    // npm の後処理（postinstall）や、入れた CLI の起動が使う node を、使っている node にする。
    let search_path = node
        .parent()
        .map(|directory| {
            let current = std::env::var_os("PATH").unwrap_or_default();
            std::env::join_paths(
                std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(&current)),
            )
            .unwrap_or(current)
        })
        .unwrap_or_default();
    let output = |path: Option<&Path>| -> Stdio {
        path.and_then(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        })
        .map(Stdio::from)
        .unwrap_or_else(Stdio::null)
    };
    let mut child = npm
        .command()
        .args(["install", "-g", "--prefix"])
        .arg(&prefix)
        .args(["--no-audit", "--no-fund", "--loglevel=error"])
        .args(packages)
        .env("PATH", search_path)
        .stdin(Stdio::null())
        .stdout(output(log_path))
        .stderr(output(log_path))
        .spawn()
        .context("npm を起こせない")?;
    let deadline = std::time::Instant::now() + INSTALL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Ok(None) => {
                kill_tree(&mut child);
                anyhow::bail!(
                    "npm install が {} 分で終わらなかった",
                    INSTALL_TIMEOUT.as_secs() / 60
                );
            }
            Err(error) => anyhow::bail!("npm の終わりを待てない: {error}"),
        }
    };
    anyhow::ensure!(status.success(), "npm install が失敗した（{status}）");
    Ok(())
}

fn log_path() -> Option<PathBuf> {
    Some(paths::logs_dir()?.join("agent-setup.log"))
}

fn open_log(path: Option<&Path>) -> Option<std::fs::File> {
    let path = path?;
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("セットアップのログの置き場を作れない: {error}");
            return None;
        }
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(file) => Some(file),
        Err(error) => {
            eprintln!("セットアップのログを開けない: {error}");
            None
        }
    }
}

fn write_log(log: &mut Option<std::fs::File>, line: &str) {
    if let Some(file) = log.as_mut() {
        if let Err(error) = writeln!(file, "{line}") {
            eprintln!("セットアップのログに書けない: {error}");
        }
    }
}

/// ログの最後の数行（空行を除く・長すぎる行は切る）。
fn read_tail(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read(path) else {
        return Vec::new();
    };
    tail_lines(&String::from_utf8_lossy(&text), LOG_TAIL_LINES)
}

fn tail_lines(text: &str, count: usize) -> Vec<String> {
    let lines: Vec<String> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take(200).collect())
        .collect();
    lines[lines.len().saturating_sub(count)..].to_vec()
}

/// 子プロセスを子孫ごと止める。Windows の npm の `.cmd` は cmd.exe → node と 2 段になるので、
/// cmd だけを止めると node が残る（出力を持ったまま待ち続ける）。
fn kill_tree(child: &mut Child) {
    if cfg!(windows) {
        let stopped = child_process::command("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Err(error) = stopped {
            eprintln!("taskkill を起こせない: {error}");
        }
    }
    if let Err(error) = child.kill() {
        // 既に終わっている（taskkill が止めた）時もここに来る。
        if error.kind() != std::io::ErrorKind::InvalidInput {
            eprintln!("子プロセスを止められない: {error}");
        }
    }
    if let Err(error) = child.wait() {
        eprintln!("子プロセスの終わりを待てない: {error}");
    }
}

// ── ログイン ─────────────────────────────────────────────────────────────────

/// 裏で動かしている CLI のログイン（`claude auth login` / `codex login` など）。CLI が自分でブラウザを
/// 開き、終われば 0 で終わる。Claude Code はブラウザに出たコードを標準入力で受け取る（[`Self::send_code`]）。
/// 落とす（Drop）と、終わっていなければ止める。
pub struct LoginSession {
    child: Child,
    stdin: Option<ChildStdin>,
    output: Arc<Mutex<String>>,
    finished: Option<bool>,
}

/// その組み込みのログインを裏で始める（このマシンだけ）。
pub fn start_login(kind: &AgentKind) -> Result<LoginSession> {
    host::refresh_search_path();
    let mut words = kind.login_cmd.split_whitespace();
    let program = words.next().context("ログインのコマンドが空")?;
    let path = find_in_path(program).with_context(|| format!("{program} が見つからない"))?;
    let mut child = child_process::command(&path)
        .args(words)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("{} のログインを起こせない", kind.label))?;
    let output = Arc::new(Mutex::new(String::new()));
    for stream in [
        child.stdout.take().map(|out| Box::new(out) as Box<dyn std::io::Read + Send>),
        child.stderr.take().map(|err| Box::new(err) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let output = output.clone();
        std::thread::spawn(move || collect_output(stream, &output));
    }
    let stdin = child.stdin.take();
    Ok(LoginSession {
        child,
        stdin,
        output,
        finished: None,
    })
}

/// 出力を読み続けて溜める（CLI は UTF-8 で書く・途中で切れた文字は置き換える）。
fn collect_output(mut stream: Box<dyn std::io::Read + Send>, output: &Mutex<String>) {
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if let Ok(mut text) = output.lock() {
                    text.push_str(&String::from_utf8_lossy(&buffer[..read]));
                }
            }
        }
    }
}

impl LoginSession {
    /// ここまでの出力。
    pub fn output(&self) -> String {
        self.output
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default()
    }

    /// ブラウザで開く URL（CLI が「開かなければここへ」と出した物・最後の 1 つ）。
    pub fn url(&self) -> Option<String> {
        login_url(&self.output())
    }

    /// コードの貼り付けを待っているか（Claude Code の「Paste code here」）。
    pub fn asks_for_code(&self) -> bool {
        asks_for_code(&self.output())
    }

    /// ブラウザに出たコードを渡す。
    pub fn send_code(&mut self, code: &str) -> Result<()> {
        let stdin = self.stdin.as_mut().context("ログインが入力を受け付けていない")?;
        writeln!(stdin, "{}", code.trim()).context("コードを渡せない")?;
        stdin.flush().context("コードを渡せない")?;
        Ok(())
    }

    /// 終わったか（`Some(true)` = 成功・`Some(false)` = 失敗・`None` = まだ）。
    pub fn poll(&mut self) -> Option<bool> {
        if self.finished.is_none() {
            match self.child.try_wait() {
                Ok(Some(status)) => self.finished = Some(status.success()),
                Ok(None) => {}
                Err(error) => {
                    eprintln!("ログインの終わりを待てない: {error}");
                    self.finished = Some(false);
                }
            }
        }
        self.finished
    }

    /// やめる（子孫ごと止める）。
    pub fn cancel(&mut self) {
        if self.finished.is_none() {
            self.stdin = None;
            kill_tree(&mut self.child);
            self.finished = Some(false);
        }
    }

    /// 失敗の時に出す、出力の最後の数行。
    pub fn output_tail(&self) -> Vec<String> {
        tail_lines(&self.output(), LOG_TAIL_LINES)
    }
}

impl Drop for LoginSession {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// 出力の中の最後の `https://…`（空白・引用符・山括弧で終わる）。
fn login_url(output: &str) -> Option<String> {
    output
        .split(|character: char| character.is_whitespace() || matches!(character, '"' | '\'' | '<' | '>'))
        .filter(|word| word.starts_with("https://"))
        .last()
        .map(|word| word.trim_end_matches(['.', ',', ')']).to_string())
}

fn asks_for_code(output: &str) -> bool {
    output.to_lowercase().contains("paste code")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(id: &str) -> &'static AgentKind {
        AGENTS
            .iter()
            .find(|agent| agent.id == id)
            .expect("組み込みに在る")
    }

    /// 公式の配布物の名前と中のフォルダ（プラットフォームごと）。無い組み合わせは `None`。
    #[test]
    fn node_archives_follow_the_official_names() {
        let windows = node_archive_for("windows", "x86_64").expect("Windows x64");
        assert_eq!(windows.file, format!("node-v{NODE_VERSION}-win-x64.zip"));
        assert_eq!(windows.folder, format!("node-v{NODE_VERSION}-win-x64"));
        assert_eq!(
            windows.url(),
            format!("https://nodejs.org/dist/v{NODE_VERSION}/node-v{NODE_VERSION}-win-x64.zip")
        );
        let mac = node_archive_for("macos", "aarch64").expect("Apple Silicon");
        assert_eq!(mac.folder, format!("node-v{NODE_VERSION}-darwin-arm64"));
        assert!(node_archive_for("freebsd", "x86_64").is_none());
        for (_, _, sha256) in NODE_ARCHIVES {
            assert_eq!(sha256.len(), 64);
            assert!(sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn node_versions_are_read_by_major() {
        assert_eq!(node_major("v24.22.0\n"), Some(24));
        assert_eq!(node_major("v18.0.0"), Some(18));
        assert_eq!(node_major("garbage"), None);
    }

    /// 入れる物の内訳: アダプタが CLI と別の物は、CLI だけを `-g` で入れ、アダプタは版を固定して別に置く。
    /// CLI がそのまま ACP で動く物（Copilot）はアダプタを持たない。uv / curl で入れる物は対象外。
    #[test]
    fn the_install_plan_separates_the_cli_and_the_adapter() {
        let codex = install_plan(kind("codex"), None).expect("npm で入れる");
        assert_eq!(codex.cli_packages, vec!["@openai/codex".to_string()]);
        let (adapter, bin) = codex.adapter.expect("アダプタは別");
        assert!(adapter.starts_with("@agentclientprotocol/codex-acp@"), "{adapter}");
        assert_eq!(bin, "codex-acp");
        let claude = install_plan(kind("claude"), None).expect("npm で入れる");
        assert_eq!(claude.cli_packages, vec!["@anthropic-ai/claude-code".to_string()]);
        let copilot = install_plan(kind("copilot"), None).expect("npm で入れる");
        assert_eq!(copilot.cli_packages, vec!["@github/copilot".to_string()]);
        assert_eq!(copilot.adapter, None);
        assert!(install_plan(kind("kimi"), None).is_none(), "uv で入れる");
        assert!(install_plan(kind("grok"), None).is_none(), "curl で入れる");
        assert!(can_install(kind("claude")));
        assert!(!can_install(kind("kimi")));
    }

    /// ログインの出力から URL を拾い、コードの貼り付けを待っているかを見る（実物の出力の形）。
    #[test]
    fn login_output_gives_the_url_and_whether_a_code_is_wanted() {
        let claude = "Opening browser to sign in…\nIf the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=y\nPaste code here if prompted > ";
        assert_eq!(
            login_url(claude).as_deref(),
            Some("https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=y")
        );
        assert!(asks_for_code(claude));
        let codex = "Starting local login server on http://localhost:1455.\nIf your browser did not open, navigate to this URL to authenticate:\n\nhttps://auth.openai.com/oauth/authorize?response_type=code&state=z\n\nOn a remote or headless machine? Use `codex login --device-auth` instead.";
        assert_eq!(
            login_url(codex).as_deref(),
            Some("https://auth.openai.com/oauth/authorize?response_type=code&state=z"),
            "http の localhost は拾わない・最後の https"
        );
        assert!(!asks_for_code(codex));
        assert_eq!(login_url("no url here"), None);
    }

    /// **ネットワークを使う**（既定では走らない・`cargo test -p acp_client -- --ignored node_is_downloaded`）:
    /// 公式の Node.js を一時の necoder の置き場へ落とし、SHA-256 を照合して展開し、その node と同梱の npm が
    /// 起きることまで確かめる。パソコンに Node.js が無い人の「インストール」の最初の段そのもの。
    #[test]
    #[ignore]
    fn node_is_downloaded_verified_and_runnable() {
        let home = tempfile::tempdir().expect("一時フォルダ");
        std::env::set_var("NECODER_HOME", home.path());
        download_node().expect("落として展開できる");
        let node = managed_node().expect("置き場が決まる");
        assert!(node.starts_with(home.path()), "{}", node.display());
        let version = child_process::command(&node)
            .arg("--version")
            .output()
            .expect("node が起きる");
        assert_eq!(
            String::from_utf8_lossy(&version.stdout).trim(),
            format!("v{NODE_VERSION}")
        );
        let npm = install::Npm::find(&node).expect("同梱の npm が見つかる");
        let npm_version = npm.command().arg("--version").output().expect("npm が起きる");
        assert!(npm_version.status.success(), "{npm_version:?}");
        std::env::remove_var("NECODER_HOME");
    }

    #[test]
    fn the_log_tail_skips_blank_lines() {
        assert_eq!(
            tail_lines("a\n\nb\n  \nc\nd\n", 3),
            vec!["b".to_string(), "c".to_string(), "d".to_string()]
        );
        assert_eq!(tail_lines("", 3), Vec::<String>::new());
    }

    /// ログインを裏で起こし、標準入力で渡したコードを受け取って終わるまで（本物の子プロセス）。
    /// 偽の CLI は「Paste code here」を出して 1 行読み、`ok` なら 0 で終わる。
    #[test]
    fn a_login_session_passes_the_code_and_reports_the_end() {
        let directory = tempfile::tempdir().expect("一時フォルダ");
        // Windows は cmd のバッチ（PowerShell は CI のランナーで起動に 20 秒を超えることがあり、何も出さずに
        // 時間切れになった）。`set /p` が標準入力から 1 行読む（行末が LF だけでも合うよう先頭 2 文字で比べる）。
        let (program, args): (PathBuf, Vec<String>) = if cfg!(windows) {
            let script = directory.path().join("fake-login.cmd");
            std::fs::write(
                &script,
                "@echo off\r\necho visit: https://example.invalid/login\r\necho Paste code here if prompted ^> \r\nset /p code=\r\nif \"%code:~0,2%\"==\"ok\" (exit /b 0) else (exit /b 3)\r\n",
            )
            .expect("書ける");
            (
                PathBuf::from("cmd"),
                vec![
                    "/D".to_string(),
                    "/C".to_string(),
                    script.to_string_lossy().into_owned(),
                ],
            )
        } else {
            (
                PathBuf::from("/bin/sh"),
                vec![
                    "-c".to_string(),
                    "echo 'visit: https://example.invalid/login'; echo 'Paste code here if prompted > '; read code; [ \"$code\" = ok ]".to_string(),
                ],
            )
        };
        let mut child = child_process::command(&program)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("起こせる");
        let output = Arc::new(Mutex::new(String::new()));
        for stream in [
            child.stdout.take().map(|out| Box::new(out) as Box<dyn std::io::Read + Send>),
            child.stderr.take().map(|err| Box::new(err) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let output = output.clone();
            std::thread::spawn(move || collect_output(stream, &output));
        }
        let stdin = child.stdin.take();
        let mut session = LoginSession {
            child,
            stdin,
            output,
            finished: None,
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !session.asks_for_code() {
            assert!(std::time::Instant::now() < deadline, "{}", session.output());
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(session.url().as_deref(), Some("https://example.invalid/login"));
        session.send_code("ok").expect("渡せる");
        let finished = loop {
            if let Some(finished) = session.poll() {
                break finished;
            }
            assert!(std::time::Instant::now() < deadline, "{}", session.output());
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(finished, "{}", session.output());
    }
}
