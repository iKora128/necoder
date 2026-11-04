//! 実エージェントの ACP ログインの確認用プローブ。`run_session_on` を通して、ログインの方式の広告・
//! `authenticate`・URL の受け取り・セッションの開き直しを見る。**prompt は送らない**（利用料が発生しない）。
//! URL の中身は出さず長さだけ出す。ブラウザでのサインインはしない（指定の秒数で打ち切る）。
//!
//! 使い方: `cargo run -p acp_client --example probe_login -- <実行ファイル> <方式の id> <待つ秒数>`
//! 認証情報を汚さないよう `HOME` を空のフォルダへ向けて流す。環境変数はそのままエージェントへ渡る。
use acp_client::{AgentCommand, AgentEvent, SessionCommand, SessionPreferences};
use futures::StreamExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let program = args.next().expect("実行ファイル");
    let method = args.next().expect("方式の id");
    let wait: u64 = args
        .next()
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or(30);
    let cwd: PathBuf = std::env::current_dir().expect("cwd");
    let mut command = AgentCommand::new(PathBuf::from(program), Vec::new(), cwd);
    for name in ["HOME", "GEMINI_API_KEY", "BROWSER"] {
        if let Ok(value) = std::env::var(name) {
            command.env.insert(name.to_string(), value);
        }
    }
    let (command_tx, command_rx) = futures::channel::mpsc::unbounded();
    let (event_tx, mut event_rx) = futures::channel::mpsc::unbounded();
    let started = Instant::now();
    let session = std::thread::spawn(move || {
        futures::executor::block_on(acp_client::run_session_on(
            host::LocalHost::shared(),
            command,
            SessionPreferences {
                login_prompt: true,
                ..Default::default()
            },
            command_rx,
            event_tx,
        ))
    });
    let watcher_tx = command_tx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(wait));
        println!("{wait} 秒で打ち切り");
        watcher_tx.close_channel();
    });
    let mut command_tx = Some(command_tx);
    futures::executor::block_on(async {
        while let Some(event) = event_rx.next().await {
            let seconds = started.elapsed().as_secs_f32();
            match event {
                AgentEvent::AuthRequired { choices, detail } => {
                    let ids: Vec<&str> = choices.iter().map(|choice| choice.id.as_str()).collect();
                    println!("[{seconds:5.1}s] AuthRequired {ids:?} detail={detail:?}");
                    if detail.is_none() {
                        if let Some(command_tx) = &command_tx {
                            command_tx
                                .unbounded_send(SessionCommand::Authenticate(method.clone()))
                                .ok();
                        }
                    }
                }
                AgentEvent::AuthLoginUrl(url) => {
                    let host_end = url.len().min(32);
                    println!(
                        "[{seconds:5.1}s] AuthLoginUrl {}… ({} bytes)",
                        &url[..host_end],
                        url.len()
                    );
                }
                AgentEvent::AuthLoginCompleted => println!("[{seconds:5.1}s] AuthLoginCompleted"),
                AgentEvent::SessionStarted { session_id, .. } => {
                    println!("[{seconds:5.1}s] SessionStarted {session_id}");
                    command_tx = None;
                }
                AgentEvent::Failed(error) => println!("[{seconds:5.1}s] Failed {error}"),
                _ => {}
            }
        }
    });
    drop(command_tx);
    match session.join() {
        Ok(outcome) => println!("終了: {outcome:?}"),
        Err(_) => println!("セッションのスレッドが落ちた"),
    }
}
