//! PWA 向けの限定 API。固定 thread/permission ID を UI スレッドで再照合する。
use super::*;
use serde_json::{json, Value};

const TEXT_LIMIT: usize = 4_000;
const PERMISSION_LIMIT: usize = 180_000;

fn text(value: &str) -> String {
    value[..floor_char_boundary(value, value.len().min(TEXT_LIMIT))].to_string()
}

pub(crate) fn turn_id(thread: &Thread) -> String {
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

/// 承認を聞かずに進めるモードか（Claude Code の `bypassPermissions`・Codex の `full-access` /
/// `agent-full-access`・`yolo`）。スマホからはこのモードへ上げさせない（Mac で今そのモードの時だけ選べる）。
/// 盗まれた端末から「聞かずに何でも走らせる」スレッドを作らせないための線で、デスクトップの
/// [`is_bypass_mode`] より広く取る（`agent-full-access` も含める）。
fn skips_approval(mode_id: &str) -> bool {
    let lowered = mode_id.to_lowercase();
    is_bypass_mode(&lowered)
        || lowered.contains("full-access")
        || lowered.contains("full_access")
        || lowered.contains("yolo")
}

/// スマホに出す設定の 1 区分（今の値と選択肢）。選択肢はエージェントが広告した物だけ（在庫）で、
/// 広告がまだ無ければ空（候補を作らない）。`label` は今の値の表示名（広告に無ければ value_id のまま）。
fn setting_json(current: &SharedString, choices: &[SelectorChoice]) -> Value {
    json!({
        "current": (!current.is_empty()).then(|| current.as_ref()),
        "label": (!current.is_empty()).then(|| label_for(choices, current).to_string()),
        "choices": choices.iter().map(|choice| json!({
            "id": choice.value.as_ref(), "label": choice.label.as_ref(),
        })).collect::<Vec<_>>(),
    })
}

/// 権限モードの区分。聞かずに進めるモードは、今の値でない限り `allowed: false`（選べない行として出す）。
fn mode_json(current: &SharedString, choices: &[SelectorChoice]) -> Value {
    json!({
        "current": (!current.is_empty()).then(|| current.as_ref()),
        "label": (!current.is_empty()).then(|| label_for(choices, current).to_string()),
        "choices": choices.iter().map(|choice| json!({
            "id": choice.value.as_ref(), "label": choice.label.as_ref(),
            "allowed": !skips_approval(&choice.value) || choice.value == *current,
        })).collect::<Vec<_>>(),
    })
}

/// スマホから来た設定の値を確かめる: 広告にある value_id か（候補を作らない）、権限モードなら
/// 聞かずに進めるモードへ上げていないか。
fn check_setting(
    selector: Selector,
    value: &SharedString,
    current: &SharedString,
    choices: &[SelectorChoice],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        choices.iter().any(|choice| &choice.value == value),
        "setting_unavailable"
    );
    if selector == Selector::Mode {
        anyhow::ensure!(!skips_approval(value) || value == current, "mode_needs_mac");
    }
    Ok(())
}

/// スマホの設定の名前 → necoder の区分。
fn remote_selector(name: &str) -> anyhow::Result<Selector> {
    Ok(match name {
        "agent" => Selector::Agent,
        "model" => Selector::Model,
        "effort" => Selector::Effort,
        "mode" => Selector::Mode,
        _ => anyhow::bail!("invalid_setting"),
    })
}

/// 新しいスレッドを作った時の初期値（[`AgentPanel::add_thread`] とエージェントのピルの切り替えと同じ規則）。
struct FreshSettings {
    model: SharedString,
    effort: SharedString,
    mode: SharedString,
}

impl AgentPanel {
    pub fn remote_threads(&self, cx: &App) -> Value {
        Value::Array(self.threads.iter().map(|thread| json!({
            "id": thread.id, "name": thread.name.as_ref(), "agent": thread.agent.as_ref(),
            // 一覧と会話の上端に出すモデル・思考量（表示名。広告がまだ無ければ value_id・未選択なら null）。
            "model": self.remote_label(thread, Selector::Model, cx),
            "effort": self.remote_label(thread, Selector::Effort, cx),
            "turn_id": turn_id(thread), "running": thread.running,
            "blocked": thread.pending_permission.is_some() || thread.pending_elicitation.is_some(),
            "session_lost": thread.session_lost, "tokens_used": thread.tokens_used,
            "digest": thread.digest.as_ref().map(|v| text(v)),
            "permission_id": thread.pending_permission.as_ref().map(|p| &p.remote_id),
            "question_pending": thread.pending_elicitation.is_some(),
            "question_id": thread.pending_elicitation.as_ref().map(|q| &q.remote_id),
            // スマホでもデスクトップと同じ識別色で見分ける（UI-SPEC §1.2）。
            "color": format!("#{:06x}", theme_core::hex_from_color(thread.color)),
        })).collect())
    }

