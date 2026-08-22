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

/// 監督スレッドの表示名（IntegrationSpace の panel 内で名前により再利用する）。表示言語に追従する。
pub(crate) fn coordinator_thread_name() -> String {
    i18n::t!("control.coordinator_thread")
}

/// 同梱ロケール分の監督スレッド名。表示言語を切り替えても、前の言語で作った監督スレッドを
/// 取り違えずに再利用するため、探す時はこの全部で照合する（名前は保存済みのユーザーデータ）。
pub(crate) fn coordinator_thread_names() -> Vec<String> {
    i18n::available_locales()
        .into_iter()
        .filter_map(|locale| i18n::translate_in(locale, "control.coordinator_thread"))
        .collect()
}

/// 監督スレッドの名前か（ロケール横断）。
pub(crate) fn is_coordinator_thread_name(name: &str) -> bool {
    coordinator_thread_names().iter().any(|known| known == name)
}

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
        let prompt = i18n::t!(
            "control.coordinator_prompt",
            "event" => event,
            "title" => &title,
            "digest" => digest_line,
            "exe" => exe,
        );
        let names = coordinator_thread_names();
        panel.update(cx, |panel, cx| {
            let index = panel.ensure_named_thread(&coordinator_thread_name(), &names, &agent, cx);
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
