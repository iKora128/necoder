//! Worktree-native Fleet orchestration API.
//!
//! GUI / CLI / MCP は同じ `storage::TaskSpaceRecord` と project の Git safety gate を使う。
//! Coordinator はこの API で Task を作り、Agent は status/result を報告し、別プロセスは永続 DB を
//! poll して待てる。GUI プロセスの一時 state に依存しないため再起動後も継続可能。

use anyhow::{Context as _, Result};
use host::LocalHost;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use storage::{SpaceKind, Storage, TaskPhase, TaskSpaceRecord};

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn open_storage() -> Result<Storage> {
    let path = std::env::var("SHIRUSHI_DB")
        .map(PathBuf::from)
        .ok()
        .or_else(storage::default_db_path)
        .context("Task ledger の保存先を決められません")?;
    Storage::open(&path)
}

fn slug(value: &str) -> String {
    let slug = value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

fn record_json(record: &TaskSpaceRecord) -> Value {
    json!({
        "id": record.id,
        "repository_id": record.repository_id,
        "root": record.root,
        "branch": record.branch,
        "title": record.title,
        "kind": record.kind.as_str(),
        "phase": record.phase.as_str(),
        "base_oid": record.base_oid,
        "head_oid": record.head_oid,
        "result_summary": record.result_summary,
        "depends_on": record.depends_on,
        "created_at": record.created_at,
        "updated_at": record.updated_at,
    })
}

/// CLI/MCP 境界の文字列 → enum。不正値はエラー（有効値の一覧つき）。
pub(crate) fn parse_phase(value: &str) -> Result<TaskPhase> {
    TaskPhase::from_str(value).with_context(|| {
        let valid = TaskPhase::ALL.map(TaskPhase::as_str).join(" | ");
        format!("不正な phase: {value}（有効: {valid}）")
    })
}

/// 起動中 GUI への IPC（P5・1 接続 1 リクエスト・1 行 JSON）。GUI が居なければ明確なエラー。
/// 守るべき操作（spawn/send）はこの経路 = Shirushi の CLI/MCP にだけ置く（計画 §0-8）。
pub(crate) fn gui_request(method: &str, params: Value) -> Result<Value> {
    use std::io::{BufRead as _, BufReader, Write as _};
    let socket_path = workspace::control_socket_path()
        .context("GUI socket の場所が決められません（HOME が無い）")?;
    let mut stream = std::os::unix::net::UnixStream::connect(&socket_path).with_context(|| {
        format!(
            "GUI が起動していません（{} に接続できない）。Shirushi を開いてから実行してください",
            socket_path.display()
        )
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .context("IPC timeout 設定に失敗")?;
    let request = json!({ "method": method, "params": params });
    writeln!(stream, "{request}").context("IPC 送信に失敗")?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("IPC 応答の読み取りに失敗")?;
    let response: Value = serde_json::from_str(&line).context("IPC 応答が JSON でない")?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    } else {
        let error = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("不明なエラー");
        anyhow::bail!("{error}")
    }
}

/// `fleet events [since_id]`: 全 Task 横断の task_events 差分（古い順・最大 200 件）。
/// GUI 不在なら DB 直読み・稼働中（ロック）は IPC（P5）。監督/CLI は最後の id を覚えて差分だけ読む。
pub(crate) fn events_since(since_id: i64) -> Result<Value> {
    let events = match open_storage().and_then(|storage| storage.load_task_events_since(since_id, 200)) {
        Ok(events) => events,
        Err(error) if is_lock_error(&error) => {
            return gui_request("events", json!({ "since_id": since_id }));
        }
        Err(error) => return Err(error),
    };
    Ok(Value::Array(
        events
            .into_iter()
            .map(|event| {
                json!({
                    "id": event.id,
                    "task_id": event.task_id,
                    "kind": event.kind,
                    "payload": serde_json::from_str::<Value>(&event.payload)
                        .unwrap_or(Value::String(event.payload)),
                    "created_at": event.created_at,
                })
            })
            .collect(),
    ))
}

pub(crate) fn create_task(root: &Path, title: &str) -> Result<TaskSpaceRecord> {
    let root = std::fs::canonicalize(root).context("IntegrationSpace を開けません")?;
    let host = LocalHost::shared();
    let base_oid =
        project::git_head_oid_on(host.as_ref(), &root).context("Git repository ではありません")?;
    let repository_id = project::repository_id_on(host.as_ref(), &root);
    let repo_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo");
    let parent = root.parent().context("worktree の作成先がありません")?;
    let stem = slug(title);
    let used = project::git_branches_on(host.as_ref(), &root);
    let mut number = 1usize;
    let (branch, target) = loop {
        let suffix = if number == 1 {
            stem.clone()
        } else {
            format!("{stem}-{number}")
        };
        let branch = format!("task/{suffix}");
        let target = parent.join(format!("{repo_name}-task-{suffix}"));
        if !used.contains(&branch) && !target.exists() {
            break (branch, target);
        }
        number += 1;
    };
    project::create_task_worktree_on(host.as_ref(), &root, &target, &branch)?;
    let target = std::fs::canonicalize(&target).unwrap_or(target);
    let now = unix_ms();
    let record = TaskSpaceRecord {
        id: project::stable_worktree_id_on(host.as_ref(), &target),
        repository_id,
        root: target,
        branch: Some(branch),
        title: title.to_string(),
        kind: SpaceKind::Task,
        phase: TaskPhase::Planned,
        base_oid: Some(base_oid.clone()),
        head_oid: Some(base_oid),
        result_summary: None,
        depends_on: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    match open_storage() {
        Ok(storage) => {
            storage.upsert_task_space(&record)?;
            storage.append_task_event(
                &record.id,
                "task_created",
                &json!({ "source": "orchestration_api", "root": record.root }).to_string(),
            )?;
        }
        // GUI 稼働中（DB ロック）: 台帳への登録は GUI（単一 writer）に頼む。worktree は作成済み。
        Err(error) if is_lock_error(&error) => {
            gui_request("record_task", record_json(&record))?;
        }
        Err(error) => return Err(error),
    }
    Ok(record)
}

/// Turso は排他ロック＝GUI 稼働中は直接 DB を開けない。その時は GUI IPC（単一 writer）へ回す。
fn is_lock_error(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}");
    text.contains("Locking error") || text.contains("locked by another process")
}

/// IPC 応答の record JSON → `TaskSpaceRecord`（GUI 経由読み書きの復路・P5）。
fn record_from_json(value: &Value) -> Result<TaskSpaceRecord> {
    Ok(TaskSpaceRecord {
        id: value.get("id").and_then(Value::as_str).context("record id")?.to_string(),
        repository_id: value
            .get("repository_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        root: PathBuf::from(value.get("root").and_then(Value::as_str).unwrap_or_default()),
        branch: value.get("branch").and_then(Value::as_str).map(str::to_string),
        title: value.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
        kind: if value.get("kind").and_then(Value::as_str) == Some("integration") {
            SpaceKind::Integration
        } else {
            SpaceKind::Task
        },
        phase: value
            .get("phase")
            .and_then(Value::as_str)
            .and_then(TaskPhase::from_str)
            .unwrap_or(TaskPhase::Planned),
        base_oid: value.get("base_oid").and_then(Value::as_str).map(str::to_string),
        head_oid: value.get("head_oid").and_then(Value::as_str).map(str::to_string),
        result_summary: value
            .get("result_summary")
            .and_then(Value::as_str)
            .map(str::to_string),
        depends_on: value
            .get("depends_on")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        created_at: value.get("created_at").and_then(Value::as_i64).unwrap_or(0),
        updated_at: value.get("updated_at").and_then(Value::as_i64).unwrap_or(0),
    })
}

pub(crate) fn list_tasks(root: &Path) -> Result<Vec<TaskSpaceRecord>> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let repository_id = project::repository_id_on(&LocalHost, &root);
    let records = match open_storage().and_then(|storage| storage.load_task_spaces()) {
        Ok(records) => records,
        Err(error) if is_lock_error(&error) => gui_request("tasks", json!({}))?
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| record_from_json(value).ok())
                    .collect()
            })
            .unwrap_or_default(),
        Err(error) => return Err(error),
    };
    Ok(records
        .into_iter()
        .filter(|task| task.repository_id == repository_id)
        .collect())
}

