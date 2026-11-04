//! connector — claude.ai アカウントのコネクタの一覧を調べる（`docs/CHAT.md` §7）。
//!
//! コネクタは necoder の設定を通らずに Claude のセッションへ入ってくるので、何が繋がっているかは
//! Claude Code に聞くしかない。`claude` を stream-json で起こして `mcp_status` の制御要求を送ると、
//! **モデルを呼ばずに**（課金なしで）接続の状態と道具の一覧が返る。道具の種類（読み取り・削除・外への
//! 送信）は名前から推測せず、サーバが付けた注記をそのまま使う。

use anyhow::{anyhow, Context as _, Result};
use serde_json::{json, Value};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// 応答が来なければ次の実行ファイルへ移るまでの時間（古い CLI は制御要求に答えない）。
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(8);
/// 接続中（`pending`）のサーバを待つ上限。これを過ぎたら分かった分だけ返す。
const SETTLE_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(1500);
/// 一覧が空でも、これだけは待つ。起動直後の `mcp_status` はアカウントのコネクタをまだ取りに行って
/// おらず、空で返る（本当に 1 つも繋いでいない人は、この時間だけ待って空と答える）。
const EMPTY_GRACE: Duration = Duration::from_secs(8);

/// 道具が何をするか。設定の「道具の権限」はこの単位でまとめて変えられる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ToolAccess {
    /// 読むだけ（`readOnly`）。
    Read,
    /// 作る・変える（注記なし）。
    Write,
    /// 消す・取り消しにくい変更（`destructive`）。
    Delete,
    /// 外へ届く（`openWorld`）。Gmail なら送信・返信・転送。
    Send,
}

impl ToolAccess {
    /// 表示順。
    pub const ALL: [ToolAccess; 4] = [
        ToolAccess::Read,
        ToolAccess::Write,
        ToolAccess::Delete,
        ToolAccess::Send,
    ];

