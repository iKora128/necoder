//! codex_limits — Codex の 5 時間枠・週枠を `codex app-server` に訊く（O11・利用者が使用量の画面を
//! 開いた時だけ）。
//!
//! codex-acp は rate limit を ACP に出さない（`/status` の文だけ）。Codex CLI 本体の app-server は
//! JSON-RPC の `account/rateLimits/read` を持つので、それを 1 回だけ呼んで閉じる。**認証は codex 本体が
//! 扱う**（necoder は `~/.codex/auth.json` を読まない・トークンを持たない）。常駐もポーリングもしない。
//!
//! 形は codex 0.144.6 の `codex app-server generate-json-schema` で確かめた: 1 行 1 JSON（`jsonrpc` の欄は
//! 無い）、`initialize`（`clientInfo` 必須）→ `initialized` 通知 → 要求。応答を待つ間に要求していない
//! 通知（例 `remoteControl/status/changed`）が挟まる。未ログインだと
//! `{"error": {"code": -32600, "message": "codex account authentication required to read rate limits"}}`。

use crate::usage::{rate_limits_from_codex, RateLimits};
use anyhow::{anyhow, bail, Context as _, Result};
use host::{CommandSpec, Host as _, LocalHost};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// 応答を待つ上限（起動 + 2 往復）。codex は初回に状態 DB を作るので少し長めに取る。
const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// `codex app-server` を起こし、`account/rateLimits/read` を 1 回呼んで閉じる（ブロッキング）。
///
/// `env` は `settings.json` の `agent_servers.codex.env`（`CODEX_HOME` の差し替えなど）。エージェントと
/// 同じアカウントを見るために同じものを渡す。UI スレッドから呼ばない（バックグラウンドで）。
pub fn read_codex_rate_limits(codex: &Path, env: &BTreeMap<String, String>) -> Result<RateLimits> {
    read_rate_limits_with(
        &codex.to_string_lossy(),
        &["app-server".to_string()],
        env,
        READ_TIMEOUT,
    )
}

/// 起動方法を差し替えられる本体（テストは偽の app-server を python で立てる）。
fn read_rate_limits_with(
    program: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<RateLimits> {
    let spec = CommandSpec::new(program, std::env::temp_dir())
        .args(args.iter().cloned())
        .envs(env.clone());
    // `HostProcess` は drop で子を kill + wait する＝どの道で抜けても残らない。
    let mut process = LocalHost
        .spawn_process(&spec)
        .context("codex app-server を起動できない")?;
    let mut stdin = process.take_stdin()?;
    let stdout = process.take_stdout()?;

    // 読みはスレッドへ（ブロッキング読みに締め切りを付けるため）。子が死ねば EOF で抜ける。
    let (line_tx, line_rx) = mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("codex-limits-reader".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        })
        .context("codex app-server の読み取りスレッドを起動できない")?;
    let outcome = exchange(&mut stdin, &line_rx, Instant::now() + timeout);
    // stdin を閉じれば codex は自分で終わる。少しだけ待ち、残っていれば drop で kill する。
    drop(stdin);
    let closing = Instant::now() + Duration::from_millis(500);
    while process.is_alive() && Instant::now() < closing {
        std::thread::sleep(Duration::from_millis(20));
    }
    outcome
}

/// initialize → initialized → `account/rateLimits/read` の 1 往復。
fn exchange(
    stdin: &mut Box<dyn Write + Send>,
    lines: &mpsc::Receiver<String>,
    deadline: Instant,
) -> Result<RateLimits> {
    let mut send = |message: Value| -> Result<()> {
        writeln!(stdin, "{message}").context("codex app-server へ書けない")?;
        stdin.flush().context("codex app-server へ書けない")
    };
    send(json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "necoder", "title": "necoder", "version": env!("CARGO_PKG_VERSION")}
        }
    }))?;
    wait_for_response(lines, 1, deadline).context("codex app-server の initialize")?;
    send(json!({"method": "initialized"}))?;
    send(json!({"id": 2, "method": "account/rateLimits/read"}))?;
    let result = wait_for_response(lines, 2, deadline)?;
    rate_limits_from_codex(&result).context("codex の応答に 5 時間枠・週枠が無い")
}

