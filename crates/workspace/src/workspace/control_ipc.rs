//! GUI ライブ制御 IPC（FLEET-CONTROL-PLAN P5・`mcp.rs` 冒頭で予告していた「後続」の本体）。
//!
//! 起動中の GUI が Unix socket（`~/.necoder/gui.sock`・0600）で headless CLI/MCP からの
//! 制御を受ける。これで **spawn の断絶**（`fleet_create_task` は worktree を作るだけで
//! エージェントを起こせない）が解消し、監督（P6）が編隊を実際に動かせるようになる。
//!
//! **単一 writer の原則（Turso は排他ロック）**: GUI が生きている間、Task ledger の読み書きは
//! GUI のストレージハンドル（1 本のワーカースレッド）に集約する。headless CLI/MCP は DB が
//! ロックされていたらこの socket 経由で読む/書く（GUI 不在時は従来どおり直接 DB）。
//!
//! 設計:
//! - プロトコル = 1 接続 1 リクエスト。1 行 JSON 要求 → 1 行 JSON 応答（`{"ok":bool,...}`）。
//! - accept ループは std スレッド。**I/O はしない**（task_id をそのまま UI へ渡し、UI 側は
//!   メモリで解決 → 足りない時だけ background executor + GUI のストレージハンドルで読む。
//!   Host/DB を UI スレッドで呼ばない規律・ARCHITECTURE §9）。
//! - 守るべき操作（spawn / send / 遷移）はこの socket = **necoder の MCP/CLI にだけ**置く
//!   （計画 §0-8。Herdr socket 直叩きは台帳と permission の迂回路になるため作らない）。
//! - digest は **3 段圧縮**を守る: 事実層 + Tier1（+キャッシュ済み Tier2）のみ。transcript は返さない。

use crate::workspace::*;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};

/// GUI 制御 socket のパス（`NECODER_GUI_SOCK` で差し替え可・テスト用）。
/// macOS の `SUN_LEN`（~104B）に収まる短いパスであること。
pub fn control_socket_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NECODER_GUI_SOCK") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".necoder/gui.sock"))
}

/// accept スレッド → UI スレッドへ渡す 1 仕事（I/O 前・生パラメータのまま）。
pub(crate) struct ControlJob {
    method: String,
    params: serde_json::Value,
    /// 応答（1 行 JSON）。accept スレッドが recv_timeout で待っている。
    respond: std::sync::mpsc::Sender<serde_json::Value>,
}

fn ok(value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "ok": true, "result": value })
}

fn err(message: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({ "ok": false, "error": message.to_string() })
}

