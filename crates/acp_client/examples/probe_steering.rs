//! **実エージェントへの差し込み（`_session/steering`）の通し確認**（ARCHITECTURE §7.6）。
//!
//! 窓を出さずに、necoder と同じ部品（`acp_client::run_session_on`）で 1 ターン回す: 使い捨てのフォルダで
//! 「`sleep 6` を走らせてから ALPHA とだけ答えて」と頼み、シェルの手順が始まったら `Steer`
//! （「BRAVO に変えて」）を送る。確かめること:
//! - initialize の広告（`SessionStarted.steerable`）
//! - 差し込みの結果（`Injected` のはず）と、`session/cancel` を出していないこと（このプローブは送らない）
//! - ターンが 1 本のまま閉じること（zeron が Claude / Codex のアダプタを捨てた「ターンを開いたままにする」
//!   になっていないか）と、閉じるまでの時間
//! - 答えが BRAVO（差し込みが効いた）
//!
//! 使い方: `cargo run -p acp_client --example probe_steering -- "Claude Code" haiku`
//! 引数: エージェントの表示名・モデル・思考量（後ろ 2 つは省略可。広告の表示名か value_id。課金を小さく
//! するため軽いモデルと低い思考量を渡す）。課金されるので CI では回さない。シェルの許可は「今回だけ許可」
//! で答える（頼むのは `sleep 6` だけ）。
//!
//! アダプタは necoder と同じ解決（公開レジストリのキャッシュ → 管理導入）で選ぶ。特定の版を確かめる時は
//! `PROBE_ADAPTER=<アダプタの実行ファイル>`（例: npx キャッシュの `node_modules/.bin/codex-acp`）で
//! 差し替える — 解決に任せるとまだ無い版を管理導入（アプリの置き場への npm install）してしまう。
use acp_client::{AgentEvent, PermissionKind, SessionCommand, SessionPreferences, SteerOutcome};
use futures::StreamExt as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// ターンの開始から終わりまでの上限（ここを超えたら「閉じなかった」として報告する）。
const TURN_TIMEOUT: Duration = Duration::from_secs(240);
/// シェルの手順が始まらないまま、ここまで来たら差し込む（モデルがツールを使わなかった時の保険）。
const STEER_FALLBACK: Duration = Duration::from_secs(20);
/// シェルの手順が始まってから差し込むまでの間（`sleep 6` の最中に渡す）。
const STEER_INTO_TOOL: Duration = Duration::from_millis(1500);
/// ターンが閉じた後、余計なターンや更新が来ないかを見張る時間。
const QUIET_AFTER_END: Duration = Duration::from_secs(5);

const PROMPT: &str = "Use your shell tool to run exactly `sleep 6` (nothing else). \
When it finishes, reply with exactly one word: ALPHA.";
const STEER: &str =
    "Change of plan: when the command finishes, reply with exactly one word: BRAVO \
(not ALPHA).";

