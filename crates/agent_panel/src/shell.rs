//! shell — composer の `!` でシェルコマンドを直に実行する（Claude Code CLI の bash モード相当・#37）。
//!
//! `!` は CLI の画面側の機能で、ACP にも claude-agent-acp にも無い（`session/prompt` に
//! `!git status` を流すと、ただの発話としてモデルが読んでターンとトークンを使う）。
//! だから necoder が自分で実行する: 宛先の cwd（Remote SSH ならリモート側・Chat はチャットの
//! フォルダ）で走らせ、出力を transcript に出し、**次にエージェントへ送る 1 通**の先頭へ
//! `<bash-input>` / `<bash-stdout>` として添える（CLI が会話へ差し込むのと同じ形＝Claude は
//! 「ユーザーが手で打ったコマンドとその出力」と読む）。実行しただけではエージェントを起こさない。
//!
//! 対話はできない（stdin は即 EOF）。時間では打ち切らない — 人が打ったコマンドで、ビルドやテストは
//! 数分かかるのが普通だから。止めるのは停止ボタン（スレッドを閉じた時・パネルが消えた時も止める）。
//! 実行中も出力の末尾を流して見せる（終わるまで無言にしない）。止める時はシェルが起動した子・孫まで止める。
//! stderr はシェル側で stdout へ寄せる（`HostProcess` は stdout しか持たない。順序も端末と同じになる）。

use super::*;
use host::{CommandSpec, HostProcess};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 終わったか・出力が増えたかを見に行く最初の間隔（短いコマンドを待たせない）。
const SHELL_FIRST_POLL: Duration = Duration::from_millis(25);
/// 走り続けるコマンドの出力を transcript へ流す間隔（見に行く間隔はここまで伸ばす）。
const SHELL_LIVE_INTERVAL: Duration = Duration::from_millis(400);
/// 実行中に見せる出力の末尾の行数（全部は終わってから）。
const SHELL_LIVE_TAIL_LINES: usize = 8;
/// 終了を見に行く間隔。
const SHELL_POLL: Duration = Duration::from_millis(30);
/// 子が終わった後、出力の読み残しを待つ上限（バックグラウンドへ逃げた孫が stdout を握り続けても
/// ここで見切る＝結果の表示を待たせない）。
const SHELL_READER_GRACE: Duration = Duration::from_millis(500);
/// 手元に溜める出力の上限。超えた分は読み捨てる（パイプを詰まらせて子を止めないため読むのは続ける）。
const SHELL_OUTPUT_MAX_BYTES: usize = 4 * 1024 * 1024;
/// エージェントへ添える出力の上限（文字数）。Claude Code の Bash ツールが出力を丸める幅と同じ。
const SHELL_CONTEXT_MAX_CHARS: usize = 30_000;

/// `!` 実行の終わり方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellStatus {
    Running,
    /// 終了した。終了コードはシグナルで落ちた場合 `None`。
    Exited(Option<i32>),
    Stopped,
    /// 起動できなかった（出力欄に理由を入れる）。
    Failed,
}

/// エージェントへまだ渡していない `!` の出力（次の送信で添える）。
#[derive(Debug, Clone)]
pub(crate) struct ShellContext {
    /// 添付行のチップに出すコマンド。
    pub command: SharedString,
    /// prompt の先頭へ足す本文（`<bash-input>` … の塊・末尾改行つき）。
    pub block: String,
}

/// 送信する本文が `!` で始まっていれば、`!` の後ろ（前後の空白を除く）を返す。
/// 空文字（`!` だけ）もそのまま返す — 呼び出し側は「何もしない」を選ぶ（エージェントへ流さない）。
pub(crate) fn shell_input(prompt: &str) -> Option<&str> {
    prompt.trim().strip_prefix('!').map(str::trim)
}

/// 出力の 1 行目に載せる「このシェルの PID」の目印（打ち切り時に子ごと止めるため・表示からは外す）。
const PID_MARKER: &str = "\u{1e}NECODER_SHELL_PID=";

/// stderr を stdout へ寄せて、利用者のコマンドを流すスクリプト。
/// POSIX は先頭で自分の PID を目印付きで出し（`exec` で置き換わっても PID は変わらない）、
/// `exec 2>&1` で以後の全コマンド（複数行・パイプ）の stderr を寄せる。
/// `cmd.exe` にはどちらも無いので、括弧で包んだ全体に `2>&1` を掛ける（PID は手元の子から取る）。
fn merged_script(command: &str, posix: bool) -> String {
    if posix {
        format!("printf '{PID_MARKER}%s\\n' \"$$\"\nexec 2>&1\n{command}")
    } else {
        format!("({command}) 2>&1")
    }
}