fn task_by_id(task_id: &str) -> Result<TaskSpaceRecord> {
    match open_storage().and_then(|storage| storage.load_task_spaces()) {
        Ok(tasks) => tasks
            .into_iter()
            .find(|task| task.id == task_id)
            .with_context(|| format!("Task が見つかりません: {task_id}")),
        Err(error) if is_lock_error(&error) => {
            record_from_json(&gui_request("task", json!({ "task_id": task_id }))?)
        }
        Err(error) => Err(error),
    }
}

/// phase 遷移の orchestration 側入口。GUI の `transition_task_space` と同じ
/// `Storage::commit_task_transition`（snapshot + task_events を同一 transaction）を通る（P0）。
/// GUI 稼働中（DB ロック）は IPC で GUI の遷移入口へ回す＝単一 writer（P5）。
pub(crate) fn update_task(
    task_id: &str,
    phase: TaskPhase,
    summary: Option<&str>,
) -> Result<TaskSpaceRecord> {
    let storage = match open_storage() {
        Ok(storage) => storage,
        Err(error) if is_lock_error(&error) => {
            let result = gui_request(
                "update_task",
                json!({ "task_id": task_id, "phase": phase.as_str(), "summary": summary }),
            )?;
            return record_from_json(&result);
        }
        Err(error) => return Err(error),
    };
    let mut task = task_by_id(task_id)?;
    task.phase = phase;
    task.result_summary = summary.map(str::to_string).or(task.result_summary);
    task.head_oid = project::git_head_oid_on(&LocalHost, &task.root);
    task.updated_at = unix_ms();
    let payload = json!({
        "phase": phase.as_str(),
        "source": "orchestration_api",
        "summary": summary,
        "head_oid": task.head_oid,
    })
    .to_string();
    storage.commit_task_transition(&task, &payload)?;
    Ok(task)
}

