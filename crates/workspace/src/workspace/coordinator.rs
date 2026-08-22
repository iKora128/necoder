//! 監督（Coordinator）席 — **任命制のただの ACP スレッド**（FLEET-CONTROL-PLAN P6）。
//!
//! 設計（計画 §0-1）: 台帳が記憶・**監督は状態を持たない**（交代・再起動が自由）。
//! - 任命 = settings.json の `coordinator_agent`（表示名・None = 未任命。既定ドリフト禁止の原則）。
//! - 住まい = IntegrationSpace の AgentPanel 内の pinned thread「監督」（名前で再利用）。
//! - **wake はイベント駆動**（常駐ポーリング禁止）: Task の Done/Failed 遷移で即時・Blocked は
//!   15s 閾値（マスコット worry と同じ）で「まだ待っているか」を確かめてから 1 ターン渡す。
//! - 道具 = `necoder fleet` CLI（監督のエージェントは自分の shell で実行する。守るべき操作を
//!   necoder の CLI/MCP にだけ置く原則 §0-8 と両立 — Herdr 直叩きの迂回路は与えない）。
//! - integrate は radar clean + **人間 gate**（監督は提案まで・テンプレートで明示）。
//! - 采配の監査 = 監督ターンの完了を `coordinator` イベントとして task_events + ニュースへ
//!   （丸チップ・NewsKind::Coordinator）。

use crate::workspace::*;

/// 監督スレッドの固定名（IntegrationSpace の panel 内で名前により再利用する）。
pub(crate) const COORDINATOR_THREAD_NAME: &str = "監督";

impl Workspace {
    /// 監督へ 1 ターン渡す（イベント駆動 wake・P6）。未任命・IntegrationSpace 不在・
    /// 監督スレッド実行中（重ねない＝次のイベントで最新状態ごと読む）は静かにスキップ。
    pub(crate) fn wake_coordinator(
        &mut self,
        event: &str,
        title: SharedString,
        digest: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = settings::get(cx).coordinator_agent.clone() else {
            return;
        };
        let Some(integration) = self
            .project_sessions
            .projects
            .iter()
            .position(|slot| slot.task_space.is_integration())
        else {
            return;
        };
        let Some(session) = self.project_sessions.sessions.get(integration) else {
            return;
        };
        let panel = session.agent_panel.clone();
        let exe = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "necoder".to_string());
        let digest_line = digest
            .as_ref()
            .map(|digest| format!(" — {digest}"))
            .unwrap_or_default();
        // 役割・規律・道具のテンプレート + 変化分の digest（フル transcript は渡さない＝3 段圧縮）。
        let prompt = format!(
            "あなたは necoder 編隊の監督（coordinator）です。\n\
             役割: 状況を読み、次の采配を決めて fleet CLI で指示する。\n\
             規律: 自分ではコードを書かない・手を動かさない（指示と采配のみ）。\
             integrate の最終承認は人間（あなたは `fleet review` で radar を確認して提案するまで）。\n\
             \n\
             イベント: [{event}] {title}{digest_line}\n\
             \n\
             道具（shell で実行）:\n\
             {exe} fleet list .              — この repo の Task 一覧\n\
             {exe} fleet digest <id>        — Task の事実層 + digest（今なにをしているか）\n\
             {exe} fleet events [since-id]  — 台帳イベントの差分\n\
             {exe} fleet spawn-agent <id> [agent] [prompt...] — Task にエージェントを起こす\n\
             {exe} fleet send <id> <message...>               — 追撃指示\n\
             {exe} fleet status <id> <phase> [summary]        — phase 遷移の報告\n\
             {exe} fleet wait <id> <phase|activity> [秒]      — 待つ（phase=台帳 / activity=live）\n\
             {exe} fleet depend <id> <on...> / wait-deps <id> <phase> — 依存の宣言と待ち\n\
             {exe} fleet review <id>        — Conflict Radar（merge_ready / changes_requested へ）\n\
             \n\
             まず `{exe} fleet list .` と `digest` で現況を読み、必要な采配だけ実行して、\
             最後に判断を 1〜3 行で要約してください。何もする必要がなければ「静観」とだけ答えてください。"
        );
        panel.update(cx, |panel, cx| {
            let index = panel.ensure_named_thread(COORDINATOR_THREAD_NAME, &agent, cx);
            if panel.thread_busy(index) {
                return; // 実行中は重ねない（次のイベント時に最新状態ごと読む）
            }
            panel.send_prompt_text(prompt, cx);
        });
        cx.notify();
    }

    /// Blocked の wake は 15s 閾値（worry と同じ）: 15 秒待ってまだ Blocked なら渡す。
    /// 即時に人間が許可した場合は監督を起こさない（注意の節約）。
    pub(crate) fn wake_coordinator_for_blocked(
        &mut self,
        session_index: usize,
        title: SharedString,
        digest: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if settings::get(cx).coordinator_agent.is_none() {
            return;
        }
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(15))
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                let still_blocked = workspace
                    .project_sessions
                    .sessions
                    .get(session_index)
                    .is_some_and(|session| {
                        session
                            .agent_panel
                            .read(cx)
                            .statuses()
                            .iter()
                            .any(|status| status.activity == agent_panel::ThreadActivity::Blocked)
                    });
                if still_blocked {
                    workspace.wake_coordinator("blocked", title, digest, cx);
                }
            });
        })
        .detach();
    }

    /// 監督ターン完了の監査（P6）: 采配を `coordinator` イベントとして台帳 + ニュースへ。
    /// ニュースの丸チップ（NewsKind::Coordinator）は「監督の発言」の形の印。
    pub(crate) fn record_coordinator_decision(
        &mut self,
        color: Hsla,
        digest: Option<&SharedString>,
        summary: &SharedString,
        cx: &mut Context<Self>,
    ) {
        let text = digest.cloned().unwrap_or_else(|| summary.clone());
        self.push_news(
            NewsKind::Coordinator,
            color,
            SharedString::from(i18n::t!("control.coordinator")),
            text.clone(),
        );
        if let Some(storage) = self.persistence.storage.clone() {
            let payload = serde_json::json!({ "text": text.as_ref() }).to_string();
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) =
                        storage.append_task_event("coordinator", "coordinator", &payload)
                    {
                        eprintln!("監督の采配を記録できない: {error:#}");
                    }
                })
                .detach();
        }
        cx.notify();
    }
}
