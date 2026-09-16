//! Captain 席 — **任命制のただの ACP スレッド**（FLEET-V2 §5）。
//!
//! 設計（FLEET-V2 §5.1）: 台帳が記憶・**Captain は状態を持たない**（交代・再起動が自由）。
//! - 任命 = settings.json の `captain_agent`（表示名・None = 未任命。既定ドリフト禁止の原則）。
//! - 住まい = IntegrationSpace の AgentPanel 内の pinned thread「Captain」（名前で再利用）。
//! - **wake はイベント駆動**（常駐ポーリング禁止・§5.3）: Task の Done/Failed 遷移で即時・Blocked は
//!   15s 閾値（マスコット worry と同じ）で「まだ待っているか」を確かめてから 1 ターン渡す。
//! - 道具 = `necoder fleet` CLI（Captain のエージェントは自分の shell で実行する。守るべき操作を
//!   necoder の CLI/MCP にだけ置く原則と両立 — Herdr 直叩きの迂回路は与えない）。
//! - integrate は radar clean + **人間 gate**（Captain は提案まで・§5.2 の権限表）。
//! - 采配の監査 = Captain ターンの完了を `captain` イベントとして task_events + ニュースへ
//!   （丸チップ・NewsKind::Captain）。

use crate::workspace::*;

/// Captain スレッドの表示名（IntegrationSpace の panel 内で名前により再利用する）。表示言語に追従する。
/// Captain バーの見出し・ニュースの帰属名と同じ語（`captain.title`）を使う。
pub(crate) fn captain_thread_name() -> String {
    i18n::t!("captain.title")
}

/// 同梱ロケール分の Captain スレッド名。表示言語を切り替えても、前の言語で作った Captain スレッドを
/// 取り違えずに再利用するため、探す時はこの全部で照合する（名前は保存済みのユーザーデータ）。
pub(crate) fn captain_thread_names() -> Vec<String> {
    i18n::available_locales()
        .into_iter()
        .filter_map(|locale| i18n::translate_in(locale, "captain.title"))
        .collect()
}

/// Captain スレッドの名前か（ロケール横断）。
pub(crate) fn is_captain_thread_name(name: &str) -> bool {
    captain_thread_names().iter().any(|known| known == name)
}

impl Workspace {
    /// Captain へ 1 ターン渡す（イベント駆動 wake・§5.3）。未任命・IntegrationSpace 不在・
    /// Captain スレッド実行中（重ねない＝次のイベントで最新状態ごと読む）は静かにスキップ。
    pub(crate) fn wake_captain(
        &mut self,
        source: usize,
        event: &str,
        title: SharedString,
        digest: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = settings::get(cx).captain_agent.clone() else {
            return;
        };
        let Some(repository) = self.project_sessions.projects.get(source).map(|slot| slot.repository_key().to_string()) else { return; };
        let Some(integration) = self.project_sessions.projects.iter().position(|slot| slot.task_space.is_integration() && slot.repository_key() == repository) else { return; };
        if event != "flush" {
            self.chrome.captain_pending.entry(repository.clone()).or_default().push(format!("{event}: {title} — {}", digest.as_deref().unwrap_or("")));
        }
        let Some(session) = self.project_sessions.sessions.get(integration) else {
            return;
        };
        let panel = session.fleet_agents[0].clone();
        let exe = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "necoder".to_string());
        let digest_line = digest
            .as_ref()
            .map(|digest| format!(" — {digest}"))
            .unwrap_or_default();
        // 役割・規律・道具のテンプレート + 変化分の digest（フル transcript は渡さない＝3 段圧縮）。
        let prompt = i18n::t!(
            "captain.prompt",
            "event" => event,
            "title" => &title,
            "digest" => digest_line,
            "exe" => exe,
        );
        let names = captain_thread_names();
        let index = panel.update(cx, |panel, cx| panel.ensure_named_thread(&captain_thread_name(), &names, &agent, cx));
        if panel.read(cx).thread_busy(index) { return; }
        let events = self.chrome.captain_pending.remove(&repository).unwrap_or_default();
        if events.is_empty() { return; }
        let facts = self.captain_facts(&repository, cx);
        panel.update(cx, |panel, cx| {
            panel.focus_thread(index, cx);
            panel.send_ledger_event(format!("{prompt}\n{facts}\n{}", events.join("\n")), cx);
        });
        cx.notify();
    }

    /// Blocked の wake は 15s 閾値（worry と同じ）: 15 秒待ってまだ Blocked なら渡す。
    /// 即時に人間が許可した場合は Captain を起こさない（注意の節約）。
    pub(crate) fn wake_captain_for_blocked(
        &mut self,
        session_index: usize,
        title: SharedString,
        digest: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if settings::get(cx).captain_agent.is_none() {
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
                    workspace.wake_captain(session_index, "blocked", title, digest, cx);
                }
            });
        })
        .detach();
    }

    /// Captain ターン完了の監査（§5.6）: 采配を `captain` イベントとして台帳 + ニュースへ。
    /// ニュースの丸チップ（NewsKind::Captain）は「Captain の発言」の形の印。
    pub(crate) fn record_captain_decision(
        &mut self,
        color: Hsla,
        digest: Option<&SharedString>,
        summary: &SharedString,
        cx: &mut Context<Self>,
    ) {
        let text = digest.cloned().unwrap_or_else(|| summary.clone());
        self.push_news(
            NewsKind::Captain,
            color,
            SharedString::from(i18n::t!("captain.title")),
            text.clone(),
        );
        if let Some(storage) = self.persistence.storage.clone() {
            let payload = serde_json::json!({ "text": text.as_ref() }).to_string();
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = storage.append_task_event("captain", "captain", &payload) {
                        eprintln!("Captain の采配を記録できない: {error:#}");
                    }
                })
                .detach();
        }
        cx.notify();
    }
}

