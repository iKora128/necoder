//! Worktree-native Fleet orchestration API.
//!
//! GUI / CLI / MCP は同じ `storage::TaskSpaceRecord` と project の Git safety gate を使う。
//! Captain はこの API で Task を作り、Agent は status/result を報告し、別プロセスは永続 DB を
//! poll して待てる。GUI プロセスの一時 state に依存しないため再起動後も継続可能。
//!
//! CLI の `<task>` は id だけでなく `branch:` / `name:` / `active`（GUI で選択中）でも指せる
//! （[`TaskSelector`]）。指し方の形は stablyai/orca@646e9a5 の
//! `docs/site/content/docs/cli/reference.mdx` §Selectors（MIT）を参考にし、解決の順序と
//! 範囲（リポジトリ内・統合先は id のみ）は necoder の台帳に合わせて独立に決めた。

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
    let path = std::env::var("NECODER_DB")
        .map(PathBuf::from)
        .ok()
        .or_else(storage::default_db_path)
        .context("Task ledger の保存先を決められません")?;
    Storage::open(&path)
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

/// `necoder fleet` のサブコマンド 1 つ（名前・引数の書式・要旨）。
///
/// 引数が足りない時の「使い方」と `necoder skills get` の本文がこの表を共有する
/// ＝サブコマンドを足したら両方に出る（手で書いた説明が実装とずれない）。
pub(crate) struct FleetCommand {
    pub(crate) name: &'static str,
    /// `name` に続く引数の書式（`<必須>` / `[省略可]` / `...` = 残りの引数を空白で繋いで 1 つにする）。
    pub(crate) arguments: &'static str,
    pub(crate) summary: &'static str,
    /// 起動中の GUI が要る（GUI への IPC で動く）。
    pub(crate) needs_gui: bool,
    /// 人間だけが実行する操作（エージェントは実行しない・FLEET-V2 §5.2）。
    pub(crate) human_only: bool,
}

/// `necoder fleet` の全サブコマンド（`run_cli` の分岐と同じ並び）。
pub(crate) const FLEET_COMMANDS: &[FleetCommand] = &[
    FleetCommand {
        name: "create",
        arguments: "[root] [title]",
        summary: "統合先の HEAD から task/* ブランチと worktree を作り、台帳に登録する（出力の id を以後の引数に使う）",
        needs_gui: false,
        human_only: false,
    },
    FleetCommand {
        name: "list",
        arguments: "[root]",
        summary: "このリポジトリの Task を一覧する",
        needs_gui: false,
        human_only: false,
    },
    FleetCommand {
        name: "status",
        arguments: "<task> <phase> [summary]",
        summary: "Task の phase を進め、要約を残す（報告）",
        needs_gui: false,
        human_only: false,
    },
    FleetCommand {
        name: "wait",
        arguments: "<task> <phase|activity> [timeout-seconds]",
        summary: "phase（台帳）か activity（GUI の今の動き）が指定の値になるまで待つ（既定 600 秒。activity は要 GUI）",
        needs_gui: false,
        human_only: false,
    },
    FleetCommand {
        name: "depend",
        arguments: "<task> <depends-on...>",
        summary: "依存する Task を全部置き換えで宣言する",
        needs_gui: false,
        human_only: false,
    },
    FleetCommand {
        name: "wait-deps",
        arguments: "<task> <phase> [timeout-seconds]",
        summary: "依存がすべて指定の phase になるまで待つ（既定 600 秒）",
        needs_gui: false,
        human_only: false,
    },
    FleetCommand {
        name: "review",
        arguments: "<task> [integration-root]",
        summary: "Conflict Radar（統合先を変えない merge の試算）を走らせ、merge_ready か changes_requested へ進める",
        needs_gui: false,
        human_only: false,
    },
    FleetCommand {
        name: "integrate",
        arguments: "<task> [integration-root]",
        summary: "merge_ready の Task を統合先へ merge する",
        needs_gui: false,
        human_only: true,
    },
    FleetCommand {
        name: "spawn-agent",
        arguments: "<task> [agent] [prompt...]",
        summary: "Task の worktree でエージェントのスレッドを起こし、prompt があれば送る",
        needs_gui: true,
        human_only: false,
    },
    FleetCommand {
        name: "send",
        arguments: "<task> <message...>",
        summary: "Task のアクティブなスレッドへ追撃の prompt を送る",
        needs_gui: true,
        human_only: false,
    },
    FleetCommand {
        name: "digest",
        arguments: "<task>",
        summary: "Task の事実と要約（phase・計画・digest・トークン）。transcript は返さない。GUI が無ければ台帳の分だけ",
        needs_gui: false,
        human_only: false,
    },
    FleetCommand {
        name: "events",
        arguments: "[since-id]",
        summary: "全 Task の台帳イベントを、since-id より後から古い順に最大 200 件",
        needs_gui: false,
        human_only: false,
    },
];

