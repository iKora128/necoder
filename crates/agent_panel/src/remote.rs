//! PWA 向けの限定 API。固定 thread/permission ID を UI スレッドで再照合する。
use super::*;
use serde_json::{json, Value};

const TEXT_LIMIT: usize = 4_000;
const PERMISSION_LIMIT: usize = 180_000;

fn text(value: &str) -> String {
    value[..floor_char_boundary(value, value.len().min(TEXT_LIMIT))].to_string()
}

fn turn_id(thread: &Thread) -> String {
    format!("{}:{:?}", thread.session_serial, thread.turn_started_at)
}

fn permission_complete(pending: &PendingPermission) -> bool {
    let size = pending.raw_input.as_ref().map_or(0, |raw| raw.len())
        + pending
            .diffs
            .iter()
            .map(|diff| {
                diff.path.len()
                    + diff.old_text.as_ref().map_or(0, String::len)
                    + diff.new_text.len()
            })
            .sum::<usize>();
    size <= PERMISSION_LIMIT
        && (pending
            .raw_input
            .as_ref()
            .is_some_and(|v| !v.trim().is_empty())
            || !pending.diffs.is_empty())
}

impl AgentPanel {
    pub fn remote_threads(&self) -> Value {
        Value::Array(self.threads.iter().map(|thread| json!({
            "id": thread.id, "name": thread.name.as_ref(), "agent": thread.agent.as_ref(),
            "turn_id": turn_id(thread), "running": thread.running,
            "blocked": thread.pending_permission.is_some() || thread.pending_elicitation.is_some(),
            "session_lost": thread.session_lost, "tokens_used": thread.tokens_used,
            "digest": thread.digest.as_ref().map(|v| text(v)),
            "permission_id": thread.pending_permission.as_ref().map(|p| &p.remote_id),
            "question_pending": thread.pending_elicitation.is_some(),
            "question_id": thread.pending_elicitation.as_ref().map(|q| &q.remote_id),
        })).collect())
    }

