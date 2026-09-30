//! shell — composer の先頭の `!` で、人が打ったシェルコマンドを走らせる（シェルモード・#37）。
//!
//! Claude Code の CLI の bash モードと同じ所作: `!git status` はエージェントへ送らず、そのスレッドの
//! 作業ディレクトリで走らせ（手元は `$SHELL -c`・SSH 先は Host の `sh -c`）、transcript に
//! `! <コマンド>` と `⎿ 出力` の行を出す。モデルのターンではないのでトークンは増えない。結果は
//! **次のプロンプト**に `<bash-input>` / `<bash-stdout>` / `<bash-stderr>` で包んで添え、添えたら
//! 渡し済みにする（composer のチップで「添える予定」を見せ、× で外せる）。
//!
//! **解釈するのは composer から人が送った時だけ**（[`AgentPanel::submit`] の入口）。スマホ（リモート
//! 管制）・`ne` / MCP（Captain を含む）・Fleet の注記・キュー / steer・引き継ぎの前置きはどれも
//! `send_prompt_*` を通るので `!` は文のまま届く — AI や遠隔から承認なしにコマンドが走る穴を作らない。

use super::*;
use host::{CapturedOutput, CommandSpec, LiveOutput, UserCommandOutput};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// transcript と次のプロンプトに載せる出力の上限（stdout と stderr の合計・文字）。Claude Code の
/// bash の既定（30,000 字）に合わせる。超えた分は頭と尻を残して間を省き、省いたことを本文に書く。
const SHELL_OUTPUT_MAX_CHARS: usize = 30_000;

/// Host が取り込む上限（stream ごと・バイト）。頭と尻の半分ずつが、4 バイトの文字でも
/// [`SHELL_OUTPUT_MAX_CHARS`] の半分を覆う大きさ（画面に載せる分は Host が必ず持っている）。
const SHELL_CAPTURE_BYTES: usize = 128 * 1024;

/// 走っている間の出力を `!` の行へ描き直す間隔（届いた分があった時だけ描く）。
const SHELL_LIVE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// 走っている間に見せる出力の末尾の行数（全部は終わってから・畳みの規則も終わってから）。
const SHELL_LIVE_TAIL_LINES: usize = 8;

/// 止めた時に runner が SIGTERM を送るまでの猶予（終了の時だけ待つ・[`AgentPanel::new`]）。
/// runner は 20ms ごとに印を見るので、ふつうはこの間に届く。
pub(crate) const SHELL_STOP_ON_QUIT_WAIT: std::time::Duration =
    std::time::Duration::from_millis(60);

/// composer の本文がシェルのコマンドか（`!` の後ろ・前後の空白を除いた物を返す）。
///
/// 1 文字目が `!` の時だけ（` !foo` のように空白で始まる入力は今までどおり文として送る）。
/// `!` の後ろが空（`!` だけ・空白だけ）ならコマンドではない。
pub(crate) fn shell_command_input(text: &str) -> Option<&str> {
    let command = text.strip_prefix('!')?.trim();
    (!command.is_empty()).then_some(command)
}

/// このホストで人のコマンドを流す spec。手元はユーザーのシェル（`$SHELL -c`・無ければ `sh -c`）、
/// SSH 先は Host の既存の口（`sh -c`）。Windows の手元は Host の口（`cmd.exe /C`）に任せる
/// （`$SHELL` が MSYS のパスを指していても起動できないため）。
fn shell_command_spec(host: &dyn Host, command: &str, cwd: &Path) -> CommandSpec {
    if !host.is_remote() && !cfg!(windows) {
        if let Some(shell) = std::env::var_os("SHELL").filter(|shell| !shell.is_empty()) {
            return CommandSpec::new(shell.to_string_lossy(), cwd).args(["-c", command]);
        }
    }
    host.shell_script(command, cwd)
}

/// ローカルのパスをホーム起点（`~/…`）で短く見せる。
fn tilde_path(path: &Path) -> String {
    match paths::home_dir().and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(relative) if relative.as_os_str().is_empty() => "~".to_string(),
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    }
}

/// 走らせた順の通し番号（背景の結果を正しい行へ届ける鍵。行の添字は履歴の再生でずれる）。
fn next_shell_run_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// シェルの行の状態。**色相は使わない**（UI-SPEC §1.3）: 状態は文字で出す。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ShellStatus {
    /// 走っている。`stopping` = 止める印を立てて、終わるのを待っている（手元）。
    Running { stopping: bool },
    /// 終わった（`None` = シグナルで終わった）。
    Exited(Option<i32>),
    /// 止めた・走っている途中で necoder が終わった。`detached` = SSH 先で止められず、向こうでは
    /// 最後まで動く。
    Interrupted { detached: bool },
    /// 起動できなかった。
    Failed(SharedString),
}

/// `!` で走らせたコマンド 1 回（transcript の `Entry::Shell`）。閉じたスレッドの退避（`ClosedThread`）で
/// 一時ファイルへ書くので serde を持つ。退避は走り終えてからなので、止める印は書かない。
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct ShellRun {
    pub(crate) id: u64,
    pub(crate) command: SharedString,
    /// 丸めた stdout / stderr（端末の制御文字を除き、上限を超えた分は間を省いた物）。
    pub(crate) stdout: SharedString,
    pub(crate) stderr: SharedString,
    /// 画面に出す出力（stdout の後に stderr・端末と同じく 1 本に並べる）。描画のたびに組み直さない。
    pub(crate) output: SharedString,
    pub(crate) status: ShellStatus,
    /// 次のプロンプトに添える予定（添えた・× で外した・復元した物は `false`）。保存しない
    /// （再起動を越えては添えない＝引き継ぎの前置きと同じ）。
    pub(crate) pending: bool,
    /// 止める印（走っている間だけ）。
    #[serde(skip)]
    cancel: Option<Arc<AtomicBool>>,
    /// 走っている間に届いた出力の末尾（走っている間だけ・描画が覗く）。
    #[serde(skip)]
    live: Option<LiveOutput>,
    /// 走っている途中で止められるか（手元だけ）。できなければ止めた時に待たずに畳む。
    can_stop: bool,
    /// 走っている途中で保存した行の id（終わったら同じ行を書き換える）。
    stored_turn: Option<i64>,
}