/// `fleet wait` が待てる activity（GUI の今の動き。台帳の phase とは別の軸）。
pub(crate) const ACTIVITIES: &[&str] = &["idle", "working", "blocked", "done", "interrupted"];

/// `necoder fleet <name>` の引数が足りない時のエラー（表の書式で「使い方」を出す）。
fn usage_error(name: &str) -> anyhow::Error {
    match FLEET_COMMANDS.iter().find(|command| command.name == name) {
        Some(command) => anyhow::anyhow!(
            "使い方: necoder fleet {} {}",
            command.name,
            command.arguments
        ),
        None => anyhow::anyhow!("{}", fleet_usage()),
    }
}

/// 全サブコマンドの「使い方」1 行。
fn fleet_usage() -> String {
    let commands = FLEET_COMMANDS
        .iter()
        .map(|command| format!("{} {}", command.name, command.arguments))
        .collect::<Vec<_>>()
        .join(" | ");
    format!("使い方: necoder fleet <{commands}>")
}

/// CLI/MCP 境界の文字列 → enum。不正値はエラー（有効値の一覧つき）。
pub(crate) fn parse_phase(value: &str) -> Result<TaskPhase> {
    TaskPhase::from_str(value).with_context(|| {
        let valid = TaskPhase::ALL.map(TaskPhase::as_str).join(" | ");
        format!("不正な phase: {value}（有効: {valid}）")
    })
}