/// 指定 id の応答が来るまで読む（通知・他の id は読み捨てる）。エラー応答は codex の文言のまま返す。
fn wait_for_response(lines: &mpsc::Receiver<String>, id: u64, deadline: Instant) -> Result<Value> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("codex app-server が時間内に応答しない");
        }
        let line = match lines.recv_timeout(remaining) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("codex app-server が応答の前に終了した")
            }
        };
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            continue; // JSON でない行（起動時の出力など）は読み捨てる
        };
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            let text = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(anyhow!("{text}"));
        }
        return message
            .get("result")
            .cloned()
            .context("codex app-server の応答に result が無い");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::LimitWindow;

    /// 偽の app-server: initialize に答え、要求していない通知を挟んでから rate limit を返す。
    const FAKE_APP_SERVER: &str = r#"
import json, sys
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
initialized = False
for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        assert "jsonrpc" not in msg
        assert msg["params"]["clientInfo"]["name"] == "necoder"
        send({"id": msg["id"], "result": {"userAgent": "fake", "codexHome": "/tmp/x",
                                          "platformFamily": "unix", "platformOs": "macos"}})
    elif method == "initialized":
        initialized = True
    elif method == "account/rateLimits/read":
        assert initialized, "initialized を送ってから要求する"
        send({"method": "remoteControl/status/changed", "params": {"status": "disabled"}})
        send({"id": msg["id"], "result": {"rateLimits": {
            "primary": {"usedPercent": 42, "windowDurationMins": 300, "resetsAt": 1790000000},
            "secondary": {"usedPercent": 18, "windowDurationMins": 10080, "resetsAt": 1790400000}}}})
"#;

    /// 偽の app-server: 未ログインの時の codex と同じエラー応答を返す。
    const FAKE_SIGNED_OUT: &str = r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") == "initialize":
        sys.stdout.write(json.dumps({"id": msg["id"], "result": {}}) + "\n")
    elif msg.get("method") == "account/rateLimits/read":
        sys.stdout.write(json.dumps({"id": msg["id"], "error": {"code": -32600,
            "message": "codex account authentication required to read rate limits"}}) + "\n")
    sys.stdout.flush()
"#;

    fn run_fake(script: &str, timeout: Duration) -> Option<Result<RateLimits>> {
        let python = crate::find_in_path("python3")?;
        Some(read_rate_limits_with(
            &python.to_string_lossy(),
            &["-c".to_string(), script.to_string()],
            &BTreeMap::new(),
            timeout,
        ))
    }

    #[test]
    fn reads_both_windows_skipping_unrequested_notifications() {
        let Some(result) = run_fake(FAKE_APP_SERVER, Duration::from_secs(10)) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        let limits = result.expect("読める");
        let windows: Vec<(LimitWindow, Option<f64>)> = limits
            .windows
            .iter()
            .map(|window| (window.window.clone(), window.used_percent))
            .collect();
        assert_eq!(
            windows,
            vec![
                (LimitWindow::FiveHour, Some(42.0)),
                (LimitWindow::Weekly, Some(18.0))
            ]
        );
    }

    #[test]
    fn signed_out_codex_reports_its_own_message() {
        let Some(result) = run_fake(FAKE_SIGNED_OUT, Duration::from_secs(10)) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        let error = result.expect_err("未ログインは失敗");
        assert!(
            error.to_string().contains("authentication required"),
            "{error:#}"
        );
    }

    /// 応答しない app-server は締め切りで諦める（固まらない）。子は drop で片付く。
    #[test]
    fn a_silent_app_server_times_out() {
        let Some(result) = run_fake(
            "import sys, time\nfor line in sys.stdin:\n    time.sleep(30)\n",
            Duration::from_millis(500),
        ) else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        let error = result.expect_err("締め切りで失敗");
        assert!(error.to_string().contains("initialize"), "{error:#}");
    }
}
