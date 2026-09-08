//! local/remote 共通の host 境界と Remote SSH wire protocol。
//!
//! `std::fs` / `std::process` を UI・project model から隔離し、同じ API を local process と
//! SSH 上の `necoder-remote-server` へ向ける。設計根拠は
//! `docs/research/remote-ssh-2026.md`。Zed の GPL コードは取り込まず、公開仕様を基に独立実装する。

use anyhow::{anyhow, bail, Context as _, Result};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io::{BufRead as _, BufReader};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

pub const PROTOCOL_VERSION: u16 = 3;
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

const FRAME_MAGIC: [u8; 4] = *b"SHRS";
const FRAME_HEADER_LEN: usize = 28;
const MAX_META_LEN: usize = 8 * 1024 * 1024;
const MAX_BODY_LEN: usize = 256 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SEARCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const COMMAND_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
/// ハンドシェイク（Hello）だけは短く切る。再接続の総待ち時間を有界にするための上限で、
/// 「TCP は生きているが相手が黙っている」（sleep 復帰直後の死んだ ControlMaster 越し）を
/// 30s ではなく 10s で見切る。見切った後は master を作り直して 1 回だけやり直す。
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// 生存確認（Ping）の待ち時間。heartbeat はこれで「切れた」と判定して張り直すので、
/// sleep 復帰から復旧が始まるまでの遅れの上限になる（30s だと復帰のたびに 30s 待たされる）。
/// 短くしても誤爆しないのは、時間切れ時に受信バイトの進みを見て「混んでいるだけ」を
/// 除外するため（[`ReconnectingClient::heartbeat_once`]）。
const PING_TIMEOUT: Duration = Duration::from_secs(5);

/// GUI のメインスレッド id。[`mark_main_thread`] が一度だけ書き込む。
static MAIN_THREAD: std::sync::OnceLock<thread::ThreadId> = std::sync::OnceLock::new();

/// 「このスレッドがメインスレッドだ」と登録する。GUI が窓を開ける直前に 1 回だけ呼ぶ。
///
/// 以後、remote host への blocking request がこのスレッドから飛んだら
/// [`assert_off_main_thread`] が捕まえる。登録しなければ検査は丸ごと無効なので、
/// テスト・CLI・remote-server 側は何も変わらない。
pub fn mark_main_thread() {
    let _already_marked = MAIN_THREAD.set(thread::current().id());
}

/// remote への blocking request が UI スレッドから飛んでいないか検査する。
///
/// [`Host`] は「blocking API なので UI thread では呼ばず background executor へ載せる」という
/// 規約で書かれているが、規約は破られる。実際 2026-09-05 のハングは、プロジェクト切替が
/// UI スレッドから `list_files` を呼び、sleep 復帰直後の SSH 再接続を丸ごと待って
/// **20 秒間アプリが無反応**になったものだった。local では一瞬なので誰も気付けない。
///
/// そこで規約をコードで守らせる。**debug ビルドは panic・release は警告して続行**。
/// `NECODER_STRICT_MAIN_THREAD_IO=1` で release でも panic、`NECODER_ALLOW_MAIN_THREAD_IO=1`
/// で検査自体を止められる。
///
/// 既知の違反は 2026-09-07 までに全部背景へ移した（エクスプローラのツリー再構築・watch の
/// 開きタブ照合・端末/LSP 起動・エージェント遷移の HEAD 取得・エージェント起動のコマンド探索・
/// project を新規に開く経路・分割ペイン・hunk の revert・hot-exit 復元・別窓/別 root で開く
/// `host_for_project`）。release を panic にしないのは、見落としが 1 つあっただけでユーザーの
/// アプリを落とすより、固まって警告を残す方がましだから。
fn assert_off_main_thread(what: &str) {
    if MAIN_THREAD.get() != Some(&thread::current().id()) {
        return;
    }
    if std::env::var_os("NECODER_ALLOW_MAIN_THREAD_IO").is_some() {
        return;
    }
    let message = format!(
        "UI スレッドから remote host の blocking request（{what}）を呼んでいる。\n\
         SSH 再接続を待つ間アプリ全体が固まる。cx.background_executor().spawn(..) へ載せること。"
    );
    let strict =
        cfg!(debug_assertions) || std::env::var_os("NECODER_STRICT_MAIN_THREAD_IO").is_some();
    if strict {
        panic!("{message}");
    }
    eprintln!("[necoder] 警告: {message}");
}

/// Host 内で一意なパスの revision。content hash を含むため、mtime/size が同じ外部変更も検出する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRevision {
    pub len: u64,
    pub modified_ns: u128,
    pub content_hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContent {
    pub bytes: Vec<u8>,
    pub revision: FileRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteCondition {
    /// 既存状態を問わない。明示的な overwrite/save-as 用。
    Any,
    /// 新規作成。既に存在したら conflict。
    NotExists,
    /// 読み込み時の revision と一致するときだけ保存。
    Matches(FileRevision),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMetadata {
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub len: u64,
    pub modified_ns: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
}

/// POSIX シェルで 1 行スクリプトを流す spec。**リモートホストは常にこれ**（相手は Linux）。
pub fn posix_shell_script(script: &str, cwd: &Path) -> CommandSpec {
    CommandSpec::new("sh", cwd).args(["-c", script])
}

/// Windows の `cmd.exe` で 1 行スクリプトを流す spec。**ローカルが Windows のときだけ**。
pub fn windows_shell_script(script: &str, cwd: &Path) -> CommandSpec {
    CommandSpec::new("cmd.exe", cwd).args(["/C", script])
}

impl CommandSpec {
    pub fn new(program: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: HashMap::new(),
        }
    }

    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// 環境変数を積む（子プロセスの env に足す。既存の env は消さない）。
    pub fn envs(
        mut self,
        env: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.env.extend(
            env.into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// LSP/ACP など長寿命 stdio process。Drop 時に child と SSH session を確実に畳む。
pub struct HostProcess {
    child: Child,
    stdin: Option<Box<dyn Write + Send>>,
    stdout: Option<Box<dyn Read + Send>>,
    _transport: Option<Arc<SshTransport>>,
}

impl HostProcess {
    pub fn take_stdin(&mut self) -> Result<Box<dyn Write + Send>> {
        self.stdin.take().context("process stdin は既に取得済み")
    }

    pub fn take_stdout(&mut self) -> Result<Box<dyn Read + Send>> {
        self.stdout.take().context("process stdout は既に取得済み")
    }

    /// プロセスがまだ生きているか。再接続後の handle 再同期で「死んでいたら再 spawn」の判定に使う
    /// （remote では ControlMaster が落ちると子 ssh セッションが終了する＝ここで検知できる・M13）。
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        let _kill = self.child.kill();
        let _wait = self.child.wait();
    }
}

/// remote watch の購読ハンドル（M13）。`recv_*` で「変更のあった project 相対パス列」を受ける。
/// drop すると監視が止まり keeper スレッドが終了する。local host では使わない（notify watch を使う）。
pub struct HostWatch {
    receiver: mpsc::Receiver<Vec<PathBuf>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl HostWatch {
    /// 変更通知を timeout 付きで待つ（daemon の poll 差分・相対パス列）。
    ///
    /// `Timeout` と `Disconnected` を呼び出し側が区別できるよう、`mpsc` の結果をそのまま返す。
    /// 購読が入れ替わって sender が drop された後、切断を「変更無し」に丸めると
    /// `recv_timeout` が即時エラー→再試行の busy loop になるため。
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<Vec<PathBuf>, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    /// 非ブロッキングで溜まった通知を 1 つ取る。
    pub fn try_recv(&self) -> Option<Vec<PathBuf>> {
        self.receiver.try_recv().ok()
    }
}

impl Drop for HostWatch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// local PTY の shell、または ControlMaster を使う remote `ssh -tt` command。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLaunch {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSearchSpec {
    pub pattern: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub max_matches: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSearchHit {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    pub line_text: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status_code == Some(0)
    }
}

/// Project/workspace が使う OS 境界。blocking API なので UI thread では呼ばず background executor へ載せる。
pub trait Host: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn is_remote(&self) -> bool;
    /// 永続化に使う project URI。local host は `None`、SSH host は資格情報を含まない URI を返す。
    fn project_uri(&self, _path: &Path) -> Option<String> {
        None
    }
    /// 同じ接続先で別 project root を開く。worktree の別窓表示に使う。
    fn host_for_project(&self, path: &Path) -> Result<Arc<dyn Host>>;
    fn canonicalize(&self, path: &Path) -> Result<PathBuf>;
    fn metadata(&self, path: &Path) -> Result<HostMetadata>;
    fn read_dir(&self, path: &Path) -> Result<Vec<HostEntry>>;
    fn read_file(&self, path: &Path) -> Result<FileContent>;
    fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        condition: WriteCondition,
    ) -> Result<FileRevision>;
    fn list_files(&self, root: &Path, limit: usize) -> Result<Vec<PathBuf>>;
    fn search_project(
        &self,
        root: &Path,
        spec: &TextSearchSpec,
        file_limit: usize,
    ) -> Result<Vec<TextSearchHit>>;
    fn run_command(&self, spec: &CommandSpec) -> Result<CommandOutput>;
    /// このホストで 1 行スクリプトを流す [`CommandSpec`] を組む。
    ///
    /// **分岐キーは「このホストの OS」であって `cfg!(target_os)` ではない**
    /// （WINDOWS-PORT.md §D3）。`run_command` は同じ呼び出しがローカルにもリモートにも飛ぶので、
    /// コンパイル先で分岐すると **Windows クライアントからリモート Linux へ `cmd.exe` を送る**
    /// ことになる。既定は POSIX（リモートは常に unix なのでこれで正しい）。
    fn shell_script(&self, script: &str, cwd: &Path) -> CommandSpec {
        posix_shell_script(script, cwd)
    }
    /// このホストで POSIX シェル（`sh` と `head` / `grep` 等）が使えるか。
    ///
    /// **Windows のローカルホストだけが `false`**。リモートは常に Linux なので `true`。
    /// POSIX 構文で組んだスクリプトを流す機能は、実行前にこれを見て「Windows では未対応」と
    /// 明示的に断ること。`cmd.exe` へ素で渡すと不可解なエラーになるだけで誰も得をしない。
    fn has_posix_shell(&self) -> bool {
        true
    }
    fn spawn_process(&self, spec: &CommandSpec) -> Result<HostProcess>;
    fn terminal_launch(&self, cwd: &Path) -> Result<Option<TerminalLaunch>>;
    /// project root の変更監視を開始する（remote SSH のみ実装・M13）。local は `None` を返し、
    /// workspace 側の notify watch を使う。返した [`HostWatch`] を drop すると監視は止まる。
    fn watch(self: Arc<Self>) -> Result<Option<HostWatch>> {
        Ok(None)
    }
    /// いまの接続状態（remote のみ意味を持つ。local は常に [`ConnectionState::Connected`]）。
    /// atomic の読みだけで I/O はしないので UI スレッドから呼んでよい。
    fn connection_state(&self) -> ConnectionState {
        ConnectionState::Connected
    }
    /// 接続状態の変化を購読する（remote のみ。local は `None`）。状態が変わるたびに新しい値が
    /// 届く。受信側が drop したら購読は自然に外れる。
    fn watch_connection(&self) -> Option<mpsc::Receiver<ConnectionState>> {
        None
    }
    /// 今すぐ再接続を試みる（remote のみ。local は no-op）。**呼び出し側をブロックしない**
    /// — 接続は背景スレッドで張り、結果は [`Host::watch_connection`] に流れる。heartbeat は
    /// 到達不能が続くと 60 秒までバックオフするので、「復帰したのに次の試行まで待つ」を
    /// ユーザー操作で短絡するための入口。
    fn reconnect(&self) {}
}

/// remote host の接続状態（statusbar の SSH チップが表示する。M9・2026-09-08）。
///
/// 遷移は [`ReconnectingClient`] が一手に握る: 遅延接続は `Unconnected` で始まり、最初の request
/// または heartbeat で `Connecting` → `Connected`。切断を検知して張り直す間は `Connecting`、
/// 張り直しに失敗すると `Disconnected`（heartbeat がバックオフしながら再試行する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// まだ一度も繋いでいない（起動時の遅延接続）。
    Unconnected,
    /// 接続を張っている最中（初回も再接続も）。
    Connecting,
    /// 繋がっている。
    Connected,
    /// 切れていて、直近の張り直しも失敗した。次の試行を待っている。
    Disconnected,
}

impl ConnectionState {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => ConnectionState::Connecting,
            2 => ConnectionState::Connected,
            3 => ConnectionState::Disconnected,
            _ => ConnectionState::Unconnected,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            ConnectionState::Unconnected => 0,
            ConnectionState::Connecting => 1,
            ConnectionState::Connected => 2,
            ConnectionState::Disconnected => 3,
        }
    }
}

#[derive(Debug, Default)]
pub struct LocalHost;

impl LocalHost {
    pub fn shared() -> Arc<dyn Host> {
        Arc::new(Self)
    }
}

impl Host for LocalHost {
    fn id(&self) -> &str {
        "local"
    }

    fn display_name(&self) -> &str {
        "Local"
    }

    fn is_remote(&self) -> bool {
        false
    }

    /// ローカルは「このプロセスが動いている OS」で決まる＝ここだけが `cfg!` を見てよい場所。
    fn shell_script(&self, script: &str, cwd: &Path) -> CommandSpec {
        if cfg!(windows) {
            windows_shell_script(script, cwd)
        } else {
            posix_shell_script(script, cwd)
        }
    }

    fn has_posix_shell(&self) -> bool {
        !cfg!(windows)
    }

    fn host_for_project(&self, path: &Path) -> Result<Arc<dyn Host>> {
        let path = paths::canonicalize(path)?;
        if !path.is_dir() {
            bail!("project root は directory ではない: {}", path.display());
        }
        Ok(Self::shared())
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        paths::canonicalize(path).with_context(|| format!("パスを解決できない: {}", path.display()))
    }

    fn metadata(&self, path: &Path) -> Result<HostMetadata> {
        metadata_for(path)
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<HostEntry>> {
        let read =
            std::fs::read_dir(path).with_context(|| format!("読めない: {}", path.display()))?;
        let mut entries = Vec::new();
        for entry in read {
            let entry = entry?;
            let file_type = entry.file_type()?;
            entries.push(HostEntry {
                path: entry.path(),
                name: entry.file_name().to_string_lossy().to_string(),
                is_dir: file_type.is_dir(),
                is_symlink: file_type.is_symlink(),
            });
        }
        Ok(entries)
    }

    fn read_file(&self, path: &Path) -> Result<FileContent> {
        read_file_local(path)
    }

    fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        condition: WriteCondition,
    ) -> Result<FileRevision> {
        write_file_local(path, bytes, condition)
    }

    fn list_files(&self, root: &Path, limit: usize) -> Result<Vec<PathBuf>> {
        list_files_local(root, limit)
    }

    fn search_project(
        &self,
        root: &Path,
        spec: &TextSearchSpec,
        file_limit: usize,
    ) -> Result<Vec<TextSearchHit>> {
        search_project_local(root, spec, file_limit)
    }

    fn run_command(&self, spec: &CommandSpec) -> Result<CommandOutput> {
        run_command_local(spec)
    }

    fn spawn_process(&self, spec: &CommandSpec) -> Result<HostProcess> {
        spawn_process_local(spec, None)
    }

    fn terminal_launch(&self, _cwd: &Path) -> Result<Option<TerminalLaunch>> {
        if cfg!(windows) {
            // Windows は alacritty の既定が `powershell` 固定なので、pwsh を優先させる（§W4）。
            return Ok(Some(pick_windows_shell(is_in_path)));
        }
        // unix は alacritty の既定（`$SHELL` / passwd）に任せる
        // ＝ mac の挙動を 1 ミリも変えない（§D8）。
        Ok(None)
    }
}

/// Windows の既定シェルを選ぶ。**`pwsh`（PowerShell 7）を優先**し、無ければ OS 同梱の `powershell`。
///
/// `alacritty_terminal` の Windows 既定は **`powershell` 固定**（0.26 の `tty/windows/mod.rs`）なので、
/// pwsh を使いたければこちら側から明示的に渡す必要がある。
///
/// 探索を**引数で受け取る**のは、どのプラットフォームでもテストするため（`paths` crate と同じ理由。
/// `#[cfg(windows)]` でテストを切ると Windows 上でしか検証されず、mac / Linux の CI が守れない）。
fn pick_windows_shell(is_available: impl Fn(&str) -> bool) -> TerminalLaunch {
    let program = ["pwsh", "powershell"]
        .into_iter()
        .find(|candidate| is_available(candidate))
        // どちらも PATH に無いのは考えにくいが、その時は alacritty と同じ既定へ倒す
        .unwrap_or("powershell");
    TerminalLaunch {
        program: program.to_string(),
        args: Vec::new(),
    }
}

/// 探すべきファイル名の候補。
///
/// **Windows は拡張子込みでないと見つからない**（PATH 解決が `PATHEXT` に依る）。
/// 例: ACP の `claude` は Windows では `claude.cmd`、rust-analyzer は `rust-analyzer.exe`。
///
/// 既に拡張子が付いていればそのまま。unix は常に 1 候補。
///
/// **`.cmd` / `.bat` は「見つけたあと起動できるか」も気になるが、そちらは問題ない** —
/// Rust の `std::process::Command` は `.bat` / `.cmd` を検出して `cmd.exe` 経由で起動する
/// （`cmd_scripts_can_be_spawned_directly` テストで固定済み）。
pub fn executable_names(binary: &str) -> Vec<String> {
    if !cfg!(windows) || Path::new(binary).extension().is_some() {
        return vec![binary.to_string()];
    }
    ["", ".exe", ".cmd", ".bat"]
        .iter()
        .map(|extension| format!("{binary}{extension}"))
        .collect()
}