/// 起動中 GUI への IPC（P5・1 接続 1 リクエスト・1 行 JSON）。GUI が居なければ明確なエラー。
/// 守るべき操作（spawn/send）はこの経路 = necoder の CLI/MCP にだけ置く（計画 §0-8）。
pub(crate) fn gui_request(method: &str, params: Value) -> Result<Value> {
    use std::io::{BufRead as _, BufReader, Write as _};
    // 口の実体は unix socket / 名前付きパイプ（WINDOWS-PORT.md §D2）。ここは中身を知らない。
    let socket_path = workspace::control_socket_path()
        .context("GUI socket の場所が決められません（ホームディレクトリが分からない）")?;
    let mut stream = workspace::ControlStream::connect(&socket_path).with_context(|| {
        format!(
            "GUI が起動していません（{} に接続できない）。necoder を開いてから実行してください",
            socket_path.display()
        )
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .context("IPC timeout 設定に失敗")?;
    let request = json!({ "method": method, "params": params });
    writeln!(stream, "{request}").context("IPC 送信に失敗")?;
    stream.flush().context("IPC 送信に失敗")?;
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
/// GUI 不在なら DB 直読み・稼働中（ロック）は IPC（P5）。Captain/CLI は最後の id を覚えて差分だけ読む。
pub(crate) fn events_since(since_id: i64) -> Result<Value> {
    let events =
        match open_storage().and_then(|storage| storage.load_task_events_since(since_id, 200)) {
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
    let root = paths::canonicalize(root).context("IntegrationSpace を開けません")?;
    let host = LocalHost::shared();
    let base_oid =
        project::git_head_oid_on(host.as_ref(), &root).context("Git repository ではありません")?;
    let repository_id = project::repository_id_on(host.as_ref(), &root);
    let (target, branch, setup_failure) = project::create_named_task_on(host.as_ref(), &root, title, true)?;
    let target = paths::canonicalize(&target).unwrap_or(target);
    let now = unix_ms();
    let record = TaskSpaceRecord {
        id: project::stable_worktree_id_on(host.as_ref(), &target),
        repository_id,
        root: target,
        branch: Some(branch),
        title: title.to_string(),
        kind: SpaceKind::Task,
        phase: if setup_failure.is_some() { TaskPhase::Failed } else { TaskPhase::Planned },
        base_oid: Some(base_oid.clone()),
        head_oid: Some(base_oid),
        result_summary: setup_failure,
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
        id: value
            .get("id")
            .and_then(Value::as_str)
            .context("record id")?
            .to_string(),
        repository_id: value
            .get("repository_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        root: PathBuf::from(
            value
                .get("root")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        branch: value
            .get("branch")
            .and_then(Value::as_str)
            .map(str::to_string),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
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
        base_oid: value
            .get("base_oid")
            .and_then(Value::as_str)
            .map(str::to_string),
        head_oid: value
            .get("head_oid")
            .and_then(Value::as_str)
            .map(str::to_string),
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

/// 台帳の全 Task（全リポジトリ）。GUI 稼働中（DB ロック）は GUI の単一 writer に読んでもらう。
fn load_task_records() -> Result<Vec<TaskSpaceRecord>> {
    match open_storage().and_then(|storage| storage.load_task_spaces()) {
        Ok(records) => Ok(records),
        Err(error) if is_lock_error(&error) => Ok(gui_request("tasks", json!({}))?
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| record_from_json(value).ok())
                    .collect()
            })
            .unwrap_or_default()),
        Err(error) => Err(error),
    }
}

/// `root` のリポジトリの台帳上の id（worktree のどこから見ても同じ・git の共通ディレクトリ基準）。
fn repository_id_of(root: &Path) -> String {
    let root = paths::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    project::repository_id_on(&LocalHost, &root)
}

pub(crate) fn list_tasks(root: &Path) -> Result<Vec<TaskSpaceRecord>> {
    let repository_id = repository_id_of(root);
    Ok(load_task_records()?
        .into_iter()
        .filter(|task| task.repository_id == repository_id)
        .collect())
}

/// CLI の `<task>` の指し方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskSelector {
    /// `active` = GUI で選択中の Task（GUI に聞く）。
    Active,
    /// `id:<id>`
    Id(String),
    /// `branch:<ブランチ>`（`task/` は省ける）
    Branch(String),
    /// `name:<名前>`（Task の名前と完全一致）
    Name(String),
    /// 前置きなし: id → ブランチ → 名前の順に探す。
    Any(String),
}

impl TaskSelector {
    pub(crate) fn parse(raw: &str) -> Self {
        if raw == "active" {
            return Self::Active;
        }
        if let Some(id) = raw.strip_prefix("id:") {
            return Self::Id(id.to_string());
        }
        if let Some(branch) = raw.strip_prefix("branch:") {
            return Self::Branch(branch.to_string());
        }
        if let Some(name) = raw.strip_prefix("name:") {
            return Self::Name(name.to_string());
        }
        Self::Any(raw.to_string())
    }
}

impl std::fmt::Display for TaskSelector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(formatter, "active"),
            Self::Id(id) => write!(formatter, "id:{id}"),
            Self::Branch(branch) => write!(formatter, "branch:{branch}"),
            Self::Name(name) => write!(formatter, "name:{name}"),
            Self::Any(value) => write!(formatter, "{value}"),
        }
    }
}

