//! Captain の分解案（FLEET-V2 §5.5）— **worktree とブランチは人間が承認した分だけ切る**。
//!
//! Captain は `fleet_propose_tasks`（Captain 用の MCP・§5.8）で「この N 本に分けたい」を出すだけで、
//! worktree は作らない。案は DB（`captain_proposals`）に置いて要対応に**分解案カード**を 1 枚出し、
//! 人間が印を付けた行だけ ＋ Task と同じ流れ（`task_creation.rs` の作成中の行・取り消し・やり直し）で切る。
//! 裁きは台帳（`proposal_approved` / `proposal_rejected`）に積み、Captain を起こす（§5.3）。
//!
//! 実験で分かったこと（JOURNAL 2026-09-24）: Captain は 1 行の誤字にも迷わず worktree を切る。
//! necoder 自身なら 1 本ごとに GPUI の初回ビルドが走るので、切る前の承認がいちばん効く gate になる。

use super::fleet_view::FanoutTask;
use crate::workspace::*;
use anyhow::Context as _;

/// Captain 用の MCP（`necoder mcp --captain`）が出す道具。**これ以外は一覧に出さず、呼ばれても断る**。
/// 席の決まり（§5.8）の許可判断もこの名前で照合する（MCP と許可の二重の関所を 1 つの表で持つ）。
///
/// 出さないもの: `write_file`（書かない）・`fleet_create_task`（切るのは承認後の necoder）・
/// `fleet_update_task`（phase は担当が報告する）・`fleet_wait_task`（席を塞ぐ・結果は知らせで届く）・
/// `fleet_integrate_task`（人間 gate）。
pub const CAPTAIN_MCP_TOOLS: &[&str] = &[
    "fleet_list_tasks",
    "fleet_digest",
    "fleet_events",
    "fleet_propose_tasks",
    "fleet_spawn_agent",
    "fleet_send",
    "fleet_set_depends",
    "fleet_review_task",
    "list_files",
    "read_file",
    "search",
    "git_status",
];

/// 1 つの案に載せられる行の上限。それ以上は分けすぎ（研究の結論: 分けすぎは逐次依存で遅くなる）。
pub const MAX_PROPOSED_TASKS: usize = 6;

/// 分解案の 1 行 = 切りたい Task 1 本。目的と完了条件は**必須**（委任文の型・研究 P1）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProposedTask {
    pub title: String,
    pub goal: String,
    pub done_when: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// 担当エージェントの表示名（省略 = 既定のエージェント）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// `fleet_propose_tasks` の引数を確かめる。返り値は `(行, 分け方の理由)`。
/// エラー文はそのまま Captain へ返る（何を直せばよいかが分かる文にする）。
pub fn validate_proposed_tasks(
    arguments: &serde_json::Value,
) -> Result<(Vec<ProposedTask>, Option<String>), String> {
    let Some(items) = arguments.get("tasks").and_then(serde_json::Value::as_array) else {
        return Err(i18n::t!("captain.proposal_err_tasks"));
    };
    if items.is_empty() {
        return Err(i18n::t!("captain.proposal_err_tasks"));
    }
    if items.len() > MAX_PROPOSED_TASKS {
        return Err(i18n::t!("captain.proposal_err_too_many", "max" => MAX_PROPOSED_TASKS));
    }
    let text = |item: &serde_json::Value, key: &str| {
        item.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let mut tasks = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let row = index + 1;
        let required = |key: &str| {
            text(item, key)
                .ok_or_else(|| i18n::t!("captain.proposal_err_field", "row" => row, "field" => key))
        };
        let title = required("title")?;
        let goal = required("goal")?;
        let done_when = required("done_when")?;
        let agent = match text(item, "agent") {
            None => None,
            Some(raw) => Some(resolve_agent_label(&raw).ok_or_else(|| {
                i18n::t!(
                    "captain.proposal_err_agent",
                    "row" => row,
                    "agent" => &raw,
                    "known" => acp_client::AGENTS.iter().map(|kind| kind.label).collect::<Vec<_>>().join(", ")
                )
            })?),
        };
        tasks.push(ProposedTask {
            title,
            goal,
            done_when,
            scope: text(item, "scope"),
            agent,
        });
    }
    let note = arguments
        .get("note")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .map(str::to_string);
    Ok((tasks, note))
}

/// エージェントの指定を表示名へ寄せる（表示名 / id / 大文字小文字違いを受ける）。未知なら `None`。
fn resolve_agent_label(raw: &str) -> Option<String> {
    let wanted = raw.trim().to_lowercase();
    acp_client::AGENTS
        .iter()
        .find(|kind| kind.label.to_lowercase() == wanted || kind.id == wanted)
        .map(|kind| kind.label.to_string())
}

