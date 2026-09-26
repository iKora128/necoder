//! 要対応（attention）— Fleet サイドバー常設のキュー（FLEET-V2 §3.2-1・元は管制タブ P3 の ③）。
//!
//! 管制タブ（ヘッダ / Captain バー / 稼働カード / 統合パイプライン）は F3（2026-09-16）で削除し、
//! ここには **キューの導出**（`control_attention_queue`・台帳と memory から毎 render 再導出）、
//! **カード描画**（`render_attention_card`・許可/拒否・Radar・Integrate・確認の**インライン操作**）、
//! ⏎ / ⌘⇧U の先頭没入、Captain バーの ✳ 総括（Tier2・5s デバウンス）だけが残る。
//! 全画面の管制はリモート管制（P9・`mock/fleet-dashboard.html`）の正として別に残す。
//!
//! **規律**（FLEET-ARCHITECTURE 不変条件）: 色=スレッド/Task 識別のみ・状態=形と動き・
//! ✳ テラコッタ=LLM 生成の印（Tier 2 まで出さない）・render 中に Git/DB/Host I/O をしない（全て memory 読み）。

use crate::workspace::*;

/// 要対応キューの 1 項目。優先順は Blocked（経過時間降順）→ Failed → Review 系 → Done 未確認。
struct AttentionItem {
    session_index: usize,
    panel: Entity<AgentPanel>,
    thread_index: usize,
    color: Hsla,
    title: SharedString,
    branch: Option<SharedString>,
    kind: AttentionKind,
}

enum AttentionKind {
    /// 承認待ち（インライン許可/拒否。選択肢ラベルは ACP がエージェントから広告されたものをそのまま使う）。
    Permission(agent_panel::PermissionCard),
    /// 質問待ち（O12）。選択肢はスレッドのカードで選ぶ（複数フィールドにまたがり得る）ので、
    /// ここは質問文を見せて没入させるだけ。
    Question(agent_panel::QuestionCard),
    /// Task が failed（没入して修正指示 / 破棄）。
    Failed {
        digest: Option<SharedString>,
        tier2: Option<SharedString>,
    },
    /// review_ready / changes_requested / merge_ready（Radar・Integrate の人間 gate）。
    Review {
        phase: TaskPhase,
        digest: Option<SharedString>,
        tier2: Option<SharedString>,
    },
    /// スレッドの Done 未確認ラッチ（「確認」で Done→Idle の確認済み遷移・herdr の done/idle 区別）。
    DoneUnread {
        digest: Option<SharedString>,
        tier2: Option<SharedString>,
    },
}

/// ✳ 総括に渡す件数（キュー収集と同じ 1 パスで数える）。
struct ControlStats {
    attention: usize,
    done_unread: usize,
    working: usize,
}

/// ✳ 行（Tier 2・P4）。**✳ テラコッタ = LLM 生成テキストの印**（規律）+ イタリック。
fn tier2_line(tier2: &Option<SharedString>, theme: &Theme) -> Option<gpui::AnyElement> {
    tier2.as_ref().map(|text| {
        div()
            .flex()
            .items_baseline()
            .gap(px(5.))
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(theme_core::claude_bullet())
                    .child("✳"),
            )
            .child(
                div()
                    .flex_1() // min_w_0 は flex_1 とセット（単独だと幅 0 に潰れて消える）
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(10.))
                    .italic()
                    .text_color(theme.fg1)
                    .child(text.clone()),
            )
            .into_any_element()
    })
}

/// 経過秒の短い表示（45s / 12m / 1h05）。
fn elapsed_label(secs: u64) -> SharedString {
    if secs < 60 {
        SharedString::from(format!("{secs}s"))
    } else if secs < 3600 {
        SharedString::from(format!("{}m", secs / 60))
    } else {
        SharedString::from(format!("{}h{:02}", secs / 3600, (secs % 3600) / 60))
    }
}

impl Workspace {
    /// ⏎: 要対応キューの先頭へ没入（keymap "FleetControl" context / Captain バーの「次」ボタン）。
    pub(crate) fn control_next(
        &mut self,
        _: &ControlNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.chrome.fleet_mode {
            return;
        }
        let Some(head) = self.control_attention_queue(cx).into_iter().find(|item| Some(self.project_sessions.projects[item.session_index].repository_key()) == self.active_repository_key()) else {
            return;
        };
        self.project_sessions.sessions[head.session_index].agent_panel = head.panel;
        self.immerse_from_control(head.session_index, head.thread_index, window, cx);
    }