impl ShellRun {
    fn running(
        command: &str,
        cancel: Arc<AtomicBool>,
        live: LiveOutput,
        can_stop: bool,
    ) -> ShellRun {
        ShellRun {
            id: next_shell_run_id(),
            command: SharedString::from(command.to_string()),
            stdout: SharedString::default(),
            stderr: SharedString::default(),
            output: SharedString::default(),
            status: ShellStatus::Running { stopping: false },
            pending: true,
            cancel: Some(cancel),
            live: Some(live),
            can_stop,
            stored_turn: None,
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(self.status, ShellStatus::Running { .. })
    }

    /// 走っている間に届いた出力の末尾 [`SHELL_LIVE_TAIL_LINES`] 行（端末の制御文字を除いた物）。
    fn live_tail(&self) -> Option<String> {
        let live = self.live.as_ref()?;
        let text = normalize_terminal_output(&String::from_utf8_lossy(&live.tail()));
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let tail = lines[lines.len().saturating_sub(SHELL_LIVE_TAIL_LINES)..].join("\n");
        (!tail.is_empty()).then_some(tail)
    }

    /// 止める（esc / ■）。走っていれば `true`。手元は印を立てて終わるのを待つ（止めるまでの出力が
    /// 届く）。SSH 先は途中で止める口が無いので、待たずに「中断」として畳む（向こうでは最後まで動く）。
    fn request_stop(&mut self) -> bool {
        let ShellStatus::Running { stopping } = self.status else {
            return false;
        };
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Release);
        }
        if !self.can_stop {
            self.status = ShellStatus::Interrupted { detached: true };
            self.cancel = None;
            self.live = None;
        } else if !stopping {
            self.status = ShellStatus::Running { stopping: true };
        }
        true
    }

    /// 背景の結果を反映する。
    fn finish(&mut self, result: anyhow::Result<UserCommandOutput>) {
        self.cancel = None;
        self.live = None;
        match result {
            Ok(output) => {
                let (stdout, stderr) = shell_output_texts(&output.stdout, &output.stderr);
                self.set_output(SharedString::from(stdout), SharedString::from(stderr));
                self.status = if output.cancelled {
                    ShellStatus::Interrupted { detached: false }
                } else {
                    ShellStatus::Exited(output.status_code)
                };
            }
            Err(error) => {
                self.status = ShellStatus::Failed(SharedString::from(format!("{error:#}")));
            }
        }
    }

    /// 保存した行の id を控える（走っている途中で保存した時だけ＝終わったら書き換える）。
    pub(crate) fn note_stored(&mut self, turn_id: i64) {
        if self.is_running() {
            self.stored_turn = Some(turn_id);
        }
    }

    /// 丸めた stdout / stderr を持ち、画面に出す 1 本（[`Self::output`]）も組んでおく。
    fn set_output(&mut self, stdout: SharedString, stderr: SharedString) {
        self.output = match (stdout.is_empty(), stderr.is_empty()) {
            (false, false) => SharedString::from(format!("{stdout}\n{stderr}")),
            (false, true) => stdout.clone(),
            (true, false) => stderr.clone(),
            (true, true) => SharedString::default(),
        };
        self.stdout = stdout;
        self.stderr = stderr;
    }

    /// 状態の一言（成功・走っている間は無し）。**中立の文字**で出す（色相を使わない）。
    pub(crate) fn status_label(&self) -> Option<String> {
        match &self.status {
            ShellStatus::Running { .. } | ShellStatus::Exited(Some(0)) => None,
            ShellStatus::Exited(Some(code)) => {
                Some(i18n::t!("agent.shell_exit_code", "code" => code))
            }
            ShellStatus::Exited(None) => Some(i18n::t!("agent.shell_signal")),
            ShellStatus::Interrupted { detached: false } => {
                Some(i18n::t!("agent.shell_interrupted"))
            }
            ShellStatus::Interrupted { detached: true } => {
                Some(i18n::t!("agent.shell_interrupted_remote"))
            }
            ShellStatus::Failed(message) => {
                Some(i18n::t!("agent.shell_failed", "message" => message.as_ref()))
            }
        }
    }

    /// コピー・transcript 内検索の本文。
    pub(crate) fn plain_text(&self) -> String {
        let mut text = format!("! {}", self.command);
        if !self.output.is_empty() {
            text.push('\n');
            text.push_str(&self.output);
        }
        if let Some(label) = self.status_label() {
            text.push('\n');
            text.push_str(&label);
        }
        text
    }

    /// DB の turn の中身（role `shell`・JSON 1 つ）。
    pub(crate) fn stored_content(&self) -> String {
        let (status, code, message) = match &self.status {
            ShellStatus::Running { .. } => ("running", None, None),
            ShellStatus::Exited(code) => ("exited", *code, None),
            ShellStatus::Interrupted { detached: false } => ("interrupted", None, None),
            ShellStatus::Interrupted { detached: true } => ("detached", None, None),
            ShellStatus::Failed(message) => ("failed", None, Some(message.to_string())),
        };
        serde_json::json!({
            "command": self.command.as_ref(),
            "stdout": self.stdout.as_ref(),
            "stderr": self.stderr.as_ref(),
            "status": status,
            "code": code,
            "message": message,
        })
        .to_string()
    }

    /// DB の turn から復元する。**走っている途中で保存した物は「中断」**として読む（necoder を
    /// 終了した・落ちた時の行）。読めなければ `None`。
    pub(crate) fn from_stored(content: &str) -> Option<ShellRun> {
        let value: serde_json::Value = serde_json::from_str(content).ok()?;
        let text = |key: &str| SharedString::from(value[key].as_str().unwrap_or("").to_string());
        let status = match value["status"].as_str()? {
            "exited" => ShellStatus::Exited(
                value["code"]
                    .as_i64()
                    .and_then(|code| i32::try_from(code).ok()),
            ),
            "failed" => ShellStatus::Failed(text("message")),
            "detached" => ShellStatus::Interrupted { detached: true },
            _ => ShellStatus::Interrupted { detached: false },
        };
        let mut run = ShellRun {
            id: next_shell_run_id(),
            command: SharedString::from(value["command"].as_str()?.to_string()),
            stdout: SharedString::default(),
            stderr: SharedString::default(),
            output: SharedString::default(),
            status,
            pending: false,
            cancel: None,
            live: None,
            can_stop: false,
            stored_turn: None,
        };
        run.set_output(text("stdout"), text("stderr"));
        Some(run)
    }
}

/// 次のプロンプトに添える物（終わっていて、まだ添えていない行）。無ければ `None`。
/// 戻り値は（添える本文, 添えた行の id）。本文は `context_prefix` の末尾に入る（改行で終わる）。
pub(crate) fn pending_shell_attachment(entries: &[Entry]) -> Option<(String, Vec<u64>)> {
    let runs: Vec<&ShellRun> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Shell(run) if run.pending && !run.is_running() => Some(run.as_ref()),
            _ => None,
        })
        .collect();
    if runs.is_empty() {
        return None;
    }
    let mut text = i18n::t!("agent.shell_context_intro");
    text.push('\n');
    for run in &runs {
        text.push_str(&format!("<bash-input>{}</bash-input>\n", run.command));
        text.push_str(&format!("<bash-stdout>{}</bash-stdout>\n", run.stdout));
        text.push_str(&format!("<bash-stderr>{}</bash-stderr>\n", run.stderr));
        match &run.status {
            ShellStatus::Exited(Some(0)) | ShellStatus::Running { .. } => {}
            ShellStatus::Exited(Some(code)) => {
                text.push_str(&format!("<bash-exit-code>{code}</bash-exit-code>\n"));
            }
            _ => {
                if let Some(label) = run.status_label() {
                    text.push_str(&format!("<bash-note>{label}</bash-note>\n"));
                }
            }
        }
    }
    Some((text, runs.iter().map(|run| run.id).collect()))
}

/// 添えた行を渡し済みにする。
pub(crate) fn mark_shell_runs_delivered(entries: &mut [Entry], delivered: &[u64]) {
    for entry in entries {
        if let Entry::Shell(run) = entry {
            if delivered.contains(&run.id) {
                run.pending = false;
            }
        }
    }
}

/// スレッドの走っているコマンドを全部止める。SSH 先で待たずに畳んだ行のうち、保存済みの物は
/// DB を書き換える必要がある: その（行の id, 中身）を返す。
fn stop_thread_shell_runs(thread: &mut Thread) -> (bool, Vec<(i64, String)>) {
    let mut stopped = false;
    let mut rewrites = Vec::new();
    for entry in &mut thread.entries {
        if let Entry::Shell(run) = entry {
            if run.request_stop() {
                stopped = true;
                if !run.is_running() {
                    if let Some(turn_id) = run.stored_turn.take() {
                        rewrites.push((turn_id, run.stored_content()));
                    }
                }
            }
        }
    }
    (stopped, rewrites)
}