/// 分解案の id（`proposal-<ms>-<連番>`）。台帳の `task_id` 列にも入るので Task の id と衝突しない接頭辞にする。
pub fn new_proposal_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let serial = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(
        "proposal-{:x}-{:x}{serial:x}",
        now_unix_ms(),
        std::process::id()
    )
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 裁きを DB に残せなかった時、分解案のカードを戻すか（R09）。DB の失敗なら戻す（何も切っていないので
/// 押し直せる）。別の窓が先に裁いた・DB に無い案（[`storage::ProposalNotPending`]）は戻さない（押し直しても
/// 裁けないカードを残さない）。
fn card_survives_failed_resolve(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<storage::ProposalNotPending>()
        .is_none()
}

/// 承認した行を担当への最初の指示にする。**1 行目は題名**（＋ Task と同じく 1 行目から Task 名とブランチ名を作る）。
pub(crate) fn delegation_prompt(task: &ProposedTask) -> String {
    let mut prompt = i18n::t!(
        "captain.delegation",
        "title" => &task.title,
        "goal" => &task.goal,
        "done_when" => &task.done_when,
    );
    if let Some(scope) = &task.scope {
        prompt.push('\n');
        prompt.push_str(&i18n::t!("captain.delegation_scope", "scope" => scope));
    }
    prompt
}

/// 画面に出している分解案（DB の行 + 読み解いた行 + 行ごとの印）。
pub(crate) struct CaptainProposal {
    pub(crate) record: storage::CaptainProposalRecord,
    pub(crate) tasks: Vec<ProposedTask>,
    /// 行ごとの印（既定は全部 true）。外した行は承認しても切らない。
    pub(crate) selected: Vec<bool>,
}

impl CaptainProposal {
    /// DB の行から組む。`tasks` が読めない案は捨てずにエラーにする（黙って消さない）。
    pub(crate) fn from_record(record: storage::CaptainProposalRecord) -> anyhow::Result<Self> {
        let tasks: Vec<ProposedTask> = serde_json::from_str(&record.tasks)
            .with_context(|| format!("分解案 {} の行を読めない", record.id))?;
        let selected = vec![true; tasks.len()];
        Ok(Self {
            record,
            tasks,
            selected,
        })
    }

    pub(crate) fn chosen_count(&self) -> usize {
        self.selected.iter().filter(|selected| **selected).count()
    }
}

impl Workspace {
    /// 分解案を画面へ足す（IPC の `propose_tasks` と起動時の復元）。同じ id は重ねない。
    pub(crate) fn add_captain_proposal(
        &mut self,
        proposal: CaptainProposal,
        cx: &mut Context<Self>,
    ) {
        if self
            .chrome
            .captain_proposals
            .iter()
            .any(|existing| existing.record.id == proposal.record.id)
        {
            return;
        }
        self.chrome.captain_proposals.push(proposal);
        cx.notify();
    }

