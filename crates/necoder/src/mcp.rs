//! mcp — necoder の **MCP サーバ**（`necoder mcp [root]`）。AI エージェント（Claude 等）が
//! necoder のプロジェクトを操作するための口（差別化の核＝AI エージェントネイティブ）。
//!
//! transport は MCP 標準の **stdio・改行区切り JSON-RPC**（Content-Length ではない）。同期ループで十分。
//! 公開ツール: `list_files` / `read_file` / `write_file` / `search` / `git_status`。
//! `root` は引数（`necoder mcp <root>`）→無ければ CWD。プロジェクトのファイルを読み書き/検索/差分できる。
//!
//! 注: 起動中の GUI 窓へ「開く」指示を送るライブ制御は IPC ソケットが要る（後続）。v1 は
//! プロジェクト（ファイル）レベルの操作に集中する。設定 CLI（`necoder config`）と同じ「書き手」の一つ。

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// どの道具を出すか。`Captain` は Captain の席（FLEET-V2 §5.8）に necoder が自動で渡す版で、
/// 書く道具・Task を直接切る道具・phase を書き換える道具・待つ道具・integrate を出さない。
/// 逆に Captain の席だけの道具（承認待ちへの推薦 `fleet_recommend`・§5.5）は `Full` に出さない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profile {
    Full,
    Captain,
}

impl Profile {
    fn offers(self, tool: &str) -> bool {
        match self {
            Profile::Full => !workspace::CAPTAIN_ONLY_MCP_TOOLS.contains(&tool),
            Profile::Captain => workspace::CAPTAIN_MCP_TOOLS.contains(&tool),
        }
    }

    /// 一覧に出していない道具を名前で呼ばれた時の断り文。
    fn refusal(self, tool: &str) -> String {
        match self {
            Profile::Full => format!("この道具は Captain の席だけで使える: {tool}"),
            Profile::Captain => format!("この道具は Captain の席では使えない: {tool}"),
        }
    }
}

/// `necoder mcp [--captain] [root]` を処理したら true（GUI を開かず終了）。
pub fn run() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("mcp") {
        return false;
    }
    let profile = if args.iter().any(|arg| arg == "--captain") {
        Profile::Captain
    } else {
        Profile::Full
    };
    let root = args
        .iter()
        .skip(1)
        .find(|arg| !arg.starts_with("--"))
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let root = paths::canonicalize(&root).unwrap_or(root);
    serve(&root, profile);
    true
}

/// stdio の JSON-RPC ループ（改行区切り）。EOF/エラーで終了。
fn serve(root: &Path, profile: Profile) {
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
        if let Some(response) = handle(&request, root, profile) {
            if let Ok(text) = serde_json::to_string(&response) {
                let mut out = stdout.lock();
                let _ = writeln!(out, "{text}");
                let _ = out.flush();
            }
        }
    }
}

/// 1 リクエストを処理して応答 Value を返す（通知は None）。
fn handle(request: &Value, root: &Path, profile: Profile) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str)?;
    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0", "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": { "name": "necoder", "version": "0.1.0" },
                "capabilities": { "tools": {} }
            }
        })),
        // 通知（id 無し）は応答しない。
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {} })),
        "tools/list" => Some(json!({
            "jsonrpc": "2.0", "id": id,
            "result": { "tools": tool_schemas_for(profile) }
        })),
        "tools/call" => Some(handle_tool_call(id, request, root, profile)),
        _ => id.map(|id| {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "method not found" } })
        }),
    }
}

/// `tools/list` に出す道具。Captain 版は席の道具だけに絞る（FLEET-V2 §5.8）。
fn tool_schemas_for(profile: Profile) -> Value {
    let full = tool_schemas();
    let captain_only = captain_only_tool_schemas();
    Value::Array(
        full.as_array()
            .into_iter()
            .flatten()
            .chain(captain_only.as_array().into_iter().flatten())
            .filter(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| profile.offers(name))
            })
            .cloned()
            .collect(),
    )
}

