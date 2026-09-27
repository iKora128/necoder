//! Captain 席 — **任命制のただの ACP スレッド**（FLEET-V2 §5）。
//!
//! 設計（FLEET-V2 §5.1）: 台帳が記憶・**Captain は状態を持たない**（交代・再起動が自由）。
//! - 任命 = settings.json の `captain_agent`（表示名・None = 未任命。既定ドリフト禁止の原則）。
//! - 住まい = IntegrationSpace の AgentPanel の「Captain」スレッド。**席**（`agent_panel::SeatPolicy`）に座る:
//!   道具は Captain 用の MCP（`necoder mcp --captain`）だけ、権限の問いには necoder が決まりで答える（§5.8）。
//! - **wake はイベント駆動**（常駐ポーリング禁止・§5.3）。渡すのは**台帳の未読**: Captain がどこまで読んだかを
//!   DB（`captain_cursors`）に置き、起こす要求を 2 秒まとめてから、その位置より後の出来事を 1 通にする。
//!   位置を進めるのはターンが終わってから（途中で落ちたら同じ出来事をもう一度渡す）。
//! - 会話が膨らんだら同じタブのまま新しい会話へ交代する（§5.6）。前置き（役割・現況・直近の采配）が続きを渡す。
//! - integrate は radar clean + **人間 gate**（Captain は提案まで・§5.2 の権限表）。Task を切るのも
//!   分解案の承認の後（§5.5・`captain_proposals.rs`）。
//! - 采配の監査 = Captain ターンの完了を `captain` イベントとして task_events + ニュースへ
//!   （丸チップ・NewsKind::Captain）。

use crate::workspace::*;

/// Captain の席の名前（`agent_panel::SeatPolicy::id`）。
pub(crate) const CAPTAIN_SEAT: &str = "captain";
/// 起こす要求をまとめる時間（§5.3・同じ Task の連続遷移を 1 通に畳む）。
const CAPTAIN_WAKE_COALESCE: std::time::Duration = std::time::Duration::from_secs(2);
/// 1 通に載せる出来事の上限（新しい方から。残りは `fleet_events` で読める）。
const CAPTAIN_WAKE_MAX_EVENTS: usize = 40;
/// 未読を読む上限（長く眠っていたリポジトリで読みすぎない）。
const CAPTAIN_EVENT_SCAN_CAP: i64 = 2000;
/// 会話の交代: 文脈が上限のこの割合（%）を超えたら（§5.6）。
const CAPTAIN_ROTATE_CONTEXT_PERCENT: u64 = 40;
/// 会話の交代: 前回の交代からこの回数起きたら。
const CAPTAIN_ROTATE_WAKES: u32 = 20;
/// 前置きに載せる直近の采配の件数（§5.9）。
const CAPTAIN_RECENT_DECISIONS: usize = 5;
/// 1 行に載せる digest の長さの上限（文字）。
const CAPTAIN_LINE_DIGEST_CHARS: usize = 160;

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