    fn remote_label(&self, thread: &Thread, selector: Selector, cx: &App) -> Option<String> {
        let current = match selector {
            Selector::Model => &thread.model,
            Selector::Effort => &thread.effort,
            Selector::Mode => &thread.permission_mode,
            Selector::Agent => &thread.agent,
        };
        if current.is_empty() {
            return None;
        }
        Some(label_for(&self.selector_choices_for(thread, selector, cx), current).to_string())
    }

    /// 会話の画面の設定（エージェント・モデル・思考量・権限モード）。エージェントは会話を始める前だけ、
    /// 権限モードは Chat と Captain の席では変えない（デスクトップのピルと同じ決まり）。
    fn remote_settings(&self, thread: &Thread, cx: &App) -> Value {
        let agent_fixed = !thread.entries.is_empty() || thread.running || thread.seat.is_some();
        let mode_fixed = self.chat_mode || thread.seat.is_some();
        let agents = if agent_fixed {
            Vec::new()
        } else {
            self.selector_choices_for(thread, Selector::Agent, cx)
        };
        json!({
            "agent": {
                "current": thread.agent.as_ref(), "fixed": agent_fixed,
                "choices": agents.iter().map(|choice| json!({"id": choice.value.as_ref(), "label": choice.label.as_ref()})).collect::<Vec<_>>(),
            },
            "model": setting_json(&thread.model, &self.selector_choices_for(thread, Selector::Model, cx)),
            "effort": setting_json(&thread.effort, &self.selector_choices_for(thread, Selector::Effort, cx)),
            "mode": if mode_fixed { Value::Null } else {
                mode_json(&thread.permission_mode, &self.selector_choices_for(thread, Selector::Mode, cx))
            },
        })
    }

    /// 新しいスレッドを `agent` で作った時の初期値。既定のエージェントは [`AgentPanel::add_thread`] と同じ
    /// （その agent の記憶 → 空なら直前のタブの値・権限モードは直前のタブから引き継ぐ）、ほかの
    /// エージェントはピルで切り替えた時と同じ（その agent の記憶だけ・[`apply_agent_sticky`]）。
    fn fresh_settings(&self, agent: &SharedString, cx: &App) -> FreshSettings {
        let sticky = |config_id| agent_sticky_value(agent.as_ref(), config_id, cx);
        let previous = self.threads.get(self.active);
        let mut fresh = if *agent != default_agent_name(cx) {
            FreshSettings {
                model: sticky("model"),
                effort: sticky("effort"),
                mode: sticky("mode"),
            }
        } else {
            let inherited = |own: SharedString, from: fn(&Thread) -> &SharedString| match previous {
                Some(previous) if own.is_empty() && previous.agent == *agent => {
                    from(previous).clone()
                }
                _ => own,
            };
            FreshSettings {
                model: inherited(sticky("model"), |thread| &thread.model),
                effort: inherited(sticky("effort"), |thread| &thread.effort),
                mode: previous
                    .map(|previous| previous.permission_mode.clone())
                    .unwrap_or_else(|| sticky("mode")),
            }
        };
        // 記憶の無いスレッドは、権限の既定が「聞かずに進める」なら広告の中のそのモードで始まる（O16・
        // `on_event` の Modes と同じ）。シートにも始まる時のモードを出す。
        if fresh.mode.is_empty() && settings::bypass_permissions_by_default(cx) {
            let modes = self.fresh_choices(agent, Selector::Mode, cx);
            if let Some(bypass) = modes.iter().find(|choice| is_bypass_mode(&choice.value)) {
                fresh.mode = bypass.value.clone();
            }
        }
        fresh
    }

