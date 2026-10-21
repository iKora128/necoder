//! local/remote 共通の host 境界と Remote SSH wire protocol。
//!
//! `std::fs` / `std::process` を UI・project model から隔離し、同じ API を local process と
//! SSH 上の `necoder-remote-server` へ向ける。設計根拠は `docs/research/remote-ssh-2026.md`、
//! 通り道（ssh 1 本の上にファイル操作・プロセス・端末・TCP を載せる）は `docs/REMOTE-CHANNEL.md`。
//! Zed の GPL コードは取り込まず、公開仕様を基に独立実装する。

mod server_stream;
mod ssh_link;
mod stream;
#[cfg(test)]
mod stream_tests;

use anyhow::{anyhow, bail, Context as _, Result};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use server_stream::{Link, ServerStreams};
use std::collections::HashMap;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io::{BufRead as _, BufReader};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use stream::{ClientStream, StreamHub, StreamKind, StreamReader, StreamWriter};

pub use ssh_link::{
    askpass_attempt, askpass_token_is_live, test_ssh_connection, SshConnectError, SshFailure,
};
pub use stream::{HostPty, ProcessExit, PtyExitWatch, PtySize};

/// v4: ストリーム（プロセス・PTY・TCP）を同じ接続に載せた（docs/REMOTE-CHANNEL.md §2）。
pub const PROTOCOL_VERSION: u16 = 4;
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

const FRAME_MAGIC: [u8; 4] = *b"SHRS";
const FRAME_HEADER_LEN: usize = 28;
const MAX_META_LEN: usize = 8 * 1024 * 1024;
const MAX_BODY_LEN: usize = 256 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SEARCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const COMMAND_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
/// ハンドシェイク（Hello）だけは短く切る。再接続の総待ち時間を有界にするための上限で、
/// 「TCP は生きているが相手が黙っている」（sleep 復帰直後の死んだ接続）を 30s ではなく 10s で見切る。
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

/// LSP/ACP など長寿命 stdio process。Drop 時に子を確実に畳む（remote は接続先の daemon に止めさせる）。
pub struct HostProcess {
    backend: ProcessBackend,
    stdin: Option<Box<dyn Write + Send>>,
    stdout: Option<Box<dyn Read + Send>>,
    stderr: StderrTail,
}

enum ProcessBackend {
    Local(Child),
    /// 接続先の daemon が持つプロセス（RPC のストリーム）。`_connection` は接続（ssh）を生かしておく。
    Remote {
        stream: Arc<ClientStream>,
        _connection: Arc<dyn Send + Sync>,
    },
}

impl HostProcess {
    pub fn take_stdin(&mut self) -> Result<Box<dyn Write + Send>> {
        self.stdin.take().context("process stdin は既に取得済み")
    }

    pub fn take_stdout(&mut self) -> Result<Box<dyn Read + Send>> {
        self.stdout.take().context("process stdout は既に取得済み")
    }

    /// プロセスがまだ生きているか。remote は接続が切れても daemon 側で生きていれば `true`
    /// （繋ぎ直したら続きから流れる）。daemon がプロセスを見失った時だけ `false` になる。
    pub fn is_alive(&mut self) -> bool {
        self.try_exit_status().is_none()
    }

    /// 終わっていればその終わり方（まだ動いていれば `None`）。
    pub fn try_exit_status(&mut self) -> Option<ProcessExit> {
        match &mut self.backend {
            ProcessBackend::Local(child) => child
                .try_wait()
                .ok()
                .flatten()
                .map(ProcessExit::from_status),
            ProcessBackend::Remote { stream, .. } => stream.exit(),
        }
    }

    /// stderr の末尾（[`StderrTail`]）。
    pub fn stderr_tail(&self) -> StderrTail {
        self.stderr.clone()
    }
}

/// 長寿命の子（ACP エージェント・言語サーバ）の stderr の末尾。起動してすぐ落ちた子の理由を見せる
/// ために持つ。**上限つきでメモリにだけ置き、ファイルにもログにも書かない**（秘密が混ざり得るので、
/// 見せるかどうか・どう伏せるかは呼び手が決める）。子は止まらないよう、読む側は最後まで読み続けて
/// 古い行から捨てる。色などの制御文字は落とす。
#[derive(Clone, Default)]
pub struct StderrTail {
    shared: Arc<(Mutex<StderrTailState>, std::sync::Condvar)>,
}

#[derive(Default)]
struct StderrTailState {
    lines: std::collections::VecDeque<String>,
    /// 子が stderr を閉じた（終わった）。
    closed: bool,
}

impl StderrTail {
    /// 持っておく行数（古い行から捨てる）。
    pub const MAX_LINES: usize = 40;
    /// 1 行に持つバイト数（超えた分は捨てて `…` を付ける）。改行の無い長い出力でメモリを食わない。
    pub const MAX_LINE_BYTES: usize = 1000;

    /// 今持っている行（古い順）。
    pub fn lines(&self) -> Vec<String> {
        let (state, _) = &*self.shared;
        state
            .lock()
            .map(|state| state.lines.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 子が stderr を閉じるまで最長 `timeout` 待つ（閉じたら `true`）。すぐ落ちた子の最後の行を、
    /// 読む側が追いつく前に取りこぼさないため。
    pub fn wait_closed(&self, timeout: Duration) -> bool {
        let (state, closed) = &*self.shared;
        let Ok(guard) = state.lock() else {
            return false;
        };
        closed
            .wait_timeout_while(guard, timeout, |state| !state.closed)
            .map(|(state, _)| state.closed)
            .unwrap_or(false)
    }

    fn push(&self, line: String) {
        let (state, _) = &*self.shared;
        if let Ok(mut state) = state.lock() {
            if state.lines.len() == Self::MAX_LINES {
                state.lines.pop_front();
            }
            state.lines.push_back(line);
        }
    }

    fn close(&self) {
        let (state, closed) = &*self.shared;
        if let Ok(mut state) = state.lock() {
            state.closed = true;
        }
        closed.notify_all();
    }

    /// `pipe` を別スレッドで最後まで読み、末尾を持つ。
    fn capture(mut pipe: impl Read + Send + 'static) -> StderrTail {
        let tail = StderrTail::default();
        let writer = tail.clone();
        let spawned = thread::Builder::new()
            .name("stderr-tail".to_string())
            .spawn(move || {
                // 8 KB ずつ読む（行の区切りは自分で見る＝改行の無い長い出力でも 1 行ぶんしか持たない）。
                let mut chunk = [0u8; 8192];
                let mut line: Vec<u8> = Vec::new();
                let mut truncated = false;
                loop {
                    let read = match pipe.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    };
                    for &byte in &chunk[..read] {
                        if byte == b'\n' {
                            writer.push(stderr_line(&line, truncated));
                            line.clear();
                            truncated = false;
                        } else if line.len() < Self::MAX_LINE_BYTES {
                            line.push(byte);
                        } else {
                            truncated = true;
                        }
                    }
                }
                if !line.is_empty() {
                    writer.push(stderr_line(&line, truncated));
                }
                writer.close();
            });
        if let Err(error) = spawned {
            eprintln!("stderr を読むスレッドを立てられない: {error}");
            tail.close();
        }
        tail
    }
}

/// stderr の 1 行を見せられる文字にする（UTF-8 に直し、色などの制御文字・エスケープ列を落とす）。
fn stderr_line(bytes: &[u8], truncated: bool) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut line = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            // ESC [ … 終端（CSI・色）/ ESC ] … BEL か ESC \（OSC・題名）/ ESC と次の 1 文字。
            '\u{1b}' => match characters.next() {
                Some('[') => {
                    for next in characters.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next) = characters.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            characters.next_if_eq(&'\\');
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => line.push(' '),
            character if character.is_control() => {}
            character => line.push(character),
        }
    }
    if truncated {
        line.push('…');
    }
    line
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        match &mut self.backend {
            ProcessBackend::Local(child) => {
                let _kill = child.kill();
                let _wait = child.wait();
            }
            ProcessBackend::Remote { stream, .. } => stream.close(),
        }
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

/// 端末の起こし方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalLaunch {
    /// 手元の PTY で起こすプログラム（Windows の既定シェルの指定など）。
    Program(TerminalProgram),
    /// 接続先の daemon が PTY を持つ端末（入出力は RPC のストリームで中継する）。
    Remote(RemoteTerminal),
}

/// 手元の PTY で起こすプログラムと引数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalProgram {
    pub program: String,
    pub args: Vec<String>,
}

/// 接続先で開く端末。[`Self::open`] は接続を待つので **UI スレッドから呼ばない**
/// （`TerminalView` は背景のスレッドで開き、開くまでの打鍵は溜めておく）。
#[derive(Clone)]
pub struct RemoteTerminal {
    opener: Arc<dyn Fn(PtySize) -> Result<HostPty> + Send + Sync>,
    /// 表示用（`user@host`）。
    label: String,
    /// 等しさの鍵（接続先 + 開く場所）。同じ場所を指す launch は作り直しても等しい
    /// （ドックが「launch が変わった」と誤って端末を作り直さない）。
    key: String,
}

impl RemoteTerminal {
    pub fn open(&self, size: PtySize) -> Result<HostPty> {
        assert_off_main_thread("OpenTerminal");
        (self.opener)(size)
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// テスト用: 任意の開き方を持つ端末。
    #[doc(hidden)]
    pub fn from_opener(
        label: impl Into<String>,
        opener: impl Fn(PtySize) -> Result<HostPty> + Send + Sync + 'static,
    ) -> Self {
        let label = label.into();
        Self {
            opener: Arc::new(opener),
            key: label.clone(),
            label,
        }
    }
}

impl std::fmt::Debug for RemoteTerminal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteTerminal")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// 同じ接続先の同じ場所なら等しい。
impl PartialEq for RemoteTerminal {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for RemoteTerminal {}

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

/// 上限つきで取り込んだ出力 1 本（[`Host::run_user_command`]）。頭と尻を残して間を捨てる
/// （`cat` の頭も `cargo build` の尻も落とさない・transcript のツール結果と同じ考え方）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapturedOutput {
    /// 先頭から（上限の半分まで）。間を捨てていなければ全部ここにある。
    pub head: Vec<u8>,
    /// 末尾（上限の残り半分まで）。間を捨てていなければ空。
    pub tail: Vec<u8>,
    /// `head` と `tail` の間で捨てたバイト数（0 = 全部持っている）。
    pub omitted: u64,
}

impl CapturedOutput {
    /// 手元に全部ある出力（remote の結果）を同じ上限で丸める。
    fn from_bytes(bytes: &[u8], limit: usize) -> Self {
        let mut buffer = CaptureBuffer::new(limit);
        buffer.push(bytes);
        buffer.take()
    }
}

/// [`Host::run_user_command`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserCommandOutput {
    /// 終了コード。シグナルで終わった・止めた・走らせなかった時は `None`。
    pub status_code: Option<i32>,
    pub stdout: CapturedOutput,
    pub stderr: CapturedOutput,
    /// 取り消しで止めた（出力は止めるまでの分）。始める前に取り消されていれば何も走らせていない。
    pub cancelled: bool,
}

impl UserCommandOutput {
    /// 始める前に取り消された（起動もしていない）。
    fn not_started() -> Self {
        Self {
            status_code: None,
            stdout: CapturedOutput::default(),
            stderr: CapturedOutput::default(),
            cancelled: true,
        }
    }
}

/// 走っている間の出力の末尾（エージェントパネルの `!` の行に流して見せる・#37）。
///
/// 取り込み（[`CapturedOutput`]・終わってから返す）とは別に、**表示のためだけに**最後の
/// [`LIVE_OUTPUT_BYTES`] を持つ。stdout と stderr は届いた順に 1 本へ並べる（端末と同じ）。
/// 読み手のスレッドと UI で共有するので、clone は同じ中身を指す。
#[derive(Clone, Default)]
pub struct LiveOutput(Arc<Mutex<LiveTail>>);

#[derive(Default)]
struct LiveTail {
    bytes: std::collections::VecDeque<u8>,
    received: u64,
}

/// [`LiveOutput`] が持つ末尾の大きさ（画面に出すのは数行なので十分）。
const LIVE_OUTPUT_BYTES: usize = 16 * 1024;

impl LiveOutput {
    fn push(&self, chunk: &[u8]) {
        let mut tail = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tail.received += chunk.len() as u64;
        tail.bytes.extend(chunk);
        let excess = tail.bytes.len().saturating_sub(LIVE_OUTPUT_BYTES);
        tail.bytes.drain(..excess);
    }

    /// これまでに届いたバイト数（増えた時だけ描き直すため）。
    pub fn received(&self) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .received
    }

    /// 届いた出力の末尾（最大 [`LIVE_OUTPUT_BYTES`]・先頭は文字の途中から始まりうる）。
    pub fn tail(&self) -> Vec<u8> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bytes
            .iter()
            .copied()
            .collect()
    }
}

/// 頭と尻だけを持つ取り込み口（[`CapturedOutput`] を作る）。読み手のスレッドと取り出す側で共有する。
struct CaptureBuffer {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    head_limit: usize,
    tail_limit: usize,
    omitted: u64,
    /// 取り出した後（[`Self::take`]）。まだ読み手が生きていても（`&` の孫がパイプを握っている）以後は捨てる。
    taken: bool,
}

impl CaptureBuffer {
    fn new(limit: usize) -> Self {
        let head_limit = limit / 2;
        Self {
            head: Vec::new(),
            tail: std::collections::VecDeque::new(),
            head_limit,
            tail_limit: limit - head_limit,
            omitted: 0,
            taken: false,
        }
    }

    fn push(&mut self, mut bytes: &[u8]) {
        if self.taken {
            return;
        }
        let room = self.head_limit.saturating_sub(self.head.len());
        if room > 0 {
            let taken = room.min(bytes.len());
            self.head.extend_from_slice(&bytes[..taken]);
            bytes = &bytes[taken..];
        }
        if bytes.is_empty() {
            return;
        }
        if bytes.len() >= self.tail_limit {
            // 1 回で尻の上限を超える: 今までの尻は全部押し出される。
            self.omitted += (self.tail.len() + bytes.len() - self.tail_limit) as u64;
            self.tail.clear();
            self.tail.extend(&bytes[bytes.len() - self.tail_limit..]);
            return;
        }
        let overflow = (self.tail.len() + bytes.len()).saturating_sub(self.tail_limit);
        self.tail.drain(..overflow);
        self.omitted += overflow as u64;
        self.tail.extend(bytes);
    }