fn main() {
    let mut args = std::env::args().skip(1);
    let label = args.next().unwrap_or_else(|| "Claude Code".to_string());
    let model = args.next();
    let effort = args.next();
    let scratch = std::env::temp_dir().join(format!("necoder-steer-probe-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("使い捨てのフォルダを作れない");

    let host = host::LocalHost::shared();
    let kind = acp_client::AgentKind::by_label(&label).expect("未知のエージェント");
    let adapter = std::env::var("PROBE_ADAPTER")
        .ok()
        .map(|command| acp_client::AgentOverride {
            command: Some(command),
            ..acp_client::AgentOverride::default()
        });
    let command = kind
        .resolve_command_on(
            host.as_ref(),
            scratch.clone(),
            adapter.as_ref(),
            acp_client::registry::load_cached().as_ref(),
        )
        .expect("解決に失敗")
        .expect("エージェントが導入されていない");
    println!("起動: {} {:?}", command.path.display(), command.args);
    println!("作業フォルダ: {}", scratch.display());

    let preferences = SessionPreferences {
        model,
        effort,
        ..SessionPreferences::default()
    };
    let (command_tx, command_rx) = futures::channel::mpsc::unbounded();
    let (event_tx, mut event_rx) = futures::channel::mpsc::unbounded();
    let session = std::thread::spawn(move || {
        if let Err(error) = futures::executor::block_on(acp_client::run_session_on(
            Arc::clone(&host),
            command,
            preferences,
            command_rx,
            event_tx,
        )) {
            eprintln!("セッションが終わった: {error:#}");
        }
    });
    command_tx
        .unbounded_send(SessionCommand::Prompt(PROMPT.to_string()))
        .expect("送信路");

    let mut report = Report::default();
    futures::executor::block_on(async {
        let started = Instant::now();
        loop {
            let deadline = match (report.turn_ended, report.turn_started) {
                (Some(ended), _) => ended + QUIET_AFTER_END,
                (None, Some(turn_started)) => turn_started + TURN_TIMEOUT,
                (None, None) => started + TURN_TIMEOUT,
            };
            // 保険: シェルの手順が始まらなくても、ターン中なら一定時間で差し込む。
            if report.steered_at.is_none()
                && report.turn_ended.is_none()
                && report
                    .turn_started
                    .is_some_and(|at| at.elapsed() > STEER_FALLBACK)
            {
                report.steer(&command_tx, "（保険: シェルの手順が始まらなかった）");
            }
            let steer_due = report
                .tool_running_since
                .filter(|_| report.steered_at.is_none() && report.turn_ended.is_none())
                .map(|since| since + STEER_INTO_TOOL);
            let wake = steer_due.map_or(deadline, |due| due.min(deadline));
            let Some(event) = next_before(&mut event_rx, wake).await else {
                if steer_due.is_some_and(|due| Instant::now() >= due) {
                    report.steer(&command_tx, "（シェルの手順が走っている最中）");
                    continue;
                }
                break;
            };
            match event {
                AgentEvent::SessionStarted {
                    session_id,
                    steerable,
                    ..
                } => {
                    println!("[session] {session_id} steerable={steerable}");
                    report.steerable = Some(steerable);
                }
                AgentEvent::Configs(configs) => {
                    for config in configs {
                        let choices: Vec<&str> =
                            config.choices.iter().map(|(id, _)| id.as_str()).collect();
                        println!(
                            "[config {:?}] current={} choices={choices:?}",
                            config.category, config.current
                        );
                    }
                }
                AgentEvent::TurnStarted => {
                    report.turns_started += 1;
                    report.turn_started.get_or_insert_with(Instant::now);
                    println!("[turn started] #{}", report.turns_started);
                }
                AgentEvent::ToolStarted(info) => {
                    // シェルの手順が始まったら、少し待ってから差し込む（`sleep 6` が走っている最中に
                    // 渡したい。Claude はここではまだ呼び出しを書いている途中で、走り出すのは ~0.3 秒後。
                    // Codex は走っている間の更新を送らないので、始まりを起点にする）。
                    if info.kind == Some(acp_client::ToolCallKind::Execute)
                        && report.tool_running_since.is_none()
                    {
                        report.tool_running_since = Some(Instant::now());
                    }
                    let title = info.title.unwrap_or_default();
                    println!(
                        "[tool] {} {title} {:?} {}",
                        info.id,
                        info.kind,
                        report.since_turn_start()
                    );
                }
                AgentEvent::ToolUpdated(info)
                    if info.completed.is_some() || info.output.is_some() =>
                {
                    let output = info.output.unwrap_or_default();
                    println!(
                        "[tool update] {} completed={:?} {} output={:?}",
                        info.id,
                        info.completed,
                        report.since_turn_start(),
                        output.chars().take(120).collect::<String>()
                    );
                }
                AgentEvent::PermissionRequest {
                    title,
                    options,
                    respond,
                    ..
                } => {
                    let allow = options
                        .iter()
                        .position(|option| option.kind == PermissionKind::Allow)
                        .unwrap_or(0);
                    println!("[permission] {title} → {}", options[allow].label);
                    respond.unbounded_send(allow).ok();
                }
                AgentEvent::AgentChunk(text) => {
                    if report.steer_outcome.is_some() {
                        report.text_after_steer.push_str(&text);
                    }
                    report.text.push_str(&text);
                }
                AgentEvent::Steered { id, outcome } => {
                    println!(
                        "[steered] id={id} {outcome:?} {}",
                        report.since_turn_start()
                    );
                    report.steer_outcome = Some(outcome);
                    report.steer_outcome_at = Some(Instant::now());
                }
                AgentEvent::TurnEnded { reason } => {
                    println!("[turn ended] {reason:?} {}", report.since_turn_start());
                    report.turn_end_reason = Some(reason);
                    report.turn_ended = Some(Instant::now());
                }
                AgentEvent::Failed(error) => {
                    println!("[failed] {error}");
                    report.failed = Some(error);
                    report.turn_ended.get_or_insert_with(Instant::now);
                }
                AgentEvent::Notice(notice) => println!("[notice] {notice}"),
                AgentEvent::SessionLost => {
                    println!("[session lost]");
                    break;
                }
                _ => {}
            }
        }
    });
    drop(command_tx);
    if session.join().is_err() {
        eprintln!("セッションのスレッドが panic した");
    }
    report.print();
    if let Err(error) = std::fs::remove_dir_all(&scratch) {
        eprintln!("使い捨てのフォルダを消せない: {error}");
    }
}

/// `deadline` までに次のイベントを待つ（来なければ `None`）。
async fn next_before(
    event_rx: &mut futures::channel::mpsc::UnboundedReceiver<AgentEvent>,
    deadline: Instant,
) -> Option<AgentEvent> {
    use futures::future::{select, Either};
    let timer = async_io::Timer::at(deadline);
    let next = event_rx.next();
    futures::pin_mut!(timer, next);
    match select(next, timer).await {
        Either::Left((event, _)) => event,
        Either::Right(_) => None,
    }
}

#[derive(Default)]
struct Report {
    steerable: Option<bool>,
    turns_started: usize,
    turn_started: Option<Instant>,
    /// シェルの手順が始まった時刻（最初の実行系の `ToolStarted`）。
    tool_running_since: Option<Instant>,
    steered_at: Option<Instant>,
    steer_outcome: Option<SteerOutcome>,
    steer_outcome_at: Option<Instant>,
    turn_end_reason: Option<acp_client::TurnEnd>,
    turn_ended: Option<Instant>,
    failed: Option<String>,
    text: String,
    text_after_steer: String,
}

impl Report {
    fn steer(
        &mut self,
        command_tx: &futures::channel::mpsc::UnboundedSender<SessionCommand>,
        when: &str,
    ) {
        println!("[steer] {when} {}", self.since_turn_start());
        self.steered_at = Some(Instant::now());
        command_tx
            .unbounded_send(SessionCommand::Steer {
                id: 1,
                text: STEER.to_string(),
                images: Vec::new(),
            })
            .expect("送信路");
    }

    fn since_turn_start(&self) -> String {
        self.turn_started
            .map(|at| format!("+{:.1}s", at.elapsed().as_secs_f64()))
            .unwrap_or_default()
    }

    fn print(&self) {
        let between = |from: Option<Instant>, to: Option<Instant>| match (from, to) {
            (Some(from), Some(to)) => {
                format!("{:.1}s", to.saturating_duration_since(from).as_secs_f64())
            }
            _ => "—".to_string(),
        };
        println!("\n===== 差し込みの通し確認 =====");
        println!("広告（steerable）: {:?}", self.steerable);
        println!("送ったターン: {}（1 本のままのはず）", self.turns_started);
        println!("session/cancel: 送っていない（このプローブは Cancel を送らない）");
        println!(
            "差し込みの結果: {:?}（送ってから {}）",
            self.steer_outcome,
            between(self.steered_at, self.steer_outcome_at)
        );
        println!(
            "ターンの終わり: {:?}（差し込みから {}・開始から {}）",
            self.turn_end_reason,
            between(self.steered_at, self.turn_ended),
            between(self.turn_started, self.turn_ended)
        );
        if let Some(error) = &self.failed {
            println!("失敗: {error}");
        }
        println!("差し込みの後の本文: {:?}", self.text_after_steer.trim());
        println!("本文の全部: {:?}", self.text.trim());
        let closed = self.turn_end_reason.is_some();
        let effective = self.text.contains("BRAVO");
        println!(
            "判定: ターンが閉じた={closed} / 差し込みが効いた（BRAVO）={effective} / 余計なターン={}",
            self.turns_started > 1
        );
    }
}