    /// 新しいスレッドの選択肢（エージェントの広告の在庫で引く・まだ無いスレッドなので
    /// [`AgentPanel::prospective_stock_key_for_agent`]）。
    fn fresh_choices(
        &self,
        agent: &SharedString,
        selector: Selector,
        cx: &App,
    ) -> Vec<SelectorChoice> {
        let Some(stock) = self
            .catalog
            .get(&self.prospective_stock_key_for_agent(agent.clone(), cx))
        else {
            return Vec::new();
        };
        match selector {
            Selector::Model => config_choices(&stock.configs, ConfigCategory::Model),
            Selector::Effort => config_choices(&stock.configs, ConfigCategory::ThoughtLevel),
            Selector::Mode => stock
                .modes
                .iter()
                .map(|(id, name)| SelectorChoice {
                    value: id.clone(),
                    label: name.clone(),
                })
                .collect(),
            Selector::Agent => Vec::new(),
        }
    }

    /// スマホの新しいスレッドのシートに出す物（snapshot のプロジェクトごと・`remote_control.rs`）: 使える
    /// エージェントと、それぞれのモデル・思考量・権限モードの初期値と選択肢。Chat には出さない。
    pub fn remote_new_thread_options(&self, cx: &App) -> Value {
        if self.chat_mode {
            return Value::Null;
        }
        let default_agent = default_agent_name(cx);
        let agents: Vec<Value> = agent_choices(SharedString::default(), cx)
            .into_iter()
            .map(|choice| {
                let agent = choice.value;
                let fresh = self.fresh_settings(&agent, cx);
                json!({
                    "name": agent.as_ref(), "default": agent == default_agent,
                    "model": setting_json(&fresh.model, &self.fresh_choices(&agent, Selector::Model, cx)),
                    "effort": setting_json(&fresh.effort, &self.fresh_choices(&agent, Selector::Effort, cx)),
                    "mode": mode_json(&fresh.mode, &self.fresh_choices(&agent, Selector::Mode, cx)),
                })
            })
            .collect();
        json!({ "agents": agents })
    }