fn record_json(record: &storage::TaskSpaceRecord) -> serde_json::Value {
    serde_json::json!({
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

impl Workspace {
    /// IPC サーバを起動する（main から 1 回）。二重起動（別窓・別プロセス）は静かにスキップ。
    pub fn start_control_ipc(&mut self, cx: &mut Context<Self>) {
        let Some(socket_path) = control_socket_path() else {
            return;
        };
        if let Some(parent) = socket_path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        // 生きている GUI が既に居るなら二重 bind しない。死んだ socket ファイルなら除去して継ぐ。
        if UnixStream::connect(&socket_path).is_ok() {
            return;
        }
        let _ = std::fs::remove_file(&socket_path);
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("管制 IPC を開けない: {error}");
                return;
            }
        };
        // 0600: 同一ユーザーのみ（socket は uid で守られるが明示しておく）。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
        }
        let (job_tx, mut job_rx) = futures::channel::mpsc::unbounded::<ControlJob>();
        // accept ループ（std スレッド）: 解析だけしてジョブ化。I/O はしない。
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let job_tx = job_tx.clone();
                std::thread::spawn(move || serve_connection(stream, job_tx));
            }
        });
        // UI 側の消費ループ。
        cx.spawn(async move |workspace, cx| {
            use futures::StreamExt as _;
            while let Some(job) = job_rx.next().await {
                let handled = workspace.update(cx, |workspace, cx| {
                    workspace.handle_control_job(job, cx);
                });
                if handled.is_err() {
                    break; // window ごと閉じた
                }
            }
        })
        .detach();
    }

    /// UI スレッドでの 1 仕事。メモリで済むものは即応答・DB が要るものは background へ
    /// （GUI のストレージハンドル = 単一ワーカーを使うので headless とロック衝突しない）。
    fn handle_control_job(&mut self, job: ControlJob, cx: &mut Context<Self>) {
        let ControlJob {
            method,
            params,
            respond,
        } = job;
        let task_id = params
            .get("task_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        match method.as_str() {
            "spawn_agent" => {
                let agent = params
                    .get("agent")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let prompt = params
                    .get("prompt")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                // 既に開いていれば即時。無ければ record を background で解決してから開く。
                if let Some(index) = self.session_index_by_task(&task_id) {
                    let result = self.ipc_spawn_into(index, agent, prompt, cx);
                    let _ = respond.send(ok(result));
                    return;
                }
                let Some(storage) = self.persistence.storage.clone() else {
                    let _ = respond.send(err(i18n::t!("ipc.err_no_storage")));
                    return;
                };
                cx.spawn(async move |workspace, cx| {
                    let task_id_for_load = task_id.clone();
                    let record = cx
                        .background_executor()
                        .spawn(async move {
                            storage.load_task_spaces().map(|records| {
                                records
                                    .into_iter()
                                    .find(|record| record.id == task_id_for_load)
                            })
                        })
                        .await;
                    let response = match record {
                        Ok(Some(record)) => workspace
                            .update(cx, |workspace, cx| {
                                workspace.open_folder_in_rail(
                                    host::LocalHost::shared(),
                                    record.root.clone(),
                                    record.branch.clone(),
                                    cx,
                                );
                                match workspace.session_index_by_task(&record.id) {
                                    Some(index) => {
                                        ok(workspace.ipc_spawn_into(index, agent, prompt, cx))
                                    }
                                    None => err(i18n::t!(
                                        "ipc.err_open_worktree",
                                        "path" => record.root.display()
                                    )),
                                }
                            })
                            .unwrap_or_else(|_| err(i18n::t!("ipc.err_gui_gone"))),
                        Ok(None) => err(i18n::t!("ipc.err_task_not_found", "id" => task_id)),
                        Err(error) => err(format!("{error:#}")),
                    };
                    let _ = respond.send(response);
                })
                .detach();
            }
            "send" => {
                let Some(message) = params
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                else {
                    let _ = respond.send(err(i18n::t!("ipc.err_message_required")));
                    return;
                };
                let Some(index) = self.session_index_by_task(&task_id) else {
                    let _ = respond.send(err(i18n::t!("ipc.err_task_not_open_spawn")));
                    return;
                };
                let panel = self.project_sessions.sessions[index].agent_panel.clone();
                panel.update(cx, |panel, cx| panel.send_prompt_text(message, cx));
                cx.notify();
                let _ = respond.send(ok(serde_json::json!({ "session_index": index })));
            }
            "digest" => {
                let response = match self.session_index_by_task(&task_id) {
                    Some(index) => ok(self.ipc_digest(index, cx)),
                    None => err(i18n::t!("ipc.err_task_not_open")),
                };
                let _ = respond.send(response);
            }
            "task" => {
                // 開いている slot はメモリが最鮮度（GUI が唯一の writer）。無ければ台帳から。
                if let Some(index) = self.session_index_by_task(&task_id) {
                    let slot = &self.project_sessions.projects[index];
                    let record = slot.task_space.to_record(slot);
                    let _ = respond.send(ok(record_json(&record)));
                    return;
                }
                self.respond_from_storage(
                    respond,
                    move |storage| {
                        let record = storage
                            .load_task_spaces()?
                            .into_iter()
                            .find(|record| record.id == task_id)
                            .ok_or_else(|| {
                                anyhow::anyhow!(i18n::t!("ipc.err_task_not_found", "id" => task_id))
                            })?;
                        Ok(record_json(&record))
                    },
                    cx,
                );
            }
            "tasks" => {
                self.respond_from_storage(
                    respond,
                    move |storage| {
                        Ok(serde_json::Value::Array(
                            storage
                                .load_task_spaces()?
                                .into_iter()
                                .map(|record| record_json(&record))
                                .collect(),
                        ))
                    },
                    cx,
                );
            }
            "update_task" => {
                let Some(phase) = params
                    .get("phase")
                    .and_then(serde_json::Value::as_str)
                    .and_then(TaskPhase::from_str)
                else {
                    let _ = respond.send(err(i18n::t!("ipc.err_bad_phase")));
                    return;
                };
                let summary = params
                    .get("summary")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                // 開いている slot は GUI の遷移入口へ（メモリ+台帳+ニュースが揃う）。
                if let Some(index) = self.session_index_by_task(&task_id) {
                    if let Some(slot) = self.project_sessions.projects.get_mut(index) {
                        if let Some(summary) = &summary {
                            slot.task_space.result_summary =
                                Some(SharedString::from(summary.clone()));
                        }
                    }
                    self.transition_task_space(
                        index,
                        phase,
                        "orchestration_api",
                        summary.as_deref(),
                        cx,
                    );
                    let slot = &self.project_sessions.projects[index];
                    let record = slot.task_space.to_record(slot);
                    let _ = respond.send(ok(record_json(&record)));
                    return;
                }
                // 開いていない Task は台帳だけ進める（headless update と同じ内容を GUI の handle で）。
                self.respond_from_storage(
                    respond,
                    move |storage| {
                        let mut record = storage
                            .load_task_spaces()?
                            .into_iter()
                            .find(|record| record.id == task_id)
                            .ok_or_else(|| {
                                anyhow::anyhow!(i18n::t!("ipc.err_task_not_found", "id" => task_id))
                            })?;
                        record.phase = phase;
                        record.result_summary = summary.clone().or(record.result_summary);
                        let payload = serde_json::json!({
                            "phase": phase.as_str(),
                            "source": "orchestration_api_via_gui",
                            "summary": summary,
                        })
                        .to_string();
                        storage.commit_task_transition(&record, &payload)?;
                        Ok(record_json(&record))
                    },
                    cx,
                );
            }
            "record_task" => {
                // headless `fleet create` の台帳登録（worktree は CLI 側で作成済み・GUI = 単一 writer）。
                let record = storage::TaskSpaceRecord {
                    // record JSON は "id"（record_json の形）。"task_id" は他 method の慣例なので両対応。
                    id: params
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&task_id)
                        .to_string(),
                    repository_id: params
                        .get("repository_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    root: PathBuf::from(
                        params
                            .get("root")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default(),
                    ),
                    branch: params
                        .get("branch")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    title: params
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    kind: if params.get("kind").and_then(serde_json::Value::as_str)
                        == Some("integration")
                    {
                        SpaceKind::Integration
                    } else {
                        SpaceKind::Task
                    },
                    phase: params
                        .get("phase")
                        .and_then(serde_json::Value::as_str)
                        .and_then(TaskPhase::from_str)
                        .unwrap_or(TaskPhase::Planned),
                    base_oid: params
                        .get("base_oid")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    head_oid: params
                        .get("head_oid")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    result_summary: None,
                    depends_on: Vec::new(),
                    created_at: params
                        .get("created_at")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                    updated_at: params
                        .get("updated_at")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                };
                if record.id.is_empty() {
                    let _ = respond.send(err(i18n::t!("ipc.err_record_id_required")));
                    return;
                }
                self.respond_from_storage(respond, move |storage| {
                    storage.upsert_task_space(&record)?;
                    storage.append_task_event(
                        &record.id,
                        "task_created",
                        &serde_json::json!({ "source": "orchestration_api_via_gui", "root": record.root })
                            .to_string(),
                    )?;
                    Ok(record_json(&record))
                }, cx);
            }
            "set_depends" => {
                let depends_on: Vec<String> = params
                    .get("depends_on")
                    .and_then(serde_json::Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                self.respond_from_storage(
                    respond,
                    move |storage| {
                        storage.set_task_depends(&task_id, &depends_on)?;
                        Ok(serde_json::json!({ "task_id": task_id, "depends_on": depends_on }))
                    },
                    cx,
                );
            }
            "events" => {
                let since = params
                    .get("since_id")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                self.respond_from_storage(respond, move |storage| {
                    Ok(serde_json::Value::Array(
                        storage
                            .load_task_events_since(since, 200)?
                            .into_iter()
                            .map(|event| {
                                serde_json::json!({
                                    "id": event.id,
                                    "task_id": event.task_id,
                                    "kind": event.kind,
                                    "payload": serde_json::from_str::<serde_json::Value>(&event.payload)
                                        .unwrap_or(serde_json::Value::String(event.payload)),
                                    "created_at": event.created_at,
                                })
                            })
                            .collect(),
                    ))
                }, cx);
            }
            other => {
                let _ = respond.send(err(i18n::t!("ipc.err_unknown_method", "name" => other)));
            }
        }
    }

    /// GUI のストレージハンドル（単一ワーカー）で読み書きして応答する（UI スレッドをブロックしない）。
    fn respond_from_storage(
        &self,
        respond: std::sync::mpsc::Sender<serde_json::Value>,
        operation: impl FnOnce(&storage::Storage) -> anyhow::Result<serde_json::Value> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(storage) = self.persistence.storage.clone() else {
            let _ = respond.send(err(i18n::t!("ipc.err_no_storage")));
            return;
        };
        cx.background_executor()
            .spawn(async move {
                let response = match operation(&storage) {
                    Ok(value) => ok(value),
                    Err(error) => err(format!("{error:#}")),
                };
                let _ = respond.send(response);
            })
            .detach();
    }

    fn session_index_by_task(&self, task_id: &str) -> Option<usize> {
        self.project_sessions
            .projects
            .iter()
            .position(|slot| slot.task_space.id.as_str() == task_id)
    }

    /// slot が開いている前提で thread を起こし（空スレッドは使い回し）、必要なら prompt を送る。
    fn ipc_spawn_into(
        &mut self,
        index: usize,
        agent: Option<String>,
        prompt: Option<String>,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        // Task セルにも出す（編隊グリッド・上限 8 は既存規則）。
        let space_id = self.project_sessions.projects[index].task_space.id.clone();
        if self.chrome.fleet_cells.len() < 8
            && !self
                .chrome
                .fleet_cells
                .iter()
                .any(|pane| matches!(pane, FleetPane::Task { space } if *space == space_id))
        {
            self.chrome
                .fleet_cells
                .push(FleetPane::Task { space: space_id });
        }
        let panel = self.project_sessions.sessions[index].agent_panel.clone();
        let thread_index = panel.update(cx, |panel, cx| {
            let thread_index = panel.acquire_thread(agent, cx);
            if let Some(prompt) = prompt {
                panel.send_prompt_text(prompt, cx); // TurnStarted → 台帳 working は既存経路で遷移
            }
            thread_index
        });
        cx.notify();
        serde_json::json!({ "session_index": index, "thread_index": thread_index })
    }

    /// 事実層 + Tier1（+キャッシュ済み Tier2）。**フル transcript は返さない**（3 段圧縮・計画 §P5）。
    fn ipc_digest(&self, index: usize, cx: &mut Context<Self>) -> serde_json::Value {
        let slot = &self.project_sessions.projects[index];
        let statuses = self.project_sessions.sessions[index]
            .agent_panel
            .read(cx)
            .statuses();
        let threads: Vec<serde_json::Value> = statuses
            .iter()
            .map(|status| {
                serde_json::json!({
                    "name": status.name.as_ref(),
                    "activity": match status.activity {
                        agent_panel::ThreadActivity::Idle => "idle",
                        agent_panel::ThreadActivity::Working => "working",
                        agent_panel::ThreadActivity::Blocked => "blocked",
                        agent_panel::ThreadActivity::Done { interrupted: false } => "done",
                        agent_panel::ThreadActivity::Done { interrupted: true } => "interrupted",
                    },
                    "agent": status.agent.as_ref(),
                    "digest": status.digest.as_ref().map(SharedString::as_ref),
                    "tier2": status.tier2.as_ref().map(SharedString::as_ref),
                    "plan_done": status.plan_done,
                    "plan_total": status.plan_total,
                    "files_touched": status.files_touched,
                    "tokens_used": status.tokens_used,
                    "turn_elapsed_secs": status.turn_elapsed_secs,
                })
            })
            .collect();
        serde_json::json!({
            "task_id": slot.task_space.id.as_str(),
            "title": slot.task_space.title.as_ref(),
            "kind": slot.task_space.kind.as_str(),
            "phase": slot.task_space.phase.as_str(),
            "branch": slot.branch.clone().or_else(|| slot.worktree_branch.clone()),
            "result_summary": slot.task_space.result_summary.as_ref().map(SharedString::as_ref),
            "threads": threads,
        })
    }
}