/// 取り込んだ stdout / stderr を画面とプロンプトに載せる形へ（端末の制御文字を除き、合計が
/// [`SHELL_OUTPUT_MAX_CHARS`] を超えたら頭と尻を残して間を省く）。
pub(crate) fn shell_output_texts(
    stdout: &CapturedOutput,
    stderr: &CapturedOutput,
) -> (String, String) {
    let stdout = captured_text(stdout);
    let stderr = captured_text(stderr);
    let (stdout_budget, stderr_budget) = split_output_budget(
        stdout.chars().count(),
        stderr.chars().count(),
        SHELL_OUTPUT_MAX_CHARS,
    );
    (
        cut_middle(&stdout, stdout_budget),
        cut_middle(&stderr, stderr_budget),
    )
}

/// 1 本の出力を文字列へ。Host が間を捨てていれば（[`CapturedOutput::omitted`]）、頭と尻の間に
/// 省いた印を挟む。
fn captured_text(output: &CapturedOutput) -> String {
    let head = normalize_terminal_output(&decode_head(&output.head));
    if output.omitted == 0 {
        return head.trim_end().to_string();
    }
    let tail = normalize_terminal_output(&decode_tail(&output.tail));
    format!(
        "{}\n{}\n{}",
        head.trim_end(),
        i18n::t!("agent.shell_output_cut"),
        tail.trim().trim_start_matches('\n')
    )
}

