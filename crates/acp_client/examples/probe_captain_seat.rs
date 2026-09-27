//! **実エージェントを Captain の席で動かす通し確認**（FLEET-V2 §5.8）。
//!
//! 窓を出さずに、necoder が Captain に使うのと同じ部品で 1 ターン回す:
//! `acp_client::run_session_on`（ACP）+ `session/new` の `mcpServers` に `necoder mcp --captain`
//! + 席の権限モード（Claude Code = `default` / Codex = `read-only`）+ 席と同じ裁定で許可の問いに答える
//! （読む・探す・考える・fetch と necoder の道具は許可、それ以外は拒否）。
//!
//! 確かめること: エージェントが necoder の道具で分解案を出すか・shell や編集に手を出さないか・
//! 許可の問いがどんな形で届くか（codex-acp は MCP の承認を「題名なし・種別 実行・`_meta` の印」で送る）。
//!
//! 使い方（練習用 repo と隔離した台帳を用意してから）:
//! ```sh
//! PROBE_NECODER_HOME=/tmp/lab/home PROBE_GUI_SOCK=/tmp/lab/gui.sock \
//!   cargo run -p acp_client --example probe_captain_seat -- Codex /tmp/lab/app /tmp/lab/prompt.txt
//! ```
//! 引数: エージェントの表示名・練習用 repo・prompt のファイル。necoder の本体は `NECODER_BIN`（既定は
//! `target/debug/necoder`）。課金されるので CI では回さない。練習用 repo と隔離した台帳だけを触る。
use acp_client::{AgentEvent, SessionCommand, SessionPreferences, ToolCallKind};
use futures::StreamExt as _;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TURN_TIMEOUT: Duration = Duration::from_secs(600);

/// 席の道具（`workspace::CAPTAIN_MCP_TOOLS` と同じ並び。acp_client は workspace を知らないので写す）。
const CAPTAIN_TOOLS: &[&str] = &[
    "fleet_list_tasks",
    "fleet_digest",
    "fleet_events",
    "fleet_propose_tasks",
    "fleet_spawn_agent",
    "fleet_send",
    "fleet_set_depends",
    "fleet_review_task",
    "list_files",
    "read_file",
    "search",
    "git_status",
];