    fn take(&mut self) -> CapturedOutput {
        self.taken = true;
        let mut head = std::mem::take(&mut self.head);
        let mut tail: Vec<u8> = std::mem::take(&mut self.tail).into();
        let omitted = std::mem::take(&mut self.omitted);
        if omitted == 0 {
            // 間を捨てていない＝頭と尻は地続き。1 本にして返す。
            head.append(&mut tail);
        }
        CapturedOutput {
            head,
            tail,
            omitted,
        }
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
    /// **冪等（読み取り専用）と分かっている** command。remote では「送ったが結果不明」で
    /// 接続が張り直された場合に**再送してよい**。`command -v` や `uname` のような探索に使う。
    /// 既定は [`Self::run_command`] と同じ（local に再送の概念は無い）。
    fn run_command_retry_safe(&self, spec: &CommandSpec) -> Result<CommandOutput> {
        self.run_command(spec)
    }
    /// `cancel` が立ったら結果を要らないものとして `Ok(None)` を返す command（変更レビューの
    /// `git diff` のように、画面を閉じた・基準を変えたら用が無くなるもの）。**途中で止めても何も
    /// 壊れない読み取り専用の command だけに使う**（git なら `--no-optional-locks` を付ける）。
    /// local は子の出力を待つ間に `cancel` を見て、立ったら子を止める（残りを計算させない）。
    /// 既定（remote）は daemon との 1 往復の途中で止める口が無いので、最後まで走らせてから見る。
    fn run_command_cancellable(
        &self,
        spec: &CommandSpec,
        cancel: &AtomicBool,
    ) -> Result<Option<CommandOutput>> {
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        let output = self.run_command(spec)?;
        Ok((!cancel.load(Ordering::Acquire)).then_some(output))
    }
    /// 人が打ったシェルコマンドを流す（エージェントパネルの `!`・#37）。
    /// [`Self::run_command_cancellable`] と違い、**副作用のある command に使う**前提で、止めるのは
    /// 人の操作（esc / ■）だけ。stdin は空（対話しない）。
    ///
    /// - 出力は stream ごとに頭と尻を合わせて `output_limit` バイトまで持ち、間は読み捨てる
    ///   （子は止めない）。巨大な `cat` や止まらない出力でメモリを食わない
    /// - `cancel` が立ったら止め、そこまでの出力を `cancelled` 付きで返す
    ///
    /// - 走っている間の出力は `live` へ流す（表示用の末尾。取り込みの上限とは別）
    ///
    /// 既定は `run_command` の 1 往復で、途中で止める口も途中の出力も無い（取り消されても最後まで
    /// 走らせて結果を返す・[`Self::can_stop_user_command`] が `false`＝呼ぶ側は待たずに畳む）。
    /// 手元（[`LocalHost`]）と SSH 先（[`RemoteHost`]）はどちらも止められる実装を持つ。
    fn run_user_command(
        &self,
        spec: &CommandSpec,
        cancel: &AtomicBool,
        output_limit: usize,
        live: &LiveOutput,
    ) -> Result<UserCommandOutput> {
        if cancel.load(Ordering::Acquire) {
            return Ok(UserCommandOutput::not_started());
        }
        let output = self.run_command(spec)?;
        live.push(&output.stdout);
        live.push(&output.stderr);
        Ok(UserCommandOutput {
            status_code: output.status_code,
            stdout: CapturedOutput::from_bytes(&output.stdout, output_limit),
            stderr: CapturedOutput::from_bytes(&output.stderr, output_limit),
            cancelled: false,
        })
    }
    /// [`Self::run_user_command`] を走っている途中で止められるか（手元だけ `true`）。
    fn can_stop_user_command(&self) -> bool {
        false
    }
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
    /// 接続先の TCP ポート `remote_port` を手元の 127.0.0.1 へ転送する（O5・手元で待ち受け、来た接続を
    /// RPC の TCP ストリームで中継する）。手元の番号は、同じ番号が空いていればそれ、塞がっていれば空いている番号。
    /// 返すのは手元の番号（すでに転送していればその番号）。再接続しても待ち受けはそのまま。
    /// 手元のプロジェクトは転送するものが無いので `Err`。
    fn forward_port(&self, _remote_port: u16) -> Result<u16> {
        bail!("手元のプロジェクトのポートは転送しない")
    }
    /// [`Host::forward_port`] の転送をやめる（`-O cancel`）。**UI スレッドから呼ばない**。
    fn cancel_forward(&self, remote_port: u16) -> Result<()> {
        bail!("{remote_port} は転送していない")
    }
    /// いま張っている転送（接続先の番号, 手元の番号）。ネットワークに触らない。
    fn forwarded_ports(&self) -> Vec<(u16, u16)> {
        Vec::new()
    }
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

    fn run_command_cancellable(
        &self,
        spec: &CommandSpec,
        cancel: &AtomicBool,
    ) -> Result<Option<CommandOutput>> {
        run_command_cancellable_local(spec, cancel)
    }

    fn run_user_command(
        &self,
        spec: &CommandSpec,
        cancel: &AtomicBool,
        output_limit: usize,
        live: &LiveOutput,
    ) -> Result<UserCommandOutput> {
        run_user_command_local(spec, cancel, output_limit, live)
    }

    fn can_stop_user_command(&self) -> bool {
        true
    }

    fn spawn_process(&self, spec: &CommandSpec) -> Result<HostProcess> {
        spawn_process_local(spec)
    }

    fn terminal_launch(&self, _cwd: &Path) -> Result<Option<TerminalLaunch>> {
        if cfg!(windows) {
            // Windows は alacritty の既定が `powershell` 固定なので、pwsh を優先させる（§W4）。
            return Ok(Some(TerminalLaunch::Program(pick_windows_shell(
                is_in_path,
            ))));
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
fn pick_windows_shell(is_available: impl Fn(&str) -> bool) -> TerminalProgram {
    let program = ["pwsh", "powershell"]
        .into_iter()
        .find(|candidate| is_available(candidate))
        // どちらも PATH に無いのは考えにくいが、その時は alacritty と同じ既定へ倒す
        .unwrap_or("powershell");
    TerminalProgram {
        program: program.to_string(),
        args: Vec::new(),
    }
}

/// 流すコマンドの前後に端末へ出す一言（設定の「インストール」「ログイン」・2026-10-19）。
///
/// 以前は端末がいきなり開いてコマンドの出力だけが流れ、何を実行したのか・終わったのか・失敗したのかが
/// 画面に無かった（初めての人が「開いたけど何も起きない？」と迷った・#69）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandNotes {
    /// 流す前（例「▶ 次のコマンドを実行します: …」）。
    pub start: String,
    /// 終了コード 0 の後。
    pub success: String,
    /// 失敗の後。
    pub failure: String,
}

/// `command` を流したあと、そのまま対話シェルに残る端末の起動（プログラム, 引数）。
/// 設定の「インストール」「ログイン」「ターミナルで実行」が使う。`notes` があれば前後に一言を出す。
///
/// Windows は `/bin/sh` が無いので PowerShell の `-NoExit`。`-ExecutionPolicy Bypass` は
/// 既定の Restricted だと `npm i -g` が `npm.ps1` を弾かれて失敗するため（このプロセスだけに効く）。
pub fn command_then_shell(command: &str, notes: Option<&CommandNotes>) -> (String, Vec<String>) {
    if cfg!(windows) {
        let shell = pick_windows_shell(is_in_path);
        let script = match notes {
            Some(notes) => powershell_script_with_notes(command, notes),
            None => command.to_string(),
        };
        return (shell.program, windows_command_then_shell_args(&script));
    }
    let script = match notes {
        Some(notes) => sh_script_with_notes(command, notes),
        None => command.to_string(),
    };
    (
        "/bin/sh".to_string(),
        vec![
            "-lc".to_string(),
            format!("{script}; exec \"${{SHELL:-/bin/zsh}}\" -l"),
        ],
    )
}

fn windows_command_then_shell_args(command: &str) -> Vec<String> {
    ["-NoLogo", "-NoExit", "-ExecutionPolicy", "Bypass", "-Command", command]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// PowerShell の単引用符の文字列（中の `'` は `''` に重ねる）。
fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// sh の単引用符の文字列（中の `'` は閉じて逃がしてから開き直す）。
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// 前に一言 → コマンド → 成否で一言（PowerShell）。`$?` はコマンドの直後に読む。
fn powershell_script_with_notes(command: &str, notes: &CommandNotes) -> String {
    format!(
        "Write-Host {start}; Write-Host ''; {command}; $necoderOk = $?; Write-Host ''; \
         if ($necoderOk) {{ Write-Host {success} }} else {{ Write-Host {failure} }}",
        start = powershell_quote(&notes.start),
        success = powershell_quote(&notes.success),
        failure = powershell_quote(&notes.failure),
    )
}

/// 前に一言 → コマンド → 成否で一言（sh）。
fn sh_script_with_notes(command: &str, notes: &CommandNotes) -> String {
    format!(
        "printf '%s\\n\\n' {start}; {command}; necoder_status=$?; printf '\\n'; \
         if [ \"$necoder_status\" -eq 0 ]; then printf '%s\\n' {success}; \
         else printf '%s\\n' {failure}; fi",
        start = sh_quote(&notes.start),
        success = sh_quote(&notes.success),
        failure = sh_quote(&notes.failure),
    )
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
    [".exe", ".cmd", ".bat"]
        .iter()
        .map(|extension| format!("{binary}{extension}"))
        .collect()
}

/// PATH から実行ファイルを探す（Windows は `PATHEXT` 相当の拡張子も試す）。
pub fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    find_in_search_path(binary, &path)
}

fn find_in_search_path(binary: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    let names = executable_names(binary);
    std::env::split_paths(path).find_map(|directory| {
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

/// **Windows**: レジストリのシステム / ユーザー PATH のうち、このプロセスの PATH に無いディレクトリを
/// 末尾へ足す。足したら新しい PATH を返す（変化なし・Windows 以外は `None`）。
///
/// Windows のプロセスは環境を親（たいていエクスプローラ）から継ぐので、necoder を起動した後や
/// エクスプローラが変更を拾い損ねた後に入れたツール（`winget install OpenJS.NodeJS.LTS` の node・
/// ネイティブ版 claude の `%USERPROFILE%\.local\bin`）が**再起動するまで見えない**（issue #69）。
/// 既存の並びには触らず足すだけなので、ターミナルから起動した開発時の PATH 優先順位も変えない。
///
/// 起動時と、エージェントの準備状況を読み直すたびに呼ぶ。
pub fn refresh_search_path() -> Option<String> {
    #[cfg(windows)]
    {
        let current = std::env::var("PATH").unwrap_or_default();
        let system = windows_registry_search_path();
        let merged = append_missing_search_dirs(&current, &system)?;
        // SAFETY: Windows の std は環境の読み書きを内部ロックで直列化する。C 側の getenv と
        // 競合するのは unix の事情で、ここ（cfg(windows)）には当たらない。
        unsafe { std::env::set_var("PATH", &merged) };
        Some(merged)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// necoder が自分で入れた道具（Node.js・エージェントの CLI）を探す場所（在っても無くても返す・使う順）。
/// 置き場は [`paths::toolchain_dir`]。Node.js の公式の配布物は Windows では直下に `node.exe`、
/// mac / Linux では `bin/node`。npm の `--prefix` も Windows は直下・それ以外は `bin/` に実行ファイルを置く。
pub fn toolchain_search_dirs() -> Vec<PathBuf> {
    paths::toolchain_dir()
        .map(|root| toolchain_search_dirs_in(&root, cfg!(windows)))
        .unwrap_or_default()
}

/// [`toolchain_search_dirs`] の本体（置き場と OS を渡せる＝どのプラットフォームでもテストできる）。
pub fn toolchain_search_dirs_in(root: &Path, windows: bool) -> Vec<PathBuf> {
    if windows {
        vec![root.join("node"), root.join("npm")]
    } else {
        vec![root.join("node").join("bin"), root.join("npm").join("bin")]
    }
}

/// 起動時に 1 回だけ（**窓が開く前＝実質シングルスレッド**の main の冒頭）: necoder の道具箱を PATH の
/// **末尾**へ足す。まだ何も入れていなくても足しておく — 後で「インストール」で入れた物が、
/// 環境を書き換え直さずにそのまま見つかる（mac / Linux で動いている途中に `set_var` しないため）。
/// 末尾なので、パソコンに入っている node や CLI が先に使われる。ターミナル・エージェント・LSP など
/// necoder が起こす子プロセスもこの PATH を継ぐ。
pub fn append_toolchain_to_path() {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut directories: Vec<PathBuf> = std::env::split_paths(&current).collect();
    let before = directories.len();
    for directory in toolchain_search_dirs() {
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    if directories.len() == before {
        return;
    }
    match std::env::join_paths(directories) {
        Ok(joined) => std::env::set_var("PATH", joined),
        Err(error) => eprintln!("necoder の道具箱を PATH に足せない: {error}"),
    }
}

/// `current`（`;` 区切り）に無いディレクトリを `system` から順に末尾へ足す。足す物が無ければ `None`。
/// 比較は Windows の流儀（大文字小文字・末尾の `\` を区別しない）。
#[cfg_attr(not(windows), allow(dead_code))]
fn append_missing_search_dirs(current: &str, system: &str) -> Option<String> {
    fn normalized(directory: &str) -> String {
        directory
            .trim()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    }
    let mut known: std::collections::HashSet<String> = current
        .split(';')
        .map(normalized)
        .filter(|directory| !directory.is_empty())
        .collect();
    let missing: Vec<&str> = system
        .split(';')
        .map(str::trim)
        .filter(|directory| !directory.is_empty() && known.insert(normalized(directory)))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let mut merged = current.trim_end_matches(';').to_string();
    for directory in missing {
        if !merged.is_empty() {
            merged.push(';');
        }
        merged.push_str(directory);
    }
    Some(merged)
}

/// システム → ユーザーの順に連結した PATH（`%SystemRoot%` 等は展開済み）。
#[cfg(windows)]
fn windows_registry_search_path() -> String {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    [
        (
            HKEY_LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        ),
        (HKEY_CURRENT_USER, "Environment"),
    ]
    .into_iter()
    .filter_map(|(root, key)| windows_registry_string(root, key, "Path"))
    .collect::<Vec<_>>()
    .join(";")
}

/// REG_SZ / REG_EXPAND_SZ の値を読む（REG_EXPAND_SZ は `RegGetValueW` が展開して返す）。
#[cfg(windows)]
fn windows_registry_string(
    root: windows_sys::Win32::System::Registry::HKEY,
    key: &str,
    value: &str,
) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};
    const ERROR_SUCCESS: u32 = 0;
    const ERROR_MORE_DATA: u32 = 234;
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() };
    let (key, value) = (wide(key), wide(value));
    let mut size: u32 = 0;
    // 展開後の長さは 1 回目の問い合わせで見積もりしか返らないことがあるので、足りなければ測り直す。
    for _ in 0..4 {
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        let data = if buffer.is_empty() {
            std::ptr::null_mut()
        } else {
            buffer.as_mut_ptr().cast()
        };
        // SAFETY: key / value は NUL 終端の UTF-16。data は size バイト以上の領域か null（長さだけ問う）。
        let status = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                data,
                &mut size,
            )
        };
        match status {
            ERROR_SUCCESS if !buffer.is_empty() => {
                let length = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
                return Some(String::from_utf16_lossy(&buffer[..length]));
            }
            ERROR_SUCCESS | ERROR_MORE_DATA => continue,
            _ => return None,
        }
    }
    None
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
    let output = child_process::command(&spec.program)
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

/// 取り消しの印を見る間隔（[`run_command_cancellable_local`]）。出力が届けばすぐ起きるので、
/// 普段の command はこの分だけ遅れることはない（黙って計算している間の見回りの間隔）。
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// [`Host::run_command_cancellable`] の local 実装。stdout と stderr は別々のスレッドで読み切り
/// （片方のパイプが埋まると子が止まる）、待つ間に `cancel` が立ったら子を止めて回収し `None` を返す。
/// 読み手のスレッドは待たない（子が止まってパイプが閉じれば自然に終わる）。
fn run_command_cancellable_local(
    spec: &CommandSpec,
    cancel: &AtomicBool,
) -> Result<Option<CommandOutput>> {
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let mut child = child_process::command(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("process を起動できない: {}", spec.program))?;
    let pipes = child
        .stdout
        .take()
        .zip(child.stderr.take())
        .context("process の stdout / stderr が無い");
    let (sender, receiver) = mpsc::channel();
    let started = pipes.and_then(|(stdout, stderr)| {
        read_pipe_in_background(stdout, 0, sender.clone())?;
        read_pipe_in_background(stderr, 1, sender)
    });
    if let Err(error) = started {
        stop_child(&mut child);
        return Err(error);
    }
    // [stdout, stderr]。両方が読み終わる（= 子が書き終えた）まで待つ。
    let mut outputs: [Option<Vec<u8>>; 2] = [None, None];
    while outputs.iter().any(Option::is_none) {
        match receiver.recv_timeout(CANCEL_POLL_INTERVAL) {
            Ok((slot, Ok(bytes))) => outputs[slot] = Some(bytes),
            Ok((_, Err(error))) => {
                stop_child(&mut child);
                return Err(error).context("process の出力を読めない");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancel.load(Ordering::Acquire) {
                    stop_child(&mut child);
                    return Ok(None);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop_child(&mut child);
                bail!("process の出力を読むスレッドが落ちた");
            }
        }
    }
    let status = child.wait().context("process の終了を待てない")?;
    let [Some(stdout), Some(stderr)] = outputs else {
        bail!("process の出力が揃わない");
    };
    Ok(Some(CommandOutput {
        status_code: status.code(),
        stdout,
        stderr,
    }))
}

/// パイプを最後まで読み、`slot`（0 = stdout / 1 = stderr）と一緒に `sender` へ渡すスレッドを立てる。
fn read_pipe_in_background(
    mut pipe: impl Read + Send + 'static,
    slot: usize,
    sender: mpsc::Sender<(usize, std::io::Result<Vec<u8>>)>,
) -> Result<()> {
    thread::Builder::new()
        .name("necoder-command-output".to_string())
        .spawn(move || {
            let mut bytes = Vec::new();
            let read = pipe.read_to_end(&mut bytes).map(|_| bytes);
            // 受け手が先に抜けていたら（取り消した）読んだ分は要らない。
            let _unwanted = sender.send((slot, read));
        })
        .context("process の出力を読むスレッドを起動できない")?;
    Ok(())
}

/// 取り消した子を止めて回収する（ゾンビを残さない）。すでに終わっていれば kill は失敗するが、それで良い。
fn stop_child(child: &mut Child) {
    let _already_exited = child.kill();
    if let Err(error) = child.wait() {
        eprintln!("止めた process を回収できない: {error}");
    }
}

/// 人が打ったコマンドを止める時、まず行儀よく終わらせる（SIGTERM）猶予。git などは受け取ると
/// lock を片付けてから終わる。過ぎても子が生きていればグループごと SIGKILL。
const USER_COMMAND_STOP_GRACE: Duration = Duration::from_secs(2);

/// 子が終わった後、出力のパイプが閉じるのを待つ上限。`&` で背景に回した孫がパイプを握ったままだと
/// いつまでも閉じないので、ここで打ち切って結果を返す（孫は止めない＝端末で `&` を付けた時と同じ）。
const USER_COMMAND_PIPE_GRACE: Duration = Duration::from_millis(300);

/// [`Host::run_user_command`] の local 実装。
///
/// 子は**自分のプロセスグループ**で起こす（unix）。止める時にグループごとシグナルを送れば、
/// パイプの後段や `cargo` が起こした `rustc` まで止まる（子だけ止めると孫が残り、出力のパイプも
/// 閉じない）。stdout と stderr は別々のスレッドで上限つきで読み（片方が埋まると子が止まる）、
/// 待つ間は [`CANCEL_POLL_INTERVAL`] ごとに子の終わりと取り消しの印を見る。
fn run_user_command_local(
    spec: &CommandSpec,
    cancel: &AtomicBool,
    output_limit: usize,
    live: &LiveOutput,
) -> Result<UserCommandOutput> {
    if cancel.load(Ordering::Acquire) {
        return Ok(UserCommandOutput::not_started());
    }
    let mut command = child_process::command(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
        // SAFETY: fork と exec の間の子で走る。中で呼ぶのは async-signal-safe な関数だけ
        // （sigemptyset・sigprocmask・signal）で、メモリも確保しない。
        unsafe {
            command.pre_exec(reset_child_signals);
        }
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("process を起動できない: {}", spec.program))?;
    let buffers = [
        Arc::new(Mutex::new(CaptureBuffer::new(output_limit))),
        Arc::new(Mutex::new(CaptureBuffer::new(output_limit))),
    ];
    let (sender, receiver) = mpsc::channel::<()>();
    let pipes = child
        .stdout
        .take()
        .zip(child.stderr.take())
        .context("process の stdout / stderr が無い");
    let started = pipes.and_then(|(stdout, stderr)| {
        capture_pipe_in_background(stdout, buffers[0].clone(), live.clone(), sender.clone())?;
        capture_pipe_in_background(stderr, buffers[1].clone(), live.clone(), sender)
    });
    if let Err(error) = started {
        stop_user_command(&mut child);
        return Err(error);
    }
    let mut open_pipes = 2usize;
    let mut status: Option<std::process::ExitStatus> = None;
    let mut exited_at: Option<std::time::Instant> = None;
    let mut stop_started_at: Option<std::time::Instant> = None;
    let mut killed = false;
    loop {
        match receiver.recv_timeout(CANCEL_POLL_INTERVAL) {
            Ok(()) => open_pipes = open_pipes.saturating_sub(1),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => open_pipes = 0,
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    status = Some(exit);
                    exited_at = Some(std::time::Instant::now());
                }
                Ok(None) => {}
                Err(error) => {
                    stop_user_command(&mut child);
                    return Err(error).context("process の終了を確かめられない");
                }
            }
        }
        // シグナルは子をまだ回収していない間だけ送る（回収した後はグループの id が再利用されうる）。
        if status.is_none() && cancel.load(Ordering::Acquire) {
            match stop_started_at {
                None => {
                    signal_process_group(&mut child, StopSignal::Terminate);
                    stop_started_at = Some(std::time::Instant::now());
                }
                Some(started) if !killed && started.elapsed() >= USER_COMMAND_STOP_GRACE => {
                    signal_process_group(&mut child, StopSignal::Kill);
                    killed = true;
                }
                Some(_) => {}
            }
        }
        if let Some(exited_at) = exited_at {
            if open_pipes == 0 || exited_at.elapsed() >= USER_COMMAND_PIPE_GRACE {
                break;
            }
        }
    }
    let take = |buffer: &Arc<Mutex<CaptureBuffer>>| {
        buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    };
    Ok(UserCommandOutput {
        status_code: status.and_then(|status| status.code()),
        stdout: take(&buffers[0]),
        stderr: take(&buffers[1]),
        cancelled: stop_started_at.is_some(),
    })
}

/// [`run_user_command_over_process`] が出力の 1 行目に載せる「このシェルの PID」の目印（表示からは外す）。
const USER_COMMAND_PID_MARKER: &str = "\u{1e}NECODER_USER_COMMAND_PID=";

/// 目印の行はこれより長くならない（届かなければ目印ではない＝そのまま出力として扱う）。
const USER_COMMAND_PID_LINE_MAX: usize = 64;

/// 人のコマンドを **`spawn_process` の子として**走らせる（SSH 先の [`Host::run_user_command`]）。
///
/// daemon との 1 往復（`run_command`）だと途中で止める口が無く、出力も終わるまで届かない。
/// `ssh` の子として直に走らせれば、出力は届いたそばから `live` へ流せる。止める時は、先頭で出させた
/// シェルの PID から**同じホストで**子・孫まで TERM を送り（`pgrep -P` を辿る）、
/// [`USER_COMMAND_STOP_GRACE`] を過ぎても残っていれば KILL する（手元のプロセスグループと同じ手順）。
/// `ssh` の子の stderr は捨てられるので、シェル側で stdout へ寄せる（stderr は空で返す）。
fn run_user_command_over_process(
    host: &dyn Host,
    spec: &CommandSpec,
    cancel: &AtomicBool,
    output_limit: usize,
    live: &LiveOutput,
) -> Result<UserCommandOutput> {
    if cancel.load(Ordering::Acquire) {
        return Ok(UserCommandOutput::not_started());
    }
    let mut words = vec![quote_posix(&spec.program)];
    words.extend(spec.args.iter().map(|arg| quote_posix(arg)));
    // PID を出してから本来のコマンドへ exec で置き換わる（PID は変わらない＝そこから辿れる）。
    let script = format!(
        "printf '{USER_COMMAND_PID_MARKER}%s\\n' \"$$\"\nexec 2>&1\nexec {}",
        words.join(" ")
    );
    let mut wrapped = posix_shell_script(&script, &spec.cwd);
    wrapped.env = spec.env.clone();
    let mut process = host.spawn_process(&wrapped)?;
    drop(process.take_stdin()?); // stdin は空（対話しない）
    let stdout = process.take_stdout()?;
    let buffer = Arc::new(Mutex::new(CaptureBuffer::new(output_limit)));
    let pid = Arc::new(Mutex::new(None::<u32>));
    let (sender, receiver) = mpsc::channel::<()>();
    capture_marked_pipe_in_background(stdout, buffer.clone(), live.clone(), pid.clone(), sender)?;

    let mut pipe_open = true;
    let mut status: Option<ProcessExit> = None;
    let mut exited_at: Option<std::time::Instant> = None;
    let mut stop_started_at: Option<std::time::Instant> = None;
    let mut killed = false;
    loop {
        match receiver.recv_timeout(CANCEL_POLL_INTERVAL) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => pipe_open = false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if status.is_none() {
            if let Some(exit) = process.try_exit_status() {
                status = Some(exit);
                exited_at = Some(std::time::Instant::now());
            }
        }
        if status.is_none() && cancel.load(Ordering::Acquire) {
            let marked = *pid
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match (stop_started_at, marked) {
                // PID がまだ届いていない（起動の直後）＝届くのを待ってから送る。
                (None, None) => {}
                (None, Some(pid)) => {
                    signal_remote_tree(host, &spec.cwd, pid, "TERM");
                    stop_started_at = Some(std::time::Instant::now());
                }
                (Some(started), Some(pid))
                    if !killed && started.elapsed() >= USER_COMMAND_STOP_GRACE =>
                {
                    signal_remote_tree(host, &spec.cwd, pid, "KILL");
                    killed = true;
                }
                // KILL しても終わりが届かない（接続が詰まった）＝見切って手元の handle を畳む。
                (Some(started), _) if started.elapsed() >= USER_COMMAND_STOP_GRACE * 2 => break,
                _ => {}
            }
        }
        if let Some(exited_at) = exited_at {
            if !pipe_open || exited_at.elapsed() >= USER_COMMAND_PIPE_GRACE {
                break;
            }
        }
    }
    drop(process); // 終わっていれば片付けるだけ・見切った時は daemon に止めさせる
    let captured = buffer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    Ok(UserCommandOutput {
        status_code: status.and_then(|status| status.code()),
        stdout: captured,
        stderr: CapturedOutput::default(),
        cancelled: stop_started_at.is_some(),
    })
}

/// `pid` とその子・孫へシグナルを送るスクリプト（POSIX）。親を先に STOP して新しい子を産ませず、
/// 子から順に送ってから CONT で起こす（STOP 中は TERM を受け取っても片付けに進めない）。
/// `pgrep` が無い環境では `pid` 本体にだけ届く。
fn signal_tree_script(pid: u32, signal: &str) -> String {
    format!(
        "signal_tree() {{ kill -STOP \"$1\" 2>/dev/null; for child in $(pgrep -P \"$1\" 2>/dev/null); do signal_tree \"$child\"; done; kill -{signal} \"$1\" 2>/dev/null; kill -CONT \"$1\" 2>/dev/null; }}\nsignal_tree {pid}"
    )
}

fn signal_remote_tree(host: &dyn Host, cwd: &Path, pid: u32, signal: &str) {
    let spec = posix_shell_script(&signal_tree_script(pid, signal), cwd);
    if let Err(error) = host.run_command(&spec) {
        eprintln!("コマンドを止められない（{signal}）: {error:#}");
    }
}

/// [`capture_pipe_in_background`] の、先頭に PID の目印行が来るパイプ版。目印は `pid` へ控えて
/// 出力からは外す（目印でなければそのまま出力として扱う）。
fn capture_marked_pipe_in_background(
    mut pipe: impl Read + Send + 'static,
    buffer: Arc<Mutex<CaptureBuffer>>,
    live: LiveOutput,
    pid: Arc<Mutex<Option<u32>>>,
    done: mpsc::Sender<()>,
) -> Result<()> {
    thread::Builder::new()
        .name("necoder-command-output".to_string())
        .spawn(move || {
            let mut chunk = [0u8; 16 * 1024];
            // 目印の行を読み切るまでは手元に溜める（`None` = もう目印を探さない）。
            let mut head: Option<Vec<u8>> = Some(Vec::new());
            let push = |bytes: &[u8]| {
                buffer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(bytes);
                live.push(bytes);
            };
            loop {
                let read = match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        eprintln!("コマンドの出力を読めない: {error}");
                        break;
                    }
                };
                let Some(pending) = head.as_mut() else {
                    push(&chunk[..read]);
                    continue;
                };
                pending.extend_from_slice(&chunk[..read]);
                match split_pid_marker(pending) {
                    MarkerScan::Found(found, rest) => {
                        *pid.lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(found);
                        push(&rest);
                        head = None;
                    }
                    MarkerScan::NotMarker => {
                        push(pending);
                        head = None;
                    }
                    MarkerScan::Incomplete => {}
                }
            }
            if let Some(pending) = head {
                push(&pending); // 目印の行が完結しないまま終わった＝ただの出力
            }
            // 受け手が先に抜けていたら（打ち切って結果を返した）知らせは要らない。
            let _unwanted = done.send(());
        })
        .context("process の出力を読むスレッドを起動できない")?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum MarkerScan {
    /// 目印の行を読んだ（PID・その後ろの出力）。
    Found(u32, Vec<u8>),
    /// 先頭は目印ではない。
    NotMarker,
    /// まだ判断できない（1 行目を読み切っていない）。
    Incomplete,
}

fn split_pid_marker(bytes: &[u8]) -> MarkerScan {
    let marker = USER_COMMAND_PID_MARKER.as_bytes();
    let prefix = bytes.len().min(marker.len());
    if bytes[..prefix] != marker[..prefix] {
        return MarkerScan::NotMarker;
    }
    let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
        return if bytes.len() > USER_COMMAND_PID_LINE_MAX {
            MarkerScan::NotMarker
        } else {
            MarkerScan::Incomplete
        };
    };
    if newline < marker.len() {
        return MarkerScan::NotMarker;
    }
    match std::str::from_utf8(&bytes[marker.len()..newline])
        .ok()
        .and_then(|pid| pid.trim().parse().ok())
    {
        Some(pid) => MarkerScan::Found(pid, bytes[newline + 1..].to_vec()),
        None => MarkerScan::NotMarker,
    }
}

/// パイプを上限つきで最後まで読むスレッドを立てる（読み終えたら `done` へ知らせる）。
/// 上限を超えた分は [`CaptureBuffer`] が頭と尻を残して捨てる＝子は止めない。
fn capture_pipe_in_background(
    mut pipe: impl Read + Send + 'static,
    buffer: Arc<Mutex<CaptureBuffer>>,
    live: LiveOutput,
    done: mpsc::Sender<()>,
) -> Result<()> {
    thread::Builder::new()
        .name("necoder-command-output".to_string())
        .spawn(move || {
            let mut chunk = [0u8; 16 * 1024];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        buffer
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(&chunk[..read]);
                        live.push(&chunk[..read]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        eprintln!("コマンドの出力を読めない: {error}");
                        break;
                    }
                }
            }
            // 受け手が先に抜けていたら（打ち切って結果を返した）知らせは要らない。
            let _unwanted = done.send(());
        })
        .context("process の出力を読むスレッドを起動できない")?;
    Ok(())
}