/// 1 接続 = 1 リクエスト（I/O なし・解析してジョブ化するだけ）。
fn serve_connection(
    stream: UnixStream,
    job_tx: futures::channel::mpsc::UnboundedSender<ControlJob>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(10)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let respond_line = |stream: &UnixStream, value: serde_json::Value| {
        let mut stream = stream;
        let _ = writeln!(stream, "{value}");
    };
    let request: serde_json::Value = match serde_json::from_str(&line) {
        Ok(value) => value,
        Err(error) => {
            respond_line(
                &stream,
                err(i18n::t!("ipc.err_bad_json", "detail" => error)),
            );
            return;
        }
    };
    let method = request
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = request
        .get("params")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let (respond_tx, respond_rx) = std::sync::mpsc::channel();
    if job_tx
        .unbounded_send(ControlJob {
            method,
            params,
            respond: respond_tx,
        })
        .is_err()
    {
        respond_line(&stream, err(i18n::t!("ipc.err_gui_gone")));
        return;
    }
    // spawn は worktree オープンを含む＝少し待つ（UI スレッドの 1 job・通常は瞬時）。
    match respond_rx.recv_timeout(std::time::Duration::from_secs(30)) {
        Ok(response) => respond_line(&stream, response),
        Err(_) => respond_line(&stream, err(i18n::t!("ipc.err_gui_timeout"))),
    }
}
