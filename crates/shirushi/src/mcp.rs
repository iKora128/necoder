//! mcp — Shirushi の **MCP サーバ**（`shirushi mcp [root]`）。AI エージェント（Claude 等）が
//! Shirushi のプロジェクトを操作するための口（差別化の核＝AI エージェントネイティブ）。
//!
//! transport は MCP 標準の **stdio・改行区切り JSON-RPC**（Content-Length ではない）。同期ループで十分。
//! 公開ツール: `list_files` / `read_file` / `write_file` / `search` / `git_status`。
//! `root` は引数（`shirushi mcp <root>`）→無ければ CWD。プロジェクトのファイルを読み書き/検索/差分できる。
//!
//! 注: 起動中の GUI 窓へ「開く」指示を送るライブ制御は IPC ソケットが要る（後続）。v1 は
//! プロジェクト（ファイル）レベルの操作に集中する。設定 CLI（`shirushi config`）と同じ「書き手」の一つ。

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// `shirushi mcp [root]` を処理したら true（GUI を開かず終了）。
pub fn run() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("mcp") {
        return false;
    }
    let root = args
        .get(1)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    serve(&root);
    true
}

/// stdio の JSON-RPC ループ（改行区切り）。EOF/エラーで終了。
fn serve(root: &Path) {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => break, // EOF / エラー
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if let Some(response) = handle(&request, root) {
            if let Ok(text) = serde_json::to_string(&response) {
                let mut out = stdout.lock();
                let _ = writeln!(out, "{text}");
                let _ = out.flush();
            }
        }
    }
}

/// 1 リクエストを処理して応答 Value を返す（通知は None）。
fn handle(request: &Value, root: &Path) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str)?;
    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0", "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": { "name": "shirushi", "version": "0.1.0" },
                "capabilities": { "tools": {} }
            }
        })),
        // 通知（id 無し）は応答しない。
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {} })),
        "tools/list" => Some(json!({
            "jsonrpc": "2.0", "id": id,
            "result": { "tools": tool_schemas() }
        })),
        "tools/call" => Some(handle_tool_call(id, request, root)),
        _ => id.map(|id| {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "method not found" } })
        }),
    }
}