/// 出力の先頭から PID の目印行を外す（無ければそのまま）。
fn split_pid_marker(bytes: &[u8]) -> (Option<u32>, &[u8]) {
    let Some(rest) = bytes.strip_prefix(PID_MARKER.as_bytes()) else {
        return (None, bytes);
    };
    let Some(newline) = rest.iter().position(|byte| *byte == b'\n') else {
        return (None, bytes); // まだ 1 行目を読み切っていない
    };
    let pid = std::str::from_utf8(&rest[..newline])
        .ok()
        .and_then(|pid| pid.trim().parse().ok());
    (pid, &rest[newline + 1..])
}

/// 打ち切り時に、シェルが起動した子・孫まで止めるスクリプト（POSIX・リモートでも同じ）。
/// `kill` だけだとシェル本体しか死なず、`npm run dev` のような子が裏で動き続ける。
/// 親を先に STOP して新しい子を産ませず、子から順に KILL する。`pgrep` が無い環境では本体だけ止まる。
fn kill_tree_script(pid: u32) -> String {
    format!(
        "kill_tree() {{ kill -STOP \"$1\" 2>/dev/null; for child in $(pgrep -P \"$1\" 2>/dev/null); do kill_tree \"$child\"; done; kill -KILL \"$1\" 2>/dev/null; }}\nkill_tree {pid}"
    )
}

/// 走っているシェルを子ごと止める。POSIX は目印の PID から（リモートでも同じホストで実行する）、
/// Windows ローカルは `taskkill /T` で手元の子のツリーごと。
fn kill_tree(host: &dyn Host, cwd: &Path, process: &HostProcess, marker_pid: Option<u32>) {
    let spec = if host.has_posix_shell() {
        let Some(pid) = marker_pid else {
            return; // 目印が届く前（起動直後）＝子もまだ居ない。drop の kill に任せる
        };
        host.shell_script(&kill_tree_script(pid), cwd)
    } else {
        CommandSpec::new("taskkill", cwd).args(["/T", "/F", "/PID", &process.id().to_string()])
    };
    if let Err(error) = host.run_command(&spec) {
        eprintln!("`!` の子プロセスを止められない: {error:#}");
    }
}

/// 端末向けの制御（色の ESC シーケンス・`\r` で上書きする進捗表示）を落として、読める文字だけにする。
fn clean_output(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            text.push(character);
            continue;
        }
        match chars.next() {
            // CSI: `ESC [` … 終端（0x40〜0x7E）。
            Some('[') => {
                for next in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            // OSC: `ESC ]` … BEL か `ESC \`。
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // その他の 2 文字シーケンスは丸ごと落とす。
            _ => {}
        }
    }
    // `\r` だけの改行は「同じ行を書き直す」＝最後に書いた内容だけ残す（`\r\n` は普通の改行）。
    text.split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            line.rsplit('\r').next().unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 文字数で上限を超えたら、真ん中を省略して頭と尻を残す。
fn clip_middle(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let half = max_chars / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(count - half).collect();
    format!("{head}\n… ({} chars omitted) …\n{tail}", count - half * 2)
}

/// 次の送信の先頭へ添える塊。CLI の bash モードが会話へ差し込む形と同じタグを使う。
pub(crate) fn context_block(command: &str, output: &str, status: ShellStatus) -> String {
    let output = clip_middle(output, SHELL_CONTEXT_MAX_CHARS);
    let note = match status {
        ShellStatus::Running | ShellStatus::Exited(Some(0)) => String::new(),
        ShellStatus::Exited(Some(code)) => format!("(exit code {code})\n"),
        ShellStatus::Exited(None) => "(terminated by a signal)\n".to_string(),
        ShellStatus::Stopped => "(stopped by the user before finishing)\n".to_string(),
        ShellStatus::Failed => "(could not be started)\n".to_string(),
    };
    format!("<bash-input>{command}</bash-input>\n<bash-stdout>{output}</bash-stdout><bash-stderr></bash-stderr>\n{note}")
}

