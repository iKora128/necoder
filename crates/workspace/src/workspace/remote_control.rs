//! PWA ホストの Unix socket API。公開 transport は補助プロセスが担当する。
use crate::workspace::*;
use serde_json::{json, Value};

fn instance_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| format!("{}-{:?}", std::process::id(), std::time::SystemTime::now()))
}

fn reply(result: anyhow::Result<Value>) -> Value {
    match result {
        Ok(value) => json!({"ok": true, "result": value}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    }
}

impl Workspace {
    pub(crate) fn handle_remote_control(
        &mut self,
        method: &str,
        params: Value,
        respond: std::sync::mpsc::Sender<Value>,
        cx: &mut Context<Self>,
    ) {
        // Remote ホストからの知らせ: スマホ連携に新しい端末が登録された（確認まで済んだ）。
        // 種類はホストが既知の値に絞ってあるが、ここでも形を確かめてから文に入れる。
        if method == "remote_device_added" {
            let model = params["model"]
                .as_str()
                .filter(|model| {
                    ["iPhone", "iPad", "Android", "Mac", "Windows", "Linux"].contains(model)
                })
                .map(str::to_owned)
                .unwrap_or_else(|| i18n::t!("settings.remote_device_phone"));
            self.push_remote_device_toast(
                SharedString::from(i18n::t!("toast.remote_device_added", "model" => model)),
                cx,
            );
            if let Err(error) = respond.send(reply(Ok(json!({"shown": true})))) {
                eprintln!("remote device notice receiver: {error}");
            }
            return;
        }
        if method == "remote_snapshot" {
            let projects: Vec<Value> = self.project_sessions.projects.iter().enumerate().map(|(index, slot)| {
                let session = &self.project_sessions.sessions[index];
                let threads: Vec<Value> = session.fleet_agents.iter()
                    .flat_map(|panel| match panel.read(cx).remote_threads(cx) {
                        Value::Array(threads) => threads,
                        _ => Vec::new(),
                    }).collect();
                // 新しいスレッドはプロジェクトの Agent パネル（Task の外）に作る。そのシートに出す
                // エージェントとモデル等の選択肢（無い版の GUI の間、スマホはエージェント名だけで作る）。
                let new_thread = session.fleet_agents.first()
                    .map_or(Value::Null, |panel| panel.read(cx).remote_new_thread_options(cx));
                json!({"id": slot.task_space.id.as_str(), "name": slot.name.as_ref(),
                    "branch": slot.branch, "remote_host": slot.remote_host.as_ref().map(SharedString::as_ref),
                    "color": format!("#{:06x}", theme_core::hex_from_color(slot.color)),
                    "threads": threads, "new_thread": new_thread})
            }).collect();
            if let Err(error) = respond.send(reply(Ok(
                json!({"instance_id": instance_id(), "projects": projects}),
            ))) {
                eprintln!("remote snapshot receiver: {error}");
            }
            return;
        }
        if method == "remote_get_diff" {
            let target = self.project_sessions.projects.iter().find(|slot| {
                params["instance_id"].as_str() == Some(instance_id())
                    && params["task_id"].as_str() == Some(slot.task_space.id.as_str())
            });
            let Some(slot) = target else {
                if let Err(error) = respond.send(reply(Err(anyhow::anyhow!("task_not_open")))) {
                    eprintln!("remote diff: {error}");
                }
                return;
            };
            let host = slot.worktree.host().clone();
            let root = slot.worktree.root().to_path_buf();
            cx.background_executor().spawn(async move {
                // 引数は固定。外部 diff/textconv を無効化し、設定由来のプログラムを起動しない。
                let result = (|| -> anyhow::Result<Value> {
                    let output = host.run_command(&host::CommandSpec::new("git", root)
                        .args(["--no-pager", "diff", "--no-ext-diff", "--no-textconv", "HEAD", "--"]))?;
                    anyhow::ensure!(output.status_code == Some(0), "git_diff_failed");
                    anyhow::ensure!(output.stdout.len() <= 180_000, "diff_too_large_review_on_mac");
                    Ok(json!({"diff": String::from_utf8_lossy(&output.stdout), "tracked_only": true}))
                })();
                if let Err(error) = respond.send(reply(result)) { eprintln!("remote diff: {error}"); }
            }).detach();
            return;
        }
        let result = (|| -> anyhow::Result<Value> {
            anyhow::ensure!(
                params["instance_id"].as_str() == Some(instance_id()),
                "stale_instance"
            );
            let task = params["task_id"].as_str().unwrap_or("");
            let index = self
                .project_sessions
                .projects
                .iter()
                .position(|slot| slot.task_space.id.as_str() == task)
                .ok_or_else(|| anyhow::anyhow!("task_not_open"))?;
            let thread_id = params["thread_id"].as_str().unwrap_or("");
            let fleet_agents = &self.project_sessions.sessions[index].fleet_agents;
            // 新しいスレッドはプロジェクトの Agent パネルへ（前の版のスマホは既存のスレッドの ID を添えて
            // 送ってくるが、そのスレッドが Task のパネルにあっても Task の中には作らない）。
            let panel = if method == "remote_new_thread" {
                fleet_agents.first()
            } else {
                fleet_agents
                    .iter()
                    .find(|panel| panel.read(cx).contains_thread(thread_id))
            }
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("thread_not_found"))?;
            if method == "remote_thread" {
                return panel.read(cx).remote_thread(thread_id, cx);
            }
            // 承認/送信はネットワーク断を跨いで遅延実行しない。IPC キュー内の待ちも期限に含める。
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as u64;
            let expires = params["expires_at"].as_u64().unwrap_or(0);
            anyhow::ensure!(expires > now && expires <= now + 60_000, "command_expired");
            let command = method.strip_prefix("remote_").unwrap_or("");
            let result = panel.update(cx, |panel, cx| panel.remote_command(command, &params, cx));
            cx.notify();
            result
        })();
        if let Err(error) = respond.send(reply(result)) {
            eprintln!("remote command receiver: {error}");
        }
    }
}