/// 子のシグナルの状態を既定へ戻す（`pre_exec`・fork の後 exec の前に子の中で走る）。
///
/// 背景の executor のスレッド（macOS は GCD）は非同期のシグナルを**ブロック**していて、Rust の
/// `Command` はそのマスクを子へそのまま継ぐ（std の仕様・SIGPIPE の扱いしか直さない）。そのままだと
/// 子は SIGTERM も SIGINT も受け取れず、止める操作が猶予の後の SIGKILL まで効かない（git が lock を
/// 片付けられない・終了時の SIGTERM が届かず子が残る）。人のコマンドは端末と同じ既定の状態で
/// 走らせたいので、マスクを空にし、無視の設定も既定へ戻す（端末の PTY の子と同じ顔ぶれ）。
#[cfg(unix)]
fn reset_child_signals() -> std::io::Result<()> {
    // SAFETY: どれも async-signal-safe な libc の関数を、宣言どおりの引数で呼ぶだけ。
    // `mask` は sigemptyset が初期化してから読む。
    unsafe {
        let mut mask = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        if libc::sigemptyset(mask.as_mut_ptr()) != 0
            || libc::sigprocmask(libc::SIG_SETMASK, mask.as_ptr(), std::ptr::null_mut()) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        for signal in [
            libc::SIGCHLD,
            libc::SIGHUP,
            libc::SIGINT,
            libc::SIGQUIT,
            libc::SIGTERM,
            libc::SIGALRM,
        ] {
            if libc::signal(signal, libc::SIG_DFL) == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum StopSignal {
    /// SIGTERM（行儀よく終わらせる）。
    Terminate,
    /// SIGKILL。
    Kill,
}

/// 子のプロセスグループ全体へシグナルを送る。**子をまだ回収していない間だけ呼ぶ**こと
/// （回収前ならグループの id は他のプロセスへ再利用されていない）。
#[cfg(unix)]
fn signal_process_group(child: &mut Child, signal: StopSignal) {
    let signal = match signal {
        StopSignal::Terminate => libc::SIGTERM,
        StopSignal::Kill => libc::SIGKILL,
    };
    let Ok(group) = libc::pid_t::try_from(child.id()) else {
        // pid が pid_t に収まらないことは無いが、収まらなければ子だけでも止める。
        let _already_exited = child.kill();
        return;
    };
    // SAFETY: killpg はシグナルを送るだけでメモリに触れない。相手は `process_group(0)` で起こした
    // 子のグループ（id = 子の pid）で、呼ぶのは子を回収する前だけ。
    let result = unsafe { libc::killpg(group, signal) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        // ESRCH = グループがもう空（全員終わっている）。それで良い。
        if error.raw_os_error() != Some(libc::ESRCH) {
            eprintln!("コマンドのプロセスグループを止められない: {error}");
        }
    }
}

/// Windows はグループで止める口を持たない（孫は残る）。子だけ止める。
#[cfg(not(unix))]
fn signal_process_group(child: &mut Child, _signal: StopSignal) {
    if let Err(error) = child.kill() {
        eprintln!("コマンドを止められない: {error}");
    }
}

/// 起動の途中で失敗した子をグループごと止めて回収する（ゾンビも孫も残さない）。
fn stop_user_command(child: &mut Child) {
    signal_process_group(child, StopSignal::Kill);
    if let Err(error) = child.wait() {
        eprintln!("止めた process を回収できない: {error}");
    }
}

fn spawn_process_local(spec: &CommandSpec) -> Result<HostProcess> {
    let mut child = child_process::command(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(&spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // 末尾だけを持つ（すぐ落ちた子の理由を見せる・[`StderrTail`]）。読む側は最後まで読み続ける。
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("process を起動できない: {}", spec.program))?;
    let stdin = child.stdin.take().context("process stdin が無い")?;
    let stdout = child.stdout.take().context("process stdout が無い")?;
    let stderr = child.stderr.take().context("process stderr が無い")?;
    Ok(HostProcess {
        backend: ProcessBackend::Local(child),
        stdin: Some(Box::new(stdin)),
        stdout: Some(Box::new(stdout)),
        stderr: StderrTail::capture(stderr),
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
    /// ストリーム（プロセス・PTY・TCP）の中身と制御。id はストリーム id（`stream.rs`）。
    Stream = 5,
}

impl FrameKind {
    fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            1 => Ok(Self::Request),
            2 => Ok(Self::Response),
            3 => Ok(Self::Error),
            4 => Ok(Self::Event),
            5 => Ok(Self::Stream),
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
    /// パイプの stdio でプロセスを起こす（LSP・ACP・人のコマンド）。`stream_id` は client が決める
    /// （応答より先に出力が届いても、client はもう受け口を用意している）。
    Spawn {
        project_id: u64,
        stream_id: u64,
        spec: CommandSpec,
    },
    /// ログインシェルを PTY で起こす（端末）。`cwd` は project 相対。
    OpenTerminal {
        project_id: u64,
        stream_id: u64,
        cwd: PathBuf,
        size: PtySize,
        env: HashMap<String, String>,
    },
    /// 接続先の 127.0.0.1:`port` へ TCP で繋ぐ（ポート転送の 1 本）。
    OpenTcp {
        stream_id: u64,
        port: u16,
    },
    /// 繋ぎ直した client が、ストリームの続きを頼む（受け取った位置から送り直してもらう）。
    Attach {
        stream_id: u64,
        output_offset: u64,
        error_offset: u64,
    },
    /// `ne` の open（[`ServerEvent::CliOpen`]）への答え。
    CliOpenResult {
        request: u64,
        response: serde_json::Value,
    },
    Ping,
    Shutdown,
}

impl Request {
    /// 診断メッセージ用の短い名前（UI スレッド違反の指摘に「どの request か」を載せる）。
    fn name(&self) -> &'static str {
        match self {
            Self::Spawn { .. } => "Spawn",
            Self::OpenTerminal { .. } => "OpenTerminal",
            Self::OpenTcp { .. } => "OpenTcp",
            Self::Attach { .. } => "Attach",
            Self::CliOpenResult { .. } => "CliOpenResult",
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
    /// ストリームを開いた（`Spawn` / `OpenTerminal` / `OpenTcp`）。
    Opened,
    /// 付け直した。`input_offset` = daemon が受け取った入力の位置（client はそこから送り直す）。
    Attached {
        input_offset: u64,
    },
    Pong,
    Ack,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireError {
    code: String,
    message: String,
}

/// daemon → client の push イベント（[`FrameKind::Event`] frame の meta）。
#[derive(Debug, Clone, Serialize, Deserialize)]
enum ServerEvent {
    /// remote watch の変更通知。変更/追加/削除のあった project 相対パス（1 イベントの上限内に収める）。
    Watch { paths: Vec<PathBuf> },
    /// 接続先の端末で `ne <path>…` が打たれた。client は接続元の GUI で開き、
    /// [`Request::CliOpenResult`] で答える。
    CliOpen { request: u64, paths: Vec<String> },
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
/// 時間切れは接続が黙っている（sleep 復帰直後の死んだ TCP）印で、閉じたのとは扱いが違う。
/// （閉じたのなら ssh は終わっている）。
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
    // 1 frame は 1 回の書き込みで出す。header と中身を分けて書くと、間の proxy / sshd がそれぞれを
    // 別の TCP の塊で送り、Nagle と相手の遅延 ACK が噛み合って 1 往復ごとに ~40ms 待つ
    // （`-T` の ssh は TCP_NODELAY を立てない・2026-10-21 実測: 45ms → 下記で 1ms 台）。
    // 大きな body だけは写さずに続けて書く（塊が大きいので Nagle は効かない）。
    const COALESCE_BODY_LIMIT: usize = 256 * 1024;
    let inline_body = frame.body.len() <= COALESCE_BODY_LIMIT;
    let mut bytes = Vec::with_capacity(
        FRAME_HEADER_LEN + frame.meta.len() + if inline_body { frame.body.len() } else { 0 },
    );
    bytes.extend_from_slice(&FRAME_MAGIC);
    bytes.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    bytes.extend_from_slice(&[frame.kind as u8, 0]);
    bytes.extend_from_slice(&frame.id.to_le_bytes());
    bytes.extend_from_slice(&(frame.meta.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(frame.body.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&frame.meta);
    if inline_body {
        bytes.extend_from_slice(&frame.body);
    }
    writer.write_all(&bytes)?;
    if !inline_body {
        writer.write_all(&frame.body)?;
    }
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
        Request::Watch { .. }
        | Request::Unwatch { .. }
        | Request::Spawn { .. }
        | Request::OpenTerminal { .. }
        | Request::OpenTcp { .. }
        | Request::Attach { .. }
        | Request::CliOpenResult { .. } => {
            bail!(
                "{} は接続スコープで処理する（handle_request では未対応）",
                request.name()
            )
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

/// 1 接続だけの server（`serve --stdio`・テスト）。接続が終わったら、持っているプロセスは全部止める
/// （daemon ではないので、続きを受け取りに来る client は居ない）。
fn serve_stream(reader: impl Read, writer: impl Write + Send + 'static) -> Result<()> {
    let streams = Arc::new(ServerStreams::default());
    let link = Link::new(Box::new(writer), None);
    streams.connect(&link);
    let result = serve_connection(
        reader,
        link.clone(),
        Arc::new(Mutex::new(ServerState::new())),
        streams.clone(),
    );
    streams.disconnect(&link);
    streams.close_all();
    result
}

/// 1 接続ぶんの request とストリームを捌く。`state`（開いた project）と `streams`（プロセス・端末）は
/// daemon では接続をまたいで生き続ける。
fn serve_connection(
    mut reader: impl Read,
    link: Arc<Link>,
    state: SharedServerState,
    streams: Arc<ServerStreams>,
) -> Result<()> {
    const SERVER_WORKERS: usize = 8;
    const REQUEST_QUEUE: usize = 32;

    thread::scope(|scope| -> Result<()> {
        let (request_tx, request_rx) = mpsc::sync_channel::<Frame>(REQUEST_QUEUE);
        let request_rx = Arc::new(Mutex::new(request_rx));
        // remote watch（M13）: この接続スコープに 1 本の poll マネージャ。workers が Watch/Unwatch を
        // 制御チャネルで送り、マネージャが差分を [`FrameKind::Event`] frame として push する。
        // 接続が切れて workers が終わると watch_tx が全て drop → マネージャも Disconnected で終了する。
        // watch 未使用の間は recv() で完全ブロック＝idle 0%。使用中だけ POLL 間隔で起きる。
        let (watch_tx, watch_rx) = mpsc::channel::<WatchControl>();
        {
            let link = link.clone();
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
                                        serde_json::to_vec(&ServerEvent::Watch { paths: changed })
                                    {
                                        let frame = Frame {
                                            kind: FrameKind::Event,
                                            id: 0,
                                            meta,
                                            body: Vec::new(),
                                        };
                                        // 切れていれば届かないだけ（再接続で client が Watch を送り直す）。
                                        let _written = link.write(&frame);
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
            let link = link.clone();
            let state = state.clone();
            let streams = streams.clone();
            let watch_tx = watch_tx.clone();
            scope.spawn(move || loop {
                let frame = match request_rx.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => return,
                };
                let Ok(frame) = frame else {
                    return;
                };
                let parsed = serde_json::from_slice::<Request>(&frame.meta)
                    .context("invalid request metadata");
                // Attach は応答を書いた**後**に続きを送る（client は応答で入力の位置を知ってから送り直す）。
                let mut attached = None;
                let response = match parsed {
                    // Watch/Unwatch は watch マネージャへ回して Ack（handle_request は無状態のため）。
                    Ok(Request::Watch { project_id, root }) => {
                        server_resolve_existing(&state, project_id, &root).map(|absolute| {
                            let _sent = watch_tx.send(WatchControl::Start { root: absolute });
                            (Response::Ack, Vec::new())
                        })
                    }
                    Ok(Request::Unwatch { .. }) => {
                        let _sent = watch_tx.send(WatchControl::Stop);
                        Ok((Response::Ack, Vec::new()))
                    }
                    Ok(Request::Spawn {
                        project_id,
                        stream_id,
                        mut spec,
                    }) => server_resolve_existing(&state, project_id, &spec.cwd)
                        .and_then(|cwd| {
                            spec.cwd = cwd;
                            streams.spawn_process(&link, stream_id, &spec)
                        })
                        .map(|()| (Response::Opened, Vec::new())),
                    Ok(Request::OpenTerminal {
                        project_id,
                        stream_id,
                        cwd,
                        size,
                        env,
                    }) => server_resolve_existing(&state, project_id, &cwd)
                        .and_then(|cwd| streams.open_terminal(&link, stream_id, &cwd, size, &env))
                        .map(|()| (Response::Opened, Vec::new())),
                    Ok(Request::OpenTcp { stream_id, port }) => streams
                        .open_tcp(&link, stream_id, port)
                        .map(|()| (Response::Opened, Vec::new())),
                    Ok(Request::Attach {
                        stream_id,
                        output_offset,
                        error_offset,
                    }) => streams
                        .attach(&link, stream_id, output_offset, error_offset)
                        .map(|(input_offset, stream)| {
                            attached = Some(stream);
                            (Response::Attached { input_offset }, Vec::new())
                        }),
                    Ok(Request::CliOpenResult { request, response }) => {
                        streams.cli_result(request, response);
                        Ok((Response::Ack, Vec::new()))
                    }
                    Ok(request) => handle_request(&state, request, frame.body),
                    Err(error) => Err(error),
                };
                let response = match response {
                    Ok((response, body)) => Frame::response(frame.id, &response, body),
                    Err(error) => Frame::error(frame.id, "request_failed", &error),
                };
                if let Err(error) = response.and_then(|response| link.write(&response)) {
                    remote_trace(&format!(
                        "server worker {worker}: response write failed: {error:#}"
                    ));
                    return;
                }
                if let Some(stream) = attached {
                    stream.flush();
                }
            });
        }
        // 親（read ループ）は watch を送らない。workers の clone だけが manager を生かす。
        drop(watch_tx);

        let result = loop {
            remote_trace("server: waiting frame");
            let frame = match read_frame(&mut reader) {
                Ok(frame) => frame,
                Err(error)
                    if error.downcast_ref::<std::io::Error>().is_some_and(|io| {
                        matches!(
                            io.kind(),
                            std::io::ErrorKind::UnexpectedEof
                                | std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                                | std::io::ErrorKind::NotConnected
                        )
                    }) =>
                {
                    break Ok(());
                }
                // daemon が新しい接続へ乗り換えて、この接続を切った。
                Err(_) if !link.is_alive() => break Ok(()),
                Err(error) => break Err(error),
            };
            remote_trace("server: received frame");
            match frame.kind {
                FrameKind::Stream => streams.dispatch(frame),
                FrameKind::Request => {
                    let should_shutdown = serde_json::from_slice::<Request>(&frame.meta)
                        .is_ok_and(|request| matches!(request, Request::Shutdown));
                    if request_tx.send(frame).is_err() {
                        break Err(anyhow!("remote request worker queue closed"));
                    }
                    if should_shutdown {
                        break Ok(());
                    }
                }
                kind => break Err(anyhow!("server が受け取れない frame: {kind:?}")),
            }
        };
        drop(request_tx);
        result
    })
}

/// daemon の置き場（owner-only）。macOS の SUN_LEN は短いので `/tmp` 直下の短い名前にする。
#[cfg(unix)]
fn session_directory() -> PathBuf {
    // SAFETY: getuid は失敗しない（引数もメモリも触らない）。
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!(
        "/tmp/necoder-remote-{uid}-{SERVER_VERSION}-p{PROTOCOL_VERSION}"
    ))
}

#[cfg(unix)]
fn session_socket(session: &str) -> Result<PathBuf> {
    if session.len() != 64 || !session.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("session id が不正");
    }
    // owner-only directory + 160-bit socket name で安全性と長さを両立する。
    Ok(session_directory().join(format!("{}.sock", &session[..40])))
}

/// client が 1 つも繋がっていない時間がこれだけ続いたら、daemon は持っているプロセスと端末を止めて終わる
/// （以前の ControlPersist=600 と同じ長さ）。
#[cfg(unix)]
const DAEMON_IDLE: Duration = Duration::from_secs(600);

/// session ごとの daemon。プロセスと端末を持ち、接続が切れても生かしておく。新しい接続が来たら
/// それをいまの client にし、古い接続は切る（1 session = 1 client・docs/REMOTE-CHANNEL.md §2）。
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
    let streams = Arc::new(ServerStreams::default());
    // `ne` が無くても Remote SSH 本体は使える。受け口を作れなければ警告だけ（端末の `ne` が使えない）。
    let cli_gate = match RemoteCliGate::start(socket, &streams) {
        Ok(gate) => Some(gate),
        Err(error) => {
            eprintln!("necoder-remote-server: `ne` の受け口を作れない: {error:#}");
            None
        }
    };
    let mut idle_since = Instant::now();
    let result = loop {
        match listener.accept() {
            Ok((stream, _)) => {
                remote_trace("daemon: accepted proxy");
                stream.set_nonblocking(false)?;
                let reader = stream.try_clone()?;
                let closer = stream.try_clone()?;
                let link = Link::new(
                    Box::new(stream),
                    Some(Box::new(move || {
                        let _closed = closer.shutdown(std::net::Shutdown::Both);
                    })),
                );
                streams.connect(&link);
                let state = state.clone();
                let streams = streams.clone();
                let spawned = thread::Builder::new()
                    .name("necoder-daemon-connection".to_string())
                    .spawn(move || {
                        if let Err(error) =
                            serve_connection(reader, link.clone(), state, streams.clone())
                        {
                            eprintln!("remote proxy session error: {error:#}");
                        }
                        streams.disconnect(&link);
                    });
                if let Err(error) = spawned {
                    eprintln!("necoder-remote-server: 接続のスレッドを起動できない: {error}");
                }
                idle_since = Instant::now();
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if state
                    .lock()
                    .map_err(|_| anyhow!("server state lock poisoned"))?
                    .shutdown
                {
                    break Ok(());
                }
                if streams.has_client() {
                    idle_since = Instant::now();
                } else if idle_since.elapsed() >= DAEMON_IDLE {
                    break Ok(());
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => break Err(error.into()),
        }
    };
    streams.close_all();
    drop(cli_gate);
    let _cleanup = std::fs::remove_file(socket);
    result
}

/// 接続先の端末の `ne` の受け口（daemon の Unix socket）。`ne` の open を client（接続元の GUI）へ
/// Event で頼み、答えを返す。以前は ssh の `-O forward -R` で GUI の socket を接続先へ引いていた。
#[cfg(unix)]
struct RemoteCliGate {
    socket: PathBuf,
    bin_directory: PathBuf,
}

#[cfg(unix)]
impl RemoteCliGate {
    fn start(session_socket: &Path, streams: &Arc<ServerStreams>) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::net::UnixListener;

        let socket = session_socket.with_extension("cli");
        let bin_directory = session_socket.with_extension("bin");
        if socket.exists() {
            let _stale = std::fs::remove_file(&socket);
        }
        let listener = UnixListener::bind(&socket)
            .with_context(|| format!("`ne` の socket を bind できない: {}", socket.display()))?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        std::fs::create_dir_all(&bin_directory)?;
        std::fs::set_permissions(&bin_directory, std::fs::Permissions::from_mode(0o700))?;
        let executable = std::env::current_exe().context("remote server の実体が分からない")?;
        let shim = bin_directory.join("ne");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\nexec {} cli \"$@\"\n",
                quote_posix(&executable.to_string_lossy())
            ),
        )?;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700))?;
        let path = std::env::var("PATH").unwrap_or_default();
        streams.set_terminal_env(vec![
            (
                "PATH".to_string(),
                format!("{}:{path}", bin_directory.display()),
            ),
            ("NECODER_GUI_SOCK".to_string(), socket.display().to_string()),
        ]);
        let streams = streams.clone();
        thread::Builder::new()
            .name("necoder-remote-cli".to_string())
            .spawn(move || {
                for connection in listener.incoming() {
                    let Ok(connection) = connection else {
                        break;
                    };
                    let streams = streams.clone();
                    let spawned = thread::Builder::new()
                        .name("necoder-remote-cli-open".to_string())
                        .spawn(move || {
                            if let Err(error) = answer_remote_cli(connection, &streams) {
                                remote_trace(&format!("`ne` に答えられない: {error:#}"));
                            }
                        });
                    if let Err(error) = spawned {
                        remote_trace(&format!("`ne` のスレッドを起動できない: {error}"));
                    }
                }
            })
            .context("`ne` の受け口のスレッドを起動できない")?;
        Ok(Self {
            socket,
            bin_directory,
        })
    }
}

#[cfg(unix)]
impl Drop for RemoteCliGate {
    fn drop(&mut self) {
        let _socket = std::fs::remove_file(&self.socket);
        let _shims = std::fs::remove_dir_all(&self.bin_directory);
    }
}

/// `ne` の 1 接続（1 行の JSON 要求 → 1 行の JSON 応答）。受け付けるのは `open` だけ。
#[cfg(unix)]
fn answer_remote_cli(
    mut connection: std::os::unix::net::UnixStream,
    streams: &ServerStreams,
) -> Result<()> {
    connection.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut line = String::new();
    BufReader::new(&mut connection)
        .take((MAX_META_LEN + 1) as u64)
        .read_line(&mut line)?;
    if line.is_empty() {
        return Ok(());
    }
    let answer = match serde_json::from_str::<serde_json::Value>(&line) {
        Ok(request)
            if request.get("method").and_then(serde_json::Value::as_str) == Some("open") =>
        {
            let paths = request
                .get("params")
                .and_then(|params| params.get("paths"))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect();
            streams.cli_open(paths)
        }
        Ok(_) => serde_json::json!({
            "ok": false,
            "error": "remote terminal から許可されている操作は open だけです",
        }),
        Err(error) => serde_json::json!({
            "ok": false,
            "error": format!("要求が JSON ではない: {error}"),
        }),
    };
    writeln!(connection, "{answer}")?;
    connection.flush()?;
    Ok(())
}

#[cfg(unix)]
fn connect_session_proxy(session: &str) -> Result<()> {
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt as _;

    let socket = session_socket(session)?;
    let mut stream = match UnixStream::connect(&socket) {
        Ok(stream) => {
            remote_trace("proxy: connected existing daemon");
            stream
        }
        Err(_) => {
            let executable =
                std::env::current_exe().context("remote server executable path が無い")?;
            let directory = session_directory();
            std::fs::create_dir_all(&directory)?;
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
            }
            // daemon は ssh の接続より長く生きる。stderr を ssh へ繋いだままにすると、接続が切れた後の
            // 書き込みが失敗して daemon が落ちる（`eprintln!` は書けないと panic する）。置き場のログへ向ける。
            let log = std::fs::File::create(socket.with_extension("log"))
                .context("daemon のログを作れない")?;
            child_process::command(executable)
                .arg("daemon")
                .arg("--socket")
                .arg(&socket)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(log))
                // proxy（= ssh のセッション）のグループから外す。セッションへのシグナルで daemon が巻き込まれない。
                .process_group(0)
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

/// Remote SSH の統合ターミナルに置く軽量 `ne`。`NECODER_GUI_SOCK` は daemon の `ne` の受け口
/// （[`RemoteCliGate`]）で、daemon が接続元の GUI へ open だけを頼む。
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
    stream.set_read_timeout(Some(Duration::from_secs(90)))?;
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
            serve_stream(std::io::stdin().lock(), std::io::stdout())
        }
        [command, flag] if command == "serve" && flag == "--stdio" => {
            serve_stream(std::io::stdin().lock(), std::io::stdout())
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

/// 接続の子プロセス（ssh / テストの server）。drop で落として回収する。
struct ProcessOwner(Mutex<Option<Child>>);

impl ProcessOwner {
    fn new(child: Child) -> Self {
        Self(Mutex::new(Some(child)))
    }

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

    /// 終わるまで最長 `timeout` 待ち、終わっていればその終わり方（回収はしない＝drop が回収する）。
    fn wait_for_exit(&self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let status = self.0.lock().ok().and_then(|mut slot| {
                slot.as_mut()
                    .and_then(|child| child.try_wait().ok().flatten())
            });
            if status.is_some() || std::time::Instant::now() >= deadline {
                return status;
            }
            thread::sleep(Duration::from_millis(20));
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

/// 接続先から届く push（watch の変更・`ne` の open・ストリーム）の受け口。RpcClient を跨いで
/// （再接続でも）生き続けるよう [`ReconnectingClient`] が Arc で保持し、各 RpcClient の reader が
/// ここへ流す。watch の購読が無ければ変更通知は捨てる。
#[derive(Default)]
struct EventSink {
    sender: Mutex<Option<(u64, mpsc::Sender<Vec<PathBuf>>)>>,
    next_subscription_id: AtomicU64,
    /// プロセス・端末・TCP のストリーム。
    streams: Arc<StreamHub>,
    /// `ne` の open を接続元の GUI へ渡す時に刻む接続先（`ssh://user@host:port`）。空なら断る。
    authority: String,
}

impl EventSink {
    fn with_authority(authority: impl Into<String>) -> Self {
        Self {
            authority: authority.into(),
            ..Self::default()
        }
    }

    /// reader スレッドから呼ぶ。**ブロックしない**（`ne` の open は別スレッドで GUI に頼む）。
    fn receive_event(&self, event: ServerEvent) {
        match event {
            ServerEvent::Watch { paths } => self.dispatch(paths),
            ServerEvent::CliOpen { request, paths } => {
                let authority = self.authority.clone();
                let streams = self.streams.clone();
                let spawned = thread::Builder::new()
                    .name("necoder-remote-open".to_string())
                    .spawn(move || {
                        let response = open_remote_paths_in_gui(&authority, &paths);
                        let Some(link) = streams.current_link() else {
                            return;
                        };
                        if let Err(error) =
                            link.request(&Request::CliOpenResult { request, response }, Vec::new())
                        {
                            eprintln!("Remote SSH: `ne` の答えを返せない: {error:#}");
                        }
                    });
                if let Err(error) = spawned {
                    eprintln!("Remote SSH: `ne` のスレッドを起動できない: {error}");
                }
            }
        }
    }
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

    fn dispatch(&self, paths: Vec<PathBuf>) {
        if let Ok(slot) = self.sender.lock() {
            if let Some((_, sender)) = slot.as_ref() {
                let _sent = sender.send(paths);
            }
        }
    }
}

/// 接続先の端末の `ne` が頼んだパスを、接続元の GUI で開く（GUI の制御口の `open_remote`）。
/// 返すのは `ne` へそのまま返す 1 行 JSON（`{"ok":…}`）。
///
/// GUI の制御口は手元の Unix socket（mac / Linux）か名前付きパイプ（Windows）。この関数は GUI の
/// プロセスの中で走るが、口を通すことで「接続先 identity をここで刻む」検査を 1 か所（`control_ipc`）に保つ。
fn open_remote_paths_in_gui(authority: &str, paths: &[String]) -> serde_json::Value {
    if authority.is_empty() {
        return serde_json::json!({ "ok": false, "error": "この接続は `ne` を受け付けない" });
    }
    let request = serde_json::json!({
        "method": "open_remote",
        "params": { "authority": authority, "paths": paths },
    });
    match gui_control_request(&request) {
        Ok(response) => response,
        Err(error) => serde_json::json!({
            "ok": false,
            "error": format!("接続元の necoder へ渡せませんでした: {error:#}"),
        }),
    }
}

/// GUI の制御口へ 1 往復（1 行 JSON の要求 → 1 行 JSON の応答）。
fn gui_control_request(request: &serde_json::Value) -> Result<serde_json::Value> {
    use std::io::{BufRead as _, BufReader};

    let socket = paths::runtime_socket().context("GUI の制御口が無い")?;
    #[cfg(unix)]
    let connection = {
        let connection = std::os::unix::net::UnixStream::connect(&socket)
            .with_context(|| format!("GUI socket（{}）に接続できない", socket.display()))?;
        connection.set_read_timeout(Some(Duration::from_secs(60)))?;
        connection
    };
    // 名前付きパイプはファイルとして開ける（クライアント側は CreateFileW と同じ）。
    #[cfg(windows)]
    let connection = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&socket)
        .with_context(|| format!("GUI のパイプ（{}）に接続できない", socket.display()))?;
    let mut writer = &connection;
    writeln!(writer, "{request}").context("GUI へ送れない")?;
    writer.flush()?;
    let mut line = String::new();
    BufReader::new(&connection)
        .take((MAX_META_LEN + 1) as u64)
        .read_line(&mut line)
        .context("GUI 応答を読めない")?;
    anyhow::ensure!(!line.is_empty(), "GUI 応答が空です");
    serde_json::from_str(&line).context("GUI 応答が JSON ではない")
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
    event_sink: Arc<EventSink>,
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
        event_sink: Arc<EventSink>,
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
                            // Event frame（id を持たない push 通知）と Stream frame は受け口へ振り分ける。
                            match frame.kind {
                                FrameKind::Event => {
                                    if let Ok(event) =
                                        serde_json::from_slice::<ServerEvent>(&frame.meta)
                                    {
                                        event_sink.receive_event(event);
                                    }
                                    continue;
                                }
                                FrameKind::Stream => {
                                    event_sink.streams.dispatch(frame);
                                    continue;
                                }
                                FrameKind::Request | FrameKind::Response | FrameKind::Error => {}
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

    /// 応答を待たない frame（ストリームの中身と制御）を送る。
    fn send_frame(&self, frame: &Frame) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("remote writer lock poisoned"))?;
        write_frame(writer.as_mut(), frame)
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

/// SSH config の置き場。通常は `~/.ssh/config`、テストでは transport と同じ `NECODER_SSH_CONFIG`。
pub fn ssh_config_path() -> Option<PathBuf> {
    match std::env::var_os("NECODER_SSH_CONFIG") {
        Some(path) => Some(PathBuf::from(path)),
        // Windows の OpenSSH も `%USERPROFILE%\.ssh\config` を見る（paths が USERPROFILE を解決する）。
        None => paths::home_dir().map(|home| home.join(".ssh/config")),
    }
}

/// SSH config を読んで接続可能なホスト一覧を返す（読めなければ空）。
pub fn ssh_config_hosts() -> Vec<SshConfigHost> {
    let Some(path) = ssh_config_path() else {
        return Vec::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_ssh_config(&text),
        Err(_) => Vec::new(),
    }
}

/// 接続先の登録（O37・G01）で SSH config に足す 1 つ。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewSshHost {
    /// `Host` の名前（`ssh <名前>` で使う）。
    pub alias: String,
    /// `HostName`（ホスト名か IP）。
    pub hostname: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// `IdentityFile`（`~/` 可）。
    pub identity_file: Option<String>,
    /// `ProxyJump`（経由するホスト・`名前` か `user@host:port`・`,` で続けて複数）。
    pub proxy_jump: Option<String>,
}

/// 1 語の値として config に書けるか（空白・引用符・`#`・先頭の `-` を含まない）。
fn plain_ssh_word(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value
            .chars()
            .any(|character| character.is_whitespace() || matches!(character, '"' | '\'' | '#'))
}

/// ProxyJump の値か（`[user@]host[:port]` か `ssh://[user@]host[:port]` を `,` でつないだもの）。
/// どの経由先も、ユーザー名とホスト名が空でなく `-` で始まらず（ssh のオプションに化けない）、使うのは
/// ホスト名・ユーザー名・ポート・IPv6 の角括弧の文字だけ（`;` や `$` などは通さない）。先頭の経由先だけ
/// 見ていた時は `bastion,-oProxyCommand=…` を通していた（レビューで見つけた）。
fn proxy_jump_value(value: &str) -> bool {
    value.split(',').all(|hop| {
        let hop = hop.strip_prefix("ssh://").unwrap_or(hop);
        !hop.is_empty()
            && hop
                .split('@')
                .all(|part| !part.is_empty() && !part.starts_with('-'))
            && hop.chars().all(|character| {
                character.is_ascii_alphanumeric()
                    || matches!(character, '.' | '-' | '_' | '@' | ':' | '[' | ']' | '%')
            })
    })
}

impl NewSshHost {
    /// 書く前に確かめる（書けない値は理由つきで断る・config を壊さない）。
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            plain_ssh_word(&self.alias)
                && !self
                    .alias
                    .chars()
                    .any(|character| matches!(character, '*' | '?' | '!' | ',')),
            "名前には空白や記号（* ? ! , # 引用符）を使えません: {}",
            self.alias
        );
        anyhow::ensure!(
            plain_ssh_word(&self.hostname),
            "ホスト名に使えない文字があります: {}",
            self.hostname
        );
        if let Some(user) = &self.user {
            anyhow::ensure!(
                plain_ssh_word(user),
                "ユーザー名に使えない文字があります: {user}"
            );
        }
        anyhow::ensure!(self.port != Some(0), "ポートは 1〜65535");
        if let Some(identity) = &self.identity_file {
            anyhow::ensure!(
                !identity.is_empty()
                    && !identity.contains(['"', '\n', '\r'])
                    && !identity.starts_with('-'),
                "鍵のファイルに使えない文字があります: {identity}"
            );
        }
        if let Some(proxy_jump) = &self.proxy_jump {
            anyhow::ensure!(
                proxy_jump_value(proxy_jump),
                "経由するホストに使えない文字があります: {proxy_jump}"
            );
        }
        Ok(())
    }

    /// 足す塊（前に空行・necoder が足した印のコメント・末尾に改行）。空白を含む鍵のパスは引用符で囲む。
    pub fn config_block(&self) -> String {
        let mut block = format!(
            "\n# necoder で登録\nHost {}\n  HostName {}\n",
            self.alias, self.hostname
        );
        if let Some(user) = &self.user {
            block.push_str(&format!("  User {user}\n"));
        }
        if let Some(port) = self.port {
            block.push_str(&format!("  Port {port}\n"));
        }
        if let Some(proxy_jump) = &self.proxy_jump {
            block.push_str(&format!("  ProxyJump {proxy_jump}\n"));
        }
        if let Some(identity) = &self.identity_file {
            if identity.contains(char::is_whitespace) {
                block.push_str(&format!("  IdentityFile \"{identity}\"\n"));
            } else {
                block.push_str(&format!("  IdentityFile {identity}\n"));
            }
        }
        block
    }
}

/// SSH config の末尾に `host` を足す（O37・G01）。**既存の行は触らない**（追記だけ）。同じ名前の
/// `Host` があれば断る。config が無ければ作る（unix は `~/.ssh` を 0700・config を 0600）。
pub fn append_ssh_config_host(path: &Path, host: &NewSshHost) -> Result<()> {
    host.validate()?;
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("{} を読めない", path.display()));
        }
    };
    anyhow::ensure!(
        !parse_ssh_config(&existing)
            .iter()
            .any(|known| known.alias == host.alias),
        "同じ名前の接続先があります: {}",
        host.alias
    );
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("{} を作れない", parent.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("{} を開けない", path.display()))?;
    let mut text = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&host.config_block());
    file.write_all(text.as_bytes())
        .with_context(|| format!("{} に書けない", path.display()))?;
    Ok(())
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

/// 接続を張る側の抽象。実物は [`ssh_link::SshConnector`]（system OpenSSH の 1 本の接続）。
/// [`ReconnectingClient`] の再接続ロジックを ssh 無しで検証できるよう、テストは
/// 同じ protocol を喋る byte stream の対を返す connector を差す。
trait Connector: Send + Sync {
    fn connect(&self, event_sink: Arc<EventSink>) -> Result<Arc<RpcClient>>;
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
    event_sink: Arc<EventSink>,
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
        event_sink: Arc<EventSink>,
    ) -> Arc<Self> {
        Self::with_current(Some(current), connector, event_sink)
    }

    /// 接続を張らずに作る。最初の request まで ssh を起こさない（起動を止めないため）。
    fn disconnected(connector: Arc<dyn Connector>, event_sink: Arc<EventSink>) -> Arc<Self> {
        Self::with_current(None, Some(connector), event_sink)
    }

    fn with_current(
        current: Option<Arc<RpcClient>>,
        connector: Option<Arc<dyn Connector>>,
        event_sink: Arc<EventSink>,
    ) -> Arc<Self> {
        let initial_state = if current.is_some() {
            ConnectionState::Connected
        } else {
            ConnectionState::Unconnected
        };
        if let Some(current) = &current {
            event_sink.streams.set_link(current);
        }
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

    fn event_sink(&self) -> Arc<EventSink> {
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
            // 接続を張り直さず、そのまま呼び出し元へ返す。
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
        // 端末・LSP・エージェントのストリームを新しい接続へ付け直す（daemon 側で生きていれば続きから）。
        self.event_sink.streams.set_link(&replacement);
        self.event_sink.streams.reattach(&replacement);
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
    /// daemon が付けた project の番号。端末を開く口（[`RemoteTerminal`]）とも共有する
    /// （daemon が作り直されたら付け直した番号を全員が見る）。
    project_id: Arc<AtomicU64>,
    client: Arc<ReconnectingClient>,
    /// watch の入れ替えと Unwatch を直列化する。`open_project` で作る別 root とも共有。
    watch_control: Arc<Mutex<()>>,
    ssh_project: Option<SshProject>,
    connector: Option<Arc<ssh_link::SshConnector>>,
    /// ポートの転送（手元の待ち受け）。同じ接続の別 root とも共有する。
    forwards: Arc<PortForwards>,
}

impl RemoteHost {
    /// system OpenSSH で 1 本繋ぎ、互換 server を（無ければ同じ接続の中で配備して）session proxy へ接続する。
    pub fn connect_ssh(project: &SshProject, server_command: &str) -> Result<Arc<Self>> {
        let connector = Arc::new(ssh_link::SshConnector::new(project, server_command)?);
        let event_sink = Arc::new(EventSink::with_authority(project.identity()));
        let client = connector
            .connect(event_sink.clone())
            .context("SSH の接続に失敗")?;
        // path 未指定（空）= 標準 SSH と同じく remote の $HOME をルートにする（#5・「ホスト選ぶ→home」）。
        let project = if project.path.as_os_str().is_empty() {
            let home = connector.home().context("remote の $HOME が分からない")?;
            SshProject {
                path: home,
                ..project.clone()
            }
        } else {
            project.clone()
        };
        Self::connect_client(
            client,
            project.identity(),
            project.destination(),
            &project.path,
            Some(project.clone()),
            Some(connector),
            event_sink,
        )
        .context("session proxy 接続 / protocol handshake に失敗")
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
        anyhow::ensure!(
            !project.path.as_os_str().is_empty(),
            "遅延接続には保存済みの root が要る（$HOME 解決は接続が必要）"
        );
        let connector = Arc::new(ssh_link::SshConnector::new(project, server_command)?);
        let event_sink = Arc::new(EventSink::with_authority(project.identity()));
        let client =
            ReconnectingClient::disconnected(connector.clone() as Arc<dyn Connector>, event_sink);
        Ok(Arc::new(Self {
            id: project.identity(),
            display_name: project.destination(),
            root: project.path.clone(),
            // 未接続なので project id はまだ無い。最初の request が `unknown project id` を
            // 受けて `OpenProject` し直す既存経路に乗る（daemon 再起動時と同じ扱い）。
            project_id: Arc::new(AtomicU64::new(0)),
            client,
            watch_control: Arc::new(Mutex::new(())),
            ssh_project: Some(project.clone()),
            connector: Some(connector),
            forwards: Arc::default(),
        }))
    }

    /// test/development 用。指定 process の stdio を同じ protocol として使う。
    pub fn connect_process(
        command: Command,
        id: String,
        display_name: String,
        root: &Path,
    ) -> Result<Arc<Self>> {
        let event_sink = Arc::new(EventSink::with_authority(id.clone()));
        let client = rpc_client_from_command(command, event_sink.clone())?;
        Self::connect_client(client, id, display_name, root, None, None, event_sink)
    }

    #[cfg(test)]
    fn connect_io(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        id: String,
        display_name: String,
        requested_root: &Path,
    ) -> Result<Arc<Self>> {
        let event_sink = Arc::new(EventSink::with_authority(id.clone()));
        let client = RpcClient::new(reader, writer, None, event_sink.clone());
        Self::connect_client(
            client,
            id,
            display_name,
            requested_root,
            None,
            None,
            event_sink,
        )
    }

    /// テスト用: 任意の connector（byte stream の対を返す）で繋ぎ直せる host。
    #[cfg(test)]
    fn connect_with(
        connector: Arc<dyn Connector>,
        id: String,
        requested_root: &Path,
    ) -> Result<Arc<Self>> {
        let event_sink = Arc::new(EventSink::with_authority(id.clone()));
        let client = ReconnectingClient::disconnected(connector, event_sink);
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
            id: id.clone(),
            display_name: id,
            root,
            project_id: Arc::new(AtomicU64::new(project_id)),
            client,
            watch_control: Arc::new(Mutex::new(())),
            ssh_project: None,
            connector: None,
            forwards: Arc::default(),
        }))
    }

    fn connect_client(
        client: Arc<RpcClient>,
        id: String,
        display_name: String,
        requested_root: &Path,
        ssh_project: Option<SshProject>,
        connector: Option<Arc<ssh_link::SshConnector>>,
        event_sink: Arc<EventSink>,
    ) -> Result<Arc<Self>> {
        let reconnector = connector
            .clone()
            .map(|connector| connector as Arc<dyn Connector>);
        let client = ReconnectingClient::new(client, reconnector, event_sink);
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
            project_id: Arc::new(AtomicU64::new(project_id)),
            client,
            watch_control: Arc::new(Mutex::new(())),
            ssh_project,
            connector,
            forwards: Arc::default(),
        }))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 障害注入テスト用: いまの接続（ssh）を落とす。SSH host のときだけ効く（local proxy は no-op）。
    /// 落とした後の request は [`ReconnectingClient`] が繋ぎ直して回復し、端末・プロセスは
    /// daemon 側で生き残って続きから流れるはず。
    #[doc(hidden)]
    pub fn debug_drop_connection(&self) {
        if let Some(connector) = &self.connector {
            connector.drop_connection();
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
        relative_to(&self.root, path)
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
            project_id: Arc::new(AtomicU64::new(project_id)),
            client: self.client.clone(),
            watch_control: self.watch_control.clone(),
            ssh_project: self.ssh_project.clone(),
            connector: self.connector.clone(),
            forwards: self.forwards.clone(),
        }))
    }

    fn scoped_request(
        &self,
        request: impl Fn(u64) -> Request,
        body: Vec<u8>,
        retry_safe: bool,
    ) -> Result<(Response, Vec<u8>)> {
        scoped_request(
            &self.client,
            &self.project_id,
            &self.root,
            request,
            body,
            retry_safe,
        )
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

    /// `retry_safe` = 再接続後に同じ request を再送してよいか（冪等な探索だけ true）。
    /// 非冪等（書き込みや副作用のある script）は「結果不明」のまま失敗として返す。
    fn run_command_with(&self, spec: &CommandSpec, retry_safe: bool) -> Result<CommandOutput> {
        let mut spec = spec.clone();
        spec.cwd = self.relative(&spec.cwd)?;
        let (response, body) = self.scoped_request(
            move |project_id| Request::RunCommand {
                project_id,
                spec: spec.clone(),
            },
            Vec::new(),
            retry_safe,
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
}

fn relative_to(root: &Path, path: &Path) -> Result<PathBuf> {
    if path == root {
        return Ok(PathBuf::new());
    }
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .with_context(|| format!("path is outside remote project: {}", path.display()))
}

/// project を指す request を送る。daemon が作り直されて project の番号を知らなければ、開き直して
/// 1 回だけ送り直す（最初の request は拒否済みなので重複しない）。
fn scoped_request(
    client: &ReconnectingClient,
    project_id: &AtomicU64,
    root: &Path,
    request: impl Fn(u64) -> Request,
    body: Vec<u8>,
    retry_safe: bool,
) -> Result<(Response, Vec<u8>)> {
    let send = |id: u64, body: Vec<u8>| {
        if retry_safe {
            client.request(&request(id), body)
        } else {
            client.request_nonretry(&request(id), body)
        }
    };
    match send(project_id.load(Ordering::Acquire), body.clone()) {
        Ok(response) => Ok(response),
        Err(error)
            if error
                .downcast_ref::<RemoteRequestError>()
                .is_some_and(|error| error.message.contains("unknown project id")) =>
        {
            let (opened, _) = client.request(
                &Request::OpenProject {
                    path: root.to_path_buf(),
                },
                Vec::new(),
            )?;
            let Response::ProjectOpened { project_id: id, .. } = opened else {
                bail!("remote server returned invalid reopen-project response");
            };
            project_id.store(id, Ordering::Release);
            send(id, body)
        }
        Err(error) => Err(error),
    }
}

/// 接続先で端末を開く（[`RemoteTerminal::open`] の中身）。
fn open_remote_terminal(
    client: &Arc<ReconnectingClient>,
    project_id: &AtomicU64,
    root: &Path,
    cwd: &Path,
    size: PtySize,
) -> Result<HostPty> {
    let stream = client.event_sink().streams.register(StreamKind::Terminal);
    let stream_id = stream.id();
    let opened = scoped_request(
        client,
        project_id,
        root,
        |project_id| Request::OpenTerminal {
            project_id,
            stream_id,
            cwd: cwd.to_path_buf(),
            size,
            env: HashMap::new(),
        },
        Vec::new(),
        false,
    );
    match opened {
        Ok((Response::Opened, _)) => {
            stream.mark_ready();
            let connection: Arc<dyn Send + Sync> = client.clone();
            Ok(HostPty::new(stream, connection))
        }
        Ok(_) => {
            stream.close();
            bail!("接続先が端末を開けなかった（不正な応答）")
        }
        Err(error) => {
            stream.close();
            Err(error).context("接続先で端末を開けない")
        }
    }
}

/// 手元の 127.0.0.1 で待ち受けて、来た接続を 1 本ずつ接続先の TCP へ中継する（ポート転送・O5）。
///
/// 待ち受けは手元にあるので、接続が切れて繋ぎ直しても張り直しは要らない（新しい接続は
/// いまの接続の上で開く）。以前は ControlMaster の `-O forward` を張り直していた。
#[derive(Default)]
struct PortForwards {
    entries: Mutex<Vec<PortForward>>,
}

struct PortForward {
    remote: u16,
    local: u16,
    stop: Arc<AtomicBool>,
}

impl PortForwards {
    fn forward(&self, client: &Arc<ReconnectingClient>, remote: u16) -> Result<u16> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow!("転送の一覧が壊れている"))?;
        if let Some(known) = entries.iter().find(|entry| entry.remote == remote) {
            return Ok(known.local);
        }
        // 同じ番号が空いていればそれ、塞がっていれば OS が選んだ空き番号。
        let listener = std::net::TcpListener::bind(("127.0.0.1", remote))
            .or_else(|_| std::net::TcpListener::bind(("127.0.0.1", 0)))
            .context("手元に空いているポートが無い")?;
        let local = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let accept_stop = stop.clone();
        let client = Arc::downgrade(client);
        thread::Builder::new()
            .name(format!("necoder-forward-{remote}"))
            .spawn(move || {
                for connection in listener.incoming() {
                    if accept_stop.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(connection) = connection else {
                        continue;
                    };
                    let Some(client) = client.upgrade() else {
                        break;
                    };
                    let spawned = thread::Builder::new()
                        .name(format!("necoder-forward-{remote}-connection"))
                        .spawn(move || {
                            if let Err(error) = relay_forwarded(&client, remote, connection) {
                                eprintln!(
                                    "Remote SSH: ポート {remote} の転送の 1 本が切れた: {error:#}"
                                );
                            }
                        });
                    if let Err(error) = spawned {
                        eprintln!("Remote SSH: 転送のスレッドを起動できない: {error}");
                    }
                }
            })
            .context("転送の待ち受けを起動できない")?;
        entries.push(PortForward {
            remote,
            local,
            stop,
        });
        Ok(local)
    }

    fn cancel(&self, remote: u16) -> Result<()> {
        let entry = {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| anyhow!("転送の一覧が壊れている"))?;
            let position = entries
                .iter()
                .position(|entry| entry.remote == remote)
                .ok_or_else(|| anyhow!("{remote} は転送していない"))?;
            entries.remove(position)
        };
        entry.stop_listening();
        Ok(())
    }

    fn list(&self) -> Vec<(u16, u16)> {
        self.entries
            .lock()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| (entry.remote, entry.local))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl PortForward {
    /// 待ち受けを止める（止まっている accept を、自分へ繋いで起こす）。中継中の接続はそのまま続く。
    fn stop_listening(&self) {
        self.stop.store(true, Ordering::Release);
        if let Err(error) = std::net::TcpStream::connect(("127.0.0.1", self.local)) {
            eprintln!("Remote SSH: 転送の待ち受けを止められない: {error}");
        }
    }
}