/// 実行して終わるまで待つ（**blocking**・専用スレッドで呼ぶ。GPUI の background executor は
/// 数本しかないので、何分も走るコマンドで塞がない）。出力は `collected` へ溜まり続ける
/// （実行中の表示は呼び出し側がそこを覗く）。戻り値は整形済みの出力全体と終わり方。
pub(crate) fn run_blocking(
    host: &dyn Host,
    cwd: &Path,
    command: &str,
    stop: &AtomicBool,
    collected: Arc<Mutex<Collected>>,
) -> (String, ShellStatus) {
    let spec = host.shell_script(&merged_script(command, host.has_posix_shell()), cwd);
    let mut process = match host.spawn_process(&spec) {
        Ok(process) => process,
        Err(error) => return (format!("{error:#}"), ShellStatus::Failed),
    };
    // stdin を閉じる＝入力を待つコマンドは EOF で抜ける（対話はできない）。
    match process.take_stdin() {
        Ok(stdin) => drop(stdin),
        Err(error) => eprintln!("`!` の stdin を閉じられない: {error:#}"),
    }
    let stdout = match process.take_stdout() {
        Ok(stdout) => stdout,
        Err(error) => return (format!("{error:#}"), ShellStatus::Failed),
    };
    let reader = spawn_reader(stdout, collected.clone());

    let mut failure = None;
    let status = loop {
        match process.try_wait() {
            Ok(Some(exit)) => break ShellStatus::Exited(exit.code()),
            Ok(None) => {}
            Err(error) => {
                failure = Some(format!("{error:#}"));
                break ShellStatus::Failed;
            }
        }
        if stop.load(Ordering::Acquire) {
            break ShellStatus::Stopped;
        }
        std::thread::sleep(SHELL_POLL);
    };
    if status == ShellStatus::Stopped {
        let marker_pid = {
            let collected = collected
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            split_pid_marker(&collected.bytes).0
        };
        kill_tree(host, cwd, &process, marker_pid);
    }
    // 止めたならここで本体も kill される（終わっていれば回収するだけ）。
    drop(process);
    let deadline = Instant::now() + SHELL_READER_GRACE;
    while !reader.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if reader.is_finished() && reader.join().is_err() {
        eprintln!("`!` の出力を読むスレッドが panic した");
    }
    // 見切った場合、読み手は孫が stdout を閉じるまで裏で読み続けて自然に終わる（ここでは待たない）。

    let output = failure.unwrap_or_else(|| collected_text(&collected));
    (output, status)
}

/// 読み取り中の出力（実行スレッドと表示側が共有する）。
#[derive(Default)]
pub(crate) struct Collected {
    bytes: Vec<u8>,
    /// 上限を超えて読み捨てた。
    clipped: bool,
}

/// ここまでに届いた出力を、表示できる文字にして返す（PID の目印は外す）。
fn collected_text(collected: &Mutex<Collected>) -> String {
    let collected = collected
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut output = clean_output(&String::from_utf8_lossy(
        split_pid_marker(&collected.bytes).1,
    ));
    if collected.clipped {
        output.push_str(&format!("\n{}", i18n::t!("agent.shell_output_clipped")));
    }
    output
}

/// 届いたバイト数（増えた時だけ実行中の表示を更新するため）。
fn collected_len(collected: &Mutex<Collected>) -> usize {
    collected
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .bytes
        .len()
}

/// 実行中の `!`（停止の宛先。どのスレッドのものかを持つ＝閉じたスレッドの分だけ止められる）。
pub(crate) struct ShellRun {
    thread_id: String,
    stop: Arc<AtomicBool>,
}

fn spawn_reader(
    mut stdout: Box<dyn Read + Send>,
    collected: Arc<Mutex<Collected>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            let read = match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break, // kill で閉じられた＝ここまでで終わり
            };
            let mut collected = collected
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let room = SHELL_OUTPUT_MAX_BYTES.saturating_sub(collected.bytes.len());
            if read > room {
                collected.clipped = true;
            }
            collected.bytes.extend_from_slice(&chunk[..read.min(room)]);
        }
    })
}

/// storage の 1 行へ（role `shell`）。コマンドと出力に改行もタブも入りうるので JSON で持つ。
pub(crate) fn encode_turn(
    command: &str,
    output: Option<&str>,
    output_lines: usize,
    status: ShellStatus,
) -> String {
    let (status, code) = match status {
        ShellStatus::Running => ("running", None),
        ShellStatus::Exited(code) => ("exited", code),
        ShellStatus::Stopped => ("stopped", None),
        ShellStatus::Failed => ("failed", None),
    };
    serde_json::json!({
        "command": command,
        "output": output,
        "lines": output_lines,
        "status": status,
        "code": code,
    })
    .to_string()
}