    pub fn remote_thread(&self, id: &str) -> anyhow::Result<Value> {
        let thread = self
            .threads
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow::anyhow!("thread_not_found"))?;
        let start = thread.entries.len().saturating_sub(60);
        let entries: Vec<Value> = thread
            .entries
            .iter()
            .enumerate()
            .skip(start)
            .map(|(index, entry)| {
                let (kind, content) = match entry {
                    Entry::User(v) => ("user", text(v)),
                    Entry::Agent(v) => ("agent", text(v)),
                    Entry::Thinking(v) => ("thinking", text(v)),
                    Entry::Step {
                        tool, args, result, ..
                    } => (
                        "tool",
                        text(&format!(
                            "{tool}\n{args}\n{}",
                            result.as_ref().map_or("", |v| v.as_ref())
                        )),
                    ),
                    Entry::Checkpoint { label, .. } => ("checkpoint", text(label)),
                };
                json!({ "id": index, "kind": kind, "text": content })
            })
            .collect();
        let permission = thread.pending_permission.as_ref().map(|pending| {
            let complete = permission_complete(pending);
            let options: Vec<Value> = pending.options.iter().enumerate().filter_map(|(index, option)| {
                let kind = match option.kind {
                    PermissionKind::Allow if complete => "allow",
                    PermissionKind::Reject => "reject",
                    _ => return None,
                };
                Some(json!({ "id": format!("{}:{index}", pending.remote_id), "kind": kind, "label": option.label }))
            }).collect();
            let diffs: Vec<Value> = if complete { pending.diffs.iter().map(|d| json!({
                "path": d.path, "old_text": d.old_text, "new_text": d.new_text,
            })).collect() } else { Vec::new() };
            json!({ "id": pending.remote_id, "title": text(&pending.title), "complete": complete,
                "raw_input": if complete { pending.raw_input.as_ref().map(|v| v.as_ref()) } else { None },
                "diffs": diffs, "options": options })
        });
        let question = thread.pending_elicitation.as_ref().map(|q| json!({
            "id": q.remote_id, "message": text(&q.message),
            "fields": q.fields.iter().map(|field| json!({
                "name": field.name, "title": field.label,
                "choices": field.options.iter().map(|o| json!({"value": o.value, "label": o.title})).collect::<Vec<_>>()
            })).collect::<Vec<_>>()
        }));
        Ok(
            json!({ "id": thread.id, "turn_id": turn_id(thread), "entries": entries,
            "history_truncated": start > 0, "permission": permission,
            "question": question, "question_pending": thread.pending_elicitation.is_some() }),
        )
    }

    /// 受信した相関キーと現在の状態を同じ UI 更新内で照合し、既存の操作入口へ渡す。
    pub fn remote_command(
        &mut self,
        method: &str,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Value> {
        if method == "new_thread" {
            let agent = params["agent"].as_str().unwrap_or("Claude Code");
            anyhow::ensure!(AgentKind::by_label(agent).is_some(), "unknown_agent");
            anyhow::ensure!(self.threads.len() < 100, "thread_limit");
            let active = self.active;
            let index = self.new_thread_index(cx);
            self.threads[index].agent = agent.to_string().into();
            let id = self.threads[index].id.clone();
            self.switch_thread(active, cx);
            return Ok(json!({"thread_id": id}));
        }
        let id = params["thread_id"].as_str().unwrap_or("");
        let index = self
            .thread_index_by_id(id)
            .ok_or_else(|| anyhow::anyhow!("thread_not_found"))?;
        let thread = &self.threads[index];
        anyhow::ensure!(
            params["turn_id"].as_str() == Some(turn_id(thread).as_str()),
            "stale_turn"
        );
        match method {
            "send_message" => {
                let message = params["message"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("message_required"))?;
                anyhow::ensure!(
                    !message.trim().is_empty() && message.len() <= 32_000,
                    "invalid_message"
                );
                anyhow::ensure!(
                    thread.pending_permission.is_none() && thread.pending_elicitation.is_none(),
                    "answer_pending_request_first"
                );
                anyhow::ensure!(!thread.running, "wait_until_idle_or_interrupt");
                // スマホ操作でローカルの入力中タブ/下書きを切り替えない。
                let active = self.active;
                self.active = index;
                self.send_prompt_text(message.to_string(), cx);
                self.active = active;
            }
            "interrupt" => {
                self.cancel_turn(index, cx);
            }
            "question_response" => {
                let question = thread
                    .pending_elicitation
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("question_resolved"))?;
                anyhow::ensure!(
                    params["question_id"].as_str() == Some(question.remote_id.as_str()),
                    "stale_question"
                );
                let selections = if params["selections"].is_null() {
                    None
                } else {
                    let selections = params["selections"]
                        .as_object()
                        .ok_or_else(|| anyhow::anyhow!("invalid_answers"))?;
                    anyhow::ensure!(selections.len() == question.fields.len(), "invalid_answers");
                    let mut answers = Vec::new();
                    for field in &question.fields {
                        let value = selections
                            .get(&field.name)
                            .and_then(Value::as_str)
                            .ok_or_else(|| anyhow::anyhow!("answer_required"))?;
                        anyhow::ensure!(
                            field.options.iter().any(|option| option.value == value),
                            "invalid_answer"
                        );
                        answers.push((field.name.clone(), value.to_string()));
                    }
                    Some(answers)
                };
                if let Some(question) = self.threads[index].pending_elicitation.take() {
                    question
                        .respond
                        .unbounded_send(selections)
                        .map_err(|_| anyhow::anyhow!("question_channel_closed"))?;
                }
                self.sync_running_registry(cx);
                cx.notify();
            }
            "permission_response" => {
                let pending = thread
                    .pending_permission
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("permission_resolved"))?;
                anyhow::ensure!(
                    params["permission_id"].as_str() == Some(pending.remote_id.as_str()),
                    "stale_permission"
                );
                let option = pending
                    .options
                    .iter()
                    .enumerate()
                    .find(|(index, _)| {
                        params["option_id"].as_str()
                            == Some(format!("{}:{index}", pending.remote_id).as_str())
                    })
                    .ok_or_else(|| anyhow::anyhow!("invalid_option"))?;
                anyhow::ensure!(
                    option.1.kind == PermissionKind::Reject
                        || (option.1.kind == PermissionKind::Allow && permission_complete(pending)),
                    "remote_option_forbidden"
                );
                let option_index = option.0;
                self.respond_permission(index, option_index, cx);
            }
            _ => anyhow::bail!("method_not_allowed"),
        }
        Ok(json!({ "accepted": true, "thread_id": id }))
    }
}
