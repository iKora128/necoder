use super::system_notifications::AgentAlert;
use crate::workspace::*;

/// 何を待って止まったか（承認 / 質問）。トーストの文と OS 通知の題が変わるだけで、扱いは同じ（O12）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum WaitingFor {
    Permission,
    Question,
}

impl Workspace {
    pub(crate) fn on_panel_event(
        &mut self,
        panel: Entity<AgentPanel>,
        event: &agent_panel::PanelEvent,
        cx: &mut Context<Self>,
    ) {
        // Chat のパネルはプロジェクトの session ではない（TaskSpace も Captain も無い）。
        if self.chat_panel().as_ref() == Some(&panel) {
            self.on_chat_panel_event(&panel, event, cx);
            return;
        }
        let Some(session_index) = self
            .project_sessions
            .sessions
            .iter()
            .position(|session| session.fleet_agents.contains(&panel))
        else {
            return;
        };
        match event {
            agent_panel::PanelEvent::HumanSend { thread, text } => {
                if is_captain_thread_name(thread.as_ref()) {
                    self.refresh_captain_context(session_index, &panel, thread, cx);
                }
                self.record_human_send(session_index, thread, text, cx);
            }

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
                self.project_sessions.sessions[session_index].agent_panel = panel.clone();
                // window が無いので次の render で消化する（pending_transient_tab と同じ迂回・#5）。
                self.project_sessions.sessions[session_index].pending_open_history = true;
                cx.notify();
            }
            agent_panel::PanelEvent::ToggleFullScreenRequest => {
                if self.chrome.fleet_mode {
                    // Fleet では「全画面」= 舞台を 1 列にしてその Task だけを読む（⤢ と同じ）。
                    if (0..self.chrome.fleet_cells.len())
                        .any(|cell| self.fleet_cell_agent(cell).as_ref() == Some(&panel))
                    {
                        self.chrome.stage_columns = 1;
                        cx.notify();
                        return;
                    }
                }
                // child event の購読には Window が無い。ここで直接レイアウトだけ変えると、
                // 移設される AgentPanel の focus path が孤立して全 Workspace action が死ぬ。
                // Window を持つ effect-cycle の共通処理へ渡す（連続 2 回なら相殺）。
                self.chrome.pending_agent_full_screen_toggle =
                    !self.chrome.pending_agent_full_screen_toggle;
                cx.notify();
            }
            agent_panel::PanelEvent::TurnEnded {
                thread,
                thread_id,
                color,
                summary,
                digest,
                outcome,
                muted,
            } => {
                self.project_sessions.sessions[session_index].waiting_thread = None;
                let is_integration_slot = self
                    .project_sessions
                    .projects
                    .get(session_index)
                    .is_some_and(|slot| slot.task_space.is_integration());
                // Captain の采配は captain イベントとして監査（FLEET-V2 §5.6・ニュースは丸チップ）。
                if is_integration_slot && is_captain_thread_name(thread.as_ref()) {
                    self.record_captain_decision(*color, digest.as_ref(), summary, cx);
                    self.wake_captain(session_index, "flush", thread.clone(), None, cx);
                }
                if let Some(slot) = self.project_sessions.projects.get_mut(session_index) {
                    if !slot.task_space.is_integration() {
                        // 確定値は digest（最後の発言の末尾・P1）。無ければ従来の経過 summary。
                        slot.task_space.result_summary =
                            Some(digest.clone().unwrap_or_else(|| summary.clone()));
                    }
                }
                let remaining = self.project_sessions.sessions[session_index]
                    .fleet_agents
                    .iter()
                    .flat_map(|panel| panel.read(cx).statuses())
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
                // Captain wake（§5.3・Task の Done 遷移で即時・自分自身=integration は起こさない）。
                if !is_integration_slot {
                    let title = self
                        .project_sessions
                        .projects
                        .get(session_index)
                        .map(|slot| slot.task_space.title.clone())
                        .unwrap_or_else(|| thread.clone());
                    self.wake_captain(session_index, "done", title, digest.clone(), cx);
                }
                if !muted {
                    self.push_toast(
                        SharedString::from(format!("● {thread} — {summary}")),
                        *color,
                        cx,
                    );
                }
                // OS 通知（O12）。Captain の完了は自分で起きて采配した結果なので出さない
                // （Captain でも失敗・承認待ち・質問待ちは人の手番なので出す）。
                let captain_done = is_integration_slot
                    && is_captain_thread_name(thread.as_ref())
                    && *outcome == agent_panel::TurnOutcome::Completed;
                if !captain_done {
                    let place = self.notification_place(session_index);
                    let detail = digest.clone().unwrap_or_else(|| summary.clone());
                    self.post_agent_notification(
                        AgentAlert::from_outcome(*outcome),
                        &panel,
                        thread_id,
                        thread,
                        &place,
                        &detail,
                        *muted,
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
                thread_id,
                color,
                message,
                muted,
            } => {
                self.project_sessions.sessions[session_index].waiting_thread = None;
                let another_running = self.project_sessions.sessions[session_index]
                    .fleet_agents
                    .iter()
                    .flat_map(|panel| panel.read(cx).statuses())
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
                // Captain wake（§5.3・Failed 遷移で即時）。
                let failed_slot = self.project_sessions.projects.get(session_index);
                if failed_slot.is_some_and(|slot| !slot.task_space.is_integration()) {
                    let title = failed_slot
                        .map(|slot| slot.task_space.title.clone())
                        .unwrap_or_else(|| thread.clone());
                    self.wake_captain(session_index, "failed", title, Some(message.clone()), cx);
                }
                if !muted {
                    self.push_toast(
                        SharedString::from(format!("● {thread} — {message}")),
                        *color,
                        cx,
                    );
                }
                let place = self.notification_place(session_index);
                self.post_agent_notification(
                    AgentAlert::Failed,
                    &panel,
                    thread_id,
                    thread,
                    &place,
                    message,
                    *muted,
                    cx,
                );
            }
            agent_panel::PanelEvent::PermissionWaiting {
                thread,
                thread_id,
                thread_index,
                color,
                title,
                muted,
            } => self.on_thread_waiting(
                WaitingFor::Permission,
                session_index,
                &panel,
                thread,
                thread_id,
                *thread_index,
                *color,
                title,
                *muted,
                cx,
            ),
            // 質問（Elicitation）も承認待ちと同じ網に載せる（O12・以前は音だけだった）。
            agent_panel::PanelEvent::QuestionWaiting {
                thread,
                thread_id,
                thread_index,
                color,
                message,
                muted,
            } => self.on_thread_waiting(
                WaitingFor::Question,
                session_index,
                &panel,
                thread,
                thread_id,
                *thread_index,
                *color,
                message,
                *muted,
                cx,
            ),
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
                // Captain バーの総括（編隊レベル）もこの遷移でデバウンス生成を蹴る。
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
            agent_panel::PanelEvent::OpenPathRequest { path, line, column } => {
                self.open_transcript_path(session_index, path.clone(), *line, *column, cx);
            }
            // ツールカードの `▣ プレビュー`: 最初からプレビュー表示で開く（source ⇄ は ⌘⇧V）。
            agent_panel::PanelEvent::OpenPreviewRequest { path } => {
                self.project_sessions.sessions[session_index].pending_preview = Some(path.clone());
                cx.notify();
            }
            agent_panel::PanelEvent::ChatRowsChanged => {}
            agent_panel::PanelEvent::OpenUrlRequest { url } => {
                if let Err(error) = crate::crash::open_url(url) {
                    eprintln!("URL を開けない: {error:#}");
                    let color = self
                        .project_sessions
                        .projects
                        .get(session_index)
                        .map(|slot| slot.color)
                        .unwrap_or_else(|| project_color(0));
                    self.push_toast(
                        i18n::t!("link.open_failed", "target" => url.as_ref()).into(),
                        color,
                        cx,
                    );
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

    /// スレッドが人の手番（承認・質問）で止まった: statusbar の待ち表示・Task を Blocked へ・
    /// Captain を起こす予約・トースト（押すとそのスレッドへ）・OS 通知。`detail` = 何の許可か / 質問文。
    #[allow(clippy::too_many_arguments)]
    fn on_thread_waiting(
        &mut self,
        waiting_for: WaitingFor,
        session_index: usize,
        panel: &Entity<AgentPanel>,
        thread: &SharedString,
        thread_id: &SharedString,
        thread_index: usize,
        color: Hsla,
        detail: &SharedString,
        muted: bool,
        cx: &mut Context<Self>,
    ) {
        self.project_sessions.sessions[session_index].waiting_thread =
            Some((thread.clone(), color));
        cx.notify(); // 低頻度の承認待ち時計を root render から開始する（muted でも必要）。
        let reason = match waiting_for {
            WaitingFor::Permission => "permission_waiting",
            WaitingFor::Question => "question_waiting",
        };
        self.transition_task_space(
            session_index,
            TaskPhase::Blocked,
            reason,
            (!detail.is_empty()).then(|| detail.as_ref()),
            cx,
        );
        // Captain wake（§5.3・Blocked は 15s 閾値 = すぐ人間が答えたら起こさない）。
        let blocked_slot = self.project_sessions.projects.get(session_index);
        if blocked_slot.is_some_and(|slot| !slot.task_space.is_integration()) {
            let task_title = blocked_slot
                .map(|slot| slot.task_space.title.clone())
                .unwrap_or_else(|| thread.clone());
            self.wake_captain_for_blocked(
                session_index,
                task_title,
                (!detail.is_empty()).then(|| detail.clone()),
                cx,
            );
        }
        if !muted {
            let waiting = match waiting_for {
                WaitingFor::Permission => i18n::t!("agent.waiting_permission"),
                WaitingFor::Question => i18n::t!("agent.waiting_question"),
            };
            self.push_toast_linked(
                SharedString::from(format!("● {thread} — {waiting}")),
                color,
                (self.project_sessions.sessions[session_index]
                    .fleet_agents
                    .len()
                    == 1)
                    .then_some((session_index, thread_index)),
                cx,
            );
        }
        let alert = match waiting_for {
            WaitingFor::Permission => AgentAlert::Permission,
            WaitingFor::Question => AgentAlert::Question,
        };
        let place = self.notification_place(session_index);
        self.post_agent_notification(alert, panel, thread_id, thread, &place, detail, muted, cx);
    }

    /// OS 通知の本文に出す「どこの」: Task ならその題、それ以外はプロジェクト名。
    fn notification_place(&self, session_index: usize) -> SharedString {
        self.project_sessions
            .projects
            .get(session_index)
            .map(|slot| {
                if slot.task_space.is_integration() {
                    slot.name.clone()
                } else {
                    slot.task_space.title.clone()
                }
            })
            .unwrap_or_default()
    }

    /// ニュースを積む（管制 P2・新しいものが先頭・上限 100）。
    /// **task_events へ書くのと同じ場所からだけ呼ぶ**（ニュース = 台帳の鏡。Captain の采配も同じ道）。
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
    /// transcript のパスリンクを開く（M12-17）。
    ///
    /// necoder が中で見せられるもの（テキスト/画像/PDF/HTML プレビュー）はタブで開き、
    /// フォルダと necoder が描けない形式だけ OS 既定へ回す。ROADMAP の当初案は「`.html` は
    /// ブラウザ」だったが、M14 で HTML プレビュータブが入り**エクスプローラのクリックは中で開く**
    /// ようになったため、同じファイルの開き方が経路で割れないよう中で開く側へ揃えた（2026-09-16）。
    fn open_transcript_path(
        &mut self,
        session_index: usize,
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        let local = self
            .project_sessions
            .projects
            .get(session_index)
            .is_none_or(|slot| !slot.worktree.host().is_remote());
        // リモートのパスをこちら側で stat しても意味が無いので、判定はローカルの時だけ。
        if local && (path.is_dir() || opens_in_default_app(&path)) {
            if let Err(error) = project::open_with_default_app_local(&path) {
                eprintln!("既定アプリで開けない: {error:#}");
                let color = self
                    .project_sessions
                    .projects
                    .get(session_index)
                    .map(|slot| slot.color)
                    .unwrap_or_else(|| project_color(0));
                self.push_toast(
                    i18n::t!("link.open_failed", "target" => path.display().to_string()).into(),
                    color,
                    cx,
                );
            }
            return;
        }
        // ローカルの `.html` を**行指定なしで**指された時は、ソースではなく整形プレビューを出す
        // （「見せたい」意図で貼られるため。行番号つきならその行の source を見たいので下へ流す）。
        // リモートの HTML は webview を持てない（`EditorView::html_preview` が local 限定）。
        if local && line.is_none() && lang::language_for_path(&path) == Some(lang::LanguageId::Html)
        {
            self.project_sessions.sessions[session_index].pending_preview = Some(path);
            cx.notify();
            return;
        }
        // 行/桁は 1 始まりで届く（`:0` は先頭扱い）。
        self.project_sessions.sessions[session_index].pending_navigation = Some((
            path,
            line.unwrap_or(1).saturating_sub(1) as usize,
            column.unwrap_or(1).saturating_sub(1) as usize,
        ));
        cx.notify();
    }

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

/// necoder が中で描けない形式（= クリックしたら OS 既定へ回す）。
/// テキスト・画像・PDF・HTML は中のタブで開けるのでここには入れない。
fn opens_in_default_app(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "zip"
            | "gz"
            | "tgz"
            | "bz2"
            | "xz"
            | "7z"
            | "rar"
            | "dmg"
            | "pkg"
            | "app"
            | "mp4"
            | "mov"
            | "m4v"
            | "avi"
            | "mkv"
            | "mp3"
            | "wav"
            | "aiff"
            | "m4a"
            | "flac"
            | "xlsx"
            | "xls"
            | "docx"
            | "doc"
            | "pptx"
            | "ppt"
            | "sketch"
            | "psd"
            | "ai"
            | "key"
            | "numbers"
            | "pages"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `.html` は「ブラウザ」でも「ソース」でもなく **necoder の整形プレビュー**で開く
    /// （2026-09-16・ユーザー判断）。ただし行番号つきで指された時はその行の source を見たい。
    #[test]
    fn html_links_choose_preview_only_without_a_line_number() {
        let html = Path::new("docs/index.html");
        assert_eq!(
            lang::language_for_path(html),
            Some(lang::LanguageId::Html),
            "HTML の判定は lang に一本化している"
        );
        assert!(!opens_in_default_app(html), "OS 既定へ回さない");
    }

    #[test]
    fn default_app_is_only_for_what_necoder_cannot_draw() {
        // necoder が中で見せられるもの = タブで開く（HTML は M14 のプレビュー、画像・PDF は専用タブ）。
        for inside in [
            "docs/index.html",
            "crates/ui/src/links.rs",
            "assets/logo.png",
            "docs/spec.pdf",
            "README",
            ".gitignore",
        ] {
            assert!(
                !opens_in_default_app(Path::new(inside)),
                "necoder の中で開きたい: {inside}"
            );
        }
        // 中で描けないものだけ OS 既定へ。
        for outside in ["dist/necoder.dmg", "note.xlsx", "clip.MP4", "song.wav"] {
            assert!(
                opens_in_default_app(Path::new(outside)),
                "OS 既定へ回したい: {outside}"
            );
        }
    }
}