/// [`encode_turn`] の逆。読めない行はコマンドとして出すだけにする（履歴を落とさない）。
pub(crate) fn decode_turn(content: &str) -> Entry {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Entry::Shell {
            run: None,
            command: content.to_string().into(),
            output: None,
            output_lines: 0,
            status: ShellStatus::Exited(Some(0)),
        };
    };
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let code = value
        .get("code")
        .and_then(|v| v.as_i64())
        .and_then(|code| i32::try_from(code).ok());
    let status = match text("status").as_deref() {
        Some("stopped") => ShellStatus::Stopped,
        Some("failed") => ShellStatus::Failed,
        // `running` は書かない（persist が手前で止まる）。来たら途中で落ちた＝終わり方不明。
        Some("running") => ShellStatus::Exited(None),
        _ => ShellStatus::Exited(code),
    };
    Entry::Shell {
        run: None,
        command: text("command").unwrap_or_default().into(),
        output: text("output").map(SharedString::from),
        output_lines: value
            .get("lines")
            .and_then(|v| v.as_u64())
            .and_then(|lines| usize::try_from(lines).ok())
            .unwrap_or(0),
        status,
    }
}

/// 実行 id から transcript 上の `!` 行を引く。
fn shell_entry_index(entries: &[Entry], run: u64) -> Option<usize> {
    entries
        .iter()
        .position(|entry| matches!(entry, Entry::Shell { run: Some(id), .. } if *id == run))
}

/// transcript の状態表示（成功は何も出さない）。
fn status_label(status: ShellStatus) -> Option<String> {
    match status {
        ShellStatus::Running => Some(i18n::t!("agent.shell_running")),
        ShellStatus::Exited(Some(0)) => None,
        ShellStatus::Exited(Some(code)) => Some(i18n::t!("agent.shell_exit", "code" => code)),
        ShellStatus::Exited(None) => Some(i18n::t!("agent.shell_signal")),
        ShellStatus::Stopped => Some(i18n::t!("agent.shell_stopped")),
        ShellStatus::Failed => Some(i18n::t!("agent.shell_failed")),
    }
}

/// ローカルのパスをホーム起点（`~/…`）で短く見せる。
fn tilde_path(path: &Path) -> String {
    match paths::home_dir().and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(relative) if relative.as_os_str().is_empty() => "~".to_string(),
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    }
}

impl AgentPanel {
    /// composer の通知ごとに、先頭 `!`（シェルモード）かどうかとキャレット色を見直す。
    /// スレッド切替などがキャレットをスレッド色へ塗り直しても、ここで syn-mac に戻る。
    pub(crate) fn sync_composer_shell(&mut self, cx: &mut Context<Self>) {
        let composer = self.composer.read(cx);
        let shell = composer.first_visible_char() == Some('!');
        let accent = if shell {
            self.theme.syntax.macro_
        } else {
            self.active_color()
        };
        if composer.accent() != accent {
            self.composer
                .update(cx, |composer, cx| composer.set_accent(accent, cx));
        }
        if shell != self.composer_shell {
            self.composer_shell = shell;
            cx.notify();
        }
    }

    /// シェルモードの宛先チップに出す実行場所（`~/path`・リモートは `host:path`）。
    pub(crate) fn shell_place(&self) -> String {
        if self.chat_mode {
            return self
                .threads
                .get(self.active)
                .and_then(|thread| thread.chat.as_ref())
                .and_then(|chat| chat.dir.as_deref())
                .map(tilde_path)
                .unwrap_or_else(|| i18n::t!("agent.shell_place_chat"));
        }
        match &self.dest_cwd {
            Some(cwd) if self.dest_host.is_remote() => {
                format!("{}:{}", self.dest_host.display_name(), cwd.display())
            }
            Some(cwd) => tilde_path(cwd),
            None => String::new(),
        }
    }