/// 頭を UTF-8 として読む。上限で切れた末尾の文字の断片だけは落とす（`�` を出さない）。
fn decode_head(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(error) if error.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// 尻を UTF-8 として読む。先頭で切れた文字の続き（継続バイト）は落とす。
fn decode_tail(bytes: &[u8]) -> String {
    let start = bytes
        .iter()
        .take(3)
        .take_while(|byte| (**byte & 0xC0) == 0x80)
        .count();
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// 端末向けの制御を除く: 色などのエスケープ（CSI / OSC / 文字集合の指定）を消し、`\r\n` は改行に、
/// 単独の `\r`（進捗表示の上書き）はその行を書き直す扱いにする（次の文字が来た時に行を消す＝
/// `\r` で終わる行は残す）。改行とタブ以外の制御文字は捨てる。
pub(crate) fn normalize_terminal_output(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    // 行頭へ戻った（`\r`）。次に文字が来たら、この行を書き直す。
    let mut carriage_return = false;
    while let Some(character) = chars.next() {
        if carriage_return && !character.is_control() {
            let line_start = out.rfind('\n').map_or(0, |index| index + 1);
            out.truncate(line_start);
            carriage_return = false;
        }
        match character {
            '\u{1b}' => match chars.next() {
                // CSI: 引数（0x30–0x3F）と中間（0x20–0x2F）の後、終端（0x40–0x7E）まで。
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                }
                // OSC（題名・リンク）: BEL か ESC \ まで。
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // 文字集合の指定（`ESC ( B` など 3 文字）。
                Some('(' | ')' | '*' | '+' | '-' | '.' | '/') => {
                    chars.next();
                }
                // それ以外の 2 文字のエスケープ。
                _ => {}
            },
            '\r' => carriage_return = chars.peek() != Some(&'\n'),
            '\n' => {
                carriage_return = false;
                out.push('\n');
            }
            '\t' => out.push('\t'),
            other if other.is_control() => {}
            other => out.push(other),
        }
    }
    out
}

/// 上限（文字）を stdout と stderr へ配る。片方が半分に収まれば、残りは全部もう片方へ。
fn split_output_budget(stdout: usize, stderr: usize, limit: usize) -> (usize, usize) {
    if stdout + stderr <= limit {
        return (stdout, stderr);
    }
    let half = limit / 2;
    if stdout <= half {
        return (stdout, limit - stdout);
    }
    if stderr <= half {
        return (limit - stderr, stderr);
    }
    (half, limit - half)
}

/// `budget` 文字を超えたら頭と尻の半分ずつを残し、間に省いた印を挟む。
fn cut_middle(text: &str, budget: usize) -> String {
    let count = text.chars().count();
    if count <= budget {
        return text.to_string();
    }
    let head = budget / 2;
    let tail = budget - head;
    let head_text: String = text.chars().take(head).collect();
    let tail_text: String = text.chars().skip(count - tail).collect();
    format!(
        "{}\n{}\n{}",
        head_text.trim_end(),
        i18n::t!("agent.shell_output_cut"),
        tail_text.trim_start_matches('\n')
    )
}

impl AgentPanel {
    /// `!` のコマンドを、スレッドの作業ディレクトリで走らせる（composer の送信からだけ呼ぶ）。
    /// 作業ディレクトリが無い（プロジェクト未確定・フォルダの無いチャット）時は走らせず、composer の
    /// 本文も残す（理由は composer の下のヒントが出している）。
    pub(crate) fn run_shell_command(
        &mut self,
        thread_index: usize,
        command: &str,
        cx: &mut Context<Self>,
    ) {
        if self.threads.get(thread_index).is_none() {
            return;
        }
        let Some(cwd) = self.shell_cwd(thread_index) else {
            cx.notify();
            return;
        };
        self.composer.update(cx, |composer, cx| composer.clear(cx));
        let host = self.dest_host.clone();
        let spec = shell_command_spec(host.as_ref(), command, &cwd);
        let cancel = Arc::new(AtomicBool::new(false));
        let live = LiveOutput::default();
        let run = ShellRun::running(
            command,
            cancel.clone(),
            live.clone(),
            host.can_stop_user_command(),
        );
        let run_id = run.id;
        let Some(thread) = self.threads.get_mut(thread_index) else {
            return;
        };
        thread.draft.clear();
        thread.entries.push(Entry::Shell(Box::new(run)));
        let thread_id = thread.id.clone();
        // 待機中なら走り出した時点で保存する（途中で necoder を終了しても、再起動後に「中断」として
        // 読める）。ターンの途中は保存しない（生成中の途切れた本文を DB に残さない）＝ターンの終わりに
        // 一緒に書く。
        if !thread.running {
            self.persist_thread(thread_index);
        }
        // Host は blocking API＝背景で待つ（UI は止めない）。止まらないコマンド（開発サーバ・
        // `tail -f`）は止めるまで背景のスレッドを 1 本使う（ほかの Host 呼び出しと同じ扱い）。
        let running_live = live.clone();
        let running = cx.background_executor().spawn(async move {
            host.run_user_command(&spec, &cancel, SHELL_CAPTURE_BYTES, &running_live)
        });
        // 走っている間は、届いた出力を行へ描き直す（終わるまで無言にしない）。
        let live_thread_id = thread_id.clone();
        cx.spawn(async move |panel, cx| {
            let mut shown = 0;
            loop {
                cx.background_executor().timer(SHELL_LIVE_INTERVAL).await;
                let received = live.received();
                let changed = received != shown;
                shown = received;
                let still_running = panel
                    .update(cx, |panel, cx| {
                        panel.refresh_shell_live(&live_thread_id, run_id, changed, cx)
                    })
                    .unwrap_or(false);
                if !still_running {
                    return;
                }
            }
        })
        .detach();
        cx.spawn(async move |panel, cx| {
            let result = running.await;
            // パネルが先に無くなっていれば（窓を閉じた）結果は要らない。
            panel
                .update(cx, |panel, cx| {
                    panel.finish_shell_run(&thread_id, run_id, result, cx)
                })
                .ok();
        })
        .detach();
        if self.active == thread_index {
            // 自分が打ったコマンドの行は、遡って読んでいる最中でも見える所へ。
            self.transcript_list.scroll_to_end();
        }
        cx.notify();
    }

    /// 走っている `!` の行を描き直す（届いた出力があった時だけ）。もう走っていなければ `false`
    /// （描き直しの見回りを終える）。
    fn refresh_shell_live(
        &mut self,
        thread_id: &str,
        run_id: u64,
        changed: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(thread_index) = self.thread_index_by_id(thread_id) else {
            return false; // 閉じたスレッド（走っている物は閉じる時に止めている）
        };
        let Some(entry_index) = self.threads[thread_index].entries.iter().position(
            |entry| matches!(entry, Entry::Shell(run) if run.id == run_id && run.is_running()),
        ) else {
            return false;
        };
        if changed && thread_index == self.active {
            self.transcript_list
                .remeasure_items(entry_index..entry_index + 1);
            cx.notify();
        }
        true
    }

    /// `!` のコマンドを走らせる場所。エージェントと同じ（Chat はチャットのフォルダ・それ以外は
    /// 宛先のプロジェクト）。チャットのフォルダは空だと片付けられているので作り直す。
    fn shell_cwd(&self, thread_index: usize) -> Option<PathBuf> {
        let cwd = self.session_cwd(thread_index)?;
        if self.chat_mode {
            if let Err(error) = chat_core::folder::ensure(&cwd) {
                eprintln!("チャットのフォルダを用意できない: {error:#}");
                return None;
            }
        }
        Some(cwd)
    }

    /// `!` が使えない理由（composer のヒントに出す）。使えるなら `None`。
    fn shell_unavailable_reason(&self) -> Option<String> {
        if self.session_cwd(self.active).is_some() {
            return None;
        }
        Some(if self.chat_mode {
            i18n::t!("agent.shell_hint_no_folder")
        } else {
            i18n::t!("agent.shell_hint_no_project")
        })
    }

    /// 背景の結果を行へ反映する。閉じたスレッド（⌘⇧T で戻せる）にも届ける。止めて先に畳んだ行
    /// （SSH 先）に遅れて届いた結果は捨てる。
    fn finish_shell_run(
        &mut self,
        thread_id: &str,
        run_id: u64,
        result: anyhow::Result<UserCommandOutput>,
        cx: &mut Context<Self>,
    ) {
        let open_index = self.thread_index_by_id(thread_id);
        let thread = match open_index {
            Some(index) => self.threads.get_mut(index),
            // 閉じたスレッドは走っている行がある間は退避しない（`ClosedThread::spill_entries`）ので、
            // 本文はまだメモリにある。
            None => self
                .closed_threads
                .iter_mut()
                .map(|closed| &mut closed.thread)
                .find(|thread| thread.id == thread_id),
        };
        let Some(thread) = thread else {
            return;
        };
        let Some(entry_index) = thread.entries.iter().position(
            |entry| matches!(entry, Entry::Shell(run) if run.id == run_id && run.is_running()),
        ) else {
            return;
        };
        let Some(Entry::Shell(run)) = thread.entries.get_mut(entry_index) else {
            return;
        };
        run.finish(result);
        let rewrite = run
            .stored_turn
            .take()
            .map(|turn_id| (turn_id, run.stored_content()));
        let unsaved_while_idle =
            rewrite.is_none() && entry_index >= thread.persisted_entries && !thread.running;
        match (rewrite, open_index) {
            // 走っている途中で保存した行を、最終の形へ書き換える。
            (Some((turn_id, content)), _) => self.rewrite_stored_turn(turn_id, content, cx),
            // まだ保存していない行（保存できなかった）。待機中なら今書く（ターンの途中ならターンの
            // 終わりに一緒に書かれる）。
            (None, Some(index)) if unsaved_while_idle => self.persist_thread(index),
            _ => {}
        }
        if open_index == Some(self.active) {
            self.transcript_list
                .remeasure_items(entry_index..entry_index + 1);
        }
        cx.notify();
    }

    /// 保存済みの turn の中身を書き換える（背景で・UI スレッドで DB を待たない）。
    fn rewrite_stored_turn(&self, turn_id: i64, content: String, cx: &mut Context<Self>) {
        let Some(storage) = self.storage.clone() else {
            return;
        };
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = storage.update_turn(turn_id, &content) {
                    eprintln!("シェルの結果を保存できない: {error:#}");
                }
            })
            .detach();
    }

    /// スレッドで走っている `!` のコマンドを止める（esc）。走っていれば `true`。
    pub(crate) fn stop_shell_runs(&mut self, thread_index: usize, cx: &mut Context<Self>) -> bool {
        let Some(thread) = self.threads.get_mut(thread_index) else {
            return false;
        };
        let (stopped, rewrites) = stop_thread_shell_runs(thread);
        for (turn_id, content) in rewrites {
            self.rewrite_stored_turn(turn_id, content, cx);
        }
        if stopped {
            cx.notify();
        }
        stopped
    }

    /// 行の ■ から 1 つだけ止める。
    fn stop_shell_run(&mut self, run_id: u64, cx: &mut Context<Self>) {
        let Some(thread) = self.threads.get_mut(self.active) else {
            return;
        };
        let mut rewrite = None;
        for entry in &mut thread.entries {
            if let Entry::Shell(run) = entry {
                if run.id == run_id && run.request_stop() && !run.is_running() {
                    rewrite = run
                        .stored_turn
                        .take()
                        .map(|turn_id| (turn_id, run.stored_content()));
                }
            }
        }
        if let Some((turn_id, content)) = rewrite {
            self.rewrite_stored_turn(turn_id, content, cx);
        }
        cx.notify();
    }

    /// 閉じる・消すスレッドの走っているコマンドを止める（タブの × / ⌘W・チャットの削除）。
    pub(crate) fn stop_shell_runs_of(&self, thread: &mut Thread, cx: &mut Context<Self>) {
        let (_, rewrites) = stop_thread_shell_runs(thread);
        for (turn_id, content) in rewrites {
            self.rewrite_stored_turn(turn_id, content, cx);
        }
    }

    /// パネルの全スレッド（閉じた物も）の走っているコマンドを止める（パネルが無くなる・終了）。
    /// 走っていた物があれば `true`。DB は書き換えない（再起動後は「中断」として読める）。
    pub(crate) fn stop_all_shell_runs(&mut self) -> bool {
        let mut stopped = false;
        for thread in self.threads.iter_mut().chain(
            self.closed_threads
                .iter_mut()
                .map(|closed| &mut closed.thread),
        ) {
            stopped |= stop_thread_shell_runs(thread).0;
        }
        stopped
    }

    /// 添える予定の結果を外す（composer のチップの ×）。
    fn discard_shell_result(&mut self, run_id: u64, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            mark_shell_runs_delivered(&mut thread.entries, &[run_id]);
        }
        cx.notify();
    }

    /// transcript の `!` の行: `! <コマンド>` の見出し（等幅）→ `⎿ 出力`（長ければ畳む・ツールの
    /// 結果と同じ）→ 状態の一言（終了コード・中断など・中立の文字）。走っている間は ⎿ に
    /// 点字スピナーと「実行中」、■ で止める。
    pub(crate) fn render_shell_entry(
        &self,
        index: usize,
        run: &ShellRun,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let code_font = ui::code_font(cx);
        let mut body = div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(4.))
            // コマンドはインラインコードと同じ見た目（syn-mac 文字 + bg3 面・§1.3）。文字の幅だけ取る。
            .child(
                div().flex().child(
                    div()
                        .flex_initial()
                        .min_w_0()
                        .px(px(5.))
                        .rounded(px(4.))
                        .bg(theme.bg3)
                        .font_family(code_font.clone())
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.syntax.macro_)
                        .child(self.selectable_text(run.command.clone(), cx)),
                ),
            );
        let result_row = div()
            .flex()
            .items_start()
            .gap(px(4.))
            .pt(px(3.))
            .font_family(code_font.clone())
            .text_size(px(11.))
            .text_color(theme.fg2)
            .child(div().flex_none().child("⎿"));
        if let ShellStatus::Running { stopping } = run.status {
            let run_id = run.id;
            let mut row = result_row
                .items_center()
                .child(working_spinner(("shell-spinner", index), 8.0, theme.fg2))
                .child(SharedString::from(if stopping {
                    i18n::t!("agent.shell_stopping")
                } else {
                    i18n::t!("agent.shell_running")
                }));
            if !stopping {
                row = row.child(
                    div()
                        .id(("shell-stop", index))
                        .ml(px(6.))
                        .px(px(8.))
                        .py(px(1.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(theme.border)
                        .font_family(ui::ui_font(cx))
                        .text_size(px(10.5))
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                        .child(SharedString::from(i18n::t!("agent.shell_stop")))
                        .tooltip(Tooltip::text(
                            i18n::t!("agent.shell_stop_tip"),
                            theme.clone(),
                        ))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |panel, _, _window, cx| {
                                // transcript のドラッグ選択と、ルートの「composer へフォーカスを戻す」に流さない。
                                cx.stop_propagation();
                                panel.stop_shell_run(run_id, cx);
                            }),
                        ),
                );
            }
            body = body.child(row);
            if let Some(tail) = run.live_tail() {
                body = body.child(
                    div()
                        .pl(px(15.))
                        .min_w_0()
                        .font_family(code_font.clone())
                        .text_size(px(11.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(tail)),
                );
            }
        } else {
            let output = run.output.clone();
            let row = if output.is_empty() {
                result_row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(SharedString::from(i18n::t!("agent.shell_no_output"))),
                )
            } else {
                let line_count = output.lines().count().max(1);
                let collapsible =
                    line_count > STEP_COLLAPSE_MIN_LINES || output.len() > STEP_COLLAPSE_MIN_BYTES;
                if collapsible {
                    let expanded = self.is_step_expanded(index);
                    let header = div()
                        .id(("shell-result", index))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .cursor_pointer()
                        .child(div().flex_none().text_size(px(8.)).child(if expanded {
                            "▾"
                        } else {
                            "▸"
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(step_result_summary(output.as_ref(), line_count)),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |panel, _, _window, cx| {
                                cx.stop_propagation();
                                panel.toggle_step(index, cx);
                            }),
                        );
                    let mut column = div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(px(3.))
                        .child(header);
                    if expanded {
                        column = column.child(div().min_w_0().child(self.push_selectable(
                            output,
                            Vec::new(),
                            cx,
                        )));
                    }
                    result_row.child(column)
                } else {
                    result_row.child(div().flex_1().min_w_0().child(self.push_selectable(
                        output,
                        Vec::new(),
                        cx,
                    )))
                }
            };
            body = body.child(row);
            if let Some(label) = run.status_label() {
                body = body.child(
                    div()
                        .pl(px(15.))
                        .text_size(px(11.))
                        .text_color(theme.fg1)
                        .child(SharedString::from(label)),
                );
            }
        }
        div()
            .flex()
            .gap(px(8.))
            .text_size(px(12.5))
            .child(
                div()
                    .flex_none()
                    .font_family(code_font)
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.syntax.macro_)
                    .child("!"),
            )
            .child(body)
            .into_any_element()
    }

    /// `! シェル` バッジ（syn-mac 文字 + bg3 面＝インラインコードと同じ見た目）。宛先チップの右に出す。
    pub(crate) fn render_shell_badge(&self) -> gpui::Div {
        div()
            .flex_none()
            .px(px(5.))
            .rounded(px(4.))
            .bg(self.theme.bg3)
            .text_color(self.theme.syntax.macro_)
            .font_weight(FontWeight::SEMIBOLD)
            .child(SharedString::from(i18n::t!("agent.shell_badge")))
    }

    /// シェルモードの宛先チップに出す実行場所（`~/path`・SSH 先は `host:path`）。走らせられない
    /// （作業ディレクトリが無い）時は `None`（理由は composer の下のヒントが出す）。
    pub(crate) fn shell_place(&self) -> Option<String> {
        let cwd = self.session_cwd(self.active)?;
        if !self.chat_mode && self.dest_host.is_remote() {
            return Some(format!(
                "{}:{}",
                self.dest_host.display_name(),
                cwd.display()
            ));
        }
        Some(tilde_path(&cwd))
    }

    /// composer の下のヒント（本文が `!` で始まっている間だけ）: シェルで走ること・エージェントには
    /// 送らないこと・結果は次の送信に添えること。走らせられない時はその理由。
    pub(crate) fn render_shell_hint(&self, cx: &App) -> Option<gpui::AnyElement> {
        if !self.shell_input {
            return None;
        }
        let theme = &self.theme;
        let text = self
            .shell_unavailable_reason()
            .unwrap_or_else(|| i18n::t!("agent.shell_hint"));
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .pt(px(4.))
                .text_size(px(10.5))
                .text_color(theme.fg2)
                .child(
                    div()
                        .flex_none()
                        .font_family(ui::code_font(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.syntax.macro_)
                        .child("!"),
                )
                .child(div().flex_1().min_w_0().child(SharedString::from(text)))
                .into_any_element(),
        )
    }

    /// 次の送信に添える予定の結果（composer の添付チップと同じ列）。× で外す。
    pub(crate) fn render_shell_chips(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let Some(thread) = self.threads.get(self.active) else {
            return Vec::new();
        };
        let theme = self.theme.clone();
        let code_font = ui::code_font(cx);
        thread
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Shell(run) if run.pending && !run.is_running() => {
                    Some((run.id, run.command.clone()))
                }
                _ => None,
            })
            .enumerate()
            .map(|(position, (run_id, command))| {
                div()
                    .id(("shell-chip", position))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(7.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .bg(theme.bg3)
                    .text_size(px(10.5))
                    .text_color(theme.fg1)
                    .child(
                        div()
                            .font_family(code_font.clone())
                            .text_color(theme.syntax.macro_)
                            .child(format!("! {}", ellipsize_middle(&command, 36))),
                    )
                    .tooltip(Tooltip::text(
                        i18n::t!("agent.shell_pending_tip"),
                        theme.clone(),
                    ))
                    .child(
                        div()
                            .id(("shell-chip-x", position))
                            .text_color(theme.fg2)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.fg0))
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |panel, _, _window, cx| {
                                    cx.stop_propagation();
                                    panel.discard_shell_result(run_id, cx);
                                }),
                            ),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

