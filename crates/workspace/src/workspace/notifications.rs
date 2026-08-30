use crate::workspace::*;

impl Workspace {
    pub(crate) fn on_panel_event(
        &mut self,
        panel: Entity<AgentPanel>,
        event: &agent_panel::PanelEvent,
        cx: &mut Context<Self>,
    ) {
        let session_index = self
            .project_sessions
            .sessions
            .iter()
            .position(|session| session.agent_panel == panel)
            .unwrap_or(self.project_sessions.active);
        match event {
            agent_panel::PanelEvent::TurnStarted { .. } => {
                self.transition_task_space(
                    session_index,
                    TaskPhase::Working,
                    "agent_turn_started",
                    None,
                    cx,
                );
            }
            agent_panel::PanelEvent::OpenHistoryRequest => {
                // window が無いので次の render で消化する（pending_transient_tab と同じ迂回・#5）。
                self.project_sessions.sessions[session_index].pending_open_history = true;
                cx.notify();
            }
            agent_panel::PanelEvent::ToggleFullScreenRequest => {
                // child event の購読には Window が無い。ここで直接レイアウトだけ変えると、
                // 移設される AgentPanel の focus path が孤立して全 Workspace action が死ぬ。
                // Window を持つ effect-cycle の共通処理へ渡す（連続 2 回なら相殺）。
                self.chrome.pending_agent_full_screen_toggle =
                    !self.chrome.pending_agent_full_screen_toggle;
                cx.notify();
            }
            agent_panel::PanelEvent::TurnEnded {
                thread,
                color,
                summary,
                digest,
                muted,
            } => {
                self.project_sessions.sessions[session_index].waiting_thread = None;
                let is_integration_slot = self
                    .project_sessions
                    .projects
                    .get(session_index)
                    .is_some_and(|slot| slot.task_space.is_integration());
                // 監督の采配は coordinator イベントとして監査（P6・ニュースは丸チップ）。
                if is_integration_slot && is_coordinator_thread_name(thread.as_ref()) {
                    self.record_coordinator_decision(*color, digest.as_ref(), summary, cx);
                }
                if let Some(slot) = self.project_sessions.projects.get_mut(session_index) {
                    if !slot.task_space.is_integration() {
                        // 確定値は digest（最後の発言の末尾・P1）。無ければ従来の経過 summary。
                        slot.task_space.result_summary =
                            Some(digest.clone().unwrap_or_else(|| summary.clone()));
                    }
                }
                let remaining = panel
                    .read(cx)
                    .statuses()
                    .into_iter()
                    .map(|status| status.activity)
                    .max_by_key(|activity| activity.urgency());
                let (phase, reason) = match remaining {
                    Some(agent_panel::ThreadActivity::Blocked) => {
                        (TaskPhase::Blocked, "another_agent_blocked")
                    }
                    Some(agent_panel::ThreadActivity::Working) => {
                        (TaskPhase::Working, "another_agent_working")
                    }
                    _ => (TaskPhase::ReviewReady, "all_agent_turns_ended"),
                };
                self.transition_task_space(session_index, phase, reason, digest.as_deref(), cx);
                // 監督 wake（P6・Task の Done 遷移で即時・自分自身=integration は起こさない）。
                if !is_integration_slot {
                    let title = self
                        .project_sessions
                        .projects
                        .get(session_index)
                        .map(|slot| slot.task_space.title.clone())
                        .unwrap_or_else(|| thread.clone());
                    self.wake_coordinator("done", title, digest.clone(), cx);
                }
                if !muted {
                    self.push_toast(
                        SharedString::from(format!("● {thread} — {summary}")),
                        *color,
                        cx,
                    );
                }
                // Todo ボード: そのスレッドの実行中マーカーを解除し、板を読み直す
                // （エージェントが todos.md をチェックしたら watch より先に即反映・M12-10）。
                self.project_sessions.sessions[session_index]
                    .todo_panel
                    .update(cx, |panel, cx| panel.clear_running_color(*color, cx));
                self.reload_todo_board_for(session_index, cx);
            }
            agent_panel::PanelEvent::TurnFailed {
                thread,
                color,
                message,
                muted,
            } => {
                self.project_sessions.sessions[session_index].waiting_thread = None;
                let another_running = panel
                    .read(cx)
                    .statuses()
                    .into_iter()
                    .any(|status| status.activity == agent_panel::ThreadActivity::Working);
                self.transition_task_space(
                    session_index,
                    if another_running {
                        TaskPhase::Working
                    } else {
                        TaskPhase::Failed
                    },
                    "agent_turn_failed",
                    Some(message),
                    cx,
                );
                // 監督 wake（P6・Failed 遷移で即時）。
                let failed_slot = self.project_sessions.projects.get(session_index);
                if failed_slot.is_some_and(|slot| !slot.task_space.is_integration()) {
                    let title = failed_slot
                        .map(|slot| slot.task_space.title.clone())
                        .unwrap_or_else(|| thread.clone());
                    self.wake_coordinator("failed", title, Some(message.clone()), cx);
                }
                if !muted {
                    self.push_toast(
                        SharedString::from(format!("● {thread} — {message}")),
                        *color,
                        cx,
                    );
                }
            }
            agent_panel::PanelEvent::PermissionWaiting {
                thread,
                thread_index,
                color,
                title,
                muted,
            } => {
                self.project_sessions.sessions[session_index].waiting_thread =
                    Some((thread.clone(), *color));
                cx.notify(); // 低頻度の承認待ち時計を root render から開始する（muted でも必要）。
                self.transition_task_space(
                    session_index,
                    TaskPhase::Blocked,
                    "permission_waiting",
                    (!title.is_empty()).then(|| title.as_ref()),
                    cx,
                );
                // 監督 wake（P6・Blocked は 15s 閾値 = すぐ人間が許可したら起こさない）。
                let blocked_slot = self.project_sessions.projects.get(session_index);
                if blocked_slot.is_some_and(|slot| !slot.task_space.is_integration()) {
                    let task_title = blocked_slot
                        .map(|slot| slot.task_space.title.clone())
                        .unwrap_or_else(|| thread.clone());
                    self.wake_coordinator_for_blocked(
                        session_index,
                        task_title,
                        (!title.is_empty()).then(|| title.clone()),
                        cx,
                    );
                }
                if !muted {
                    self.push_toast_linked(
                        SharedString::from(format!(
                            "● {thread} — {}",
                            i18n::t!("agent.waiting_permission")
                        )),
                        *color,
                        Some((session_index, *thread_index)),
                        cx,
                    );
                }
            }
            agent_panel::PanelEvent::ThreadAutoNamed { name } => {
                // AI 命名の引き継ぎ（2026-07-24）: Task 名がプレースホルダ（"Task N"）のままなら
                // 最初のスレッド名を Task 名にする。手動改名済み（プレースホルダでない）は触らない。
                if let Some(slot) = self.project_sessions.projects.get_mut(session_index) {
                    if !slot.task_space.is_integration()
                        && is_placeholder_task_title(&slot.task_space.title)
                    {
                        slot.task_space.title = name.clone();
                        slot.name = name.clone();
                        self.persist_task_space(session_index, cx);
                        cx.notify();
                    }
                }
            }
            agent_panel::PanelEvent::SummaryReady { thread, tier2 } => {
                // Tier 2 要約（P4）を ledger にキャッシュ（再起動後も残る）。状態は運ばない＝上書きしない。
                let _ = thread;
                if let Some(slot) = self.project_sessions.projects.get(session_index) {
                    if !slot.task_space.is_integration() {
                        if let Some(storage) = self.persistence.storage.clone() {
                            let task_id = slot.task_space.id.as_str().to_string();
                            let payload =
                                serde_json::json!({ "tier2": tier2.as_ref() }).to_string();
                            cx.background_executor()
                                .spawn(async move {
                                    if let Err(error) =
                                        storage.append_task_event(&task_id, "tier2", &payload)
                                    {
                                        eprintln!("tier2 要約の記録に失敗: {error:#}");
                                    }
                                })
                                .detach();
                        }
                    }
                }
                // 監督バーの総括（編隊レベル）もこの遷移でデバウンス生成を蹴る。
                self.schedule_control_summary(cx);
                cx.notify();
            }
            agent_panel::PanelEvent::OpenDiffRequest {
                title,
                old_text,
                new_text,
            } => {
                // 提案 diff を transient タブでレビュー（M12-6）。window が要るのでイベントから取得不可 →
                // 承認カードはアクティブ窓でしか押せないため、直近 focus の window handle を使う。
                if let Some(diff_text) = project::unified_diff_texts(old_text, new_text, title) {
                    let mut buffer = Buffer::from_str(&diff_text);
                    buffer.set_read_only(true);
                    self.project_sessions.sessions[session_index].pending_transient_tab = Some((
                        PathBuf::from(i18n::t!("difftab.proposal_title", "title" => title)),
                        buffer,
                    ));
                    cx.notify();
                }
            }
            agent_panel::PanelEvent::FilesTouched { files, color } => {
                for file in files {
                    self.project_sessions.sessions[session_index]
                        .agent_touched
                        .insert(file.clone(), *color);
                    // 開いていれば gutter をスレッド色に（生中継の帰属・M12-3）。
                    if let Some(editor) = self.project_sessions.sessions[session_index]
                        .tabs
                        .iter()
                        .find(|tab| &tab.path == file)
                        .and_then(|tab| tab.editor().cloned())
                    {
                        let color = *color;
                        editor.update(cx, |view, cx| view.set_agent_mark_color(Some(color), cx));
                    }
                }
                cx.notify();
            }
        }
    }