    /// `!コマンド` を実行する（submit から）。エージェントの生成中でも待たない（送信待ちの列に積まない）。
    pub(crate) fn run_shell(&mut self, command: String, cx: &mut Context<Self>) {
        let thread_index = self.active;
        let Some(thread_id) = self
            .threads
            .get(thread_index)
            .map(|thread| thread.id.clone())
        else {
            return;
        };
        // 実行場所。Chat は（ローカルの）チャットのフォルダ — 最初の 1 通より先なら、ここで作る。
        let place: Result<(Arc<dyn Host>, PathBuf), String> = if self.chat_mode {
            self.ensure_chat_dir(thread_index, &command, cx)
                .map(|dir| (LocalHost::shared(), dir))
        } else {
            self.dest_cwd
                .clone()
                .map(|cwd| (self.dest_host.clone(), cwd))
                .ok_or_else(|| i18n::t!("agent.err_no_project"))
        };
        let (host, cwd) = match place {
            Ok(place) => place,
            Err(message) => {
                if let Some(thread) = self.threads.get_mut(thread_index) {
                    thread.entries.push(Entry::Shell {
                        run: None,
                        command: command.into(),
                        output: Some(message.into()),
                        output_lines: 1,
                        status: ShellStatus::Failed,
                    });
                }
                cx.notify();
                return;
            }
        };
        if self.chat_mode {
            self.persist_thread(thread_index);
            self.refresh_chat_rows(cx);
        }
        let run = self.next_shell_run;
        self.next_shell_run += 1;
        let stop = Arc::new(AtomicBool::new(false));
        self.shell_stops.insert(
            run,
            ShellRun {
                thread_id: thread_id.clone(),
                stop: stop.clone(),
            },
        );
        if let Some(thread) = self.threads.get_mut(thread_index) {
            thread.entries.push(Entry::Shell {
                run: Some(run),
                command: command.clone().into(),
                output: None,
                output_lines: 0,
                status: ShellStatus::Running,
            });
            thread.last_active_at_ms = now_unix_ms();
        }
        cx.notify();
        let collected = Arc::new(Mutex::new(Collected::default()));
        let (done_tx, mut done_rx) = futures::channel::oneshot::channel();
        let runner_collected = collected.clone();
        std::thread::spawn(move || {
            let result = run_blocking(host.as_ref(), &cwd, &command, &stop, runner_collected);
            // 受け手（パネル）が先に消えていれば、結果の行き先は無い。
            done_tx.send(result).ok();
        });
        // 終わりは「見に行く」（実行スレッドから GPUI のタスクを起こさない）。短いコマンドを待たせない
        // よう間隔は短く始めて、走り続けるものほど [`SHELL_LIVE_INTERVAL`] まで伸ばす。
        cx.spawn(async move |panel, cx| {
            let mut shown_len = 0;
            let mut interval = SHELL_FIRST_POLL;
            loop {
                cx.background_executor().timer(interval).await;
                interval = (interval * 2).min(SHELL_LIVE_INTERVAL);
                match done_rx.try_recv() {
                    Ok(None) => {}
                    finished => {
                        let (output, status) = finished.ok().flatten().unwrap_or_else(|| {
                            // 実行スレッドが結果を返さずに落ちた。
                            (i18n::t!("agent.shell_failed"), ShellStatus::Failed)
                        });
                        // パネルが閉じていれば表示先が無い＝結果は捨てる。
                        panel
                            .update(cx, |panel, cx| {
                                panel.finish_shell(&thread_id, run, output, status, cx)
                            })
                            .ok();
                        return;
                    }
                }
                let len = collected_len(&collected);
                if len == shown_len {
                    continue;
                }
                shown_len = len;
                let text = collected_text(&collected);
                let shown = panel.update(cx, |panel, cx| {
                    panel.show_shell_progress(&thread_id, run, &text, cx)
                });
                if shown.is_err() {
                    return; // パネルが消えた（実行は on_release の停止で止まる）
                }
            }
        })
        .detach();
    }

    /// 実行中の出力を `!` 行へ流す（末尾だけ見せる。全文は終わってから）。
    fn show_shell_progress(
        &mut self,
        thread_id: &str,
        run: u64,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let active = self
            .threads
            .get(self.active)
            .map(|thread| thread.id.clone());
        let Some(thread) = self.shell_thread_mut(thread_id) else {
            return;
        };
        let Some(entry_index) = shell_entry_index(&thread.entries, run) else {
            return;
        };
        let lines: Vec<&str> = text.lines().collect();
        let tail = lines[lines.len().saturating_sub(SHELL_LIVE_TAIL_LINES)..].join("\n");
        if let Entry::Shell {
            output,
            output_lines,
            ..
        } = &mut thread.entries[entry_index]
        {
            *output = (!tail.is_empty()).then(|| SharedString::from(tail));
            *output_lines = lines.len();
        }
        if active.as_deref() == Some(thread_id) {
            self.transcript_list
                .remeasure_items(entry_index..entry_index + 1);
            cx.notify();
        }
    }

    /// `!` を走らせたスレッド（開いているもの・閉じて復元待ちのもの両方から探す）。
    fn shell_thread_mut(&mut self, thread_id: &str) -> Option<&mut Thread> {
        self.threads
            .iter_mut()
            .chain(self.closed_threads.iter_mut())
            .find(|thread| thread.id == thread_id)
    }

    /// スレッドを閉じる時: そのスレッドで走っている `!` を止める（裏で走り続けさせない）。
    pub(crate) fn stop_thread_shells(&self, thread_id: &str) {
        for shell in self.shell_stops.values() {
            if shell.thread_id == thread_id {
                shell.stop.store(true, Ordering::Release);
            }
        }
    }