/// 台帳の記録から、selector に合う Task を 1 つだけ選ぶ（純関数）。
///
/// - ブランチと名前は `repository_id`（今いるリポジトリ）の Task の中だけで探す＝別のリポジトリの
///   同名を拾わない。id は全リポジトリで一意なので、どこからでも指せる
/// - 統合先（main）は id で指した時だけ選べる（名前やブランチの取り違えで main を操作しない）
/// - 1 つに絞れなければエラー（候補を並べる）。当て推量で片方を選ばない
pub(crate) fn select_task(
    records: &[TaskSpaceRecord],
    selector: &TaskSelector,
    repository_id: &str,
) -> Result<TaskSpaceRecord> {
    let in_repository = || {
        records.iter().filter(|record| {
            record.repository_id == repository_id && record.kind == SpaceKind::Task
        })
    };
    let by_branch = |branch: &str| -> Vec<&TaskSpaceRecord> {
        in_repository()
            .filter(|record| {
                record.branch.as_deref().is_some_and(|candidate| {
                    candidate == branch || candidate.strip_prefix("task/") == Some(branch)
                })
            })
            .collect()
    };
    let by_name = |name: &str| -> Vec<&TaskSpaceRecord> {
        in_repository()
            .filter(|record| record.title == name)
            .collect()
    };
    let by_id = |id: &str| -> Vec<&TaskSpaceRecord> {
        records.iter().filter(|record| record.id == id).collect()
    };
    let found = match selector {
        TaskSelector::Active => anyhow::bail!("active は GUI に問い合わせて解決します"),
        TaskSelector::Id(id) => by_id(id),
        TaskSelector::Branch(branch) => by_branch(branch),
        TaskSelector::Name(name) => by_name(name),
        TaskSelector::Any(value) => {
            let mut found = by_id(value);
            if found.is_empty() {
                found = by_branch(value);
            }
            if found.is_empty() {
                found = by_name(value);
            }
            found
        }
    };
    let describe = |record: &TaskSpaceRecord| {
        format!(
            "  {} | {} | {}",
            record.id,
            record.title,
            record.branch.as_deref().unwrap_or("-")
        )
    };
    match found.as_slice() {
        [record] => Ok((*record).clone()),
        [] => {
            let candidates: Vec<String> = in_repository().take(20).map(describe).collect();
            if candidates.is_empty() {
                anyhow::bail!(
                    "Task が見つかりません: {selector}（このリポジトリに Task がありません）"
                )
            }
            anyhow::bail!(
                "Task が見つかりません: {selector}\nこのリポジトリの Task（id | 名前 | ブランチ）:\n{}",
                candidates.join("\n")
            )
        }
        many => {
            anyhow::bail!(
            "Task を 1 つに絞れません: {selector}（{} 件）。id か branch: で指してください:\n{}",
            many.len(),
            many.iter().map(|record| describe(record)).collect::<Vec<_>>().join("\n")
        )
        }
    }
}