impl Workspace {
    pub(super) fn captain_facts(&self, repository: &str, cx: &App) -> String {
        self.project_sessions.projects.iter().enumerate()
            .filter(|(_, slot)| slot.repository_key() == repository && !slot.task_space.is_integration())
            .map(|(index, slot)| {
                let statuses = self.project_sessions.sessions[index].agent_statuses(cx);
                let digest = statuses.iter().filter_map(|(_, _, status)| status.digest.as_deref()).collect::<Vec<_>>().join(" / ");
                format!("{} | {} | {} | {}", slot.task_space.id.0, slot.task_space.title, slot.task_space.phase.as_str(), digest)
            }).collect::<Vec<_>>().join("\n")
    }

    pub(super) fn record_human_send(&mut self, index: usize, thread: &SharedString, text: &str, cx: &mut Context<Self>) {
        let slot = &self.project_sessions.projects[index];
        if slot.task_space.is_integration() { return; }
        let id = slot.task_space.id.0.clone();
        let title = slot.task_space.title.clone();
        let color = slot.color;
        let repository = slot.repository_key().to_string();
        self.chrome.captain_pending.entry(repository).or_default().push(format!("human_send: {title} / {thread}: {text}"));
        self.push_news(NewsKind::HumanSend, color, title, text.to_string().into());
        if let Some(storage) = self.persistence.storage.clone() {
            let payload = serde_json::json!({"thread": thread.as_ref(), "text": text}).to_string();
            cx.background_executor().spawn(async move {
                if let Err(error) = storage.append_task_event(&id, "human_send", &payload) { eprintln!("human_send: {error:#}"); }
            }).detach();
        }
        cx.notify();
    }
}

impl Workspace {
    pub(super) fn render_captain_card(&self, index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let session = &self.project_sessions.sessions[index];
        let repository = self.project_sessions.projects[index].repository_key();
        let panel = session.fleet_agents[0].clone();
        let mut tabs = div().flex_none().flex().gap(px(12.)).px(px(12.)).h(px(30.));
        for (tab, key) in [(0usize, "captain.title"), (1, "captain.log"), (2, "captain.tasks")] {
            tabs = tabs.child(div().id(("captain-tab", tab)).cursor_pointer().text_size(px(12.)).text_color(self.theme.fg1)
                .border_b_2().border_color(if tab == self.chrome.captain_tab { self.accent() } else { self.theme.border })
                .child(SharedString::from(i18n::t!(key))).on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.chrome.captain_tab = tab; cx.notify();
                })));
        }
        let body = match self.chrome.captain_tab {
            1 => {
                let mut log = div().id("captain-log").size_full().overflow_y_scroll().p(px(12.));
                for item in self.notifications.news.iter().filter(|item| item.kind == NewsKind::Captain) {
                    log = log.child(div().py(px(6.)).text_size(px(12.)).text_color(self.theme.fg1)
                        .child(SharedString::from(format!("{} · ✳ {}", agent_panel::relative_time_label(item.at_ms), item.text))));
                }
                log.into_any_element()
            }
            2 => {
                let mut tasks = div().id("captain-tasks").size_full().overflow_y_scroll().p(px(12.));
                for (target, slot) in self.project_sessions.projects.iter().enumerate().filter(|(_, slot)| slot.repository_key() == repository && !slot.task_space.is_integration()) {
                    tasks = tasks.child(div().id(("captain-task", target)).cursor_pointer().py(px(6.)).text_size(px(12.)).text_color(slot.color)
                        .child(slot.task_space.title.clone()).on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| this.switch_project(target, window, cx))));
                }
                tasks.into_any_element()
            }
            _ => panel.cached(StyleRefinement::default().flex().flex_col().size_full()).into_any_element(),
        };
        div().flex_1().min_w_0().min_h_0().flex().flex_col().rounded(px(8.)).overflow_hidden().border_1().border_color(self.theme.border).bg(self.theme.bg0)
            .child(div().h(px(2.)).flex_none().bg(self.accent()))
            .child(div().px(px(12.)).py(px(8.)).text_size(px(13.)).text_color(self.theme.fg0).child(SharedString::from(format!("⚑ Captain · {}", self.project_sessions.projects[index].branch.as_deref().unwrap_or("")))))
            .child(div().px(px(12.)).pb(px(8.)).text_size(px(10.)).text_color(self.theme.fg2).child(i18n::t!("captain.human_gate")))
            .child(tabs).child(div().flex_1().min_h_0().child(body)).into_any_element()
    }
}