    /// パネルが消える時: 走っている `!` を全部止める。
    pub(crate) fn stop_all_shells(&self) {
        for shell in self.shell_stops.values() {
            shell.stop.store(true, Ordering::Release);
        }
    }

    /// 実行が終わった `!` の結果を transcript へ書き、次の送信に添える分として積む。
    fn finish_shell(
        &mut self,
        thread_id: &str,
        run: u64,
        output: String,
        status: ShellStatus,
        cx: &mut Context<Self>,
    ) {
        self.shell_stops.remove(&run);
        // 開いているスレッドの位置（閉じたスレッドなら None＝表示も保存もしない）。
        let open_index = self.thread_index_by_id(thread_id);
        let (shown, output_lines) = cap_output(&output);
        // 閉じたスレッドの分も書き換える（⌘⇧T で戻した時に「実行中…」のまま残さない）。
        let Some(thread) = self.shell_thread_mut(thread_id) else {
            return;
        };
        let Some(entry_index) = shell_entry_index(&thread.entries, run) else {
            return;
        };
        let Entry::Shell { command, .. } = &thread.entries[entry_index] else {
            return;
        };
        let command = command.clone();
        thread.shell_context.push(ShellContext {
            command: command.clone(),
            block: context_block(&command, &shown, status),
        });
        thread.entries[entry_index] = Entry::Shell {
            run: None,
            command,
            output: (!shown.is_empty()).then(|| SharedString::from(shown)),
            output_lines,
            status,
        };
        let agent_running = thread.running;
        let Some(thread_index) = open_index else {
            return;
        };
        if thread_index == self.active {
            // 出力が付いて背が伸びた＝測り直す（末尾以外は描画側が測り直さない）。
            self.transcript_list
                .remeasure_items(entry_index..entry_index + 1);
        }
        // 生成中に書くと、書きかけの本文まで確定行として残ってしまう。ターン終了の保存に任せる。
        if !agent_running {
            self.persist_thread(thread_index);
        }
        cx.notify();
    }

    fn stop_shell(&mut self, run: u64) {
        if let Some(shell) = self.shell_stops.get(&run) {
            shell.stop.store(true, Ordering::Release);
        }
    }