    /// 編隊レベルの ✳ 総括（Tier 2・P4）: キューに影響する遷移から 5s デバウンスして oneshot 生成。
    /// 生成中に次の遷移が来たら世代カウンタで捨てる（最新だけが走る）。失敗時は前の文のまま
    /// （**要約は状態を上書きしない** — 数字とキューは常に事実層から）。
    pub(crate) fn schedule_control_summary(&mut self, cx: &mut Context<Self>) {
        if !settings::get(cx).tier2_summaries || !self.chrome.fleet_mode {
            return;
        }
        let Some(template) = agent_panel::oneshot_template(&settings::get(cx).default_agent) else {
            return; // oneshot 対応外の既定 Agent → 事実文のまま（自然フォールバック）
        };
        let Some(slot) = self
            .project_sessions
            .projects
            .iter()
            .find(|slot| slot.task_space.is_integration())
            .or_else(|| self.project_sessions.projects.first())
        else {
            return;
        };
        let host = slot.worktree.host().clone();
        let cwd = slot.worktree.root().to_path_buf();
        self.control_summary_gen = self.control_summary_gen.wrapping_add(1);
        let generation = self.control_summary_gen;
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            // デバウンス: 5s の間に新しい遷移が来ていたら、この世代は生成しない（次の世代が走る）。
            let facts = workspace.update(cx, |workspace, cx| {
                (workspace.control_summary_gen == generation)
                    .then(|| workspace.control_summary_facts(cx))
            });
            let Ok(Some(input)) = facts else {
                return;
            };
            let generated = cx
                .background_executor()
                .spawn(async move {
                    // 引用符 / $ / バッククォート禁止（sh -c 埋め込み・oneshot_line_on の約束）。
                    let prompt = i18n::t!("control.summary_prompt");
                    project::oneshot_line_on(host.as_ref(), &cwd, &input, template, &prompt, 80)
                })
                .await;
            let Ok(line) = generated else {
                return; // 失敗 → 事実文のまま（UI は欠けない）
            };
            let _ = workspace.update(cx, |workspace, cx| {
                if workspace.control_summary_gen == generation {
                    workspace.control_summary = Some(SharedString::from(line));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 総括 oneshot へ渡す現況（事実層の圧縮・digest の 3 段圧縮の中段。フル transcript は渡さない）。
    fn control_summary_facts(&self, cx: &App) -> String {
        let queue = self.control_attention_queue(cx);
        let stats = self.control_stats(&queue, cx);
        let mut lines = vec![i18n::t!(
            "control.facts_stats",
            "working" => stats.working,
            "attention" => stats.attention,
            "done" => stats.done_unread,
        )];
        for item in queue.iter().take(4) {
            let detail = match &item.kind {
                AttentionKind::Permission(card) => i18n::t!(
                    "control.facts_permission",
                    "elapsed" => elapsed_label(card.waited_secs),
                    "title" => &card.title,
                ),
                AttentionKind::Question(card) => i18n::t!(
                    "control.facts_question",
                    "elapsed" => elapsed_label(card.waited_secs),
                    "message" => &card.message,
                ),
                AttentionKind::Failed { digest, .. } => {
                    i18n::t!("control.facts_failed", "digest" => digest.clone().unwrap_or_default())
                }
                AttentionKind::Review { phase, digest, .. } => {
                    format!("{}: {}", phase.as_str(), digest.clone().unwrap_or_default())
                }
                AttentionKind::DoneUnread { digest, .. } => {
                    i18n::t!("control.facts_done_unread", "digest" => digest.clone().unwrap_or_default())
                }
            };
            let branch = item
                .branch
                .as_ref()
                .map(|branch| format!(" ({branch})"))
                .unwrap_or_default();
            lines.push(format!("- {}{branch}: {detail}", item.title));
        }
        lines.join("\n")
    }

    /// 管制からの没入 = セル面（Graph）へ切替えて該当スレッドのセルを拡大（既存 reveal を再利用）。
    fn immerse_from_control(
        &mut self,
        session_index: usize,
        thread_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_agent_in_fleet(session_index, thread_index, window, cx);
    }

    /// 要対応キューを組む（描画のたびに memory から再導出・状態は持たない＝台帳が記憶の原則）。
    fn control_attention_queue(&self, cx: &App) -> Vec<AttentionItem> {
        let mut blocked: Vec<(u64, AttentionItem)> = Vec::new();
        let mut failed = Vec::new();
        let mut review = Vec::new();
        let mut done = Vec::new();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            let Some(session) = self.project_sessions.sessions.get(index) else {
                continue;
            };
            let statuses = session.agent_statuses(cx);
            let is_task = !slot.task_space.is_integration();
            let card_title: SharedString = if is_task {
                slot.task_space.title.clone()
            } else {
                slot.name.clone()
            };
            let branch = slot
                .branch
                .clone()
                .or_else(|| slot.worktree_branch.clone())
                .map(SharedString::from);
            // phase 由来のカードが立つ Task は、同じ事象の Done ラッチを重ねて出さない。
            let slot_covered = is_task
                && matches!(
                    slot.task_space.phase,
                    TaskPhase::Failed
                        | TaskPhase::ReviewReady
                        | TaskPhase::ChangesRequested
                        | TaskPhase::MergeReady
                );
            for (panel, thread_index, status) in &statuses {
                let thread_index = *thread_index;
                match status.activity {
                    agent_panel::ThreadActivity::Blocked => {
                        // 承認待ちが先（両方来たらスレッドのカードも承認を先に出す）、無ければ質問待ち。
                        let waiting = match panel.read(cx).permission_card(thread_index) {
                            Some(card) => Some((card.waited_secs, AttentionKind::Permission(card))),
                            None => panel
                                .read(cx)
                                .question_card(thread_index)
                                .map(|card| (card.waited_secs, AttentionKind::Question(card))),
                        };
                        if let Some((waited_secs, kind)) = waiting {
                            blocked.push((
                                waited_secs,
                                AttentionItem {
                                    session_index: index,
                                    panel: panel.clone(),
                                    thread_index,
                                    color: status.color,
                                    title: card_title.clone(),
                                    branch: branch.clone(),
                                    kind,
                                },
                            ));
                        }
                    }
                    agent_panel::ThreadActivity::Done { .. } if !slot_covered => {
                        done.push(AttentionItem {
                            session_index: index,
                            panel: panel.clone(),
                            thread_index,
                            color: status.color,
                            title: card_title.clone(),
                            branch: branch.clone(),
                            kind: AttentionKind::DoneUnread {
                                digest: status.digest.clone(),
                                tier2: status.tier2.clone(),
                            },
                        });
                    }
                    _ => {}
                }
            }
            if is_task {
                let representative = statuses
                    .iter()
                    .max_by_key(|(_, _, status)| status.activity.urgency())
                    .map(|(panel, thread_index, status)| {
                        (
                            *thread_index,
                            status.color,
                            status.digest.clone(),
                            status.tier2.clone(),
                            panel.clone(),
                        )
                    })
                    .unwrap_or((0, slot.color, None, None, session.agent_panel.clone()));
                let digest = slot.task_space.result_summary.clone().or(representative.2);
                let tier2 = representative.3;
                match slot.task_space.phase {
                    TaskPhase::Failed => failed.push(AttentionItem {
                        session_index: index,
                        panel: representative.4.clone(),
                        thread_index: representative.0,
                        color: representative.1,
                        title: card_title.clone(),
                        branch: branch.clone(),
                        kind: AttentionKind::Failed { digest, tier2 },
                    }),
                    TaskPhase::ReviewReady
                    | TaskPhase::ChangesRequested
                    | TaskPhase::MergeReady => review.push(AttentionItem {
                        session_index: index,
                        panel: representative.4.clone(),
                        thread_index: representative.0,
                        color: representative.1,
                        title: card_title.clone(),
                        branch: branch.clone(),
                        kind: AttentionKind::Review {
                            phase: slot.task_space.phase,
                            digest,
                            tier2,
                        },
                    }),
                    _ => {}
                }
            }
        }
        // Blocked は待ち時間の長い順（mock: 経過時間順）。
        blocked.sort_by(|a, b| b.0.cmp(&a.0));
        let mut queue: Vec<AttentionItem> = blocked.into_iter().map(|(_, item)| item).collect();
        queue.extend(failed);
        queue.extend(review);
        queue.extend(done);
        queue
    }

    /// titlebar のモード切替に出す**要対応の件数**（`◐ N`・FLEET-V2 §3.0）。キュー全体を組まずに
    /// 数だけ数える（titlebar は毎フレーム描かれるので、カードの生成と clone を持ち込まない）。
    /// 数える対象 = 承認待ち・質問待ち（Blocked）のスレッド + Failed な Task。Dock のバッジ（O12）も
    /// これを全窓で足す（数え方は `dock_badge::attention_count` の 1 か所）。
    pub(crate) fn attention_badge_count(&self, cx: &App) -> usize {
        let mut activities = Vec::new();
        let mut failed_tasks = 0;
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            let Some(session) = self.project_sessions.sessions.get(index) else {
                continue;
            };
            for panel in &session.fleet_agents {
                activities.extend(
                    panel
                        .read(cx)
                        .beacons()
                        .into_iter()
                        .map(|(_, _, activity)| activity),
                );
            }
            if !slot.task_space.is_integration() && slot.task_space.phase == TaskPhase::Failed {
                failed_tasks += 1;
            }
        }
        super::dock_badge::attention_count(activities, failed_tasks)
    }

    /// Task が**質問だけ**で止まっているなら、その質問のスレッド（承認待ちが 1 つでもあれば None
    /// ＝ Task カードの「次へ」は従来どおり「許可」）。「次へ」を「答える」にするのに使う（O12）。
    pub(super) fn question_only_thread(
        &self,
        session_index: usize,
        cx: &App,
    ) -> Option<(Entity<AgentPanel>, usize)> {
        let session = self.project_sessions.sessions.get(session_index)?;
        let mut question = None;
        for panel in &session.fleet_agents {
            let reader = panel.read(cx);
            for (thread, (_, _, activity)) in reader.beacons().into_iter().enumerate() {
                if activity != agent_panel::ThreadActivity::Blocked {
                    continue;
                }
                if reader.permission_card(thread).is_some() {
                    return None;
                }
                if question.is_none() && reader.question_card(thread).is_some() {
                    question = Some((panel.clone(), thread));
                }
            }
        }
        question
    }

    /// ヘッダの数字（キューと同じソースから 1 パス・render 内 memory 読みのみ）。
    fn control_stats(&self, queue: &[AttentionItem], cx: &App) -> ControlStats {
        let mut stats = ControlStats {
            attention: 0,
            done_unread: 0,
            working: 0,
        };
        for item in queue {
            match &item.kind {
                AttentionKind::Permission(_)
                | AttentionKind::Question(_)
                | AttentionKind::Failed { .. } => stats.attention += 1,
                AttentionKind::Review { .. } | AttentionKind::DoneUnread { .. } => {
                    stats.done_unread += 1
                }
            }
        }
        for session in &self.project_sessions.sessions {
            for (_, _, status) in session.agent_statuses(cx) {
                if status.activity == agent_panel::ThreadActivity::Working {
                    stats.working += 1;
                }
            }
        }
        stats
    }

    /// ③ 要対応キュー（左 336px）。カード内のボタンが**その場で**判断を完了させる（没入不要が原則）。
    pub(super) fn render_stage_attention(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let queue: Vec<_> = self.control_attention_queue(cx).into_iter().filter(|item| {
            Some(self.project_sessions.projects[item.session_index].repository_key()) == self.active_repository_key()
        }).collect();
        let mut list = div().id("stage-attention").flex_none().max_h(px(300.)).overflow_y_scroll().flex().flex_col().gap(px(6.)).p(px(8.))
            .child(div().text_size(px(10.)).text_color(self.theme.fg2).child(SharedString::from(format!("{} {}", i18n::t!("control.queue"), queue.len()))));
        for (position, item) in queue.iter().enumerate() { list = list.child(self.render_attention_card(position, item, cx)); }
        list.into_any_element()
    }

    /// キューの 1 カード。ボタン列は種別ごと（許可系はエージェント広告ラベルをそのまま使う）。
    fn render_attention_card(
        &self,
        position: usize,
        item: &AttentionItem,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let session_index = item.session_index;
        let thread_index = item.thread_index;
        let focus_panel = item.panel.clone();
        let urgent = matches!(
            item.kind,
            AttentionKind::Permission(_) | AttentionKind::Question(_)
        );
        let activity = match &item.kind {
            AttentionKind::Permission(_) | AttentionKind::Question(_) => {
                agent_panel::ThreadActivity::Blocked
            }
            AttentionKind::Failed { .. } => agent_panel::ThreadActivity::Done { interrupted: true },
            AttentionKind::Review { .. } | AttentionKind::DoneUnread { .. } => {
                agent_panel::ThreadActivity::Done { interrupted: false }
            }
        };
        // ボタンの共通見た目。primary は先頭アクション（許可 / Integrate / 確認）。
        let button = |id: (&'static str, usize),
                      label: SharedString,
                      primary: bool,
                      theme: &Theme|
         -> gpui::Stateful<gpui::Div> {
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
        let mut card = div()
            .id(("control-card", position))
            .capture_any_mouse_down(cx.listener(move |this, _, _, _| {
                this.project_sessions.sessions[session_index].agent_panel = focus_panel.clone();
            }))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(9.))
            .rounded(px(7.))
            .border_1()
            .border_color(if urgent { theme.err } else { theme.border })
            .bg(theme.bg0)
            .cursor_pointer()
            // カード本体クリック = 没入（ボタンは stop_propagation で上書き）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.immerse_from_control(session_index, thread_index, window, cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(agent_panel::activity_dot(
                        ("control-card-dot", position),
                        9.0,
                        item.color,
                        activity,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(item.title.clone()),
                    )
                    .when_some(item.branch.clone(), |element, branch| {
                        element.child(
                            div()
                                .flex_none()
                                .text_size(px(9.5))
                                .text_color(theme.fg2)
                                .child(SharedString::from(format!("⎇ {branch}"))),
                        )
                    })
                    // 先頭カード = ⏎ の行き先バッジ。
                    .when(position == 0, |element| {
                        element.child(
                            div()
                                .flex_none()
                                .px(px(5.))
                                .py(px(1.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(9.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("control.next_badge"))),
                        )
                    }),
            );
        card = match &item.kind {
            AttentionKind::Permission(permission) => {
                let mut buttons = div().flex().flex_wrap().gap(px(5.));
                for (option_index, _kind, label) in &permission.options {
                    let option_index = *option_index;
                    let panel = item.panel.clone();
                    buttons = buttons.child(
                        button(
                            ("control-perm", position * 8 + option_index),
                            label.clone(),
                            option_index == 0,
                            &theme,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                cx.stop_propagation();
                                panel.update(cx, |panel, cx| {
                                    panel.respond_permission(thread_index, option_index, cx);
                                });
                                // 判断で block は解けた＝台帳も Working へ（turn end が最終確定する）。
                                this.transition_task_space(
                                    session_index,
                                    TaskPhase::Working,
                                    "permission_resolved",
                                    None,
                                    cx,
                                );
                            }),
                        ),
                    );
                }
                let meta = SharedString::from(format!(
                    "{} {} · {}",
                    i18n::t!("control.waiting"),
                    elapsed_label(permission.waited_secs),
                    i18n::t!("control.files", "n" => permission.diff_files)
                ));
                card.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.fg1)
                        .child(SharedString::from(format!("「{}」", permission.title))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(9.5))
                        .text_color(theme.err)
                        .child(meta),
                )
                .child(buttons)
            }
            AttentionKind::Question(question) => card
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.fg1)
                        .child(SharedString::from(format!("「{}」", question.message))),
                )
                .child(
                    div()
                        .text_size(px(9.5))
                        .text_color(theme.err)
                        .child(SharedString::from(format!(
                            "{} {}",
                            i18n::t!("control.waiting"),
                            elapsed_label(question.waited_secs)
                        ))),
                )
                .child(
                    div().flex().gap(px(5.)).child(
                        button(
                            ("control-answer", position),
                            SharedString::from(i18n::t!("control.answer")),
                            true,
                            &theme,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.immerse_from_control(session_index, thread_index, window, cx);
                            }),
                        ),
                    ),
                ),
            AttentionKind::Failed { digest, tier2 } => card
                .when_some(digest.clone(), |element, digest| {
                    element.child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.fg1)
                            .overflow_hidden()
                            .child(digest),
                    )
                })
                .children(tier2_line(tier2, &theme))
                .child(
                    div()
                        .flex()
                        .gap(px(5.))
                        .child(
                            button(
                                ("control-fix", position),
                                SharedString::from(i18n::t!("control.fix_instruct")),
                                true,
                                &theme,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.immerse_from_control(
                                        session_index,
                                        thread_index,
                                        window,
                                        cx,
                                    );
                                }),
                            ),
                        )
                        .child(
                            button(
                                ("control-discard", position),
                                SharedString::from(i18n::t!("control.discard")),
                                false,
                                &theme,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.transition_task_space(
                                        session_index,
                                        TaskPhase::Archived,
                                        "task_discarded",
                                        None,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                ),
            AttentionKind::Review {
                phase,
                digest,
                tier2,
            } => {
                let phase = *phase;
                let space = self.project_sessions.projects[session_index]
                    .task_space
                    .id
                    .clone();
                let space_for_integrate = space.clone();
                card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .px(px(5.))
                                .py(px(1.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(9.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(phase.as_str())),
                        )
                        .when_some(digest.clone(), |element, digest| {
                            element.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(10.5))
                                    .text_color(theme.fg1)
                                    .child(digest),
                            )
                        }),
                )
                .children(tier2_line(tier2, &theme))
                .child(
                    div()
                        .flex()
                        .gap(px(5.))
                        .when(phase == TaskPhase::MergeReady, |element| {
                            element.child(
                                button(
                                    ("control-integrate", position),
                                    SharedString::from(i18n::t!("control.integrate")),
                                    true,
                                    &theme,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.integrate_task(space_for_integrate.clone(), cx);
                                    }),
                                ),
                            )
                        })
                        .when(phase != TaskPhase::MergeReady, |element| {
                            element.child(
                                button(
                                    ("control-radar", position),
                                    SharedString::from(i18n::t!("control.review")),
                                    true,
                                    &theme,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.review_task_for_merge(space.clone(), cx);
                                    }),
                                ),
                            )
                        })
                        .child(
                            button(
                                ("control-open", position),
                                SharedString::from(i18n::t!("control.open")),
                                false,
                                &theme,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.immerse_from_control(
                                        session_index,
                                        thread_index,
                                        window,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                )
            }
            AttentionKind::DoneUnread { digest, tier2 } => {
                let panel = item.panel.clone();
                card.when_some(digest.clone(), |element, digest| {
                    element.child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.fg1)
                            .overflow_hidden()
                            .child(digest),
                    )
                })
                .children(tier2_line(tier2, &theme))
                .child(
                    div()
                        .flex()
                        .gap(px(5.))
                        .child(
                            button(
                                ("control-ack", position),
                                SharedString::from(i18n::t!("control.confirm")),
                                true,
                                &theme,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _, _window, cx| {
                                    cx.stop_propagation();
                                    // Done→Idle の確認済み遷移（P3・herdr の done/idle 区別）。
                                    panel.update(cx, |panel, cx| {
                                        panel.mark_done_seen(thread_index, cx);
                                    });
                                }),
                            ),
                        )
                        .child(
                            button(
                                ("control-open-done", position),
                                SharedString::from(i18n::t!("control.open")),
                                false,
                                &theme,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.immerse_from_control(
                                        session_index,
                                        thread_index,
                                        window,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                )
            }
        };
        card.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 質問（Elicitation）で止まったスレッドは承認待ちと同じ網に載る（O12）: ◐N（= Dock バッジ）・
    /// 要対応の先頭・statusbar の待ち表示・押せるトースト。以前は音だけでどれにも出なかった。
    #[gpui::test]
    fn a_question_is_treated_like_a_permission_wait(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_question_attention_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project.clone()], Theme::dark(), None, cx)
        });
        cx.run_until_parked();
        assert_eq!(
            workspace.read_with(cx, |workspace, cx| workspace.attention_badge_count(cx)),
            0
        );

        workspace.update_in(cx, |workspace, _window, cx| {
            let panel = workspace.project_sessions.sessions[0].agent_panel.clone();
            panel.update(cx, |panel, cx| panel.debug_ask_question(cx));
        });
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, cx| {
            assert_eq!(
                workspace.attention_badge_count(cx),
                1,
                "質問待ちも ◐N と Dock バッジに数える"
            );
            let queue = workspace.control_attention_queue(cx);
            assert!(
                matches!(queue.first().map(|item| &item.kind), Some(AttentionKind::Question(card)) if card.message.as_ref() == "バッファはどちらで実装しますか？"),
                "要対応の先頭に質問のカードが出る"
            );
            assert!(
                workspace.project_sessions.sessions[0].waiting_thread.is_some(),
                "statusbar の待ち表示"
            );
            let waiting = i18n::t!("agent.waiting_question");
            assert!(
                workspace
                    .notifications
                    .toasts
                    .iter()
                    .any(|(text, _, _, link)| text.contains(&waiting) && link.is_some()),
                "押すとスレッドへ飛べるトースト"
            );
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
