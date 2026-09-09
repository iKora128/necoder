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
    pub(crate) fn handle_remote_control(&mut self, method: &str, params: Value,
        respond: std::sync::mpsc::Sender<Value>, cx: &mut Context<Self>) {
        if method == "remote_snapshot" {
            let projects: Vec<Value> = self.project_sessions.projects.iter().enumerate().map(|(index, slot)| {
                json!({"id": slot.task_space.id.as_str(), "name": slot.name.as_ref(),
                    "branch": slot.branch, "remote_host": slot.remote_host.as_ref().map(SharedString::as_ref),
                    "threads": self.project_sessions.sessions[index].agent_panel.read(cx).remote_threads()})
            }).collect();
            if let Err(error) = respond.send(reply(Ok(json!({"instance_id": instance_id(), "projects": projects})))) {
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
                if let Err(error) = respond.send(reply(Err(anyhow::anyhow!("task_not_open")))) { eprintln!("remote diff: {error}"); }
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
            anyhow::ensure!(params["instance_id"].as_str() == Some(instance_id()), "stale_instance");
            let task = params["task_id"].as_str().unwrap_or("");
            let index = self.project_sessions.projects.iter().position(|slot| slot.task_space.id.as_str() == task)
                .ok_or_else(|| anyhow::anyhow!("task_not_open"))?;
            let panel = self.project_sessions.sessions[index].agent_panel.clone();
            if method == "remote_thread" {
                return panel.read(cx).remote_thread(params["thread_id"].as_str().unwrap_or(""));
            }
            // 承認/送信はネットワーク断を跨いで遅延実行しない。IPC キュー内の待ちも期限に含める。
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64;
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
