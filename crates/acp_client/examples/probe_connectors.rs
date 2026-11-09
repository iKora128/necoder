//! probe_connectors — claude.ai のコネクタの状態が毎ターン届くか、起こし直しでモデルの切り替えが
//! 会話に記録されないかを、実物の Claude で確かめる（2026-11-09 の Chat の不具合の検証用）。
//!
//! ```sh
//! cargo run -p acp_client --example probe_connectors -- <作業フォルダ> [モデル] [最初の送信までの秒]
//! ```
//!
//! 1 回目のプロセスで 2 ターン送り（ターンごとに `Connectors` が届くか）、プロセスを畳んで
//! `session/load` で起こし直してもう 1 ターン送る（アイドル停止の後の送信と同じ道）。
//! 終わったら `~/.claude/projects/…/<session>.jsonl` に `/model` の記録が無いかを見る。

use acp_client::preset::SessionPreset;
use acp_client::{AgentEvent, SessionCommand, SessionPreferences};
use futures::StreamExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TURN_TIMEOUT: Duration = Duration::from_secs(180);

fn main() {
    let mut args = std::env::args().skip(1);
    let folder = PathBuf::from(args.next().expect("作業フォルダ"));
    let model = args.next().unwrap_or_else(|| "opus".to_string());
    // 起こしてから最初の送信までの待ち（秒）。一覧が揃うまでの時間を測る。
    let first_delay: f32 = args
        .next()
        .and_then(|text| text.parse().ok())
        .unwrap_or(0.0);
    let host = host::LocalHost::shared();
    let kind = acp_client::AgentKind::by_label("Claude Code").expect("Claude Code");

    let mut session_id = None;
    for (round, prompts) in [
        (
            "起動",
            vec!["Reply with just: OK", "Reply with just: OK again"],
        ),
        ("起こし直し", vec!["Reply with just: back"]),
    ] {
        println!("== {round}");
        let command = kind
            .resolve_command_on(
                host.as_ref(),
                folder.clone(),
                None,
                acp_client::registry::load_cached().as_ref(),
            )
            .expect("解決に失敗")
            .expect("Claude が導入されていない");
        let mut env = std::collections::BTreeMap::new();
        env.insert("CLAUDE_CODE_ENABLE_CFC".to_string(), "0".to_string());
        // コネクタを取り損ねた状態を作る（起こし直しの道を通す）。
        if std::env::var_os("PROBE_NO_CONNECTORS").is_some() {
            env.insert(
                "ENABLE_CLAUDEAI_MCP_SERVERS".to_string(),
                "false".to_string(),
            );
        }
        let preferences = SessionPreferences {
            model: Some(model.clone()),
            resume: session_id.clone(),
            preset: SessionPreset {
                tools: Some(vec!["Read".to_string(), "ToolSearch".to_string()]),
                setting_sources: Some(Vec::new()),
                watch_connectors: true,
                env,
                ..SessionPreset::default()
            },
            ..SessionPreferences::default()
        };
        let (command_tx, command_rx) = futures::channel::mpsc::unbounded();
        let (event_tx, mut event_rx) = futures::channel::mpsc::unbounded();
        let session_host = Arc::clone(&host);
        let worker = std::thread::spawn(move || {
            if let Err(error) = futures::executor::block_on(acp_client::run_session_on(
                session_host,
                command,
                preferences,
                command_rx,
                event_tx,
            )) {
                eprintln!("セッションが終わった: {error:#}");
            }
        });
        let started = Instant::now();
        std::thread::sleep(Duration::from_secs_f32(first_delay));
        for prompt in prompts {
            command_tx
                .unbounded_send(SessionCommand::Prompt(prompt.to_string()))
                .expect("送信路");
            let turn_started = Instant::now();
            futures::executor::block_on(async {
                while turn_started.elapsed() < TURN_TIMEOUT {
                    let Some(event) = event_rx.next().await else {
                        println!("セッションが途中で終わった");
                        return;
                    };
                    let at = started.elapsed().as_secs_f32();
                    match event {
                        AgentEvent::SessionStarted {
                            session_id: id,
                            resumed,
                            ..
                        } => {
                            println!("[{at:5.1}s] session {id} resumed={resumed}");
                            session_id = Some(id);
                        }
                        AgentEvent::Connectors(report) => println!(
                            "[{at:5.1}s] connectors connected={} pending={} failed={} needs_auth={} none_usable={}",
                            report.connected.len(),
                            report.pending.len(),
                            report.failed.len(),
                            report.needs_auth.len(),
                            report.none_usable()
                        ),
                        AgentEvent::ConnectorsReconnecting => {
                            println!("[{at:5.1}s] 起こし直す")
                        }
                        AgentEvent::ConnectorsUnavailable => println!("[{at:5.1}s] 繋がらない"),
                        AgentEvent::TurnStarted => println!("[{at:5.1}s] turn started"),
                        AgentEvent::AgentChunk(text) => print!("{text}"),
                        AgentEvent::TurnEnded { reason } => {
                            println!("\n[{at:5.1}s] turn ended {reason:?}");
                            return;
                        }
                        AgentEvent::Failed(error) => {
                            println!("[{at:5.1}s] failed {error}");
                            return;
                        }
                        _ => {}
                    }
                }
                println!("時間切れ");
            });
        }
        drop(command_tx);
        worker.join().ok();
    }
    println!("session={}", session_id.unwrap_or_default());
}
