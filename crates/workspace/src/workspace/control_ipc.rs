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
use std::time::Duration;

/// bind に失敗した時の再試行間隔（生きた owner が消えるのを待つ・権限などの一時障害も同じ）。
const BIND_RETRY: Duration = Duration::from_secs(15);

/// askpass（パスワード入力）を人が打ち終わるまで待つ上限。ssh 側はこの間ずっと
/// askpass プロセスの終了を待っている＝長すぎると接続が固まったように見えるので 3 分で切る。
const ASKPASS_TIMEOUT: Duration = Duration::from_secs(180);

/// 「この socket に今いるのは自分か」を確かめる間隔。socket ファイルが外から消された・
/// 別プロセスに奪われた状態を**自分で**見つけて張り直すための心拍（2026-09-11）。
const HEALTH_INTERVAL: Duration = Duration::from_secs(30);

/// GUI 制御 IPC の口（`NECODER_GUI_SOCK` で差し替え可・テスト用）。決定は `paths` crate。
///
/// - macOS/Linux: Unix domain socket のパス。macOS の `SUN_LEN`（~104B）に収まる短さであること
/// - Windows: **名前付きパイプ名**（`\\.\pipe\necoder-gui-<user>`）。ファイルパスではない
pub fn control_socket_path() -> Option<PathBuf> {
    paths::runtime_socket()
}

/// `terminal_read` で返す scrollback の上限（端末の履歴は 10,000 行）。
const TERMINAL_READ_SCROLLBACK_MAX: u64 = 10_000;

/// `terminal_wait` の 1 要求で待つ上限。応答の待ち上限（`serve_connection` の 30 秒）より短く切り、
/// 長く待つ時は CLI が要求を繰り返す。
const TERMINAL_WAIT_MAX: Duration = Duration::from_secs(20);

/// `terminal_wait` が出力の止まり具合を見直す間隔の上限。
const TERMINAL_WAIT_TICK: Duration = Duration::from_millis(250);

/// CLI から見える端末 1 つ（どのプロジェクトの、どこに置いた端末か）。
struct ControlTerminal {
    session_index: usize,
    terminal: Entity<TerminalView>,
    /// `"dock"` = 下ドックのタブ / `"task"` = Fleet の Task カードに置いた端末。
    place: &'static str,
    /// `active` で指せる端末（選択中のプロジェクトの下ドックのアクティブなタブ）。
    active: bool,
}

/// CLI の端末ハンドル（`t<番号>`）。GUI のエンティティ番号なので、GUI が生きている間だけ有効
/// （再起動や端末を閉じた後は `ne terminal list` で取り直す）。
fn terminal_handle(terminal: &Entity<TerminalView>) -> String {
    format!("t{}", terminal.entity_id().as_u64())
}