    pub fn remote_thread(&self, id: &str, cx: &App) -> anyhow::Result<Value> {
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
                    // necoder の知らせ（人の発話と分ける）。見出しの時刻と出所を本文の頭に載せる。
                    Entry::AutoPrompt(auto) => (
                        "auto_prompt",
                        text(&format!("{}\n{}", auto.label(), auto.text)),
                    ),
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
                    Entry::Notice(v) => ("notice", text(v)),
                    // `!` の行（#37）はツールの行と同じ見せ方（PWA に種類を足さない）。
                    Entry::Shell(run) => ("tool", text(&run.plain_text())),
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
                "name": field.name, "title": field.label, "multi": field.multi,
                // 自分で書いて答えられる質問か（Other 欄つき・O17）。
                "custom": field.custom_answer.is_some(),
                "choices": field.options.iter().map(|o| json!({"value": o.value, "label": o.title})).collect::<Vec<_>>()
            })).collect::<Vec<_>>()
        }));
        Ok(
            json!({ "id": thread.id, "turn_id": turn_id(thread), "entries": entries,
            "history_truncated": start > 0, "permission": permission,
            "question": question, "question_pending": thread.pending_elicitation.is_some(),
            "settings": self.remote_settings(thread, cx) }),
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
            return self.remote_new_thread(params, cx);
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
                    // 自分で書いた答え（O17・Other 欄つきの質問だけ）。無ければ空。
                    let empty = serde_json::Map::new();
                    let custom = match &params["custom"] {
                        Value::Null => &empty,
                        Value::Object(custom) => custom,
                        _ => anyhow::bail!("invalid_answers"),
                    };
                    let known = |name: &String| question.fields.iter().any(|f| &f.name == name);
                    anyhow::ensure!(
                        selections.keys().all(known) && custom.keys().all(known),
                        "invalid_answers"
                    );
                    let mut answers = Vec::new();
                    for field in &question.fields {
                        // 複数選択は配列、単一選択は文字列で来る。片方しか受けないと、
                        // スマホ側の 1 フィールド分の型違いで質問ごと答えられなくなる。
                        // 書いて答える質問は選ばなくてよい（無い・空文字・空の配列）。
                        let values: Vec<String> = match selections.get(&field.name) {
                            None => Vec::new(),
                            Some(Value::String(value)) if value.is_empty() => Vec::new(),
                            Some(Value::String(value)) => vec![value.clone()],
                            Some(Value::Array(values)) => values
                                .iter()
                                .map(|value| {
                                    value
                                        .as_str()
                                        .map(str::to_string)
                                        .ok_or_else(|| anyhow::anyhow!("invalid_answer"))
                                })
                                .collect::<anyhow::Result<Vec<String>>>()?,
                            Some(_) => anyhow::bail!("invalid_answer"),
                        };
                        anyhow::ensure!(
                            field.multi || values.len() <= 1,
                            "invalid_answer" // 単一選択に複数返させない
                        );
                        for value in &values {
                            anyhow::ensure!(
                                field.options.iter().any(|option| &option.value == value),
                                "invalid_answer"
                            );
                        }
                        let written = match custom.get(&field.name) {
                            None => "",
                            Some(Value::String(text)) => text.trim(),
                            Some(_) => anyhow::bail!("invalid_answer"),
                        };
                        anyhow::ensure!(
                            written.is_empty() || field.custom_answer.is_some(),
                            "invalid_answer" // Other 欄の無い質問に書かせない
                        );
                        anyhow::ensure!(written.chars().count() <= 4_000, "invalid_answer");
                        anyhow::ensure!(
                            !values.is_empty() || !written.is_empty(),
                            "answer_required"
                        );
                        if !values.is_empty() {
                            answers.push((field.name.clone(), values));
                        }
                        if let Some(custom_name) =
                            field.custom_answer.as_ref().filter(|_| !written.is_empty())
                        {
                            answers.push((custom_name.clone(), vec![written.to_string()]));
                        }
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
            "set_setting" => {
                let selector = remote_selector(params["setting"].as_str().unwrap_or(""))?;
                let value = SharedString::from(
                    params["value"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("invalid_setting"))?
                        .to_string(),
                );
                if selector == Selector::Agent {
                    anyhow::ensure!(
                        thread.entries.is_empty() && !thread.running && thread.seat.is_none(),
                        "agent_fixed"
                    );
                }
                if selector == Selector::Mode {
                    anyhow::ensure!(!self.chat_mode && thread.seat.is_none(), "setting_fixed");
                }
                let current = match selector {
                    Selector::Agent => thread.agent.clone(),
                    Selector::Model => thread.model.clone(),
                    Selector::Effort => thread.effort.clone(),
                    Selector::Mode => thread.permission_mode.clone(),
                };
                check_setting(
                    selector,
                    &value,
                    &current,
                    &self.selector_choices_for(thread, selector, cx),
                )?;
                // 同じ値は送らない（bypass のまま選び直すと、ピルの道では承認待ちを許可してしまう）。
                if value != current {
                    // ピルで選んだのと同じ道（記憶にも書く）。Mac で見ているタブは動かさない。
                    let active = self.active;
                    self.active = index;
                    self.select_option(selector, value, cx);
                    self.active = active;
                    cx.notify();
                }
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

    /// スマホから新しいスレッドを作る。エージェント・モデル・思考量・権限モードは省いてよく（既定のまま）、
    /// 渡された物は作る**前に**全部確かめる（作ってから断ると空のタブが残る）。決め方はデスクトップで
    /// ＋ を押してピルで選んだのと同じ道（記憶にも書く）。Mac で見ているタブは動かさない。
    fn remote_new_thread(
        &mut self,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(!self.chat_mode, "method_not_allowed");
        let agent = match params["agent"].as_str() {
            Some(agent) => SharedString::from(agent.to_string()),
            None => default_agent_name(cx),
        };
        anyhow::ensure!(
            settings::agent_by_label(cx, &agent).is_some(),
            "unknown_agent"
        );
        anyhow::ensure!(
            agent_choices(SharedString::default(), cx)
                .iter()
                .any(|choice| choice.value == agent),
            "agent_unavailable"
        );
        anyhow::ensure!(self.threads.len() < 100, "thread_limit");
        let fresh = self.fresh_settings(&agent, cx);
        let mut chosen = Vec::new();
        for (name, selector, current) in [
            ("model", Selector::Model, &fresh.model),
            ("effort", Selector::Effort, &fresh.effort),
            ("mode", Selector::Mode, &fresh.mode),
        ] {
            let Some(value) = params[name].as_str() else {
                continue;
            };
            let value = SharedString::from(value.to_string());
            if value == *current {
                continue;
            }
            check_setting(
                selector,
                &value,
                current,
                &self.fresh_choices(&agent, selector, cx),
            )?;
            chosen.push((selector, value));
        }
        let active = self.active;
        let index = self.new_thread_index(cx);
        if self.threads[index].agent != agent {
            // エージェントのピルで切り替えたのと同じ道（その agent の記憶で開き直す・先張りも張り直す）。
            self.select_option(Selector::Agent, agent, cx);
        }
        for (selector, value) in chosen {
            self.select_option(selector, value, cx);
        }
        let id = self.threads[index].id.clone();
        self.switch_thread(active, cx);
        Ok(json!({ "thread_id": id }))
    }
}