    /// ニュースを積む（管制 P2・新しいものが先頭・上限 100）。
    /// **task_events へ書くのと同じ場所からだけ呼ぶ**（ニュース = 台帳の鏡。将来の監督の采配も同じ道）。
    pub(crate) fn push_news(
        &mut self,
        kind: NewsKind,
        color: Hsla,
        title: SharedString,
        text: SharedString,
    ) {
        let at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        self.notifications.news.insert(
            0,
            NewsItem {
                at_ms,
                color,
                title,
                text,
                kind,
            },
        );
        self.notifications.news.truncate(100);
    }

    /// phase 遷移 1 件をニュース行の文へ写像する（mock の書式: 「承認待ち — 内容」「→ merge_ready — radar ✓」）。
    pub(crate) fn news_text_for_phase(
        phase: TaskPhase,
        digest: Option<&str>,
    ) -> (NewsKind, SharedString) {
        let text = match phase {
            TaskPhase::Blocked => i18n::t!("news.waiting", "title" => digest.unwrap_or("…")),
            TaskPhase::Failed => i18n::t!("news.failed", "detail" => digest.unwrap_or("")),
            _ => match digest {
                Some(digest) => format!("→ {} — {digest}", phase.as_str()),
                None => format!("→ {}", phase.as_str()),
            },
        };
        let kind = match phase {
            TaskPhase::Blocked => NewsKind::Permission,
            TaskPhase::Integrating | TaskPhase::Integrated => NewsKind::Integration,
            TaskPhase::ReviewReady => NewsKind::Digest,
            _ => NewsKind::PhaseChange,
        };
        (kind, SharedString::from(text))
    }