    /// MCP の注記から決める。読むだけと明記された物を最優先し、注記が無い物は「書く」に倒す。
    fn from_annotations(annotations: Option<&Value>) -> ToolAccess {
        let flag = |key: &str| {
            annotations
                .and_then(|annotations| annotations.get(key))
                .and_then(Value::as_bool)
                .or_else(|| {
                    annotations
                        .and_then(|a| a.get(format!("{key}Hint")))
                        .and_then(Value::as_bool)
                })
                .unwrap_or(false)
        };
        if flag("readOnly") {
            ToolAccess::Read
        } else if flag("openWorld") {
            ToolAccess::Send
        } else if flag("destructive") {
            ToolAccess::Delete
        } else {
            ToolAccess::Write
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorTool {
    pub name: String,
    pub access: ToolAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorState {
    Connected,
    NeedsAuth,
    /// 接続の途中で確かめる時間が尽きた。
    Pending,
    Failed,
}

/// claude.ai アカウントに繋いだサービス 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connector {
    /// Claude Code での名前（例 `claude.ai Gmail`）。
    pub name: String,
    /// ChatGPT のアプリ ID。Claude のコネクタは None。
    pub codex_app_id: Option<String>,
    /// 接続先（例 `https://gmailmcp.googleapis.com/mcp/v1`）。
    pub url: Option<String>,
    pub state: ConnectorState,
    pub tools: Vec<ConnectorTool>,
}

impl Connector {
    /// 画面に出す名前（`claude.ai ` を外す）。
    pub fn display_name(&self) -> &str {
        self.name
            .strip_prefix("claude.ai ")
            .or_else(|| self.name.strip_prefix("ChatGPT "))
            .unwrap_or(&self.name)
    }

    /// 接続先のホスト名。
    pub fn host(&self) -> Option<&str> {
        let url = self.url.as_deref()?;
        let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
        rest.split('/').next().filter(|host| !host.is_empty())
    }
}

/// `mcp_status` の応答からコネクタだけを取り出す（`scope` が `claudeai` の物）。
/// 名前順に並べる（応答の順は接続の早さで変わるので、開くたびに行が動かないように）。
pub fn parse_mcp_status(response: &Value) -> Vec<Connector> {
    let mut connectors: Vec<Connector> = response
        .get("mcpServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|server| server.get("scope").and_then(Value::as_str) == Some("claudeai"))
        .filter_map(|server| {
            let name = server.get("name")?.as_str()?.to_string();
            let state = match server.get("status").and_then(Value::as_str) {
                Some("connected") => ConnectorState::Connected,
                Some("needs-auth") => ConnectorState::NeedsAuth,
                Some("pending") => ConnectorState::Pending,
                _ => ConnectorState::Failed,
            };
            let url = server
                .get("config")
                .and_then(|config| config.get("url"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let tools = server
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|tool| {
                    Some(ConnectorTool {
                        name: tool.get("name")?.as_str()?.to_string(),
                        access: ToolAccess::from_annotations(tool.get("annotations")),
                    })
                })
                .collect();
            Some(Connector {
                name,
                codex_app_id: None,
                url,
                state,
                tools,
            })
        })
        .collect();
    connectors.sort_by(|left, right| left.name.cmp(&right.name));
    connectors
}

/// 試す `claude` の順。PATH の古い CLI より、公式インストーラの置き場を先に見る。
fn candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = paths::home_dir() {
        candidates.push(home.join(".local/bin/claude"));
        candidates.push(home.join(".claude/local/claude"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/claude"));
    candidates.push(PathBuf::from("/usr/local/bin/claude"));
    candidates.push(PathBuf::from("claude"));
    // CLI を入れていなくても、necoder が起こすアダプタには Claude Code の実行ファイルが同梱されている。
    if let Some(home) = paths::home_dir() {
        candidates.extend(bundled_binaries(&home));
    }
    candidates
}

/// npx のキャッシュにあるアダプタ同梱の `claude`（新しい順）。他の OS 向けの空のフォルダは飛ばす。
fn bundled_binaries(home: &std::path::Path) -> Vec<PathBuf> {
    let binary_name = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    let Ok(caches) = std::fs::read_dir(home.join(".npm/_npx")) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for cache in caches.flatten() {
        let scope = cache.path().join("node_modules/@anthropic-ai");
        let Ok(packages) = std::fs::read_dir(scope) else {
            continue;
        };
        for package in packages.flatten() {
            let is_native = package
                .file_name()
                .to_string_lossy()
                .starts_with("claude-agent-sdk-");
            let binary = package.path().join(binary_name);
            if !is_native {
                continue;
            }
            if let Ok(modified) = binary.metadata().and_then(|meta| meta.modified()) {
                found.push((modified, binary));
            }
        }
    }
    found.sort_by(|left, right| right.0.cmp(&left.0));
    found.into_iter().map(|(_, binary)| binary).collect()
}

/// コネクタの一覧を調べる。**ブロックする**ので背景で呼ぶ。
pub fn probe_claude_connectors() -> Result<Vec<Connector>> {
    let mut last_error = None;
    for candidate in candidates() {
        if candidate.is_absolute() && !candidate.exists() {
            continue;
        }
        match probe_with(&candidate) {
            Ok(connectors) => return Ok(connectors),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("claude が見つかりません")))
}

/// 終わる時に必ず止める（抜け道が増えても子プロセスを残さない）。
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Err(error) = self.0.kill() {
            // 既に終わっている時も `kill` は失敗する。止められなかったことだけ残す。
            eprintln!("コネクタの確認に使った claude を止められません: {error}");
        }
        if let Err(error) = self.0.wait() {
            eprintln!("コネクタの確認に使った claude を回収できません: {error}");
        }
    }
}

/// 一覧が出そろったか。接続中が残っていれば待ち、空なら [`EMPTY_GRACE`] までは待つ。
fn is_settled(connectors: &[Connector], elapsed: Duration) -> bool {
    if connectors.is_empty() {
        return elapsed >= EMPTY_GRACE;
    }
    connectors
        .iter()
        .all(|connector| connector.state != ConnectorState::Pending)
}

fn probe_with(binary: &std::path::Path) -> Result<Vec<Connector>> {
    let mut command = child_process::command(binary);
    command
        .args([
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            // user / project の MCP を起こさない（知りたいのはアカウントのコネクタだけ）。
            "--setting-sources",
            "",
            "--tools",
            "Read",
        ])
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        // necoder 自身の設定で切っていても、一覧を見るには読み込ませる。
        .env_remove("ENABLE_CLAUDEAI_MCP_SERVERS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(home) = paths::home_dir() {
        command.current_dir(home);
    }
    let mut child = KillOnDrop(
        command
            .spawn()
            .with_context(|| format!("{} を起動できません", binary.display()))?,
    );
    let mut stdin = child.0.stdin.take().context("標準入力が取れません")?;
    let stdout = child.0.stdout.take().context("標準出力が取れません")?;

    let (line_tx, line_rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if line_tx.send(value).is_err() {
                return;
            }
        }
    });

    let mut request = |id: &str, subtype: &str| -> Result<()> {
        let message = json!({
            "type": "control_request",
            "request_id": id,
            "request": { "subtype": subtype },
        });
        writeln!(stdin, "{message}").context("要求を送れません")?;
        stdin.flush().context("要求を送れません")
    };
    let wait = |id: &str, timeout: Duration| -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = line_rx
                .recv_timeout(remaining)
                .map_err(|_| anyhow!("claude が制御要求に答えません"))?;
            if message.get("type").and_then(Value::as_str) != Some("control_response") {
                continue;
            }
            let Some(response) = message.get("response") else {
                continue;
            };
            if response.get("request_id").and_then(Value::as_str) == Some(id) {
                return Ok(response.get("response").cloned().unwrap_or(Value::Null));
            }
        }
    };

    request("initialize", "initialize")?;
    wait("initialize", INITIALIZE_TIMEOUT)?;

    let started = Instant::now();
    let deadline = started + SETTLE_TIMEOUT;
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        let id = format!("status-{attempt}");
        request(&id, "mcp_status")?;
        let connectors = parse_mcp_status(&wait(&id, INITIALIZE_TIMEOUT)?);
        if is_settled(&connectors, started.elapsed()) || Instant::now() >= deadline {
            return Ok(connectors);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Codex App Server の MCP 状態をアプリ単位へまとめる。認証情報・メール本文は取得しない。
pub fn parse_codex_connectors(response: &Value) -> Vec<Connector> {
    let mut apps = std::collections::BTreeMap::<String, Connector>::new();
    for server in response
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if server.get("name").and_then(Value::as_str) != Some("codex_apps") {
            continue;
        }
        for (name, tool) in server
            .get("tools")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let Some(meta) = tool.get("_meta") else {
                continue;
            };
            let Some(id) = meta.get("connector_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(label) = meta.get("connector_name").and_then(Value::as_str) else {
                continue;
            };
            let app = apps.entry(id.into()).or_insert_with(|| Connector {
                name: format!("ChatGPT {label}"),
                codex_app_id: Some(id.into()),
                url: Some("https://chatgpt.com/apps".into()),
                state: ConnectorState::Connected,
                tools: Vec::new(),
            });
            app.tools.push(ConnectorTool {
                name: name.clone(),
                access: ToolAccess::from_annotations(tool.get("annotations")),
            });
        }
    }
    let mut apps: Vec<_> = apps.into_values().collect();
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps
}

/// モデルを起こさず ChatGPT の接続済みアプリと道具を調べる。
pub fn probe_codex_connectors(env: &std::collections::BTreeMap<String, String>) -> Result<Vec<Connector>> {
    let mut child = KillOnDrop(
        child_process::command(env.get("CODEX_PATH").map(String::as_str).unwrap_or("codex"))
            .envs(env)
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Codex App Server を起動できません")?,
    );
    let mut stdin = child.0.stdin.take().context("標準入力が取れません")?;
    let stdout = child.0.stdout.take().context("標準出力が取れません")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if tx.send(value).is_err() {
                    break;
                }
            }
        }
    });
    let mut request = |id: u64, method: &str, params: Value| -> Result<Value> {
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})
        )?;
        stdin.flush()?;
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            let response = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .context("Codex が接続の確認に答えません")?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(anyhow!("Codex: {error}"));
            }
            return response
                .get("result")
                .cloned()
                .context("Codex の応答に結果がありません");
        }
    };
    request(
        1,
        "initialize",
        json!({"clientInfo":{"name":"necoder", "version":env!("CARGO_PKG_VERSION")}, "capabilities":{"experimentalApi":true}}),
    )?;
    let mut apps = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let result = request(
            2,
            "mcpServerStatus/list",
            json!({"limit":100, "cursor":cursor}),
        )?;
        apps.extend(parse_codex_connectors(&result));
        cursor = result.get("nextCursor").cloned().unwrap_or(Value::Null);
        if cursor.is_null() {
            break;
        }
    }
    Ok(apps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_response_keeps_only_account_connectors_and_their_tool_kinds() {
        let response = json!({ "mcpServers": [
            { "name": "claude.ai Gmail", "status": "connected", "scope": "claudeai",
              "config": { "type": "claudeai-proxy", "url": "https://gmailmcp.googleapis.com/mcp/v1" },
              "tools": [
                { "name": "search_threads", "annotations": { "readOnly": true } },
                { "name": "create_draft", "annotations": {} },
                { "name": "trash_thread", "annotations": { "destructive": true } },
                { "name": "reply", "annotations": { "openWorld": true } },
                { "name": "unannotated" },
              ] },
            { "name": "claude.ai Stripe", "status": "needs-auth", "scope": "claudeai", "tools": [] },
            { "name": "claude-in-chrome", "status": "connected", "scope": "dynamic", "tools": [] },
        ] });
        let connectors = parse_mcp_status(&response);
        assert_eq!(connectors.len(), 2, "アカウントのコネクタだけ");
        let gmail = &connectors[0];
        assert_eq!(gmail.display_name(), "Gmail");
        assert_eq!(gmail.host(), Some("gmailmcp.googleapis.com"));
        assert_eq!(gmail.state, ConnectorState::Connected);
        let access: Vec<ToolAccess> = gmail.tools.iter().map(|tool| tool.access).collect();
        assert_eq!(
            access,
            [
                ToolAccess::Read,
                ToolAccess::Write,
                ToolAccess::Delete,
                ToolAccess::Send,
                ToolAccess::Write,
            ]
        );
        assert_eq!(connectors[1].state, ConnectorState::NeedsAuth);
    }

    #[test]
    fn an_empty_list_right_after_startup_is_not_the_answer_yet() {
        let connector = |state| Connector {
            name: "claude.ai Gmail".to_string(),
            codex_app_id: None,
            url: None,
            state,
            tools: Vec::new(),
        };
        assert!(
            !is_settled(&[], Duration::from_secs(1)),
            "まだ取りに行っていない"
        );
        assert!(is_settled(&[], EMPTY_GRACE), "待っても空なら本当に無い");
        assert!(!is_settled(
            &[connector(ConnectorState::Pending)],
            Duration::from_secs(1)
        ));
        assert!(is_settled(
            &[connector(ConnectorState::Connected)],
            Duration::from_secs(1)
        ));
    }

    #[test]
    fn the_binary_bundled_with_the_adapter_is_found_in_the_npx_cache() {
        let home = std::env::temp_dir().join(format!("necoder_connector_{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        let scope = home.join(".npm/_npx/abc/node_modules/@anthropic-ai");
        let native = scope.join("claude-agent-sdk-darwin-arm64");
        let other_platform = scope.join("claude-agent-sdk-linux-x64");
        let unrelated = scope.join("sdk");
        for directory in [&native, &other_platform, &unrelated] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let binary_name = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        std::fs::write(native.join(binary_name), "").unwrap();
        std::fs::write(unrelated.join(binary_name), "").unwrap();
        assert_eq!(bundled_binaries(&home), [native.join(binary_name)]);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn read_only_wins_over_other_annotations() {
        let annotations = json!({ "readOnly": true, "openWorld": true });
        assert_eq!(
            ToolAccess::from_annotations(Some(&annotations)),
            ToolAccess::Read
        );
    }
}

#[cfg(test)]
mod codex_tests {
    use super::*;
    #[test]
    fn app_tools_keep_their_provider_id_and_safety_annotations() {
        let apps = parse_codex_connectors(&json!({"data":[{"name":"codex_apps", "tools":{
            "gmail.read": {"annotations":{"readOnlyHint":true}, "_meta":{"connector_id":"gmail-id", "connector_name":"Gmail"}},
            "gmail.send": {"annotations":{"openWorldHint":true}, "_meta":{"connector_id":"gmail-id", "connector_name":"Gmail"}}
        }}, {"name":"other", "tools":{}}]}));
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].display_name(), "Gmail");
        assert_eq!(apps[0].codex_app_id.as_deref(), Some("gmail-id"));
        assert_eq!(apps[0].tools[0].access, ToolAccess::Read);
        assert_eq!(apps[0].tools[1].access, ToolAccess::Send);
    }
}
