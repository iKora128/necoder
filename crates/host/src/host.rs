//! local/remote 共通の host 境界と Remote SSH wire protocol。
//!
//! `std::fs` / `std::process` を UI・project model から隔離し、同じ API を local process と
//! SSH 上の `necoder-remote-server` へ向ける。設計根拠は
//! `docs/research/remote-ssh-2026.md`。Zed の GPL 実装は移植せず、公開仕様を基に独立実装する。

use anyhow::{anyhow, bail, Context as _, Result};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

pub const PROTOCOL_VERSION: u16 = 2;
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

const FRAME_MAGIC: [u8; 4] = *b"SHRS";
const FRAME_HEADER_LEN: usize = 28;
const MAX_META_LEN: usize = 8 * 1024 * 1024;
const MAX_BODY_LEN: usize = 256 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SEARCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const COMMAND_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

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
    /// 変更通知を timeout 付きで待つ（daemon の poll 差分・相対パス列）。無ければ `None`。
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Vec<PathBuf>> {
        self.receiver.recv_timeout(timeout).ok()
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
        paths::canonicalize(path)
            .with_context(|| format!("パスを解決できない: {}", path.display()))
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
    fn timeout(&self) -> Duration {
        match self {
            Self::SearchProject { .. } => SEARCH_REQUEST_TIMEOUT,
            Self::RunCommand { .. } => COMMAND_REQUEST_TIMEOUT,
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

pub fn serve_remote_server_cli() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
        _ => bail!("使い方: necoder-remote-server <serve --stdio | proxy --session ID | version>"),
    }
}

// ── Multiplexed RPC client ──

struct ProcessOwner(Mutex<Option<Child>>);

impl Drop for ProcessOwner {
    fn drop(&mut self) {
        let Ok(child) = self.0.get_mut() else {
            return;
        };
        if let Some(child) = child.as_mut() {
            let _kill = child.kill();
            let _wait = child.wait();
        }
    }
}

type PendingResponses = Arc<Mutex<HashMap<u64, mpsc::SyncSender<Result<Frame, String>>>>>;

/// remote watch の [`FrameKind::Event`] frame を現在の購読者へ配る共有シンク。RpcClient を跨いで
/// （再接続でも）生き続けるよう [`ReconnectingClient`] が Arc で保持し、各 RpcClient の reader が
/// Event をここへ流す。購読が無ければ捨てる。
#[derive(Default)]
struct WatchEventSink {
    sender: Mutex<Option<mpsc::Sender<Vec<PathBuf>>>>,
}

impl WatchEventSink {
    fn set(&self, sender: Option<mpsc::Sender<Vec<PathBuf>>>) {
        if let Ok(mut slot) = self.sender.lock() {
            *slot = sender;
        }
    }

    fn dispatch(&self, event: WatchEvent) {
        if let Ok(slot) = self.sender.lock() {
            if let Some(sender) = slot.as_ref() {
                let _sent = sender.send(event.paths);
            }
        }
    }
}

struct RpcClient {
    writer: Mutex<Box<dyn Write + Send>>,
    pending: PendingResponses,
    next_request_id: AtomicU64,
    _owner: Option<Arc<ProcessOwner>>,
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
        mut reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        owner: Option<Arc<ProcessOwner>>,
        event_sink: Arc<WatchEventSink>,
    ) -> Arc<Self> {
        let pending = Arc::new(Mutex::new(HashMap::<
            u64,
            mpsc::SyncSender<Result<Frame, String>>,
        >::new()));
        let reader_pending = pending.clone();
        thread::Builder::new()
            .name("necoder-remote-reader".to_string())
            .spawn(move || {
                let failure = loop {
                    match read_frame(reader.as_mut()) {
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
            _owner: owner,
        })
    }

    fn request(&self, request: &Request, body: Vec<u8>) -> Result<(Response, Vec<u8>)> {
        let timeout = request.timeout();
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
                return Err(anyhow!(
                    "remote request timeout/disconnect after {}s: {error}",
                    timeout.as_secs()
                ));
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
struct SshTransport {
    project: SshProject,
    control_dir: PathBuf,
    control_path: PathBuf,
}

impl SshTransport {
    fn connect(project: &SshProject) -> Result<Arc<Self>> {
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
        let transport = Arc::new(Self {
            project: project.clone(),
            control_dir,
            control_path,
        });
        transport.start_master()?;
        Ok(transport)
    }

    fn start_master(&self) -> Result<()> {
        let mut master = ssh_command();
        master
            .args(["-M", "-N", "-f"])
            .arg("-o")
            .arg(format!("ControlPath={}", self.control_path.display()))
            .args([
                "-o",
                // 死んだ/到達不能なホストで GUI が無限にハングしないよう接続打ち切りを入れる
                // （既定は OS の TCP タイムアウト任せ＝分単位。sleep/VPN 断からの復帰性に効く）。
                "ConnectTimeout=10",
                "-o",
                "ControlPersist=600",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ExitOnForwardFailure=yes",
            ]);
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

    fn session_args(&self, tty: bool, remote_command: &str) -> Vec<String> {
        let mut args = vec![
            if tty { "-tt" } else { "-T" }.to_string(),
            "-S".to_string(),
            self.control_path.display().to_string(),
            "-o".to_string(),
            "ControlMaster=no".to_string(),
        ];
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
    let development =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/necoder-remote-server");
    development.is_file().then_some(development)
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
        self.exit_master();
        let _cleanup = std::fs::remove_dir_all(&self.control_dir);
    }
}

struct SshConnector {
    transport: Arc<SshTransport>,
    server_command: String,
    session: String,
}

impl SshConnector {
    fn command(&self) -> Command {
        let remote_command = format!(
            "exec {} proxy --session {}",
            quote_posix(&self.server_command),
            quote_posix(&self.session)
        );
        let mut command = self.transport.command(false, &remote_command);
        command.stderr(Stdio::inherit());
        command
    }

    fn connect(&self, event_sink: Arc<WatchEventSink>) -> Result<Arc<RpcClient>> {
        self.transport.ensure_master()?;
        let client = rpc_client_from_command(self.command(), event_sink)?;
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
        Ok(client)
    }
}

struct ReconnectingClient {
    current: Mutex<Arc<RpcClient>>,
    connector: Option<Arc<SshConnector>>,
    reconnect_lock: Mutex<()>,
    event_sink: Arc<WatchEventSink>,
    generation: AtomicU64,
}

impl ReconnectingClient {
    fn new(
        current: Arc<RpcClient>,
        connector: Option<Arc<SshConnector>>,
        event_sink: Arc<WatchEventSink>,
    ) -> Arc<Self> {
        let client = Arc::new(Self {
            current: Mutex::new(current),
            connector,
            reconnect_lock: Mutex::new(()),
            event_sink,
            generation: AtomicU64::new(0),
        });
        if client.connector.is_some() {
            let weak = Arc::downgrade(&client);
            thread::Builder::new()
                .name("necoder-remote-heartbeat".to_string())
                .spawn(move || loop {
                    thread::sleep(Duration::from_secs(5));
                    let Some(client) = weak.upgrade() else {
                        break;
                    };
                    let _heartbeat = client.request(&Request::Ping, Vec::new());
                })
                .expect("remote heartbeat thread spawn");
        }
        client
    }

    fn event_sink(&self) -> Arc<WatchEventSink> {
        self.event_sink.clone()
    }

    /// 再接続のたびに増える世代番号。watch keeper が「監視を張り直す」判定に使う。
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
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
        let current = self
            .current
            .lock()
            .map_err(|_| anyhow!("remote client lock poisoned"))?
            .clone();
        match current.request(request, body.clone()) {
            Ok(response) => Ok(response),
            // peer が request を処理して返したエラーは接続障害ではない。保存競合や ENOENT で
            // ControlMaster を張り直さず、そのまま呼び出し元へ返す。
            Err(error) if error.downcast_ref::<RemoteRequestError>().is_some() => Err(error),
            Err(original) => {
                let Some(connector) = &self.connector else {
                    return Err(original);
                };
                let _guard = self
                    .reconnect_lock
                    .lock()
                    .map_err(|_| anyhow!("remote reconnect lock poisoned"))?;
                let replacement = {
                    let latest = self
                        .current
                        .lock()
                        .map_err(|_| anyhow!("remote client lock poisoned"))?
                        .clone();
                    if Arc::ptr_eq(&latest, &current) {
                        let replacement = connector
                            .connect(self.event_sink.clone())
                            .context("Remote SSH 再接続に失敗")?;
                        *self
                            .current
                            .lock()
                            .map_err(|_| anyhow!("remote client lock poisoned"))? =
                            replacement.clone();
                        // 世代を進める → watch keeper が新接続で監視を張り直す。
                        self.generation.fetch_add(1, Ordering::Relaxed);
                        replacement
                    } else {
                        latest
                    }
                };
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
}

pub struct RemoteHost {
    id: String,
    display_name: String,
    root: PathBuf,
    project_id: AtomicU64,
    client: Arc<ReconnectingClient>,
    ssh_project: Option<SshProject>,
    transport: Option<Arc<SshTransport>>,
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
        let connector = Arc::new(SshConnector {
            transport: transport.clone(),
            server_command,
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
            ssh_project,
            transport,
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
            ssh_project: self.ssh_project.clone(),
            transport: self.transport.clone(),
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
        self.client.event_sink().set(Some(sender));
        self.send_watch()?;
        let stop = Arc::new(AtomicBool::new(false));
        let keeper_stop = stop.clone();
        let keeper = self.clone();
        thread::Builder::new()
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
                        // 再接続が起きた → 新しい接続で監視を張り直す（失敗しても次周で再試行）。
                        seen = generation;
                        let _resubscribed = keeper.send_watch();
                    }
                }
                // best-effort で監視停止 + シンクを外す。
                let _unwatched = keeper.scoped_request(
                    |project_id| Request::Unwatch { project_id },
                    Vec::new(),
                    true,
                );
                keeper.client.event_sink().set(None);
            })
            .context("remote watch keeper thread spawn")?;
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
        transport.ensure_master()?;
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
            .command(false, &remote_command)
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
        transport.ensure_master()?;
        self.relative(cwd)?;
        let remote_command = format!(
            "cd {} && exec \"${{SHELL:-/bin/sh}}\" -l",
            quote_posix(&cwd.to_string_lossy())
        );
        Ok(Some(TerminalLaunch {
            program: "ssh".to_string(),
            args: transport.session_args(true, &remote_command),
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
        let shell = pick_windows_shell(|candidate| candidate == "pwsh" || candidate == "powershell");
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
        stdout.read_to_string(&mut output).expect("stdout を読めない");

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
}