/// Captain へ渡す出来事か（§5.3）。working / planned などの途中経過と、Captain 自身の記録（`captain` /
/// `captain_proposed`）・`tier2` は渡さない（現況表と digest で足りる）。作られた Task は渡す（id が要る）。
fn is_captain_news(event: &storage::TaskEventRecord) -> bool {
    match event.kind.as_str() {
        "human_send" | "task_created" | "proposal_approved" | "proposal_rejected" => true,
        "phase_changed" => {
            let payload: serde_json::Value =
                serde_json::from_str(&event.payload).unwrap_or_default();
            let field = |key: &str| {
                payload
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            matches!(
                field("phase").as_str(),
                "blocked"
                    | "review_ready"
                    | "changes_requested"
                    | "merge_ready"
                    | "failed"
                    | "integrated"
                    | "archived"
            ) || field("reason") == "task_created"
        }
        _ => false,
    }
}

/// 1 通の本文（台帳の未読を古い順に 1 行ずつ・新しい方から最大 40 件）。渡すものが無ければ空。
pub(crate) fn captain_wake_lines(events: &[storage::RepositoryEvent]) -> Vec<String> {
    let news: Vec<&storage::RepositoryEvent> = events
        .iter()
        .filter(|event| is_captain_news(&event.event))
        .collect();
    let omitted = news.len().saturating_sub(CAPTAIN_WAKE_MAX_EVENTS);
    let mut lines = Vec::with_capacity(news.len() - omitted + 1);
    if omitted > 0 {
        lines.push(i18n::t!("captain.wake_omitted", "n" => omitted));
    }
    lines.extend(news.iter().skip(omitted).map(|event| captain_event_line(event)));
    lines
}

/// 台帳の 1 件を Captain が采配に使える 1 行へ（`#通し番号 Task-id 題名: 何が起きたか`）。
/// Task の id を必ず載せる（`fleet_send` / `fleet_spawn_agent` の宛先に使う）。
fn captain_event_line(event: &storage::RepositoryEvent) -> String {
    let payload: serde_json::Value = serde_json::from_str(&event.event.payload).unwrap_or_default();
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let titles = |key: &str| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .unwrap_or_default()
    };
    let detail = match event.event.kind.as_str() {
        "phase_changed" if text("reason") == "task_created" => i18n::t!("captain.line_created"),
        "phase_changed" => {
            let digest: String = text("digest").chars().take(CAPTAIN_LINE_DIGEST_CHARS).collect();
            if digest.is_empty() {
                format!("→ {}", text("phase"))
            } else {
                format!("→ {} — {digest}", text("phase"))
            }
        }
        "human_send" => i18n::t!("captain.line_human_send", "text" => text("text")),
        "task_created" => i18n::t!("captain.line_created"),
        "proposal_approved" => i18n::t!(
            "captain.line_approved",
            "approved" => titles("approved"),
            "skipped" => titles("skipped"),
        ),
        "proposal_rejected" => i18n::t!("captain.line_rejected"),
        other => other.to_string(),
    };
    let title = event
        .title
        .as_deref()
        .map(|title| format!(" {title}"))
        .unwrap_or_default();
    format!("#{} {}{title}: {detail}", event.event.id, event.event.task_id)
}

/// 会話を交代するか（§5.6）。上限が分からない（0）エージェントは回数だけで決める。
fn should_rotate_captain(tokens_used: u32, tokens_max: u32, wakes_since_rotation: u32) -> bool {
    let over_context = tokens_max > 0
        && u64::from(tokens_used) * 100 >= u64::from(tokens_max) * CAPTAIN_ROTATE_CONTEXT_PERCENT;
    over_context || wakes_since_rotation >= CAPTAIN_ROTATE_WAKES
}