#[cfg(debug_assertions)]
impl AgentPanel {
    /// 開発用（`NECODER_SHELL_PROBE`）: 本物の経路（composer の送信と同じ入口）で見本のコマンドを
    /// 走らせ、`!` の行（成功・失敗・長い出力の畳み・実行中）と、composer のヒント・添える予定の
    /// チップを写す。`expanded` なら長い出力を開いておく。`chat` はフォルダの無いチャット（送っても
    /// 走らず、本文が残ってヒントが理由を出す）、`chat-folder` はフォルダのあるチャットで走らせる。
    pub fn debug_shell_probe(&mut self, mode: &str, window: &mut Window, cx: &mut Context<Self>) {
        let commands: &[&str] = match mode {
            "chat" => &[],
            "chat-folder" => &["ls -R", "pwd"],
            _ => &[
                "echo hello from necoder",
                "git status --short --branch",
                "echo 'missing.txt: No such file or directory' >&2; exit 3",
                "seq 1 400",
                // 走っている行（届いた出力の末尾が流れて見える）。
                "echo compiling necoder; seq 1 3; sleep 6",
            ],
        };
        for command in commands {
            self.composer.update(cx, |composer, cx| {
                composer.set_plain_text(&format!("!{command}"), cx)
            });
            self.submit(cx);
        }
        if mode == "expanded" {
            if let Some(thread) = self.threads.get(self.active) {
                // 長い出力（`seq`）は末尾から 2 つ目の行。
                let index = thread.entries.len().saturating_sub(2);
                self.expanded_steps.insert((thread.id.clone(), index));
            }
        }
        let draft = if mode == "chat" {
            "!ls"
        } else {
            "!git log --oneline -3"
        };
        self.composer
            .update(cx, |composer, cx| composer.set_plain_text(draft, cx));
        if mode == "chat" {
            // フォルダが無いので走らない（本文は残る）。
            self.submit(cx);
        }
        self.focus_composer(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    //! 本物のシェルでコマンドを走らせるテストは POSIX の書き方（`;`・`sleep`・`pwd -P`）なので unix だけ
    //! （Windows の手元は `cmd.exe /C`）。走らせないテスト（入口の判定・止める配線・SSH 先の中断）はどこでも。

    use super::*;

    /// 設定と作業ディレクトリを一時ディレクトリへ向ける（本物の置き場・書類に触れない）。
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(cx: &mut gpui::TestAppContext, label: &str) -> Fixture {
            let root = std::env::temp_dir().join(format!(
                "necoder_shell_mode_{label}_{}_{}",
                std::process::id(),
                now_unix_ms()
            ));
            std::fs::create_dir_all(root.join("project")).expect("一時ディレクトリを作れる");
            let settings = root.join("settings.json");
            std::fs::write(
                &settings,
                serde_json::json!({
                    "onboarded": true,
                    // 本物のエージェント・CLI を起こさない（先張り・自前の命名・✳ 要約）。
                    "agent_prewarm": false,
                    "agent_auto_name": false,
                    "tier2_summaries": false,
                    "chat": { "directory": root.join("chats") },
                })
                .to_string(),
            )
            .expect("設定を書ける");
            cx.update(|cx| settings::init(Some(settings.clone()), None, cx));
            Fixture { root }
        }

        fn project(&self) -> PathBuf {
            self.root.join("project")
        }

        #[cfg(unix)]
        fn storage(&self) -> storage::Storage {
            storage::Storage::open(&self.root.join("necoder.db")).expect("DB を開ける")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.root) {
                eprintln!("一時ディレクトリを消せない: {error}");
            }
        }
    }