/// PATH から実行ファイルを探す（Windows は `PATHEXT` 相当の拡張子も試す）。
pub fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names = executable_names(binary);
    std::env::split_paths(&path).find_map(|directory| {
        names
            .iter()
            .map(|name| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// PATH に実行ファイルがあるか。
fn is_in_path(binary: &str) -> bool {
    find_in_path(binary).is_some()
}

fn metadata_for(path: &Path) -> Result<HostMetadata> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("metadata を読めない: {}", path.display()))?;
    Ok(HostMetadata {
        is_file: metadata.is_file(),
        is_dir: metadata.is_dir(),
        is_symlink: metadata.file_type().is_symlink(),
        len: metadata.len(),
        modified_ns: modified_ns(&metadata),
    })
}

fn modified_ns(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn content_hash(bytes: &[u8]) -> u64 {
    // FNV-1a は暗号用途ではなく、外部変更の衝突検知用。高速で実装が固定される。
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn revision_for(metadata: &std::fs::Metadata, bytes: &[u8]) -> FileRevision {
    FileRevision {
        len: metadata.len(),
        modified_ns: modified_ns(metadata),
        content_hash: content_hash(bytes),
    }
}

fn read_file_local(path: &Path) -> Result<FileContent> {
    let bytes =
        std::fs::read(path).with_context(|| format!("ファイルを読めない: {}", path.display()))?;
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("metadata を読めない: {}", path.display()))?;
    Ok(FileContent {
        revision: revision_for(&metadata, &bytes),
        bytes,
    })
}

fn write_file_local(path: &Path, bytes: &[u8], condition: WriteCondition) -> Result<FileRevision> {
    match condition {
        WriteCondition::Any => {}
        WriteCondition::NotExists => {
            if path.exists() {
                bail!("保存競合: 既に存在する: {}", path.display());
            }
        }
        WriteCondition::Matches(expected) => {
            // 外部削除は競合にしない（上書きで壊す相手が居ない）: ⌘S で作り直し = 未保存の
            // 作業を救出できる（git checkout がファイルを消した最中の保存など。VSCode 同挙動）。
            // 読めない（権限等・存在はする）場合は判定不能 → 上書きしない側に倒す。
            if path.exists() {
                let current = read_file_local(path)
                    .with_context(|| format!("保存競合の判定に失敗: {}", path.display()))?;
                if current.revision != expected {
                    bail!("保存競合: 外部で変更されている: {}", path.display());
                }
            }
        }
    }

    let parent = path.parent().context("保存先に親ディレクトリが無い")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("保存先ディレクトリを作れない: {}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("buffer");
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
    let temp = parent.join(format!(
        ".{name}.necoder-{}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .with_context(|| format!("一時ファイルを作れない: {}", temp.display()))?;
        if let Ok(metadata) = std::fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("一時ファイルを保存先へ置換できない: {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _cleanup = std::fs::remove_file(&temp);
    }
    result?;
    read_file_local(path).map(|content| content.revision)
}

fn list_files_local(root: &Path, limit: usize) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for result in walker {
        if files.len() >= limit {
            break;
        }
        let entry = result?;
        if entry.file_type().is_some_and(|kind| kind.is_file()) {
            files.push(entry.into_path());
        }
    }
    files.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
    Ok(files)
}

/// remote watch のスナップショット: gitignore 準拠でファイルの (mtime_ns, len) を集める（`.git` は除外）。
/// list_files_local と同じ走査で、差分検出のために mtime/len を持つ点だけ違う。
fn watch_snapshot(root: &Path) -> HashMap<PathBuf, (u128, u64)> {
    let mut snapshot = HashMap::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for result in walker {
        if snapshot.len() >= WATCH_SNAPSHOT_LIMIT {
            break;
        }
        let Ok(entry) = result else { continue };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|delta| delta.as_nanos())
            .unwrap_or(0);
        snapshot.insert(entry.into_path(), (mtime, meta.len()));
    }
    snapshot
}

/// 2 スナップショットの差分（追加/変更/削除）を root 相対パスで返す（上限内）。
fn watch_diff(
    old: &HashMap<PathBuf, (u128, u64)>,
    new: &HashMap<PathBuf, (u128, u64)>,
    root: &Path,
) -> Vec<PathBuf> {
    let mut changed = Vec::new();
    for (path, stamp) in new {
        if old.get(path) != Some(stamp) {
            if let Ok(relative) = path.strip_prefix(root) {
                changed.push(relative.to_path_buf());
            }
        }
    }
    for path in old.keys() {
        if !new.contains_key(path) {
            if let Ok(relative) = path.strip_prefix(root) {
                changed.push(relative.to_path_buf());
            }
        }
    }
    changed.truncate(WATCH_EVENT_PATH_LIMIT);
    changed
}

/// daemon 側の watch マネージャへの制御メッセージ（接続スコープ内で 1 スレッドが受ける）。
enum WatchControl {
    Start { root: PathBuf },
    Stop,
}

fn search_project_local(
    root: &Path,
    spec: &TextSearchSpec,
    file_limit: usize,
) -> Result<Vec<TextSearchHit>> {
    if spec.pattern.is_empty() || spec.max_matches == 0 {
        return Ok(Vec::new());
    }
    let pattern = if spec.is_regex {
        spec.pattern.clone()
    } else {
        regex::escape(&spec.pattern)
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(!spec.case_sensitive)
        .build()
        .context("検索パターンが不正")?;
    let mut hits = Vec::new();
    for path in list_files_local(root, file_limit)? {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let mut line_start = 0usize;
        for (line, raw_line) in text.split_inclusive('\n').enumerate() {
            let content = raw_line
                .strip_suffix('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line))
                .unwrap_or(raw_line);
            for found in regex.find_iter(content) {
                hits.push(TextSearchHit {
                    path: path.clone(),
                    line,
                    column: found.start(),
                    byte_start: line_start + found.start(),
                    byte_end: line_start + found.end(),
                    line_text: content.to_string(),
                });
                if hits.len() >= spec.max_matches {
                    return Ok(hits);
                }
            }
            line_start += raw_line.len();
        }
    }
    Ok(hits)
}

fn run_command_local(spec: &CommandSpec) -> Result<CommandOutput> {
    let output = Command::new(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(&spec.env)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("process を起動できない: {}", spec.program))?;
    Ok(CommandOutput {
        status_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn spawn_process_local(
    spec: &CommandSpec,
    transport: Option<Arc<SshTransport>>,
) -> Result<HostProcess> {
    let mut child = Command::new(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(&spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("process を起動できない: {}", spec.program))?;
    let stdin = child.stdin.take().context("process stdin が無い")?;
    let stdout = child.stdout.take().context("process stdout が無い")?;
    Ok(HostProcess {
        child,
        stdin: Some(Box::new(stdin)),
        stdout: Some(Box::new(stdout)),
        _transport: transport,
    })
}

// ── Wire protocol ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum FrameKind {
    Request = 1,
    Response = 2,
    Error = 3,
    Event = 4,
}

impl FrameKind {
    fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            1 => Ok(Self::Request),
            2 => Ok(Self::Response),
            3 => Ok(Self::Error),
            4 => Ok(Self::Event),
            _ => bail!("unknown frame kind: {byte}"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum Request {
    Hello {
        client_version: String,
    },
    OpenProject {
        path: PathBuf,
    },
    Canonicalize {
        project_id: u64,
        path: PathBuf,
    },
    Metadata {
        project_id: u64,
        path: PathBuf,
    },
    ReadDir {
        project_id: u64,
        path: PathBuf,
    },
    ReadFile {
        project_id: u64,
        path: PathBuf,
    },
    WriteFile {
        project_id: u64,
        path: PathBuf,
        condition: WriteCondition,
    },
    ListFiles {
        project_id: u64,
        root: PathBuf,
        limit: usize,
    },
    SearchProject {
        project_id: u64,
        root: PathBuf,
        spec: TextSearchSpec,
        file_limit: usize,
    },
    RunCommand {
        project_id: u64,
        spec: CommandSpec,
    },
    /// project root の変更監視を開始する。daemon は以後 [`FrameKind::Event`] frame を push する
    /// （remote watch・M13）。接続が切れると監視も自然終了し、再接続後にクライアントが再送する。
    Watch {
        project_id: u64,
        root: PathBuf,
    },
    /// 変更監視を止める。
    Unwatch {
        project_id: u64,
    },
    Ping,
    Shutdown,
}

impl Request {
    /// 診断メッセージ用の短い名前（UI スレッド違反の指摘に「どの request か」を載せる）。
    fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "Hello",
            Self::OpenProject { .. } => "OpenProject",
            Self::Canonicalize { .. } => "Canonicalize",
            Self::Metadata { .. } => "Metadata",
            Self::ReadDir { .. } => "ReadDir",
            Self::ReadFile { .. } => "ReadFile",
            Self::WriteFile { .. } => "WriteFile",
            Self::ListFiles { .. } => "ListFiles",
            Self::SearchProject { .. } => "SearchProject",
            Self::RunCommand { .. } => "RunCommand",
            Self::Watch { .. } => "Watch",
            Self::Unwatch { .. } => "Unwatch",
            Self::Ping => "Ping",
            Self::Shutdown => "Shutdown",
        }
    }

    fn timeout(&self) -> Duration {
        match self {
            Self::SearchProject { .. } => SEARCH_REQUEST_TIMEOUT,
            Self::RunCommand { .. } => COMMAND_REQUEST_TIMEOUT,
            // 再接続の一段目。ここを 30s 待つと復旧が体感で「固まった」になる。
            Self::Hello { .. } => HELLO_TIMEOUT,
            // 生存確認。切断の検知がこの長さで決まる。
            Self::Ping => PING_TIMEOUT,
            _ => REQUEST_TIMEOUT,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum Response {
    Hello {
        protocol_version: u16,
        server_version: String,
        os: String,
        arch: String,
        capabilities: Vec<String>,
    },
    ProjectOpened {
        project_id: u64,
        root: PathBuf,
    },
    Path(PathBuf),
    Metadata(HostMetadata),
    Entries(Vec<HostEntry>),
    File {
        revision: FileRevision,
    },
    Written {
        revision: FileRevision,
    },
    Paths(Vec<PathBuf>),
    SearchHits(Vec<TextSearchHit>),
    Command {
        status_code: Option<i32>,
        stdout_len: usize,
        stderr_len: usize,
    },
    Pong,
    Ack,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireError {
    code: String,
    message: String,
}

/// daemon → client の push イベント（[`FrameKind::Event`] frame の meta）。remote watch の変更通知。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WatchEvent {
    /// 変更/追加/削除のあった project 相対パス（1 イベントの上限内に収める）。
    paths: Vec<PathBuf>,
}

/// remote watch が 1 イベントで運ぶ相対パスの上限（frame を肥大させない）。
const WATCH_EVENT_PATH_LIMIT: usize = 512;
/// remote watch のポーリング間隔（daemon 側・SSH 越しなので数百 ms で十分）。
const WATCH_POLL_INTERVAL: Duration = Duration::from_millis(700);
/// remote watch が 1 周でスナップショットするファイル数の上限（巨大 tree の暴走防止）。
const WATCH_SNAPSHOT_LIMIT: usize = 50_000;

#[derive(Debug)]
struct RemoteRequestError {
    code: String,
    message: String,
}

impl std::fmt::Display for RemoteRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "remote {}: {}", self.code, self.message)
    }
}

impl std::error::Error for RemoteRequestError {}

/// request が時間切れになった（相手から何も返らなかった）。「接続が閉じた」と区別するための型。
/// [`SshConnector::connect`] は Hello が**これで**落ちたときだけ ControlMaster を作り直す
/// （閉じたのなら master は生きていて、落ちたのは remote-server 側）。
/// [`ReconnectingClient`] はこれを受けたとき受信バイトの進みを見て「混んでいるだけ」を除外する。
#[derive(Debug)]
struct RequestTimeout {
    request: &'static str,
    after: Duration,
}

impl std::fmt::Display for RequestTimeout {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "remote request timeout after {}s ({}): 相手から応答が無い",
            self.after.as_secs_f64(),
            self.request
        )
    }
}

impl std::error::Error for RequestTimeout {}

#[derive(Debug)]
struct Frame {
    kind: FrameKind,
    id: u64,
    meta: Vec<u8>,
    body: Vec<u8>,
}

impl Frame {
    fn request(id: u64, request: &Request, body: Vec<u8>) -> Result<Self> {
        Ok(Self {
            kind: FrameKind::Request,
            id,
            meta: serde_json::to_vec(request)?,
            body,
        })
    }

    fn response(id: u64, response: &Response, body: Vec<u8>) -> Result<Self> {
        Ok(Self {
            kind: FrameKind::Response,
            id,
            meta: serde_json::to_vec(response)?,
            body,
        })
    }

    fn error(id: u64, code: &str, error: &anyhow::Error) -> Result<Self> {
        let error = WireError {
            code: code.to_string(),
            message: format!("{error:#}"),
        };
        Ok(Self {
            kind: FrameKind::Error,
            id,
            meta: serde_json::to_vec(&error)?,
            body: Vec::new(),
        })
    }
}

fn write_frame(writer: &mut dyn Write, frame: &Frame) -> Result<()> {
    if frame.meta.len() > MAX_META_LEN {
        bail!("frame metadata too large: {}", frame.meta.len());
    }
    if frame.body.len() > MAX_BODY_LEN {
        bail!("frame body too large: {}", frame.body.len());
    }
    writer.write_all(&FRAME_MAGIC)?;
    writer.write_all(&PROTOCOL_VERSION.to_le_bytes())?;
    writer.write_all(&[frame.kind as u8, 0])?;
    writer.write_all(&frame.id.to_le_bytes())?;
    writer.write_all(&(frame.meta.len() as u32).to_le_bytes())?;
    writer.write_all(&(frame.body.len() as u64).to_le_bytes())?;
    writer.write_all(&frame.meta)?;
    writer.write_all(&frame.body)?;
    writer.flush()?;
    Ok(())
}

fn read_frame(reader: &mut dyn Read) -> Result<Frame> {
    let mut fixed = [0u8; FRAME_HEADER_LEN];
    reader.read_exact(&mut fixed)?;
    if fixed[0..4] != FRAME_MAGIC {
        bail!("invalid frame magic");
    }
    let version = u16::from_le_bytes([fixed[4], fixed[5]]);
    if version != PROTOCOL_VERSION {
        bail!("protocol version mismatch: peer={version}, client={PROTOCOL_VERSION}");
    }
    let kind = FrameKind::from_byte(fixed[6])?;
    let id = u64::from_le_bytes(fixed[8..16].try_into()?);
    let meta_len = u32::from_le_bytes(fixed[16..20].try_into()?) as usize;
    let body_len_u64 = u64::from_le_bytes(fixed[20..28].try_into()?);
    let body_len = usize::try_from(body_len_u64).context("frame body length does not fit usize")?;
    if meta_len > MAX_META_LEN {
        bail!("frame metadata too large: {meta_len}");
    }
    if body_len > MAX_BODY_LEN {
        bail!("frame body too large: {body_len}");
    }
    let mut meta = vec![0; meta_len];
    reader.read_exact(&mut meta)?;
    let mut body = vec![0; body_len];
    reader.read_exact(&mut body)?;
    Ok(Frame {
        kind,
        id,
        meta,
        body,
    })
}

// ── Remote server ──

struct ServerState {
    projects: HashMap<u64, PathBuf>,
    next_project_id: u64,
    shutdown: bool,
}

impl ServerState {
    fn new() -> Self {
        Self {
            projects: HashMap::new(),
            next_project_id: 1,
            shutdown: false,
        }
    }

    fn root(&self, project_id: u64) -> Result<&Path> {
        self.projects
            .get(&project_id)
            .map(PathBuf::as_path)
            .ok_or_else(|| anyhow!("unknown project id: {project_id}"))
    }
}

type SharedServerState = Arc<Mutex<ServerState>>;

fn server_root(state: &SharedServerState, project_id: u64) -> Result<PathBuf> {
    state
        .lock()
        .map_err(|_| anyhow!("server state lock poisoned"))?
        .root(project_id)
        .map(Path::to_path_buf)
}

fn server_resolve_existing(
    state: &SharedServerState,
    project_id: u64,
    relative: &Path,
) -> Result<PathBuf> {
    validate_relative_path(relative)?;
    let root = server_root(state, project_id)?;
    let resolved = paths::canonicalize(root.join(relative))?;
    if !resolved.starts_with(&root) {
        bail!("path escapes project root: {}", relative.display());
    }
    Ok(resolved)
}

fn server_resolve_write(
    state: &SharedServerState,
    project_id: u64,
    relative: &Path,
) -> Result<PathBuf> {
    validate_relative_path(relative)?;
    let root = server_root(state, project_id)?;
    let joined = root.join(relative);
    let parent = joined.parent().context("write path has no parent")?;
    let parent = paths::canonicalize(parent)?;
    if !parent.starts_with(&root) {
        bail!("path escapes project root: {}", relative.display());
    }
    Ok(parent.join(joined.file_name().context("write path has no file name")?))
}

fn server_relative(state: &SharedServerState, project_id: u64, path: &Path) -> Result<PathBuf> {
    Ok(path
        .strip_prefix(server_root(state, project_id)?)?
        .to_path_buf())
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("path must be project-relative: {}", path.display())
            }
        }
    }
    Ok(())
}

fn handle_request(
    state: &SharedServerState,
    request: Request,
    body: Vec<u8>,
) -> Result<(Response, Vec<u8>)> {
    match request {
        Request::Hello { .. } => Ok((
            Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                server_version: SERVER_VERSION.to_string(),
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                capabilities: vec![
                    "fs-v1".to_string(),
                    "atomic-write-v1".to_string(),
                    "process-output-v1".to_string(),
                    "search-v1".to_string(),
                    "session-daemon-v1".to_string(),
                ],
            },
            Vec::new(),
        )),
        Request::OpenProject { path } => {
            let root = paths::canonicalize(&path)
                .with_context(|| format!("project root を解決できない: {}", path.display()))?;
            if !root.is_dir() {
                bail!("project root は directory ではない: {}", root.display());
            }
            let mut state = state
                .lock()
                .map_err(|_| anyhow!("server state lock poisoned"))?;
            let project_id = state.next_project_id;
            state.next_project_id += 1;
            state.projects.insert(project_id, root.clone());
            Ok((Response::ProjectOpened { project_id, root }, Vec::new()))
        }
        Request::Canonicalize { project_id, path } => {
            let path = server_resolve_existing(state, project_id, &path)?;
            let path = server_relative(state, project_id, &path)?;
            Ok((Response::Path(path), Vec::new()))
        }
        Request::Metadata { project_id, path } => {
            let path = server_resolve_existing(state, project_id, &path)?;
            Ok((Response::Metadata(metadata_for(&path)?), Vec::new()))
        }
        Request::ReadDir { project_id, path } => {
            let path = server_resolve_existing(state, project_id, &path)?;
            let mut entries = LocalHost.read_dir(&path)?;
            for entry in &mut entries {
                entry.path = server_relative(state, project_id, &entry.path)?;
            }
            Ok((Response::Entries(entries), Vec::new()))
        }
        Request::ReadFile { project_id, path } => {
            let path = server_resolve_existing(state, project_id, &path)?;
            let content = read_file_local(&path)?;
            Ok((
                Response::File {
                    revision: content.revision,
                },
                content.bytes,
            ))
        }
        Request::WriteFile {
            project_id,
            path,
            condition,
        } => {
            let path = server_resolve_write(state, project_id, &path)?;
            let revision = write_file_local(&path, &body, condition)?;
            Ok((Response::Written { revision }, Vec::new()))
        }
        Request::ListFiles {
            project_id,
            root,
            limit,
        } => {
            let root = server_resolve_existing(state, project_id, &root)?;
            let mut paths = list_files_local(&root, limit)?;
            for path in &mut paths {
                *path = server_relative(state, project_id, path)?;
            }
            Ok((Response::Paths(paths), Vec::new()))
        }
        Request::SearchProject {
            project_id,
            root,
            spec,
            file_limit,
        } => {
            let root = server_resolve_existing(state, project_id, &root)?;
            let mut hits = search_project_local(&root, &spec, file_limit)?;
            for hit in &mut hits {
                hit.path = server_relative(state, project_id, &hit.path)?;
            }
            Ok((Response::SearchHits(hits), Vec::new()))
        }
        Request::RunCommand {
            project_id,
            mut spec,
        } => {
            spec.cwd = server_resolve_existing(state, project_id, &spec.cwd)?;
            let output = run_command_local(&spec)?;
            let stdout_len = output.stdout.len();
            let stderr_len = output.stderr.len();
            let mut body = output.stdout;
            body.extend(output.stderr);
            Ok((
                Response::Command {
                    status_code: output.status_code,
                    stdout_len,
                    stderr_len,
                },
                body,
            ))
        }
        // Watch/Unwatch は serve ループが watch マネージャへ回すのでここには来ない（handle_request は
        // 無状態でこの接続の watch チャネルを持たない）。万一直接届いても panic させず拒否する。
        Request::Watch { .. } | Request::Unwatch { .. } => {
            bail!("watch/unwatch は接続スコープで処理する（handle_request では未対応）")
        }
        Request::Ping => Ok((Response::Pong, Vec::new())),
        Request::Shutdown => {
            state
                .lock()
                .map_err(|_| anyhow!("server state lock poisoned"))?
                .shutdown = true;
            Ok((Response::Ack, Vec::new()))
        }
    }
}

fn serve_stream(reader: impl Read, writer: impl Write + Send) -> Result<()> {
    serve_stream_with_state(reader, writer, Arc::new(Mutex::new(ServerState::new())))
}

fn serve_stream_with_state(
    mut reader: impl Read,
    mut writer: impl Write + Send,
    state: SharedServerState,
) -> Result<()> {
    const SERVER_WORKERS: usize = 8;
    const REQUEST_QUEUE: usize = 32;

    thread::scope(|scope| -> Result<()> {
        let (request_tx, request_rx) = mpsc::sync_channel::<Frame>(REQUEST_QUEUE);
        let request_rx = Arc::new(Mutex::new(request_rx));
        let writer = Arc::new(Mutex::new(&mut writer));
        // remote watch（M13）: この接続スコープに 1 本の poll マネージャ。workers が Watch/Unwatch を
        // 制御チャネルで送り、マネージャが差分を [`FrameKind::Event`] frame として共有 writer へ push する。
        // 接続が切れて workers が終わると watch_tx が全て drop → マネージャも Disconnected で終了する。
        // watch 未使用の間は recv() で完全ブロック＝idle 0%。使用中だけ POLL 間隔で起きる。
        let (watch_tx, watch_rx) = mpsc::channel::<WatchControl>();
        {
            let writer = writer.clone();
            scope.spawn(move || {
                let mut watching: Option<(PathBuf, HashMap<PathBuf, (u128, u64)>)> = None;
                loop {
                    let control = if watching.is_some() {
                        match watch_rx.recv_timeout(WATCH_POLL_INTERVAL) {
                            Ok(control) => Some(control),
                            Err(mpsc::RecvTimeoutError::Timeout) => None, // → poll 1 周
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match watch_rx.recv() {
                            Ok(control) => Some(control),
                            Err(_) => break, // 接続終了
                        }
                    };
                    match control {
                        Some(WatchControl::Start { root }) => {
                            watching = Some((root.clone(), watch_snapshot(&root)));
                        }
                        Some(WatchControl::Stop) => watching = None,
                        None => {
                            if let Some((root, previous)) = watching.as_mut() {
                                let fresh = watch_snapshot(root);
                                let changed = watch_diff(previous, &fresh, root);
                                *previous = fresh;
                                if !changed.is_empty() {
                                    if let Ok(meta) =
                                        serde_json::to_vec(&WatchEvent { paths: changed })
                                    {
                                        let frame = Frame {
                                            kind: FrameKind::Event,
                                            id: 0,
                                            meta,
                                            body: Vec::new(),
                                        };
                                        if let Ok(mut writer) = writer.lock() {
                                            let _written = write_frame(&mut **writer, &frame);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
        }
        for worker in 0..SERVER_WORKERS {
            let request_rx = request_rx.clone();
            let writer = writer.clone();
            let state = state.clone();
            let watch_tx = watch_tx.clone();
            scope.spawn(move || {
                loop {
                    let frame = match request_rx.lock() {
                        Ok(receiver) => receiver.recv(),
                        Err(_) => return,
                    };
                    let Ok(frame) = frame else {
                        return;
                    };
                    let parsed = serde_json::from_slice::<Request>(&frame.meta)
                        .context("invalid request metadata");
                    let response = match parsed {
                        // Watch/Unwatch は watch マネージャへ回して Ack（handle_request は無状態のため）。
                        Ok(Request::Watch { project_id, root }) => {
                            match server_resolve_existing(&state, project_id, &root) {
                                Ok(absolute) => {
                                    let _sent =
                                        watch_tx.send(WatchControl::Start { root: absolute });
                                    Ok((Response::Ack, Vec::new()))
                                }
                                Err(error) => Err(error),
                            }
                        }
                        Ok(Request::Unwatch { .. }) => {
                            let _sent = watch_tx.send(WatchControl::Stop);
                            Ok((Response::Ack, Vec::new()))
                        }
                        Ok(request) => handle_request(&state, request, frame.body),
                        Err(error) => Err(error),
                    };
                    let response = match response {
                        Ok((response, body)) => Frame::response(frame.id, &response, body),
                        Err(error) => Frame::error(frame.id, "request_failed", &error),
                    };
                    let write_result = response.and_then(|response| {
                        let mut writer = writer
                            .lock()
                            .map_err(|_| anyhow!("remote writer lock poisoned"))?;
                        write_frame(&mut **writer, &response)
                    });
                    if let Err(error) = write_result {
                        remote_trace(&format!(
                            "server worker {worker}: response write failed: {error:#}"
                        ));
                        return;
                    }
                }
            });
        }
        // 親（read ループ）は watch を送らない。workers の clone だけが manager を生かす。
        drop(watch_tx);

        loop {
            remote_trace("server: waiting frame");
            let frame = match read_frame(&mut reader) {
                Ok(frame) => frame,
                Err(error)
                    if error.downcast_ref::<std::io::Error>().is_some_and(|io| {
                        matches!(
                            io.kind(),
                            std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe
                        )
                    }) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            };
            remote_trace("server: received frame");
            if frame.kind != FrameKind::Request {
                bail!("server expected request frame");
            }
            let should_shutdown = serde_json::from_slice::<Request>(&frame.meta)
                .is_ok_and(|request| matches!(request, Request::Shutdown));
            request_tx
                .send(frame)
                .map_err(|_| anyhow!("remote request worker queue closed"))?;
            if should_shutdown {
                break;
            }
        }
        drop(request_tx);
        Ok(())
    })
}

#[cfg(unix)]
fn session_socket(session: &str) -> Result<PathBuf> {
    if session.len() != 64 || !session.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("session id が不正");
    }
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|uid| !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit()))
        .context("remote uid を取得できない")?;
    // macOS の SUN_LEN は短い。owner-only directory + 160-bit socket name で安全性と長さを両立する。
    Ok(PathBuf::from(format!(
        "/tmp/necoder-remote-{uid}-{SERVER_VERSION}-p{PROTOCOL_VERSION}"
    ))
    .join(format!("{}.sock", &session[..40])))
}

#[cfg(unix)]
fn serve_daemon(socket: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::net::UnixListener;
    use std::time::Instant;

    let parent = socket.parent().context("session socket に親が無い")?;
    std::fs::create_dir_all(parent)?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    if socket.exists() {
        let _stale = std::fs::remove_file(socket);
    }
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("session socket を bind できない: {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let state = Arc::new(Mutex::new(ServerState::new()));
    let mut idle_since = Instant::now();
    let result = loop {
        match listener.accept() {
            Ok((stream, _)) => {
                remote_trace("daemon: accepted proxy");
                stream.set_nonblocking(false)?;
                let reader = stream.try_clone()?;
                if let Err(error) = serve_stream_with_state(reader, stream, state.clone()) {
                    eprintln!("remote proxy session error: {error:#}");
                }
                if state
                    .lock()
                    .map_err(|_| anyhow!("server state lock poisoned"))?
                    .shutdown
                {
                    break Ok(());
                }
                idle_since = Instant::now();
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if idle_since.elapsed() >= Duration::from_secs(600) {
                    break Ok(());
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => break Err(error.into()),
        }
    };
    let _cleanup = std::fs::remove_file(socket);
    result
}

#[cfg(unix)]
fn connect_session_proxy(session: &str) -> Result<()> {
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;

    let socket = session_socket(session)?;
    let mut stream = match UnixStream::connect(&socket) {
        Ok(stream) => {
            remote_trace("proxy: connected existing daemon");
            stream
        }
        Err(_) => {
            let executable =
                std::env::current_exe().context("remote server executable path が無い")?;
            Command::new(executable)
                .arg("daemon")
                .arg("--socket")
                .arg(&socket)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .context("remote session daemon を起動できない")?;
            let mut connected = None;
            for _ in 0..100 {
                match UnixStream::connect(&socket) {
                    Ok(stream) => {
                        connected = Some(stream);
                        break;
                    }
                    Err(_) => thread::sleep(Duration::from_millis(20)),
                }
            }
            let stream = connected.context("remote session daemon に接続できない")?;
            remote_trace("proxy: connected new daemon");
            stream
        }
    };
    let mut input_stream = stream.try_clone()?;
    thread::Builder::new()
        .name("necoder-proxy-input".to_string())
        .spawn(move || {
            remote_trace("proxy: input copy start");
            let mut stdin = std::io::stdin().lock();
            let _copied = std::io::copy(&mut stdin, &mut input_stream);
            remote_trace("proxy: input copy end");
            let _shutdown = input_stream.shutdown(Shutdown::Write);
        })?;
    let mut stdout = std::io::stdout().lock();
    remote_trace("proxy: output copy start");
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        stdout.write_all(&buffer[..read])?;
        // stdout は端末向け LineWriter の場合がある。binary frame は改行を含まないので毎回 flush。
        stdout.flush()?;
    }
    remote_trace("proxy: output copy end");
    Ok(())
}

/// Remote SSH の統合ターミナルに置く軽量 `ne`。`NECODER_GUI_SOCK` は SSH reverse forward
/// の接続先であり、その先のローカル gateway が `open` 以外を拒否して接続先 identity を付ける。
#[cfg(unix)]
fn run_remote_cli(args: &[String]) -> Result<()> {
    use std::os::unix::net::UnixStream;

    match args.first().map(String::as_str) {
        Some("-h") | Some("--help") => {
            println!("使い方: ne [<path>]...");
            println!("  Remote SSH の接続元 necoder でパスを開きます");
            return Ok(());
        }
        Some("-V") | Some("--version") => {
            println!("necoder {SERVER_VERSION}");
            return Ok(());
        }
        _ => {}
    }
    let cwd = std::env::current_dir().context("カレントディレクトリが分かりません")?;
    let mut paths = Vec::new();
    let mut skipped = Vec::new();
    for argument in args {
        let path = Path::new(argument);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        if !absolute.exists() {
            skipped.push(absolute.display().to_string());
            continue;
        }
        paths.push(
            std::fs::canonicalize(&absolute)
                .unwrap_or(absolute)
                .display()
                .to_string(),
        );
    }
    for path in &skipped {
        eprintln!("見つからない（スキップ）: {path}");
    }
    let socket = std::env::var_os("NECODER_GUI_SOCK")
        .map(PathBuf::from)
        .context("この ne は necoder の Remote SSH ターミナル内でのみ使えます")?;
    let mut stream = UnixStream::connect(&socket)
        .with_context(|| format!("接続元の necoder に接続できません（{}）", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    let request = serde_json::json!({ "method": "open", "params": { "paths": paths } });
    writeln!(stream, "{request}").context("接続元への open 送信に失敗")?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(stream)
        .take((MAX_META_LEN + 1) as u64)
        .read_line(&mut line)
        .context("接続元からの応答を読めません")?;
    anyhow::ensure!(!line.is_empty(), "接続元からの応答が空です");
    anyhow::ensure!(line.len() <= MAX_META_LEN, "接続元からの応答が大きすぎます");
    let response: serde_json::Value =
        serde_json::from_str(&line).context("接続元からの応答が JSON ではありません")?;
    if response.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        bail!(
            "{}",
            response
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("接続元で open に失敗しました")
        );
    }
    let opened = response
        .get("result")
        .and_then(|result| result.get("opened"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if opened == 0 {
        println!("接続元の necoder を前面に出しました");
    } else {
        println!("接続元の necoder で開きました（{opened} 件）");
    }
    Ok(())
}

#[cfg(not(unix))]
fn run_remote_cli(_args: &[String]) -> Result<()> {
    bail!("Remote SSH の ne は Unix remote でのみ使えます")
}

pub fn serve_remote_server_cli() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|command| command == "cli") {
        return run_remote_cli(&args[1..]);
    }
    match args.as_slice() {
        [command] if command == "version" || command == "--version" => {
            println!("{SERVER_VERSION} protocol={PROTOCOL_VERSION}");
            Ok(())
        }
        [command] if command == "serve" => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            serve_stream(stdin.lock(), stdout)
        }
        [command, flag] if command == "serve" && flag == "--stdio" => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            serve_stream(stdin.lock(), stdout)
        }
        #[cfg(unix)]
        [command, flag, session] if command == "proxy" && flag == "--session" => {
            connect_session_proxy(session)
        }
        #[cfg(unix)]
        [command, flag, socket] if command == "daemon" && flag == "--socket" => {
            serve_daemon(Path::new(socket))
        }
        _ => bail!(
            "使い方: necoder-remote-server <serve --stdio | proxy --session ID | cli [PATH]... | version>"
        ),
    }
}

// ── Multiplexed RPC client ──

struct ProcessOwner(Mutex<Option<Child>>);

impl ProcessOwner {
    /// session プロセスを落として回収する。2 回目以降は no-op。
    fn kill(&self) {
        let Ok(mut slot) = self.0.lock() else {
            return;
        };
        if let Some(mut child) = slot.take() {
            let _kill = child.kill();
            let _wait = child.wait();
        }
    }
}

impl Drop for ProcessOwner {
    fn drop(&mut self) {
        self.kill();
    }
}

type PendingResponses = Arc<Mutex<HashMap<u64, mpsc::SyncSender<Result<Frame, String>>>>>;

/// 受信バイト数を数える reader。「request が時間切れになったが接続は生きている（相手が大きな
/// 応答を流している最中で、Pong がその後ろに並んでいるだけ）」を、切断と区別するために使う。
struct CountingReader {
    inner: Box<dyn Read + Send>,
    received: Arc<AtomicU64>,
}

impl Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.received.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

/// remote watch の [`FrameKind::Event`] frame を現在の購読者へ配る共有シンク。RpcClient を跨いで
/// （再接続でも）生き続けるよう [`ReconnectingClient`] が Arc で保持し、各 RpcClient の reader が
/// Event をここへ流す。購読が無ければ捨てる。
#[derive(Default)]
struct WatchEventSink {
    sender: Mutex<Option<(u64, mpsc::Sender<Vec<PathBuf>>)>>,
    next_subscription_id: AtomicU64,
}

impl WatchEventSink {
    /// 新しい購読を現在の配送先にする。戻り値は、古い keeper が新しい
    /// 購読を誤って解除しないための世代 ID。
    fn subscribe(&self, sender: mpsc::Sender<Vec<PathBuf>>) -> u64 {
        let id = self.next_subscription_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut slot) = self.sender.lock() {
            *slot = Some((id, sender));
        }
        id
    }

    fn is_current(&self, id: u64) -> bool {
        self.sender
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|(current, _)| *current == id))
            .unwrap_or(false)
    }

    /// `id` がまだ現在の購読なら外す。古い watch の遅れた Drop で、後から
    /// 開始した watch の sender を drop しない。
    fn clear_if_current(&self, id: u64) -> bool {
        let Ok(mut slot) = self.sender.lock() else {
            return false;
        };
        if slot.as_ref().is_some_and(|(current, _)| *current == id) {
            *slot = None;
            true
        } else {
            false
        }
    }

    fn dispatch(&self, event: WatchEvent) {
        if let Ok(slot) = self.sender.lock() {
            if let Some((_, sender)) = slot.as_ref() {
                let _sent = sender.send(event.paths);
            }
        }
    }
}

struct RpcClient {
    writer: Mutex<Box<dyn Write + Send>>,
    pending: PendingResponses,
    next_request_id: AtomicU64,
    /// この接続で受信した総バイト数（reader スレッドが進める）。
    received: Arc<AtomicU64>,
    owner: Option<Arc<ProcessOwner>>,
}

fn rpc_client_from_command(
    mut command: Command,
    event_sink: Arc<WatchEventSink>,
) -> Result<Arc<RpcClient>> {
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = command
        .spawn()
        .context("remote server process を起動できない")?;
    let stdin = child.stdin.take().context("remote server stdin が無い")?;
    let stdout = child.stdout.take().context("remote server stdout が無い")?;
    let owner = Arc::new(ProcessOwner(Mutex::new(Some(child))));
    Ok(RpcClient::new(
        Box::new(stdout),
        Box::new(stdin),
        Some(owner),
        event_sink,
    ))
}

impl RpcClient {
    fn new(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        owner: Option<Arc<ProcessOwner>>,
        event_sink: Arc<WatchEventSink>,
    ) -> Arc<Self> {
        let pending = Arc::new(Mutex::new(HashMap::<
            u64,
            mpsc::SyncSender<Result<Frame, String>>,
        >::new()));
        let received = Arc::new(AtomicU64::new(0));
        let mut reader = CountingReader {
            inner: reader,
            received: received.clone(),
        };
        let reader_pending = pending.clone();
        thread::Builder::new()
            .name("necoder-remote-reader".to_string())
            .spawn(move || {
                let failure = loop {
                    match read_frame(&mut reader) {
                        Ok(frame) => {
                            // Event frame（id を持たない push 通知）は watch シンクへ振り分ける。
                            if matches!(frame.kind, FrameKind::Event) {
                                if let Ok(event) = serde_json::from_slice::<WatchEvent>(&frame.meta)
                                {
                                    event_sink.dispatch(event);
                                }
                                continue;
                            }
                            let sender = reader_pending
                                .lock()
                                .ok()
                                .and_then(|mut pending| pending.remove(&frame.id));
                            if let Some(sender) = sender {
                                let _sent = sender.send(Ok(frame));
                            }
                        }
                        Err(error) => break format!("remote connection closed: {error:#}"),
                    }
                };
                if let Ok(mut pending) = reader_pending.lock() {
                    for (_, sender) in pending.drain() {
                        let _sent = sender.send(Err(failure.clone()));
                    }
                }
            })
            .expect("remote reader thread spawn");
        Arc::new(Self {
            writer: Mutex::new(writer),
            pending,
            next_request_id: AtomicU64::new(1),
            received,
            owner,
        })
    }

    /// この接続で受信した総バイト数。2 時点の差が 0 なら、その間 相手から何も届いていない。
    fn received_bytes(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// この接続を見限る: 応答待ちの全 request を即座にエラーで返し、session プロセスを落とす。
    ///
    /// 再接続が決まった時点で呼ぶ。呼ばないと、切れた接続に乗っていた request は各自の
    /// タイムアウト（最大 30s）まで待ち続けるし、パイプが詰まって `write` で止まっている
    /// スレッドは ssh が自分で死ぬ（ServerAlive で最大 45s）まで writer ロックを握ったままになる。
    /// プロセスを落とせばパイプが閉じて `write` は EPIPE で即座に戻る。
    fn abort(&self, reason: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            for (_, sender) in pending.drain() {
                let _sent = sender.send(Err(reason.to_string()));
            }
        }
        if let Some(owner) = &self.owner {
            owner.kill();
        }
    }

    fn request(&self, request: &Request, body: Vec<u8>) -> Result<(Response, Vec<u8>)> {
        self.request_with_timeout(request, body, request.timeout())
    }

    fn request_with_timeout(
        &self,
        request: &Request,
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<(Response, Vec<u8>)> {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let frame = Frame::request(id, request, body)?;
        let (sender, receiver) = mpsc::sync_channel(1);
        self.pending
            .lock()
            .map_err(|_| anyhow!("pending request lock poisoned"))?
            .insert(id, sender);
        let write_result = self
            .writer
            .lock()
            .map_err(|_| anyhow!("remote writer lock poisoned"))
            .and_then(|mut writer| write_frame(writer.as_mut(), &frame));
        if let Err(error) = write_result {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(error);
        }
        let frame = match receiver.recv_timeout(timeout) {
            Ok(frame) => frame.map_err(anyhow::Error::msg)?,
            Err(error) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&id);
                }
                return Err(match error {
                    mpsc::RecvTimeoutError::Timeout => RequestTimeout {
                        request: request.name(),
                        after: timeout,
                    }
                    .into(),
                    mpsc::RecvTimeoutError::Disconnected => {
                        anyhow!("remote request channel closed ({})", request.name())
                    }
                });
            }
        };
        match frame.kind {
            FrameKind::Response => Ok((serde_json::from_slice(&frame.meta)?, frame.body)),
            FrameKind::Error => {
                let error: WireError = serde_json::from_slice(&frame.meta)?;
                Err(RemoteRequestError {
                    code: error.code,
                    message: error.message,
                }
                .into())
            }
            _ => bail!("unexpected remote frame kind: {:?}", frame.kind),
        }
    }
}

/// `ssh://` project URI。password/userinfo の password は受け付けない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshProject {
    pub host: String,
    pub username: Option<String>,
    pub port: Option<u16>,
    pub path: PathBuf,
}

impl SshProject {
    pub fn parse(uri: &str) -> Result<Self> {
        let parsed = url::Url::parse(uri).context("SSH URI が不正")?;
        if parsed.scheme() != "ssh" {
            bail!("SSH URI ではない: {uri}");
        }
        if parsed.password().is_some() {
            bail!("password を SSH URI に保存できない");
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            bail!("SSH URI の query/fragment は未対応");
        }
        let host = parsed
            .host_str()
            .context("SSH URI に host が無い")?
            .to_string();
        let username = (!parsed.username().is_empty()).then(|| parsed.username().to_string());
        let path = percent_decode_path(parsed.path())?;
        // path 未指定（ssh://host / ssh://host/ / ~）は「空」= home マーカー。接続時に remote の
        // $HOME をルートにする（標準 SSH と同じ「ログインで home に入る」・VSCode Remote 風・#5）。
        let path = if path.as_os_str().is_empty()
            || path == Path::new("/")
            || path == Path::new("/~")
            || path == Path::new("~")
        {
            PathBuf::new()
        } else {
            path
        };
        Ok(Self {
            host,
            username,
            port: parsed.port(),
            path,
        })
    }

    pub fn destination(&self) -> String {
        match &self.username {
            Some(username) => format!("{username}@{}", self.host),
            None => self.host.clone(),
        }
    }

    pub fn identity(&self) -> String {
        self.uri_for_path(Path::new("/"))
            .trim_end_matches('/')
            .to_string()
    }

    /// password を含まない正規 URI。`url` crate に path/user/IPv6 の escape を任せる。
    pub fn uri_for_path(&self, path: &Path) -> String {
        let mut uri = url::Url::parse("ssh://localhost/").expect("固定 SSH URL は妥当");
        uri.set_host(Some(&self.host))
            .expect("parse 済み SSH host は妥当");
        uri.set_username(self.username.as_deref().unwrap_or(""))
            .expect("parse 済み SSH username は妥当");
        uri.set_port(self.port).expect("SSH port は妥当");
        uri.set_path(&path.to_string_lossy());
        uri.to_string()
    }
}

/// `~/.ssh/config` の 1 エントリ（Remote SSH ホストピッカー用・M13）。
/// alias は `ssh <alias>` / `ssh://<alias>/path` でそのまま使える（system OpenSSH が解決）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConfigHost {
    /// `Host` 行のエイリアス。
    pub alias: String,
    /// `HostName`（実ホスト/IP）。表示用・無ければ None。
    pub hostname: Option<String>,
    /// `User`。表示用・無ければ None。
    pub user: Option<String>,
}

/// SSH config を読んで接続可能なホスト一覧を返す（読めなければ空）。
/// 通常は `~/.ssh/config`、テストでは transport と同じ `NECODER_SSH_CONFIG` を使う。
pub fn ssh_config_hosts() -> Vec<SshConfigHost> {
    let path = match std::env::var_os("NECODER_SSH_CONFIG") {
        Some(path) => PathBuf::from(path),
        None => {
            // Windows の OpenSSH も `%USERPROFILE%.sshnfig` を見る（paths が USERPROFILE を解決する）。
            let Some(home) = paths::home_dir() else {
                return Vec::new();
            };
            home.join(".ssh/config")
        }
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_ssh_config(&text),
        Err(_) => Vec::new(),
    }
}

/// ssh_config テキストを Host 単位で列挙する（IO 無し = テスト可能）。
/// v1 の割り切り: ワイルドカード/否定パターン(`*` `?` `!`)は接続先にならないので除外・
/// `Include` は展開しない・オプションは各ブロック内のみ見る（ssh の first-match 累積は未実装）。
fn parse_ssh_config(text: &str) -> Vec<SshConfigHost> {
    let mut hosts: Vec<SshConfigHost> = Vec::new();
    // 直近の Host 行で確定した alias 群の hosts 内インデックス（後続の HostName/User を貼る先）。
    let mut current: Vec<usize> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((keyword, value)) = split_ssh_config_line(line) else {
            continue;
        };
        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                current.clear();
                for alias in value.split_whitespace() {
                    if alias.contains('*') || alias.contains('?') || alias.starts_with('!') {
                        continue; // パターンは接続先ではない
                    }
                    match hosts.iter().position(|host| host.alias == alias) {
                        Some(index) => current.push(index), // 既出 alias は重複させない
                        None => {
                            current.push(hosts.len());
                            hosts.push(SshConfigHost {
                                alias: alias.to_string(),
                                hostname: None,
                                user: None,
                            });
                        }
                    }
                }
            }
            "hostname" => {
                for &index in &current {
                    if hosts[index].hostname.is_none() && !value.is_empty() {
                        hosts[index].hostname = Some(value.to_string());
                    }
                }
            }
            "user" => {
                for &index in &current {
                    if hosts[index].user.is_none() && !value.is_empty() {
                        hosts[index].user = Some(value.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    hosts
}

/// ssh_config の 1 行を (keyword, value) に割る。区切りは空白 or `=`（前後空白可・値の囲み `"` は外す）。
fn split_ssh_config_line(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut end = 0;
    while end < bytes.len() && !bytes[end].is_ascii_whitespace() && bytes[end] != b'=' {
        end += 1;
    }
    if end == 0 {
        return None;
    }
    let keyword = &line[..end];
    let mut rest = line[end..].trim_start();
    if let Some(stripped) = rest.strip_prefix('=') {
        rest = stripped.trim_start();
    }
    Some((keyword, rest.trim().trim_matches('"')))
}

fn percent_decode_path(path: &str) -> Result<PathBuf> {
    let mut decoded = Vec::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                bail!("incomplete percent escape in SSH path");
            }
            let value = u8::from_str_radix(&path[index + 1..index + 3], 16)
                .context("invalid percent escape in SSH path")?;
            decoded.push(value);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).context("SSH path must be UTF-8")?;
    Ok(PathBuf::from(decoded))
}

/// 1 destination に1本だけ作る system OpenSSH multiplex transport。
/// ControlMaster（と、master になり得る standalone session）に付ける接続オプション。
const MASTER_OPTIONS: [&str; 5] = [
    // 死んだ/到達不能なホストで GUI が無限にハングしないよう接続打ち切りを入れる
    // （既定は OS の TCP タイムアウト任せ＝分単位。sleep/VPN 断からの復帰性に効く）。
    "ConnectTimeout=10",
    "ControlPersist=600",
    "ServerAliveInterval=15",
    "ServerAliveCountMax=3",
    "ExitOnForwardFailure=yes",
];

/// Remote terminal の `ne` から接続元 GUI へ戻る、open 専用のローカル gateway。
///
/// GUI の `gui.sock` をそのまま `ssh -R` すると、remote 上の任意プロセスへ fleet/send など
/// 制御 IPC の全権限まで渡してしまう。そこで SSH が転送する先をこの socket に限定し、`open`
/// 以外を拒否した上で、接続先 identity をローカル側で刻んだ `open_remote` へ変換する。
#[cfg(unix)]
struct RemoteCliGateway {
    socket: PathBuf,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(unix)]
impl RemoteCliGateway {
    fn bind(socket: PathBuf, gui_socket: PathBuf, authority: String) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::net::UnixListener;

        let listener = UnixListener::bind(&socket).with_context(|| {
            format!("remote CLI gateway を bind できない: {}", socket.display())
        })?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = stop.clone();
        let cleanup_socket = socket.clone();
        thread::Builder::new()
            .name("necoder-remote-cli-gateway".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    if thread_stop.load(Ordering::Acquire) {
                        break;
                    }
                    match stream {
                        Ok(mut stream) => {
                            if let Err(error) = forward_remote_cli_open(
                                &mut stream,
                                &gui_socket,
                                &authority,
                            ) {
                                let response = serde_json::json!({
                                    "ok": false,
                                    "error": format!("接続元の necoder へ渡せませんでした: {error:#}"),
                                });
                                let _written = writeln!(stream, "{response}");
                            }
                        }
                        Err(error) => {
                            eprintln!("remote CLI gateway の accept に失敗: {error}");
                            break;
                        }
                    }
                }
                let _cleanup = std::fs::remove_file(cleanup_socket);
            })
            .context("remote CLI gateway thread を起動できない")?;
        Ok(Self { socket, stop })
    }
}

#[cfg(unix)]
impl Drop for RemoteCliGateway {
    fn drop(&mut self) {
        use std::os::unix::net::UnixStream;

        self.stop.store(true, Ordering::Release);
        // blocking accept を起こす。接続後は stop を先に見るので要求として処理されない。
        let _wake = UnixStream::connect(&self.socket);
    }
}

/// gateway の 1 接続を処理する。remote から受け付ける method は `open` だけ。
#[cfg(unix)]
fn forward_remote_cli_open(
    remote: &mut std::os::unix::net::UnixStream,
    gui_socket: &Path,
    authority: &str,
) -> Result<()> {
    use std::os::unix::net::UnixStream;

    remote.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut line = String::new();
    // `try_open_in_running_gui` の生存確認は connect して即 close する。その接続は静かに無視する。
    if BufReader::new(&mut *remote)
        .take((MAX_META_LEN + 1) as u64)
        .read_line(&mut line)?
        == 0
    {
        return Ok(());
    }
    anyhow::ensure!(
        line.len() <= MAX_META_LEN,
        "remote CLI request が大きすぎる"
    );
    let request: serde_json::Value =
        serde_json::from_str(&line).context("remote CLI request が JSON ではない")?;
    anyhow::ensure!(
        request.get("method").and_then(serde_json::Value::as_str) == Some("open"),
        "remote terminal から許可されている操作は open だけです"
    );
    let paths: Vec<&str> = request
        .get("params")
        .and_then(|params| params.get("paths"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let forwarded = serde_json::json!({
        "method": "open_remote",
        "params": { "authority": authority, "paths": paths },
    });
    let mut gui = UnixStream::connect(gui_socket)
        .with_context(|| format!("GUI socket（{}）に接続できない", gui_socket.display()))?;
    gui.set_read_timeout(Some(Duration::from_secs(60)))?;
    writeln!(gui, "{forwarded}").context("GUI への open 転送に失敗")?;
    gui.flush()?;
    let mut response = String::new();
    BufReader::new(gui)
        .take((MAX_META_LEN + 1) as u64)
        .read_line(&mut response)
        .context("GUI 応答を読めない")?;
    anyhow::ensure!(!response.is_empty(), "GUI 応答が空です");
    anyhow::ensure!(response.len() <= MAX_META_LEN, "GUI 応答が大きすぎる");
    remote.write_all(response.as_bytes())?;
    remote.flush()?;
    Ok(())
}

#[cfg(unix)]
struct RemoteCliForward {
    remote_socket: PathBuf,
    remote_bin_dir: PathBuf,
    _gateway: RemoteCliGateway,
}

struct SshTransport {
    project: SshProject,
    control_dir: PathBuf,
    control_path: PathBuf,
    #[cfg(unix)]
    remote_cli_forward: Option<RemoteCliForward>,
}

impl SshTransport {
    /// 制御ソケットの置き場だけ用意する。**ここではネットワークに触らない**。
    /// master は最初に必要になった時点で [`Self::ensure_master`] が起こす。
    fn new(project: &SshProject) -> Result<Arc<Self>> {
        static NEXT_CONTROL: AtomicU64 = AtomicU64::new(1);
        let serial = NEXT_CONTROL.fetch_add(1, Ordering::Relaxed);
        let identity_hash = content_hash(project.identity().as_bytes());
        // UNIX domain socket の長さ制限を避けるため短い /tmp 配下を使う。directory は owner-only。
        let control_dir = PathBuf::from(format!(
            "/tmp/necoder-ssh-{}-{identity_hash:016x}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&control_dir).with_context(|| {
            format!(
                "SSH control directory を作れない: {}",
                control_dir.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&control_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let control_path = control_dir.join("master");
        #[cfg(unix)]
        let remote_cli_forward = if let Some(gui_socket) = paths::runtime_socket() {
            let token = random_session_id()?;
            let token = &token[..24];
            let gateway_socket = control_dir.join("cli-gateway.sock");
            match RemoteCliGateway::bind(gateway_socket, gui_socket, project.identity()) {
                Ok(gateway) => Some(RemoteCliForward {
                    remote_socket: PathBuf::from(format!("/tmp/necoder-cli-{token}.sock")),
                    remote_bin_dir: PathBuf::from(format!("/tmp/necoder-cli-{token}")),
                    _gateway: gateway,
                }),
                Err(error) => {
                    // Remote SSH 本体は使えるように保つ。端末内 `ne` だけが無効になる。
                    eprintln!(
                        "remote CLI gateway を準備できない（ne は接続元へ戻りません）: {error:#}"
                    );
                    None
                }
            }
        } else {
            None
        };
        Ok(Arc::new(Self {
            project: project.clone(),
            control_dir,
            control_path,
            #[cfg(unix)]
            remote_cli_forward,
        }))
    }

    /// 置き場を用意して master も起こす（対話的に開くとき＝失敗を即その場で返したいとき）。
    fn connect(project: &SshProject) -> Result<Arc<Self>> {
        let transport = Self::new(project)?;
        transport.start_master()?;
        Ok(transport)
    }

    fn start_master(&self) -> Result<()> {
        let mut master = ssh_command();
        master
            .args(["-M", "-N", "-f"])
            .arg("-o")
            .arg(format!("ControlPath={}", self.control_path.display()));
        for option in MASTER_OPTIONS {
            master.args(["-o", option]);
        }
        #[cfg(unix)]
        if let Some(forward) = &self.remote_cli_forward {
            // remote へ GUI 本体の socket を直出しせず、open 専用 gateway だけを reverse forward。
            master
                .args(["-o", "StreamLocalBindUnlink=yes"])
                .args(["-o", "StreamLocalBindMask=0177"])
                .arg("-R")
                .arg(format!(
                    "{}:{}",
                    forward.remote_socket.display(),
                    forward._gateway.socket.display()
                ));
        }
        if let Some(port) = self.project.port {
            master.args(["-p", &port.to_string()]);
        }
        let status = master
            .arg(self.project.destination())
            .status()
            .context("OpenSSH ControlMaster を起動できない")?;
        if !status.success() {
            bail!("OpenSSH ControlMaster の接続に失敗: {status}");
        }
        Ok(())
    }

    fn ensure_master(&self) -> Result<()> {
        // `terminal_launch` / `spawn_process` は request を経由しないが、ここで master を
        // 起こす＝接続そのもの。UI スレッドから呼ばれていないかは request と同じ検査に掛ける。
        assert_off_main_thread("ensure_master");
        let status = ssh_command()
            .args(["-S", &self.control_path.to_string_lossy(), "-O", "check"])
            .arg(self.project.destination())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("OpenSSH ControlMaster を確認できない")?;
        if status.success() {
            return Ok(());
        }
        let _stale = std::fs::remove_file(&self.control_path);
        self.start_master()
    }

    /// RPC session 用の引数。master は [`Self::ensure_master`] が別途起こしている前提
    /// （`ControlMaster=no`・無ければ失敗する）。
    fn session_args(&self, tty: bool, remote_command: &str) -> Vec<String> {
        self.session_args_with(tty, remote_command, false)
    }

    /// `standalone` = master が無ければ**この session 自身が master になる**（`ControlMaster=auto`）。
    ///
    /// 端末・LSP・ACP のプロセスはこれで起こす。事前に `ensure_master` で接続を確かめる必要が
    /// なくなり、「起動仕様を組むだけ」の関数がネットワークに触らずに済む（UI スレッドから
    /// 呼ばれる。sleep 復帰直後だとそこで master の張り直しを待って固まっていた）。
    /// master が居れば従来どおり多重化に乗る。居なければ TCP をこの session が張り、終了後も
    /// `ControlPersist` で残るので次からは多重化される。
    fn session_args_with(&self, tty: bool, remote_command: &str, standalone: bool) -> Vec<String> {
        let mut args = vec![
            if tty { "-tt" } else { "-T" }.to_string(),
            "-S".to_string(),
            self.control_path.display().to_string(),
        ];
        if standalone {
            for option in MASTER_OPTIONS {
                args.extend(["-o".to_string(), option.to_string()]);
            }
            args.extend(["-o".to_string(), "ControlMaster=auto".to_string()]);
        } else {
            args.extend(["-o".to_string(), "ControlMaster=no".to_string()]);
        }
        if let Some(port) = self.project.port {
            args.extend(["-p".to_string(), port.to_string()]);
        }
        args.push(self.project.destination());
        args.push(remote_command.to_string());
        args
    }

    fn command(&self, tty: bool, remote_command: &str) -> Command {
        let mut command = ssh_command();
        command.args(self.session_args(tty, remote_command));
        command
    }

    /// master 無しでも自力で繋ぐ session（[`Self::session_args_with`] の `standalone`）。
    fn standalone_command(&self, tty: bool, remote_command: &str) -> Command {
        let mut command = ssh_command();
        command.args(self.session_args_with(tty, remote_command, true));
        command
    }

    fn output(&self, remote_command: &str) -> Result<CommandOutput> {
        let output = self
            .command(false, remote_command)
            .stdin(Stdio::null())
            .output()
            .context("SSH bootstrap command を実行できない")?;
        Ok(CommandOutput {
            status_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn compatible_server(&self, command: &str) -> bool {
        let probe = format!("exec {} version", quote_posix(command));
        self.output(&probe).is_ok_and(|output| {
            output.success()
                && String::from_utf8_lossy(&output.stdout).trim()
                    == format!("{SERVER_VERSION} protocol={PROTOCOL_VERSION}")
        })
    }

    fn ensure_remote_server(&self, preferred_command: &str) -> Result<String> {
        if self.compatible_server(preferred_command) {
            return Ok(preferred_command.to_string());
        }

        // remote の platform を先に検出（配備バイナリを remote target に合わせて選ぶため・#1）。
        let platform =
            self.output("printf '%s\\n%s\\n%s\\n' \"$HOME\" \"$(uname -s)\" \"$(uname -m)\"")?;
        if !platform.success() {
            bail!(
                "remote platform を検出できない: {}",
                String::from_utf8_lossy(&platform.stderr).trim()
            );
        }
        let platform =
            String::from_utf8(platform.stdout).context("remote platform 応答が UTF-8 ではない")?;
        let mut lines = platform.lines();
        let home = lines
            .next()
            .filter(|home| !home.is_empty())
            .context("remote HOME が空")?;
        let remote_os = lines.next().context("remote OS 応答が無い")?;
        let remote_arch = lines.next().context("remote arch 応答が無い")?;

        // 配備バイナリ: 明示指定（同一プラットフォーム検査を飛ばす）→ remote target 用の自動発見
        // （per-target キャッシュ / .app 同梱 / same-platform の dev ビルド・#1）。
        let explicit_artifact = std::env::var_os("NECODER_REMOTE_SERVER_BINARY").map(PathBuf::from);
        let artifact = explicit_artifact
            .clone()
            .or_else(|| find_remote_server_for(remote_os, remote_arch))
            .with_context(|| {
                format!(
                    "remote ({remote_os}/{remote_arch}) 用の necoder-remote-server が見つからない。\
                     CI 生成物を ~/.local/share/necoder/remote/artifacts/<target>/ に置くか、\
                     NECODER_REMOTE_SERVER_BINARY=<remote向けartifact> を指定してください"
                )
            })?;
        if !artifact.is_file() {
            bail!(
                "remote server artifact がファイルではない: {}",
                artifact.display()
            );
        }

        let install_dir = PathBuf::from(home)
            .join(".local/share/necoder/remote/servers")
            .join(format!("{SERVER_VERSION}-p{PROTOCOL_VERSION}"));
        let installed = install_dir.join("necoder-remote-server");
        if self.compatible_server(&installed.to_string_lossy()) {
            return Ok(installed.to_string_lossy().to_string());
        }
        let temporary = install_dir.join(format!(".upload-{}", std::process::id()));
        let bootstrap = format!(
            "umask 077 && mkdir -p {} && cat > {} && chmod 700 {} && mv -f {} {}",
            quote_posix(&install_dir.to_string_lossy()),
            quote_posix(&temporary.to_string_lossy()),
            quote_posix(&temporary.to_string_lossy()),
            quote_posix(&temporary.to_string_lossy()),
            quote_posix(&installed.to_string_lossy()),
        );
        let mut child = self
            .command(false, &bootstrap)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .context("remote server upload session を起動できない")?;
        let mut source = std::fs::File::open(&artifact).with_context(|| {
            format!("remote server artifact を読めない: {}", artifact.display())
        })?;
        let mut stdin = child
            .stdin
            .take()
            .context("remote server upload stdin が無い")?;
        std::io::copy(&mut source, &mut stdin).context("remote server artifact の転送に失敗")?;
        drop(stdin);
        let status = child
            .wait()
            .context("remote server upload の終了を待てない")?;
        if !status.success() {
            bail!("remote server upload に失敗: {status}");
        }

        // checksum: 転送破損/改竄を検出する（version 文字列一致より厳密）。両端でハッシュツール
        // （sha256sum / shasum）が使えるときだけ突合し、片方でも欠ければ version/protocol 検査に委ねる。
        if let Some(expected) = local_file_sha256(&artifact) {
            let probe = format!(
                "sha256sum {path} 2>/dev/null || shasum -a 256 {path} 2>/dev/null || true",
                path = quote_posix(&installed.to_string_lossy())
            );
            if let Ok(output) = self.output(&probe) {
                if let Some(actual) = parse_sha256_hex(&output.stdout) {
                    if actual != expected {
                        // 壊れたバイナリを残さない（次回は再アップロードからやり直せる）。
                        let _rm = self.output(&format!(
                            "rm -f {}",
                            quote_posix(&installed.to_string_lossy())
                        ));
                        bail!(
                            "配備した remote server の checksum 不一致（転送破損の可能性）: \
                             expected {expected}, actual {actual}"
                        );
                    }
                }
            }
        }

        let installed = installed.to_string_lossy().to_string();
        if !self.compatible_server(&installed) {
            bail!("配備した remote server の version/protocol 検証に失敗");
        }
        // 検証に通ってから、現行 version 以外の古い server を掃除する（best-effort）。
        self.cleanup_old_servers(&install_dir);
        Ok(installed)
    }

    /// `~/.local/share/necoder/remote/servers/` 配下の、現行 version 以外の server ディレクトリを
    /// 削除する（best-effort）。version が上がるたびに旧バイナリが溜まって容量を食うのを防ぐ。
    fn cleanup_old_servers(&self, install_dir: &Path) {
        let Some(servers_dir) = install_dir.parent() else {
            return;
        };
        let Some(current) = install_dir.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let sweep = format!(
            "cd {dir} 2>/dev/null || exit 0; for entry in */; do [ -d \"$entry\" ] || continue; \
             name=\"${{entry%/}}\"; [ \"$name\" = {cur} ] && continue; rm -rf -- \"$name\"; done",
            dir = quote_posix(&servers_dir.to_string_lossy()),
            cur = quote_posix(current),
        );
        let _swept = self.output(&sweep);
    }
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

/// `ssh` サブプロセスの土台。`NECODER_SSH_CONFIG` が指定されていれば `-F <config>` を先頭に
/// 前置きする。テスト（Docker の隔離ホスト）やサンドボックス運用で、ユーザーの `~/.ssh/config`
/// や `known_hosts` を汚さずに remote を検証するための seam。未指定なら system ssh の既定どおり。
fn ssh_command() -> Command {
    let mut command = Command::new("ssh");
    if let Some(config) = std::env::var_os("NECODER_SSH_CONFIG") {
        command.arg("-F").arg(config);
    }
    command
}

/// ローカルのハッシュツール（`sha256sum` か BSD/macOS の `shasum -a 256`）で SHA-256 hex を計算する。
/// どちらも無ければ `None`（checksum 検証は best-effort でスキップし、version/protocol 検査に委ねる）。
fn local_file_sha256(path: &Path) -> Option<String> {
    let path = path.to_string_lossy().to_string();
    let candidates: [(&str, Vec<String>); 2] = [
        ("sha256sum", vec![path.clone()]),
        ("shasum", vec!["-a".to_string(), "256".to_string(), path]),
    ];
    for (program, args) in candidates {
        if let Ok(output) = Command::new(program).args(&args).output() {
            if output.status.success() {
                if let Some(hex) = parse_sha256_hex(&output.stdout) {
                    return Some(hex);
                }
            }
        }
    }
    None
}

/// `sha256sum` / `shasum -a 256` の出力（`<hex>  <path>`）から先頭の 64 桁 hex を取り出す。
fn parse_sha256_hex(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let token = text.split_whitespace().next()?;
    // GNU coreutils は filename に特殊文字があると `\<hash>` と行頭 `\` を付ける。
    let token = token.trim_start_matches('\\');
    (token.len() == 64 && token.chars().all(|character| character.is_ascii_hexdigit()))
        .then(|| token.to_ascii_lowercase())
}

/// 構造化接続ログ（1 行 = 1 フェーズ・stderr）。接続は低頻度イベントなので常時出す。
/// 失敗診断（どの段で・どれだけ掛かったか）を残すのが目的。
fn ssh_log(destination: &str, phase: &str, elapsed_from: Option<std::time::Instant>) {
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

/// remote の uname (os, arch) を Rust target triple へ（配備バイナリの探索キー・#1）。
/// Linux は static-musl（glibc 依存なし）、macOS は apple-darwin。未知は None。
fn remote_target_triple(remote_os: &str, remote_arch: &str) -> Option<String> {
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

/// remote target 用の配備バイナリを探す（#1 の自動発見・env 指定なしで mac→Linux を通す）:
/// 1) per-target キャッシュ `~/.local/share/necoder/remote/artifacts/<triple>/necoder-remote-server`
/// 2) .app 同梱 `<exe>/../Resources/remote/<triple>/necoder-remote-server`（インストール版）
/// 3) same-platform なら従来の sibling / dev ビルド（[`find_local_remote_server`]）
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
                let bundled = dir
                    .join("../Resources/remote")
                    .join(&triple)
                    .join("necoder-remote-server");
                if bundled.is_file() {
                    return Some(bundled);
                }
            }
        }
    }
    if same_platform(remote_os, remote_arch) {
        return find_local_remote_server();
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

impl SshTransport {
    /// ControlMaster を明示終了する（多重化された全 session が同時に落ちる）。通常の Drop と、
    /// 障害注入テストの「ControlMaster kill → 次 request が再接続で回復」検証の両方で使う。
    fn exit_master(&self) {
        let mut command = ssh_command();
        command
            .args(["-S", &self.control_path.to_string_lossy(), "-O", "exit"])
            .arg(self.project.destination())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _status = command.status();
    }
}

impl Drop for SshTransport {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(forward) = &self.remote_cli_forward {
            // token 付きの専用パスだけを対象にし、remote の通常ファイルには触れない。
            let cleanup = format!(
                "rm -f -- {socket} {shim}; rmdir -- {bin} 2>/dev/null || true",
                socket = quote_posix(&forward.remote_socket.to_string_lossy()),
                shim = quote_posix(&forward.remote_bin_dir.join("ne").to_string_lossy()),
                bin = quote_posix(&forward.remote_bin_dir.to_string_lossy()),
            );
            let _cleaned = self.output(&cleanup);
        }
        self.exit_master();
        #[cfg(unix)]
        drop(self.remote_cli_forward.take());
        let _cleanup = std::fs::remove_dir_all(&self.control_dir);
    }
}

struct SshConnector {
    transport: Arc<SshTransport>,
    /// 配備前は設定値、配備後は解決済みのコマンド。遅延接続では最初の接続まで配備しないので、
    /// 「まだ確かめていない」状態を持てる必要がある。
    server_command: Arc<Mutex<String>>,
    /// 配備の確認が済んだか。済んだ後は再接続のたびに配備し直さない。
    server_ready: std::sync::atomic::AtomicBool,
    session: String,
}

impl SshConnector {
    fn command(&self) -> Command {
        let server_command = self
            .server_command
            .lock()
            .map(|command| command.clone())
            .unwrap_or_default();
        let remote_command = format!(
            "exec {} proxy --session {}",
            quote_posix(&server_command),
            quote_posix(&self.session)
        );
        let mut command = self.transport.command(false, &remote_command);
        command.stderr(Stdio::inherit());
        command
    }

    /// remote-server が使える状態か確かめ、必要なら配備する（初回だけ）。
    fn ensure_server(&self) -> Result<()> {
        if self.server_ready.load(Ordering::Acquire) {
            return Ok(());
        }
        let preferred = self
            .server_command
            .lock()
            .map_err(|_| anyhow!("remote server command lock poisoned"))?
            .clone();
        let resolved = self
            .transport
            .ensure_remote_server(&preferred)
            .context("remote-server の配備に失敗")?;
        *self
            .server_command
            .lock()
            .map_err(|_| anyhow!("remote server command lock poisoned"))? = resolved;
        self.server_ready.store(true, Ordering::Release);
        Ok(())
    }

    /// 新しい session を張って Hello を交わす（[`HELLO_TIMEOUT`] で打ち切る）。
    fn handshake(&self, event_sink: Arc<WatchEventSink>) -> Result<Arc<RpcClient>> {
        let client = rpc_client_from_command(self.command(), event_sink)?;
        verify_hello(&client)?;
        Ok(client)
    }
}

/// Hello を交わして protocol 版を確かめる。接続経路（ssh / テストの UnixStream）に依らず同じ。
fn verify_hello(client: &RpcClient) -> Result<()> {
    let (hello, _) = client.request(
        &Request::Hello {
            client_version: SERVER_VERSION.to_string(),
        },
        Vec::new(),
    )?;
    let Response::Hello {
        protocol_version, ..
    } = hello
    else {
        bail!("remote server returned invalid hello response");
    };
    if protocol_version != PROTOCOL_VERSION {
        bail!("remote protocol mismatch: {protocol_version}");
    }
    Ok(())
}

/// 接続を張る側の抽象。実物は [`SshConnector`]（OpenSSH ControlMaster 越し）。
/// [`ReconnectingClient`] の再接続ロジックを ssh 無しで検証できるよう、テストは
/// UnixStream の対で同じ protocol を喋る connector を差す。
trait Connector: Send + Sync {
    fn connect(&self, event_sink: Arc<WatchEventSink>) -> Result<Arc<RpcClient>>;
}

impl Connector for SshConnector {
    fn connect(&self, event_sink: Arc<WatchEventSink>) -> Result<Arc<RpcClient>> {
        self.transport.ensure_master()?;
        self.ensure_server()?;
        match self.handshake(event_sink.clone()) {
            Ok(client) => Ok(client),
            // `ssh -O check` は **master プロセスが生きているか**しか見ない。sleep 復帰直後は
            // master は生きたまま TCP だけ死んでいることがあり（ServerAlive が気付くまで最大 45s）、
            // check を通った master 越しの新 session が黙って固まる。1 回で見切って
            // master ごと作り直す — 待ち続けるより速いし、結果も決定的になる。
            //
            // 作り直すのは **Hello が時間切れ**（相手が黙っている）のときだけ。session が即座に
            // 閉じたのなら master は通っていて、落ちたのは remote-server の起動側（バイナリが無い・
            // 落ちた）。そこで master を殺しても直らず、同じ master に乗っている端末を巻き添えに
            // するだけになる。
            Err(stale) if stale.downcast_ref::<RequestTimeout>().is_some() => {
                self.transport.exit_master();
                self.transport.start_master().with_context(|| {
                    format!("ControlMaster の作り直しに失敗（元の失敗: {stale:#}）")
                })?;
                self.handshake(event_sink)
            }
            Err(error) => Err(error),
        }
    }
}

struct ReconnectingClient {
    /// **まだ一度も繋いでいない間は `None`**。起動時の遅延接続はこの状態から始まり、
    /// 最初の request で接続する。以後は切断のたびに張り直す。
    current: Mutex<Option<Arc<RpcClient>>>,
    connector: Option<Arc<dyn Connector>>,
    reconnect_lock: Mutex<()>,
    /// 直近の接続失敗（時刻とメッセージ）。待っている間に起きた失敗を、待ち人が
    /// もう一度やり直さないための印（[`Self::connect_now`]）。
    last_failure: Mutex<Option<(std::time::Instant, String)>>,
    event_sink: Arc<WatchEventSink>,
    generation: AtomicU64,
    /// いまの接続状態（[`ConnectionState::as_u8`]）。UI が I/O 無しで読める唯一の窓。
    state: AtomicU8,
    /// 状態変化の購読者。送れなくなった（受信側が drop した）ものは次の通知で外す。
    state_listeners: Mutex<Vec<mpsc::Sender<ConnectionState>>>,
}

impl ReconnectingClient {
    fn new(
        current: Arc<RpcClient>,
        connector: Option<Arc<dyn Connector>>,
        event_sink: Arc<WatchEventSink>,
    ) -> Arc<Self> {
        Self::with_current(Some(current), connector, event_sink)
    }

    /// 接続を張らずに作る。最初の request まで ssh を起こさない（起動を止めないため）。
    fn disconnected(connector: Arc<dyn Connector>, event_sink: Arc<WatchEventSink>) -> Arc<Self> {
        Self::with_current(None, Some(connector), event_sink)
    }

    fn with_current(
        current: Option<Arc<RpcClient>>,
        connector: Option<Arc<dyn Connector>>,
        event_sink: Arc<WatchEventSink>,
    ) -> Arc<Self> {
        let initial_state = if current.is_some() {
            ConnectionState::Connected
        } else {
            ConnectionState::Unconnected
        };
        let client = Arc::new(Self {
            current: Mutex::new(current),
            connector,
            reconnect_lock: Mutex::new(()),
            last_failure: Mutex::new(None),
            event_sink,
            generation: AtomicU64::new(0),
            state: AtomicU8::new(initial_state.as_u8()),
            state_listeners: Mutex::new(Vec::new()),
        });
        if client.connector.is_some() {
            let weak = Arc::downgrade(&client);
            thread::Builder::new()
                .name("necoder-remote-heartbeat".to_string())
                .spawn(move || {
                    // 生きている間は 5s ごと。落ちている間は指数バックオフで 60s まで伸ばす。
                    // 到達不能なホスト（電源断・VPN 断）に対して 5s ごとに ssh を起こし続けると、
                    // 接続タイムアウトぶんプロセスが積み上がるだけで誰も得をしない。
                    const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
                    const HEARTBEAT_MAX_BACKOFF: Duration = Duration::from_secs(60);
                    let mut interval = HEARTBEAT_INTERVAL;
                    loop {
                        thread::sleep(interval);
                        let Some(client) = weak.upgrade() else {
                            break;
                        };
                        interval = if client.heartbeat_once(PING_TIMEOUT) {
                            HEARTBEAT_INTERVAL
                        } else {
                            (interval * 2).min(HEARTBEAT_MAX_BACKOFF)
                        };
                    }
                })
                .expect("remote heartbeat thread spawn");
        }
        client
    }

    /// 生存確認を 1 回行い、切れていれば張り直す。戻り値は「いま繋がっているか」。
    ///
    /// Ping が `ping_timeout` 内に返らなくても、その間に受信バイトが進んでいれば**生きている**
    /// と判定して張り直さない。低速回線で大きなファイルを流している最中は Pong がその後ろに
    /// 並ぶだけで、そこで切ると転送が巻き添えになり、再送がまた回線を埋めて永久に繋がらない。
    /// 何も届いていない時間切れだけを切断とみなす（sleep 復帰直後の死んだ TCP はこちら）。
    fn heartbeat_once(&self, ping_timeout: Duration) -> bool {
        let Ok(current) = self.current.lock().map(|current| current.clone()) else {
            return false;
        };
        let Some(current) = current else {
            // まだ一度も繋いでいない（起動時の遅延接続で、最初の request がまだ来ていない
            // か失敗した）。ここで繋いでおけば最初の操作が速い。失敗はバックオフに乗る。
            return self.request(&Request::Ping, Vec::new()).is_ok();
        };
        let received_before = current.received_bytes();
        match current.request_with_timeout(&Request::Ping, Vec::new(), ping_timeout) {
            Ok(_pong) => true,
            Err(error) if error.downcast_ref::<RemoteRequestError>().is_some() => true,
            Err(error)
                if error.downcast_ref::<RequestTimeout>().is_some()
                    && current.received_bytes() != received_before =>
            {
                true
            }
            Err(_dead) => match self.connect_now(Some(&current)) {
                Ok(replacement) => replacement.request(&Request::Ping, Vec::new()).is_ok(),
                Err(_unreachable) => false,
            },
        }
    }

    fn event_sink(&self) -> Arc<WatchEventSink> {
        self.event_sink.clone()
    }

    /// 再接続のたびに増える世代番号。watch keeper が「監視を張り直す」判定に使う。
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// いまの接続状態（atomic 読みだけ・I/O 無し）。
    fn state(&self) -> ConnectionState {
        ConnectionState::from_u8(self.state.load(Ordering::Relaxed))
    }

    /// 状態を更新し、変わったときだけ購読者へ流す。受信側が消えた購読はここで外す。
    fn set_state(&self, state: ConnectionState) {
        let previous = self.state.swap(state.as_u8(), Ordering::Relaxed);
        if previous == state.as_u8() {
            return;
        }
        if let Ok(mut listeners) = self.state_listeners.lock() {
            listeners.retain(|listener| listener.send(state).is_ok());
        }
    }

    /// 状態変化の購読口。購読時点の状態は [`Self::state`] で別途読む（変化だけが流れる）。
    fn subscribe_state(&self) -> mpsc::Receiver<ConnectionState> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut listeners) = self.state_listeners.lock() {
            listeners.push(sender);
        }
        receiver
    }

    /// ユーザー操作による「今すぐ再接続」。背景スレッドで heartbeat を 1 回だけ前倒しする。
    /// 生きていれば何もしない（healthy な接続を捨てて張り直したりしない）、未接続なら初回接続、
    /// 死んでいれば張り直し — 判定は [`Self::heartbeat_once`] と同じなので、定期 heartbeat と
    /// 二重に走っても `reconnect_lock` と superseded 判定で 1 本に畳まれる。
    fn reconnect_in_background(self: &Arc<Self>) {
        let client = self.clone();
        let spawned = thread::Builder::new()
            .name("necoder-remote-reconnect".to_string())
            .spawn(move || {
                if !client.heartbeat_once(PING_TIMEOUT) {
                    eprintln!("Remote SSH: 手動再接続に失敗（heartbeat が引き続き再試行します）");
                }
            });
        if let Err(error) = spawned {
            eprintln!("Remote SSH: 再接続スレッドを起動できない: {error}");
        }
    }

    fn request(&self, request: &Request, body: Vec<u8>) -> Result<(Response, Vec<u8>)> {
        self.request_inner(request, body, true)
    }

    fn request_nonretry(&self, request: &Request, body: Vec<u8>) -> Result<(Response, Vec<u8>)> {
        self.request_inner(request, body, false)
    }

    fn request_inner(
        &self,
        request: &Request,
        body: Vec<u8>,
        retry_safe: bool,
    ) -> Result<(Response, Vec<u8>)> {
        // SSH 越しの往復・再接続待ちが UI スレッドに乗っていないかをここで一括検査する
        // （RemoteHost の全 request がこの 1 点を通る）。
        assert_off_main_thread(request.name());
        let current = self
            .current
            .lock()
            .map_err(|_| anyhow!("remote client lock poisoned"))?
            .clone();
        // まだ一度も繋いでいない（起動時の遅延接続）。ここが最初の接続になる。
        let Some(current) = current else {
            let connected = self.connect_now(None)?;
            return connected.request(request, body);
        };
        let received_before = current.received_bytes();
        match current.request(request, body.clone()) {
            Ok(response) => Ok(response),
            // peer が request を処理して返したエラーは接続障害ではない。保存競合や ENOENT で
            // ControlMaster を張り直さず、そのまま呼び出し元へ返す。
            Err(error) if error.downcast_ref::<RemoteRequestError>().is_some() => Err(error),
            // 時間切れでも、その間に受信が進んでいるなら接続は生きている（混んでいるだけ）。
            // 張り直すと進行中の転送を巻き添えにする上、再送がまた回線を埋める。
            // 時間切れをそのまま返す。
            Err(error)
                if error.downcast_ref::<RequestTimeout>().is_some()
                    && current.received_bytes() != received_before =>
            {
                Err(error)
            }
            Err(original) => {
                if self.connector.is_none() {
                    return Err(original);
                }
                let replacement = self
                    .connect_now(Some(&current))
                    .context("Remote SSH 再接続に失敗")?;
                if retry_safe {
                    replacement.request(request, body)
                } else {
                    Err(original.context(
                        "接続は復旧したが、結果不明の非冪等 request は安全のため再送しない",
                    ))
                }
            }
        }
    }

    /// 接続を張る（初回も再接続も同じ経路）。`stale` は「自分が掴んでいた壊れた接続」で、
    /// 別スレッドが既に張り直していたらそれをそのまま使う（二重接続を避ける）。
    fn connect_now(&self, stale: Option<&Arc<RpcClient>>) -> Result<Arc<RpcClient>> {
        // ロックを取る**前**の時刻。待っている間に起きた失敗と、自分が来る前の失敗を
        // 区別するために使う（下の「積み上がり抑制」）。
        let arrived = std::time::Instant::now();
        let Some(connector) = &self.connector else {
            bail!("remote host に connector が無い（接続を張れない）");
        };
        let _guard = self
            .reconnect_lock
            .lock()
            .map_err(|_| anyhow!("remote reconnect lock poisoned"))?;
        let latest = self
            .current
            .lock()
            .map_err(|_| anyhow!("remote client lock poisoned"))?
            .clone();
        // 待っている間に他のスレッドが張り直していたか（初回は None かどうかで見る）。
        let superseded = match (&latest, stale) {
            (Some(latest), Some(stale)) => !Arc::ptr_eq(latest, stale),
            (Some(_), None) => true,
            (None, _) => false,
        };
        if superseded {
            if let Some(latest) = latest {
                return Ok(latest);
            }
        }
        // 積み上がり抑制: **自分が待っている間に**他のスレッドが試して失敗したのなら、
        // 同じことをもう一度やらない。到達不能なホストでは 1 回の試行に十数秒かかる
        // （Hello 10s → master 作り直し 10s）ので、待ち行列の全員が順番に払うと
        // 「切れている間はいつまでも終わらない」になる。到着より前の失敗なら自分は
        // 新しい試行＝ユーザーの再操作なので、そのまま繋ぎに行く（復旧を止めない）。
        if let Some((failed_at, message)) = self
            .last_failure
            .lock()
            .ok()
            .and_then(|failure| failure.clone())
        {
            if failed_at > arrived {
                bail!("{message}");
            }
        }
        // 見限る接続に残っている request をここで全部失敗させ、session プロセスも落とす。
        // 待たせたままだと各自のタイムアウトまで（最大 30s）帰ってこないし、書き込みで
        // 詰まったスレッドは writer ロックを握ったまま ssh の自滅（最大 45s）を待つことになる。
        // 失敗した側は自分で `connect_now` に来て、張り直し済みの接続（superseded）を受け取る。
        self.set_state(ConnectionState::Connecting);
        if let Some(dead) = &latest {
            dead.abort("remote connection replaced: 再接続のため古い接続を破棄した");
        }
        let replacement = match connector.connect(self.event_sink.clone()) {
            Ok(replacement) => replacement,
            Err(error) => {
                if let Ok(mut failure) = self.last_failure.lock() {
                    *failure = Some((std::time::Instant::now(), format!("{error:#}")));
                }
                self.set_state(ConnectionState::Disconnected);
                return Err(error);
            }
        };
        if let Ok(mut failure) = self.last_failure.lock() {
            *failure = None;
        }
        *self
            .current
            .lock()
            .map_err(|_| anyhow!("remote client lock poisoned"))? = Some(replacement.clone());
        // 世代を進める → watch keeper が新接続で監視を張り直す。
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.set_state(ConnectionState::Connected);
        Ok(replacement)
    }
}

pub struct RemoteHost {
    id: String,
    display_name: String,
    root: PathBuf,
    project_id: AtomicU64,
    client: Arc<ReconnectingClient>,
    /// watch の入れ替えと Unwatch を直列化する。`open_project` で作る別 root とも共有。
    watch_control: Arc<Mutex<()>>,
    ssh_project: Option<SshProject>,
    transport: Option<Arc<SshTransport>>,
    /// terminal を開く時点での remote-server 実体。遅延接続後の自動配備結果も共有される。
    #[cfg(unix)]
    remote_server_command: Option<Arc<Mutex<String>>>,
}

impl RemoteHost {
    /// system OpenSSH の ControlMaster を作り、互換 server を配備して session proxy へ接続する。
    pub fn connect_ssh(project: &SshProject, server_command: &str) -> Result<Arc<Self>> {
        if server_command.is_empty() || server_command.contains(['\n', '\r', '\0']) {
            bail!("invalid remote server command");
        }
        // 構造化接続ログ: フェーズごとの所要を stderr に出し、失敗はどの段で落ちたかを
        // エラー文脈（context）に載せる → トーストの `{:#}` に段名がそのまま出る。
        let destination = project.destination();
        ssh_log(&destination, "接続開始", None);

        let phase = std::time::Instant::now();
        let transport = SshTransport::connect(project).context("SSH ControlMaster の確立に失敗")?;
        ssh_log(&destination, "ControlMaster 確立", Some(phase));

        let phase = std::time::Instant::now();
        let server_command = transport
            .ensure_remote_server(server_command)
            .context("remote-server の配備に失敗")?;
        ssh_log(&destination, "remote-server 配備", Some(phase));

        // path 未指定（空）= 標準 SSH と同じく remote の $HOME をルートにする（#5・「ホスト選ぶ→home」）。
        let project = if project.path.as_os_str().is_empty() {
            let home = transport
                .output("printf %s \"$HOME\"")
                .context("remote $HOME の解決に失敗")?;
            anyhow::ensure!(
                home.success(),
                "remote の $HOME を取得できない: {}",
                String::from_utf8_lossy(&home.stderr).trim()
            );
            let home = String::from_utf8_lossy(&home.stdout).trim().to_string();
            anyhow::ensure!(!home.is_empty(), "remote の $HOME が空");
            SshProject {
                path: PathBuf::from(home),
                ..project.clone()
            }
        } else {
            project.clone()
        };
        let project = &project;
        let session = random_session_id()?;
        let remote_server_command = Arc::new(Mutex::new(server_command));
        let connector = Arc::new(SshConnector {
            transport: transport.clone(),
            // 対話経路では上で配備済み。再接続で配備し直さないよう ready を立てておく。
            server_command: remote_server_command,
            server_ready: std::sync::atomic::AtomicBool::new(true),
            session,
        });
        let command = connector.command();
        let display_name = project.destination();
        let phase = std::time::Instant::now();
        let host = Self::connect_process_inner(
            command,
            project.identity(),
            display_name,
            &project.path,
            Some(project.clone()),
            Some(transport),
            Some(connector),
        )
        .context("session proxy 接続 / protocol handshake に失敗")?;
        ssh_log(&destination, "session 確立（接続完了）", Some(phase));
        Ok(host)
    }

    /// **繋がずに** SSH host を作る（起動時の復元用）。
    ///
    /// 前回終了時に保存した URI とパスだけで作り、ssh は最初の request まで起こさない。
    /// 窓が出るまでにネットワークを待たないための入口で、`connect_ssh` と違い
    /// 失敗はここでは分からない（最初の request のエラーとして出る）。
    ///
    /// `root` が要るのは、`$HOME` の解決に接続が要るため。空パスは対話経路
    /// （[`Self::connect_ssh`]）でしか扱えない。
    pub fn lazy_ssh(project: &SshProject, server_command: &str) -> Result<Arc<Self>> {
        if server_command.is_empty() || server_command.contains(['\n', '\r', '\0']) {
            bail!("invalid remote server command");
        }
        anyhow::ensure!(
            !project.path.as_os_str().is_empty(),
            "遅延接続には保存済みの root が要る（$HOME 解決は接続が必要）"
        );
        let transport = SshTransport::new(project).context("SSH control directory の準備に失敗")?;
        let remote_server_command = Arc::new(Mutex::new(server_command.to_string()));
        let connector = Arc::new(SshConnector {
            transport: transport.clone(),
            server_command: remote_server_command.clone(),
            // 未配備。最初の接続で `ensure_server` が確かめる。
            server_ready: std::sync::atomic::AtomicBool::new(false),
            session: random_session_id()?,
        });
        let event_sink = Arc::new(WatchEventSink::default());
        let client = ReconnectingClient::disconnected(connector as Arc<dyn Connector>, event_sink);
        Ok(Arc::new(Self {
            id: project.identity(),
            display_name: project.destination(),
            root: project.path.clone(),
            // 未接続なので project id はまだ無い。最初の request が `unknown project id` を
            // 受けて `OpenProject` し直す既存経路に乗る（daemon 再起動時と同じ扱い）。
            project_id: AtomicU64::new(0),
            client,
            watch_control: Arc::new(Mutex::new(())),
            ssh_project: Some(project.clone()),
            transport: Some(transport),
            #[cfg(unix)]
            remote_server_command: Some(remote_server_command),
        }))
    }

    /// test/development 用。指定 process の stdio を同じ protocol として使う。
    pub fn connect_process(
        command: Command,
        id: String,
        display_name: String,
        root: &Path,
    ) -> Result<Arc<Self>> {
        Self::connect_process_inner(command, id, display_name, root, None, None, None)
    }

    fn connect_process_inner(
        command: Command,
        id: String,
        display_name: String,
        root: &Path,
        ssh_project: Option<SshProject>,
        transport: Option<Arc<SshTransport>>,
        connector: Option<Arc<SshConnector>>,
    ) -> Result<Arc<Self>> {
        let event_sink = Arc::new(WatchEventSink::default());
        let client = rpc_client_from_command(command, event_sink.clone())?;
        Self::connect_client(
            client,
            id,
            display_name,
            root,
            ssh_project,
            transport,
            connector,
            event_sink,
        )
    }

    // 唯一の呼び出し元が `UnixStream::pair()` を使う unix 限定テストなので cfg を揃える
    // （揃えないと Windows で dead_code 警告になる）。
    #[cfg(all(test, unix))]
    fn connect_io(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        id: String,
        display_name: String,
        requested_root: &Path,
        ssh_project: Option<SshProject>,
        transport: Option<Arc<SshTransport>>,
    ) -> Result<Arc<Self>> {
        let event_sink = Arc::new(WatchEventSink::default());
        let client = RpcClient::new(reader, writer, None, event_sink.clone());
        Self::connect_client(
            client,
            id,
            display_name,
            requested_root,
            ssh_project,
            transport,
            None,
            event_sink,
        )
    }

    fn connect_client(
        client: Arc<RpcClient>,
        id: String,
        display_name: String,
        requested_root: &Path,
        ssh_project: Option<SshProject>,
        transport: Option<Arc<SshTransport>>,
        connector: Option<Arc<SshConnector>>,
        event_sink: Arc<WatchEventSink>,
    ) -> Result<Arc<Self>> {
        #[cfg(unix)]
        let remote_server_command = connector
            .as_ref()
            .map(|connector| connector.server_command.clone());
        let connector = connector.map(|connector| connector as Arc<dyn Connector>);
        let client = ReconnectingClient::new(client, connector, event_sink);
        let (hello, _) = client.request(
            &Request::Hello {
                client_version: SERVER_VERSION.to_string(),
            },
            Vec::new(),
        )?;
        let Response::Hello {
            protocol_version, ..
        } = hello
        else {
            bail!("remote server returned invalid hello response");
        };
        if protocol_version != PROTOCOL_VERSION {
            bail!("remote protocol mismatch: {protocol_version}");
        }
        let (opened, _) = client.request(
            &Request::OpenProject {
                path: requested_root.to_path_buf(),
            },
            Vec::new(),
        )?;
        let Response::ProjectOpened { project_id, root } = opened else {
            bail!("remote server returned invalid open-project response");
        };
        Ok(Arc::new(Self {
            id,
            display_name,
            root,
            project_id: AtomicU64::new(project_id),
            client,
            watch_control: Arc::new(Mutex::new(())),
            ssh_project,
            transport,
            #[cfg(unix)]
            remote_server_command,
        }))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 障害注入テスト用: ControlMaster を落とす。SSH host のときだけ効く（local proxy は no-op）。
    /// 落とした後の request は [`ReconnectingClient`] が master 再生成 + 再接続で回復するはず。
    #[doc(hidden)]
    pub fn debug_stop_master(&self) {
        if let Some(transport) = &self.transport {
            transport.exit_master();
        }
    }

    /// 障害注入テスト用: これまでに接続を張り直した回数。0 = 一度も再接続していない。
    /// 「復帰したのは張り直したからか、たまたま元の接続が生きていたからか」を切り分ける。
    #[doc(hidden)]
    pub fn debug_reconnect_generation(&self) -> u64 {
        self.client.generation()
    }

    /// 明示的な接続削除・テスト用。通常の Drop では daemon を残して再接続可能にする。
    pub fn shutdown_session(&self) -> Result<()> {
        let (response, _) = self
            .client
            .request_nonretry(&Request::Shutdown, Vec::new())?;
        if matches!(response, Response::Ack) {
            Ok(())
        } else {
            bail!("remote server returned invalid shutdown response")
        }
    }

    fn relative(&self, path: &Path) -> Result<PathBuf> {
        if path == self.root {
            return Ok(PathBuf::new());
        }
        path.strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .with_context(|| format!("path is outside remote project: {}", path.display()))
    }

    fn absolute(&self, relative: PathBuf) -> PathBuf {
        self.root.join(relative)
    }

    fn open_project(&self, requested_root: &Path) -> Result<Arc<Self>> {
        let (opened, _) = self.client.request(
            &Request::OpenProject {
                path: requested_root.to_path_buf(),
            },
            Vec::new(),
        )?;
        let Response::ProjectOpened { project_id, root } = opened else {
            bail!("remote server returned invalid open-project response");
        };
        Ok(Arc::new(Self {
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            root,
            project_id: AtomicU64::new(project_id),
            client: self.client.clone(),
            watch_control: self.watch_control.clone(),
            ssh_project: self.ssh_project.clone(),
            transport: self.transport.clone(),
            #[cfg(unix)]
            remote_server_command: self.remote_server_command.clone(),
        }))
    }

    fn scoped_request(
        &self,
        request: impl Fn(u64) -> Request,
        body: Vec<u8>,
        retry_safe: bool,
    ) -> Result<(Response, Vec<u8>)> {
        let project_id = self.project_id.load(Ordering::Acquire);
        let first = if retry_safe {
            self.client.request(&request(project_id), body.clone())
        } else {
            self.client
                .request_nonretry(&request(project_id), body.clone())
        };
        match first {
            Ok(response) => Ok(response),
            Err(error)
                if error
                    .downcast_ref::<RemoteRequestError>()
                    .is_some_and(|error| error.message.contains("unknown project id")) =>
            {
                // daemon 自体が再起動した場合。最初の request は拒否済みなので再送しても重複しない。
                let (opened, _) = self.client.request(
                    &Request::OpenProject {
                        path: self.root.clone(),
                    },
                    Vec::new(),
                )?;
                let Response::ProjectOpened { project_id, .. } = opened else {
                    bail!("remote server returned invalid reopen-project response");
                };
                self.project_id.store(project_id, Ordering::Release);
                if retry_safe {
                    self.client.request(&request(project_id), body)
                } else {
                    self.client.request_nonretry(&request(project_id), body)
                }
            }
            Err(error) => Err(error),
        }
    }

    /// project 全体（相対 root = 空）の変更監視を daemon へ要求する。冪等（Start は張り直し）。
    fn send_watch(&self) -> Result<()> {
        let (response, _) = self.scoped_request(
            |project_id| Request::Watch {
                project_id,
                root: PathBuf::new(),
            },
            Vec::new(),
            true,
        )?;
        if matches!(response, Response::Ack) {
            Ok(())
        } else {
            bail!("remote watch が不正な応答を返した");
        }
    }

    /// remote watch を開始する: event_sink に配送チャネルを繋ぎ、初回 Watch を送り、
    /// 再接続（generation 変化）で自動的に張り直す keeper を立てる。返した [`HostWatch`] を
    /// drop すると keeper が Unwatch を送って終了する。
    fn start_watch(self: &Arc<Self>) -> Result<HostWatch> {
        use std::sync::atomic::{AtomicBool, Ordering::Acquire};
        let (sender, receiver) = mpsc::channel::<Vec<PathBuf>>();
        let event_sink = self.client.event_sink();
        let subscription_id = {
            // server は 1 connection に watch を 1 本持つ。古い keeper の後始末と新しい
            // Watch が交差しないよう、remote request と sink の入れ替えを同じ lock で束ねる。
            let _control = self
                .watch_control
                .lock()
                .map_err(|_| anyhow!("remote watch control lock poisoned"))?;
            self.send_watch()?;
            event_sink.subscribe(sender)
        };
        let stop = Arc::new(AtomicBool::new(false));
        let keeper_stop = stop.clone();
        let keeper = self.clone();
        let spawned = thread::Builder::new()
            .name("necoder-remote-watch-keeper".to_string())
            .spawn(move || {
                let mut seen = keeper.client.generation();
                while !keeper_stop.load(Acquire) {
                    thread::sleep(Duration::from_millis(1500));
                    if keeper_stop.load(Acquire) {
                        break;
                    }
                    let generation = keeper.client.generation();
                    if generation != seen {
                        let Ok(_control) = keeper.watch_control.lock() else {
                            break;
                        };
                        if !keeper.client.event_sink().is_current(subscription_id) {
                            // 後から始まった watch が現在の購読。古い root を張り直さない。
                            break;
                        }
                        // 再接続が起きた → 新しい接続で監視を張り直す（失敗しても次周で再試行）。
                        seen = generation;
                        let _resubscribed = keeper.send_watch();
                    }
                }
                let Ok(_control) = keeper.watch_control.lock() else {
                    return;
                };
                // 自分がまだ現在の watch のときだけ、best-effort で監視停止 + sink を外す。
                // 入れ替え済みの古い keeper は、新しい watch を Unwatch しない。
                if keeper.client.event_sink().is_current(subscription_id) {
                    let _unwatched = keeper.scoped_request(
                        |project_id| Request::Unwatch { project_id },
                        Vec::new(),
                        true,
                    );
                    keeper.client.event_sink().clear_if_current(subscription_id);
                }
            });
        if let Err(error) = spawned {
            if let Ok(_control) = self.watch_control.lock() {
                if event_sink.is_current(subscription_id) {
                    let _unwatched = self.scoped_request(
                        |project_id| Request::Unwatch { project_id },
                        Vec::new(),
                        true,
                    );
                    event_sink.clear_if_current(subscription_id);
                }
            }
            return Err(error).context("remote watch keeper thread spawn");
        }
        Ok(HostWatch { receiver, stop })
    }
}

impl Host for RemoteHost {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn is_remote(&self) -> bool {
        true
    }

    fn connection_state(&self) -> ConnectionState {
        self.client.state()
    }

    fn watch_connection(&self) -> Option<mpsc::Receiver<ConnectionState>> {
        Some(self.client.subscribe_state())
    }

    fn reconnect(&self) {
        self.client.reconnect_in_background();
    }

    fn project_uri(&self, path: &Path) -> Option<String> {
        self.ssh_project
            .as_ref()
            .map(|project| project.uri_for_path(path))
    }

    fn host_for_project(&self, path: &Path) -> Result<Arc<dyn Host>> {
        self.open_project(path).map(|host| host as Arc<dyn Host>)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        let path = self.relative(path)?;
        let (response, _) = self.scoped_request(
            move |project_id| Request::Canonicalize {
                project_id,
                path: path.clone(),
            },
            Vec::new(),
            true,
        )?;
        match response {
            Response::Path(path) => Ok(self.absolute(path)),
            _ => bail!("invalid canonicalize response"),
        }
    }

    fn metadata(&self, path: &Path) -> Result<HostMetadata> {
        let path = self.relative(path)?;
        let (response, _) = self.scoped_request(
            move |project_id| Request::Metadata {
                project_id,
                path: path.clone(),
            },
            Vec::new(),
            true,
        )?;
        match response {
            Response::Metadata(metadata) => Ok(metadata),
            _ => bail!("invalid metadata response"),
        }
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<HostEntry>> {
        let path = self.relative(path)?;
        let (response, _) = self.scoped_request(
            move |project_id| Request::ReadDir {
                project_id,
                path: path.clone(),
            },
            Vec::new(),
            true,
        )?;
        match response {
            Response::Entries(mut entries) => {
                for entry in &mut entries {
                    entry.path = self.absolute(std::mem::take(&mut entry.path));
                }
                Ok(entries)
            }
            _ => bail!("invalid read-dir response"),
        }
    }

    fn read_file(&self, path: &Path) -> Result<FileContent> {
        let path = self.relative(path)?;
        let (response, body) = self.scoped_request(
            move |project_id| Request::ReadFile {
                project_id,
                path: path.clone(),
            },
            Vec::new(),
            true,
        )?;
        match response {
            Response::File { revision } => Ok(FileContent {
                bytes: body,
                revision,
            }),
            _ => bail!("invalid read-file response"),
        }
    }

    fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        condition: WriteCondition,
    ) -> Result<FileRevision> {
        let path = self.relative(path)?;
        let (response, _) = self.scoped_request(
            move |project_id| Request::WriteFile {
                project_id,
                path: path.clone(),
                condition: condition.clone(),
            },
            bytes.to_vec(),
            false,
        )?;
        match response {
            Response::Written { revision } => Ok(revision),
            _ => bail!("invalid write-file response"),
        }
    }

    fn list_files(&self, root: &Path, limit: usize) -> Result<Vec<PathBuf>> {
        let root = self.relative(root)?;
        let (response, _) = self.scoped_request(
            move |project_id| Request::ListFiles {
                project_id,
                root: root.clone(),
                limit,
            },
            Vec::new(),
            true,
        )?;
        match response {
            Response::Paths(paths) => {
                Ok(paths.into_iter().map(|path| self.absolute(path)).collect())
            }
            _ => bail!("invalid list-files response"),
        }
    }

    fn search_project(
        &self,
        root: &Path,
        spec: &TextSearchSpec,
        file_limit: usize,
    ) -> Result<Vec<TextSearchHit>> {
        let root = self.relative(root)?;
        let spec = spec.clone();
        let (response, _) = self.scoped_request(
            move |project_id| Request::SearchProject {
                project_id,
                root: root.clone(),
                spec: spec.clone(),
                file_limit,
            },
            Vec::new(),
            true,
        )?;
        match response {
            Response::SearchHits(mut hits) => {
                for hit in &mut hits {
                    hit.path = self.absolute(std::mem::take(&mut hit.path));
                }
                Ok(hits)
            }
            _ => bail!("invalid search response"),
        }
    }

    fn run_command(&self, spec: &CommandSpec) -> Result<CommandOutput> {
        let mut spec = spec.clone();
        spec.cwd = self.relative(&spec.cwd)?;
        let (response, body) = self.scoped_request(
            move |project_id| Request::RunCommand {
                project_id,
                spec: spec.clone(),
            },
            Vec::new(),
            false,
        )?;
        match response {
            Response::Command {
                status_code,
                stdout_len,
                stderr_len,
            } => {
                if stdout_len.checked_add(stderr_len) != Some(body.len()) {
                    bail!("invalid command output lengths");
                }
                Ok(CommandOutput {
                    status_code,
                    stdout: body[..stdout_len].to_vec(),
                    stderr: body[stdout_len..].to_vec(),
                })
            }
            _ => bail!("invalid command response"),
        }
    }

    fn spawn_process(&self, spec: &CommandSpec) -> Result<HostProcess> {
        let transport = self
            .transport
            .clone()
            .context("SSH process transport が無い")?;
        // ここで `ensure_master` は呼ばない（UI スレッドから来る＝接続待ちで固まる）。
        // session は master が無ければ自分で繋ぐ（standalone）。
        self.relative(&spec.cwd)?;
        for key in spec.env.keys() {
            if !valid_env_key(key) {
                bail!("不正な environment key: {key}");
            }
        }
        let mut words = vec![quote_posix(&spec.program)];
        words.extend(spec.args.iter().map(|arg| quote_posix(arg)));
        let env = spec
            .env
            .iter()
            .map(|(key, value)| quote_posix(&format!("{key}={value}")))
            .collect::<Vec<_>>()
            .join(" ");
        let env = if env.is_empty() {
            String::new()
        } else {
            format!("env {env} ")
        };
        let remote_command = format!(
            "cd {} && exec {env}{}",
            quote_posix(&spec.cwd.to_string_lossy()),
            words.join(" ")
        );
        let mut child = transport
            .standalone_command(false, &remote_command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("remote process を起動できない: {}", spec.program))?;
        let stdin = child.stdin.take().context("remote process stdin が無い")?;
        let stdout = child
            .stdout
            .take()
            .context("remote process stdout が無い")?;
        Ok(HostProcess {
            child,
            stdin: Some(Box::new(stdin)),
            stdout: Some(Box::new(stdout)),
            _transport: Some(transport),
        })
    }

    fn terminal_launch(&self, cwd: &Path) -> Result<Option<TerminalLaunch>> {
        let transport = self
            .transport
            .as_ref()
            .context("SSH terminal transport が無い")?;
        // 起動仕様を組むだけ。ネットワークには触らない（session が master 無しでも自分で繋ぐ）。
        // 以前はここで `ensure_master` を呼んでおり、端末を開くたび・project を開くたびに
        // UI スレッドで接続確認（切れていれば張り直し）を待っていた。
        self.relative(cwd)?;
        let basic_command = || {
            format!(
                "cd {} && exec \"${{SHELL:-/bin/sh}}\" -l",
                quote_posix(&cwd.to_string_lossy())
            )
        };
        #[cfg(unix)]
        let remote_command = match (
            &transport.remote_cli_forward,
            self.remote_server_command
                .as_ref()
                .and_then(|command| command.lock().ok())
                .map(|command| command.clone()),
        ) {
            (Some(forward), Some(server_command)) if !server_command.is_empty() => {
                // login shell の中でも `ne` が必ずこの接続元へ戻るよう、配備済みの軽量
                // remote-server を CLI として呼ぶ専用 shim を一時 PATH の先頭へ置く。
                let shim = format!(
                    "#!/bin/sh\nexec {} cli \"$@\"\n",
                    quote_posix(&server_command)
                );
                format!(
                    "umask 077 && mkdir -p {bin} && printf %s {shim_body} > {shim} && \
                     chmod 700 {shim} && cd {cwd} && \
                     export PATH={bin}:\"$PATH\" NECODER_GUI_SOCK={socket} && \
                     exec \"${{SHELL:-/bin/sh}}\" -l",
                    bin = quote_posix(&forward.remote_bin_dir.to_string_lossy()),
                    shim_body = quote_posix(&shim),
                    shim = quote_posix(&forward.remote_bin_dir.join("ne").to_string_lossy()),
                    cwd = quote_posix(&cwd.to_string_lossy()),
                    socket = quote_posix(&forward.remote_socket.to_string_lossy()),
                )
            }
            _ => basic_command(),
        };
        #[cfg(not(unix))]
        let remote_command = basic_command();
        Ok(Some(TerminalLaunch {
            program: "ssh".to_string(),
            args: transport.session_args_with(true, &remote_command, true),
        }))
    }

    fn watch(self: Arc<Self>) -> Result<Option<HostWatch>> {
        Ok(Some(self.start_watch()?))
    }
}

fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn quote_posix(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn remote_trace(message: &str) {
    if std::env::var_os("NECODER_REMOTE_TRACE").is_some() {
        eprintln!("[remote-trace] {message}");
    }
}

fn random_session_id() -> Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .context("session id 用の OS random source を開けない")?
        .read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod windows_terminal_tests {
    use super::*;

    /// **`pwsh` を優先する**（W4 の受入条件）。VSCode / Terminal と同じ流儀で、
    /// PowerShell 7 が入っていればそちらを使う。
    #[test]
    fn pwsh_wins_when_it_is_installed() {
        let shell =
            pick_windows_shell(|candidate| candidate == "pwsh" || candidate == "powershell");
        assert_eq!(shell.program, "pwsh");
        assert!(shell.args.is_empty());
    }

    /// pwsh が無ければ OS 同梱の `powershell`。
    #[test]
    fn falls_back_to_bundled_powershell() {
        let shell = pick_windows_shell(|candidate| candidate == "powershell");
        assert_eq!(shell.program, "powershell");
    }

    /// どちらも見つからない場合でも**空文字を返さない**。
    /// 空だと alacritty 側が別の既定に倒れて、何が起動したのか分からなくなる。
    #[test]
    fn never_yields_an_empty_program() {
        let shell = pick_windows_shell(|_| false);
        assert_eq!(shell.program, "powershell");
    }

    /// **`.cmd` を直接 spawn できるか**（WINDOWS-PORT.md §W4 / §4）。
    ///
    /// ACP の `claude` は Windows では `claude.cmd`。§4 には「`CreateProcess` は `.cmd` を直接
    /// 実行できない」と書いてあるが、**Rust の `std::process::Command` は `.bat` / `.cmd` を検出して
    /// `cmd.exe` 経由で起動する**（CVE-2024-24576 の対応以降）。necoder は生の `CreateProcess` を
    /// 使っていないので、この罠には**当たらない**はず。
    ///
    /// 「はず」で済ませると Rust 側の挙動が変わったときに黙って壊れるので、ここで固定する。
    #[cfg(windows)]
    #[test]
    fn cmd_scripts_can_be_spawned_directly() {
        let dir = std::env::temp_dir().join(format!("necoder-cmd-spawn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れない");
        let script = dir.join("necoder-probe.cmd");
        std::fs::write(&script, "@echo off\r\necho NECODER_CMD_OK\r\n")
            .expect("スクリプトを書けない");

        let spec = CommandSpec::new(script.to_string_lossy(), &dir);
        let mut process = spawn_process_local(&spec, None)
            .expect(".cmd を起動できない（Rust の Command が .cmd を扱えなくなった可能性）");
        let mut stdout = process.take_stdout().expect("stdout が無い");
        let mut output = String::new();
        stdout
            .read_to_string(&mut output)
            .expect("stdout を読めない");

        assert!(
            output.contains("NECODER_CMD_OK"),
            "`.cmd` の出力が取れない（実際の出力: {output:?}）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// unix では launch を返さない＝alacritty の `$SHELL` 既定に任せる（mac の挙動不変・§D8）。
    /// Windows では必ず返す（返さないと `powershell` 固定になり pwsh が選べない）。
    #[test]
    fn local_launch_is_returned_only_on_windows() {
        let launch = LocalHost
            .terminal_launch(Path::new("."))
            .expect("terminal_launch は失敗しない");
        if cfg!(windows) {
            let launch = launch.expect("Windows では既定シェルを明示する");
            assert!(
                launch.program == "pwsh" || launch.program == "powershell",
                "想定外のシェル: {}",
                launch.program
            );
        } else {
            assert!(launch.is_none(), "unix は alacritty の既定に任せる");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;

    #[test]
    fn host_watch_distinguishes_timeout_from_disconnection() {
        let (sender, receiver) = mpsc::channel();
        let watch = HostWatch {
            receiver,
            stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

        assert_eq!(
            watch.recv_timeout(Duration::from_millis(1)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        drop(sender);
        assert_eq!(
            watch.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected),
            "sender 切断は即時に判別でき、pump が busy loop から抜けられる"
        );
    }

    #[test]
    fn stale_watch_cannot_clear_replacement_subscription() {
        let sink = WatchEventSink::default();
        let (first_sender, first_receiver) = mpsc::channel();
        let first = sink.subscribe(first_sender);
        let (second_sender, second_receiver) = mpsc::channel();
        let second = sink.subscribe(second_sender);

        assert_eq!(
            first_receiver.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected),
            "購読の入れ替えで古い pump を終了させる"
        );
        assert!(!sink.clear_if_current(first));
        sink.dispatch(WatchEvent {
            paths: vec![PathBuf::from("new.txt")],
        });
        assert_eq!(
            second_receiver.recv_timeout(Duration::from_secs(1)),
            Ok(vec![PathBuf::from("new.txt")]),
            "古い keeper の後始末で新しい購読を外さない"
        );
        assert!(sink.clear_if_current(second));
        assert_eq!(
            second_receiver.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    fn scratch(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "necoder-host-{tag}-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        let _removed = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

    #[cfg(unix)]
    fn short_socket_scratch() -> PathBuf {
        let path = PathBuf::from(format!(
            "/tmp/necoder-gw-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        let _removed = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn remote_cli_gateway_only_forwards_open_with_trusted_authority() {
        use std::os::unix::net::UnixListener;

        let root = short_socket_scratch();
        let gui_socket = root.join("gui.sock");
        let gateway_socket = root.join("gateway.sock");
        let gui_listener = UnixListener::bind(&gui_socket).unwrap();
        let gui = thread::spawn(move || {
            let (mut stream, _) = gui_listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["method"], "open_remote");
            assert_eq!(request["params"]["authority"], "ssh://dev@example.com");
            assert_eq!(request["params"]["paths"][0], "/srv/project");
            writeln!(
                stream,
                "{}",
                serde_json::json!({ "ok": true, "result": { "opened": 1 } })
            )
            .unwrap();
        });
        let gateway = RemoteCliGateway::bind(
            gateway_socket.clone(),
            gui_socket,
            "ssh://dev@example.com".to_string(),
        )
        .unwrap();
        let mut remote = UnixStream::connect(gateway_socket).unwrap();
        writeln!(
            remote,
            "{}",
            serde_json::json!({
                "method": "open",
                "params": { "paths": ["/srv/project"] },
            })
        )
        .unwrap();
        let mut response = String::new();
        BufReader::new(remote).read_line(&mut response).unwrap();
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["opened"], 1);
        gui.join().unwrap();
        drop(gateway);
        let _removed = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn remote_cli_gateway_rejects_control_methods() {
        let root = short_socket_scratch();
        let gateway_socket = root.join("gateway.sock");
        let gateway = RemoteCliGateway::bind(
            gateway_socket.clone(),
            root.join("no-gui.sock"),
            "ssh://dev@example.com".to_string(),
        )
        .unwrap();
        let mut remote = UnixStream::connect(gateway_socket).unwrap();
        writeln!(
            remote,
            "{}",
            serde_json::json!({ "method": "send", "params": {} })
        )
        .unwrap();
        let mut response = String::new();
        BufReader::new(remote).read_line(&mut response).unwrap();
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["ok"], false);
        assert!(response["error"]
            .as_str()
            .is_some_and(|error| error.contains("open だけ")));
        drop(gateway);
        let _removed = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_ssh_config_hosts() {
        let text = "\
# コメント行
Host web1 web2
    HostName 10.0.0.1
    User deploy

Host *
    ForwardAgent yes

Host gpu
  HostName=gpu.example.com
  User = alice
";
        let hosts = parse_ssh_config(text);
        assert_eq!(hosts.len(), 3, "web1/web2/gpu の 3 件（* は除外）");
        assert_eq!(hosts[0].alias, "web1");
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.1"));
        assert_eq!(hosts[0].user.as_deref(), Some("deploy"));
        // 複数 alias は同ブロックの HostName/User を共有。
        assert_eq!(hosts[1].alias, "web2");
        assert_eq!(hosts[1].user.as_deref(), Some("deploy"));
        // `=` 区切り・前後空白を許容。
        assert_eq!(hosts[2].alias, "gpu");
        assert_eq!(hosts[2].hostname.as_deref(), Some("gpu.example.com"));
        assert_eq!(hosts[2].user.as_deref(), Some("alice"));
        // ワイルドカードは一覧に出さない。
        assert!(hosts.iter().all(|host| host.alias != "*"));
    }

    #[test]
    fn remote_target_triple_maps_uname_to_musl() {
        // Linux は static-musl・arch 別名（amd64/arm64）も吸収する（#1 の自動発見キー）。
        assert_eq!(
            remote_target_triple("Linux", "x86_64").as_deref(),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            remote_target_triple("linux", "aarch64").as_deref(),
            Some("aarch64-unknown-linux-musl")
        );
        assert_eq!(
            remote_target_triple("Linux", "arm64").as_deref(),
            Some("aarch64-unknown-linux-musl")
        );
        assert_eq!(
            remote_target_triple("Darwin", "arm64").as_deref(),
            Some("aarch64-apple-darwin")
        );
        // 未知の OS/arch は None（＝自動発見せず明示指定を促す）。
        assert_eq!(remote_target_triple("Windows", "x86_64"), None);
        assert_eq!(remote_target_triple("Linux", "riscv64"), None);
    }

    #[test]
    fn parse_sha256_hex_reads_both_gnu_and_bsd_output() {
        let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        // GNU sha256sum: `<hex>  <path>`
        assert_eq!(
            parse_sha256_hex(format!("{hash}  server\n").as_bytes()).as_deref(),
            Some(hash)
        );
        // BSD/macOS shasum -a 256: 同形式・大文字も吸収
        assert_eq!(
            parse_sha256_hex(format!("{}  ./server", hash.to_ascii_uppercase()).as_bytes())
                .as_deref(),
            Some(hash)
        );
        // GNU の filename エスケープ（行頭 `\`）を剥がす
        assert_eq!(
            parse_sha256_hex(format!("\\{hash}  weird\\nname").as_bytes()).as_deref(),
            Some(hash)
        );
        // ツール不在などで空/非 hex のときは None
        assert_eq!(parse_sha256_hex(b""), None);
        assert_eq!(parse_sha256_hex(b"sha256sum: not found"), None);
        assert_eq!(parse_sha256_hex(b"abc123  short"), None);
    }

    #[test]
    fn frame_round_trip_and_limits() {
        let request = Request::Ping;
        let frame = Frame::request(42, &request, b"body".to_vec()).unwrap();
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        let decoded = read_frame(&mut bytes.as_slice()).unwrap();
        assert_eq!(decoded.id, 42);
        assert_eq!(decoded.kind, FrameKind::Request);
        assert_eq!(decoded.body, b"body");
        assert!(matches!(
            serde_json::from_slice(&decoded.meta).unwrap(),
            Request::Ping
        ));

        let mut corrupt = bytes;
        corrupt[20..28].copy_from_slice(&((MAX_BODY_LEN as u64) + 1).to_le_bytes());
        assert!(read_frame(&mut corrupt.as_slice()).is_err());
    }

    #[test]
    fn local_atomic_write_detects_external_change() {
        let root = scratch("conflict");
        let path = root.join("file.txt");
        std::fs::write(&path, "one").unwrap();
        let first = read_file_local(&path).unwrap();
        std::fs::write(&path, "two").unwrap();
        let error =
            write_file_local(&path, b"mine", WriteCondition::Matches(first.revision)).unwrap_err();
        assert!(error.to_string().contains("保存競合"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        let _removed = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_atomic_write_recreates_externally_deleted_file() {
        // 外部削除は競合にしない（上書きで壊す相手が居ない）= 未保存の作業を ⌘S で救出できる。
        let root = scratch("recreate");
        let path = root.join("file.txt");
        std::fs::write(&path, "one").unwrap();
        let first = read_file_local(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        let revision =
            write_file_local(&path, b"rescued", WriteCondition::Matches(first.revision)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "rescued");
        // 作り直し後の revision で続きの保存も通る（普通の編集ループに復帰）。
        write_file_local(&path, b"rescued again", WriteCondition::Matches(revision)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "rescued again");
        let _removed = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ssh_uri_parsing_preserves_identity_and_path() {
        let project = SshProject::parse("ssh://alice@example.com:2222/home/alice/a%20b").unwrap();
        assert_eq!(project.host, "example.com");
        assert_eq!(project.username.as_deref(), Some("alice"));
        assert_eq!(project.port, Some(2222));
        assert_eq!(project.path, Path::new("/home/alice/a b"));
        assert_eq!(project.destination(), "alice@example.com");
        assert_eq!(project.identity(), "ssh://alice@example.com:2222");
        assert_eq!(
            project.uri_for_path(Path::new("/home/alice/a b")),
            "ssh://alice@example.com:2222/home/alice/a%20b"
        );
        assert!(SshProject::parse("ssh://alice:secret@example.com/code").is_err());
        assert!(SshProject::parse("ssh://example.com/code?proxy=bad").is_err());
    }

    #[test]
    fn ssh_uri_without_path_means_home() {
        // path 未指定は「空」= home マーカー（接続時に remote の $HOME へ解決・#5）。もう bail しない。
        for uri in [
            "ssh://example.com",
            "ssh://example.com/",
            "ssh://user@example.com/~",
        ] {
            let project = SshProject::parse(uri).unwrap_or_else(|error| panic!("{uri}: {error:#}"));
            assert!(
                project.path.as_os_str().is_empty(),
                "{uri} は home（空パス）のはず: {:?}",
                project.path
            );
            assert_eq!(project.host, "example.com");
        }
        // 具体パスは従来どおり保持する。
        assert_eq!(
            SshProject::parse("ssh://example.com/srv/app").unwrap().path,
            Path::new("/srv/app")
        );
    }

    #[cfg(unix)]
    #[test]
    fn remote_protocol_reads_writes_lists_and_runs_processes() {
        let scratch_root = scratch("rpc");
        std::fs::create_dir_all(scratch_root.join("src")).unwrap();
        std::fs::write(
            scratch_root.join("src/lib.rs"),
            "pub fn one() -> u8 { 1 }\n",
        )
        .unwrap();

        // UnixStream pair は SSH stdio と同じ全二重 byte stream。sandbox 内でも実 protocol を通せる。
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let reader = server_stream.try_clone().unwrap();
            serve_stream(reader, server_stream).unwrap();
        });
        let reader = Box::new(client_stream.try_clone().unwrap());
        let writer = Box::new(client_stream);
        let remote = RemoteHost::connect_io(
            reader,
            writer,
            "test".to_string(),
            "Test Remote".to_string(),
            &scratch_root,
            None,
            None,
        )
        .unwrap();

        let root = remote.root().to_path_buf();
        let file = root.join("src/lib.rs");
        let content = remote.read_file(&file).unwrap();
        assert_eq!(content.bytes, b"pub fn one() -> u8 { 1 }\n");
        remote
            .write_file(
                &file,
                b"pub fn two() -> u8 { 2 }\n",
                WriteCondition::Matches(content.revision),
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "pub fn two() -> u8 { 2 }\n"
        );

        let entries = remote.read_dir(&root.join("src")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, file);
        let files = remote.list_files(&root, 50).unwrap();
        assert_eq!(files, vec![file.clone()]);

        let output = remote
            .run_command(&CommandSpec::new("git", &root).args(["--version"]))
            .unwrap();
        assert!(output.success());
        assert!(String::from_utf8_lossy(&output.stdout).starts_with("git version"));

        let hits = remote
            .search_project(
                &root,
                &TextSearchSpec {
                    pattern: "pub fn TWO".to_string(),
                    is_regex: false,
                    case_sensitive: false,
                    max_matches: 10,
                },
                100,
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, file);
        assert_eq!(hits[0].line, 0);

        // 長い process request 中でも metadata/heartbeat が詰まらないことを実 wire で検証する。
        let gate = root.join("gate.fifo");
        assert!(Command::new("mkfifo")
            .arg(&gate)
            .status()
            .unwrap()
            .success());
        let started = root.join("process-started");
        let process_remote = remote.clone();
        let process_root = root.clone();
        let process = thread::spawn(move || {
            process_remote
                .run_command(
                    &CommandSpec::new("sh", &process_root)
                        .args(["-c", "touch process-started && cat gate.fifo >/dev/null"]),
                )
                .unwrap()
        });
        for _ in 0..100 {
            if started.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(started.exists(), "blocking process が開始されなかった");
        let release_gate = gate.clone();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_secs(2));
            std::fs::write(release_gate, b"release").unwrap();
        });
        let before = std::time::Instant::now();
        let metadata = remote.metadata(&file).unwrap();
        assert!(metadata.is_file);
        assert!(
            before.elapsed() < Duration::from_millis(500),
            "metadata request が長い process の後ろで直列化された"
        );
        release.join().unwrap();
        assert!(process.join().unwrap().success());

        let error = remote.read_file(&root.join("../outside")).unwrap_err();
        assert!(error.downcast_ref::<RemoteRequestError>().is_some());
        let (shutdown, _) = remote
            .client
            .request(&Request::Shutdown, Vec::new())
            .unwrap();
        assert!(matches!(shutdown, Response::Ack));
        drop(remote);
        server.join().unwrap();
        let _removed = std::fs::remove_dir_all(scratch_root);
    }

    /// UnixStream の対で実 protocol を喋る connector。ssh 無しで [`ReconnectingClient`] の
    /// 再接続ロジック（見限り・張り直し・透過再送）を検証するための差し替え。
    #[cfg(unix)]
    struct PairConnector {
        connections: AtomicU64,
    }

    #[cfg(unix)]
    impl Connector for PairConnector {
        fn connect(&self, event_sink: Arc<WatchEventSink>) -> Result<Arc<RpcClient>> {
            self.connections.fetch_add(1, Ordering::Relaxed);
            let (client_stream, server_stream) = UnixStream::pair()?;
            thread::spawn(move || {
                let reader = server_stream.try_clone().expect("server stream clone");
                let _served = serve_stream(reader, server_stream);
            });
            let reader = Box::new(client_stream.try_clone()?);
            let client = RpcClient::new(reader, Box::new(client_stream), None, event_sink);
            verify_hello(&client)?;
            Ok(client)
        }
    }

    /// 相手が黙っている接続（sleep 復帰直後の「TCP だけ死んだ」の再現）。server 側の端を返す
    /// ので、drop すれば client の reader は EOF で終わる。
    #[cfg(unix)]
    fn silent_client(event_sink: Arc<WatchEventSink>) -> (Arc<RpcClient>, UnixStream) {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let reader = Box::new(client_stream.try_clone().unwrap());
        (
            RpcClient::new(reader, Box::new(client_stream), None, event_sink),
            server_stream,
        )
    }

    /// 到達不能ホストの再現: 接続を張ろうとすると必ず失敗する connector。
    struct UnreachableConnector;

    impl Connector for UnreachableConnector {
        fn connect(&self, _event_sink: Arc<WatchEventSink>) -> Result<Arc<RpcClient>> {
            bail!("host unreachable (test)")
        }
    }

    /// 接続状態は「張り直しの前後」で Connecting → Connected と遷移し、購読者に変化だけが届く。
    /// statusbar の SSH チップはこの遷移を見て色と再接続ボタンを出す（2026-09-08）。
    #[cfg(unix)]
    #[test]
    fn connection_state_follows_reconnect() {
        let sink = Arc::new(WatchEventSink::default());
        let (dead, _server_end) = silent_client(sink.clone());
        let connector = Arc::new(PairConnector {
            connections: AtomicU64::new(0),
        });
        let client =
            ReconnectingClient::new(dead, Some(connector.clone() as Arc<dyn Connector>), sink);
        assert_eq!(client.state(), ConnectionState::Connected);
        let states = client.subscribe_state();

        // 黙っている接続を見限って張り直す（heartbeat 相当）。
        assert!(client.heartbeat_once(Duration::from_millis(200)));
        assert_eq!(client.state(), ConnectionState::Connected);
        assert_eq!(
            states.try_iter().collect::<Vec<_>>(),
            vec![ConnectionState::Connecting, ConnectionState::Connected],
            "張り直しの開始と完了が順に届く"
        );
        assert_eq!(connector.connections.load(Ordering::Relaxed), 1);
    }

    /// 張り直しに失敗すると Disconnected に落ち、遅延接続はまだ繋いでいない Unconnected で始まる。
    #[cfg(unix)]
    #[test]
    fn connection_state_reports_disconnected_when_reconnect_fails() {
        let sink = Arc::new(WatchEventSink::default());
        let (dead, _server_end) = silent_client(sink.clone());
        let client = ReconnectingClient::new(
            dead,
            Some(Arc::new(UnreachableConnector) as Arc<dyn Connector>),
            sink,
        );
        let states = client.subscribe_state();
        assert!(!client.heartbeat_once(Duration::from_millis(200)));
        assert_eq!(client.state(), ConnectionState::Disconnected);
        assert_eq!(
            states.try_iter().collect::<Vec<_>>(),
            vec![ConnectionState::Connecting, ConnectionState::Disconnected]
        );

        let lazy = ReconnectingClient::disconnected(
            Arc::new(UnreachableConnector) as Arc<dyn Connector>,
            Arc::new(WatchEventSink::default()),
        );
        assert_eq!(lazy.state(), ConnectionState::Unconnected);
    }

    #[cfg(unix)]
    #[test]
    fn request_timeout_is_typed_and_distinguishable_from_close() {
        let sink = Arc::new(WatchEventSink::default());
        let (silent, server_end) = silent_client(sink);
        let before = silent.received_bytes();
        let error = silent
            .request_with_timeout(&Request::Ping, Vec::new(), Duration::from_millis(200))
            .unwrap_err();
        assert!(
            error.downcast_ref::<RequestTimeout>().is_some(),
            "黙っている相手は時間切れ: {error:#}"
        );
        assert_eq!(silent.received_bytes(), before, "何も受信していない");

        // 相手が閉じたときは「時間切れ」ではなく「閉じた」。master を作り直す判定に効く。
        drop(server_end);
        let error = silent.request(&Request::Ping, Vec::new()).unwrap_err();
        assert!(
            error.downcast_ref::<RequestTimeout>().is_none(),
            "閉じた接続は時間切れ扱いにしない: {error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reconnect_fails_requests_waiting_on_the_dead_connection_fast() {
        let scratch_root = scratch("reconnect");
        let sink = Arc::new(WatchEventSink::default());
        let (dead, _server_end) = silent_client(sink.clone());
        let connector = Arc::new(PairConnector {
            connections: AtomicU64::new(0),
        });
        let client =
            ReconnectingClient::new(dead, Some(connector.clone() as Arc<dyn Connector>), sink);

        // 死んだ接続に乗った request（通常の 30s タイムアウト）。
        let waiter_client = client.clone();
        let waiter_root = scratch_root.clone();
        let waiter = thread::spawn(move || {
            let started = std::time::Instant::now();
            let result =
                waiter_client.request(&Request::OpenProject { path: waiter_root }, Vec::new());
            (result, started.elapsed())
        });
        thread::sleep(Duration::from_millis(150));

        // heartbeat 相当: 黙っている接続を見限って張り直す。
        assert!(client.heartbeat_once(Duration::from_millis(200)));

        // 待っていた request は 30s を待たされず、張り直した接続で透過的に成功する。
        let (result, elapsed) = waiter.join().unwrap();
        let (response, _) = result.expect("張り直した接続で成功する");
        assert!(matches!(response, Response::ProjectOpened { .. }));
        assert!(
            elapsed < Duration::from_secs(5),
            "古い接続のタイムアウトを待たされた: {elapsed:?}"
        );
        assert_eq!(connector.connections.load(Ordering::Relaxed), 1);
        assert_eq!(client.generation(), 1);
        let _removed = std::fs::remove_dir_all(scratch_root);
    }

    /// 必ず失敗する connector（到達不能なホストの代役）。1 回の試行に時間がかかることまで含めて
    /// 再現するため、失敗の前に少し眠る。
    #[cfg(unix)]
    struct FailingConnector {
        attempts: Arc<AtomicU64>,
        delay: Duration,
    }

    #[cfg(unix)]
    impl Connector for FailingConnector {
        fn connect(&self, _event_sink: Arc<WatchEventSink>) -> Result<Arc<RpcClient>> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            thread::sleep(self.delay);
            bail!("host に到達できない（テスト）")
        }
    }

    /// 切れている間に溜まった待ち人が、それぞれ再接続を試し直さない。
    ///
    /// 到達不能なホストでは 1 回の試行に十数秒かかる。待ち行列の全員が順番に払うと
    /// 「切れている間は何をしても終わらない」になる（2026-09-07 のユーザー報告の体感）。
    /// 待っている間に起きた失敗はそのまま受け取り、**到着より前**の失敗しか無いとき
    /// （＝ユーザーの新しい操作）だけ繋ぎに行く。
    #[cfg(unix)]
    #[test]
    fn queued_requests_do_not_each_pay_for_their_own_failed_reconnect() {
        let sink = Arc::new(WatchEventSink::default());
        let (dead, _server_end) = silent_client(sink.clone());
        let attempts = Arc::new(AtomicU64::new(0));
        let connector = Arc::new(FailingConnector {
            attempts: attempts.clone(),
            delay: Duration::from_millis(500),
        });
        let client =
            ReconnectingClient::new(dead.clone(), Some(connector as Arc<dyn Connector>), sink);

        let waiters: Vec<_> = (0..4)
            .map(|_| {
                let client = client.clone();
                let dead = dead.clone();
                thread::spawn(move || client.connect_now(Some(&dead)).is_err())
            })
            .collect();
        for waiter in waiters {
            assert!(waiter.join().unwrap(), "到達不能なので全員 Err で返る");
        }
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            1,
            "待ち行列の全員が再接続を試し直している（切断中に操作するほど遅くなる）"
        );

        // 後から来た操作（＝ユーザーの再試行）は、前の失敗に足を引っ張られず繋ぎに行く。
        assert!(client.connect_now(Some(&dead)).is_err());
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            2,
            "失敗の後に来た操作まで抑止すると、復旧できなくなる"
        );
    }

    #[cfg(unix)]
    #[test]
    fn heartbeat_keeps_a_slow_but_flowing_connection() {
        let sink = Arc::new(WatchEventSink::default());
        let (busy, mut server_end) = silent_client(sink.clone());
        let connector = Arc::new(PairConnector {
            connections: AtomicU64::new(0),
        });
        let client = ReconnectingClient::new(
            busy.clone(),
            Some(connector.clone() as Arc<dyn Connector>),
            sink,
        );

        // 相手は「大きな応答の途中」: 誰も待っていない id の応答を少しずつ流す。Pong はその後ろ。
        let mut bytes = Vec::new();
        let frame = Frame::response(999, &Response::Pong, vec![0u8; 4096]).unwrap();
        write_frame(&mut bytes, &frame).unwrap();
        let dripper = thread::spawn(move || {
            for chunk in bytes.chunks(128) {
                server_end.write_all(chunk).expect("drip");
                thread::sleep(Duration::from_millis(15));
            }
            server_end
        });

        let received_before = busy.received_bytes();
        assert!(
            client.heartbeat_once(Duration::from_millis(250)),
            "受信が進んでいる接続は生きている扱い"
        );
        assert!(busy.received_bytes() > received_before);
        assert_eq!(
            connector.connections.load(Ordering::Relaxed),
            0,
            "混んでいるだけの接続を張り直してはいけない"
        );
        assert_eq!(client.generation(), 0);
        drop(dripper.join().unwrap());
    }
}