impl Drop for PortForwards {
    fn drop(&mut self) {
        if let Ok(entries) = self.entries.get_mut() {
            for entry in entries.drain(..) {
                entry.stop_listening();
            }
        }
    }
}

/// 転送の 1 本: 手元の接続 ↔ 接続先の TCP。片方が閉じたら、もう片方の書く側も閉じる。
fn relay_forwarded(
    client: &Arc<ReconnectingClient>,
    remote: u16,
    connection: std::net::TcpStream,
) -> Result<()> {
    let stream = client.event_sink().streams.register(StreamKind::Tcp);
    let stream_id = stream.id();
    let opened = client.request_nonretry(
        &Request::OpenTcp {
            stream_id,
            port: remote,
        },
        Vec::new(),
    );
    let handle = stream::TcpStreamHandle::new(stream.clone());
    match opened {
        Ok((Response::Opened, _)) => stream.mark_ready(),
        Ok(_) => bail!("接続先が TCP を開けなかった（不正な応答）"),
        Err(error) => return Err(error).context("接続先の TCP に繋げない"),
    }
    connection.set_nodelay(true)?;
    let mut upstream = handle.writer();
    let mut local_reader = connection.try_clone()?;
    let uploader = thread::Builder::new()
        .name(format!("necoder-forward-{remote}-up"))
        .spawn(move || {
            let _copied = std::io::copy(&mut local_reader, &mut upstream);
            // drop(upstream) で接続先へ EOF が届く。
        })?;
    let mut downstream = handle.reader();
    let mut local_writer = connection.try_clone()?;
    let _copied = std::io::copy(&mut downstream, &mut local_writer);
    let _closed = connection.shutdown(std::net::Shutdown::Write);
    if uploader.join().is_err() {
        eprintln!("Remote SSH: 転送の上りのスレッドが落ちた");
    }
    drop(handle);
    Ok(())
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

    fn forward_port(&self, remote_port: u16) -> Result<u16> {
        self.forwards.forward(&self.client, remote_port)
    }

    fn cancel_forward(&self, remote_port: u16) -> Result<()> {
        self.forwards.cancel(remote_port)
    }

    fn forwarded_ports(&self) -> Vec<(u16, u16)> {
        self.forwards.list()
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
        self.run_command_with(spec, false)
    }

    fn run_command_retry_safe(&self, spec: &CommandSpec) -> Result<CommandOutput> {
        self.run_command_with(spec, true)
    }

    /// daemon の 1 往復ではなくストリームの子として走らせる＝途中で止められ、出力も流れる。
    fn run_user_command(
        &self,
        spec: &CommandSpec,
        cancel: &AtomicBool,
        output_limit: usize,
        live: &LiveOutput,
    ) -> Result<UserCommandOutput> {
        run_user_command_over_process(self, spec, cancel, output_limit, live)
    }

    fn can_stop_user_command(&self) -> bool {
        true
    }

    /// 接続先の daemon にプロセスを起こさせ、stdio をストリームで繋ぐ。
    ///
    /// **待たずに返す**（UI スレッドから呼ばれる経路がある・接続を待つと固まる）。起動の request は
    /// 背景で送り、それまでに書いた stdin は溜めておいて、起きてから流す。起動に失敗したら stdout は
    /// すぐ閉じ、理由は stderr の末尾（[`HostProcess::stderr_tail`]）に出る。
    fn spawn_process(&self, spec: &CommandSpec) -> Result<HostProcess> {
        for key in spec.env.keys() {
            if !valid_env_key(key) {
                bail!("不正な environment key: {key}");
            }
        }
        let mut spec = spec.clone();
        spec.cwd = self.relative(&spec.cwd)?;
        let stream = self
            .client
            .event_sink()
            .streams
            .register(StreamKind::Process);
        let stream_id = stream.id();
        let client = self.client.clone();
        let project_id = self.project_id.clone();
        let root = self.root.clone();
        let opening = stream.clone();
        let program = spec.program.clone();
        thread::Builder::new()
            .name("necoder-remote-spawn".to_string())
            .spawn(move || {
                let opened = scoped_request(
                    &client,
                    &project_id,
                    &root,
                    |project_id| Request::Spawn {
                        project_id,
                        stream_id,
                        spec: spec.clone(),
                    },
                    Vec::new(),
                    false,
                );
                match opened {
                    Ok((Response::Opened, _)) => opening.mark_ready(),
                    Ok(_) => opening.fail(&format!("{program} を起動できない（不正な応答）")),
                    Err(error) => opening.fail(&format!("{program} を起動できない: {error:#}")),
                }
            })
            .context("remote process を起こすスレッドを起動できない")?;
        let connection: Arc<dyn Send + Sync> = self.client.clone();
        Ok(HostProcess {
            backend: ProcessBackend::Remote {
                stream: stream.clone(),
                _connection: connection,
            },
            stdin: Some(Box::new(StreamWriter::new(stream.clone()))),
            stdout: Some(Box::new(StreamReader::new(
                stream.clone(),
                stream::CHANNEL_OUTPUT,
            ))),
            stderr: StderrTail::capture(StreamReader::new(stream, stream::CHANNEL_ERROR)),
        })
    }

    /// 接続先の daemon が PTY を持つ端末。開くのは `TerminalView` が背景で行う
    /// （ここはネットワークに触らない＝UI スレッドから呼んでよい）。
    fn terminal_launch(&self, cwd: &Path) -> Result<Option<TerminalLaunch>> {
        let relative = self.relative(cwd)?;
        let client = self.client.clone();
        let project_id = self.project_id.clone();
        let root = self.root.clone();
        let key = format!("{}#{}", self.id, cwd.display());
        Ok(Some(TerminalLaunch::Remote(RemoteTerminal {
            opener: Arc::new(move |size| {
                open_remote_terminal(&client, &project_id, &root, &relative, size)
            }),
            label: self.display_name.clone(),
            key,
        })))
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

/// 接続先の sh へ二重引用符の中にそのまま埋め込める文字だけか（`$` `"` `` ` `` `\` などで
/// 意味が変わらない・remote-server のコマンド名やパス）。
fn paths_safe_for_shell(value: &str) -> bool {
    value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '/' | '+' | '-')
    })
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

    /// Node.js が同梱する Unix 用 npx と Windows 用 npx.cmd を取り違えない（#4）。
    #[cfg(windows)]
    #[test]
    fn path_lookup_skips_unix_wrappers_on_windows() {
        let directory = std::env::temp_dir().join(format!("necoder-path-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("npx"), "#!/usr/bin/env bash\n").unwrap();
        std::fs::write(directory.join("npx.cmd"), "@echo off\r\n").unwrap();
        let search_path = std::env::join_paths([&directory]).unwrap();
        assert_eq!(find_in_search_path("npx", &search_path), Some(directory.join("npx.cmd")));
        assert_eq!(find_in_search_path("npx.cmd", &search_path), Some(directory.join("npx.cmd")));
        std::fs::remove_file(directory.join("npx.cmd")).unwrap();
        assert_eq!(find_in_search_path("npx", &search_path), None);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// 設定の「インストール」等は Windows では PowerShell で流して残す（`/bin/sh` は無い・#69）。
    #[test]
    fn windows_command_terminal_runs_in_powershell_and_stays() {
        let args = windows_command_then_shell_args("npm i -g x");
        assert_eq!(
            args,
            ["-NoLogo", "-NoExit", "-ExecutionPolicy", "Bypass", "-Command", "npm i -g x"]
        );
        if cfg!(windows) {
            let (program, _) = command_then_shell("echo", None);
            assert!(program == "pwsh" || program == "powershell", "{program}");
        } else {
            assert_eq!(command_then_shell("echo", None).0, "/bin/sh", "mac は変えない");
        }
    }

    /// necoder の道具箱の PATH: Windows は Node.js の配布物の直下と npm の prefix の直下、mac / Linux は
    /// それぞれの `bin/`（Node.js の公式の配布物と `npm install -g --prefix` の置き方）。
    #[test]
    fn toolchain_dirs_follow_how_node_and_npm_lay_out_files() {
        let root = Path::new("toolchain");
        assert_eq!(
            toolchain_search_dirs_in(root, true),
            vec![root.join("node"), root.join("npm")]
        );
        assert_eq!(
            toolchain_search_dirs_in(root, false),
            vec![root.join("node").join("bin"), root.join("npm").join("bin")]
        );
    }

    /// 前後の一言: 成否で出し分け、引用符を含む文でも壊れない。**本物のシェルで流して確かめる**
    /// （Windows は PowerShell・それ以外は sh。`-NoExit` と `exec $SHELL` は付けない形で流す）。
    #[test]
    fn command_notes_tell_success_and_failure_in_a_real_shell() {
        let notes = CommandNotes {
            start: "▶ it's starting".to_string(),
            success: "✓ done".to_string(),
            failure: "✗ didn't work".to_string(),
        };
        let run = |command: &str| -> String {
            let output = if cfg!(windows) {
                child_process::command("powershell")
                    .args([
                        "-NoLogo",
                        "-NoProfile",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        &powershell_script_with_notes(command, &notes),
                    ])
                    .output()
            } else {
                child_process::command("/bin/sh")
                    .args(["-c", &sh_script_with_notes(command, &notes)])
                    .output()
            }
            .expect("シェルを起こせる");
            String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
        };
        let ok = run("echo hello");
        assert!(ok.contains("it's starting"), "{ok}");
        assert!(ok.contains("hello"), "{ok}");
        assert!(ok.contains("✓ done") || ok.contains("done"), "{ok}");
        assert!(!ok.contains("didn't work"), "{ok}");
        let failed = run(if cfg!(windows) { "cmd /c exit 3" } else { "false" });
        assert!(failed.contains("didn't work"), "{failed}");
        assert!(!failed.contains("✓ done"), "{failed}");
    }

    /// 起動後に入れたツールの PATH（レジストリ側）を、既存の並びを変えずに末尾へ足す（#69）。
    #[test]
    fn registry_search_dirs_are_appended_without_reordering() {
        let current = r"C:\dev\bin;C:\Windows\system32";
        let system = r"C:\WINDOWS\System32\;C:\Program Files\nodejs\;;C:\Users\a\.local\bin";
        assert_eq!(
            append_missing_search_dirs(current, system).as_deref(),
            Some(r"C:\dev\bin;C:\Windows\system32;C:\Program Files\nodejs\;C:\Users\a\.local\bin")
        );
        // 全部既に在る（大文字小文字・末尾 `\` 違い）なら書き換えない。
        assert_eq!(
            append_missing_search_dirs(r"C:\Program Files\nodejs;", r"c:\program files\NODEJS\"),
            None
        );
        assert_eq!(append_missing_search_dirs("", r"C:\a").as_deref(), Some(r"C:\a"));
    }

    /// 実機のレジストリから PATH が読めて、`%SystemRoot%` が展開されていること。
    #[cfg(windows)]
    #[test]
    fn registry_search_path_is_expanded() {
        let path = windows_registry_search_path();
        assert!(
            path.to_lowercase().contains(r"\system32"),
            "システム PATH が読めている: {path}"
        );
        assert!(!path.contains('%'), "REG_EXPAND_SZ は展開済み: {path}");
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
        let mut process = spawn_process_local(&spec)
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
            let Some(TerminalLaunch::Program(launch)) = launch else {
                panic!("Windows では既定シェルを明示する: {launch:?}");
            };
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
    use crate::stream_tests::stream_pair;

    /// 長寿命の子の stderr の末尾: すぐ落ちた子の最後の行と終わり方が取れる（読む側が追いつくまで
    /// 待てる）。stdout は ACP 用にそのまま残る。
    #[cfg(unix)]
    #[test]
    fn a_process_keeps_the_tail_of_its_stderr() {
        let spec = CommandSpec::new("sh", std::env::temp_dir()).args([
            "-c",
            "echo 'first line' >&2; printf '\\033[31mDEEPSEEK_API_KEY is not set\\033[0m\\n' >&2; exit 3",
        ]);
        let mut process = LocalHost.spawn_process(&spec).expect("起動できる");
        let tail = process.stderr_tail();
        assert!(
            tail.wait_closed(Duration::from_secs(5)),
            "子が閉じるまで待てる"
        );
        assert_eq!(
            tail.lines(),
            vec![
                "first line".to_string(),
                "DEEPSEEK_API_KEY is not set".to_string()
            ],
            "色のエスケープ列は落とす"
        );
        let mut status = None;
        for _ in 0..100 {
            status = process.try_exit_status();
            if status.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(status.and_then(|status| status.code()), Some(3));
    }

    /// 末尾は上限つき: 行数は最後の [`StderrTail::MAX_LINES`] 行・1 行は [`StderrTail::MAX_LINE_BYTES`]
    /// まで（改行の無い長い出力でもメモリを食わない）。制御文字は落とし、タブは空白にする。
    #[test]
    fn the_stderr_tail_is_bounded() {
        let mut text = String::new();
        for number in 0..(StderrTail::MAX_LINES + 10) {
            text.push_str(&format!("line {number}\n"));
        }
        text.push_str(&"x".repeat(StderrTail::MAX_LINE_BYTES * 3));
        let tail = StderrTail::capture(std::io::Cursor::new(text.into_bytes()));
        assert!(tail.wait_closed(Duration::from_secs(5)));
        let lines = tail.lines();
        assert_eq!(lines.len(), StderrTail::MAX_LINES);
        assert_eq!(lines[0], "line 11", "古い行から捨てる");
        let last = lines.last().expect("最後の行");
        assert_eq!(last.chars().count(), StderrTail::MAX_LINE_BYTES + 1);
        assert!(last.ends_with('…'), "切った印");
        assert_eq!(stderr_line(b"a\tb\x07c\r", false), "a bc");
        assert_eq!(
            stderr_line(b"\x1b]0;title\x07done", false),
            "done",
            "題名の OSC も落とす"
        );
    }

    /// 取り消せる command（local）: 取り消さなければ出力と終わり方を返す。待っている間に取り消すと
    /// 子を止めてすぐ戻る（最後まで走らせない）。始める前に取り消されていれば起動もしない。
    #[cfg(unix)]
    #[test]
    fn cancellable_command_stops_the_child_when_cancelled() {
        let cancel = Arc::new(AtomicBool::new(false));
        let short = CommandSpec::new("sh", std::env::temp_dir())
            .args(["-c", "printf abc; echo oops >&2; exit 3"]);
        let output = LocalHost
            .run_command_cancellable(&short, &cancel)
            .expect("起動できる")
            .expect("取り消していない");
        assert_eq!(output.stdout, b"abc");
        assert_eq!(output.stderr, b"oops\n");
        assert_eq!(output.status_code, Some(3));

        // 30 秒眠る子を 100ms 後に取り消す（`exec` = 止める相手が sleep そのもの）。
        let flag = cancel.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::Release);
        });
        let started = std::time::Instant::now();
        let sleeper = CommandSpec::new("sh", std::env::temp_dir()).args(["-c", "exec sleep 30"]);
        let output = LocalHost
            .run_command_cancellable(&sleeper, &cancel)
            .expect("起動できる");
        assert!(output.is_none(), "取り消した結果は返さない");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "子を止めてすぐ戻る: {:?}",
            started.elapsed()
        );
        canceller.join().expect("取り消し役のスレッド");

        // 取り消し済みなら起動しない（印のファイルが作られない）。
        let marker =
            std::env::temp_dir().join(format!("necoder_cancelled_command_{}", std::process::id()));
        let touch = CommandSpec::new("touch", std::env::temp_dir())
            .args([marker.to_string_lossy().to_string()]);
        let output = LocalHost
            .run_command_cancellable(&touch, &cancel)
            .expect("起動しないで戻る");
        assert!(output.is_none());
        assert!(!marker.exists(), "取り消した後の command は走らせない");
    }

    #[test]
    fn pid_marker_line_is_found_or_left_as_output() {
        let marked = format!("{USER_COMMAND_PID_MARKER}4242\nhello");
        assert_eq!(
            split_pid_marker(marked.as_bytes()),
            MarkerScan::Found(4242, b"hello".to_vec())
        );
        assert_eq!(
            split_pid_marker(&USER_COMMAND_PID_MARKER.as_bytes()[..5]),
            MarkerScan::Incomplete
        );
        assert_eq!(split_pid_marker(b"plain output\n"), MarkerScan::NotMarker);
    }

    /// SSH 先の実装（`spawn_process` の子として走らせる）を、手元の Host で通す。
    #[cfg(unix)]
    #[test]
    fn user_command_over_process_streams_merges_stderr_and_reports_exit() {
        let cancel = AtomicBool::new(false);
        let live = LiveOutput::default();
        let spec = CommandSpec::new("sh", std::env::temp_dir())
            .args(["-c", "printf abc; echo oops >&2; exit 3"]);
        let output = run_user_command_over_process(&LocalHost, &spec, &cancel, 1_000, &live)
            .expect("起動できる");
        assert_eq!(
            output.stdout.head, b"abcoops\n",
            "目印は外れ、stderr は stdout へ寄る"
        );
        assert!(output.stderr.head.is_empty());
        assert_eq!(output.status_code, Some(3));
        assert!(!output.cancelled);
        assert_eq!(live.tail(), b"abcoops\n");
    }

    /// 止めると、シェルが起こした子まで止まる（`npm run dev` のような長生きプロセスを残さない）。
    /// 走っている間の出力は `live` に届いている。
    #[cfg(unix)]
    #[test]
    fn user_command_over_process_stops_the_whole_tree() {
        let cancel = Arc::new(AtomicBool::new(false));
        let live = LiveOutput::default();
        let pid_file =
            std::env::temp_dir().join(format!("necoder-user-command-child-{}", std::process::id()));
        let script = format!(
            "echo started; sleep 37 & echo $! > '{}'; wait",
            pid_file.display()
        );
        let spec = CommandSpec::new("sh", std::env::temp_dir()).args(["-c", script.as_str()]);
        let stopper = {
            let cancel = cancel.clone();
            let live = live.clone();
            thread::spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while live.tail() != b"started\n" && std::time::Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                cancel.store(true, Ordering::Release);
            })
        };
        let output = run_user_command_over_process(&LocalHost, &spec, &cancel, 1_000, &live)
            .expect("起動できる");
        stopper.join().expect("止める側のスレッド");
        assert!(output.cancelled);
        assert_eq!(output.stdout.head, b"started\n");
        let child = std::fs::read_to_string(&pid_file).expect("子の PID が書かれている");
        std::fs::remove_file(&pid_file).expect("後始末");
        let alive = child_process::command("kill")
            .args(["-0", child.trim()])
            .stderr(Stdio::null())
            .status()
            .expect("kill -0 を実行できる")
            .success();
        assert!(!alive, "子プロセス {} が生き残っている", child.trim());
    }

    #[test]
    fn live_output_keeps_only_the_tail() {
        let live = LiveOutput::default();
        live.push(&vec![b'a'; LIVE_OUTPUT_BYTES]);
        live.push(b"end");
        assert_eq!(live.received(), LIVE_OUTPUT_BYTES as u64 + 3);
        let tail = live.tail();
        assert_eq!(tail.len(), LIVE_OUTPUT_BYTES);
        assert!(tail.ends_with(b"end"));
    }

    /// 人が打ったコマンド（エージェントパネルの `!`・#37）: 出力と終了コードをそのまま返す。
    /// 上限を超えた出力は頭と尻を残して間を捨てる（子は止めない＝最後まで読み切る）。
    #[cfg(unix)]
    #[test]
    fn user_command_returns_output_and_keeps_head_and_tail() {
        let cancel = AtomicBool::new(false);
        let short = CommandSpec::new("sh", std::env::temp_dir())
            .args(["-c", "printf abc; echo oops >&2; exit 3"]);
        let output = LocalHost
            .run_user_command(&short, &cancel, 1_000, &LiveOutput::default())
            .expect("起動できる");
        assert_eq!(output.stdout.head, b"abc");
        assert_eq!(output.stdout.omitted, 0);
        assert_eq!(output.stderr.head, b"oops\n");
        assert_eq!(output.status_code, Some(3));
        assert!(!output.cancelled);
        assert!(LocalHost.can_stop_user_command());

        let long = CommandSpec::new("sh", std::env::temp_dir()).args(["-c", "seq 1 100000"]);
        let output = LocalHost
            .run_user_command(&long, &cancel, 1_000, &LiveOutput::default())
            .expect("起動できる");
        assert_eq!(output.status_code, Some(0));
        assert_eq!(output.stdout.head.len(), 500);
        assert_eq!(output.stdout.tail.len(), 500);
        assert!(output.stdout.head.starts_with(b"1\n2\n3\n"));
        assert!(output.stdout.tail.ends_with(b"99999\n100000\n"));
        let total: u64 = (1..=100_000u64)
            .map(|n| n.to_string().len() as u64 + 1)
            .sum();
        assert_eq!(output.stdout.omitted, total - 1_000, "捨てた量を数える");

        // remote（既定の実装）も同じ上限で丸める。
        let captured = CapturedOutput::from_bytes(b"0123456789", 4);
        assert_eq!(
            (captured.head, captured.tail, captured.omitted),
            (b"01".to_vec(), b"89".to_vec(), 6)
        );
        let captured = CapturedOutput::from_bytes(b"0123", 4);
        assert_eq!(
            (captured.head, captured.tail, captured.omitted),
            (b"0123".to_vec(), Vec::new(), 0)
        );
    }

    /// 止めるとプロセスグループごと止まる: `&` で起こした孫（`sleep`）も残らない。止めるまでの
    /// 出力は返す。始める前に取り消されていれば走らせない。
    #[cfg(unix)]
    #[test]
    fn stopping_a_user_command_takes_down_its_process_group() {
        let scratch =
            std::env::temp_dir().join(format!("necoder_user_command_{}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("一時ディレクトリを作れる");
        let pid_file = scratch.join("sleeper.pid");
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let watched = pid_file.clone();
        let canceller = thread::spawn(move || {
            // 孫が pid を書き出してから止める（書く前に止めると確かめる相手が居ない）。
            for _ in 0..500 {
                if std::fs::read_to_string(&watched).is_ok_and(|pid| pid.ends_with('\n')) {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            flag.store(true, Ordering::Release);
        });
        let spec = CommandSpec::new("sh", &scratch).args([
            "-c",
            "echo started; sleep 30 & echo $! > sleeper.pid; wait; echo never",
        ]);
        let started = std::time::Instant::now();
        let output = LocalHost
            .run_user_command(&spec, &cancel, 1_000, &LiveOutput::default())
            .expect("起動できる");
        canceller.join().expect("取り消し役のスレッド");
        assert!(output.cancelled);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "止めたらすぐ戻る: {:?}",
            started.elapsed()
        );
        assert_eq!(output.stdout.head, b"started\n", "止めるまでの出力は返す");
        let pid: libc::pid_t = std::fs::read_to_string(&pid_file)
            .expect("孫の pid")
            .trim()
            .parse()
            .expect("pid は数");
        // 孫はグループへのシグナルで止まる（親に回収されるまで一瞬ゾンビで残るので少し待つ）。
        let alive = |pid: libc::pid_t| {
            // SAFETY: シグナル 0 は送らずに存在だけを確かめる。
            unsafe { libc::kill(pid, 0) == 0 }
        };
        let mut gone = false;
        for _ in 0..200 {
            if !alive(pid) {
                gone = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !gone {
            // 後始末（失敗の時も孫を残さない）。
            // SAFETY: 自分が起こした孫の pid。
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        assert!(gone, "孫の sleep（pid {pid}）がグループごと止まる");

        // SIGTERM を無視するコマンドも、猶予の後の SIGKILL で止まる（無視は孫の sleep にも継がれる）。
        let stubborn = CommandSpec::new("sh", &scratch)
            .args(["-c", "trap '' TERM; echo started; sleep 30; echo never"]);
        let cancel_stubborn = Arc::new(AtomicBool::new(false));
        let flag = cancel_stubborn.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            flag.store(true, Ordering::Release);
        });
        let started = std::time::Instant::now();
        let output = LocalHost
            .run_user_command(&stubborn, &cancel_stubborn, 1_000, &LiveOutput::default())
            .expect("起動できる");
        canceller.join().expect("取り消し役のスレッド");
        assert!(output.cancelled);
        assert!(
            started.elapsed() >= USER_COMMAND_STOP_GRACE
                && started.elapsed() < USER_COMMAND_STOP_GRACE + Duration::from_secs(5),
            "SIGTERM を無視されたら猶予の後に SIGKILL: {:?}",
            started.elapsed()
        );
        assert_eq!(output.stdout.head, b"started\n");

        // 取り消し済みなら起動しない（印のファイルが作られない）。
        let marker = scratch.join("never-started");
        let touch =
            CommandSpec::new("touch", &scratch).args([marker.to_string_lossy().to_string()]);
        let output = LocalHost
            .run_user_command(&touch, &cancel, 1_000, &LiveOutput::default())
            .expect("起動しないで戻る");
        assert!(output.cancelled && output.status_code.is_none());
        assert!(!marker.exists(), "取り消した後の command は走らせない");
        std::fs::remove_dir_all(&scratch).ok();
    }

    /// 背景の executor のスレッド（macOS の GCD）はシグナルをブロックしていて、Rust の `Command` は
    /// そのマスクを子へ継ぐ。それでも子は既定の状態で走る＝SIGTERM で猶予（SIGKILL）を待たずに止まる。
    /// 実害: 継いだままだと SIGTERM が届かず、止めるたびに 2 秒待ち、終了時の SIGTERM では残っていた。
    #[cfg(unix)]
    #[test]
    fn a_user_command_does_not_inherit_the_callers_blocked_signals() {
        let runner = thread::spawn(|| {
            // GCD のワーカーと同じく、呼ぶスレッドで SIGTERM などをブロックする。
            // SAFETY: このスレッドのシグナルマスクを変えるだけ。`blocked` は sigemptyset が初期化する。
            unsafe {
                let mut blocked = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
                libc::sigemptyset(blocked.as_mut_ptr());
                for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
                    libc::sigaddset(blocked.as_mut_ptr(), signal);
                }
                libc::pthread_sigmask(libc::SIG_BLOCK, blocked.as_ptr(), std::ptr::null_mut());
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let flag = cancel.clone();
            let canceller = thread::spawn(move || {
                thread::sleep(Duration::from_millis(200));
                flag.store(true, Ordering::Release);
            });
            let spec = CommandSpec::new("sh", std::env::temp_dir())
                .args(["-c", "echo started; sleep 30; echo never"]);
            let started = std::time::Instant::now();
            let output = LocalHost.run_user_command(&spec, &cancel, 1_000, &LiveOutput::default());
            let elapsed = started.elapsed();
            canceller.join().expect("取り消し役のスレッド");
            (output, elapsed)
        });
        let (output, elapsed) = runner.join().expect("呼ぶスレッド");
        let output = output.expect("起動できる");
        assert!(output.cancelled);
        assert_eq!(output.stdout.head, b"started\n");
        assert!(
            elapsed < Duration::from_millis(1_500),
            "SIGTERM で止まる（SIGKILL の猶予を待たない）: {elapsed:?}"
        );
    }

    /// `&` で背景に回した孫がパイプを握っていても、子が終われば待ちすぎずに返す（孫は止めない＝
    /// 端末で `&` を付けた時と同じ）。
    #[cfg(unix)]
    #[test]
    fn a_user_command_returns_even_if_a_background_job_holds_the_pipe() {
        let scratch =
            std::env::temp_dir().join(format!("necoder_user_command_bg_{}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("一時ディレクトリを作れる");
        let cancel = AtomicBool::new(false);
        let spec = CommandSpec::new("sh", &scratch)
            .args(["-c", "sleep 30 & echo $! > sleeper.pid; echo done"]);
        let started = std::time::Instant::now();
        let output = LocalHost
            .run_user_command(&spec, &cancel, 1_000, &LiveOutput::default())
            .expect("起動できる");
        let pid: Option<libc::pid_t> = std::fs::read_to_string(scratch.join("sleeper.pid"))
            .ok()
            .and_then(|pid| pid.trim().parse().ok());
        if let Some(pid) = pid {
            // 後始末: 背景に回った孫は自分で止める。
            // SAFETY: 自分が起こした孫の pid。
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "孫を待たずに返す: {:?}",
            started.elapsed()
        );
        assert!(!output.cancelled);
        assert_eq!(output.status_code, Some(0));
        assert_eq!(output.stdout.head, b"done\n");
        assert!(pid.is_some(), "孫の pid が書かれている");
        std::fs::remove_dir_all(&scratch).ok();
    }

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
        let sink = EventSink::default();
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
        sink.dispatch(vec![PathBuf::from("new.txt")]);
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

    /// O37（G01）: 接続先の登録は config の末尾に足すだけ。同じ名前・書けない値は断り、元の行は触らない。
    #[test]
    fn registering_a_host_appends_a_block() {
        let dir = std::env::temp_dir().join(format!("necoder-ssh-register-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(".ssh/config");
        let host = NewSshHost {
            alias: "devbox".into(),
            hostname: "192.168.1.20".into(),
            user: Some("me".into()),
            port: Some(2222),
            identity_file: Some("~/.ssh/My Keys/id_ed25519".into()),
            proxy_jump: Some("admin@bastion.example.com:2200,inner".into()),
        };
        append_ssh_config_host(&path, &host).expect("無ければ作って書く");
        let written = std::fs::read_to_string(&path).expect("読める");
        assert!(written.contains("Host devbox\n  HostName 192.168.1.20\n  User me\n  Port 2222\n  ProxyJump admin@bastion.example.com:2200,inner\n"));
        assert!(
            written.contains("IdentityFile \"~/.ssh/My Keys/id_ed25519\""),
            "{written}"
        );
        let hosts = parse_ssh_config(&written);
        assert_eq!(hosts[0].alias, "devbox");
        assert_eq!(hosts[0].user.as_deref(), Some("me"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "config は本人だけ");
        }

        assert!(
            append_ssh_config_host(&path, &host).is_err(),
            "同じ名前は断る"
        );
        std::fs::write(&path, "Host old\n  HostName old.example.com").unwrap();
        let other = NewSshHost {
            alias: "other".into(),
            hostname: "other.example.com".into(),
            ..NewSshHost::default()
        };
        append_ssh_config_host(&path, &other).expect("足せる");
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.starts_with("Host old\n  HostName old.example.com\n\n# necoder"),
            "元の行は触らず、改行を補って足す: {written}"
        );
        for bad in [
            NewSshHost {
                alias: "two words".into(),
                hostname: "h".into(),
                ..NewSshHost::default()
            },
            NewSshHost {
                alias: "web*".into(),
                hostname: "h".into(),
                ..NewSshHost::default()
            },
            NewSshHost {
                alias: "ok".into(),
                hostname: "-oProxyCommand=x".into(),
                ..NewSshHost::default()
            },
            NewSshHost {
                alias: "ok".into(),
                hostname: "h".into(),
                port: Some(0),
                ..NewSshHost::default()
            },
            NewSshHost {
                alias: "ok".into(),
                hostname: "h".into(),
                proxy_jump: Some("gw -oProxyCommand=x".into()),
                ..NewSshHost::default()
            },
            NewSshHost {
                alias: "ok".into(),
                hostname: "h".into(),
                proxy_jump: Some("bastion,-oProxyCommand=x".into()),
                ..NewSshHost::default()
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        // 経由先は全部を確かめる（2 つ目以降の `-` や `;` も断る）。
        for bad in [
            "bastion,-oProxyCommand=x",
            "bastion;touch x",
            "bastion,",
            "admin@-x",
            "ssh://-oProxyCommand=x",
            "gw$(id)",
        ] {
            assert!(!proxy_jump_value(bad), "{bad}");
        }
        for good in [
            "admin@bastion.example.com:2200,inner",
            "ssh://me@[2001:db8::1]:2222",
            "jump_host-1",
            "none",
        ] {
            assert!(proxy_jump_value(good), "{good}");
        }
        let _ = std::fs::remove_dir_all(&dir);
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
    fn openssh_failures_are_told_apart() {
        let changed = "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
            @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n\
            Host key verification failed.";
        assert_eq!(
            SshFailure::classify(changed),
            Some(SshFailure::HostKeyChanged)
        );
        for (log, expected) in [
            ("Host key verification failed.", SshFailure::HostKeyUnknown),
            (
                "No ED25519 host key is known for devbox and you have requested strict checking.",
                SshFailure::HostKeyUnknown,
            ),
            (
                "user@devbox: Permission denied (publickey,password).",
                SshFailure::Authentication,
            ),
            (
                "Received disconnect from 10.0.0.2 port 22:2: Too many authentication failures\n\
                 Permission denied (publickey).",
                SshFailure::TooManyKeys,
            ),
            (
                "@         WARNING: UNPROTECTED PRIVATE KEY FILE!          @\n\
                 Permission denied (publickey).",
                SshFailure::BadPermissions,
            ),
            (
                "Bad owner or permissions on /Users/me/.ssh/config",
                SshFailure::BadPermissions,
            ),
            (
                "ssh: Could not resolve hostname devbx: nodename nor servname provided, or not known",
                SshFailure::UnknownHost,
            ),
            (
                "ssh: connect to host devbox port 2222: Connection refused",
                SshFailure::Refused,
            ),
            (
                "ssh: connect to host 10.0.0.9 port 22: Operation timed out",
                SshFailure::TimedOut,
            ),
            (
                "Unable to negotiate with 10.0.0.2 port 22: no matching host key type found. Their offer: ssh-rsa",
                SshFailure::Negotiation,
            ),
        ] {
            assert_eq!(SshFailure::classify(log), Some(expected), "{log}");
        }
        assert_eq!(
            SshFailure::classify("kex_exchange_identification: read: Connection reset"),
            None
        );
    }

    #[test]
    fn a_failed_connection_carries_what_openssh_wrote() {
        let error = anyhow::Error::new(SshConnectError::new(
            "exit status: 255",
            "\r\nuser@devbox: Permission denied (publickey).\r\n\n",
        ))
        .context("SSH の接続に失敗");
        let connect = error
            .downcast_ref::<SshConnectError>()
            .expect("文脈を足しても取り出せる");
        assert_eq!(connect.failure, Some(SshFailure::Authentication));
        assert_eq!(
            connect.reason,
            "user@devbox: Permission denied (publickey)."
        );
        assert_eq!(
            format!("{error:#}"),
            "SSH の接続に失敗: OpenSSH の接続に失敗: exit status: 255: \
             user@devbox: Permission denied (publickey)."
        );
        let silent = SshConnectError::new("exit status: 255", "");
        assert_eq!(
            silent.to_string(),
            "OpenSSH の接続に失敗: exit status: 255",
            "書かれていなければ終わり方だけ"
        );
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

        // byte stream の対は SSH stdio と同じ全二重。sandbox 内でも実 protocol を通せる。
        let (client_stream, server_stream) = stream_pair().unwrap();
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
        assert!(child_process::command("mkfifo")
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

    /// byte stream の対（loopback TCP）で実 protocol を喋る connector。ssh 無しで [`ReconnectingClient`] の
    /// 再接続ロジック（見限り・張り直し・透過再送）を検証するための差し替え。
    struct PairConnector {
        connections: AtomicU64,
    }

    impl Connector for PairConnector {
        fn connect(&self, event_sink: Arc<EventSink>) -> Result<Arc<RpcClient>> {
            self.connections.fetch_add(1, Ordering::Relaxed);
            let (client_stream, server_stream) = stream_pair()?;
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
    fn silent_client(event_sink: Arc<EventSink>) -> (Arc<RpcClient>, std::net::TcpStream) {
        let (client_stream, server_stream) = stream_pair().unwrap();
        let reader = Box::new(client_stream.try_clone().unwrap());
        (
            RpcClient::new(reader, Box::new(client_stream), None, event_sink),
            server_stream,
        )
    }

    /// 到達不能ホストの再現: 接続を張ろうとすると必ず失敗する connector。
    struct UnreachableConnector;

    impl Connector for UnreachableConnector {
        fn connect(&self, _event_sink: Arc<EventSink>) -> Result<Arc<RpcClient>> {
            bail!("host unreachable (test)")
        }
    }

    /// 接続状態は「張り直しの前後」で Connecting → Connected と遷移し、購読者に変化だけが届く。
    /// statusbar の SSH チップはこの遷移を見て色と再接続ボタンを出す（2026-09-08）。
    #[test]
    fn connection_state_follows_reconnect() {
        let sink = Arc::new(EventSink::default());
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
    #[test]
    fn connection_state_reports_disconnected_when_reconnect_fails() {
        let sink = Arc::new(EventSink::default());
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
            Arc::new(EventSink::default()),
        );
        assert_eq!(lazy.state(), ConnectionState::Unconnected);
    }

    #[test]
    fn request_timeout_is_typed_and_distinguishable_from_close() {
        let sink = Arc::new(EventSink::default());
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

    #[test]
    fn reconnect_fails_requests_waiting_on_the_dead_connection_fast() {
        let scratch_root = scratch("reconnect");
        let sink = Arc::new(EventSink::default());
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
    struct FailingConnector {
        attempts: Arc<AtomicU64>,
        delay: Duration,
    }

    impl Connector for FailingConnector {
        fn connect(&self, _event_sink: Arc<EventSink>) -> Result<Arc<RpcClient>> {
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
    #[test]
    fn queued_requests_do_not_each_pay_for_their_own_failed_reconnect() {
        let sink = Arc::new(EventSink::default());
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

    #[test]
    fn heartbeat_keeps_a_slow_but_flowing_connection() {
        let sink = Arc::new(EventSink::default());
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

/// Task の環境ファイルは shell として評価しない。値の `=` と空白はそのまま子へ渡す。
pub fn task_environment(host: &dyn Host, root: &Path) -> Result<HashMap<String, String>> {
    let path = root.join(".necoder/task.env");
    if host.metadata(&path).is_err() { return Ok(HashMap::new()); }
    let content = host.read_file(&path)?;
    let text = std::str::from_utf8(&content.bytes)?;
    let mut environment = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let (key, value) = line.split_once('=').with_context(|| format!("task.env:{}: KEY=VALUE が必要です", index + 1))?;
        let valid = !key.is_empty() && key.chars().enumerate().all(|(index, c)| c == '_' || c.is_ascii_alphabetic() || (index > 0 && c.is_ascii_digit()));
        anyhow::ensure!(valid, "task.env:{}: 無効なキー", index + 1);
        environment.insert(key.to_string(), value.to_string());
    }
    Ok(environment)
}

#[cfg(test)]
mod task_environment_tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("necoder_task_env_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".necoder")).expect("一時ディレクトリを作れない");
        root
    }

    #[test]
    fn task_env_is_key_value_lines_not_shell() {
        let root = temp_root("parse");
        std::fs::write(
            root.join(".necoder/task.env"),
            "# comment\n\nCARGO_TARGET_DIR=/main/target\nURL=http://x?a=1=2\n",
        )
        .unwrap();
        let environment = task_environment(LocalHost::shared().as_ref(), &root).unwrap();
        assert_eq!(environment["CARGO_TARGET_DIR"], "/main/target");
        // 最初の '=' だけが区切り。値の '=' はそのまま子へ渡す（shell 評価しない）。
        assert_eq!(environment["URL"], "http://x?a=1=2");
        assert_eq!(environment.len(), 2);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_task_env_is_empty_and_bad_lines_are_errors() {
        let root = temp_root("missing");
        assert!(task_environment(LocalHost::shared().as_ref(), &root).unwrap().is_empty());
        std::fs::write(root.join(".necoder/task.env"), "1BAD=x\n").unwrap();
        assert!(task_environment(LocalHost::shared().as_ref(), &root).is_err(), "数字始まりのキーは拒否");
        std::fs::write(root.join(".necoder/task.env"), "NOEQUALS\n").unwrap();
        assert!(task_environment(LocalHost::shared().as_ref(), &root).is_err(), "KEY=VALUE でない行は拒否");
        std::fs::remove_dir_all(&root).unwrap();
    }

}