    /// 送信路だけ差し込む（本物のエージェントを起こさずに「送った prompt」を見る）。
    fn attach_fake_session(
        panel: &mut AgentPanel,
        index: usize,
    ) -> mpsc::UnboundedReceiver<SessionCommand> {
        let (command_tx, commands) = mpsc::unbounded::<SessionCommand>();
        panel.threads[index].command_tx = Some(command_tx);
        commands
    }

    /// composer に打って送る（人の送信と同じ入口）。コマンドは背景のタスクに積まれ、テストの
    /// `run_until_parked` で本物のプロセスとして走る。
    fn type_and_submit(panel: &mut AgentPanel, text: &str, cx: &mut Context<AgentPanel>) {
        panel
            .composer
            .update(cx, |composer, cx| composer.set_plain_text(text, cx));
        panel.submit(cx);
    }

    fn shell_runs(panel: &AgentPanel, index: usize) -> Vec<&ShellRun> {
        panel.threads[index]
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Shell(run) => Some(run.as_ref()),
                _ => None,
            })
            .collect()
    }

    fn prompt_text(commands: &mut mpsc::UnboundedReceiver<SessionCommand>) -> String {
        match commands.try_recv() {
            Ok(SessionCommand::Prompt(text)) => text,
            other => panic!("prompt が送られていない: {other:?}"),
        }
    }

    #[test]
    fn only_a_leading_bang_with_a_command_is_a_shell_command() {
        assert_eq!(shell_command_input("!git status"), Some("git status"));
        assert_eq!(shell_command_input("!  ls -la  \n"), Some("ls -la"));
        assert_eq!(
            shell_command_input("!echo a\necho b"),
            Some("echo a\necho b"),
            "複数行はそのままシェルへ"
        );
        assert_eq!(shell_command_input(" !git status"), None, "空白で始まる");
        assert_eq!(shell_command_input("!"), None, "`!` だけ");
        assert_eq!(shell_command_input("!   \n "), None, "空白だけ");
        assert_eq!(shell_command_input(""), None);
        assert_eq!(shell_command_input("git status!"), None);
    }

    #[test]
    fn terminal_controls_are_removed_from_the_output() {
        assert_eq!(
            normalize_terminal_output("\u{1b}[1;31merror\u{1b}[0m: x\r\n"),
            "error: x\n"
        );
        assert_eq!(
            normalize_terminal_output("10%\r50%\r\u{1b}[K100%\ndone"),
            "100%\ndone",
            "進捗の上書きは最後の姿だけ"
        );
        assert_eq!(
            normalize_terminal_output("最後の行\r"),
            "最後の行",
            "`\\r` で終わる行は消さない"
        );
        assert_eq!(
            normalize_terminal_output(
                "\u{1b}]0;title\u{7}a\u{1b}]8;;http://x\u{1b}\\b\u{1b}(Bc\u{7}"
            ),
            "abc"
        );
        assert_eq!(normalize_terminal_output("a\tb\u{8}c"), "a\tbc");
    }

    #[test]
    fn long_output_is_cut_in_the_middle_and_says_so() {
        let marker = i18n::t!("agent.shell_output_cut");
        assert_eq!(split_output_budget(10, 20, 100), (10, 20));
        assert_eq!(split_output_budget(10, 200, 100), (10, 90));
        assert_eq!(split_output_budget(200, 10, 100), (90, 10));
        assert_eq!(split_output_budget(200, 300, 100), (50, 50));

        let lines: String = (1..=20_000).map(|number| format!("{number}\n")).collect();
        let captured = CapturedOutput {
            head: lines.into_bytes(),
            tail: Vec::new(),
            omitted: 0,
        };
        let (stdout, stderr) = shell_output_texts(&captured, &CapturedOutput::default());
        assert!(stderr.is_empty());
        assert!(stdout.starts_with("1\n2\n3\n"));
        assert!(stdout.ends_with("19999\n20000"));
        assert!(stdout.contains(&marker), "切ったことが分かる");
        assert!(stdout.chars().count() <= SHELL_OUTPUT_MAX_CHARS + marker.chars().count() + 2);

        // Host が間を捨てていれば（取り込みの上限）、頭と尻の間に同じ印が入る。尻の先頭で切れた
        // 文字の断片・頭の末尾で切れた断片は落とす（`�` を出さない）。
        let captured = CapturedOutput {
            head: "先頭".as_bytes()[..5].to_vec(),
            tail: "末尾".as_bytes()[1..].to_vec(),
            omitted: 1_000,
        };
        let (stdout, _) = shell_output_texts(&captured, &CapturedOutput::default());
        assert_eq!(stdout, format!("先\n{marker}\n尾"));
    }

    #[test]
    fn a_shell_run_round_trips_through_storage_and_a_running_one_reads_as_interrupted() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut run = ShellRun::running("cargo test", cancel, LiveOutput::default(), true);
        let running = run.stored_content();
        run.finish(Ok(UserCommandOutput {
            status_code: Some(101),
            stdout: CapturedOutput {
                head: b"test result: FAILED\n".to_vec(),
                tail: Vec::new(),
                omitted: 0,
            },
            stderr: CapturedOutput::default(),
            cancelled: false,
        }));
        let restored = ShellRun::from_stored(&run.stored_content()).expect("読める");
        assert_eq!(restored.command.as_ref(), "cargo test");
        assert_eq!(restored.stdout.as_ref(), "test result: FAILED");
        assert_eq!(restored.status, ShellStatus::Exited(Some(101)));
        assert!(!restored.pending, "再起動を越えては添えない");

        let interrupted = ShellRun::from_stored(&running).expect("読める");
        assert_eq!(
            interrupted.status,
            ShellStatus::Interrupted { detached: false },
            "走っている途中で終わった物は中断"
        );
        assert!(ShellRun::from_stored("壊れた行").is_none());
    }

    /// 人が composer から送った `!echo hi` と `!sh -c 'exit 3'` は、スレッドの作業ディレクトリで
    /// シェルとして走り、出力と終了コードが行に出る。エージェントには何も送らない（ターンもトークンも
    /// 使わない）。
    #[cfg(unix)]
    #[gpui::test]
    fn a_bang_command_runs_in_the_threads_directory(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "run");
        std::fs::write(fixture.project().join("marker.txt"), "x").expect("印を置ける");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        let mut commands = panel.update(cx, |panel, cx| {
            panel.dest_cwd = Some(fixture.project());
            let active = panel.active;
            let commands = attach_fake_session(panel, active);
            type_and_submit(panel, "!echo hi; ls", cx);
            assert_eq!(panel.composer_text(cx), "", "composer は空になる");
            assert!(
                !panel.threads[active].running,
                "エージェントのターンは始めない"
            );
            let runs = shell_runs(panel, active);
            assert_eq!(runs.len(), 1);
            assert!(runs[0].is_running());
            type_and_submit(panel, "!sh -c 'exit 3'", cx);
            commands
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            let runs = shell_runs(panel, panel.active);
            assert_eq!(runs[0].command.as_ref(), "echo hi; ls");
            assert_eq!(
                runs[0].stdout.as_ref(),
                "hi\nmarker.txt",
                "作業ディレクトリで走る"
            );
            assert_eq!(runs[0].status, ShellStatus::Exited(Some(0)));
            assert_eq!(runs[0].status_label(), None);
            assert_eq!(runs[1].status, ShellStatus::Exited(Some(3)));
            assert_eq!(
                runs[1].status_label(),
                Some(i18n::t!("agent.shell_exit_code", "code" => 3))
            );
            assert_eq!(panel.threads[panel.active].tokens_used, 0);
        });
        assert!(
            commands.try_recv().is_err(),
            "エージェントには何も送っていない"
        );
    }

    /// 結果は次のプロンプトに 1 回だけ添わる（2 回目は添わない）。slash コマンドには添えず、次の
    /// 通常の送信まで残す。× で外した物は添えない。
    #[cfg(unix)]
    #[gpui::test]
    fn the_output_goes_with_the_next_prompt_once(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "attach");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        let mut commands = panel.update(cx, |panel, cx| {
            panel.dest_cwd = Some(fixture.project());
            let active = panel.active;
            let commands = attach_fake_session(panel, active);
            type_and_submit(panel, "!echo hi", cx);
            type_and_submit(panel, "!echo dropped", cx);
            assert!(
                panel.render_shell_chips(cx).is_empty(),
                "走っている間はまだ添えない"
            );
            commands
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            assert_eq!(
                panel.render_shell_chips(cx).len(),
                2,
                "添える予定をチップで見せる"
            );
            let dropped = shell_runs(panel, active)[1].id;
            panel.discard_shell_result(dropped, cx);
            assert_eq!(panel.render_shell_chips(cx).len(), 1);

            panel.send_prompt_text("/compact".to_string(), cx);
            panel.threads[active].running = false;
            assert_eq!(prompt_text(&mut commands), "/compact", "slash には添えない");

            type_and_submit(panel, "これを見て直して", cx);
            panel.threads[active].running = false;
            let prompt = prompt_text(&mut commands);
            assert!(prompt.starts_with(&i18n::t!("agent.shell_context_intro")));
            assert!(prompt.contains(
                "<bash-input>echo hi</bash-input>\n<bash-stdout>hi</bash-stdout>\n<bash-stderr></bash-stderr>\n"
            ));
            assert!(!prompt.contains("dropped"), "× で外した物は添えない");
            assert!(prompt.ends_with("\nこれを見て直して"));
            assert!(
                matches!(panel.threads[active].entries.last(), Some(Entry::User(text)) if text.as_ref() == "これを見て直して"),
                "transcript は人の本文のまま"
            );
            assert!(panel.render_shell_chips(cx).is_empty(), "添えたら渡し済み");

            type_and_submit(panel, "もう一度", cx);
            panel.threads[active].running = false;
            assert_eq!(prompt_text(&mut commands), "もう一度", "2 回目は添えない");
        });
    }

    /// 人の composer 以外の入口では `!` を解釈しない: スマホ（リモート管制）・`ne` / MCP・Captain の
    /// 台帳・Fleet の注記・キュー（ターン完了で流れる物）。どれも文のままエージェントへ届き、
    /// 作業ディレクトリでは何も走らない。
    #[gpui::test]
    fn a_bang_from_anywhere_but_the_composer_is_sent_as_text(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "not_human");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            panel.dest_cwd = Some(fixture.project());
            let active = panel.active;
            let mut commands = attach_fake_session(panel, active);
            let mut settle = |panel: &mut AgentPanel| {
                panel.threads[active].running = false;
                prompt_text(&mut commands)
            };

            // `ne` / MCP・開発用の自動送信（`send_prompt_text`）。
            panel.send_prompt_text("!touch pwned-cli".to_string(), cx);
            assert_eq!(settle(panel), "!touch pwned-cli");

            // スマホ（リモート管制の `send_message`）。
            let params = serde_json::json!({
                "thread_id": panel.threads[active].id,
                "turn_id": remote::turn_id(&panel.threads[active]),
                "message": "!touch pwned-phone",
            });
            panel
                .remote_command("send_message", &params, cx)
                .expect("スマホからの送信");
            assert_eq!(settle(panel), "!touch pwned-phone");

            // Fleet の注記の一括送信（`send_user_prompt_to`）。
            assert!(panel.send_user_prompt_to(active, "!touch pwned-note".to_string(), cx));
            assert_eq!(settle(panel), "!touch pwned-note");

            // necoder の知らせ（Captain の台帳の未読・`send_auto_prompt_to`）。印で囲んだ文のまま届く。
            panel.send_auto_prompt_to(active, "台帳", "!touch pwned-captain".to_string(), cx);
            let sent = settle(panel);
            assert!(
                sent.starts_with("<necoder-event") && sent.contains("\n!touch pwned-captain\n"),
                "{sent}"
            );

            // 生成中に積んだ送信待ちは、ターン完了で文のまま流れる。
            panel.threads[active].running = true;
            assert!(panel.send_user_prompt_to(active, "!touch pwned-queue".to_string(), cx));
            panel.on_event(
                active,
                AgentEvent::TurnEnded {
                    reason: TurnEnd::Completed,
                },
                cx,
            );
            assert_eq!(settle(panel), "!touch pwned-queue");
            assert!(
                shell_runs(panel, active).is_empty(),
                "シェルの行は 1 つも無い"
            );
        });
        cx.run_until_parked();
        let stray: Vec<_> = std::fs::read_dir(fixture.project())
            .expect("作業ディレクトリ")
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(stray.is_empty(), "何も実行されていない: {stray:?}");
    }

    /// 走っている途中で止めると、子（とその孫）を止めて、止めるまでの出力が残り、行は「中断」。
    /// 印は esc / ■ が立てる物と同じ（止めるまでの間はテストのスレッドが待っているので、印は別の
    /// スレッドから立てる）。
    #[cfg(unix)]
    #[gpui::test]
    fn stopping_a_running_command_keeps_the_output_so_far(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "stop_midway");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        let cancel = panel.update(cx, |panel, cx| {
            panel.dest_cwd = Some(fixture.project());
            type_and_submit(panel, "!echo started; sleep 30; echo never", cx);
            let active = panel.active;
            shell_runs(panel, active)[0]
                .cancel
                .clone()
                .expect("走っている間は印がある")
        });
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            cancel.store(true, Ordering::Release);
        });
        let started = std::time::Instant::now();
        cx.run_until_parked();
        canceller.join().expect("取り消し役のスレッド");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "止めたらすぐ終わる: {:?}",
            started.elapsed()
        );
        panel.read_with(cx, |panel, _| {
            let run = shell_runs(panel, panel.active)[0];
            assert_eq!(run.status, ShellStatus::Interrupted { detached: false });
            assert_eq!(run.stdout.as_ref(), "started", "止めるまでの出力は残る");
            assert_eq!(
                run.status_label(),
                Some(i18n::t!("agent.shell_interrupted"))
            );
        });
    }

    /// esc（composer にフォーカスがあっても・キー配送そのものを通す）は、まず `!` のコマンドを止める。
    /// エージェントのターンは止めず、もう一度の esc で止まる。
    #[gpui::test]
    fn escape_stops_the_command_before_the_turn(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "escape");
        cx.update(|cx| {
            let bindings = keymap_core::load_bindings(keymap_core::DEFAULT_KEYMAP_JSON, cx)
                .expect("既定 keymap がロードできる");
            cx.bind_keys(bindings);
        });
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update_in(cx, |panel, window, cx| {
            panel.dest_cwd = Some(fixture.project());
            panel.focus_composer(window, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            // 背景に積んだだけ（まだ走っていない）で esc が届く＝止める印が先に立つ。
            type_and_submit(panel, "!sleep 30", cx);
            // エージェントのターンも走っている（送信路の無い形＝止めればローカルで畳まれる）。
            let active = panel.active;
            panel.threads[active].running = true;
        });
        let started = std::time::Instant::now();
        cx.simulate_keystrokes("escape");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "止めたコマンドは走らせない: {:?}",
            started.elapsed()
        );
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                shell_runs(panel, panel.active)[0].status,
                ShellStatus::Interrupted { detached: false }
            );
            assert!(
                panel.threads[panel.active].running,
                "1 回目の esc はコマンドだけを止める"
            );
        });
        cx.simulate_keystrokes("escape");
        assert!(
            !panel.read_with(cx, |panel, _| panel.threads[panel.active].running),
            "2 回目の esc でターンを止める"
        );
    }

    /// SSH 先のように途中で止められないホストでは、止めた時に待たずに「中断」として畳み、向こうでは
    /// 最後まで動くと書く。遅れて届いた結果は捨てる。
    #[gpui::test]
    fn a_command_that_cannot_be_stopped_is_detached(cx: &mut gpui::TestAppContext) {
        let _fixture = Fixture::new(cx, "detach");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            let run = ShellRun::running(
                "make deploy",
                Arc::new(AtomicBool::new(false)),
                LiveOutput::default(),
                false,
            );
            let run_id = run.id;
            let thread_id = panel.threads[active].id.clone();
            panel.threads[active]
                .entries
                .push(Entry::Shell(Box::new(run)));
            assert!(panel.stop_shell_runs(active, cx));
            let run = shell_runs(panel, active)[0];
            assert_eq!(run.status, ShellStatus::Interrupted { detached: true });
            assert_eq!(
                run.status_label(),
                Some(i18n::t!("agent.shell_interrupted_remote"))
            );
            panel.finish_shell_run(
                &thread_id,
                run_id,
                Ok(UserCommandOutput {
                    status_code: Some(0),
                    stdout: CapturedOutput {
                        head: b"deployed".to_vec(),
                        tail: Vec::new(),
                        omitted: 0,
                    },
                    stderr: CapturedOutput::default(),
                    cancelled: false,
                }),
                cx,
            );
            let run = shell_runs(panel, active)[0];
            assert!(run.stdout.is_empty(), "遅れて届いた結果は捨てる");
            assert!(!panel.stop_shell_runs(active, cx), "もう走っていない");
        });
    }

    /// 行はトランスクリプトと一緒に保存され、再起動後も読める。走り出した時点で保存するので、途中で
    /// necoder が終わった物は「中断」として読める。終わったら同じ行を最終の形へ書き換える（行は増えない）。
    #[cfg(unix)]
    #[gpui::test]
    fn shell_rows_are_saved_and_restored(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "restore");
        let storage = fixture.storage();
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        let thread_id = panel.update(cx, |panel, cx| {
            panel.set_storage(storage.clone(), cx);
            panel.dest_cwd = Some(fixture.project());
            // 背景に積んだだけ（まだ走っていない）＝ここで終了したのと同じ DB の姿。
            type_and_submit(panel, "!echo saved", cx);
            panel.threads[panel.active].id.clone()
        });
        let restore = |cx: &mut gpui::VisualTestContext| {
            let restarted = cx.new(|cx| AgentPanel::new(Theme::dark(), cx));
            restarted.update(cx, |restarted, cx| {
                restarted.set_storage(storage.clone(), cx);
                let runs = shell_runs(restarted, restarted.active);
                assert_eq!(runs.len(), 1);
                assert_eq!(runs[0].command.as_ref(), "echo saved");
                assert!(!runs[0].pending, "再起動を越えては添えない");
                (runs[0].status.clone(), runs[0].stdout.clone())
            })
        };
        assert_eq!(
            restore(cx),
            (
                ShellStatus::Interrupted { detached: false },
                SharedString::default()
            ),
            "走っている途中で終わった物は中断として読める"
        );

        cx.run_until_parked();
        assert_eq!(
            restore(cx),
            (ShellStatus::Exited(Some(0)), SharedString::from("saved")),
            "終わった結果が同じ行に書き戻る"
        );
        let turns = storage.load_recent_turns(&thread_id, 10).expect("読める");
        assert_eq!(turns.len(), 1, "書き換えで行は増えない");
        assert_eq!(turns[0].0, "shell");
    }

    /// Chat のスレッド: フォルダがあればそこで走らせる。無ければ走らせず、本文も残す（ヒントが案内する）。
    #[cfg(unix)]
    #[gpui::test]
    fn a_chat_runs_in_its_folder_or_explains_why_it_cannot(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "chat");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new_chat(Theme::dark(), cx));
        panel.update(cx, |panel, cx| type_and_submit(panel, "!ls", cx));
        let folder = fixture.root.join("chats").join("相談");
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            assert!(
                shell_runs(panel, active).is_empty(),
                "フォルダが無ければ走らせない"
            );
            assert_eq!(panel.composer_text(cx), "!ls", "本文は残す");
            assert!(panel.shell_input, "ヒントを出している");
            assert_eq!(
                panel.shell_unavailable_reason(),
                Some(i18n::t!("agent.shell_hint_no_folder"))
            );

            // 最初の送信でできるフォルダ（空で片付けられていても作り直す）。
            panel.threads[active].chat = Some(chat::ChatThreadState {
                dir: Some(folder.clone()),
                ..Default::default()
            });
            assert_eq!(panel.shell_unavailable_reason(), None);
            type_and_submit(panel, "!pwd -P", cx);
            assert!(folder.is_dir());
        });
        cx.run_until_parked();
        let canonical = std::fs::canonicalize(&folder).expect("フォルダ");
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                shell_runs(panel, panel.active)[0].stdout.as_ref(),
                canonical.to_string_lossy()
            );
        });
    }
}