impl Workspace {
    /// Captain を起こす要求（イベント駆動・§5.3）。未任命なら何もしない。2 秒まとめてから、
    /// 台帳の未読を 1 通にして渡す（渡すものが無ければ起こさない）。
    pub(crate) fn request_captain_wake(&mut self, repository: &str, cx: &mut Context<Self>) {
        if settings::get(cx).captain_agent.is_none() {
            return;
        }
        if !self
            .chrome
            .captain_wake_scheduled
            .insert(repository.to_string())
        {
            return; // もうまとめている最中
        }
        let repository = repository.to_string();
        cx.spawn(async move |workspace, cx| {
            cx.background_executor().timer(CAPTAIN_WAKE_COALESCE).await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.chrome.captain_wake_scheduled.remove(&repository);
                workspace.deliver_captain_unread(&repository, cx);
            });
        })
        .detach();
    }

    /// Task の slot から起こす（呼び手の多くは session index しか持っていない）。
    pub(crate) fn wake_captain(&mut self, session_index: usize, cx: &mut Context<Self>) {
        let Some(repository) = self
            .project_sessions
            .projects
            .get(session_index)
            .map(|slot| slot.repository_key().to_string())
        else {
            return;
        };
        self.request_captain_wake(&repository, cx);
    }

    /// Blocked の wake は 15s 閾値（worry と同じ）: 15 秒待ってまだ Blocked なら起こす。
    /// 即時に人間が許可した場合は Captain を起こさない（注意の節約）。
    pub(crate) fn wake_captain_for_blocked(&mut self, session_index: usize, cx: &mut Context<Self>) {
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
                    workspace.wake_captain(session_index, cx);
                }
            });
        })
        .detach();
    }

    /// このリポジトリの統合先（Captain の住まい）の slot。
    pub(crate) fn captain_integration_index(&self, repository: &str) -> Option<usize> {
        self.project_sessions.projects.iter().position(|slot| {
            slot.task_space.is_integration() && slot.repository_key() == repository
        })
    }

    /// Captain の席の決まり（§5.8）。remote の統合先は `None`（MCP の子プロセスはエージェントと同じ
    /// host で動くので、手元の necoder を渡せない）＝席なしの従来どおり。
    fn captain_seat_policy(&self, integration: usize) -> Option<agent_panel::SeatPolicy> {
        let slot = self.project_sessions.projects.get(integration)?;
        if slot.worktree.host().is_remote() {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        let root = slot.worktree.root().display().to_string();
        Some(agent_panel::SeatPolicy {
            id: CAPTAIN_SEAT,
            mcp_servers: vec![acp_client::mcp::McpServerConfig {
                name: "necoder".to_string(),
                source: acp_client::mcp::McpSource::Necoder,
                enabled: true,
                transport: acp_client::mcp::McpTransport::Stdio {
                    command: exe.display().to_string(),
                    args: vec!["mcp".to_string(), "--captain".to_string(), root],
                    env: std::collections::BTreeMap::new(),
                },
            }],
            allowed_tools: CAPTAIN_MCP_TOOLS.iter().map(|tool| tool.to_string()).collect(),
            denied_notice: i18n::t!("captain.seat_denied"),
            violation_notice: i18n::t!("captain.seat_violation"),
        })
    }

    /// Captain の席のスレッドを用意する（席 → 名前の順で探し、無ければ作る）。席の決まりと前置きを付ける。
    /// Captain のエージェントを替えていたら、前の Captain は名前を変えて普通のスレッドに退かせ、新しい席を作る
    /// （前の会話は残る・§5.7 の交代）。未任命・統合先の panel が無いなら `None`。
    pub(crate) fn ensure_captain_thread(
        &mut self,
        integration: usize,
        cx: &mut Context<Self>,
    ) -> Option<(Entity<AgentPanel>, usize)> {
        let agent = settings::get(cx).captain_agent.clone()?;
        let repository = self
            .project_sessions
            .projects
            .get(integration)?
            .repository_key()
            .to_string();
        let panel = self
            .project_sessions
            .sessions
            .get(integration)?
            .fleet_agents
            .first()?
            .clone();
        let seat = self.captain_seat_policy(integration);
        let context = self.captain_context(&repository, cx);
        let names = captain_thread_names();
        let index = panel.update(cx, |panel, cx| {
            let existing = panel
                .seat_thread(CAPTAIN_SEAT)
                .or_else(|| names.iter().find_map(|name| panel.thread_index_named(name)));
            if let Some(existing) = existing {
                let current = panel.thread_agent(existing).unwrap_or_default();
                if current.as_ref() != agent.as_str() {
                    panel.retire_seat_thread(
                        existing,
                        i18n::t!("captain.retired_name", "agent" => current.as_ref()),
                        cx,
                    );
                }
            }
            // 表示中のタブは変えない（wake が人間の見ている会話を奪わない）。前面に出すのは ⌘0 の側。
            let index = panel.seat_thread(CAPTAIN_SEAT).unwrap_or_else(|| {
                panel.ensure_named_thread_quietly(&captain_thread_name(), &names, &agent, cx)
            });
            if let Some(seat) = seat {
                panel.assign_seat(index, seat, cx);
            }
            panel.set_prompt_context(index, context);
            index
        });
        Some((panel, index))
    }

    /// 台帳の未読を読み、渡すものがあれば 1 通にして Captain へ（§5.3）。Captain が走っている間は
    /// 重ねない（ターンが終わったら [`Self::finish_captain_turn`] が続きを確かめる）。
    fn deliver_captain_unread(&mut self, repository: &str, cx: &mut Context<Self>) {
        if settings::get(cx).captain_agent.is_none()
            || self.chrome.captain_inflight.contains_key(repository)
            || self.captain_integration_index(repository).is_none()
        {
            return; // 統合先を開いていなければ、開いた時に届く（位置は進めない）
        }
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        let repository = repository.to_string();
        cx.spawn(async move |workspace, cx| {
            let reader = storage.clone();
            let key = repository.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    let cursor = match reader.load_captain_cursor(&key)? {
                        Some(cursor) => cursor,
                        None => {
                            // 初めての任命: その時点の末尾から始める（過去の全履歴は渡さない）。
                            let latest = reader.latest_task_event_id()?;
                            reader.save_captain_cursor(&key, latest)?;
                            latest
                        }
                    };
                    reader.load_repository_events_since(&key, cursor, CAPTAIN_EVENT_SCAN_CAP)
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                let events = match loaded {
                    Ok(events) => events,
                    Err(error) => {
                        eprintln!("Captain へ渡す台帳を読めない: {error:#}");
                        return;
                    }
                };
                let Some(last) = events.last().map(|event| event.event.id) else {
                    return; // 未読なし
                };
                let lines = captain_wake_lines(&events);
                if lines.is_empty() {
                    // 途中経過だけ: 起こさずに位置だけ進める（次回また同じ行を読み直さない）。
                    cx.background_executor()
                        .spawn(async move {
                            if let Err(error) = storage.save_captain_cursor(&repository, last) {
                                eprintln!("Captain の読んだ位置を保存できない: {error:#}");
                            }
                        })
                        .detach();
                    return;
                }
                workspace.send_captain_wake(&repository, lines, last, cx);
            });
        })
        .detach();
    }

    /// 1 通を Captain へ送る。必要なら先に会話を交代する（§5.6）。表示中のタブは奪わない。
    fn send_captain_wake(
        &mut self,
        repository: &str,
        lines: Vec<String>,
        last_event_id: i64,
        cx: &mut Context<Self>,
    ) {
        if self.chrome.captain_inflight.contains_key(repository) {
            return;
        }
        let Some(integration) = self.captain_integration_index(repository) else {
            return;
        };
        let Some((panel, index)) = self.ensure_captain_thread(integration, cx) else {
            return;
        };
        if panel.read(cx).thread_busy(index) {
            return; // 人間と話している最中。ターンが終わったら続きを確かめる
        }
        let wakes = self
            .chrome
            .captain_wakes_since_rotation
            .get(repository)
            .copied()
            .unwrap_or(0);
        let (tokens_used, tokens_max) = panel.read(cx).thread_tokens(index);
        if should_rotate_captain(tokens_used, tokens_max, wakes)
            && panel.update(cx, |panel, cx| {
                panel.rotate_thread_session(index, i18n::t!("captain.rotated"), cx)
            })
        {
            self.chrome
                .captain_wakes_since_rotation
                .insert(repository.to_string(), 0);
        }
        *self
            .chrome
            .captain_wakes_since_rotation
            .entry(repository.to_string())
            .or_insert(0) += 1;
        self.chrome
            .captain_inflight
            .insert(repository.to_string(), last_event_id);
        let message = format!("{}\n{}", i18n::t!("captain.wake_header"), lines.join("\n"));
        panel.update(cx, |panel, cx| panel.send_ledger_event_to(index, message, cx));
        cx.notify();
    }

    /// Captain のターンが終わった（§5.3）。1 通を渡していたら、その位置まで「読んだ」と DB に書き、
    /// 続けて未読を確かめる（実行中に届いた出来事）。失敗したターンは位置を進めない（次の自然な wake で
    /// 同じ出来事をもう一度渡す。失敗のたびに起こし直して空回りしない）。
    pub(crate) fn finish_captain_turn(
        &mut self,
        session_index: usize,
        succeeded: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self
            .project_sessions
            .projects
            .get(session_index)
            .map(|slot| slot.repository_key().to_string())
        else {
            return;
        };
        let inflight = self.chrome.captain_inflight.remove(&repository);
        if !succeeded {
            return;
        }
        if let (Some(last), Some(storage)) = (inflight, self.persistence.storage.clone()) {
            let key = repository.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = storage.save_captain_cursor(&key, last) {
                        eprintln!("Captain の読んだ位置を保存できない: {error:#}");
                    }
                })
                .detach();
        }
        self.request_captain_wake(&repository, cx);
    }

    /// 起動時: Captain が任命されていて統合先が開いているリポジトリで、未読があれば 1 回起こす（§5.3）。
    pub(crate) fn resume_captains(&mut self, cx: &mut Context<Self>) {
        if settings::get(cx).captain_agent.is_none() {
            return;
        }
        let repositories: Vec<String> = self
            .project_sessions
            .projects
            .iter()
            .filter(|slot| slot.task_space.is_integration() && !slot.worktree.host().is_remote())
            .map(|slot| slot.repository_key().to_string())
            .collect();
        for repository in repositories {
            self.request_captain_wake(&repository, cx);
        }
    }

    /// Captain ターン完了の監査（§5.6）: 采配を `captain` イベントとして台帳 + ニュースへ。
    /// ニュースの丸チップ（NewsKind::Captain）は「Captain の発言」の形の印。直近の采配はリポジトリ別に
    /// 控え、会話の交代の後も前置きで渡す（§5.9）。
    pub(crate) fn record_captain_decision(
        &mut self,
        session_index: usize,
        color: Hsla,
        digest: Option<&SharedString>,
        summary: &SharedString,
        cx: &mut Context<Self>,
    ) {
        let text = digest.cloned().unwrap_or_else(|| summary.clone());
        if let Some(repository) = self
            .project_sessions
            .projects
            .get(session_index)
            .map(|slot| slot.repository_key().to_string())
        {
            let recent = self.chrome.captain_recent.entry(repository).or_default();
            recent.push_back(text.to_string());
            while recent.len() > CAPTAIN_RECENT_DECISIONS {
                recent.pop_front();
            }
        }
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
    /// Captain スレッドの前置き（役割 + 現況表 + 直近の采配・§5.9）。台帳イベントだけでなく
    /// **人間が Captain に直接書いた発話にも**前置される（§5.3「発話 + 現況」）。これが無いと
    /// 人間から始めた Captain は自分の役割を知らず、普通のエージェントとして手を動かしてしまう。
    pub(super) fn captain_context(&self, repository: &str, cx: &App) -> String {
        let role = i18n::t!("captain.role");
        let facts = self.captain_facts(repository, cx);
        let mut context = format!("{role}\n{}\n{facts}\n", i18n::t!("captain.facts"));
        if let Some(recent) = self
            .chrome
            .captain_recent
            .get(repository)
            .filter(|recent| !recent.is_empty())
        {
            context.push_str(&i18n::t!("captain.recent"));
            context.push('\n');
            for decision in recent {
                context.push_str("- ");
                context.push_str(decision);
                context.push('\n');
            }
        }
        context
    }

    /// 人間が Captain に書いた後、次のターン用に現況を差し替える（⌘0 で開いた時点の表のままにしない）。
    pub(super) fn refresh_captain_context(
        &self,
        session_index: usize,
        panel: &Entity<AgentPanel>,
        thread: &SharedString,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get(session_index) else { return; };
        if !slot.task_space.is_integration() { return; }
        let context = self.captain_context(slot.repository_key(), cx);
        panel.update(cx, |panel, _| {
            if let Some(index) = panel.thread_index_named(thread) {
                panel.set_prompt_context(index, context);
            }
        });
    }

    pub(super) fn captain_facts(&self, repository: &str, cx: &App) -> String {
        self.project_sessions.projects.iter().enumerate()
            .filter(|(_, slot)| slot.repository_key() == repository && !slot.task_space.is_integration())
            .map(|(index, slot)| {
                let statuses = self.project_sessions.sessions[index].agent_statuses(cx);
                let digest = statuses.iter().filter_map(|(_, _, status)| status.digest.as_deref()).collect::<Vec<_>>().join(" / ");
                format!("{} | {} | {} | {}", slot.task_space.id.0, slot.task_space.title, slot.task_space.phase.as_str(), digest)
            }).collect::<Vec<_>>().join("\n")
    }

    /// 人間が Task に直接書いた（介入・§5.4）: 台帳へ `human_send`（原文）を積み、ニュースに載せる。
    /// Captain は単独では起こさない（次の wake に台帳の未読として同乗する）。
    pub(super) fn record_human_send(&mut self, index: usize, thread: &SharedString, text: &str, cx: &mut Context<Self>) {
        let slot = &self.project_sessions.projects[index];
        if slot.task_space.is_integration() { return; }
        let id = slot.task_space.id.0.clone();
        let title = slot.task_space.title.clone();
        let color = slot.color;
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
    /// ブリッジの会話ペインに出す「Captain を任命する」面（未任命で ⌘0 / ⚑ タブ / Captain バーを押した時）。
    /// 任命 = 設定 `captain_agent` に表示名を書くだけ（§5.7）。ここに UI が無いと settings.json の手書きが唯一の
    /// 入口になり、Captain が実質使えない（2026-09-20 のユーザー報告）。
    pub(super) fn render_captain_appoint(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let agents = acp_client::authenticated_agent_labels();
        let mut choices = div().flex().flex_wrap().gap(px(8.));
        for (index, agent) in agents.iter().enumerate() {
            let label = agent.to_string();
            choices = choices.child(
                div().id(("captain-appoint", index)).flex().items_center().gap(px(7.)).h(px(30.)).px(px(12.)).rounded(px(6.))
                    .border_1().border_color(theme.border).bg(theme.bg2).text_size(px(12.)).text_color(theme.fg0)
                    .cursor_pointer().hover(|style| style.border_color(accent))
                    .child(agent_panel::agent_badge(label.as_str(), 14.0))
                    .child(SharedString::from(label.clone()))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                        settings::set_user_value(cx, "captain_agent", serde_json::Value::String(label.clone()));
                        this.chrome.captain_appointing = false;
                        this.focus_captain(&FocusCaptain, window, cx);
                    })),
            );
        }
        div().size_full().flex().flex_col().justify_center().gap(px(14.)).px(px(28.))
            .child(div().flex().items_center().gap(px(8.)).text_size(px(15.)).text_color(theme.fg0)
                .child(div().text_color(accent).child("⚑"))
                .child(SharedString::from(i18n::t!("captain.appoint_title"))))
            .child(div().max_w(px(560.)).text_size(px(12.)).text_color(theme.fg1).child(SharedString::from(i18n::t!("captain.appoint_body"))))
            .child(if agents.is_empty() {
                div().text_size(px(12.)).text_color(theme.fg2).child(SharedString::from(i18n::t!("captain.appoint_no_agent"))).into_any_element()
            } else {
                choices.into_any_element()
            })
            .child(div().text_size(px(10.5)).text_color(theme.fg2).child(SharedString::from(i18n::t!("captain.human_gate"))))
            .into_any_element()
    }

    /// ブリッジのサイドペイン: Captain の采配ログ（ニュースの Captain 行だけを時系列で）。
    pub(super) fn render_captain_log(&self) -> gpui::AnyElement {
        let mut log = div().id("captain-log").size_full().overflow_y_scroll().p(px(12.));
        let mut empty = true;
        for item in self.notifications.news.iter().filter(|item| item.kind == NewsKind::Captain) {
            empty = false;
            log = log.child(div().flex().items_baseline().gap(px(6.)).py(px(5.)).text_size(px(12.)).text_color(self.theme.fg1)
                .child(div().flex_none().text_size(px(10.)).text_color(self.theme.fg2).child(SharedString::from(agent_panel::relative_time_label(item.at_ms))))
                .child(div().flex_none().text_size(px(10.)).text_color(theme_core::claude_bullet()).child("✳"))
                .child(div().flex_1().min_w_0().child(item.text.clone())));
        }
        if empty {
            log = log.child(div().text_size(px(12.)).text_color(self.theme.fg2).child(SharedString::from(i18n::t!("captain.log_empty"))));
        }
        log.into_any_element()
    }

    /// Captain の采配ログの件数（ペインバーのトグルに添える）。
    pub(super) fn captain_log_count(&self) -> usize {
        self.notifications.news.iter().filter(|item| item.kind == NewsKind::Captain).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: i64, task: &str, kind: &str, payload: serde_json::Value, title: Option<&str>) -> storage::RepositoryEvent {
        storage::RepositoryEvent {
            event: storage::TaskEventRecord {
                id,
                task_id: task.to_string(),
                kind: kind.to_string(),
                payload: payload.to_string(),
                created_at: 0,
            },
            title: title.map(str::to_string),
        }
    }

    /// Captain の役割文は道具の一覧を持たない（MCP の道具が自分の説明を持つ・§5.9）。代わりに
    /// 「提案する」「承認は人間」「編集しない」を全ロケールで持つ。
    #[test]
    fn role_prompt_speaks_of_proposals_not_shell_commands_in_every_locale() {
        for locale in i18n::available_locales() {
            let role = i18n::translate_in(locale, "captain.role")
                .unwrap_or_else(|| panic!("captain.role missing in {locale}"));
            assert!(role.contains("fleet_propose_tasks"), "{locale}: {role}");
            assert!(!role.contains("fleet create"), "shell の CLI を案内しない: {locale}");
            assert!(!role.contains("%{exe}"), "実行ファイルのパスを埋めない: {locale}");
        }
    }

    /// 渡すのは采配に要る出来事だけ（§5.3）。途中経過と Captain 自身の記録は落とし、Task の id を必ず載せる。
    #[test]
    fn wake_lines_keep_what_the_captain_needs() {
        let events = vec![
            event(1, "space-a", "phase_changed", serde_json::json!({"phase": "working"}), Some("rope")),
            event(2, "space-a", "phase_changed", serde_json::json!({"phase": "planned", "reason": "task_created"}), Some("rope")),
            event(3, "space-a", "phase_changed", serde_json::json!({"phase": "review_ready", "digest": "ropey に置換した"}), Some("rope")),
            event(4, "space-a", "tier2", serde_json::json!({"text": "要約"}), Some("rope")),
            event(5, "space-a", "human_send", serde_json::json!({"thread": "rope", "text": "テストも"}), Some("rope")),
            event(6, "proposal-1", "captain_proposed", serde_json::json!([]), None),
            event(7, "proposal-1", "proposal_approved", serde_json::json!({"approved": ["割り算"], "skipped": ["README"]}), None),
            event(8, "captain", "captain", serde_json::json!({"text": "静観"}), None),
        ];
        let lines = captain_wake_lines(&events);
        assert_eq!(lines.len(), 4, "{lines:#?}");
        assert!(lines[0].starts_with("#2 space-a rope:"), "作られた Task は id ごと渡す: {}", lines[0]);
        assert!(lines[1].contains("review_ready") && lines[1].contains("ropey に置換した"));
        assert!(lines[2].contains("テストも"));
        assert!(lines[3].starts_with("#7 proposal-1:") && lines[3].contains("割り算") && lines[3].contains("README"));
    }

    #[test]
    fn wake_lines_keep_the_newest_and_say_how_many_were_left_out() {
        let events: Vec<_> = (1..=45)
            .map(|id| event(id, "space-a", "human_send", serde_json::json!({"text": format!("{id}")}), Some("t")))
            .collect();
        let lines = captain_wake_lines(&events);
        assert_eq!(lines.len(), CAPTAIN_WAKE_MAX_EVENTS + 1);
        assert!(lines[0].contains('5'), "省いた件数を先頭に: {}", lines[0]);
        assert!(lines[1].starts_with("#6 "), "新しい方から 40 件: {}", lines[1]);
        assert!(lines.last().is_some_and(|line| line.starts_with("#45 ")));
    }

    #[test]
    fn the_captain_rotates_on_a_full_context_or_after_many_wakes() {
        assert!(!should_rotate_captain(10_000, 200_000, 0));
        assert!(should_rotate_captain(80_000, 200_000, 0), "40% で交代");
        assert!(should_rotate_captain(0, 200_000, CAPTAIN_ROTATE_WAKES));
        assert!(!should_rotate_captain(900_000, 0, 1), "上限が分からなければ回数だけで決める");
    }
}