pub(crate) fn wait_task(
    task_id: &str,
    phase: TaskPhase,
    timeout: Duration,
) -> Result<TaskSpaceRecord> {
    let started = Instant::now();
    loop {
        let task = task_by_id(task_id)?;
        if task.phase == phase {
            return Ok(task);
        }
        anyhow::ensure!(
            started.elapsed() < timeout,
            "Task wait が timeout: {} != {}",
            task.phase.as_str(),
            phase.as_str()
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// activity 待ち（P6・`fleet wait` の runtime 対応）: GUI live の rollup activity が一致するまで。
/// phase（台帳）と別軸の「今なにをしているか」を待てる（例: blocked を待って人を呼ぶ・idle を待って追撃）。
pub(crate) fn wait_activity(task_id: &str, target: &str, timeout: Duration) -> Result<Value> {
    const ACTIVITIES: &[&str] = &["idle", "working", "blocked", "done", "interrupted"];
    anyhow::ensure!(
        ACTIVITIES.contains(&target),
        "不正な activity: {target}（有効: {}）",
        ACTIVITIES.join(" | ")
    );
    let rank = |activity: &str| match activity {
        "blocked" => 3,
        "working" => 2,
        "done" | "interrupted" => 1,
        _ => 0,
    };
    let started = Instant::now();
    loop {
        let digest = gui_request("digest", json!({ "task_id": task_id }))?;
        let rollup = digest
            .get("threads")
            .and_then(Value::as_array)
            .and_then(|threads| {
                threads
                    .iter()
                    .filter_map(|thread| thread.get("activity").and_then(Value::as_str))
                    .max_by_key(|activity| rank(activity))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "idle".to_string());
        if rollup == target {
            return Ok(digest);
        }
        anyhow::ensure!(
            started.elapsed() < timeout,
            "activity wait が timeout: {rollup} != {target}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// 依存の設定（P6）。GUI 稼働中（ロック）は IPC の単一 writer 経由。
pub(crate) fn set_depends(task_id: &str, depends_on: &[String]) -> Result<TaskSpaceRecord> {
    match open_storage() {
        Ok(storage) => storage.set_task_depends(task_id, depends_on)?,
        Err(error) if is_lock_error(&error) => {
            gui_request(
                "set_depends",
                json!({ "task_id": task_id, "depends_on": depends_on }),
            )?;
        }
        Err(error) => return Err(error),
    }
    task_by_id(task_id)
}

/// 依存がすべて `phase` に達するまで待つ（P6・「B の完了を待って merge」の道具）。
pub(crate) fn wait_deps(task_id: &str, phase: TaskPhase, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    loop {
        let task = task_by_id(task_id)?;
        if task.depends_on.is_empty() {
            return Ok(json!({ "task_id": task.id, "depends_on": [], "note": "依存なし" }));
        }
        let states: Vec<(String, TaskPhase)> = task
            .depends_on
            .iter()
            .map(|dep| task_by_id(dep).map(|record| (record.id, record.phase)))
            .collect::<Result<_>>()?;
        if states.iter().all(|(_, state)| *state == phase) {
            return Ok(json!({
                "task_id": task.id,
                "depends_on": states
                    .iter()
                    .map(|(id, state)| json!({ "id": id, "phase": state.as_str() }))
                    .collect::<Vec<_>>(),
            }));
        }
        anyhow::ensure!(
            started.elapsed() < timeout,
            "依存待ちが timeout: {:?}",
            states
                .iter()
                .map(|(id, state)| format!("{id}={}", state.as_str()))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub(crate) fn review_task(task_id: &str, integration_root: &Path) -> Result<TaskSpaceRecord> {
    let task = task_by_id(task_id)?;
    let branch = task.branch.as_deref().context("Task branch がありません")?;
    let preview = project::preview_merge_on(&LocalHost, integration_root, branch)?;
    if preview.clean {
        update_task(task_id, TaskPhase::MergeReady, Some("Conflict Radar: clean"))
    } else {
        update_task(task_id, TaskPhase::ChangesRequested, Some(&preview.detail))
    }
}

pub(crate) fn integrate_task(task_id: &str, integration_root: &Path) -> Result<TaskSpaceRecord> {
    let task = task_by_id(task_id)?;
    anyhow::ensure!(
        task.phase == TaskPhase::MergeReady,
        "Task は merge_ready ではありません"
    );
    let branch = task.branch.as_deref().context("Task branch がありません")?;
    update_task(task_id, TaskPhase::Integrating, None)?;
    match project::integrate_branch_on(&LocalHost, integration_root, branch) {
        Ok(head) => update_task(
            task_id,
            TaskPhase::Integrated,
            Some(&format!("integrated at {head}")),
        ),
        Err(error) => {
            let _ = update_task(
                task_id,
                TaskPhase::MergeReady,
                Some(&format!("integration failed: {error:#}")),
            );
            Err(error)
        }
    }
}

fn print_record(record: &TaskSpaceRecord) {
    println!(
        "{}",
        serde_json::to_string_pretty(&record_json(record)).unwrap_or_default()
    );
}

/// `shirushi fleet …` を処理したら true。GUI は開かない。
pub(crate) fn run_cli() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("fleet") {
        return false;
    }
    let result = match args.get(1).map(String::as_str) {
        Some("create") => {
            let root = args.get(2).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            let title = args.get(3).map(String::as_str).unwrap_or("Task");
            create_task(&root, title).map(|record| print_record(&record))
        }
        Some("list") => {
            let root = args.get(2).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            list_tasks(&root).map(|tasks| {
                let values: Vec<_> = tasks.iter().map(record_json).collect();
                println!("{}", serde_json::to_string_pretty(&values).unwrap_or_default());
            })
        }
        Some("status") => match (args.get(2), args.get(3)) {
            (Some(id), Some(phase)) => parse_phase(phase).and_then(|phase| {
                update_task(id, phase, args.get(4).map(String::as_str))
                    .map(|record| print_record(&record))
            }),
            _ => Err(anyhow::anyhow!("使い方: shirushi fleet status <task-id> <phase> [summary]")),
        },
        Some("wait") => match (args.get(2), args.get(3)) {
            (Some(id), Some(target)) => {
                let seconds = args.get(4).and_then(|value| value.parse().ok()).unwrap_or(600);
                let timeout = Duration::from_secs(seconds);
                // phase（台帳）と activity（GUI live）の両対応（P6）。まず phase として解釈。
                match parse_phase(target) {
                    Ok(phase) => wait_task(id, phase, timeout).map(|record| print_record(&record)),
                    Err(_) => wait_activity(id, target, timeout).map(|digest| {
                        println!("{}", serde_json::to_string_pretty(&digest).unwrap_or_default())
                    }),
                }
            }
            _ => Err(anyhow::anyhow!(
                "使い方: shirushi fleet wait <task-id> <phase|activity> [timeout-seconds]"
            )),
        },
        Some("depend") => match (args.get(2), args.len() > 3) {
            (Some(id), true) => set_depends(id, &args[3..].to_vec())
                .map(|record| print_record(&record)),
            _ => Err(anyhow::anyhow!("使い方: shirushi fleet depend <task-id> <depends-on-id...>")),
        },
        Some("wait-deps") => match (args.get(2), args.get(3)) {
            (Some(id), Some(phase)) => parse_phase(phase).and_then(|phase| {
                let seconds = args.get(4).and_then(|value| value.parse().ok()).unwrap_or(600);
                wait_deps(id, phase, Duration::from_secs(seconds)).map(|result| {
                    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default())
                })
            }),
            _ => Err(anyhow::anyhow!(
                "使い方: shirushi fleet wait-deps <task-id> <phase> [timeout-seconds]"
            )),
        },
        Some("review") => match args.get(2) {
            Some(id) => {
                let root = args.get(3).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
                review_task(id, &root).map(|record| print_record(&record))
            }
            None => Err(anyhow::anyhow!("使い方: shirushi fleet review <task-id> [integration-root]")),
        },
        Some("integrate") => match args.get(2) {
            Some(id) => {
                let root = args.get(3).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
                integrate_task(id, &root).map(|record| print_record(&record))
            }
            None => Err(anyhow::anyhow!("使い方: shirushi fleet integrate <task-id> [integration-root]")),
        },
        // ── ここから GUI ライブ制御（P5・要 GUI 起動） ──
        Some("spawn-agent") => match args.get(2) {
            Some(id) => {
                let agent = args.get(3).cloned();
                let prompt = (args.len() > 4).then(|| args[4..].join(" "));
                gui_request(
                    "spawn_agent",
                    json!({ "task_id": id, "agent": agent, "prompt": prompt }),
                )
                .map(|result| println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default()))
            }
            None => Err(anyhow::anyhow!(
                "使い方: shirushi fleet spawn-agent <task-id> [agent] [prompt...]"
            )),
        },
        Some("send") => match (args.get(2), args.len() > 3) {
            (Some(id), true) => gui_request(
                "send",
                json!({ "task_id": id, "message": args[3..].join(" ") }),
            )
            .map(|result| println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default())),
            _ => Err(anyhow::anyhow!("使い方: shirushi fleet send <task-id> <message...>")),
        },
        Some("digest") => match args.get(2) {
            Some(id) => gui_request("digest", json!({ "task_id": id }))
                // GUI 不在時は台帳のみで答える（phase/result_summary は再起動を跨いで残る）。
                .or_else(|gui_error| {
                    task_by_id(id).map(|record| {
                        json!({
                            "task_id": record.id,
                            "title": record.title,
                            "kind": record.kind.as_str(),
                            "phase": record.phase.as_str(),
                            "branch": record.branch,
                            "result_summary": record.result_summary,
                            "gui": format!("offline（{gui_error:#}）"),
                        })
                    })
                })
                .map(|result| println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default())),
            None => Err(anyhow::anyhow!("使い方: shirushi fleet digest <task-id>")),
        },
        Some("events") => {
            let since = args.get(2).and_then(|value| value.parse().ok()).unwrap_or(0);
            events_since(since)
                .map(|result| println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default()))
        }
        _ => Err(anyhow::anyhow!(
            "使い方: shirushi fleet <create [root] [title] | list [root] | status <id> <phase> [summary] | wait <id> <phase|activity> [seconds] | wait-deps <id> <phase> [seconds] | depend <id> <on...> | review <id> [root] | integrate <id> [root] | spawn-agent <id> [agent] [prompt...] | send <id> <message...> | digest <id> | events [since-id]>"
        )),
    };
    if let Err(error) = result {
        eprintln!("{error:#}");
    }
    true
}