/// 席と同じ裁定（`agent_panel::seat` の `seat_allows` と同じ規則）。
fn allows(kind: Option<ToolCallKind>, title: &str, mcp_tool: bool) -> bool {
    let names_a_seat_tool = CAPTAIN_TOOLS.iter().any(|tool| title.contains(tool));
    if mcp_tool {
        return names_a_seat_tool;
    }
    match kind {
        Some(ToolCallKind::Read | ToolCallKind::Search | ToolCallKind::Think | ToolCallKind::Fetch) => true,
        Some(ToolCallKind::Edit | ToolCallKind::Delete | ToolCallKind::Move | ToolCallKind::Execute) => false,
        Some(ToolCallKind::Other) | None => names_a_seat_tool,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let label = args.next().unwrap_or_else(|| "Codex".to_string());
    let repo = PathBuf::from(args.next().expect("練習用 repo のパス"));
    let prompt = std::fs::read_to_string(args.next().expect("prompt のファイル")).expect("prompt を読めない");
    let necoder = std::env::var("NECODER_BIN").unwrap_or_else(|_| "target/debug/necoder".to_string());
    let necoder = std::fs::canonicalize(&necoder).expect("necoder の本体が無い");

    let host = host::LocalHost::shared();
    let kind = acp_client::AgentKind::by_label(&label).expect("未知のエージェント");
    let command = kind
        .resolve_command_on(host.as_ref(), repo.clone(), None, acp_client::registry::load_cached().as_ref())
        .expect("解決に失敗")
        .expect("エージェントが導入されていない");
    println!("起動: {} {:?}", command.path.display(), command.args);
    let mode = match kind.id {
        "claude" => Some("default".to_string()),
        "codex" => Some("read-only".to_string()),
        _ => None,
    };
    // 隔離するのは MCP の子プロセスの台帳とソケットだけ（`PROBE_NECODER_HOME` / `PROBE_GUI_SOCK`）。
    // プローブ自身の `NECODER_HOME` を差し替えると公開レジストリのキャッシュも見えなくなり、
    // アプリと違う版のアダプタを起こしてしまう（2026-09-25 に Zed のキャッシュの古い codex-acp を拾った）。
    let mut env = BTreeMap::new();
    for (from, to) in [("PROBE_NECODER_HOME", "NECODER_HOME"), ("PROBE_GUI_SOCK", "NECODER_GUI_SOCK")] {
        if let Ok(value) = std::env::var(from) {
            env.insert(to.to_string(), value);
        }
    }
    let mut preferences = SessionPreferences {
        mode,
        ..SessionPreferences::default()
    };
    // 本番の席と同じ関数で、necoder の MCP だけを持たせる（他の MCP の持ち込みを止める）。
    acp_client::preset::restrict_mcp_to(
        &mut preferences,
        kind.id,
        vec![acp_client::mcp::McpServerConfig {
            name: "necoder".to_string(),
            source: acp_client::mcp::McpSource::Necoder,
            enabled: true,
            transport: acp_client::mcp::McpTransport::Stdio {
                command: necoder.display().to_string(),
                args: vec!["mcp".to_string(), "--captain".to_string(), repo.display().to_string()],
                env,
            },
        }],
        &acp_client::mcp::codex_configured_server_names(),
    );
    println!("mcpServers={} preset.env={:?}", preferences.mcp_servers.len(), preferences.preset.env.keys().collect::<Vec<_>>());
    let (command_tx, command_rx) = futures::channel::mpsc::unbounded();
    let (event_tx, mut event_rx) = futures::channel::mpsc::unbounded();
    std::thread::spawn(move || {
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
        .unbounded_send(SessionCommand::Prompt(prompt))
        .expect("送信路");

    let started = Instant::now();
    let mut titles: HashMap<String, String> = HashMap::new();
    let mut answer = String::new();
    futures::executor::block_on(async {
        loop {
            if started.elapsed() > TURN_TIMEOUT {
                println!("時間切れ");
                break;
            }
            let Some(event) = event_rx.next().await else {
                println!("セッションが途中で終わった");
                break;
            };
            match event {
                AgentEvent::SessionStarted { session_id, .. } => println!("[session] {session_id}"),
                AgentEvent::Modes { current, .. } => println!("[mode] {current}"),
                AgentEvent::ModeChanged(mode) => println!("[mode] → {mode}"),
                AgentEvent::ToolStarted(info) => {
                    let title = info.title.clone().unwrap_or_default();
                    println!("[tool] {:?} {title}", info.kind);
                    titles.insert(info.id.clone(), title);
                }
                AgentEvent::ToolUpdated(info) => {
                    if let Some(title) = info.title.clone().filter(|title| !title.is_empty()) {
                        titles.insert(info.id.clone(), title);
                    }
                    if let Some(completed) = info.completed {
                        let output: String = info.output.unwrap_or_default().chars().take(160).collect();
                        println!("  -> completed={completed} {output}");
                    }
                }
                AgentEvent::PermissionRequest {
                    title,
                    kind,
                    options,
                    tool_call_id,
                    mcp_tool,
                    respond,
                    ..
                } => {
                    let known = titles.get(&tool_call_id).cloned().unwrap_or_default();
                    let judged = format!("{title} {known}");
                    let allowed = allows(kind, &judged, mcp_tool);
                    println!(
                        "[permission] title={title:?} kind={kind:?} id={tool_call_id} mcp={mcp_tool} known={known:?} -> {}",
                        if allowed { "許可" } else { "拒否" }
                    );
                    let wanted = if allowed {
                        acp_client::PermissionKind::Allow
                    } else {
                        acp_client::PermissionKind::Reject
                    };
                    let choice = options
                        .iter()
                        .position(|option| option.kind == wanted)
                        .unwrap_or(if allowed { 0 } else { options.len().saturating_sub(1) });
                    respond.unbounded_send(choice).ok();
                }
                AgentEvent::AgentChunk(text) => answer.push_str(&text),
                AgentEvent::TurnEnded { reason } => {
                    println!("[turn ended] {reason:?}");
                    break;
                }
                AgentEvent::Failed(error) => {
                    println!("[failed] {error}");
                    break;
                }
                _ => {}
            }
        }
    });
    println!("\n[answer]\n{answer}");
}