    /// 起動時: 裁いていない分解案と、分解案から作られた Task（`⚑` 帰属）を DB から戻す。
    pub(crate) fn restore_captain_proposals(&mut self, cx: &mut Context<Self>) {
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        cx.spawn(async move |workspace, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    let proposals = storage.load_pending_captain_proposals();
                    let origins = storage.load_task_ids_with_event(CAPTAIN_TASK_EVENT);
                    (proposals, origins)
                })
                .await;
            let (proposals, origins) = loaded;
            let _ = workspace.update(cx, |workspace, cx| {
                match proposals {
                    Ok(records) => {
                        for record in records {
                            match CaptainProposal::from_record(record) {
                                Ok(proposal) => workspace.add_captain_proposal(proposal, cx),
                                Err(error) => eprintln!("{error:#}"),
                            }
                        }
                    }
                    Err(error) => eprintln!("分解案を復元できない: {error:#}"),
                }
                match origins {
                    Ok(ids) => {
                        for slot in &mut workspace.project_sessions.projects {
                            if ids.iter().any(|id| id == slot.task_space.id.as_str()) {
                                slot.task_space.captain_origin = true;
                            }
                        }
                    }
                    Err(error) => eprintln!("⚑ 帰属を復元できない: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 分解案カードの行の印を切り替える。
    pub(crate) fn toggle_proposal_row(
        &mut self,
        proposal_id: &str,
        row: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some(selected) = self
            .chrome
            .captain_proposals
            .iter_mut()
            .find(|proposal| proposal.record.id == proposal_id)
            .and_then(|proposal| proposal.selected.get_mut(row))
        {
            *selected = !*selected;
            cx.notify();
        }
    }

    /// 承認: 印の付いた行だけブランチと worktree を切る。印が 0 本なら却下と同じ。
    /// 先に DB で裁きを確定させてから切る（二度押し・別窓との競合で worktree を二重に切らない）。
    pub(crate) fn approve_captain_proposal(&mut self, proposal_id: &str, cx: &mut Context<Self>) {
        let Some(position) = self
            .chrome
            .captain_proposals
            .iter()
            .position(|proposal| proposal.record.id == proposal_id)
        else {
            return;
        };
        if self.chrome.captain_proposals[position].chosen_count() == 0 {
            self.reject_captain_proposal(proposal_id, cx);
            return;
        }
        let proposal = self.chrome.captain_proposals.remove(position);
        let (chosen, skipped): (Vec<_>, Vec<_>) = proposal
            .tasks
            .iter()
            .cloned()
            .zip(proposal.selected.iter().copied())
            .partition(|(_, selected)| *selected);
        let chosen: Vec<ProposedTask> = chosen.into_iter().map(|(task, _)| task).collect();
        let outcome = serde_json::json!({
            "approved": chosen.iter().map(|task| task.title.as_str()).collect::<Vec<_>>(),
            "skipped": skipped.iter().map(|(task, _)| task.title.as_str()).collect::<Vec<_>>(),
        })
        .to_string();
        self.resolve_proposal(
            proposal,
            storage::ProposalStatus::Approved,
            outcome,
            chosen,
            cx,
        );
    }

    /// 却下: 何も作らない。
    pub(crate) fn reject_captain_proposal(&mut self, proposal_id: &str, cx: &mut Context<Self>) {
        let Some(position) = self
            .chrome
            .captain_proposals
            .iter()
            .position(|proposal| proposal.record.id == proposal_id)
        else {
            return;
        };
        let proposal = self.chrome.captain_proposals.remove(position);
        let outcome = serde_json::json!({
            "rejected": proposal.tasks.iter().map(|task| task.title.as_str()).collect::<Vec<_>>(),
        })
        .to_string();
        self.resolve_proposal(
            proposal,
            storage::ProposalStatus::Rejected,
            outcome,
            Vec::new(),
            cx,
        );
    }

    fn resolve_proposal(
        &mut self,
        proposal: CaptainProposal,
        status: storage::ProposalStatus,
        outcome: String,
        chosen: Vec<ProposedTask>,
        cx: &mut Context<Self>,
    ) {
        let repository = proposal.record.repository_id.clone();
        let proposal_id = proposal.record.id.clone();
        let Some(storage) = self.persistence.storage.clone() else {
            // 保存先が無い（テスト等）: 画面の上だけで裁く。
            self.create_proposed_tasks(&repository, &proposal_id, chosen, cx);
            cx.notify();
            return;
        };
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let id_for_resolve = proposal_id.clone();
            let resolved = cx
                .background_executor()
                .spawn(async move { storage.resolve_captain_proposal(&id_for_resolve, status, &outcome) })
                .await;
            // Err = 待っている間に窓が閉じた。裁きが DB に残っていれば、その案の Task は切られないまま
            // （承認から作成までの再開の記録は無い・UX-CODE-REVIEW R09 の残り）。
            workspace
                .update(cx, |workspace, cx| match resolved {
                    Ok(_) => {
                        workspace.create_proposed_tasks(&repository, &proposal_id, chosen, cx);
                        workspace.request_captain_wake(&repository, cx);
                    }
                    Err(error) => {
                        // 裁きを残せなかった（DB の失敗）: カードを印ごと戻す（R09・押し直せる。何も切っていない）。
                        if card_survives_failed_resolve(&error) {
                            workspace.add_captain_proposal(proposal, cx);
                        }
                        let accent = workspace.accent();
                        workspace.push_toast(
                            SharedString::from(i18n::t!("captain.proposal_err_resolve", "detail" => format!("{error:#}"))),
                            accent,
                            cx,
                        );
                    }
                })
                .ok();
        })
        .detach();
    }

    /// 承認した行を ＋ Task と同じ経路で切る（準備スクリプト・担当の起動・最初の指示まで同じ）。
    fn create_proposed_tasks(
        &mut self,
        repository: &str,
        proposal_id: &str,
        chosen: Vec<ProposedTask>,
        cx: &mut Context<Self>,
    ) {
        if chosen.is_empty() {
            return;
        }
        let Some(integration) = self.integration_slot_for(repository) else {
            let accent = self.accent();
            self.push_toast(
                SharedString::from(i18n::t!("captain.proposal_err_no_integration")),
                accent,
                cx,
            );
            return;
        };
        let jobs = chosen
            .into_iter()
            .map(|task| {
                let prompt = delegation_prompt(&task);
                let job = FanoutTask {
                    // 1 行目（題名）から Task 名とブランチ名を作る（＋ Task と同じ）。
                    slug_source: task.title.clone(),
                    branch: None,
                    agent: task.agent.clone(),
                    title_suffix: None,
                    captain_proposal: Some(proposal_id.to_string()),
                };
                (prompt, job)
            })
            .collect();
        self.create_proposed_task_jobs(integration, jobs, cx);
    }

    /// 分解案から作った Task に `⚑` 帰属を付け、台帳に `captain_task` を積む（再起動後も印が戻る）。
    pub(crate) fn mark_captain_task(
        &mut self,
        space: usize,
        proposal_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get_mut(space) else {
            return;
        };
        slot.task_space.captain_origin = true;
        let task_id = slot.task_space.id.0.clone();
        if let Some(storage) = self.persistence.storage.clone() {
            let payload = serde_json::json!({ "proposal": proposal_id }).to_string();
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) =
                        storage.append_task_event(&task_id, CAPTAIN_TASK_EVENT, &payload)
                    {
                        eprintln!("⚑ 帰属を記録できない: {error:#}");
                    }
                })
                .detach();
        }
        cx.notify();
    }

    /// 要対応に出す分解案（このリポジトリの分・古い順）。
    pub(crate) fn captain_proposals_for(
        &self,
        repository: &str,
    ) -> impl Iterator<Item = &CaptainProposal> {
        let repository = repository.to_string();
        self.chrome
            .captain_proposals
            .iter()
            .filter(move |proposal| proposal.record.repository_id == repository)
    }

    /// 分解案カードの中身（見出しの下）。**承認 / 却下を先頭に置く**: 要対応の欄は高さに上限があって
    /// スクロールするので、ボタンが行の下だと隠れる（2026-09-24 の実画面で確認）。理由は 2 行、
    /// 目的と完了は 1 行ずつに畳む（全文は Captain の会話にある）。
    pub(crate) fn render_proposal_body(
        &self,
        position: usize,
        proposal_id: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let Some(proposal) = self
            .chrome
            .captain_proposals
            .iter()
            .find(|proposal| proposal.record.id == proposal_id)
        else {
            return div().into_any_element();
        };
        let chosen = proposal.chosen_count();
        let approve_id = proposal_id.to_string();
        let reject_id = proposal_id.to_string();
        let button = |id: (&'static str, usize), label: SharedString, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(8.))
                .py(px(3.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.border)
                .when(primary, |element| element.bg(theme.bg2))
                .text_size(px(10.))
                .text_color(if primary { theme.fg0 } else { theme.fg1 })
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3))
                .child(label)
        };
        let mut body = div().flex().flex_col().gap(px(6.)).child(
            div()
                .flex()
                .gap(px(5.))
                .child(
                    button(
                        ("proposal-approve", position),
                        SharedString::from(i18n::t!("captain.proposal_approve", "n" => chosen)),
                        chosen > 0,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.approve_captain_proposal(&approve_id, cx);
                        }),
                    ),
                )
                .child(
                    button(
                        ("proposal-reject", position),
                        SharedString::from(i18n::t!("captain.proposal_reject")),
                        false,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.reject_captain_proposal(&reject_id, cx);
                        }),
                    ),
                ),
        );
        if let Some(note) = &proposal.record.note {
            body = body.child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg1)
                    .line_clamp(2)
                    .child(SharedString::from(note.clone())),
            );
        }
        let line = |text: String, color: Hsla| {
            div()
                .text_size(px(10.))
                .text_color(color)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(SharedString::from(text))
        };
        for (row, task) in proposal.tasks.iter().enumerate() {
            let selected = proposal.selected.get(row).copied().unwrap_or(false);
            let id_for_toggle = proposal_id.to_string();
            let mut detail = div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(11.))
                                .text_color(if selected { theme.fg0 } else { theme.fg2 })
                                .child(SharedString::from(task.title.clone())),
                        )
                        .when_some(task.agent.clone(), |element, agent| {
                            element.child(
                                div()
                                    .flex_none()
                                    .text_size(px(9.5))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(agent)),
                            )
                        }),
                )
                .child(line(
                    i18n::t!("captain.proposal_goal", "text" => &task.goal),
                    theme.fg1,
                ))
                .child(line(
                    i18n::t!("captain.proposal_done_when", "text" => &task.done_when),
                    theme.fg1,
                ));
            if let Some(scope) = &task.scope {
                detail = detail.child(line(
                    i18n::t!("captain.proposal_scope", "text" => scope),
                    theme.fg2,
                ));
            }
            body = body.child(
                div()
                    .id(("proposal-row", position * 16 + row))
                    .flex()
                    .items_start()
                    .gap(px(7.))
                    .py(px(2.))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_proposal_row(&id_for_toggle, row, cx);
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .pt(px(1.))
                            .text_size(px(11.))
                            .text_color(if selected { theme.fg0 } else { theme.fg2 })
                            .child(if selected { "☑" } else { "☐" }),
                    )
                    .child(detail),
            );
        }
        body.into_any_element()
    }
}

