//! Captain の分解案（FLEET-V2 §5.5）— **worktree とブランチは人間が承認した分だけ切る**。
//!
//! Captain は `fleet_propose_tasks`（Captain 用の MCP・§5.8）で「この N 本に分けたい」を出すだけで、
//! worktree は作らない。案は DB（`captain_proposals`）に置いて要対応に**分解案カード**を 1 枚出し、
//! 人間が印を付けた行だけ ＋ Task と同じ流れ（`task_creation.rs` の作成中の行・取り消し・やり直し）で切る。
//! 裁きは台帳（`proposal_approved` / `proposal_rejected`）に積み、Captain を起こす（§5.3）。
//!
//! 実験で分かったこと（JOURNAL 2026-09-24）: Captain は 1 行の誤字にも迷わず worktree を切る。
//! necoder 自身なら 1 本ごとに GPUI の初回ビルドが走るので、切る前の承認がいちばん効く gate になる。
//!
//! **承認した行は終了を越えて続けられる**（UX-CODE-REVIEW R09 の条件 1）: 行ごとの実行記録
//! （DB `captain_proposal_rows`・待機 / 作成中 / 成功 / 失敗 / やめた + 段）を承認と同じトランザクションで置き、
//! 段（worktree・準備・Task の登録・担当の起動・委任文の送信）の**前に**書き進める。DB と git は 1 つの
//! トランザクションにできないので、ブランチ名と worktree の場所は作る前に書き、やり直しは既にある物を使う
//! （同じ行で 2 本作らない）。Task の登録は台帳（Task・遷移・`⚑` 帰属）と行の記録を 1 トランザクションで書く。
//! 起動時は終わっていない行をサイドバーに「中断した行」として戻すだけで、**自動では続けない**（起動しただけで
//! エージェントを起こさない）。人が「再開」を押した行だけ、記録した段から続ける。

use super::fleet_view::FanoutTask;
use super::task_creation::{CreatedWorktree, CreationStage};
use crate::workspace::*;
use anyhow::Context as _;
use storage::{ProposalRowRecord, ProposalRowStatus, ProposalRowStep};

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
    "fleet_recommend",
    "list_files",
    "read_file",
    "search",
    "git_status",
];

/// Captain の席**だけ**に出す道具（普通の `necoder mcp` = Full 版には出さず、呼ばれても断る）。
/// `fleet_recommend` の推薦は要対応カードに「✳ Captain:」として出るので、担当が自分の承認要求に
/// Captain の名で見立てを付ける道を道具として渡さない（FLEET-V2 §5.5）。
pub const CAPTAIN_ONLY_MCP_TOOLS: &[&str] = &["fleet_recommend"];

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

/// 分解案の 1 行（案の id + 行の番号）。実行記録の鍵（R09）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ProposalRowKey {
    pub(crate) proposal_id: String,
    pub(crate) row: usize,
}

impl ProposalRowKey {
    pub(crate) fn of(record: &ProposalRowRecord) -> Self {
        Self {
            proposal_id: record.proposal_id.clone(),
            row: record.row,
        }
    }
}

/// 行の区切り（necoder が落ちうる所）。テストはここで止めて、残った記録と git から再開できることを確かめる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowCheckpoint {
    /// 行に手を付ける前（承認を確定した直後の行はここ）。
    Start,
    /// 段の記録を書いた直後（その段はまだ何もしていない）。
    Recorded(ProposalRowStep),
    /// 段をやり終えた直後（次の段の記録を書く前）。
    Done(ProposalRowStep),
}

/// いま動かしている分解案の行（この起動の全窓で共有）。同じ行を 2 つの窓で同時に動かさない。プロセスが
/// 落ちれば消えるので、起動時に DB で終わっていない行は、どの窓でも動いていない（＝中断した行）。
#[derive(Default)]
struct RunningProposalRows(std::collections::HashSet<ProposalRowKey>);

impl gpui::Global for RunningProposalRows {}

/// 中断した行の 2 段目に出す「どこで止まったか」。
pub(crate) fn interrupted_step_label(record: &ProposalRowRecord) -> String {
    i18n::t!(interrupted_step_key(record))
}

/// [`interrupted_step_label`] の i18n キー（表示言語に依らずに段を見分ける）。
fn interrupted_step_key(record: &ProposalRowRecord) -> &'static str {
    // 名前を決める前に止まった行は、まだ何も作っていない。
    if record.status == ProposalRowStatus::Waiting || record.branch.is_none() {
        return "fleet.creating_step_waiting";
    }
    match record.step {
        ProposalRowStep::Worktree => "fleet.creating_step_worktree",
        ProposalRowStep::Setup => "fleet.creating_step_setup",
        ProposalRowStep::Register => "fleet.creating_step_register",
        ProposalRowStep::Spawn => "fleet.creating_step_spawn",
        ProposalRowStep::Send => "fleet.creating_step_send",
    }
}

/// 「再開」/「やり直す」の説明。先頭にどこで止まったか（作れなかった行は理由の全文＝行には 1 行目しか出ず、
/// 再起動の後はトーストも無い）、末尾に決めてあったブランチ名（その名前の worktree を使う・作る）。
pub(crate) fn resume_tip(record: &ProposalRowRecord, interrupted: bool) -> String {
    let (status, tip) = if interrupted {
        let tip = if record.step == ProposalRowStep::Send {
            i18n::t!("fleet.creating_resume_send_tip")
        } else {
            i18n::t!("fleet.creating_resume_tip")
        };
        (
            i18n::t!("fleet.creating_interrupted", "step" => interrupted_step_label(record)),
            tip,
        )
    } else {
        (
            record.reason.clone().unwrap_or_default(),
            i18n::t!("fleet.creating_proposal_retry_tip"),
        )
    };
    let mut lines = vec![status.trim().to_string(), tip];
    if let Some(branch) = &record.branch {
        lines.push(format!("⎇ {branch}"));
    }
    lines.retain(|line| !line.is_empty());
    lines.join("\n")
}

/// 動いていない行の出し方: 作れなかった行は「作れませんでした: 理由の 1 行目 + やり直す」、それ以外（待機・
/// 作成中のまま止まった）は「中断 + 再開」。
fn resting_stage(record: &ProposalRowRecord) -> CreationStage {
    match record.status {
        ProposalRowStatus::Failed => CreationStage::Failed(SharedString::from(
            record
                .reason
                .as_deref()
                .and_then(|reason| reason.lines().next())
                .unwrap_or("")
                .trim()
                .to_string(),
        )),
        _ => CreationStage::Interrupted,
    }
}

/// 会話に人の発話が 1 つでもあるか（前の起動で委任文が届いて保存されていた・人が先に書いた）。復元した
/// スレッドは `last_prompt` を持たないので、transcript の人の発話（`▸`）で見る。
fn has_human_message(panel: &AgentPanel) -> bool {
    (0..)
        .map_while(|thread| panel.thread_id(thread).map(|_| thread))
        .any(|thread| {
            panel
                .transcript_lines(thread, usize::MAX)
                .iter()
                .any(|(bullet, _)| bullet.as_ref() == "▸")
        })
}

/// 分解案の行を ＋ Task の作成中の行として扱う時の「切り方」（題名からブランチ名・担当のエージェント）。
fn proposal_fanout_task(task: &ProposedTask) -> FanoutTask {
    FanoutTask {
        slug_source: task.title.clone(),
        branch: None,
        agent: task.agent.clone(),
        title_suffix: None,
    }
}

/// 行を動かす 1 回分の共通の材料（統合先の場所と DB）。
struct RowRun {
    host: Arc<dyn host::Host>,
    root: PathBuf,
    repository_key: String,
    storage: Option<storage::Storage>,
}

/// 1 行分の中身（作成中の行から取り出す・行を外した後も続きの段で使う）。
struct RowJob {
    record: ProposalRowRecord,
    prompt: String,
    task: FanoutTask,
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