/// 公開ツールの JSON Schema（`tools/list`）。
fn tool_schemas() -> Value {
    json!([
        {
            "name": "list_files",
            "description": "プロジェクト配下のファイル一覧（gitignore 準拠・相対パス）。",
            "inputSchema": { "type": "object", "properties": {
                "limit": { "type": "integer", "description": "最大件数（既定 2000）" }
            } }
        },
        {
            "name": "read_file",
            "description": "ファイルの内容を読む（プロジェクト相対 or 絶対パス）。",
            "inputSchema": { "type": "object", "required": ["path"], "properties": {
                "path": { "type": "string" }
            } }
        },
        {
            "name": "write_file",
            "description": "ファイルへ内容を書く（親ディレクトリは自動作成）。",
            "inputSchema": { "type": "object", "required": ["path", "content"], "properties": {
                "path": { "type": "string" }, "content": { "type": "string" }
            } }
        },
        {
            "name": "search",
            "description": "プロジェクト横断のテキスト検索（literal / regex）。",
            "inputSchema": { "type": "object", "required": ["query"], "properties": {
                "query": { "type": "string" },
                "regex": { "type": "boolean", "description": "正規表現として扱う（既定 false）" },
                "case_sensitive": { "type": "boolean", "description": "大小区別（既定 false）" }
            } }
        },
        {
            "name": "git_status",
            "description": "git の作業ツリー状態（変更/追加/削除/未追跡ファイル）。",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "fleet_create_task",
            "description": "IntegrationSpace から隔離 branch/worktree を作成し、永続 Task ledger に登録する。",
            "inputSchema": { "type": "object", "required": ["title"], "properties": {
                "title": { "type": "string" }
            } }
        },
        {
            "name": "fleet_list_tasks",
            "description": "この repository の TaskSpace と lifecycle を一覧する。",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "fleet_update_task",
            "description": "Agent が Task の phase と構造化された結果 summary を報告する。",
            "inputSchema": { "type": "object", "required": ["task_id", "phase"], "properties": {
                "task_id": { "type": "string" },
                "phase": { "type": "string", "enum": storage::TaskPhase::ALL.map(storage::TaskPhase::as_str) },
                "summary": { "type": "string" }
            } }
        },
        {
            "name": "fleet_wait_task",
            "description": "別 Task が指定 phase になるまで永続 ledger を待つ（GUI 再起動に依存しない）。",
            "inputSchema": { "type": "object", "required": ["task_id", "phase"], "properties": {
                "task_id": { "type": "string" },
                "phase": { "type": "string", "enum": storage::TaskPhase::ALL.map(storage::TaskPhase::as_str) },
                "timeout_seconds": { "type": "integer" }
            } }
        },
        {
            "name": "fleet_review_task",
            "description": "read-only merge preview (Conflict Radar) を実行し merge_ready / changes_requested へ進める。",
            "inputSchema": { "type": "object", "required": ["task_id"], "properties": {
                "task_id": { "type": "string" }
            } }
        },
        {
            "name": "fleet_integrate_task",
            "description": "merge_ready Task を clean な IntegrationSpace へ明示統合する。失敗時は merge abort。",
            "inputSchema": { "type": "object", "required": ["task_id"], "properties": {
                "task_id": { "type": "string" }
            } }
        },
        {
            "name": "fleet_spawn_agent",
            "description": "Task の worktree に AgentPanel/thread を起こし、必要なら初回 prompt を送る（要 GUI 起動・P5）。",
            "inputSchema": { "type": "object", "required": ["task_id"], "properties": {
                "task_id": { "type": "string" },
                "agent": { "type": "string", "description": "エージェント表示名（省略 = 既定）" },
                "prompt": { "type": "string" }
            } }
        },
        {
            "name": "fleet_send",
            "description": "起動中 Task のアクティブスレッドへ追撃 prompt を送る（要 GUI 起動・P5）。",
            "inputSchema": { "type": "object", "required": ["task_id", "message"], "properties": {
                "task_id": { "type": "string" },
                "message": { "type": "string" }
            } }
        },
        {
            "name": "fleet_digest",
            "description": "Task の事実層 + Tier1 digest（phase/plan/digest/tokens）。フル transcript は返さない（3 段圧縮）。",
            "inputSchema": { "type": "object", "required": ["task_id"], "properties": {
                "task_id": { "type": "string" }
            } }
        },
        {
            "name": "fleet_set_depends",
            "description": "Task の依存（depends_on）を全量置換する。wait-deps と組で「B の完了を待って merge」を組む。",
            "inputSchema": { "type": "object", "required": ["task_id", "depends_on"], "properties": {
                "task_id": { "type": "string" },
                "depends_on": { "type": "array", "items": { "type": "string" } }
            } }
        },
        {
            "name": "fleet_events",
            "description": "全 Task 横断の task_events 差分（since_id より新しいものを古い順・最大 200 件）。",
            "inputSchema": { "type": "object", "properties": {
                "since_id": { "type": "integer" }
            } }
        }
    ])
}

fn handle_tool_call(id: Option<Value>, request: &Value, root: &Path) -> Value {
    let params = request.get("params");
    let name = params.and_then(|params| params.get("name")).and_then(Value::as_str).unwrap_or("");
    let arguments = params.and_then(|params| params.get("arguments")).cloned().unwrap_or(json!({}));
    let outcome = match name {
        "list_files" => tool_list_files(&arguments, root),
        "read_file" => tool_read_file(&arguments, root),
        "write_file" => tool_write_file(&arguments, root),
        "search" => tool_search(&arguments, root),
        "git_status" => tool_git_status(root),
        "fleet_create_task" => tool_fleet_create(&arguments, root),
        "fleet_list_tasks" => tool_fleet_list(root),
        "fleet_update_task" => tool_fleet_update(&arguments),
        "fleet_wait_task" => tool_fleet_wait(&arguments),
        "fleet_review_task" => tool_fleet_review(&arguments, root),
        "fleet_integrate_task" => tool_fleet_integrate(&arguments, root),
        "fleet_spawn_agent" => tool_fleet_spawn(&arguments),
        "fleet_send" => tool_fleet_send(&arguments),
        "fleet_digest" => tool_fleet_digest(&arguments),
        "fleet_set_depends" => tool_fleet_set_depends(&arguments),
        "fleet_events" => tool_fleet_events(&arguments),
        other => Err(format!("未知のツール: {other}")),
    };
    match outcome {
        Ok(text) => json!({ "jsonrpc": "2.0", "id": id, "result": {
            "content": [ { "type": "text", "text": text } ]
        } }),
        Err(message) => json!({ "jsonrpc": "2.0", "id": id, "result": {
            "content": [ { "type": "text", "text": message } ], "isError": true
        } }),
    }
}

// ── ツール実装（reuse: project / search） ──