/// 分解案から作った Task に付ける台帳の印（`⚑` 帰属の素材）。
pub(crate) const CAPTAIN_TASK_EVENT: &str = "captain_task";

#[cfg(test)]
mod tests {
    use super::*;

    /// R09: DB の失敗ならカードを戻し、もう裁けない案（先に裁かれた・DB に無い）は戻さない。
    #[test]
    fn only_a_database_failure_brings_the_card_back() {
        assert!(card_survives_failed_resolve(&anyhow::anyhow!(
            "database is locked"
        )));
        let resolved = anyhow::Error::new(storage::ProposalNotPending {
            id: "proposal-1".into(),
            missing: false,
        });
        assert!(!card_survives_failed_resolve(&resolved));
        assert!(!card_survives_failed_resolve(
            &resolved.context("分解案の裁きの追記に失敗")
        ));
    }

    fn arguments(value: serde_json::Value) -> Result<(Vec<ProposedTask>, Option<String>), String> {
        validate_proposed_tasks(&value)
    }

    #[test]
    fn proposals_require_goal_and_done_condition_on_every_row() {
        let (tasks, note) = arguments(serde_json::json!({
            "tasks": [
                { "title": " README の誤字 ", "goal": "Calcurator を直す", "done_when": "grep で 0 件", "agent": "codex" },
                { "title": "割り算", "goal": "divide を足す", "done_when": "pytest が通る", "scope": "calc.py と tests/" }
            ],
            "note": "実装とテストは 1 本に束ねた"
        }))
        .unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].title, "README の誤字", "前後の空白は落とす");
        assert_eq!(
            tasks[0].agent.as_deref(),
            Some("Codex"),
            "id でも表示名へ寄せる"
        );
        assert_eq!(tasks[1].scope.as_deref(), Some("calc.py と tests/"));
        assert_eq!(note.as_deref(), Some("実装とテストは 1 本に束ねた"));

        assert!(arguments(serde_json::json!({ "tasks": [] })).is_err());
        assert!(arguments(serde_json::json!({})).is_err());
        let missing =
            arguments(serde_json::json!({ "tasks": [{ "title": "x", "goal": "y" }] })).unwrap_err();
        assert!(
            missing.contains("done_when"),
            "何が足りないかを返す: {missing}"
        );
        let unknown = arguments(serde_json::json!({
            "tasks": [{ "title": "x", "goal": "y", "done_when": "z", "agent": "gpt" }]
        }))
        .unwrap_err();
        assert!(
            unknown.contains("Claude Code"),
            "使える名前を添える: {unknown}"
        );
        let too_many: Vec<_> = (0..=MAX_PROPOSED_TASKS)
            .map(|index| serde_json::json!({ "title": format!("t{index}"), "goal": "g", "done_when": "d" }))
            .collect();
        assert!(arguments(serde_json::json!({ "tasks": too_many })).is_err());
    }

    #[test]
    fn delegation_prompt_starts_with_the_title() {
        let prompt = delegation_prompt(&ProposedTask {
            title: "割り算を足す".into(),
            goal: "divide を足す".into(),
            done_when: "pytest が通る".into(),
            scope: Some("calc.py".into()),
            agent: None,
        });
        assert_eq!(
            prompt.lines().next(),
            Some("割り算を足す"),
            "1 行目から Task 名とブランチ名を作る"
        );
        assert!(
            prompt.contains("divide を足す")
                && prompt.contains("pytest が通る")
                && prompt.contains("calc.py")
        );
    }

    #[test]
    fn proposal_ids_do_not_collide_with_task_ids() {
        let first = new_proposal_id();
        let second = new_proposal_id();
        assert!(first.starts_with("proposal-") && !first.starts_with("space-"));
        assert_ne!(first, second);
    }
}