    /// 起動時: 裁いていない分解案と、分解案から作られた Task（`⚑` 帰属）と、承認した案の終わっていない行
    /// （中断した行・R09）を DB から戻す。行は戻すだけで続けない（人が「再開」を押すまで何も作らない）。
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
                    let rows = storage.load_open_proposal_rows();
                    (proposals, origins, rows)
                })
                .await;
            let (proposals, origins, rows) = loaded;
            let _ = workspace.update(cx, |workspace, cx| {
                match rows {
                    Ok(rows) => workspace.restore_proposal_rows(rows, cx),
                    Err(error) => eprintln!("分解案の中断した行を復元できない: {error:#}"),
                }
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
    /// 切る行の実行記録は裁きと同じトランザクションで置く（承認の直後に落ちても、切る行が残る・R09）。
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
        let mut chosen = Vec::new();
        let mut skipped = Vec::new();
        for (row, (task, selected)) in proposal
            .tasks
            .iter()
            .zip(proposal.selected.iter().copied())
            .enumerate()
        {
            if selected {
                chosen.push((row, task.clone()));
            } else {
                skipped.push(task.title.clone());
            }
        }
        let outcome = serde_json::json!({
            "approved": chosen.iter().map(|(_, task)| task.title.as_str()).collect::<Vec<_>>(),
            "skipped": skipped,
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

    /// 裁きを DB に確定させ、承認なら印の付いた行（`chosen` = 案の中の位置と行）を切り始める。
    fn resolve_proposal(
        &mut self,
        proposal: CaptainProposal,
        status: storage::ProposalStatus,
        outcome: String,
        chosen: Vec<(usize, ProposedTask)>,
        cx: &mut Context<Self>,
    ) {
        let repository = proposal.record.repository_id.clone();
        let proposal_id = proposal.record.id.clone();
        let rows: Vec<usize> = chosen.iter().map(|(row, _)| *row).collect();
        let Some(storage) = self.persistence.storage.clone() else {
            // 保存先が無い（テスト等）: 画面の上だけで裁く（行の記録は書かない）。
            self.start_proposal_rows(&repository, &proposal_id, chosen, cx);
            cx.notify();
            return;
        };
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let id_for_resolve = proposal_id.clone();
            let resolved = cx
                .background_executor()
                .spawn(async move {
                    storage.resolve_captain_proposal(&id_for_resolve, status, &outcome, &rows)
                })
                .await;
            // Err = 待っている間に窓が閉じた。裁きと行の記録が DB に残っていれば、次の起動で中断した行
            // として戻る（R09）。
            workspace
                .update(cx, |workspace, cx| match resolved {
                    Ok(_) => {
                        workspace.start_proposal_rows(&repository, &proposal_id, chosen, cx);
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

    /// 承認した行を ＋ Task と同じ作成中の行で出し、順に切る（準備スクリプト・担当の起動・最初の指示まで）。
    /// 統合先が開いていなければ、行は中断した行として残す（記録は待機のまま・開いてから再開できる）。
    fn start_proposal_rows(
        &mut self,
        repository: &str,
        proposal_id: &str,
        chosen: Vec<(usize, ProposedTask)>,
        cx: &mut Context<Self>,
    ) {
        if chosen.is_empty() {
            return;
        }
        let integration = self.integration_slot_for(repository);
        let ids: Vec<u64> = chosen
            .iter()
            .map(|(row, task)| {
                let stage = match integration {
                    None => CreationStage::Interrupted,
                    Some(_) => CreationStage::Waiting,
                };
                self.begin_proposal_row(
                    repository,
                    &delegation_prompt(task),
                    proposal_fanout_task(task),
                    ProposalRowRecord::waiting(proposal_id, *row),
                    stage,
                )
            })
            .collect();
        let Some(integration) = integration else {
            let accent = self.accent();
            self.push_toast(
                SharedString::from(i18n::t!("captain.proposal_err_no_integration")),
                accent,
                cx,
            );
            cx.notify();
            return;
        };
        self.run_proposal_rows(integration, ids, true, cx);
    }

    /// 分解案の行を作成中の行として出す（承認・起動時の復元・作れなかった行の出し直し）。
    fn begin_proposal_row(
        &mut self,
        repository_key: &str,
        prompt: &str,
        task: FanoutTask,
        record: ProposalRowRecord,
        stage: CreationStage,
    ) -> u64 {
        let id = self.begin_task_creation(
            repository_key,
            prompt,
            &project::TaskStart::default(),
            task,
            stage,
        );
        if let Some(creation) = self
            .chrome
            .task_creations
            .iter_mut()
            .find(|creation| creation.id == id)
        {
            creation.proposal_row = Some(record);
        }
        id
    }

    /// 分解案の行を順に作る（承認・再開の共通・R09）。`ids` は作成中の行の id。行は段ごとに記録を**書いてから**
    /// 進むので、途中で落ちてもその段から続けられる。1 行の失敗で残りを止めない。`sync_base` = 最初に統合先を
    /// upstream へ早送りする（worktree を作る前の行がある時）。舞台は奪わない（fan-out ではない）。
    fn run_proposal_rows(
        &mut self,
        integration_index: usize,
        ids: Vec<u64>,
        sync_base: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get(integration_index) else {
            return;
        };
        let run = RowRun {
            host: slot.worktree.host().clone(),
            root: slot.worktree.root().to_path_buf(),
            repository_key: slot.repository_key().to_string(),
            storage: self.persistence.storage.clone(),
        };
        let rows: Vec<(u64, Option<ProposalRowKey>)> = ids
            .iter()
            .map(|id| {
                let key = self
                    .chrome
                    .task_creations
                    .iter()
                    .find(|creation| creation.id == *id)
                    .and_then(|creation| creation.proposal_row.as_ref())
                    .map(ProposalRowKey::of);
                (*id, key)
            })
            .collect();
        cx.default_global::<RunningProposalRows>()
            .0
            .extend(rows.iter().filter_map(|(_, key)| key.clone()));
        let turn = super::task_creation::creation_turn(&run.repository_key, cx);
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            // 同じリポジトリの前の回（＋ Task・分解案の行）が終わるまで待つ（行は「順番を待っています…」のまま）。
            let _turn = turn.lock().await;
            let mut finished = 0;
            let ran =
                drive_proposal_rows(&workspace, &run, &rows, sync_base, &mut finished, cx).await;
            // この回が引き受けたまま終わらなかった行（窓が閉じた・落ちたことにして止めた）も放す。
            cx.update(|cx| {
                let running = &mut cx.default_global::<RunningProposalRows>().0;
                for key in rows
                    .iter()
                    .skip(finished)
                    .filter_map(|(_, key)| key.as_ref())
                {
                    running.remove(key);
                }
            });
            if let Err(error) = ran {
                eprintln!(
                    "分解案の行をここで止めた（続きは中断した行として再開できる）: {error:#}"
                );
            }
        })
        .detach();
    }

    /// 中断した行の「再開」・作れなかった行の「やり直す」（R09）: DB の今の記録を読み直してから、その段から続ける
    /// （別の窓が先に済ませた・閉じた行を、起動時の写しのまま動かさない）。
    pub(crate) fn resume_proposal_row(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(creation) = self
            .chrome
            .task_creations
            .iter()
            .find(|creation| creation.id == id)
        else {
            return;
        };
        let Some(record) = creation.proposal_row.clone() else {
            return;
        };
        if !matches!(
            creation.stage,
            CreationStage::Interrupted | CreationStage::Failed(_)
        ) {
            return;
        }
        let previous_stage = creation.stage.clone();
        let repository_key = creation.repository_key.clone();
        let key = ProposalRowKey::of(&record);
        if self.integration_slot_for(&repository_key).is_none() {
            let accent = self.accent();
            self.push_toast(
                SharedString::from(i18n::t!("captain.proposal_err_no_integration")),
                accent,
                cx,
            );
            return;
        }
        if !cx
            .default_global::<RunningProposalRows>()
            .0
            .insert(key.clone())
        {
            // 別の窓がこの行を動かしている。
            self.remove_task_creation(id, cx);
            let accent = self.accent();
            self.push_toast(
                SharedString::from(i18n::t!("fleet.creating_proposal_running_elsewhere")),
                accent,
                cx,
            );
            return;
        }
        self.set_creation_stage(id, CreationStage::Waiting, cx);
        let Some(storage) = self.persistence.storage.clone() else {
            self.start_resumed_proposal_row(id, record, cx);
            return;
        };
        cx.spawn(async move |workspace, cx| {
            let fresh = {
                let key = key.clone();
                cx.background_executor()
                    .spawn(async move { storage.load_proposal_row(&key.proposal_id, key.row) })
                    .await
            };
            let updated = workspace.update(cx, |workspace, cx| match fresh {
                Ok(Some(fresh)) if fresh.status.is_open() => {
                    workspace.start_resumed_proposal_row(id, fresh, cx)
                }
                Ok(_) => {
                    // 別の窓で作り終えた・やめた行（記録が閉じている）。
                    workspace.release_proposal_row(&key, cx);
                    workspace.remove_task_creation(id, cx);
                    let accent = workspace.accent();
                    workspace.push_toast(
                        SharedString::from(i18n::t!("fleet.creating_proposal_done_elsewhere")),
                        accent,
                        cx,
                    );
                }
                Err(error) => {
                    workspace.release_proposal_row(&key, cx);
                    workspace.set_creation_stage(id, previous_stage, cx);
                    let accent = workspace.accent();
                    workspace.push_toast(SharedString::from(format!("{error:#}")), accent, cx);
                }
            });
            if updated.is_err() {
                // 読んでいる間に窓が閉じた。
                cx.update(|cx| {
                    cx.default_global::<RunningProposalRows>().0.remove(&key);
                });
            }
        })
        .detach();
    }

    /// 読み直した記録で行を動かす（読んでいる間にやめた行は動かさない）。
    fn start_resumed_proposal_row(
        &mut self,
        id: u64,
        record: ProposalRowRecord,
        cx: &mut Context<Self>,
    ) {
        let key = ProposalRowKey::of(&record);
        let Some(creation) = self
            .chrome
            .task_creations
            .iter_mut()
            .find(|creation| creation.id == id)
        else {
            self.release_proposal_row(&key, cx);
            return;
        };
        // worktree を作る前の行なら、＋ Task と同じく統合先を最新にしてから切る。
        let sync_base = record.step == ProposalRowStep::Worktree;
        let resting = resting_stage(&record);
        creation.proposal_row = Some(record);
        let repository_key = creation.repository_key.clone();
        let Some(integration) = self.integration_slot_for(&repository_key) else {
            self.release_proposal_row(&key, cx);
            self.set_creation_stage(id, resting, cx);
            return;
        };
        self.run_proposal_rows(integration, vec![id], sync_base, cx);
    }

    fn release_proposal_row(&mut self, key: &ProposalRowKey, cx: &mut Context<Self>) {
        cx.default_global::<RunningProposalRows>().0.remove(key);
    }

    fn set_creation_stage(&mut self, id: u64, stage: CreationStage, cx: &mut Context<Self>) {
        if let Some(creation) = self
            .chrome
            .task_creations
            .iter_mut()
            .find(|creation| creation.id == id)
        {
            creation.stage = stage;
            cx.notify();
        }
    }

    /// 作成中の行を外す（あれば）。
    fn remove_task_creation(&mut self, id: u64, cx: &mut Context<Self>) {
        let before = self.chrome.task_creations.len();
        self.chrome
            .task_creations
            .retain(|creation| creation.id != id);
        if self.chrome.task_creations.len() != before {
            cx.notify();
        }
    }

    /// 行をやめた（記録を閉じる・まだ終わっていなければ）。作ってあった worktree・Task は消さない。
    pub(crate) fn close_proposal_row(
        &mut self,
        record: &ProposalRowRecord,
        cx: &mut Context<Self>,
    ) {
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        let (proposal_id, row) = (record.proposal_id.clone(), record.row);
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = storage.discard_proposal_row(&proposal_id, row) {
                    eprintln!("分解案の行を閉じられない: {error:#}");
                }
            })
            .detach();
    }

    /// 準備の失敗で控えていた委任文を送った（「準備をやり直す」/「準備を飛ばして始める」）: 行は成功（R09）。
    pub(crate) fn finish_held_proposal_row(&mut self, space: &SpaceId, cx: &mut Context<Self>) {
        let Some(mut record) = self.chrome.held_proposal_rows.remove(space) else {
            return;
        };
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        record.status = ProposalRowStatus::Succeeded;
        record.reason = None;
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = storage.save_proposal_row(&record) {
                    eprintln!("分解案の行の成功を記録できない: {error:#}");
                }
            })
            .detach();
    }

    /// Task を片付けた（archived）・worktree を消した: その Task の分解案の行を閉じる。準備の失敗で委任文を
    /// 控えていた行も、中断した行として出していた行も（次の起動で・「再開」で、片付けた Task を開き直さない）。
    pub(crate) fn close_proposal_rows_for_task(
        &mut self,
        space: &SpaceId,
        root: &Path,
        cx: &mut Context<Self>,
    ) {
        if let Some(record) = self.chrome.held_proposal_rows.remove(space) {
            self.close_proposal_row(&record, cx);
        }
        let belongs = |record: &ProposalRowRecord| {
            record.task_id.as_deref() == Some(space.as_str())
                || record.target.as_deref() == Some(root)
        };
        let (closing, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.chrome.task_creations)
            .into_iter()
            .partition(|creation| {
                creation.proposal_row.as_ref().is_some_and(belongs)
                    && matches!(
                        creation.stage,
                        CreationStage::Interrupted | CreationStage::Failed(_)
                    )
            });
        self.chrome.task_creations = kept;
        for creation in closing {
            if let Some(record) = creation.proposal_row {
                self.close_proposal_row(&record, cx);
            }
        }
        cx.notify();
    }

    /// 起動時: 終わっていない行を中断した行（作れなかった行はやり直し）としてサイドバーに出す。続けはしない。
    /// 別の窓で動いている行・もう出している行は飛ばす。片付けた Task の行は閉じる。
    fn restore_proposal_rows(
        &mut self,
        rows: Vec<storage::OpenProposalRow>,
        cx: &mut Context<Self>,
    ) {
        let running = cx
            .try_global::<RunningProposalRows>()
            .map(|running| running.0.clone())
            .unwrap_or_default();
        for open in rows {
            let key = ProposalRowKey::of(&open.record);
            let shown = self.chrome.task_creations.iter().any(|creation| {
                creation
                    .proposal_row
                    .as_ref()
                    .map(ProposalRowKey::of)
                    .as_ref()
                    == Some(&key)
            });
            if shown || running.contains(&key) {
                continue;
            }
            if open.task_phase == Some(TaskPhase::Archived) {
                self.close_proposal_row(&open.record, cx);
                continue;
            }
            let task = serde_json::from_str::<Vec<ProposedTask>>(&open.proposal.tasks)
                .with_context(|| format!("分解案 {} の行を読めない", open.proposal.id))
                .and_then(|tasks| {
                    tasks.into_iter().nth(open.record.row).with_context(|| {
                        format!(
                            "分解案 {} に {} 行目が無い",
                            open.proposal.id,
                            open.record.row + 1
                        )
                    })
                });
            let task = match task {
                Ok(task) => task,
                Err(error) => {
                    eprintln!("{error:#}");
                    continue;
                }
            };
            let stage = resting_stage(&open.record);
            self.begin_proposal_row(
                &open.proposal.repository_id,
                &delegation_prompt(&task),
                proposal_fanout_task(&task),
                open.record,
                stage,
            );
        }
        cx.notify();
    }

    fn proposal_row_job(&self, id: u64) -> Option<RowJob> {
        let creation = self
            .chrome
            .task_creations
            .iter()
            .find(|creation| creation.id == id)?;
        Some(RowJob {
            record: creation.proposal_row.clone()?,
            prompt: creation.prompt.clone(),
            task: creation.task.clone(),
        })
    }

    /// 作成中の行の取り消しの印（`None` = 行が無い＝始める前にやめた）。
    fn proposal_row_cancelled(&self, id: u64) -> Option<bool> {
        self.chrome
            .task_creations
            .iter()
            .find(|creation| creation.id == id)
            .map(|creation| creation.cancelled)
    }

    fn slot_for_task_id(&self, task_id: &str) -> Option<usize> {
        self.project_sessions
            .projects
            .iter()
            .position(|slot| slot.task_space.id.as_str() == task_id)
    }

    /// 行の worktree の slot（レールに無ければ足す・あれば切り替えるだけ＝＋ Task と同じ）。
    fn proposal_row_slot(
        &mut self,
        worktree: Worktree,
        task_space: TaskSpace,
        branch: &str,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let root = worktree.root().to_path_buf();
        if let Some(index) = self
            .project_sessions
            .projects
            .iter()
            .position(|slot| slot.worktree.root() == root)
        {
            self.overlays.pending_project_switch = Some(index);
            cx.notify();
            return Some(index);
        }
        self.add_worktree_to_rail(worktree, task_space, Some(branch.to_string()), cx);
        self.project_sessions
            .projects
            .iter()
            .position(|slot| slot.worktree.root() == root)
    }

    fn ensure_task_cell(&mut self, space: &SpaceId) {
        let shown =
            self.chrome.fleet_cells.iter().any(
                |pane| matches!(pane, FleetPane::Task { space: existing } if existing == space),
            );
        if !shown {
            self.chrome.fleet_cells.push(FleetPane::Task {
                space: space.clone(),
            });
        }
    }

    /// 新しく作った行の Task を画面に載せる（題名・`⚑`・作った時の遷移とニュース）。台帳には書かない
    /// （呼び手が行の記録と 1 トランザクションで書く）。返すのは slot・台帳に書く snapshot・Task の host。
    fn register_proposal_row_task(
        &mut self,
        worktree: Worktree,
        task_space: TaskSpace,
        branch: &str,
        task: &FanoutTask,
        cx: &mut Context<Self>,
    ) -> Option<(usize, storage::TaskSpaceRecord, Arc<dyn host::Host>)> {
        let index = self.proposal_row_slot(worktree, task_space, branch, cx)?;
        let slot = self.project_sessions.projects.get_mut(index)?;
        if slot.task_space.is_integration() {
            return None;
        }
        let space = slot.task_space.id.clone();
        let host = slot.worktree.host().clone();
        // `:rocket:` は絵文字に（O20・A08・＋ Task と同じ）。
        slot.task_space.title = ui::emoji::expand_shortcodes(&task.slug_source).into();
        // 同じフォルダに前に居た Task の親子を引き継がない（O21・A07）。
        slot.task_space.parent = None;
        slot.task_space.captain_origin = true;
        // 日本語だけの題名（`task/task`）は、最初のスレッドの名前で改名する（O23・A23・＋ Task と同じ）。
        if let Some(auto_branch) = super::task_creation::auto_branch_plan(task, branch) {
            self.chrome.auto_branches.insert(space.clone(), auto_branch);
        }
        self.ensure_task_cell(&space);
        let record = self.apply_task_transition(index, TaskPhase::Planned, None, cx)?;
        Some((index, record, host))
    }

    /// 前の起動で台帳に載せてあった行の Task を開く（レールに無ければ足して台帳の中身を重ねる）。
    fn reopen_proposal_row_task(
        &mut self,
        worktree: Worktree,
        task_space: TaskSpace,
        branch: &str,
        stored: Option<storage::TaskSpaceRecord>,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let root = worktree.root().to_path_buf();
        let fresh = !self
            .project_sessions
            .projects
            .iter()
            .any(|slot| slot.worktree.root() == root);
        let index = self.proposal_row_slot(worktree, task_space, branch, cx)?;
        let slot = self.project_sessions.projects.get_mut(index)?;
        if fresh {
            if let Some(stored) = &stored {
                slot.task_space.overlay_stored(stored);
            }
        }
        slot.task_space.captain_origin = true;
        let space = slot.task_space.id.clone();
        self.ensure_task_cell(&space);
        cx.notify();
        Some(index)
    }

    /// 準備の失敗で Failed にしていた Task を戻す（準備をやり直して通った・「準備をやり直す」と同じ）。
    fn clear_setup_failure(&mut self, index: usize, cx: &mut Context<Self>) {
        let failed = self
            .project_sessions
            .projects
            .get(index)
            .is_some_and(|slot| slot.task_space.phase == TaskPhase::Failed);
        if failed {
            self.transition_task_space(index, TaskPhase::Planned, "setup_retried", None, cx);
        }
    }

    /// 担当のスレッドを起こす（空のスレッドがあれば使い回す＝やり直しても増えない）。
    fn acquire_proposal_row_thread(
        &mut self,
        index: usize,
        agent: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(panel) = self
            .project_sessions
            .sessions
            .get(index)
            .and_then(|session| session.fleet_agents.first())
            .cloned()
        else {
            return false;
        };
        panel.update(cx, |panel, cx| {
            panel.acquire_thread(agent, cx);
        });
        cx.notify();
        true
    }

    /// 委任文を送る。Task の会話に人の発話が既にあれば送らない（前の起動で届いていた・人が先に書いた＝
    /// 同じ指示を二度渡さない）。返り値 = 今送ったか。
    fn send_proposal_row_delegation(
        &mut self,
        index: usize,
        agent: Option<String>,
        prompt: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let delivered = self
            .project_sessions
            .sessions
            .get(index)
            .is_some_and(|session| {
                session
                    .fleet_agents
                    .iter()
                    .any(|panel| has_human_message(panel.read(cx)))
            });
        if delivered {
            return false;
        }
        self.ipc_spawn_into(index, agent, Some(prompt.to_string()), cx);
        true
    }

    /// 準備に失敗した行: Task は出したまま委任文を控え、「準備をやり直す」/「準備を飛ばして始める」を待つ
    /// （＋ Task と同じ・O20）。送ったら行は成功になる（[`Self::finish_held_proposal_row`]）。
    fn hold_proposal_row(
        &mut self,
        index: usize,
        record: ProposalRowRecord,
        prompt: &str,
        agent: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get(index) else {
            return;
        };
        let space = slot.task_space.id.clone();
        let reason = record.reason.clone().unwrap_or_default();
        self.chrome
            .pending_task_prompts
            .insert(space.clone(), prompt.to_string());
        if let Some(agent) = agent {
            self.chrome.pending_task_agents.insert(space.clone(), agent);
        }
        self.chrome.held_proposal_rows.insert(space, record);
        self.transition_task_space(index, TaskPhase::Failed, "setup_failed", Some(&reason), cx);
    }

    /// 行を「作れませんでした」にして、やり直しを出す（行を外していたら出し直す）。
    fn show_failed_proposal_row(
        &mut self,
        id: u64,
        repository_key: &str,
        job: &RowJob,
        record: ProposalRowRecord,
        error: &anyhow::Error,
        cx: &mut Context<Self>,
    ) {
        if let Some(creation) = self
            .chrome
            .task_creations
            .iter_mut()
            .find(|creation| creation.id == id)
        {
            creation.proposal_row = Some(record);
            self.fail_task_creation(id, error, cx);
            return;
        }
        let reason = format!("{error:#}");
        let first_line = reason.lines().next().unwrap_or("").trim().to_string();
        self.begin_proposal_row(
            repository_key,
            &job.prompt,
            job.task.clone(),
            record,
            CreationStage::Failed(first_line.into()),
        );
        let accent = self.accent();
        self.push_toast(SharedString::from(reason), accent, cx);
        cx.notify();
    }

    /// 開発用（`NECODER_FLEET_PROBE=interrupted`）: 前の起動で途中のまま終わった Captain の分解案の行を、段ごとに
    /// 仕込む（R09 の見た目の検証・DB には書かず worktree も作らない）。選んでいるリポジトリに出す。
    #[cfg(debug_assertions)]
    pub(crate) fn debug_seed_interrupted_rows(&mut self, cx: &mut Context<Self>) {
        let Some(repository_key) = self
            .active_slot()
            .map(|slot| slot.repository_key().to_string())
        else {
            return;
        };
        let rows = [
            (
                "rope に置き換える",
                ProposalRowStatus::Waiting,
                ProposalRowStep::Worktree,
                None,
                None,
            ),
            (
                "README の誤字",
                ProposalRowStatus::Creating,
                ProposalRowStep::Register,
                Some("task/readme"),
                None,
            ),
            (
                "Add divide",
                ProposalRowStatus::Creating,
                ProposalRowStep::Send,
                Some("task/add-divide"),
                None,
            ),
            (
                "依存を上げる",
                ProposalRowStatus::Failed,
                ProposalRowStep::Worktree,
                Some("task/deps"),
                Some("fatal: cannot lock ref 'refs/heads/task/deps': File exists"),
            ),
        ];
        let root = self
            .active_slot()
            .and_then(|slot| project::task_worktree_dir(slot.worktree.root()));
        for (row, (title, status, step, branch, reason)) in rows.into_iter().enumerate() {
            let task = ProposedTask {
                title: title.to_string(),
                goal: String::new(),
                done_when: String::new(),
                scope: None,
                agent: None,
            };
            let record = ProposalRowRecord {
                proposal_id: "proposal-probe".to_string(),
                row,
                status,
                step,
                branch: branch.map(str::to_string),
                target: branch.and_then(|branch| {
                    root.as_ref()
                        .map(|root| root.join(branch.trim_start_matches("task/")))
                }),
                task_id: None,
                reason: reason.map(str::to_string),
                updated_at: 0,
            };
            let stage = resting_stage(&record);
            self.begin_proposal_row(
                &repository_key,
                &delegation_prompt(&task),
                proposal_fanout_task(&task),
                record,
                stage,
            );
        }
        cx.notify();
    }

    /// 取り消された行（作っている最中の ×）: この回で作った worktree と今切ったブランチだけを消し、記録を閉じる。
    /// 前の起動で作ってあった worktree は消さない（外部の worktree として残る）。
    fn settle_cancelled_proposal_row(
        &mut self,
        id: u64,
        run: &RowRun,
        record: &ProposalRowRecord,
        prepared: Option<project::PlannedTaskWorktree>,
        cx: &mut Context<Self>,
    ) {
        self.remove_task_creation(id, cx);
        if let (Some(prepared), Some(branch)) = (
            prepared.filter(|prepared| prepared.created_worktree),
            record.branch.clone(),
        ) {
            self.discard_created_task(
                run.host.clone(),
                run.root.clone(),
                CreatedWorktree {
                    target: prepared.path,
                    branch,
                    new_branch: prepared.created_branch,
                    include_failure: None,
                    run_setup: false,
                },
                cx,
            );
        }
        self.close_proposal_row(record, cx);
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

/// 分解案の行を順に作る（1 回分）。`finished` = 終えた行の数（終えた行は動かしている印をすぐ外す）。
/// `Err` = 窓が閉じたか、落ちたことにして止めた（テスト）＝残りの行はこの回では作らない（記録はその時点のまま
/// 残るので、次の起動で中断した行として戻る）。
async fn drive_proposal_rows(
    workspace: &gpui::WeakEntity<Workspace>,
    run: &RowRun,
    rows: &[(u64, Option<ProposalRowKey>)],
    sync_base: bool,
    finished: &mut usize,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    if sync_base {
        // 土台（統合先の checked-out branch）を先に upstream へ早送りする（＋ Task と同じ）。
        if let Some((first, _)) = rows.first() {
            workspace.update(cx, |workspace, cx| {
                workspace.enter_creation_stage(*first, CreationStage::Syncing, cx)
            })?;
        }
        let sync = {
            let host = run.host.clone();
            let root = run.root.clone();
            cx.background_executor()
                .spawn(async move { project::sync_current_branch_on(host.as_ref(), &root) })
                .await
        };
        workspace.update(cx, |workspace, cx| workspace.report_base_sync(sync, cx))?;
    }
    for (id, key) in rows {
        drive_proposal_row(workspace, run, *id, cx).await?;
        // 終わった行はすぐに放す（作れなかった行を、この回の残りを待たずに「やり直す」で押せる）。
        if let Some(key) = key {
            cx.update(|cx| {
                cx.default_global::<RunningProposalRows>().0.remove(key);
            });
        }
        *finished += 1;
    }
    workspace.update(cx, |workspace, cx| {
        workspace.hint_fleet_mode_once(cx);
        cx.notify();
    })?;
    Ok(())
}

/// 1 行を記録した段から終わりまで進める（R09）。段の**前に**記録を書く: ブランチ名と worktree の場所は作る前、
/// Task の id は登録と同じトランザクション。やり直しは既にある物を使う（同じ行で 2 本作らない）。作れなかった段は
/// 行に理由を残し、その段からやり直せるようにする。
async fn drive_proposal_row(
    workspace: &gpui::WeakEntity<Workspace>,
    run: &RowRun,
    id: u64,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    // 行が無い = 始める前にやめた（記録は × が閉じた）。
    let Some(job) = workspace.update(cx, |workspace, _| workspace.proposal_row_job(id))? else {
        return Ok(());
    };
    let mut record = job.record.clone();
    let key = ProposalRowKey::of(&record);
    crash_point(workspace, &key, RowCheckpoint::Start, cx)?;
    // この回で用意した worktree（取り消した時に消すのは、この回で作った物だけ）。
    let mut prepared: Option<project::PlannedTaskWorktree> = None;
    loop {
        match record.step {
            ProposalRowStep::Worktree => {
                let entered = workspace.update(cx, |workspace, cx| {
                    workspace.enter_creation_stage(id, CreationStage::Worktree, cx)
                })?;
                if !entered {
                    return settle_cancelled(workspace, run, id, &record, prepared, cx);
                }
                if record.branch.is_none() || record.target.is_none() {
                    let planned = {
                        let host = run.host.clone();
                        let root = run.root.clone();
                        let title = job.task.slug_source.clone();
                        cx.background_executor()
                            .spawn(async move {
                                project::plan_task_target_on(host.as_ref(), &root, &title)
                            })
                            .await
                    };
                    match planned {
                        Ok((target, branch)) => {
                            record.target = Some(target);
                            record.branch = Some(branch);
                        }
                        Err(error) => {
                            return fail_row(workspace, run, id, &job, record, error, cx).await
                        }
                    }
                }
                record.status = ProposalRowStatus::Creating;
                record.reason = None;
                // 名前と場所を書いてから作る（書けなければ作らない＝落ちた後に 2 本目を作る余地を残さない）。
                if let Err(error) = save_row(run, &record, cx).await {
                    return fail_row(workspace, run, id, &job, record, error, cx).await;
                }
                crash_point(
                    workspace,
                    &key,
                    RowCheckpoint::Recorded(ProposalRowStep::Worktree),
                    cx,
                )?;
                match ensure_row_worktree(run, &record, cx).await {
                    Ok(worktree) => {
                        record.target = Some(worktree.path.clone());
                        prepared = Some(worktree);
                    }
                    Err(error) => {
                        return fail_row(workspace, run, id, &job, record, error, cx).await
                    }
                }
                crash_point(
                    workspace,
                    &key,
                    RowCheckpoint::Done(ProposalRowStep::Worktree),
                    cx,
                )?;
                record.step = ProposalRowStep::Setup;
                if let Err(error) = save_row(run, &record, cx).await {
                    return fail_row(workspace, run, id, &job, record, error, cx).await;
                }
            }
            ProposalRowStep::Setup => {
                if prepared.is_none() {
                    // 前の起動から続ける: worktree がまだあるか確かめる（無ければ決めた名前で作り直す）。
                    match ensure_row_worktree(run, &record, cx).await {
                        Ok(worktree) => prepared = Some(worktree),
                        Err(error) => {
                            record.step = ProposalRowStep::Worktree;
                            return fail_row(workspace, run, id, &job, record, error, cx).await;
                        }
                    }
                }
                let has_script = {
                    let host = run.host.clone();
                    let script = project::worktree_setup_script(&run.root);
                    cx.background_executor()
                        .spawn(async move { host.metadata(&script).is_ok() })
                        .await
                };
                let stage = if has_script {
                    CreationStage::Setup
                } else {
                    CreationStage::Worktree
                };
                let entered = workspace.update(cx, |workspace, cx| {
                    workspace.enter_creation_stage(id, stage, cx)
                })?;
                if !entered {
                    return settle_cancelled(workspace, run, id, &record, prepared, cx);
                }
                crash_point(
                    workspace,
                    &key,
                    RowCheckpoint::Recorded(ProposalRowStep::Setup),
                    cx,
                )?;
                // リンク・`.worktreeinclude`・準備スクリプト（どれも既にある物は置き直さない）。
                let failure = {
                    let host = run.host.clone();
                    let root = run.root.clone();
                    let target = record.target.clone().unwrap_or_default();
                    let branch = record.branch.clone().unwrap_or_default();
                    cx.background_executor()
                        .spawn(async move {
                            project::prepare_task_worktree_on(
                                host.as_ref(),
                                &root,
                                &target,
                                &branch,
                                true,
                            )
                            .err()
                            .map(|error| format!("{error:#}"))
                        })
                        .await
                };
                crash_point(
                    workspace,
                    &key,
                    RowCheckpoint::Done(ProposalRowStep::Setup),
                    cx,
                )?;
                if workspace.update(cx, |workspace, _| workspace.proposal_row_cancelled(id))?
                    == Some(true)
                {
                    return settle_cancelled(workspace, run, id, &record, prepared, cx);
                }
                // 準備の失敗は登録の後に委任文を控える（Task は出す・＋ Task と同じ）。
                record.step = ProposalRowStep::Register;
                record.reason = failure;
                if let Err(error) = save_row(run, &record, cx).await {
                    return fail_row(workspace, run, id, &job, record, error, cx).await;
                }
            }
            ProposalRowStep::Register => {
                match workspace.update(cx, |workspace, _| workspace.proposal_row_cancelled(id))? {
                    Some(true) => {
                        return settle_cancelled(workspace, run, id, &record, prepared, cx)
                    }
                    // 再開を押した後、記録を読み直している間にやめた。
                    None => return Ok(()),
                    Some(false) => {}
                }
                if prepared.is_none() {
                    match ensure_row_worktree(run, &record, cx).await {
                        Ok(worktree) => prepared = Some(worktree),
                        Err(error) => {
                            record.step = ProposalRowStep::Worktree;
                            return fail_row(workspace, run, id, &job, record, error, cx).await;
                        }
                    }
                }
                crash_point(
                    workspace,
                    &key,
                    RowCheckpoint::Recorded(ProposalRowStep::Register),
                    cx,
                )?;
                let Some(index) =
                    register_row_task(workspace, run, id, &job, &mut record, cx).await?
                else {
                    return Ok(());
                };
                crash_point(
                    workspace,
                    &key,
                    RowCheckpoint::Done(ProposalRowStep::Register),
                    cx,
                )?;
                // Task 行が出たので作成中の行は外す（＋ Task と同じ）。
                workspace.update(cx, |workspace, cx| workspace.remove_task_creation(id, cx))?;
                if record.status == ProposalRowStatus::Failed {
                    // 準備に失敗: 委任文は控えて「準備をやり直す」/「準備を飛ばして始める」を待つ。
                    let held = record.clone();
                    workspace.update(cx, |workspace, cx| {
                        workspace.hold_proposal_row(
                            index,
                            held,
                            &job.prompt,
                            job.task.agent.clone(),
                            cx,
                        )
                    })?;
                    return Ok(());
                }
            }
            ProposalRowStep::Spawn => {
                let Some(index) = open_row_task(workspace, run, id, &job, &record, cx).await?
                else {
                    return Ok(());
                };
                workspace.update(cx, |workspace, cx| workspace.remove_task_creation(id, cx))?;
                crash_point(
                    workspace,
                    &key,
                    RowCheckpoint::Recorded(ProposalRowStep::Spawn),
                    cx,
                )?;
                let agent = job.task.agent.clone();
                let acquired = workspace.update(cx, |workspace, cx| {
                    workspace.acquire_proposal_row_thread(index, agent, cx)
                })?;
                if !acquired {
                    let error = anyhow::anyhow!(i18n::t!("fleet.creating_proposal_no_panel"));
                    return fail_row(workspace, run, id, &job, record, error, cx).await;
                }
                crash_point(
                    workspace,
                    &key,
                    RowCheckpoint::Done(ProposalRowStep::Spawn),
                    cx,
                )?;
                record.step = ProposalRowStep::Send;
                if let Err(error) = save_row(run, &record, cx).await {
                    return fail_row(workspace, run, id, &job, record, error, cx).await;
                }
            }
            ProposalRowStep::Send => {
                let Some(index) = open_row_task(workspace, run, id, &job, &record, cx).await?
                else {
                    return Ok(());
                };
                workspace.update(cx, |workspace, cx| workspace.remove_task_creation(id, cx))?;
                crash_point(
                    workspace,
                    &key,
                    RowCheckpoint::Recorded(ProposalRowStep::Send),
                    cx,
                )?;
                let agent = job.task.agent.clone();
                workspace.update(cx, |workspace, cx| {
                    workspace.send_proposal_row_delegation(index, agent, &job.prompt, cx)
                })?;
                crash_point(
                    workspace,
                    &key,
                    RowCheckpoint::Done(ProposalRowStep::Send),
                    cx,
                )?;
                record.status = ProposalRowStatus::Succeeded;
                record.reason = None;
                if let Err(error) = save_row(run, &record, cx).await {
                    // 委任文は渡した。記録だけ書けなかった（次の起動では「委任文を送る途中」に見える）。
                    workspace.update(cx, |workspace, cx| {
                        let accent = workspace.accent();
                        workspace.push_toast(SharedString::from(format!("{error:#}")), accent, cx);
                    })?;
                }
                return Ok(());
            }
        }
    }
}

/// Task の登録（R09）: レールに開き、まだ台帳に載せていなければ、Task・作った時の遷移・`⚑` 帰属・行の記録を
/// **1 トランザクション**で書く。前の起動で載せてあれば（行に Task の id がある）、台帳には書かずに開くだけ
/// （出来事を二重に積まない）。`None` = 作れなかった（行に理由を出した）。
async fn register_row_task(
    workspace: &gpui::WeakEntity<Workspace>,
    run: &RowRun,
    id: u64,
    job: &RowJob,
    record: &mut ProposalRowRecord,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<Option<usize>> {
    let (worktree, task_space, branch) = match open_row_worktree(run, record, cx).await {
        Ok(opened) => opened,
        Err(error) => {
            fail_row(workspace, run, id, job, record.clone(), error, cx).await?;
            return Ok(None);
        }
    };
    let setup_failure = record.reason.clone();
    if record.task_id.is_none() {
        let registered = workspace.update(cx, |workspace, cx| {
            workspace.register_proposal_row_task(worktree, task_space, &branch, &job.task, cx)
        })?;
        let Some((index, mut task_record, task_host)) = registered else {
            let error = anyhow::anyhow!(i18n::t!("fleet.creating_proposal_not_task"));
            fail_row(workspace, run, id, job, record.clone(), error, cx).await?;
            return Ok(None);
        };
        // HEAD は手元なら画面に載せた時に取った。SSH 先はここで背景で取る。
        if task_host.is_remote() {
            let root = task_record.root.clone();
            task_record.head_oid = cx
                .background_executor()
                .spawn(async move { project::git_head_oid_on(task_host.as_ref(), &root) })
                .await;
        }
        let payload = Workspace::transition_payload(
            TaskPhase::Planned,
            "task_created",
            None,
            &task_record.head_oid,
        );
        let mut next = record.clone();
        next.task_id = Some(task_record.id.clone());
        if setup_failure.is_some() {
            next.status = ProposalRowStatus::Failed;
            next.step = ProposalRowStep::Setup;
        } else {
            next.status = ProposalRowStatus::Creating;
            next.step = ProposalRowStep::Spawn;
        }
        if let Some(storage) = run.storage.clone() {
            let attribution = serde_json::json!({
                "proposal": record.proposal_id,
                "row": record.row,
            })
            .to_string();
            let committed = {
                let next = next.clone();
                cx.background_executor()
                    .spawn(async move {
                        storage.commit_proposal_row_task(
                            &task_record,
                            &payload,
                            CAPTAIN_TASK_EVENT,
                            &attribution,
                            &next,
                        )
                    })
                    .await
            };
            // 台帳に載っていない: やり直しは準備から（同じ worktree を使って載せ直す）。
            if let Err(error) = committed {
                fail_row(workspace, run, id, job, record.clone(), error, cx).await?;
                return Ok(None);
            }
        }
        *record = next;
        return Ok(Some(index));
    }
    // 前の起動で台帳に載せてあった。
    let stored = load_stored_task(run, record, cx).await;
    let index = workspace.update(cx, |workspace, cx| {
        workspace.reopen_proposal_row_task(worktree, task_space, &branch, stored, cx)
    })?;
    let Some(index) = index else {
        let error = anyhow::anyhow!(i18n::t!("fleet.creating_proposal_not_task"));
        fail_row(workspace, run, id, job, record.clone(), error, cx).await?;
        return Ok(None);
    };
    if setup_failure.is_some() {
        record.status = ProposalRowStatus::Failed;
        record.step = ProposalRowStep::Setup;
    } else {
        workspace.update(cx, |workspace, cx| workspace.clear_setup_failure(index, cx))?;
        record.status = ProposalRowStatus::Creating;
        record.step = ProposalRowStep::Spawn;
    }
    if let Err(error) = save_row(run, record, cx).await {
        fail_row(workspace, run, id, job, record.clone(), error, cx).await?;
        return Ok(None);
    }
    Ok(Some(index))
}

/// 行の Task をレールで探す（無ければ worktree を確かめて開き、台帳の中身を重ねる＝前の起動から続ける時）。
/// `None` = 開けなかった（行に理由を出した）。
async fn open_row_task(
    workspace: &gpui::WeakEntity<Workspace>,
    run: &RowRun,
    id: u64,
    job: &RowJob,
    record: &ProposalRowRecord,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<Option<usize>> {
    let task_id = record.task_id.clone().unwrap_or_default();
    if let Some(index) =
        workspace.update(cx, |workspace, _| workspace.slot_for_task_id(&task_id))?
    {
        return Ok(Some(index));
    }
    if let Err(error) = ensure_row_worktree(run, record, cx).await {
        fail_row(workspace, run, id, job, record.clone(), error, cx).await?;
        return Ok(None);
    }
    let (worktree, task_space, branch) = match open_row_worktree(run, record, cx).await {
        Ok(opened) => opened,
        Err(error) => {
            fail_row(workspace, run, id, job, record.clone(), error, cx).await?;
            return Ok(None);
        }
    };
    let stored = load_stored_task(run, record, cx).await;
    let index = workspace.update(cx, |workspace, cx| {
        workspace.reopen_proposal_row_task(worktree, task_space, &branch, stored, cx)
    })?;
    if index.is_none() {
        let error = anyhow::anyhow!(i18n::t!("fleet.creating_proposal_not_task"));
        fail_row(workspace, run, id, job, record.clone(), error, cx).await?;
    }
    Ok(index)
}

/// 行の記録を書く（DB が無ければ何もしない＝テスト・撮影）。
async fn save_row(
    run: &RowRun,
    record: &ProposalRowRecord,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    let Some(storage) = run.storage.clone() else {
        return Ok(());
    };
    let record = record.clone();
    cx.background_executor()
        .spawn(async move { storage.save_proposal_row(&record) })
        .await
}

/// 決めてあった名前と場所の worktree を用意する（既にあれば使う・背景で）。ただし行がまだ Task を台帳に
/// 載せていないのに、前からあった worktree が**別の Task として台帳に載っていたら使わない**（中断の後に同じ
/// 名前で ＋ Task を作った・別の窓で取り込んだ物を、この行の Task にして委任文を送らない）。この行の登録は
/// 行の記録と 1 トランザクションなので、行に Task の id が無ければ、台帳の Task はこの行の物ではない。
async fn ensure_row_worktree(
    run: &RowRun,
    record: &ProposalRowRecord,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<project::PlannedTaskWorktree> {
    let target = record
        .target
        .clone()
        .context("worktree の場所が決まっていない")?;
    let branch = record
        .branch
        .clone()
        .context("ブランチ名が決まっていない")?;
    let host = run.host.clone();
    let root = run.root.clone();
    let ledger = run.storage.clone().filter(|_| record.task_id.is_none());
    cx.background_executor()
        .spawn(async move {
            let prepared =
                project::ensure_planned_task_worktree_on(host.as_ref(), &root, &target, &branch)?;
            if let (false, Some(storage)) = (prepared.created_worktree, ledger) {
                // Task の id は開いた worktree の正規化した場所から決まる（`SpaceId::for_worktree`）。
                let canonical = host
                    .canonicalize(&prepared.path)
                    .unwrap_or_else(|_| prepared.path.clone());
                let task_id = project::stable_worktree_id_on(host.as_ref(), &canonical);
                if let Some(other) = storage
                    .load_task_spaces()?
                    .into_iter()
                    .find(|task| task.id == task_id)
                {
                    anyhow::bail!(i18n::t!(
                        "fleet.creating_proposal_taken",
                        "title" => other.title,
                        "path" => prepared.path.display()
                    ));
                }
            }
            Ok(prepared)
        })
        .await
}

/// 行の worktree を開く（`.gitignore` を読む・git に聞く＝SSH 先では往復があるので背景で）。
async fn open_row_worktree(
    run: &RowRun,
    record: &ProposalRowRecord,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<(Worktree, TaskSpace, String)> {
    let target = record
        .target
        .clone()
        .context("worktree の場所が決まっていない")?;
    let branch = record
        .branch
        .clone()
        .context("ブランチ名が決まっていない")?;
    let host = run.host.clone();
    cx.background_executor()
        .spawn(async move {
            let worktree = Worktree::with_host(host, &target)?;
            let task_space = TaskSpace::for_worktree(&worktree, Some(&branch));
            Ok::<_, anyhow::Error>((worktree, task_space, branch))
        })
        .await
}

/// 前の起動で台帳に載せた行の Task の中身（題名・phase など）。読めなければ `None`（開いた worktree のまま）。
async fn load_stored_task(
    run: &RowRun,
    record: &ProposalRowRecord,
    cx: &mut gpui::AsyncApp,
) -> Option<storage::TaskSpaceRecord> {
    let storage = run.storage.clone()?;
    let task_id = record.task_id.clone()?;
    let stored = cx
        .background_executor()
        .spawn(async move { storage.load_task_spaces() })
        .await;
    match stored {
        Ok(records) => records.into_iter().find(|stored| stored.id == task_id),
        Err(error) => {
            eprintln!("Task の台帳を読めない: {error:#}");
            None
        }
    }
}

/// 行を「作れませんでした」にする（記録の段からやり直せる）。記録にも理由を書く（書けなければログだけ）。
/// 取り消しが押されていたら、失敗ではなくやめたことにする。
async fn fail_row(
    workspace: &gpui::WeakEntity<Workspace>,
    run: &RowRun,
    id: u64,
    job: &RowJob,
    mut record: ProposalRowRecord,
    error: anyhow::Error,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    if workspace.update(cx, |workspace, _| workspace.proposal_row_cancelled(id))? == Some(true) {
        return settle_cancelled(workspace, run, id, &record, None, cx);
    }
    // 登録の段の `reason` は準備の失敗を運ぶ欄。ここに失敗の理由を書くと準備の結果が分からなくなるので、
    // やり直しは準備から（準備は既にある物を置き直さない）。
    if record.step == ProposalRowStep::Register {
        record.step = ProposalRowStep::Setup;
    }
    record.status = ProposalRowStatus::Failed;
    record.reason = Some(format!("{error:#}"));
    if let Err(save_error) = save_row(run, &record, cx).await {
        eprintln!("分解案の行の失敗を記録できない: {save_error:#}");
    }
    workspace.update(cx, |workspace, cx| {
        workspace.show_failed_proposal_row(id, &run.repository_key, job, record, &error, cx)
    })?;
    Ok(())
}

fn settle_cancelled(
    workspace: &gpui::WeakEntity<Workspace>,
    run: &RowRun,
    id: u64,
    record: &ProposalRowRecord,
    prepared: Option<project::PlannedTaskWorktree>,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    workspace.update(cx, |workspace, cx| {
        workspace.settle_cancelled_proposal_row(id, run, record, prepared, cx)
    })
}

/// テスト: 決めた区切りで necoder が落ちたことにして、この回を止める（記録と git はその時点のまま・R09）。
#[cfg(test)]
fn crash_point(
    workspace: &gpui::WeakEntity<Workspace>,
    key: &ProposalRowKey,
    checkpoint: RowCheckpoint,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    let crashed = workspace.update(cx, |workspace, _| {
        workspace.chrome.proposal_row_crash.as_ref() == Some(&(key.clone(), checkpoint))
    })?;
    anyhow::ensure!(
        !crashed,
        "テスト: {} の {} 行目を {checkpoint:?} で止めた",
        key.proposal_id,
        key.row + 1
    );
    Ok(())
}

#[cfg(not(test))]
fn crash_point(
    _workspace: &gpui::WeakEntity<Workspace>,
    _key: &ProposalRowKey,
    _checkpoint: RowCheckpoint,
    _cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    Ok(())
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

    // ── 承認した行の実行記録と再開（UX-CODE-REVIEW R09 の条件 1・2） ──

    /// 使い捨ての repo（main に 1 コミット・正規化した綴り）と設定と DB。設定はエージェントを起こせない
    /// （存在しないコマンド＝本物のエージェントに送らない）。git が無ければ None。
    struct Scratch {
        base: PathBuf,
        repo: PathBuf,
        settings: PathBuf,
        storage: storage::Storage,
    }

    fn git_in(dir: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .output()
            .expect("git を起動できる")
    }

    fn scratch(label: &str) -> Option<Scratch> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let base = std::env::temp_dir().join(format!(
            "necoder_resume_{label}_{}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(base.join("repo")).expect("作業フォルダを作れる");
        let base = paths::canonicalize(&base).expect("正規化できる");
        let repo = base.join("repo");
        if !git_in(&repo, &["init", "-q", "-b", "main"])
            .status
            .success()
        {
            return None;
        }
        std::fs::write(repo.join("calc.py"), "def add(a, b):\n    return a + b\n").expect("書ける");
        git_in(&repo, &["add", "-A"]);
        git_in(&repo, &["commit", "-qm", "initial"]);
        let settings = base.join("settings.json");
        std::fs::write(
            &settings,
            r#"{"onboarded":true,"agent_prewarm":false,"captain_agent":"Claude Code",
                "tier2_summaries":false,"agent_auto_name":false,
                "agent_servers":{"claude":{"type":"custom","command":"/nonexistent/necoder-test-agent"}}}"#,
        )
        .expect("設定を書ける");
        let storage = storage::Storage::open(&base.join("necoder.db")).expect("DB を開ける");
        Some(Scratch {
            base,
            repo,
            settings,
            storage,
        })
    }

    fn settle(cx: &mut gpui::VisualTestContext) {
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(3));
        cx.run_until_parked();
    }

    /// 窓を開く（起動と同じ: 台帳・分解案・中断した行を DB から戻す）。統合先のファイルの見張りは止める。
    fn open_window<'a>(
        cx: &'a mut gpui::TestAppContext,
        scratch: &Scratch,
        window_id: &str,
    ) -> (Entity<Workspace>, &'a mut gpui::VisualTestContext) {
        let persistence = WindowPersistence {
            storage: Some(scratch.storage.clone()),
            window_id: Some(window_id.to_string()),
        };
        let repo = scratch.repo.clone();
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![repo], Theme::dark(), Some(persistence), cx)
        });
        settle(cx);
        workspace.update(cx, |workspace, _| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        (workspace, cx)
    }

    fn proposed(title: &str) -> ProposedTask {
        ProposedTask {
            title: title.to_string(),
            goal: format!("{title} をやる"),
            done_when: "テストが通る".to_string(),
            scope: None,
            agent: None,
        }
    }

    /// Captain の分解案を DB と要対応に置く（`fleet_propose_tasks` と同じ形）。返すのは案の id。
    fn propose(
        workspace: &Entity<Workspace>,
        cx: &mut gpui::VisualTestContext,
        scratch: &Scratch,
        titles: &[&str],
    ) -> String {
        let repository = workspace.update(cx, |workspace, _| {
            workspace.project_sessions.projects[0]
                .repository_key()
                .to_string()
        });
        let tasks: Vec<ProposedTask> = titles.iter().map(|title| proposed(title)).collect();
        let record = storage::CaptainProposalRecord {
            id: new_proposal_id(),
            repository_id: repository,
            root: scratch.repo.clone(),
            note: None,
            tasks: serde_json::to_string(&tasks).expect("JSON にできる"),
            status: storage::ProposalStatus::Pending,
            outcome: None,
            created_at: 1,
            resolved_at: None,
        };
        scratch
            .storage
            .insert_captain_proposal(&record)
            .expect("案を置ける");
        let proposal_id = record.id.clone();
        workspace.update(cx, |workspace, cx| {
            workspace
                .add_captain_proposal(CaptainProposal::from_record(record).expect("読める"), cx)
        });
        proposal_id
    }

    /// `row` 行目が `checkpoint` に来たら落ちたことにして、案を承認する。
    fn approve_crashing_at(
        workspace: &Entity<Workspace>,
        cx: &mut gpui::VisualTestContext,
        proposal_id: &str,
        crash: Option<(usize, RowCheckpoint)>,
    ) {
        workspace.update(cx, |workspace, cx| {
            workspace.chrome.proposal_row_crash = crash.map(|(row, checkpoint)| {
                (
                    ProposalRowKey {
                        proposal_id: proposal_id.to_string(),
                        row,
                    },
                    checkpoint,
                )
            });
            workspace.approve_captain_proposal(proposal_id, cx);
        });
        settle(cx);
    }

    fn task_branches(repo: &Path) -> Vec<String> {
        project::git_branches(repo)
            .into_iter()
            .filter(|branch| branch.starts_with("task/"))
            .collect()
    }

    /// 中断した行・作れなかった行（分解案の行だけ）: `(行の番号, 段, 作成中の行の id)`。
    fn proposal_rows_shown(
        workspace: &Entity<Workspace>,
        cx: &mut gpui::VisualTestContext,
    ) -> Vec<(usize, CreationStage, u64)> {
        workspace.update(cx, |workspace, _| {
            workspace
                .chrome
                .task_creations
                .iter()
                .filter_map(|creation| {
                    creation
                        .proposal_row
                        .as_ref()
                        .map(|record| (record.row, creation.stage.clone(), creation.id))
                })
                .collect()
        })
    }

    /// Task の台帳の出来事: `(⚑ 帰属の数, 作った出来事の数)`。
    fn ledger_counts(storage: &storage::Storage, task_id: &str) -> (usize, usize) {
        let events = storage.load_task_events(task_id).expect("台帳を読める");
        let attributions = events
            .iter()
            .filter(|event| event.kind == CAPTAIN_TASK_EVENT)
            .count();
        let created = events
            .iter()
            .filter(|event| {
                event.kind == "phase_changed" && event.payload.contains("\"task_created\"")
            })
            .count();
        (attributions, created)
    }

    /// Task の会話に届いた委任文の数（どのスレッドでも・人の発話として）。委任文の 1 行目は題名なので、題名で
    /// 見分ける（表示言語はテストの並走で入れ替わりうる＝本文の比較に頼らない）。
    fn delegations_delivered(
        workspace: &Entity<Workspace>,
        cx: &mut gpui::VisualTestContext,
        task_id: &str,
        title: &str,
    ) -> usize {
        let first_line = format!("{title}\n");
        workspace.update(cx, |workspace, cx| {
            let Some(index) = workspace.slot_for_task_id(task_id) else {
                return 0;
            };
            workspace.project_sessions.sessions[index]
                .fleet_agents
                .iter()
                .map(|panel| {
                    let panel = panel.read(cx);
                    (0..)
                        .map_while(|thread| panel.thread_id(thread).map(|_| thread))
                        .flat_map(|thread| panel.transcript_lines(thread, usize::MAX))
                        .filter(|(bullet, text)| {
                            bullet.as_ref() == "▸" && text.starts_with(&first_line)
                        })
                        .count()
                })
                .sum()
        })
    }

    /// R09 の確かめ方: 1 行の案を承認し、`checkpoint` で necoder が落ちたことにする → 同じ DB と repo で窓を
    /// 開き直す（再起動）→ 行が「中断: `label`」で出て、起動しただけでは何も作らない → 再開すると、ブランチ・
    /// worktree・Task・台帳の出来事（作った・⚑）・担当へ渡した委任文が、どれも 1 つだけになる。
    fn crash_then_resume(
        cx: &mut gpui::TestAppContext,
        checkpoint: RowCheckpoint,
        step_key: &'static str,
    ) {
        let Some(scratch) = scratch(&format!("{checkpoint:?}").replace(['(', ')'], "_")) else {
            return;
        };
        cx.update(|cx| settings::init(Some(scratch.settings.clone()), None, cx));
        let (first, cx) = open_window(cx, &scratch, "before");
        let proposal_id = propose(&first, cx, &scratch, &["Fix the parser"]);
        approve_crashing_at(&first, cx, &proposal_id, Some((0, checkpoint)));
        let crashed = scratch
            .storage
            .load_proposal_row(&proposal_id, 0)
            .expect("読める")
            .expect("承認と同じトランザクションで行が置かれる");
        assert!(
            crashed.status.is_open(),
            "{checkpoint:?}: 落ちた行は終わっていない: {crashed:?}"
        );
        let branches_at_crash = task_branches(&scratch.repo);
        let worktrees_at_crash = project::git_worktrees(&scratch.repo).len();

        // 再起動（同じ DB・同じ repo の新しい窓）。
        let (second, cx) = open_window(cx, &scratch, "after");
        let shown = proposal_rows_shown(&second, cx);
        assert_eq!(shown.len(), 1, "{checkpoint:?}: 中断した行が出る");
        let (_, stage, id) = shown[0].clone();
        assert_eq!(stage, CreationStage::Interrupted, "{checkpoint:?}");
        let record = second.update(cx, |workspace, _| {
            workspace.chrome.task_creations[0]
                .proposal_row
                .clone()
                .expect("記録の写し")
        });
        assert_eq!(interrupted_step_key(&record), step_key, "{checkpoint:?}");
        assert_eq!(
            task_branches(&scratch.repo),
            branches_at_crash,
            "{checkpoint:?}: 起動しただけでは何も作らない"
        );
        assert_eq!(
            project::git_worktrees(&scratch.repo).len(),
            worktrees_at_crash
        );
        assert!(
            second.update(cx, |workspace, _| workspace
                .project_sessions
                .projects
                .iter()
                .all(|slot| slot.task_space.is_integration())),
            "{checkpoint:?}: 起動しただけでは Task を開かない（担当を起こさない）"
        );

        second.update(cx, |workspace, cx| workspace.retry_task_creation(id, cx));
        settle(cx);

        let done = scratch
            .storage
            .load_proposal_row(&proposal_id, 0)
            .expect("読める")
            .expect("行がある");
        assert_eq!(
            done.status,
            ProposalRowStatus::Succeeded,
            "{checkpoint:?}: {done:?}"
        );
        let task_id = done.task_id.clone().expect("Task の id が残る");
        assert_eq!(
            task_branches(&scratch.repo),
            vec!["task/fix-the-parser".to_string()],
            "{checkpoint:?}: ブランチは 1 本（`-2` を作らない）"
        );
        assert_eq!(
            project::git_worktrees(&scratch.repo).len(),
            2,
            "{checkpoint:?}: worktree は 1 本（+ 統合先）"
        );
        assert_eq!(
            ledger_counts(&scratch.storage, &task_id),
            (1, 1),
            "{checkpoint:?}: ⚑ 帰属と作った出来事は 1 回ずつ"
        );
        assert_eq!(
            delegations_delivered(&second, cx, &task_id, "Fix the parser"),
            1,
            "{checkpoint:?}: 委任文は担当に 1 回"
        );
        second.update(cx, |workspace, _| {
            assert!(
                proposal_rows_shown_in(workspace).is_empty(),
                "{checkpoint:?}: 行は消える"
            );
            let index = workspace
                .slot_for_task_id(&task_id)
                .expect("Task がレールに開く");
            assert!(
                workspace.project_sessions.projects[index]
                    .task_space
                    .captain_origin,
                "{checkpoint:?}: ⚑"
            );
        });
        assert!(
            scratch
                .storage
                .load_open_proposal_rows()
                .expect("読める")
                .is_empty(),
            "{checkpoint:?}: 次の起動では何も出ない"
        );
        std::fs::remove_dir_all(&scratch.base).ok();
    }

    fn proposal_rows_shown_in(workspace: &Workspace) -> Vec<usize> {
        workspace
            .chrome
            .task_creations
            .iter()
            .filter_map(|creation| creation.proposal_row.as_ref().map(|record| record.row))
            .collect()
    }

    /// 承認を DB に確定した直後（まだ何も作っていない）。
    #[gpui::test]
    fn a_row_cut_off_right_after_approval_resumes_once(cx: &mut gpui::TestAppContext) {
        crash_then_resume(cx, RowCheckpoint::Start, "fleet.creating_step_waiting");
    }

    /// 名前と場所を記録した直後（worktree はまだ）。再開は記録した名前で作る。
    #[gpui::test]
    fn a_row_cut_off_before_its_worktree_resumes_once(cx: &mut gpui::TestAppContext) {
        crash_then_resume(
            cx,
            RowCheckpoint::Recorded(ProposalRowStep::Worktree),
            "fleet.creating_step_worktree",
        );
    }

    /// worktree を作った直後（記録の前）。再開は作り直さずにそれを使う。
    #[gpui::test]
    fn a_row_cut_off_right_after_its_worktree_resumes_once(cx: &mut gpui::TestAppContext) {
        crash_then_resume(
            cx,
            RowCheckpoint::Done(ProposalRowStep::Worktree),
            "fleet.creating_step_worktree",
        );
    }

    /// 準備を流し終えた直後（記録の前）。再開は準備をもう一度流す（既にある物は置き直さない）。
    #[gpui::test]
    fn a_row_cut_off_during_setup_resumes_once(cx: &mut gpui::TestAppContext) {
        crash_then_resume(
            cx,
            RowCheckpoint::Done(ProposalRowStep::Setup),
            "fleet.creating_step_setup",
        );
    }

    /// 台帳に載せる直前。
    #[gpui::test]
    fn a_row_cut_off_before_registration_resumes_once(cx: &mut gpui::TestAppContext) {
        crash_then_resume(
            cx,
            RowCheckpoint::Recorded(ProposalRowStep::Register),
            "fleet.creating_step_register",
        );
    }

    /// 台帳に載せた直後（Task・出来事・行は 1 トランザクション）。再開は載せ直さない。
    #[gpui::test]
    fn a_row_cut_off_right_after_registration_resumes_once(cx: &mut gpui::TestAppContext) {
        crash_then_resume(
            cx,
            RowCheckpoint::Done(ProposalRowStep::Register),
            "fleet.creating_step_spawn",
        );
    }

    /// 担当を起こす直前。
    #[gpui::test]
    fn a_row_cut_off_right_before_the_agent_starts_resumes_once(cx: &mut gpui::TestAppContext) {
        crash_then_resume(
            cx,
            RowCheckpoint::Recorded(ProposalRowStep::Spawn),
            "fleet.creating_step_spawn",
        );
    }

    /// 担当のスレッドを起こした直後（記録の前）。再開は空のスレッドを使い回す。
    #[gpui::test]
    fn a_row_cut_off_after_the_agent_started_resumes_once(cx: &mut gpui::TestAppContext) {
        crash_then_resume(
            cx,
            RowCheckpoint::Done(ProposalRowStep::Spawn),
            "fleet.creating_step_spawn",
        );
    }

    /// 委任文を送る直前。
    #[gpui::test]
    fn a_row_cut_off_before_the_instructions_resumes_once(cx: &mut gpui::TestAppContext) {
        crash_then_resume(
            cx,
            RowCheckpoint::Recorded(ProposalRowStep::Send),
            "fleet.creating_step_send",
        );
    }

    /// 委任文を渡した直後（成功を書く前）。前の起動の会話に残っていなければ、もう一度送る（届いたかは分からない）。
    #[gpui::test]
    fn a_row_cut_off_right_after_the_instructions_resumes_once(cx: &mut gpui::TestAppContext) {
        crash_then_resume(
            cx,
            RowCheckpoint::Done(ProposalRowStep::Send),
            "fleet.creating_step_send",
        );
    }

    /// 途中で落ちずに終われば、行は成功（Task の id）で閉じ、次の起動では何も出ない。印を外した行は記録しない。
    #[gpui::test]
    fn an_uninterrupted_approval_closes_every_row(cx: &mut gpui::TestAppContext) {
        let Some(scratch) = scratch("uninterrupted") else {
            return;
        };
        cx.update(|cx| settings::init(Some(scratch.settings.clone()), None, cx));
        let (workspace, cx) = open_window(cx, &scratch, "only");
        let proposal_id = propose(&workspace, cx, &scratch, &["Rope buffer", "Write README"]);
        workspace.update(cx, |workspace, cx| {
            workspace.toggle_proposal_row(&proposal_id, 1, cx)
        });
        approve_crashing_at(&workspace, cx, &proposal_id, None);
        let rows = scratch
            .storage
            .load_proposal_rows(&proposal_id)
            .expect("読める");
        assert_eq!(rows.len(), 1, "印を外した行は記録しない: {rows:?}");
        assert_eq!(rows[0].status, ProposalRowStatus::Succeeded);
        assert_eq!(rows[0].branch.as_deref(), Some("task/rope-buffer"));
        let task_id = rows[0].task_id.clone().expect("Task の id");
        assert_eq!(ledger_counts(&scratch.storage, &task_id), (1, 1));
        assert_eq!(
            delegations_delivered(&workspace, cx, &task_id, "Rope buffer"),
            1
        );
        // 次の起動（窓を開き直す）でも、終わった行は出ない。
        let (next, cx) = open_window(cx, &scratch, "next");
        assert!(proposal_rows_shown(&next, cx).is_empty());
        assert!(scratch
            .storage
            .load_open_proposal_rows()
            .expect("読める")
            .is_empty());
        std::fs::remove_dir_all(&scratch.base).ok();
    }

    /// R09（3 行中 1 行だけ失敗・途中で終了）: 1 行目は作れ、2 行目は worktree で失敗し、3 行目は worktree を
    /// 作った直後に落ちる → 再起動すると 2 行目は「作れませんでした + やり直す」、3 行目は「中断 + 再開」で出て、
    /// 1 行目は出ない → 直してやり直す・再開すると、どちらも作れ、1 行目は作り直さない（ブランチ・worktree・⚑
    /// はそれぞれ 1 つ）。
    #[gpui::test]
    fn a_failed_row_and_a_row_cut_off_mid_way_resume_without_redoing_the_finished_one(
        cx: &mut gpui::TestAppContext,
    ) {
        let Some(scratch) = scratch("three_rows") else {
            return;
        };
        cx.update(|cx| settings::init(Some(scratch.settings.clone()), None, cx));
        let (first, cx) = open_window(cx, &scratch, "before");
        // 2 行目のブランチの ref を lock しておく（`git worktree add -b` が断る＝worktree の段で失敗する）。
        let refs = scratch
            .repo
            .join(".git")
            .join("refs")
            .join("heads")
            .join("task");
        std::fs::create_dir_all(&refs).expect("作れる");
        let lock = refs.join("write-readme.lock");
        std::fs::write(&lock, "").expect("書ける");
        let proposal_id = propose(
            &first,
            cx,
            &scratch,
            &["Rope buffer", "Write README", "Add divide"],
        );
        approve_crashing_at(
            &first,
            cx,
            &proposal_id,
            Some((2, RowCheckpoint::Done(ProposalRowStep::Worktree))),
        );
        let rows = scratch
            .storage
            .load_proposal_rows(&proposal_id)
            .expect("読める");
        assert_eq!(
            rows.iter().map(|row| row.status).collect::<Vec<_>>(),
            vec![
                ProposalRowStatus::Succeeded,
                ProposalRowStatus::Failed,
                ProposalRowStatus::Creating
            ],
            "{rows:?}"
        );
        assert_eq!(rows[1].step, ProposalRowStep::Worktree);
        assert!(
            rows[1]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("lock")),
            "{:?}",
            rows[1].reason
        );
        let finished_task = rows[0].task_id.clone().expect("1 行目の Task");
        assert_eq!(
            task_branches(&scratch.repo),
            vec![
                "task/add-divide".to_string(),
                "task/rope-buffer".to_string()
            ],
            "3 行目は worktree まで作ってから落ちた"
        );

        // 再起動。
        let (second, cx) = open_window(cx, &scratch, "after");
        let shown = proposal_rows_shown(&second, cx);
        assert_eq!(shown.len(), 2, "1 行目（作り終えた）は出ない: {shown:?}");
        assert_eq!(shown[0].0, 1);
        assert!(
            matches!(&shown[0].1, CreationStage::Failed(reason) if !reason.is_empty()),
            "作れなかった行は理由の 1 行目つき: {:?}",
            shown[0].1
        );
        assert_eq!(
            (shown[1].0, shown[1].1.clone()),
            (2, CreationStage::Interrupted)
        );

        // 直して（lock を外して）2 行目をやり直し、3 行目を再開する。
        std::fs::remove_file(&lock).expect("外せる");
        second.update(cx, |workspace, cx| {
            workspace.retry_task_creation(shown[0].2, cx)
        });
        settle(cx);
        second.update(cx, |workspace, cx| {
            workspace.retry_task_creation(shown[1].2, cx)
        });
        settle(cx);

        let rows = scratch
            .storage
            .load_proposal_rows(&proposal_id)
            .expect("読める");
        assert!(
            rows.iter()
                .all(|row| row.status == ProposalRowStatus::Succeeded),
            "{rows:?}"
        );
        assert_eq!(
            rows[0].task_id.as_deref(),
            Some(finished_task.as_str()),
            "1 行目は作り直さない"
        );
        assert_eq!(
            task_branches(&scratch.repo),
            vec![
                "task/add-divide".to_string(),
                "task/rope-buffer".to_string(),
                "task/write-readme".to_string()
            ],
            "どの行もブランチは 1 本"
        );
        assert_eq!(
            project::git_worktrees(&scratch.repo).len(),
            4,
            "統合先 + 3 本"
        );
        for row in &rows {
            let task_id = row.task_id.as_deref().expect("Task の id");
            assert_eq!(
                ledger_counts(&scratch.storage, task_id),
                (1, 1),
                "{} 行目: ⚑ と作った出来事は 1 回ずつ",
                row.row + 1
            );
        }
        assert!(proposal_rows_shown(&second, cx).is_empty());
        std::fs::remove_dir_all(&scratch.base).ok();
    }

    /// やめる: 中断した行の × は記録を閉じ、次に起動しても出さない。作ってあった worktree とブランチは消さない
    /// （外部の worktree として残る）。
    #[gpui::test]
    fn dropping_an_interrupted_row_closes_it_and_keeps_the_worktree(cx: &mut gpui::TestAppContext) {
        let Some(scratch) = scratch("drop") else {
            return;
        };
        cx.update(|cx| settings::init(Some(scratch.settings.clone()), None, cx));
        let (first, cx) = open_window(cx, &scratch, "before");
        let proposal_id = propose(&first, cx, &scratch, &["Fix the parser"]);
        approve_crashing_at(
            &first,
            cx,
            &proposal_id,
            Some((0, RowCheckpoint::Done(ProposalRowStep::Worktree))),
        );
        let (second, cx) = open_window(cx, &scratch, "after");
        let shown = proposal_rows_shown(&second, cx);
        assert_eq!(shown.len(), 1);
        second.update(cx, |workspace, cx| workspace.refresh_fleet_worktrees(cx));
        settle(cx);
        assert!(
            second.update(cx, |workspace, _| workspace.external_worktrees().is_empty()),
            "中断した行の worktree は、行が引き受ける（取り込むに出さない）"
        );
        second.update(cx, |workspace, cx| {
            workspace.cancel_task_creation(shown[0].2, cx)
        });
        settle(cx);
        assert!(proposal_rows_shown(&second, cx).is_empty());
        second.update(cx, |workspace, cx| {
            workspace.forget_fleet_worktrees();
            workspace.refresh_fleet_worktrees(cx);
        });
        settle(cx);
        assert_eq!(
            second.update(cx, |workspace, _| workspace
                .external_worktrees()
                .into_iter()
                .map(|(_, branch, _)| branch)
                .collect::<Vec<_>>()),
            vec![Some("task/fix-the-parser".to_string())],
            "やめた行の worktree は外部の worktree として出る（取り込む・片付けられる）"
        );
        assert_eq!(
            scratch
                .storage
                .load_proposal_row(&proposal_id, 0)
                .expect("読める")
                .map(|row| row.status),
            Some(ProposalRowStatus::Discarded)
        );
        assert_eq!(
            task_branches(&scratch.repo),
            vec!["task/fix-the-parser".to_string()],
            "作ってあったブランチは残す"
        );
        assert_eq!(
            project::git_worktrees(&scratch.repo).len(),
            2,
            "worktree も残す"
        );
        let (third, cx) = open_window(cx, &scratch, "again");
        assert!(
            proposal_rows_shown(&third, cx).is_empty(),
            "やめた行は次の起動で出さない"
        );
        std::fs::remove_dir_all(&scratch.base).ok();
    }

    /// 準備に失敗した行（O20 と同じ）: Task は出して委任文を控え、行は「準備で失敗」（Task の id つき）で残る。
    /// 「準備を飛ばして始める」で送れば行は成功。送らずに終了したら、再起動の後に「やり直す」で準備から続け、
    /// 通れば同じ Task へ委任文を送る（Task を作り直さない・作った出来事は 1 回）。
    #[gpui::test]
    fn a_row_whose_setup_failed_is_held_and_resumes_from_setup(cx: &mut gpui::TestAppContext) {
        let Some(scratch) = scratch("setup_failed") else {
            return;
        };
        cx.update(|cx| settings::init(Some(scratch.settings.clone()), None, cx));
        let script = project::worktree_setup_script(&scratch.repo);
        std::fs::create_dir_all(script.parent().expect("置き場")).expect("作れる");
        std::fs::write(&script, "#!/bin/sh\necho broken >&2\nexit 1\n").expect("書ける");
        let (first, cx) = open_window(cx, &scratch, "before");
        let proposal_id = propose(&first, cx, &scratch, &["Fix the parser", "Write README"]);
        approve_crashing_at(&first, cx, &proposal_id, None);
        let rows = scratch
            .storage
            .load_proposal_rows(&proposal_id)
            .expect("読める");
        for row in &rows {
            assert_eq!(
                (row.status, row.step),
                (ProposalRowStatus::Failed, ProposalRowStep::Setup),
                "{row:?}"
            );
            assert!(row.task_id.is_some(), "Task は出した");
        }
        let held_task = rows[0].task_id.clone().expect("Task の id");
        let later_task = rows[1].task_id.clone().expect("Task の id");
        let index = first.update(cx, |workspace, _| {
            assert_eq!(workspace.chrome.held_proposal_rows.len(), 2);
            assert!(
                proposal_rows_shown_in(workspace).is_empty(),
                "Task のカードが引き受ける"
            );
            workspace.slot_for_task_id(&held_task).expect("Task がある")
        });
        // 1 行目: 「準備を飛ばして始める」で控えた委任文を送る → 行は成功。
        first.update(cx, |workspace, cx| {
            workspace.start_task_without_setup(index, cx)
        });
        settle(cx);
        assert_eq!(
            scratch
                .storage
                .load_proposal_row(&proposal_id, 0)
                .expect("読める")
                .map(|row| row.status),
            Some(ProposalRowStatus::Succeeded)
        );

        // 2 行目の「直して通す」は準備スクリプトが走る必要がある。手元の Windows には POSIX shell が無く
        // （`LocalHost::has_posix_shell`）準備はいつも失敗するので、ここまで（1 行目を飛ばして始める）で止める。
        if !host::LocalHost::shared().has_posix_shell() {
            std::fs::remove_dir_all(&scratch.base).ok();
            return;
        }

        // 2 行目: 送らずに終了 → 準備を直して再起動 → やり直す。
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").expect("書ける");
        let (second, cx) = open_window(cx, &scratch, "after");
        let shown = proposal_rows_shown(&second, cx);
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert_eq!(shown[0].0, 1);
        assert!(
            matches!(shown[0].1, CreationStage::Failed(_)),
            "{:?}",
            shown[0].1
        );
        second.update(cx, |workspace, cx| {
            workspace.retry_task_creation(shown[0].2, cx)
        });
        settle(cx);
        let row = scratch
            .storage
            .load_proposal_row(&proposal_id, 1)
            .expect("読める")
            .expect("行がある");
        assert_eq!(row.status, ProposalRowStatus::Succeeded, "{row:?}");
        assert_eq!(
            row.task_id.as_deref(),
            Some(later_task.as_str()),
            "同じ Task"
        );
        assert_eq!(ledger_counts(&scratch.storage, &later_task), (1, 1));
        assert_eq!(
            delegations_delivered(&second, cx, &later_task, "Write README"),
            1
        );
        second.update(cx, |workspace, _| {
            let index = workspace
                .slot_for_task_id(&later_task)
                .expect("Task を開いた");
            assert_ne!(
                workspace.project_sessions.projects[index].task_space.phase,
                TaskPhase::Failed,
                "準備が通ったので失敗から戻る"
            );
        });
        assert_eq!(task_branches(&scratch.repo).len(), 2, "Task を作り直さない");
        std::fs::remove_dir_all(&scratch.base).ok();
    }

    /// 中断の後、再開を押す前に、行の worktree が別の Task として台帳に載った（同じ名前の ＋ Task・別の窓での
    /// 取り込み）: 再開はそれをこの行の Task にしない（委任文も `⚑` も付けない）。行は「作れませんでした」で
    /// 残り、やめるか直すかは人が決める。
    #[gpui::test]
    fn a_worktree_another_task_took_over_is_not_reused(cx: &mut gpui::TestAppContext) {
        let Some(scratch) = scratch("taken") else {
            return;
        };
        cx.update(|cx| settings::init(Some(scratch.settings.clone()), None, cx));
        let (first, cx) = open_window(cx, &scratch, "before");
        let proposal_id = propose(&first, cx, &scratch, &["Fix the parser"]);
        approve_crashing_at(
            &first,
            cx,
            &proposal_id,
            Some((0, RowCheckpoint::Done(ProposalRowStep::Worktree))),
        );
        let crashed = scratch
            .storage
            .load_proposal_row(&proposal_id, 0)
            .expect("読める")
            .expect("行がある");
        let target = crashed.target.clone().expect("場所は記録してある");
        let host = host::LocalHost::shared();
        let other_id = project::stable_worktree_id_on(
            host.as_ref(),
            &paths::canonicalize(&target).expect("worktree はある"),
        );
        let repository = first.update(cx, |workspace, _| {
            workspace.project_sessions.projects[0]
                .repository_key()
                .to_string()
        });
        scratch
            .storage
            .upsert_task_space(&storage::TaskSpaceRecord {
                id: other_id.clone(),
                repository_id: repository,
                root: target.clone(),
                branch: Some("task/fix-the-parser".into()),
                title: "手で取り込んだ Task".into(),
                kind: SpaceKind::Task,
                phase: TaskPhase::Working,
                base_oid: None,
                head_oid: None,
                result_summary: None,
                depends_on: Vec::new(),
                parent: None,
                created_at: 0,
                updated_at: 0,
            })
            .expect("書ける");

        let (second, cx) = open_window(cx, &scratch, "after");
        let shown = proposal_rows_shown(&second, cx);
        assert_eq!(shown.len(), 1);
        second.update(cx, |workspace, cx| {
            workspace.retry_task_creation(shown[0].2, cx)
        });
        settle(cx);
        let row = scratch
            .storage
            .load_proposal_row(&proposal_id, 0)
            .expect("読める")
            .expect("行がある");
        assert_eq!(
            (row.status, row.step, row.task_id.clone()),
            (ProposalRowStatus::Failed, ProposalRowStep::Worktree, None),
            "{row:?}"
        );
        assert!(
            row.reason
                .as_deref()
                .is_some_and(|reason| reason.contains("手で取り込んだ Task")),
            "どの Task が使っているかを出す: {:?}",
            row.reason
        );
        assert_eq!(
            ledger_counts(&scratch.storage, &other_id),
            (0, 0),
            "別の Task に ⚑ も作った出来事も付けない"
        );
        assert!(
            second.update(cx, |workspace, _| workspace
                .slot_for_task_id(&other_id)
                .is_none()),
            "開いて委任文を送ったりしない"
        );
        let shown = proposal_rows_shown(&second, cx);
        assert!(
            matches!(shown.as_slice(), [(0, CreationStage::Failed(_), _)]),
            "{shown:?}"
        );
        std::fs::remove_dir_all(&scratch.base).ok();
    }

    /// 別々に押した「再開」も、同じリポジトリの行は 1 本ずつ順に作る: 同じ題名の 2 行が同じブランチ名を決めて
    /// 1 つの worktree を取り合わない（2 本目は `-2`）。順番を外すと、スケジューラの seed 2 で 2 行が同じ Task を
    /// 「作れた」ことになる（確かめた）。いくつかの seed で回す。
    #[gpui::test(iterations = 4)]
    fn separate_resumes_of_same_titled_rows_make_two_tasks(cx: &mut gpui::TestAppContext) {
        let Some(scratch) = scratch("same_title") else {
            return;
        };
        cx.update(|cx| settings::init(Some(scratch.settings.clone()), None, cx));
        let (first, cx) = open_window(cx, &scratch, "before");
        let proposal_id = propose(&first, cx, &scratch, &["Fix the parser", "Fix the parser"]);
        approve_crashing_at(&first, cx, &proposal_id, Some((0, RowCheckpoint::Start)));
        let (second, cx) = open_window(cx, &scratch, "after");
        let shown = proposal_rows_shown(&second, cx);
        assert_eq!(shown.len(), 2, "{shown:?}");
        second.update(cx, |workspace, cx| {
            workspace.retry_task_creation(shown[0].2, cx);
            workspace.retry_task_creation(shown[1].2, cx);
        });
        settle(cx);
        let rows = scratch
            .storage
            .load_proposal_rows(&proposal_id)
            .expect("読める");
        assert!(
            rows.iter()
                .all(|row| row.status == ProposalRowStatus::Succeeded),
            "{rows:?}"
        );
        assert_ne!(rows[0].task_id, rows[1].task_id, "別々の Task");
        assert_eq!(
            task_branches(&scratch.repo),
            vec![
                "task/fix-the-parser".to_string(),
                "task/fix-the-parser-2".to_string()
            ]
        );
        assert_eq!(project::git_worktrees(&scratch.repo).len(), 3);
        std::fs::remove_dir_all(&scratch.base).ok();
    }

    /// 準備の失敗で委任文を控えた行の Task を片付けたら（破棄 = archived）、行も閉じる: 次の起動で「やり直す」を
    /// 出して、片付けた Task を作り直したり委任文を送ったりしない。
    #[gpui::test]
    fn archiving_a_held_task_closes_its_row(cx: &mut gpui::TestAppContext) {
        let Some(scratch) = scratch("held_archived") else {
            return;
        };
        cx.update(|cx| settings::init(Some(scratch.settings.clone()), None, cx));
        let script = project::worktree_setup_script(&scratch.repo);
        std::fs::create_dir_all(script.parent().expect("置き場")).expect("作れる");
        std::fs::write(&script, "#!/bin/sh\nexit 1\n").expect("書ける");
        let (first, cx) = open_window(cx, &scratch, "before");
        let proposal_id = propose(&first, cx, &scratch, &["Fix the parser"]);
        approve_crashing_at(&first, cx, &proposal_id, None);
        let task_id = scratch
            .storage
            .load_proposal_row(&proposal_id, 0)
            .expect("読める")
            .and_then(|row| row.task_id)
            .expect("Task は出した");
        first.update(cx, |workspace, cx| {
            let index = workspace.slot_for_task_id(&task_id).expect("Task がある");
            workspace.transition_task_space(index, TaskPhase::Archived, "task_discarded", None, cx);
            assert!(workspace.chrome.held_proposal_rows.is_empty());
        });
        settle(cx);
        assert_eq!(
            scratch
                .storage
                .load_proposal_row(&proposal_id, 0)
                .expect("読める")
                .map(|row| row.status),
            Some(ProposalRowStatus::Discarded)
        );
        let (second, cx) = open_window(cx, &scratch, "after");
        assert!(proposal_rows_shown(&second, cx).is_empty());
        std::fs::remove_dir_all(&scratch.base).ok();
    }

    /// 作れなかった行は、同じ回の残りの行を作っている間でも「やり直す」を押せる（終えた行の「動かしている」印は
    /// すぐ外す）。押した行は、同じリポジトリの今の回が終わってから順に作る。
    #[gpui::test]
    fn a_failed_row_can_be_retried_while_the_same_run_continues(cx: &mut gpui::TestAppContext) {
        let Some(scratch) = scratch("retry_mid_run") else {
            return;
        };
        cx.update(|cx| settings::init(Some(scratch.settings.clone()), None, cx));
        let (first, cx) = open_window(cx, &scratch, "only");
        let refs = scratch
            .repo
            .join(".git")
            .join("refs")
            .join("heads")
            .join("task");
        std::fs::create_dir_all(&refs).expect("作れる");
        let lock = refs.join("rope-buffer.lock");
        std::fs::write(&lock, "").expect("書ける");
        let proposal_id = propose(&first, cx, &scratch, &["Rope buffer", "Write README"]);
        first.update(cx, |workspace, cx| {
            workspace.approve_captain_proposal(&proposal_id, cx)
        });
        // 1 行目が「作れませんでした」になるまで 1 歩ずつ進める（2 行目はまだ同じ回の中）。
        let row_stage = |workspace: &Workspace, row: usize| {
            workspace
                .chrome
                .task_creations
                .iter()
                .find(|creation| {
                    creation
                        .proposal_row
                        .as_ref()
                        .is_some_and(|record| record.row == row)
                })
                .map(|creation| (creation.id, creation.stage.clone()))
        };
        let mut steps = 0;
        let failed = loop {
            if let Some((id, CreationStage::Failed(_))) =
                first.read_with(cx, |workspace, _| row_stage(workspace, 0))
            {
                break id;
            }
            assert!(cx.executor().tick(), "1 行目が失敗する前に止まった");
            steps += 1;
            assert!(steps < 100_000, "1 行目が失敗しない");
        };
        assert!(
            first
                .read_with(cx, |workspace, _| row_stage(workspace, 1))
                .is_some(),
            "2 行目はまだ作っている（同じ回の途中）"
        );
        std::fs::remove_file(&lock).expect("外せる");
        first.update(cx, |workspace, cx| {
            workspace.retry_task_creation(failed, cx)
        });
        assert!(
            first
                .read_with(cx, |workspace, _| row_stage(workspace, 0))
                .is_some(),
            "「別の窓で作っています」で消さない"
        );
        settle(cx);
        let rows = scratch
            .storage
            .load_proposal_rows(&proposal_id)
            .expect("読める");
        assert!(
            rows.iter()
                .all(|row| row.status == ProposalRowStatus::Succeeded),
            "{rows:?}"
        );
        assert_eq!(
            task_branches(&scratch.repo),
            vec![
                "task/rope-buffer".to_string(),
                "task/write-readme".to_string()
            ]
        );
        std::fs::remove_dir_all(&scratch.base).ok();
    }
}