/// `open` の `positions`（`ne <path>:<line>[:<column>]`・1 始まり）→ 0 始まりの (パス, 行, 列)。
/// ファイルが無いものは落とす（開くパスと同じく、無いものは開かない）。
fn positions_from_params(params: &serde_json::Value) -> Vec<(PathBuf, usize, usize)> {
    params
        .get("positions")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| {
                    let path = PathBuf::from(value.get("path")?.as_str()?);
                    let line = value.get("line")?.as_u64()?;
                    let column = value
                        .get("column")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(1);
                    path.is_file().then(|| {
                        (
                            path,
                            line.saturating_sub(1) as usize,
                            column.saturating_sub(1) as usize,
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// accept スレッド → UI スレッドへ渡す 1 仕事（I/O 前・生パラメータのまま）。
pub(crate) struct ControlJob {
    method: String,
    params: serde_json::Value,
    /// 応答（1 行 JSON）。accept スレッドが recv_timeout で待っている。
    respond: std::sync::mpsc::Sender<serde_json::Value>,
}

pub(crate) fn ok(value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "ok": true, "result": value })
}

pub(crate) fn err(message: impl std::fmt::Display) -> serde_json::Value {
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

/// 今この socket で待ち受けているプロセスの pid を聞く（`control_ping`）。
/// 誰も居ない・壊れている・喋らないなら `None`。
///
/// `control_ping` は accept スレッドが即答する（UI スレッドを経由しない）。UI が重い時に
/// 「socket は死んでいる」と誤判定して張り直さないための分離であり、**呼び出し側が UI の
/// 消費ループを止めて待っていても詰まらない**ことの根拠でもある。
fn socket_owner_pid(path: &Path) -> Option<u32> {
    let mut stream = ControlStream::connect(path).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    writeln!(
        stream,
        "{}",
        serde_json::json!({ "method": "control_ping" })
    )
    .ok()?;
    stream.flush().ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let response: serde_json::Value = serde_json::from_str(&line).ok()?;
    response["result"]["pid"].as_u64().map(|pid| pid as u32)
}

impl Workspace {
    /// IPC サーバを起動する（main から窓ごとに 1 回）。socket の owner は**プロセス/窓を
    /// またいで常に 1 つ**で、生きた owner がいる間は AddrInUse を受けて周期リトライに回り、
    /// owner（前任プロセス・閉じられた窓）が消えたら継ぐ。この自己修復が無いと「GUI は
    /// 生きているのに socket が死んでいる」状態が固定化し、`ne` CLI が GUI 不在と誤判定して
    /// 新インスタンス起動へ落ちる（cli.rs のフォールバック要因・2026-08-30 実測）。
    pub fn start_control_ipc(&mut self, cx: &mut Context<Self>) {
        let Some(socket_path) = control_socket_path() else {
            return;
        };
        // unix socket はファイルなので置き場を掘る必要がある。Windows の名前付きパイプは
        // カーネルの名前空間にあり親ディレクトリという概念が無いので掘らない。
        #[cfg(unix)]
        if let Some(parent) = socket_path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        cx.spawn(async move |workspace, cx| {
            use futures::StreamExt as _;
            // 同じ bind 失敗を 15 秒ごとにログへ積まないための直前の文言。
            let mut reported_failure: Option<String> = None;
            loop {
                // 二重 bind の検出・死んだ socket ファイルの掃除・パーミッション（0600 /
                // 既定 DACL）はすべて control_transport が持つ（WINDOWS-PORT.md §D2）。
                let bind_path = socket_path.clone();
                let bound = cx
                    .background_executor()
                    .spawn(async move { ControlListener::bind(&bind_path) })
                    .await;
                let mut listener = match bound {
                    Ok(listener) => {
                        reported_failure = None;
                        listener
                    }
                    // AddrInUse = 生きた owner（別窓・別プロセス）が待ち受け中。消えたら継ぐ。
                    // それ以外（掃除の競合・権限など）でも**諦めない** — ここで return すると
                    // 「GUI は生きているのに socket が死んでいる」状態が窓の寿命ぶん固定化し、
                    // `ne` も Remote（QR 発行）も GUI を見つけられなくなる（2026-09-11 実測）。
                    Err(error) => {
                        if error.kind() != std::io::ErrorKind::AddrInUse {
                            let text = error.to_string();
                            if reported_failure.as_deref() != Some(text.as_str()) {
                                eprintln!(
                                    "管制 IPC を開けない（{} 秒ごとに再試行する）: {error}",
                                    BIND_RETRY.as_secs()
                                );
                                reported_failure = Some(text);
                            }
                        }
                        cx.background_executor().timer(BIND_RETRY).await;
                        if workspace.update(cx, |_, _| {}).is_err() {
                            return; // 窓ごと閉じた
                        }
                        continue;
                    }
                };
                // stop = 「この窓は消費者をやめた」印。accept はブロッキングなので、立ててから
                // 自分へ 1 本繋いで起こし、listener ごと畳ませる（socket の明け渡し）。
                let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let accept_stop = stop.clone();
                let (job_tx, mut job_rx) = futures::channel::mpsc::unbounded::<ControlJob>();
                // accept ループ（std スレッド）: 解析だけしてジョブ化。I/O はしない。
                std::thread::spawn(move || {
                    // 一時的な失敗（EINTR 等）では諦めない。連続して失敗し続けるときだけ畳む
                    // ＝Windows で次のパイプインスタンスを作れなくなった場合に空回りさせない。
                    let mut consecutive_failures = 0_u32;
                    loop {
                        match listener.accept() {
                            Ok(stream) => {
                                if accept_stop.load(std::sync::atomic::Ordering::Acquire) {
                                    break;
                                }
                                consecutive_failures = 0;
                                let job_tx = job_tx.clone();
                                std::thread::spawn(move || serve_connection(stream, job_tx));
                            }
                            Err(error) => {
                                if accept_stop.load(std::sync::atomic::Ordering::Acquire) {
                                    break;
                                }
                                consecutive_failures += 1;
                                if consecutive_failures >= 16 {
                                    eprintln!(
                                        "管制 IPC の accept が続けて失敗したので畳む: {error}"
                                    );
                                    break;
                                }
                            }
                        }
                    }
                });
                // UI 側の消費ループ。channel が尽きた＝accept スレッド死亡 → bind からやり直す。
                // 併せて HEALTH_INTERVAL ごとに「この socket に今いるのは自分か」を確かめる。
                // 外から socket ファイルを消された/別プロセスに継がれた場合、accept は永久に
                // 呼ばれず（＝この channel は尽きない）気づけないため、心拍側から見つける。
                let mut window_alive = true;
                loop {
                    let health_tick = Box::pin(cx.background_executor().timer(HEALTH_INTERVAL));
                    match futures::future::select(job_rx.next(), health_tick).await {
                        futures::future::Either::Left((Some(job), _)) => {
                            if workspace
                                .update(cx, |workspace, cx| {
                                    workspace.handle_control_job(job, cx);
                                })
                                .is_err()
                            {
                                window_alive = false; // window ごと閉じた
                                break;
                            }
                        }
                        // accept スレッドが畳まれた（job_tx が落ちた）
                        futures::future::Either::Left((None, _)) => break,
                        futures::future::Either::Right(_) => {
                            let probe_path = socket_path.clone();
                            let owner = cx
                                .background_executor()
                                .spawn(async move { socket_owner_pid(&probe_path) })
                                .await;
                            if owner == Some(std::process::id()) {
                                continue; // 健全
                            }
                            // 誰も居ない（掃除された）か、別プロセスが持っている。張り直す
                            // ＝相手が生きていれば次の bind が AddrInUse で待機側に回る。
                            eprintln!(
                                "管制 IPC の socket を見失った（応答 pid: {owner:?}）。張り直す"
                            );
                            break;
                        }
                    }
                }
                stop.store(true, std::sync::atomic::Ordering::Release);
                let _ = ControlStream::connect(&socket_path); // accept を 1 回起こして畳ませる
                if !window_alive {
                    return; // 他の窓の周期リトライが socket を継ぐ
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
            }
        })
        .detach();
    }

    /// 同じ effect cycle に認証要求が重なっても、先に待っている要求を上書きしない。
    #[cfg(unix)]
    fn queue_askpass(
        &mut self,
        prompt: String,
        attempt: u32,
        respond: std::sync::mpsc::Sender<serde_json::Value>,
        cx: &mut Context<Self>,
    ) {
        if self.chrome.pending_askpass.is_some() || self.overlays.askpass.is_some() {
            if respond.send(err(i18n::t!("askpass.err_busy"))).is_err() {
                // 要求元が終了済みなら応答を届ける先はない。
            }
            return;
        }
        self.chrome.pending_askpass = Some((prompt, attempt, respond));
        cx.notify();
    }

    /// UI スレッドでの 1 仕事。メモリで済むものは即応答・DB が要るものは background へ
    /// （GUI のストレージハンドル = 単一ワーカーを使うので headless とロック衝突しない）。
    fn handle_control_job(&mut self, job: ControlJob, cx: &mut Context<Self>) {
        if job.method.starts_with("remote_") {
            self.handle_remote_control(&job.method, job.params, job.respond, cx);
            return;
        }
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
            // SSH の askpass（`host::install_askpass` が起こした necoder 自身からの要求）。
            // **token を持つ子だけ**が秘密を要求できる（token は ssh の環境変数にしか無い）。
            // 入力欄は Window が要るので、pending effect に積んで effect cycle の末尾で開く。
            "askpass" => {
                #[cfg(unix)]
                {
                    let token = params
                        .get("token")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    let prompt = params
                        .get("prompt")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    // token の検証と「何回目の問いか」を同時に取る。2 回目以降 = 直前の入力が
                    // 拒否されて OpenSSH が訊き直している。
                    let Some(attempt) = host::askpass_attempt(token, &prompt) else {
                        let _ = respond.send(err(i18n::t!("askpass.err_token")));
                        return;
                    };
                    self.queue_askpass(prompt, attempt, respond, cx);
                }
                #[cfg(not(unix))]
                {
                    // Windows の GUI ssh は現状 askpass を使わない（WINDOWS-PORT.md §D2 の続き）。
                    let _ = respond.send(err(i18n::t!("askpass.err_token")));
                }
            }
            "open" => {
                // `ne` CLI（cli.rs）からの「このウィンドウで開いて」。絶対パス前提（絶対化は
                // cwd を知る CLI 側の責務）。存在しないものは開かず skipped で返す＝CLI が警告する。
                let (paths, skipped): (Vec<PathBuf>, Vec<PathBuf>) = params
                    .get("paths")
                    .and_then(serde_json::Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(PathBuf::from)
                            .partition(|path| path.exists())
                    })
                    .unwrap_or_default();
                let opened = paths.len();
                self.chrome.pending_external_open.extend(paths);
                // `ne <path>:<line>[:<column>]`（行・列は 1 始まり）。CLI が `path:line` を分けて送る
                // （ファイル名に `:12` を含むものと区別するのは、存在を確かめられる CLI 側の責務）。
                self.chrome
                    .pending_external_goto
                    .extend(positions_from_params(&params));
                // パス 0 件（`ne` 単体）でも前面化はする＝「実行中の necoder を呼び出す」導線。
                cx.activate(true);
                cx.notify();
                let _ = respond.send(ok(serde_json::json!({
                    "opened": opened,
                    "skipped": skipped,
                })));
            }
            "open_remote" => {
                // Remote SSH terminal の open 専用 gateway だけがこの method を作る。gateway が
                // 接続時の authority をローカル側で刻むため、remote 側から別 host に化けられない。
                let Some(authority) = params
                    .get("authority")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|authority| host::SshProject::parse(authority).ok())
                else {
                    let _ = respond.send(err("remote authority が不正です"));
                    return;
                };
                let (uris, skipped): (Vec<String>, Vec<String>) = params
                    .get("paths")
                    .and_then(serde_json::Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_string)
                            .partition(|path| path.starts_with('/'))
                    })
                    .unwrap_or_default();
                let uris = uris
                    .into_iter()
                    .map(|path| authority.uri_for_path(Path::new(&path)))
                    .collect::<Vec<_>>();
                let opened = uris.len();
                self.chrome.pending_remote_open.extend(uris);
                // 引数なしの `ne` も接続元を前面化する。
                cx.activate(true);
                cx.notify();
                let _ = respond.send(ok(serde_json::json!({
                    "opened": opened,
                    "skipped": skipped,
                })));
            }
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
                let panel = self.project_sessions.sessions[index].fleet_agents[0].clone();
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
            // `necoder fleet … active`: GUI で選択中の Task（Fleet の「選んでいる Task」と同じ = アクティブな枠）。
            "active_task" => {
                let response = match self.active_slot() {
                    Some(slot) if slot.task_space.is_integration() => err(i18n::t!(
                        "ipc.err_active_is_integration",
                        "title" => slot.task_space.title.as_ref()
                    )),
                    Some(slot) => ok(serde_json::json!({
                        "task_id": slot.task_space.id.as_str(),
                        "title": slot.task_space.title.as_ref(),
                        "branch": slot.branch.clone().or_else(|| slot.worktree_branch.clone()),
                    })),
                    None => err(i18n::t!("ipc.err_no_active_task")),
                };
                let _ = respond.send(response);
            }
            // ── 端末（`necoder terminal …`・下ドックのタブと Task カードに置いた端末）──
            "terminals" => {
                let only_task = params.get("task_id").and_then(serde_json::Value::as_str);
                let terminals: Vec<serde_json::Value> = self
                    .control_terminals(cx)
                    .into_iter()
                    .filter(|entry| {
                        only_task.is_none_or(|task| {
                            self.project_sessions.projects[entry.session_index]
                                .task_space
                                .id
                                .as_str()
                                == task
                        })
                    })
                    .map(|entry| self.terminal_entry_json(&entry, cx))
                    .collect();
                let _ = respond.send(ok(serde_json::Value::Array(terminals)));
            }
            "terminal_read" => {
                let response = match self.find_terminal(&params, cx) {
                    Ok(terminal) => {
                        let scrollback = params
                            .get("scrollback")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0)
                            .min(TERMINAL_READ_SCROLLBACK_MAX)
                            as usize;
                        let screen = terminal.read(cx).read_screen(scrollback);
                        ok(serde_json::json!({
                            "handle": terminal_handle(&terminal),
                            "scrollback": screen.scrollback,
                            "lines": screen.visible,
                            "cursor": { "line": screen.cursor_line, "column": screen.cursor_column },
                            "exited": screen.exited,
                        }))
                    }
                    Err(message) => err(message),
                };
                let _ = respond.send(response);
            }
            "terminal_send" => {
                // 送った文字はその端末でそのまま実行される＝人が設定で許可した時だけ（既定 off）。
                if !settings::get(cx).allow_terminal_send {
                    let _ = respond.send(err(i18n::t!("ipc.err_terminal_send_disabled")));
                    return;
                }
                let terminal = match self.find_terminal(&params, cx) {
                    Ok(terminal) => terminal,
                    Err(message) => {
                        let _ = respond.send(err(message));
                        return;
                    }
                };
                let enter = params
                    .get("enter")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let text = params
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if text.is_empty() && !enter {
                    let _ = respond.send(err(i18n::t!("ipc.err_text_required")));
                    return;
                }
                let input = if enter {
                    format!("{text}\r")
                } else {
                    text.to_string()
                };
                terminal.read(cx).send_input(&input);
                let _ = respond.send(ok(serde_json::json!({
                    "handle": terminal_handle(&terminal),
                    "bytes": input.len(),
                })));
            }
            "terminal_wait" => {
                let terminal = match self.find_terminal(&params, cx) {
                    Ok(terminal) => terminal,
                    Err(message) => {
                        let _ = respond.send(err(message));
                        return;
                    }
                };
                let idle_target = Duration::from_millis(
                    params
                        .get("idle_ms")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(2_000),
                );
                // 1 回の要求は応答の待ち上限（serve_connection の 30 秒）より短く切る。
                // 長く待つ時は CLI が繰り返す（`idle: false` が返る）。
                let wait_limit = Duration::from_millis(
                    params
                        .get("timeout_ms")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(TERMINAL_WAIT_MAX.as_millis() as u64),
                )
                .min(TERMINAL_WAIT_MAX);
                let started = std::time::Instant::now();
                let handle = terminal_handle(&terminal);
                cx.spawn(async move |_workspace, cx| loop {
                    let (idle, exited) = terminal.read_with(cx, |terminal, _| {
                        (terminal.output_idle(), terminal.has_exited())
                    });
                    let reached = idle >= idle_target;
                    if reached || exited || started.elapsed() >= wait_limit {
                        let _ = respond.send(ok(serde_json::json!({
                            "handle": handle,
                            "idle": reached,
                            "idle_ms": idle.as_millis() as u64,
                            "exited": exited,
                        })));
                        return;
                    }
                    let remaining = idle_target.saturating_sub(idle);
                    cx.background_executor()
                        .timer(remaining.clamp(Duration::from_millis(50), TERMINAL_WAIT_TICK))
                        .await;
                })
                .detach();
            }
            // `ne --diff <a> <b>`: 2 つのファイルの diff を、既存の diff タブ（読み取り専用の一時タブ）で開く。
            "open_diff" => self.open_diff_from_control(&params, respond, cx),
            other => {
                let _ = respond.send(err(i18n::t!("ipc.err_unknown_method", "name" => other)));
            }
        }
    }

    /// 全プロジェクトの端末（下ドックのタブ + Task カードに置いた端末）。PTY は起動しない。
    fn control_terminals(&self, cx: &App) -> Vec<ControlTerminal> {
        let mut terminals = Vec::new();
        for (session_index, session) in self.project_sessions.sessions.iter().enumerate() {
            let dock = session.terminal_dock.read(cx);
            let (tabs, active_tab) = dock.tab_terminals();
            for (tab_index, terminal) in tabs.iter().enumerate() {
                terminals.push(ControlTerminal {
                    session_index,
                    terminal: terminal.clone(),
                    place: "dock",
                    // `active` で指せるのは、選択中のプロジェクトの下ドックのアクティブなタブ。
                    active: session_index == self.project_sessions.active
                        && tab_index == active_tab,
                });
            }
            for (_, terminal) in dock.placed_terminals() {
                terminals.push(ControlTerminal {
                    session_index,
                    terminal,
                    place: "task",
                    active: false,
                });
            }
        }
        terminals
    }

    fn terminal_entry_json(&self, entry: &ControlTerminal, cx: &App) -> serde_json::Value {
        let slot = &self.project_sessions.projects[entry.session_index];
        let terminal = entry.terminal.read(cx);
        serde_json::json!({
            "handle": terminal_handle(&entry.terminal),
            "task_id": slot.task_space.id.as_str(),
            "title": slot.task_space.title.as_ref(),
            "branch": slot.branch.clone().or_else(|| slot.worktree_branch.clone()),
            "place": entry.place,
            "active": entry.active,
            "exited": terminal.has_exited(),
            "idle_ms": terminal.output_idle().as_millis() as u64,
        })
    }

    /// 要求の `terminal`（`list` のハンドルか `active`）を端末に解決する。
    fn find_terminal(
        &self,
        params: &serde_json::Value,
        cx: &App,
    ) -> Result<Entity<TerminalView>, String> {
        let Some(handle) = params
            .get("terminal")
            .and_then(serde_json::Value::as_str)
            .filter(|handle| !handle.is_empty())
        else {
            return Err(i18n::t!("ipc.err_terminal_required"));
        };
        let terminals = self.control_terminals(cx);
        let found = if handle == "active" {
            terminals.into_iter().find(|entry| entry.active)
        } else {
            terminals
                .into_iter()
                .find(|entry| terminal_handle(&entry.terminal) == handle)
        };
        match found {
            Some(entry) => Ok(entry.terminal),
            None if handle == "active" => Err(i18n::t!("ipc.err_no_active_terminal")),
            None => Err(i18n::t!("ipc.err_terminal_not_found", "handle" => handle)),
        }
    }

    /// 2 ファイルの diff を背景で作り、選択中のプロジェクトの diff タブとして開く（次の effect cycle）。
    fn open_diff_from_control(
        &mut self,
        params: &serde_json::Value,
        respond: std::sync::mpsc::Sender<serde_json::Value>,
        cx: &mut Context<Self>,
    ) {
        let path = |key: &str| {
            params
                .get(key)
                .and_then(serde_json::Value::as_str)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        };
        let (Some(left), Some(right)) = (path("left"), path("right")) else {
            let _ = respond.send(err(i18n::t!("ipc.err_diff_paths_required")));
            return;
        };
        if self.project_sessions.sessions.is_empty() {
            let _ = respond.send(err(i18n::t!("ipc.err_no_project")));
            return;
        }
        cx.spawn(async move |workspace, cx| {
            let read = |path: &Path| -> Result<String, String> {
                if !path.is_file() {
                    return Err(i18n::t!("ipc.err_not_a_file", "path" => path.display()));
                }
                std::fs::read_to_string(path).map_err(|error| {
                    i18n::t!("ipc.err_read_file", "path" => path.display(), "error" => error)
                })
            };
            let (left_for_read, right_for_read) = (left.clone(), right.clone());
            let texts = cx
                .background_executor()
                .spawn(
                    async move { Ok::<_, String>((read(&left_for_read)?, read(&right_for_read)?)) },
                )
                .await;
            let (left_text, right_text) = match texts {
                Ok(texts) => texts,
                Err(message) => {
                    let _ = respond.send(err(message));
                    return;
                }
            };
            let Some(diff_text) = project::unified_diff_labeled(
                &left_text,
                &right_text,
                &left.display().to_string(),
                &right.display().to_string(),
            ) else {
                let _ = respond.send(ok(
                    serde_json::json!({ "opened": false, "identical": true }),
                ));
                return;
            };
            let response = workspace
                .update(cx, |workspace, cx| {
                    let file_name = |path: &Path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    };
                    let title = left.with_file_name(format!(
                        "{} ⇄ {}",
                        file_name(&left),
                        file_name(&right)
                    ));
                    let mut buffer = Buffer::from_str(&diff_text);
                    buffer.set_read_only(true);
                    let active = workspace.project_sessions.active;
                    match workspace.project_sessions.sessions.get_mut(active) {
                        Some(session) => {
                            session.pending_transient_tab = Some((title.clone(), buffer));
                            cx.activate(true);
                            cx.notify();
                            ok(serde_json::json!({ "opened": true, "title": title }))
                        }
                        None => err(i18n::t!("ipc.err_no_project")),
                    }
                })
                .unwrap_or_else(|_| err(i18n::t!("ipc.err_gui_gone")));
            let _ = respond.send(response);
        })
        .detach();
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
    pub(super) fn ipc_spawn_into(
        &mut self,
        index: usize,
        agent: Option<String>,
        prompt: Option<String>,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        // Task セルにも出す。IPC の起動先は初期パネルに固定し、UI の選択に左右されない。
        let space_id = self.project_sessions.projects[index].task_space.id.clone();
        if !self
            .chrome
            .fleet_cells
            .iter()
            .any(|pane| matches!(pane, FleetPane::Task { space } if *space == space_id))
        {
            self.chrome
                .fleet_cells
                .push(FleetPane::Task { space: space_id });
        }
        let panel = self.project_sessions.sessions[index].fleet_agents[0].clone();
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
        let statuses = self.project_sessions.sessions[index].agent_statuses(cx);
        let threads: Vec<serde_json::Value> = statuses
            .iter()
            .map(|(_, _, status)| {
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
    mut stream: ControlStream,
    job_tx: futures::channel::mpsc::UnboundedSender<ControlJob>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(ASKPASS_TIMEOUT));
    // 要求を読んでから同じストリームへ応答を書く（1 接続 1 往復）。以前は `try_clone` で
    // 読み口を複製していたが、名前付きパイプの複製は `DuplicateHandle` が要るうえ、
    // 読みと書きが同時に走らないこの形では不要（BufReader の中身を借りれば足りる）。
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let respond_line = |stream: &mut ControlStream, value: serde_json::Value| {
        let _ = writeln!(stream, "{value}");
        let _ = stream.flush();
    };
    let request: serde_json::Value = match serde_json::from_str(&line) {
        Ok(value) => value,
        Err(error) => {
            respond_line(
                reader.get_mut(),
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
    let is_askpass = method == "askpass";
    let params = request
        .get("params")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    // 生存確認だけは**ここで**返す。UI スレッドに渡さないので、UI が詰まっていても
    // 「この socket の待ち受けは誰か」は必ず分かる（socket_owner_pid の相手方）。
    if method == "control_ping" {
        respond_line(
            reader.get_mut(),
            ok(serde_json::json!({ "pid": std::process::id() })),
        );
        return;
    }
    let (respond_tx, respond_rx) = std::sync::mpsc::channel();
    if job_tx
        .unbounded_send(ControlJob {
            method,
            params,
            respond: respond_tx,
        })
        .is_err()
    {
        respond_line(reader.get_mut(), err(i18n::t!("ipc.err_gui_gone")));
        return;
    }
    // spawn は worktree オープンを含む＝少し待つ（UI スレッドの 1 job・通常は瞬時）。
    // askpass だけは**人の入力**を待つので桁が違う（ssh 側も askpass の終了を待っている）。
    let wait = if is_askpass {
        ASKPASS_TIMEOUT
    } else {
        std::time::Duration::from_secs(30)
    };
    match respond_rx.recv_timeout(wait) {
        Ok(response) => respond_line(reader.get_mut(), response),
        Err(_) => respond_line(reader.get_mut(), err(i18n::t!("ipc.err_gui_timeout"))),
    }
}

// ---------------------------------------------------------------------------
// テスト — 心拍（socket の持ち主確認）の土台だけを見る。窓を要する経路は対象外。
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[gpui::test]
    fn simultaneous_askpass_requests_preserve_the_first(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let workspace = cx.new(|cx| super::Workspace::new(Vec::new(), theme_core::Theme::dark(), None, cx));
        let (first, first_reply) = std::sync::mpsc::channel();
        let (second, second_reply) = std::sync::mpsc::channel();
        workspace.update(cx, |workspace, cx| {
            workspace.queue_askpass("first".into(), 1, first, cx);
            workspace.queue_askpass("second".into(), 1, second, cx);
            let (prompt, _, respond) = workspace.chrome.pending_askpass.take().unwrap();
            assert_eq!(prompt, "first");
            respond.send(super::ok(serde_json::json!({"secret": "test-only"}))).unwrap();
        });
        assert_eq!(second_reply.try_recv().unwrap()["ok"], false);
        assert_eq!(first_reply.try_recv().unwrap()["result"]["secret"], "test-only");
    }

    use super::*;

    /// テスト用の待ち受け先。名前は短くする（macOS の `SUN_LEN` ~104B・control_transport 参照）。
    fn test_endpoint(label: &str) -> PathBuf {
        let unique = format!("nec-c{}-{label}", std::process::id());
        if cfg!(windows) {
            PathBuf::from(format!(r"\\.\pipe\{unique}"))
        } else {
            std::env::temp_dir().join(format!("{unique}.sock"))
        }
    }

    /// `control_ping` は accept 側が即答する＝UI の消費ループが居なくても pid が返る。
    /// 「socket の持ち主を外から特定できる」ことが、心拍で張り直せることの前提。
    #[test]
    fn ping_answers_with_our_pid_without_the_ui_loop() {
        let endpoint = test_endpoint("pg");
        let mut listener = ControlListener::bind(&endpoint).expect("bind できない");
        // job の受け手（UI ループ）は**作らない**。ping がそこへ回されていたら返事は来ない。
        let (job_tx, job_rx) = futures::channel::mpsc::unbounded::<ControlJob>();
        drop(job_rx);
        let server = std::thread::spawn(move || {
            let stream = listener.accept().expect("accept できない");
            serve_connection(stream, job_tx);
        });

        assert_eq!(socket_owner_pid(&endpoint), Some(std::process::id()));

        server.join().expect("サーバスレッドが落ちた");
        if !cfg!(windows) {
            let _ = std::fs::remove_file(&endpoint);
        }
    }

    /// 誰も待っていない socket は `None`＝心拍が「張り直す」と判断できる。
    #[test]
    fn ping_to_a_socket_without_a_listener_is_none() {
        assert_eq!(socket_owner_pid(&test_endpoint("nl")), None);
    }

    // ── CLI の端末・active・diff（`necoder terminal …` / `fleet … active` / `ne --diff`）──

    /// テスト用の一時フォルダ（プロジェクトと設定を置く）。本物の設定には触れない。
    fn scratch(tag: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("necoder_control_{}_{}", tag, std::process::id()));
        if directory.exists() {
            std::fs::remove_dir_all(&directory).expect("前回の残りを消せる");
        }
        std::fs::create_dir_all(directory.join("project")).expect("作れる");
        std::fs::write(directory.join("settings.json"), r#"{"onboarded":true}"#).expect("書ける");
        directory
    }

    /// プロジェクト 1 つの窓を開く。下ドックと Task カードの端末は PTY 無しで作る。
    fn open_workspace<'a>(
        directory: &Path,
        cx: &'a mut gpui::TestAppContext,
    ) -> (Entity<Workspace>, &'a mut gpui::VisualTestContext) {
        let settings_path = directory.join("settings.json");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let project = directory.join("project");
        cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project], theme_core::Theme::dark(), None, cx)
        })
    }

    /// 1 要求を UI スレッドの入口（`handle_control_job`）へ渡し、応答を受け取る（socket は通さない）。
    fn request(
        workspace: &Entity<Workspace>,
        cx: &mut gpui::VisualTestContext,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let (respond, reply) = std::sync::mpsc::channel();
        workspace.update(cx, |workspace, cx| {
            workspace.handle_control_job(
                ControlJob {
                    method: method.to_string(),
                    params,
                    respond,
                },
                cx,
            )
        });
        cx.run_until_parked();
        reply.try_recv().expect("応答がある")
    }

    /// 下ドックのタブ 1 つ + Task カードに置いた端末 1 つを作り、(タブ, カード) を返す。
    fn add_terminals(
        workspace: &Entity<Workspace>,
        cx: &mut gpui::VisualTestContext,
    ) -> (Entity<TerminalView>, Entity<TerminalView>) {
        workspace.update(cx, |workspace, cx| {
            let dock = workspace.project_sessions.sessions[0].terminal_dock.clone();
            dock.update(cx, |dock, cx| {
                dock.use_test_terminals();
                (dock.ensure_active_test(cx), dock.start_session(7, cx))
            })
        })
    }

    #[gpui::test]
    fn terminals_lists_dock_tabs_and_task_card_terminals(cx: &mut gpui::TestAppContext) {
        let directory = scratch("list");
        let (workspace, cx) = open_workspace(&directory, cx);
        let (tab, card) = add_terminals(&workspace, cx);
        let task_id = workspace.read_with(cx, |workspace, _| {
            workspace.project_sessions.projects[0]
                .task_space
                .id
                .0
                .clone()
        });

        let response = request(&workspace, cx, "terminals", serde_json::json!({}));
        assert_eq!(response["ok"], true, "{response}");
        let terminals = response["result"].as_array().expect("配列");
        assert_eq!(terminals.len(), 2);
        assert_eq!(terminals[0]["handle"], terminal_handle(&tab));
        assert_eq!(terminals[0]["place"], "dock");
        assert_eq!(terminals[0]["active"], true);
        assert_eq!(terminals[0]["task_id"], task_id.as_str());
        assert_eq!(terminals[1]["handle"], terminal_handle(&card));
        assert_eq!(terminals[1]["place"], "task");
        assert_eq!(terminals[1]["active"], false);
        assert!(terminals[0]["idle_ms"].is_u64());

        // Task で絞る（`ne terminal list <task>`）。
        let filtered = request(
            &workspace,
            cx,
            "terminals",
            serde_json::json!({ "task_id": task_id }),
        );
        assert_eq!(filtered["result"].as_array().map(Vec::len), Some(2));
        let other = request(
            &workspace,
            cx,
            "terminals",
            serde_json::json!({ "task_id": "other" }),
        );
        assert_eq!(other["result"].as_array().map(Vec::len), Some(0));
        std::fs::remove_dir_all(&directory).expect("片付けられる");
    }

    #[gpui::test]
    fn terminal_read_send_and_wait_round_trip(cx: &mut gpui::TestAppContext) {
        let directory = scratch("rsw");
        let (workspace, cx) = open_workspace(&directory, cx);
        let (tab, card) = add_terminals(&workspace, cx);
        card.update(cx, |terminal, _| {
            terminal.feed_output_for_test(b"first\r\nsecond\r\n$ ")
        });
        let handle = terminal_handle(&card);

        let read = request(
            &workspace,
            cx,
            "terminal_read",
            serde_json::json!({ "terminal": handle, "scrollback": 5 }),
        );
        assert_eq!(read["ok"], true, "{read}");
        let lines: Vec<&str> = read["result"]["lines"]
            .as_array()
            .expect("配列")
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        assert_eq!(&lines[..3], ["first", "second", "$"]);
        assert_eq!(read["result"]["cursor"]["line"], 2);
        // `active` は選択中のプロジェクトの下ドックのタブ。
        let active = request(
            &workspace,
            cx,
            "terminal_read",
            serde_json::json!({ "terminal": "active" }),
        );
        assert_eq!(active["result"]["handle"], terminal_handle(&tab));
        let missing = request(
            &workspace,
            cx,
            "terminal_read",
            serde_json::json!({ "terminal": "t0" }),
        );
        assert_eq!(missing["ok"], false);

        // 送信は既定 off（設定で許可するまで何も書かない）。
        let refused = request(
            &workspace,
            cx,
            "terminal_send",
            serde_json::json!({ "terminal": handle, "text": "ls", "enter": true }),
        );
        assert_eq!(refused["ok"], false);
        assert!(card.read_with(cx, |terminal, _| terminal
            .written_input_for_test()
            .is_empty()));
        cx.update(|_, cx| {
            settings::set_user_value(cx, "allow_terminal_send", serde_json::json!(true))
        });
        let sent = request(
            &workspace,
            cx,
            "terminal_send",
            serde_json::json!({ "terminal": handle, "text": "ls", "enter": true }),
        );
        assert_eq!(sent["ok"], true, "{sent}");
        assert_eq!(sent["result"]["bytes"], 3);
        assert_eq!(
            card.read_with(cx, |terminal, _| terminal.written_input_for_test()),
            b"ls\r".to_vec()
        );

        // 出力が止まっていれば即答、止まっていなければ期限で idle=false。
        let idle = request(
            &workspace,
            cx,
            "terminal_wait",
            serde_json::json!({ "terminal": handle, "idle_ms": 0 }),
        );
        assert_eq!(idle["result"]["idle"], true, "{idle}");
        let busy = request(
            &workspace,
            cx,
            "terminal_wait",
            serde_json::json!({ "terminal": handle, "idle_ms": 3_600_000, "timeout_ms": 0 }),
        );
        assert_eq!(busy["result"]["idle"], false, "{busy}");
        std::fs::remove_dir_all(&directory).expect("片付けられる");
    }

    #[gpui::test]
    fn active_task_names_the_selected_task_but_not_the_integration_space(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = scratch("active");
        let (workspace, cx) = open_workspace(&directory, cx);
        // ただのフォルダは統合先扱い＝ active の Task ではない。
        let refused = request(&workspace, cx, "active_task", serde_json::json!({}));
        assert_eq!(refused["ok"], false, "{refused}");
        let task_id = workspace.update(cx, |workspace, _| {
            let slot = &mut workspace.project_sessions.projects[0];
            slot.task_space.kind = SpaceKind::Task;
            slot.task_space.id.0.clone()
        });
        let active = request(&workspace, cx, "active_task", serde_json::json!({}));
        assert_eq!(active["ok"], true, "{active}");
        assert_eq!(active["result"]["task_id"], task_id.as_str());
        std::fs::remove_dir_all(&directory).expect("片付けられる");
    }

    #[gpui::test]
    fn open_diff_opens_two_files_in_a_diff_tab(cx: &mut gpui::TestAppContext) {
        let directory = scratch("diff");
        let left = directory.join("left.txt");
        let right = directory.join("right.txt");
        std::fs::write(&left, "same\nold\n").expect("書ける");
        std::fs::write(&right, "same\nnew\n").expect("書ける");
        let (workspace, cx) = open_workspace(&directory, cx);

        let opened = request(
            &workspace,
            cx,
            "open_diff",
            serde_json::json!({ "left": left, "right": right }),
        );
        assert_eq!(opened["ok"], true, "{opened}");
        assert_eq!(opened["result"]["opened"], true);
        let title = directory.join("left.txt ⇄ right.txt");
        assert_eq!(opened["result"]["title"], title.display().to_string());
        // 次の effect cycle で diff タブになる（まだなら pending に積まれている）。
        let text = workspace.update(cx, |workspace, cx| {
            let session = &workspace.project_sessions.sessions[workspace.project_sessions.active];
            if let Some((pending_title, buffer)) = &session.pending_transient_tab {
                assert_eq!(pending_title, &title);
                return buffer.text();
            }
            let tab = session
                .tabs
                .iter()
                .find(|tab| tab.path == title)
                .expect("diff タブがある");
            tab.editor()
                .expect("diff タブはエディタ")
                .read(cx)
                .buffer()
                .text()
        });
        assert!(text.contains("-old"), "{text}");
        assert!(text.contains("+new"), "{text}");

        let identical = request(
            &workspace,
            cx,
            "open_diff",
            serde_json::json!({ "left": left, "right": left }),
        );
        assert_eq!(identical["result"]["identical"], true, "{identical}");
        let missing = request(
            &workspace,
            cx,
            "open_diff",
            serde_json::json!({ "left": left, "right": directory.join("nope.txt") }),
        );
        assert_eq!(missing["ok"], false);
        std::fs::remove_dir_all(&directory).expect("片付けられる");
    }

    /// `ne <path>:<line>[:<column>]` の位置は 1 始まりで届き、0 始まりに直して積む。無いファイルは落とす。
    #[test]
    fn open_positions_become_zero_based_jumps() {
        let directory = scratch("goto");
        let file = directory.join("a.rs");
        std::fs::write(&file, "fn a() {}\n").expect("書ける");
        let positions = positions_from_params(&serde_json::json!({
            "positions": [
                { "path": file, "line": 3, "column": 2 },
                { "path": file, "line": 1 },
                { "path": directory.join("missing.rs"), "line": 9 },
            ]
        }));
        assert_eq!(positions, vec![(file.clone(), 2, 1), (file, 0, 0)]);
        assert!(positions_from_params(&serde_json::json!({})).is_empty());
        std::fs::remove_dir_all(&directory).expect("片付けられる");
    }
}