/// `<task>` を台帳の Task に解決する（`active` は GUI に、それ以外は台帳に聞く）。
/// ブランチと名前は今いるフォルダのリポジトリの中で探す。
pub(crate) fn resolve_task(raw: &str) -> Result<TaskSpaceRecord> {
    let selector = TaskSelector::parse(raw);
    if selector == TaskSelector::Active {
        let active = gui_request("active_task", json!({}))?;
        let id = active
            .get("task_id")
            .and_then(Value::as_str)
            .context("GUI の応答に task_id がありません")?;
        return task_by_id(id);
    }
    let cwd = std::env::current_dir().context("カレントディレクトリが分かりません")?;
    select_task(&load_task_records()?, &selector, &repository_id_of(&cwd))
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
        update_task(
            task_id,
            TaskPhase::MergeReady,
            Some("Conflict Radar: clean"),
        )
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

/// `necoder fleet …` を処理したら true。GUI は開かない。失敗は終了コード 1
/// （`cli.rs` と同じ。Captain やスクリプトが `&&` と終了コードで成否を判断できるように）。
pub(crate) fn run_cli() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("fleet") {
        return false;
    }
    if let Err(error) = run(&args[1..]) {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
    true
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

/// `fleet` に続く引数を処理する。`<task>` はすべて [`resolve_task`] で台帳の Task に解決してから使う。
fn run(args: &[String]) -> Result<()> {
    let argument = |index: usize| args.get(index).map(String::as_str);
    let root_at = |index: usize| {
        args.get(index)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    let seconds_at = |index: usize| {
        args.get(index)
            .and_then(|value| value.parse().ok())
            .unwrap_or(600)
    };
    match argument(0) {
        Some("create") => print_record(&create_task(&root_at(1), argument(2).unwrap_or("Task"))?),
        Some("list") => {
            let values: Vec<Value> = list_tasks(&root_at(1))?.iter().map(record_json).collect();
            print_json(&Value::Array(values));
        }
        Some("status") => {
            let (Some(task), Some(phase)) = (argument(1), argument(2)) else {
                return Err(usage_error("status"));
            };
            let task = resolve_task(task)?;
            print_record(&update_task(&task.id, parse_phase(phase)?, argument(3))?);
        }
        Some("wait") => {
            let (Some(task), Some(target)) = (argument(1), argument(2)) else {
                return Err(usage_error("wait"));
            };
            let task = resolve_task(task)?;
            let timeout = Duration::from_secs(seconds_at(3));
            // phase（台帳）と activity（GUI live）の両対応（P6）。まず phase として解釈。
            match parse_phase(target) {
                Ok(phase) => print_record(&wait_task(&task.id, phase, timeout)?),
                Err(_) => print_json(&wait_activity(&task.id, target, timeout)?),
            }
        }
        Some("depend") => {
            let Some(task) = argument(1).filter(|_| args.len() > 2) else {
                return Err(usage_error("depend"));
            };
            let task = resolve_task(task)?;
            let depends_on = args[2..]
                .iter()
                .map(|dependency| resolve_task(dependency).map(|record| record.id))
                .collect::<Result<Vec<_>>>()?;
            print_record(&set_depends(&task.id, &depends_on)?);
        }
        Some("wait-deps") => {
            let (Some(task), Some(phase)) = (argument(1), argument(2)) else {
                return Err(usage_error("wait-deps"));
            };
            let task = resolve_task(task)?;
            let timeout = Duration::from_secs(seconds_at(3));
            print_json(&wait_deps(&task.id, parse_phase(phase)?, timeout)?);
        }
        Some("review") => {
            let Some(task) = argument(1) else {
                return Err(usage_error("review"));
            };
            let task = resolve_task(task)?;
            print_record(&review_task(&task.id, &root_at(2))?);
        }
        Some("integrate") => {
            let Some(task) = argument(1) else {
                return Err(usage_error("integrate"));
            };
            let task = resolve_task(task)?;
            print_record(&integrate_task(&task.id, &root_at(2))?);
        }
        // ── ここから GUI ライブ制御（P5・要 GUI 起動） ──
        Some("spawn-agent") => {
            let Some(task) = argument(1) else {
                return Err(usage_error("spawn-agent"));
            };
            let task = resolve_task(task)?;
            let prompt = (args.len() > 3).then(|| args[3..].join(" "));
            print_json(&gui_request(
                "spawn_agent",
                json!({ "task_id": task.id, "agent": argument(2), "prompt": prompt }),
            )?);
        }
        Some("send") => {
            let Some(task) = argument(1).filter(|_| args.len() > 2) else {
                return Err(usage_error("send"));
            };
            let task = resolve_task(task)?;
            print_json(&gui_request(
                "send",
                json!({ "task_id": task.id, "message": args[2..].join(" ") }),
            )?);
        }
        Some("digest") => {
            let Some(task) = argument(1) else {
                return Err(usage_error("digest"));
            };
            let task = resolve_task(task)?;
            // GUI 不在時は台帳のみで答える（phase/result_summary は再起動を跨いで残る）。
            let digest =
                gui_request("digest", json!({ "task_id": task.id })).unwrap_or_else(|gui_error| {
                    json!({
                        "task_id": task.id,
                        "title": task.title,
                        "kind": task.kind.as_str(),
                        "phase": task.phase.as_str(),
                        "branch": task.branch,
                        "result_summary": task.result_summary,
                        "gui": format!("offline（{gui_error:#}）"),
                    })
                });
            print_json(&digest);
        }
        Some("events") => {
            let since = argument(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            print_json(&events_since(since)?);
        }
        _ => anyhow::bail!("{}", fleet_usage()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        id: &str,
        repository: &str,
        title: &str,
        branch: &str,
        kind: SpaceKind,
    ) -> TaskSpaceRecord {
        TaskSpaceRecord {
            id: id.to_string(),
            repository_id: repository.to_string(),
            root: PathBuf::from(format!("/work/{id}")),
            branch: Some(branch.to_string()),
            title: title.to_string(),
            kind,
            phase: TaskPhase::Working,
            base_oid: None,
            head_oid: None,
            result_summary: None,
            depends_on: Vec::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn ledger() -> Vec<TaskSpaceRecord> {
        vec![
            record(
                "main-space",
                "repo-a",
                "main",
                "main",
                SpaceKind::Integration,
            ),
            record(
                "space-login",
                "repo-a",
                "Fix login",
                "task/fix-login",
                SpaceKind::Task,
            ),
            record("space-docs", "repo-a", "Docs", "task/docs", SpaceKind::Task),
            // 別リポジトリの同名 Task（名前・ブランチでは拾わない）。
            record(
                "space-other",
                "repo-b",
                "Docs",
                "task/docs",
                SpaceKind::Task,
            ),
            // 名前だけが他の Task のブランチ名と同じ Task（ブランチが先に効く）。
            record(
                "space-tricky",
                "repo-a",
                "task/docs",
                "task/tricky",
                SpaceKind::Task,
            ),
            // 名前が重なる 2 件（絞れない）。
            record(
                "space-dup-1",
                "repo-a",
                "Refactor",
                "task/refactor",
                SpaceKind::Task,
            ),
            record(
                "space-dup-2",
                "repo-a",
                "Refactor",
                "task/refactor-2",
                SpaceKind::Task,
            ),
        ]
    }

    fn select(raw: &str) -> Result<String> {
        select_task(&ledger(), &TaskSelector::parse(raw), "repo-a").map(|record| record.id)
    }

    #[test]
    fn selectors_parse_prefixes() {
        assert_eq!(TaskSelector::parse("active"), TaskSelector::Active);
        assert_eq!(TaskSelector::parse("id:x"), TaskSelector::Id("x".into()));
        assert_eq!(
            TaskSelector::parse("branch:task/x"),
            TaskSelector::Branch("task/x".into())
        );
        assert_eq!(
            TaskSelector::parse("name:Fix login"),
            TaskSelector::Name("Fix login".into())
        );
        assert_eq!(TaskSelector::parse("x"), TaskSelector::Any("x".into()));
        assert_eq!(TaskSelector::parse("branch:y").to_string(), "branch:y");
    }

    #[test]
    fn selectors_resolve_by_id_branch_and_name_in_this_repository() {
        assert_eq!(select("space-login").expect("id"), "space-login");
        assert_eq!(
            select("id:space-other").expect("id は全リポジトリで引ける"),
            "space-other"
        );
        assert_eq!(
            select("branch:task/fix-login").expect("branch"),
            "space-login"
        );
        // `task/` は省ける。
        assert_eq!(select("branch:fix-login").expect("branch"), "space-login");
        assert_eq!(
            select("fix-login").expect("前置きなしのブランチ"),
            "space-login"
        );
        assert_eq!(select("name:Fix login").expect("name"), "space-login");
        assert_eq!(
            select("Fix login").expect("前置きなしの名前"),
            "space-login"
        );
        // 前置きなしはブランチが名前より先（task/docs は space-tricky の名前でもある）。
        assert_eq!(select("task/docs").expect("ブランチが先"), "space-docs");
        assert_eq!(
            select("name:task/docs").expect("name で明示"),
            "space-tricky"
        );
        // 別リポジトリの同名は候補に入らない（repo-a の Docs だけ）。
        assert_eq!(
            select("name:Docs").expect("このリポジトリの 1 件"),
            "space-docs"
        );
    }

    #[test]
    fn selectors_refuse_ambiguity_missing_tasks_and_the_integration_space() {
        let ambiguous = select("Refactor").expect_err("2 件に当たる");
        assert!(
            ambiguous.to_string().contains("1 つに絞れません"),
            "{ambiguous}"
        );
        assert!(ambiguous.to_string().contains("space-dup-2"), "{ambiguous}");
        let missing = select("nothing").expect_err("無い");
        assert!(
            missing.to_string().contains("Fix login"),
            "候補を並べる: {missing}"
        );
        // 統合先はブランチや名前では選べない（id なら選べる）。
        assert!(select("branch:main").is_err());
        assert!(select("name:main").is_err());
        assert_eq!(select("main-space").expect("id"), "main-space");
        // active は GUI に聞くもの（台帳だけでは解決しない）。
        assert!(select("active").is_err());
    }
}