    /// トーストを積む（右下・5 秒で自動で消える・UI-SPEC §8）。
    pub(crate) fn push_toast(&mut self, text: SharedString, color: Hsla, cx: &mut Context<Self>) {
        self.push_toast_linked(text, color, None, cx);
    }

    /// ジャンプ先つきトースト。`link = Some((session_index, thread_index))` ならクリックで
    /// そのプロジェクト＋スレッドへ切り替える（権限待ち通知を押して当該タブへ飛ぶ・#2）。
    pub(crate) fn push_toast_linked(
        &mut self,
        text: SharedString,
        color: Hsla,
        link: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) {
        self.notifications.toast_gen = self.notifications.toast_gen.wrapping_add(1);
        let generation = self.notifications.toast_gen;
        self.notifications
            .toasts
            .push((text, color, generation, link));
        if self.notifications.toasts.len() > 4 {
            self.notifications.toasts.remove(0);
        }
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace
                    .notifications
                    .toasts
                    .retain(|(_, _, gen, _)| *gen != generation);
                cx.notify();
            });
        })
        .detach();
    }

    /// 通知（トースト等）から権限待ちスレッドへ飛ぶ。編隊/通常で herd 行クリックと同じ挙動にする。
    pub(crate) fn jump_to_thread(
        &mut self,
        session_index: usize,
        thread_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if session_index >= self.project_sessions.sessions.len() {
            return;
        }
        if self.chrome.fleet_mode {
            // 編隊モードは Agent ドックを出さない。当該エージェントをグリッドに出して拡大する。
            self.reveal_agent_in_fleet(session_index, thread_index, window, cx);
        } else {
            self.switch_project(session_index, window, cx);
            let panel = self.project_sessions.sessions[session_index]
                .agent_panel
                .clone();
            panel.update(cx, |panel, cx| panel.focus_thread(thread_index, cx));
            self.chrome.show_right = true;
            self.agent_active = true;
            cx.notify();
        }
    }

    /// トースト描画（右下スタック・M12-5）。
    pub(crate) fn render_toasts(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.notifications.toasts.is_empty() {
            return None;
        }
        let theme = self.theme.clone();
        Some(
            div()
                .absolute()
                .bottom(px(38.))
                .right(px(16.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(self.notifications.toasts.iter().map(
                    |(text, color, generation, link)| {
                        let mut toast = div()
                            .id(("toast", *generation as usize))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(12.))
                            .py(px(8.))
                            .bg(theme.bg2)
                            .border_1()
                            .border_color(color.alpha(0.5))
                            .rounded(px(8.))
                            .shadow(vec![gpui::BoxShadow::new(
                                px(0.),
                                px(6.),
                                gpui::hsla(0., 0., 0., 0.4),
                            )
                            .blur_radius(px(16.))])
                            .text_size(px(12.))
                            .text_color(theme.fg0)
                            .child(text.clone());
                        // ジャンプ先つき（権限待ち）はクリックで当該タブへ飛べるようにする（#2）。
                        if let Some((session_index, thread_index)) = *link {
                            toast = toast.cursor_pointer().on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.jump_to_thread(session_index, thread_index, window, cx);
                                }),
                            );
                        }
                        toast
                    },
                ))
                .into_any_element(),
        )
    }

    // ── hot exit（クラッシュ耐性・M10。置き場 = Turso storage crate） ──

    // dirty バッファのスナップショットを（2 秒デバウンスで）DB へ書く。クリーンになった分は消す。
    // 編集の notify 毎に呼ばれるが、世代番号で最後の 1 回だけ実行される。書き込みは背景。
}