/// 引数の path をプロジェクトルート基準の絶対パスへ（絶対ならそのまま）。
fn resolve_path(root: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn tool_list_files(arguments: &Value, root: &Path) -> Result<String, String> {
    let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(2000) as usize;
    let worktree = project::Worktree::new(root).map_err(|error| format!("{error:#}"))?;
    let files: Vec<String> = worktree.all_files(limit).into_iter().map(|(_, relative)| relative).collect();
    Ok(files.join("\n"))
}

fn tool_read_file(arguments: &Value, root: &Path) -> Result<String, String> {
    let path = arguments.get("path").and_then(Value::as_str).ok_or("path が必要")?;
    let full = resolve_path(root, path);
    std::fs::read_to_string(&full).map_err(|error| format!("読めない ({}): {error}", full.display()))
}

fn tool_write_file(arguments: &Value, root: &Path) -> Result<String, String> {
    let path = arguments.get("path").and_then(Value::as_str).ok_or("path が必要")?;
    let content = arguments.get("content").and_then(Value::as_str).ok_or("content が必要")?;
    let full = resolve_path(root, path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("親作成に失敗: {error}"))?;
    }
    std::fs::write(&full, content).map_err(|error| format!("書けない ({}): {error}", full.display()))?;
    Ok(format!("書き込み完了: {} ({} バイト)", full.display(), content.len()))
}

fn tool_search(arguments: &Value, root: &Path) -> Result<String, String> {
    let query = arguments.get("query").and_then(Value::as_str).ok_or("query が必要")?;
    let is_regex = arguments.get("regex").and_then(Value::as_bool).unwrap_or(false);
    let case_sensitive = arguments.get("case_sensitive").and_then(Value::as_bool).unwrap_or(false);
    let worktree = project::Worktree::new(root).map_err(|error| format!("{error:#}"))?;
    let files: Vec<PathBuf> = worktree.all_files(5000).into_iter().map(|(path, _)| path).collect();
    let search_query = search::SearchQuery::new(query, is_regex, case_sensitive)
        .map_err(|error| format!("検索パターン不正: {error}"))?;
    let results = search_query.search_files(&files);
    let mut lines = Vec::new();
    let mut total = 0;
    for file in &results {
        let relative = file.path.strip_prefix(root).unwrap_or(&file.path).display().to_string();
        for found in &file.matches {
            total += 1;
            if lines.len() < 200 {
                lines.push(format!("{}:{}: {}", relative, found.line + 1, found.line_text.trim()));
            }
        }
    }
    if total == 0 {
        return Ok("該当なし".to_string());
    }
    let header = format!("{total} 件 / {} ファイル", results.len());
    Ok(format!("{header}\n{}", lines.join("\n")))
}

fn tool_git_status(root: &Path) -> Result<String, String> {
    let entries = project::git_status(root);
    if entries.is_empty() {
        return Ok("クリーン（または git 管理外）".to_string());
    }
    let lines: Vec<String> = entries
        .iter()
        .map(|(path, status)| {
            let relative = path.strip_prefix(root).unwrap_or(path).display().to_string();
            format!("{:?}\t{}", status, relative)
        })
        .collect();
    Ok(lines.join("\n"))
}

fn task_json(task: &storage::TaskSpaceRecord) -> String {
    serde_json::to_string_pretty(&json!({
        "id": task.id,
        "root": task.root,
        "branch": task.branch,
        "title": task.title,
        "kind": task.kind.as_str(),
        "phase": task.phase.as_str(),
        "base_oid": task.base_oid,
        "head_oid": task.head_oid,
        "result_summary": task.result_summary,
    }))
    .unwrap_or_default()
}

