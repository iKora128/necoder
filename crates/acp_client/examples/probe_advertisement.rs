//! 実エージェントに **prompt を 1 通も送らずに** セッションを開き、届く広告（モデル・思考量・
//! 権限モード）を出す確認用プローブ。先張り（agent_panel の prewarm）が成立する前提
//! 「`session/new` の時点でモデル一覧が広告される」を実機で確かめるために使う。
//!
//! 使い方: `cargo run -p acp_client --example probe_advertisement -- "Claude Code"`
use std::sync::Arc;

fn main() {
    let label = std::env::args().nth(1).unwrap_or("Claude Code".to_string());
    let host = host::LocalHost::shared();
    let kind = acp_client::AgentKind::by_label(&label).expect("未知のエージェント");
    let cwd = std::env::current_dir().expect("cwd");
    let command = kind
        .resolve_command_on(host.as_ref(), cwd, None, None)
        .expect("解決に失敗")
        .expect("エージェントが導入されていない");
    println!("起動: {}", command.path.display());

    let (command_tx, command_rx) = futures::channel::mpsc::unbounded();
    let (event_tx, event_rx) = futures::channel::mpsc::unbounded();
    let session = std::thread::spawn(move || {
        futures::executor::block_on(acp_client::run_session_on(
            Arc::clone(&host),
            command,
            acp_client::SessionPreferences::default(),
            command_rx,
            event_tx,
        ))
    });

    // 送信路は握ったまま（drop するとセッションが終わる）。広告が来るのを数秒だけ待つ。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    futures::executor::block_on(async move {
        use futures::StreamExt as _;
        let mut event_rx = event_rx;
        while std::time::Instant::now() < deadline {
            let Some(event) = event_rx.next().await else {
                break;
            };
            match event {
                acp_client::AgentEvent::SessionStarted {
                    session_id,
                    resumed,
                } => {
                    println!("SessionStarted: id={session_id} resumed={resumed}");
                }
                acp_client::AgentEvent::Modes { modes, current } => {
                    println!("Modes: current={current} {modes:?}");
                }
                acp_client::AgentEvent::Configs(configs) => {
                    for config in configs {
                        println!(
                            "Config[{:?}] current={} choices={:?}",
                            config.category, config.current, config.choices
                        );
                    }
                    println!("--- 広告ここまで（prompt は 1 通も送っていない）");
                    return;
                }
                acp_client::AgentEvent::Failed(error) => println!("Failed: {error}"),
                acp_client::AgentEvent::Notice(notice) => println!("Notice: {notice}"),
                _ => {}
            }
        }
        println!("時間切れ: 広告が届かなかった");
    });
    drop(command_tx);
    let _ = session.join();
}