/// 公開ツール全部の JSON Schema（Full 版の `tools/list`）。`necoder skills get --full` の道具一覧も同じものを読む。
pub(crate) fn tool_schemas() -> Value {
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
            "description": "隔離 branch/worktree を作成し、永続 Task ledger に登録する。Task の中から呼ぶと、そのブランチを起点にした子 Task になる。necoder 起動中は Fleet に出し、prompt があれば担当を起こして最初の指示として送る。",
            "inputSchema": { "type": "object", "required": ["title"], "properties": {
                "title": { "type": "string" },
                "prompt": { "type": "string", "description": "担当への最初の指示（目的と完了条件）。省略すると担当は起こさない" }
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
            "description": "Task の事実層 + Tier1 digest（phase/plan/digest/tokens）。承認待ちのスレッドには要求の id と文（threads[].permission）も返す。フル transcript は返さない（3 段圧縮）。",
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
            "name": "fleet_propose_tasks",
            "description": "Task の分解案を人間へ提案する（ブランチと worktree はまだ作らない）。人間が承認した行だけ necoder が切り、担当を起こして目的と完了条件を最初の指示として送る。返り値は承認待ち。承認と却下は次の知らせで届くので、待たずにターンを終えてよい。",
            "inputSchema": { "type": "object", "required": ["tasks"], "properties": {
                "tasks": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": workspace::MAX_PROPOSED_TASKS,
                    "items": { "type": "object", "required": ["title", "goal", "done_when"], "properties": {
                        "title": { "type": "string", "description": "Task 名（1 行・ブランチ名の元になる）" },
                        "goal": { "type": "string", "description": "目的（何のために何を変えるか）" },
                        "done_when": { "type": "string", "description": "完了条件（通るべき検証コマンドや、観察できる結果）" },
                        "scope": { "type": "string", "description": "触ってよい範囲（ファイルやディレクトリ）。他の行と重ねない" },
                        "agent": { "type": "string", "description": "担当エージェントの表示名（省略 = 既定のエージェント）" }
                    } }
                },
                "note": { "type": "string", "description": "この分け方にした理由（人間が承認の判断に使う）" }
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

/// Captain の席だけに出す道具の JSON Schema（`workspace::CAPTAIN_ONLY_MCP_TOOLS`）。Full 版の `tools/list` にも
/// `necoder skills get --full` の一覧にも載らない。
fn captain_only_tool_schemas() -> Value {
    json!([
        {
            "name": "fleet_recommend",
            "description": "承認待ちの要求 1 件に、許可してよいかの推薦を 1 行添える。人間の要対応カードに ✳ 付きで出るだけで、応答（許可・拒否）はしない — 押すのは人間。permission_id は知らせの「承認待ち」の行か fleet_digest の threads[].permission.id。要求が解決・取り消し・別の要求に替わっていたら断る（古い要求への推薦を新しい要求に付けない）。necoder の画面が開いている時だけ使える。",
            "inputSchema": { "type": "object", "required": ["task_id", "permission_id", "verdict", "reason"], "properties": {
                "task_id": { "type": "string" },
                "permission_id": { "type": "string", "description": "承認要求の id（今の要求と違えば断る）" },
                "verdict": {
                    "type": "string",
                    "enum": workspace::RecommendationVerdict::ALL.map(workspace::RecommendationVerdict::as_str),
                    "description": "allow = 許可してよい / deny = 拒否を勧める / ask_human = 人間が中身を見て決めるべき（取り消せない・Task の範囲の外 等）"
                },
                "reason": {
                    "type": "string",
                    "maxLength": workspace::MAX_RECOMMENDATION_REASON_CHARS,
                    "description": "理由を 1 行で（例: worktree の中で cargo test を走らせるだけ。書き込みなし）"
                }
            } }
        }
    ])
}

fn handle_tool_call(id: Option<Value>, request: &Value, root: &Path, profile: Profile) -> Value {
    let params = request.get("params");
    let name = params
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let arguments = params
        .and_then(|params| params.get("arguments"))
        .cloned()
        .unwrap_or(json!({}));
    // 一覧に出していない道具は、名前を知っていても呼べない（Captain の席の関所・FLEET-V2 §5.8）。
    let outcome = if !profile.offers(name) {
        Err(profile.refusal(name))
    } else {
        call_tool(name, &arguments, root)
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

fn call_tool(name: &str, arguments: &Value, root: &Path) -> Result<String, String> {
    let arguments = arguments.clone();
    match name {
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
        "fleet_propose_tasks" => tool_fleet_propose(&arguments, root),
        "fleet_recommend" => tool_fleet_recommend(&arguments),
        other => Err(format!("未知のツール: {other}")),
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
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(2000) as usize;
    let worktree = project::Worktree::new(root).map_err(|error| format!("{error:#}"))?;
    let files: Vec<String> = worktree
        .all_files(limit)
        .into_iter()
        .map(|(_, relative)| relative)
        .collect();
    Ok(files.join("\n"))
}

fn tool_read_file(arguments: &Value, root: &Path) -> Result<String, String> {
    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .ok_or("path が必要")?;
    let full = resolve_path(root, path);
    std::fs::read_to_string(&full)
        .map_err(|error| format!("読めない ({}): {error}", full.display()))
}

fn tool_write_file(arguments: &Value, root: &Path) -> Result<String, String> {
    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .ok_or("path が必要")?;
    let content = arguments
        .get("content")
        .and_then(Value::as_str)
        .ok_or("content が必要")?;
    let full = resolve_path(root, path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("親作成に失敗: {error}"))?;
    }
    std::fs::write(&full, content)
        .map_err(|error| format!("書けない ({}): {error}", full.display()))?;
    Ok(format!(
        "書き込み完了: {} ({} バイト)",
        full.display(),
        content.len()
    ))
}

fn tool_search(arguments: &Value, root: &Path) -> Result<String, String> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .ok_or("query が必要")?;
    let is_regex = arguments
        .get("regex")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let case_sensitive = arguments
        .get("case_sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let worktree = project::Worktree::new(root).map_err(|error| format!("{error:#}"))?;
    let files: Vec<PathBuf> = worktree
        .all_files(5000)
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    let search_query = search::SearchQuery::new(query, is_regex, case_sensitive)
        .map_err(|error| format!("検索パターン不正: {error}"))?;
    let results = search_query.search_files(&files);
    let mut lines = Vec::new();
    let mut total = 0;
    for file in &results {
        let relative = file
            .path
            .strip_prefix(root)
            .unwrap_or(&file.path)
            .display()
            .to_string();
        for found in &file.matches {
            total += 1;
            if lines.len() < 200 {
                lines.push(format!(
                    "{}:{}: {}",
                    relative,
                    found.line + 1,
                    found.line_text.trim()
                ));
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
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string();
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
    let title = arguments
        .get("title")
        .and_then(Value::as_str)
        .ok_or("title が必要")?;
    let prompt = arguments.get("prompt").and_then(Value::as_str);
    super::fleet::create_task(root, title, prompt)
        .map(|task| task_json(&task))
        .map_err(|error| format!("{error:#}"))
}

/// 分解案を出す（worktree は作らない・FLEET-V2 §5.5）。返り値は承認待ちの案の id。
fn tool_fleet_propose(arguments: &Value, root: &Path) -> Result<String, String> {
    super::fleet::propose_tasks(root, arguments)
        .map_err(|error| format!("{error:#}"))
        .and_then(|result| serde_json::to_string_pretty(&result).map_err(|error| error.to_string()))
}

/// 承認待ちへの推薦（FLEET-V2 §5.5・Captain の席だけ）。今の要求にだけ意味があるので GUI に渡す。
fn tool_fleet_recommend(arguments: &Value) -> Result<String, String> {
    super::fleet::recommend(arguments)
        .map_err(|error| format!("{error:#}"))
        .and_then(|result| serde_json::to_string_pretty(&result).map_err(|error| error.to_string()))
}

fn tool_fleet_list(root: &Path) -> Result<String, String> {
    super::fleet::list_tasks(root)
        .map(|tasks| tasks.iter().map(task_json).collect::<Vec<_>>().join("\n"))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_update(arguments: &Value) -> Result<String, String> {
    let task_id = arguments
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or("task_id が必要")?;
    let phase = arguments
        .get("phase")
        .and_then(Value::as_str)
        .ok_or("phase が必要")?;
    let phase = super::fleet::parse_phase(phase).map_err(|error| format!("{error:#}"))?;
    let summary = arguments.get("summary").and_then(Value::as_str);
    super::fleet::update_task(task_id, phase, summary)
        .map(|task| task_json(&task))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_wait(arguments: &Value) -> Result<String, String> {
    let task_id = arguments
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or("task_id が必要")?;
    let phase = arguments
        .get("phase")
        .and_then(Value::as_str)
        .ok_or("phase が必要")?;
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
    let task_id = arguments
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or("task_id が必要")?;
    super::fleet::review_task(task_id, root)
        .map(|task| task_json(&task))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_integrate(arguments: &Value, root: &Path) -> Result<String, String> {
    let task_id = arguments
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or("task_id が必要")?;
    super::fleet::integrate_task(task_id, root)
        .map(|task| task_json(&task))
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_spawn(arguments: &Value) -> Result<String, String> {
    let task_id = arguments
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or("task_id が必要")?;
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
    let task_id = arguments
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or("task_id が必要")?;
    let message = arguments
        .get("message")
        .and_then(Value::as_str)
        .ok_or("message が必要")?;
    super::fleet::gui_request("send", json!({ "task_id": task_id, "message": message }))
        .map(|result| result.to_string())
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_digest(arguments: &Value) -> Result<String, String> {
    let task_id = arguments
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or("task_id が必要")?;
    super::fleet::digest(task_id)
        .map(|result| result.to_string())
        .map_err(|error| format!("{error:#}"))
}

fn tool_fleet_set_depends(arguments: &Value) -> Result<String, String> {
    let task_id = arguments
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or("task_id が必要")?;
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
    let since = arguments
        .get("since_id")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    super::fleet::events_since(since)
        .map(|result| result.to_string())
        .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        // tag でテスト毎に分ける（cargo test は並列実行なので共有すると削除し合う）。
        let dir = std::env::temp_dir().join(format!("necoder_mcp_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        paths::canonicalize(&dir).unwrap()
    }

    #[test]
    fn initialize_and_tools_list() {
        let root = scratch("init");
        let init = handle(
            &json!({ "jsonrpc":"2.0","id":1,"method":"initialize" }),
            &root,
            Profile::Full,
        )
        .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "necoder");
        let list = handle(
            &json!({ "jsonrpc":"2.0","id":2,"method":"tools/list" }),
            &root,
            Profile::Full,
        )
        .unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 17);
        // 通知は応答なし。
        assert!(handle(
            &json!({ "jsonrpc":"2.0","method":"notifications/initialized" }),
            &root,
            Profile::Full
        )
        .is_none());
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
            Profile::Full,
        )
        .unwrap();
        assert_eq!(write["result"]["isError"], Value::Null); // 成功（isError 無し）
                                                             // read_file
        let read = handle(
            &json!({ "jsonrpc":"2.0","id":2,"method":"tools/call",
                "params": { "name": "read_file", "arguments": { "path": "a.rs" } } }),
            &root,
            Profile::Full,
        )
        .unwrap();
        let text = read["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("let todo"));
        // search
        let search = handle(
            &json!({ "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params": { "name": "search", "arguments": { "query": "todo" } } }),
            &root,
            Profile::Full,
        )
        .unwrap();
        let found = search["result"]["content"][0]["text"].as_str().unwrap();
        assert!(found.contains("a.rs"), "検索で a.rs が見つかる: {found}");
        // 未知メソッド → error
        let unknown = handle(
            &json!({ "jsonrpc":"2.0","id":9,"method":"nope" }),
            &root,
            Profile::Full,
        )
        .unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn tool_names(profile: Profile, root: &Path) -> Vec<String> {
        let list = handle(
            &json!({ "jsonrpc":"2.0","id":1,"method":"tools/list" }),
            root,
            profile,
        )
        .unwrap();
        list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// Captain 版（FLEET-V2 §5.8）は書く・直接切る・phase を書く・待つ・integrate を出さず、名前を知っていても呼べない。
    #[test]
    fn the_captain_profile_offers_only_the_seat_tools() {
        let root = scratch("captain");
        let captain = tool_names(Profile::Captain, &root);
        let full = tool_names(Profile::Full, &root);
        for forbidden in [
            "write_file",
            "fleet_create_task",
            "fleet_update_task",
            "fleet_wait_task",
            "fleet_integrate_task",
        ] {
            assert!(
                !captain.iter().any(|name| name == forbidden),
                "{forbidden} を出さない"
            );
            assert!(
                full.iter().any(|name| name == forbidden),
                "Full 版には残す: {forbidden}"
            );
        }
        assert!(captain.iter().any(|name| name == "fleet_propose_tasks"));
        // 表に書いた名前は全部実在する（綴り違いで席の道具が消えない）。
        for tool in workspace::CAPTAIN_MCP_TOOLS {
            assert!(
                captain.iter().any(|name| name == tool),
                "実在しない道具名: {tool}"
            );
        }
        // Captain の席だけの道具は席の表にも載っている（載っていないと席の許可の判断で断られる）。
        for tool in workspace::CAPTAIN_ONLY_MCP_TOOLS {
            assert!(workspace::CAPTAIN_MCP_TOOLS.contains(tool), "{tool}");
            assert!(
                !full.iter().any(|name| name == tool),
                "Full 版に出さない: {tool}"
            );
        }
        let refused = handle(
            &json!({ "jsonrpc":"2.0","id":2,"method":"tools/call",
                "params": { "name": "write_file", "arguments": { "path": "a.rs", "content": "x" } } }),
            &root,
            Profile::Captain,
        )
        .unwrap();
        assert_eq!(refused["result"]["isError"], true);
        assert!(!root.join("a.rs").exists(), "断った道具は何も書かない");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn call(root: &Path, profile: Profile, name: &str, arguments: Value) -> (bool, String) {
        let response = handle(
            &json!({ "jsonrpc":"2.0","id":4,"method":"tools/call",
                "params": { "name": name, "arguments": arguments } }),
            root,
            profile,
        )
        .unwrap();
        (
            response["result"]["isError"] == true,
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    }

    /// 承認待ちへの推薦（FLEET-V2 §5.5）は Captain の席だけの道具: 普通の MCP（Full 版）には一覧にも
    /// skills の本文にも出ず、名前で呼んでも断る。Captain 版は引数を GUI に触れる前に確かめる
    /// （ここで通るのは断る場合だけ＝テストが起動中の GUI へ届かない）。
    #[test]
    fn the_recommend_tool_is_the_captains_alone_and_checks_its_arguments_first() {
        let root = scratch("recommend");
        let captain = tool_names(Profile::Captain, &root);
        let full = tool_names(Profile::Full, &root);
        assert!(captain.iter().any(|name| name == "fleet_recommend"));
        assert!(!full.iter().any(|name| name == "fleet_recommend"));
        let documented = tool_schemas();
        assert!(
            !documented
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == "fleet_recommend"),
            "skills get --full の道具一覧（Full 版）に載せない"
        );
        let schema = captain_only_tool_schemas();
        let verdicts = &schema[0]["inputSchema"]["properties"]["verdict"]["enum"];
        assert_eq!(verdicts, &json!(["allow", "deny", "ask_human"]));

        let good = json!({
            "task_id": "space-rope", "permission_id": "1727-3", "verdict": "allow", "reason": "読むだけ",
        });
        let (refused, message) = call(&root, Profile::Full, "fleet_recommend", good);
        assert!(refused, "Full 版は名前で呼んでも断る");
        assert!(message.contains("Captain の席だけ"), "{message}");

        // 足りない引数は、その名前で始まる文で断る（何を直すかが分かる）。
        for (arguments, missing) in [
            (
                json!({ "permission_id": "p", "verdict": "allow", "reason": "r" }),
                "task_id",
            ),
            (
                json!({ "task_id": "t", "verdict": "allow", "reason": "r" }),
                "permission_id",
            ),
            (
                json!({ "task_id": "t", "permission_id": "p", "reason": "r" }),
                "verdict",
            ),
            (
                json!({ "task_id": "t", "permission_id": "p", "verdict": "deny" }),
                "reason",
            ),
        ] {
            let (refused, message) = call(&root, Profile::Captain, "fleet_recommend", arguments);
            assert!(refused, "{missing}: {message}");
            assert!(message.starts_with(missing), "{missing}: {message}");
        }
        let (refused, message) = call(
            &root,
            Profile::Captain,
            "fleet_recommend",
            json!({ "task_id": "t", "permission_id": "p", "verdict": "yes", "reason": "r" }),
        );
        assert!(
            refused && message.contains("ask_human"),
            "使える見立てを添える: {message}"
        );
        let (refused, message) = call(
            &root,
            Profile::Captain,
            "fleet_recommend",
            json!({ "task_id": "t", "permission_id": "p", "verdict": "deny", "reason": "1 行目\n2 行目" }),
        );
        assert!(refused, "理由は 1 行: {message}");
        assert!(!message.contains("GUI"), "GUI に触れる前に断る: {message}");
        if let Err(error) = std::fs::remove_dir_all(&root) {
            eprintln!("一時フォルダを消せない: {error}");
        }
    }

    /// 分解案の引数は GUI や DB に触れる前に確かめる（目的と完了条件が無い案は受けない）。
    #[test]
    fn a_proposal_without_a_done_condition_is_refused_before_anything_is_stored() {
        let root = scratch("propose");
        let refused = handle(
            &json!({ "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params": { "name": "fleet_propose_tasks", "arguments": { "tasks": [ { "title": "x", "goal": "y" } ] } } }),
            &root,
            Profile::Captain,
        )
        .unwrap();
        assert_eq!(refused["result"]["isError"], true);
        let message = refused["result"]["content"][0]["text"].as_str().unwrap();
        assert!(message.contains("done_when"), "{message}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