fn tool_fleet_create(arguments: &Value, root: &Path) -> Result<String, String> {
    let title = arguments.get("title").and_then(Value::as_str).ok_or("title が必要")?;
    super::fleet::create_task(root, title)
        .map(|task| task_json(&task))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_list(root: &Path) -> Result<String, String> {
    super::fleet::list_tasks(root)
        .map(|tasks| tasks.iter().map(task_json).collect::<Vec<_>>().join("\n"))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_update(arguments: &Value) -> Result<String, String> {
    let task_id = arguments.get("task_id").and_then(Value::as_str).ok_or("task_id が必要")?;
    let phase = arguments.get("phase").and_then(Value::as_str).ok_or("phase が必要")?;
    let phase = super::fleet::parse_phase(phase).map_err(|error| format!("{error:#}"))?;
    let summary = arguments.get("summary").and_then(Value::as_str);
    super::fleet::update_task(task_id, phase, summary)
        .map(|task| task_json(&task))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_wait(arguments: &Value) -> Result<String, String> {
    let task_id = arguments.get("task_id").and_then(Value::as_str).ok_or("task_id が必要")?;
    let phase = arguments.get("phase").and_then(Value::as_str).ok_or("phase が必要")?;
    let phase = super::fleet::parse_phase(phase).map_err(|error| format!("{error:#}"))?;
    let seconds = arguments
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(600)
        .min(3600);
    super::fleet::wait_task(task_id, phase, std::time::Duration::from_secs(seconds))
        .map(|task| task_json(&task))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_review(arguments: &Value, root: &Path) -> Result<String, String> {
    let task_id = arguments.get("task_id").and_then(Value::as_str).ok_or("task_id が必要")?;
    super::fleet::review_task(task_id, root)
        .map(|task| task_json(&task))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_integrate(arguments: &Value, root: &Path) -> Result<String, String> {
    let task_id = arguments.get("task_id").and_then(Value::as_str).ok_or("task_id が必要")?;
    super::fleet::integrate_task(task_id, root)
        .map(|task| task_json(&task))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_spawn(arguments: &Value) -> Result<String, String> {
    let task_id = arguments.get("task_id").and_then(Value::as_str).ok_or("task_id が必要")?;
    super::fleet::gui_request(
        "spawn_agent",
        json!({
            "task_id": task_id,
            "agent": arguments.get("agent").and_then(Value::as_str),
            "prompt": arguments.get("prompt").and_then(Value::as_str),
        }),
    )
    .map(|result| result.to_string())
    .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_send(arguments: &Value) -> Result<String, String> {
    let task_id = arguments.get("task_id").and_then(Value::as_str).ok_or("task_id が必要")?;
    let message = arguments.get("message").and_then(Value::as_str).ok_or("message が必要")?;
    super::fleet::gui_request("send", json!({ "task_id": task_id, "message": message }))
        .map(|result| result.to_string())
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_digest(arguments: &Value) -> Result<String, String> {
    let task_id = arguments.get("task_id").and_then(Value::as_str).ok_or("task_id が必要")?;
    super::fleet::gui_request("digest", json!({ "task_id": task_id }))
        .map(|result| result.to_string())
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_set_depends(arguments: &Value) -> Result<String, String> {
    let task_id = arguments.get("task_id").and_then(Value::as_str).ok_or("task_id が必要")?;
    let depends_on: Vec<String> = arguments
        .get("depends_on")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .ok_or("depends_on（配列）が必要")?;
    super::fleet::set_depends(task_id, &depends_on)
        .map(|task| task_json(&task))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_events(arguments: &Value) -> Result<String, String> {
    let since = arguments.get("since_id").and_then(Value::as_i64).unwrap_or(0);
    super::fleet::events_since(since)
        .map(|result| result.to_string())
        .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        // tag でテスト毎に分ける（cargo test は並列実行なので共有すると削除し合う）。
        let dir = std::env::temp_dir().join(format!("shirushi_mcp_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn initialize_and_tools_list() {
        let root = scratch("init");
        let init = handle(&json!({ "jsonrpc":"2.0","id":1,"method":"initialize" }), &root).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "shirushi");
        let list = handle(&json!({ "jsonrpc":"2.0","id":2,"method":"tools/list" }), &root).unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 16);
        // 通知は応答なし。
        assert!(handle(&json!({ "jsonrpc":"2.0","method":"notifications/initialized" }), &root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_then_read_and_search() {
        let root = scratch("rw");
        // write_file
        let write = handle(
            &json!({ "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params": { "name": "write_file", "arguments": { "path": "a.rs", "content": "fn main() { let todo = 1; }\n" } } }),
            &root,
        )
        .unwrap();
        assert_eq!(write["result"]["isError"], Value::Null); // 成功（isError 無し）
        // read_file
        let read = handle(
            &json!({ "jsonrpc":"2.0","id":2,"method":"tools/call",
                "params": { "name": "read_file", "arguments": { "path": "a.rs" } } }),
            &root,
        )
        .unwrap();
        let text = read["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("let todo"));
        // search
        let search = handle(
            &json!({ "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params": { "name": "search", "arguments": { "query": "todo" } } }),
            &root,
        )
        .unwrap();
        let found = search["result"]["content"][0]["text"].as_str().unwrap();
        assert!(found.contains("a.rs"), "検索で a.rs が見つかる: {found}");
        // 未知メソッド → error
        let unknown = handle(&json!({ "jsonrpc":"2.0","id":9,"method":"nope" }), &root).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        let _ = std::fs::remove_dir_all(&root);
    }
}