    /// 次の送信に添える予定の `!` の出力を外す（添付行のチップの ×）。
    fn remove_shell_context(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            if index < thread.shell_context.len() {
                thread.shell_context.remove(index);
                cx.notify();
            }
        }
    }

    /// 添える予定の `!` の出力をまとめて取り出す（送信時。取り出したら空になる）。
    pub(crate) fn take_shell_context(&mut self, thread_index: usize) -> String {
        self.threads
            .get_mut(thread_index)
            .map(|thread| {
                thread
                    .shell_context
                    .drain(..)
                    .map(|context| context.block)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `! シェル` バッジ（syn-mac 文字 + bg3 面＝インラインコードと同じ見た目）。
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

    /// 添付行に並べる「次の送信に添える `!` の出力」チップ（× で外す）。
    pub(crate) fn render_shell_context_chips(
        &self,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let theme = self.theme.clone();
        let contexts: Vec<SharedString> = self
            .threads
            .get(self.active)
            .map(|thread| {
                thread
                    .shell_context
                    .iter()
                    .map(|context| context.command.clone())
                    .collect()
            })
            .unwrap_or_default();
        contexts
            .into_iter()
            .enumerate()
            .map(|(index, command)| {
                div()
                    .id(("shell-context", index))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .max_w(px(220.))
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(theme.bg3)
                    .text_size(px(10.5))
                    .font_family("Guguru Sans Code")
                    .text_color(theme.syntax.macro_)
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(SharedString::from(format!("! {command}"))),
                    )
                    .child(
                        div()
                            .id(("shell-context-remove", index))
                            .flex_none()
                            .text_color(theme.fg2)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.fg0))
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.remove_shell_context(index, cx);
                                }),
                            ),
                    )
                    .tooltip(Tooltip::text(
                        i18n::t!("agent.shell_context_tip"),
                        theme.clone(),
                    ))
                    .into_any_element()
            })
            .collect()
    }

    /// transcript の `!` 実行行: `!` + コマンド（インラインコードの見た目）+ 状態 → `⎿ 出力`。
    pub(crate) fn render_shell_entry(
        &self,
        index: usize,
        entry: &Entry,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Entry::Shell {
            run,
            command,
            output,
            output_lines,
            status,
        } = entry
        else {
            return div().into_any_element();
        };
        let (run, output_lines, status) = (*run, *output_lines, *status);
        let theme = self.theme.clone();
        let shell_color = theme.syntax.macro_;
        let mut header = div().flex().items_start().gap(px(8.)).child(
            div()
                // 文字の幅だけ取り、長いコマンドは残り幅で折り返す（行いっぱいに面を塗らない）。
                .flex_initial()
                .min_w_0()
                .px(px(5.))
                .rounded(px(4.))
                .bg(theme.bg3)
                .font_family("Guguru Sans Code")
                .text_size(px(11.5))
                .text_color(shell_color)
                .child(self.selectable_text(command.clone(), cx)),
        );
        if let Some(label) = status_label(status) {
            header = header.child(
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(label)),
            );
        }
        if let (ShellStatus::Running, Some(run)) = (status, run) {
            header = header.child(
                div()
                    .id(("shell-stop", index))
                    .flex_none()
                    .px(px(6.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(10.5))
                    .text_color(theme.fg1)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(SharedString::from(i18n::t!("agent.shell_stop")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.stop_shell(run);
                        }),
                    ),
            );
        }
        let mut body = div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(4.))
            .child(header);
        match output {
            // 実行中は流れてくる末尾をそのまま見せる（畳まない＝進み具合が見える）。
            Some(output) if status == ShellStatus::Running => {
                body = body.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(4.))
                        .pt(px(3.))
                        .font_family("Guguru Sans Code")
                        .text_size(px(11.))
                        .text_color(theme.fg2)
                        .child(div().flex_none().child("⎿"))
                        .child(div().flex_1().min_w_0().child(output.clone())),
                );
            }
            Some(output) => {
                body = body.child(self.render_step_result(index, output, output_lines, None, cx));
            }
            None if !matches!(status, ShellStatus::Running) => {
                body = body.child(
                    div()
                        .pt(px(3.))
                        .font_family("Guguru Sans Code")
                        .text_size(px(11.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(format!(
                            "⎿ {}",
                            i18n::t!("agent.shell_no_output")
                        ))),
                );
            }
            None => {}
        }
        div()
            .flex()
            .gap(px(8.))
            .text_size(px(12.5))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(shell_color)
                    .child("!"),
            )
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_input_takes_text_after_the_bang() {
        assert_eq!(shell_input("!git status"), Some("git status"));
        assert_eq!(shell_input("  ! ls -la \n"), Some("ls -la"));
        assert_eq!(shell_input("!"), Some(""));
        assert_eq!(shell_input("git status"), None);
        assert_eq!(shell_input("これは!ではない"), None);
    }

    #[test]
    fn merged_script_sends_stderr_to_stdout() {
        assert!(merged_script("ls\nfalse", true).ends_with("\nexec 2>&1\nls\nfalse"));
        assert_eq!(merged_script("dir", false), "(dir) 2>&1");
    }

    #[test]
    fn pid_marker_is_parsed_and_hidden() {
        let bytes = format!("{PID_MARKER}4242\nhello\n");
        let (pid, rest) = split_pid_marker(bytes.as_bytes());
        assert_eq!(pid, Some(4242));
        assert_eq!(rest, b"hello\n");
        assert_eq!(split_pid_marker(b"plain"), (None, &b"plain"[..]));
    }

    #[test]
    fn clean_output_drops_colors_and_carriage_return_rewrites() {
        assert_eq!(clean_output("\u{1b}[31mred\u{1b}[0m text"), "red text");
        assert_eq!(clean_output("\u{1b}]0;title\u{7}body"), "body");
        assert_eq!(clean_output("10%\r50%\r100%\nnext"), "100%\nnext");
        assert_eq!(clean_output("windows\r\nline"), "windows\nline");
    }

    #[test]
    fn clip_middle_keeps_both_ends() {
        assert_eq!(clip_middle("short", 10), "short");
        let clipped = clip_middle(&"a".repeat(50), 10);
        assert!(clipped.starts_with("aaaaa\n"));
        assert!(clipped.ends_with("\naaaaa"));
        assert!(clipped.contains("40 chars omitted"));
    }

    #[test]
    fn context_block_uses_cli_tags_and_notes_failures() {
        let ok = context_block("git status", "clean", ShellStatus::Exited(Some(0)));
        assert_eq!(
            ok,
            "<bash-input>git status</bash-input>\n<bash-stdout>clean</bash-stdout><bash-stderr></bash-stderr>\n"
        );
        let failed = context_block("false", "", ShellStatus::Exited(Some(1)));
        assert!(failed.ends_with("(exit code 1)\n"));
        let stopped = context_block("sleep 999", "", ShellStatus::Stopped);
        assert!(stopped.ends_with("(stopped by the user before finishing)\n"));
    }

    #[test]
    fn turn_round_trips_through_storage() {
        let encoded = encode_turn("ls", Some("a\tb\nc"), 2, ShellStatus::Exited(Some(3)));
        match decode_turn(&encoded) {
            Entry::Shell {
                run,
                command,
                output,
                output_lines,
                status,
            } => {
                assert_eq!(run, None);
                assert_eq!(command.as_ref(), "ls");
                assert_eq!(output.as_deref(), Some("a\tb\nc"));
                assert_eq!(output_lines, 2);
                assert_eq!(status, ShellStatus::Exited(Some(3)));
            }
            _ => panic!("shell entry expected"),
        }
        let stopped = encode_turn("tail -f log", None, 0, ShellStatus::Stopped);
        assert!(matches!(
            decode_turn(&stopped),
            Entry::Shell {
                status: ShellStatus::Stopped,
                output: None,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn run_blocking_merges_stderr_and_reports_exit_code() {
        let host = LocalHost::shared();
        let stop = AtomicBool::new(false);
        let cwd = std::env::temp_dir();
        let (output, status) = run_blocking(
            host.as_ref(),
            &cwd,
            "echo out; echo err 1>&2; exit 3",
            &stop,
            Arc::default(),
        );
        assert_eq!(output.trim_end(), "out\nerr");
        assert_eq!(status, ShellStatus::Exited(Some(3)));
    }

    /// 別スレッドから停止を立てる（停止ボタン相当）。
    #[cfg(unix)]
    fn stop_after(stop: &Arc<AtomicBool>, delay: Duration) {
        let stop = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            stop.store(true, Ordering::Release);
        });
    }

    #[cfg(unix)]
    #[test]
    fn run_blocking_gives_eof_on_stdin_and_stops_on_request() {
        let host = LocalHost::shared();
        let stop = Arc::new(AtomicBool::new(false));
        let cwd = std::env::temp_dir();
        // stdin は即 EOF＝入力待ちで固まらない。
        let (_, status) = run_blocking(host.as_ref(), &cwd, "cat", &stop, Arc::default());
        assert_eq!(status, ShellStatus::Exited(Some(0)));
        // 時間では打ち切らない。止めるのは停止だけ。
        let started = Instant::now();
        stop_after(&stop, Duration::from_millis(300));
        let (_, status) = run_blocking(host.as_ref(), &cwd, "exec sleep 30", &stop, Arc::default());
        assert_eq!(status, ShellStatus::Stopped);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// 実行中も出力が共有バッファへ溜まっていく（実行中の表示はここを覗く）。
    #[cfg(unix)]
    #[test]
    fn output_is_visible_while_running() {
        let host = LocalHost::shared();
        let stop = Arc::new(AtomicBool::new(false));
        let cwd = std::env::temp_dir();
        let collected: Arc<Mutex<Collected>> = Arc::default();
        let runner_collected = collected.clone();
        let runner_stop = stop.clone();
        let runner = std::thread::spawn(move || {
            run_blocking(
                LocalHost::shared().as_ref(),
                &std::env::temp_dir(),
                "echo first; sleep 30",
                &runner_stop,
                runner_collected,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while collected_text(&collected).trim() != "first" && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            collected_text(&collected).trim(),
            "first",
            "終わる前に見えている"
        );
        stop.store(true, Ordering::Release);
        let (output, status) = runner.join().expect("実行スレッドが終わる");
        assert_eq!(status, ShellStatus::Stopped);
        assert_eq!(output.trim(), "first");
        drop((host, cwd));
    }

    /// 停止でシェルの子（`npm run dev` のような長生きプロセス）まで止まる。
    #[cfg(unix)]
    #[test]
    fn stopping_kills_child_processes_too() {
        let host = LocalHost::shared();
        let stop = Arc::new(AtomicBool::new(false));
        let cwd = std::env::temp_dir();
        let pid_file = cwd.join(format!("necoder-shell-child-{}", std::process::id()));
        let command = format!("sleep 37 & echo $! > '{}'; wait", pid_file.display());
        stop_after(&stop, Duration::from_millis(500));
        let (_, status) = run_blocking(host.as_ref(), &cwd, &command, &stop, Arc::default());
        assert_eq!(status, ShellStatus::Stopped);
        let child = std::fs::read_to_string(&pid_file).expect("子の PID が書かれている");
        std::fs::remove_file(&pid_file).expect("後始末");
        let alive = std::process::Command::new("kill")
            .args(["-0", child.trim()])
            .status()
            .expect("kill -0 を実行できる")
            .success();
        assert!(!alive, "子プロセス {} が生き残っている", child.trim());
    }
}
