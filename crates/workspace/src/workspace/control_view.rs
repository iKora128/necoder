//! 管制（Control）タブ — 編隊統括ダッシュボード（FLEET-CONTROL-PLAN P3・ビジュアルの正は
//! `mock/fleet-dashboard.html`）。編隊中央のタブとして系譜グラフ+グリッドと切替える。
//!
//! 構成（上から）: ① ヘッダ（repo ⎇ main + N worktrees・目標・stat チップ・トークン計）
//! ② 監督バー（集約気分マスコット・総括は P4/P6 まで未任命プレースホルダ・⏎ 次）
//! ③ 要対応キュー（左 336px・許可/拒否・Radar・Integrate・確認の**インライン操作**）
//! ④ 稼働カード（右グリッド・digest + ▰▱ + tok + 経過・クリックで没入）
//! ⑤ 統合パイプライン（TaskPhase 列にタスクチップ）。ニュース（P2）は render_fleet が下段に常設。
//!
//! **規律**（FLEET-ARCHITECTURE 不変条件）: 色=スレッド/Task 識別のみ・状態=形と動き・
//! ✳ テラコッタ=LLM 生成の印（Tier 2 まで出さない）・render 中に Git/DB/Host I/O をしない（全て memory 読み）。

use crate::workspace::*;

/// 要対応キューの 1 項目。優先順は Blocked（経過時間降順）→ Failed → Review 系 → Done 未確認。
struct AttentionItem {
    session_index: usize,
    thread_index: usize,
    color: Hsla,
    title: SharedString,
    branch: Option<SharedString>,
    kind: AttentionKind,
}

enum AttentionKind {
    /// 承認待ち（インライン許可/拒否。選択肢ラベルは ACP がエージェントから広告されたものをそのまま使う）。
    Permission(agent_panel::PermissionCard),
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

/// ヘッダの stat チップとトークン計の素材（キュー収集と同じ 1 パスで数える）。
struct ControlStats {
    attention: usize,
    done_unread: usize,
    working: usize,
    integrated: usize,
    tokens_used: u64,
    tokens_max: u64,
    any_blocked: bool,
    blocked_long: bool,
    any_working: bool,
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
    /// 管制タブを開く（編隊モードごと）。既に管制ならグラフへ戻す（タブトグル）。
    pub(crate) fn toggle_control_center(
        &mut self,
        _: &ToggleControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chrome.fleet_mode && self.chrome.fleet_center_view == FleetCenterView::Control {
            self.chrome.fleet_center_view = FleetCenterView::Graph;
        } else {
            self.chrome.fleet_mode = true;
            self.seed_fleet_cells(cx);
            self.ensure_fleet_clock(cx);
            self.chrome.fleet_center_view = FleetCenterView::Control;
            window.focus(&self.chrome.control_focus, cx); // ⏎（FleetControl context）を効かせる
        }
        cx.notify();
    }

    /// 編隊中央のタブ切替（管制 / グラフ・render_fleet のタブ帯から）。
    pub(crate) fn set_fleet_center_view(
        &mut self,
        view: FleetCenterView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chrome.fleet_center_view = view;
        if view == FleetCenterView::Control {
            window.focus(&self.chrome.control_focus, cx);
        }
        cx.notify();
    }

    /// ⏎: 要対応キューの先頭へ没入（keymap "FleetControl" context / 監督バーの「次」ボタン）。
    pub(crate) fn control_next(
        &mut self,
        _: &ControlNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !(self.chrome.fleet_mode && self.chrome.fleet_center_view == FleetCenterView::Control) {
            return;
        }
        let Some(head) = self.control_attention_queue(cx).into_iter().next() else {
            return;
        };
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
                    let prompt = "入力はマルチエージェント編隊の現況です。監督として状況と次の一手を日本語で1文にまとめて。60文字以内・1行だけ・記号や引用符や前置きなしで出力して。";
                    project::oneshot_line_on(host.as_ref(), &cwd, &input, template, prompt, 80)
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
        let mut lines = vec![format!(
            "稼働 {} · 要対応 {} · 完了未確認 {}",
            stats.working, stats.attention, stats.done_unread
        )];
        for item in queue.iter().take(4) {
            let detail = match &item.kind {
                AttentionKind::Permission(card) => format!(
                    "承認待ち {}: {}",
                    elapsed_label(card.waited_secs),
                    card.title
                ),
                AttentionKind::Failed { digest, .. } => {
                    format!("失敗: {}", digest.clone().unwrap_or_default())
                }
                AttentionKind::Review { phase, digest, .. } => {
                    format!("{}: {}", phase.as_str(), digest.clone().unwrap_or_default())
                }
                AttentionKind::DoneUnread { digest, .. } => {
                    format!("完了未確認: {}", digest.clone().unwrap_or_default())
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
        self.chrome.fleet_center_view = FleetCenterView::Graph;
        self.reveal_agent_in_fleet(session_index, thread_index, window, cx);
    }

    /// 編隊中央のタブ帯（管制 / グラフ・P3）。graph ヘッダの上に載る薄い 1 行。
    pub(crate) fn render_center_tabs(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let current = self.chrome.fleet_center_view;
        let tab = |id: &'static str,
                   label: SharedString,
                   view: FleetCenterView,
                   active: bool,
                   theme: &Theme| {
            div()
                .id(id)
                .flex_none()
                .px(px(10.))
                .py(px(3.))
                .rounded(px(5.))
                .text_size(px(10.5))
                .cursor_pointer()
                .map(|element| {
                    if active {
                        element
                            .bg(theme.bg3)
                            .text_color(theme.fg0)
                            .font_weight(FontWeight::SEMIBOLD)
                    } else {
                        element.text_color(theme.fg2)
                    }
                })
                .hover(|style| style.bg(theme.bg2))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.set_fleet_center_view(view, window, cx);
                    }),
                )
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(30.))
            .px(px(8.))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.bg0)
            .child(tab(
                "center-tab-control",
                SharedString::from(i18n::t!("control.title")),
                FleetCenterView::Control,
                current == FleetCenterView::Control,
                &theme,
            ))
            .child(tab(
                "center-tab-graph",
                SharedString::from(i18n::t!("control.tab_graph")),
                FleetCenterView::Graph,
                current == FleetCenterView::Graph,
                &theme,
            ))
            .into_any_element()
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
            let panel = session.agent_panel.read(cx);
            let statuses = panel.statuses();
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
            for (thread_index, status) in statuses.iter().enumerate() {
                match status.activity {
                    agent_panel::ThreadActivity::Blocked => {
                        if let Some(card) = panel.permission_card(thread_index) {
                            blocked.push((
                                card.waited_secs,
                                AttentionItem {
                                    session_index: index,
                                    thread_index,
                                    color: status.color,
                                    title: card_title.clone(),
                                    branch: branch.clone(),
                                    kind: AttentionKind::Permission(card),
                                },
                            ));
                        }
                    }
                    agent_panel::ThreadActivity::Done { .. } if !slot_covered => {
                        done.push(AttentionItem {
                            session_index: index,
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
                    .enumerate()
                    .max_by_key(|(_, status)| status.activity.urgency())
                    .map(|(thread_index, status)| {
                        (
                            thread_index,
                            status.color,
                            status.digest.clone(),
                            status.tier2.clone(),
                        )
                    })
                    .unwrap_or((0, slot.color, None, None));
                let digest = slot.task_space.result_summary.clone().or(representative.2);
                let tier2 = representative.3;
                match slot.task_space.phase {
                    TaskPhase::Failed => failed.push(AttentionItem {
                        session_index: index,
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

    /// ヘッダの数字（キューと同じソースから 1 パス・render 内 memory 読みのみ）。
    fn control_stats(&self, queue: &[AttentionItem], cx: &App) -> ControlStats {
        let mut stats = ControlStats {
            attention: 0,
            done_unread: 0,
            working: 0,
            integrated: 0,
            tokens_used: 0,
            tokens_max: 0,
            any_blocked: false,
            blocked_long: false,
            any_working: false,
        };
        for item in queue {
            match &item.kind {
                AttentionKind::Permission(card) => {
                    stats.attention += 1;
                    stats.any_blocked = true;
                    stats.blocked_long |= card.waited_secs >= 15; // worry 閾値（既存マスコットと同じ）
                }
                AttentionKind::Failed { .. } => stats.attention += 1,
                AttentionKind::Review { .. } | AttentionKind::DoneUnread { .. } => {
                    stats.done_unread += 1
                }
            }
        }
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if slot.task_space.phase == TaskPhase::Integrated && !slot.task_space.is_integration() {
                stats.integrated += 1;
            }
            let Some(session) = self.project_sessions.sessions.get(index) else {
                continue;
            };
            for status in session.agent_panel.read(cx).statuses() {
                if status.activity == agent_panel::ThreadActivity::Working {
                    stats.working += 1;
                    stats.any_working = true;
                }
                stats.tokens_used += status.tokens_used as u64;
                stats.tokens_max += status.tokens_max as u64;
            }
        }
        stats
    }

    /// 管制タブ本体。render_fleet の中央（タブ帯の下）に入る。
    pub(crate) fn render_control(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let queue = self.control_attention_queue(cx);
        let stats = self.control_stats(&queue, cx);
        div()
            .id("control-root")
            .track_focus(&self.chrome.control_focus)
            .key_context("FleetControl")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme.bg1)
            // どこをクリックしてもフォーカス（⏎ を効かせる）。カード内ボタンは stop_propagation 済み。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.chrome.control_focus, cx);
                }),
            )
            .child(self.render_control_header(&stats, cx))
            .child(self.render_coordinator_bar(&queue, &stats, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(10.))
                    .p(px(10.))
                    .child(self.render_attention_queue(&queue, &stats, cx))
                    .child(self.render_active_grid(cx)),
            )
            .child(self.render_pipeline(cx))
            .into_any_element()
    }

    /// ① ヘッダ: 管制 <repo> ⎇ <branch> + N worktrees ・ 目標 ・ stat チップ ・ Σ トークン。
    fn render_control_header(
        &self,
        stats: &ControlStats,
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let integration = self
            .project_sessions
            .projects
            .iter()
            .find(|slot| slot.task_space.is_integration());
        let repo = integration
            .map(|slot| slot.name.clone())
            .unwrap_or_else(|| SharedString::from("—"));
        let branch = integration
            .and_then(|slot| slot.branch.clone().or_else(|| slot.worktree_branch.clone()))
            .unwrap_or_else(|| "main".to_string());
        let task_count = self
            .project_sessions
            .projects
            .iter()
            .filter(|slot| !slot.task_space.is_integration())
            .count();
        // stat チップ（グリフは状態の形・数字は件数。要対応 > 0 だけ err 色ボーダーで注意を引く）。
        let chip = |glyph: &'static str,
                    count: usize,
                    label: SharedString,
                    urgent: bool|
         -> gpui::AnyElement {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .px(px(8.))
                .py(px(3.))
                .rounded(px(5.))
                .border_1()
                .border_color(if urgent { theme.err } else { theme.border })
                .text_size(px(10.5))
                .text_color(if urgent { theme.fg0 } else { theme.fg1 })
                .child(div().text_color(theme.fg2).child(glyph))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(SharedString::from(count.to_string())),
                )
                .child(div().text_size(px(9.5)).text_color(theme.fg2).child(label))
                .into_any_element()
        };
        let used_k = format!("{:.1}k", stats.tokens_used as f64 / 1000.0);
        let max_label = if stats.tokens_max == 0 {
            "—".to_string()
        } else {
            format!("{}k", stats.tokens_max / 1000)
        };
        let ratio = if stats.tokens_max == 0 {
            0.0
        } else {
            (stats.tokens_used as f32 / stats.tokens_max as f32).clamp(0.0, 1.0)
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(40.))
            .px(px(12.))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.bg0)
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.fg2)
                    .child(i18n::t!("control.title")),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(repo),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(format!("⎇ {branch}"))),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!(
                        "control.worktrees",
                        "n" => task_count
                    ))),
            )
            // 目標文（settings `fleet_goal`・プロジェクト設定でリポジトリごとに持てる＝ファイルが真実）。
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(match settings::get(_cx).fleet_goal.clone() {
                        Some(goal) => SharedString::from(i18n::t!("control.goal", "goal" => goal)),
                        None => SharedString::from(i18n::t!("control.goal_unset")),
                    }),
            )
            .child(chip(
                "◐",
                stats.attention,
                SharedString::from(i18n::t!("control.chip_attention")),
                stats.attention > 0,
            ))
            .child(chip(
                "✓",
                stats.done_unread,
                SharedString::from(i18n::t!("control.chip_done_unread")),
                false,
            ))
            .child(chip(
                "●",
                stats.working,
                SharedString::from(i18n::t!("control.chip_working")),
                false,
            ))
            .child(chip(
                "⇡",
                stats.integrated,
                SharedString::from(i18n::t!("control.chip_integrated")),
                false,
            ))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(10.5))
                    .text_color(theme.fg1)
                    .child(SharedString::from(format!("Σ {used_k}")))
                    .child(
                        div()
                            .w(px(70.))
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(theme.bg3)
                            .child(
                                div()
                                    .w(px(70.0 * ratio))
                                    .h_full()
                                    .rounded(px(2.))
                                    .bg(theme.fg2),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(max_label)),
                    ),
            )
            .into_any_element()
    }

    /// ② 監督バー: 集約気分マスコット + 監督（P6 まで未任命）+ 総括（Tier2 まで事実文）+ ⏎ 次。
    fn render_coordinator_bar(
        &self,
        queue: &[AttentionItem],
        stats: &ControlStats,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        // 総括は P4 の oneshot まで**決定論の事実文**（✳ は LLM 生成の印なのでまだ付けない）。
        let head_label = queue.first().map(|item| {
            let action = match &item.kind {
                AttentionKind::Permission(_) => i18n::t!("control.next_permission"),
                AttentionKind::Failed { .. } => i18n::t!("control.next_failed"),
                AttentionKind::Review { .. } => i18n::t!("control.next_review"),
                AttentionKind::DoneUnread { .. } => i18n::t!("control.next_confirm"),
            };
            SharedString::from(format!("{} — {action}", item.title))
        });
        let summary: SharedString = match queue.first() {
            Some(item) => {
                let waited = match &item.kind {
                    AttentionKind::Permission(card) => {
                        format!(" · {}", elapsed_label(card.waited_secs))
                    }
                    _ => String::new(),
                };
                SharedString::from(i18n::t!(
                    "control.summary_attention",
                    "title" => item.title.clone(),
                    "n" => stats.attention + stats.done_unread,
                    "waited" => waited
                ))
            }
            None => SharedString::from(i18n::t!("control.summary_calm", "n" => stats.working)),
        };
        let latest = self
            .notifications
            .news
            .first()
            .map(|item| agent_panel::relative_time_label(item.at_ms));
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(4.))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.bg0)
            // 集約気分 1 匹（JOURNAL 2026-07-23 の回収）: 編隊の最悪状態に追従。
            .child(agent_panel::fleet_mood_mascot(
                stats.any_blocked,
                stats.blocked_long,
                stats.any_working,
                self.window_active,
                34.0,
                self.visual_tick,
            ))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(i18n::t!("control.coordinator"))),
                    )
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            // 任命済み = エージェント表示名（P6・settings.coordinator_agent）。
                            .child(match settings::get(cx).coordinator_agent.clone() {
                                Some(agent) => SharedString::from(agent),
                                None => SharedString::from(i18n::t!("control.coordinator_none")),
                            }),
                    ),
            )
            // 総括: Tier 2（✳ = LLM 生成の印 + イタリック）があればそれ・無ければ決定論の事実文。
            // どちらでも数字とキューは事実層＝要約は状態を上書きしない（P4）。
            .child(match &self.control_summary {
                Some(tier2) => div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .flex()
                    .items_baseline()
                    .gap(px(5.))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(theme_core::claude_bullet())
                            .child("✳"),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .italic()
                            .text_color(theme.fg1)
                            .child(tier2.clone()),
                    )
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .text_color(theme.fg1)
                    .child(summary)
                    .into_any_element(),
            })
            .when_some(latest, |element, latest| {
                element.child(
                    div()
                        .flex_none()
                        .text_size(px(9.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(
                            i18n::t!("control.updated", "when" => latest),
                        )),
                )
            })
            .when_some(head_label, |element, label| {
                element.child(
                    div()
                        .id("control-next")
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .px(px(9.))
                        .py(px(4.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.bg2)
                        .text_size(px(10.5))
                        .text_color(theme.fg0)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3))
                        .child("⏎")
                        .child(SharedString::from(
                            i18n::t!("control.next", "what" => label),
                        ))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.control_next(&ControlNext, window, cx);
                            }),
                        ),
                )
            })
            .into_any_element()
    }

    /// ③ 要対応キュー（左 336px）。カード内のボタンが**その場で**判断を完了させる（没入不要が原則）。
    fn render_attention_queue(
        &self,
        queue: &[AttentionItem],
        stats: &ControlStats,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let mut list = div()
            .id("control-queue")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.));
        if queue.is_empty() {
            list = list.child(
                div()
                    .p(px(12.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("control.queue_empty"))),
            );
        }
        for (position, item) in queue.iter().enumerate() {
            list = list.child(self.render_attention_card(position, item, cx));
        }
        div()
            .flex_none()
            .w(px(336.))
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_baseline()
                    .gap(px(7.))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("control.queue"))),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(stats.attention.to_string())),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("control.queue_done"))),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(stats.done_unread.to_string())),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("control.enter_hint"))),
                    ),
            )
            .child(list)
            .into_any_element()
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
        let urgent = matches!(item.kind, AttentionKind::Permission(_));
        let activity = match &item.kind {
            AttentionKind::Permission(_) => agent_panel::ThreadActivity::Blocked,
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
                    let panel = self.project_sessions.sessions[session_index]
                        .agent_panel
                        .clone();
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
                let panel = self.project_sessions.sessions[session_index]
                    .agent_panel
                    .clone();
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

    /// ④ 稼働カード（右グリッド）: Working 中の TaskSpace + ゴースト（+ Task / 空きセル）。
    fn render_active_grid(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let mut cards: Vec<gpui::AnyElement> = Vec::new();
        let mut active_count = 0usize;
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if slot.task_space.is_integration() {
                continue;
            }
            let Some(session) = self.project_sessions.sessions.get(index) else {
                continue;
            };
            let statuses = session.agent_panel.read(cx).statuses();
            let Some((thread_index, status)) = statuses
                .iter()
                .enumerate()
                .find(|(_, status)| status.activity == agent_panel::ThreadActivity::Working)
            else {
                continue;
            };
            active_count += 1;
            let branch = slot
                .branch
                .clone()
                .or_else(|| slot.worktree_branch.clone())
                .map(SharedString::from);
            let mut meta: Vec<String> = Vec::new();
            if status.plan_total > 0 {
                meta.push(
                    super::fleet_view::plan_meter(status.plan_done, status.plan_total).to_string(),
                );
            }
            if status.files_touched > 0 {
                meta.push(i18n::t!("control.files", "n" => status.files_touched));
            }
            if status.tokens_used > 0 {
                meta.push(agent_panel::human_tokens(status.tokens_used));
            }
            if let Some(secs) = status.turn_elapsed_secs {
                meta.push(elapsed_label(secs).to_string());
            }
            let session_index = index;
            cards.push(
                div()
                    .id(("control-active", index))
                    .flex_none()
                    .w(px(300.))
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .p(px(9.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg0)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg2))
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
                                ("control-active-dot", index),
                                9.0,
                                status.color,
                                agent_panel::ThreadActivity::Working,
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
                                    .child(slot.task_space.title.clone()),
                            )
                            .when_some(branch, |element, branch| {
                                element.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(9.5))
                                        .text_color(theme.fg2)
                                        .child(SharedString::from(format!("⎇ {branch}"))),
                                )
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(9.5))
                                    .text_color(theme.fg2)
                                    .child(status.agent.clone()),
                            ),
                    )
                    .when_some(status.digest.clone(), |element, digest| {
                        element.child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(10.5))
                                .text_color(theme.fg1)
                                .child(digest),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(meta.join(" · "))),
                    )
                    .into_any_element(),
            );
        }
        // ＋ はスリム 1 行（2026-07-24 デッドスペース圧縮・セルとカードに面積を返す）。
        let free = 8usize.saturating_sub(self.chrome.fleet_cells.len());
        cards.push(
            div()
                .id("control-add-task")
                .flex_none()
                .w(px(300.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .rounded(px(7.))
                .border_1()
                .border_dashed()
                .border_color(theme.border)
                .text_size(px(10.5))
                .text_color(theme.fg2)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                .child(SharedString::from(i18n::t!("fleet.add_agent_simple")))
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(theme.fg2.alpha(0.7))
                        .child(SharedString::from(format!("{free}/8"))),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.add_worktree_agent(cx);
                    }),
                )
                .into_any_element(),
        );
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_baseline()
                    .gap(px(7.))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("control.active"))),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(active_count.to_string())),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("control.immersion_hint"))),
                    ),
            )
            .child(
                div()
                    .id("control-active-grid")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_wrap()
                    .content_start()
                    .gap(px(8.))
                    .children(cards),
            )
            .into_any_element()
    }

    /// ⑤ 統合パイプライン: TaskPhase 列（planned→…→integrated + failed 別枠）にタスクチップ。
    fn render_pipeline(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        const COLUMNS: [TaskPhase; 7] = [
            TaskPhase::Planned,
            TaskPhase::Working,
            TaskPhase::Blocked,
            TaskPhase::ReviewReady,
            TaskPhase::MergeReady,
            TaskPhase::Integrated,
            TaskPhase::Failed,
        ];
        // 表示列への写像（中間 phase は最寄りの列に畳む・Archived は出さない）。
        let column_of = |phase: TaskPhase| -> Option<TaskPhase> {
            Some(match phase {
                TaskPhase::ChangesRequested => TaskPhase::ReviewReady,
                TaskPhase::Integrating => TaskPhase::MergeReady,
                TaskPhase::Archived => return None,
                other => other,
            })
        };
        let mut columns: Vec<(TaskPhase, Vec<(usize, Hsla, SharedString)>)> =
            COLUMNS.iter().map(|phase| (*phase, Vec::new())).collect();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if slot.task_space.is_integration() {
                continue;
            }
            let Some(column) = column_of(slot.task_space.phase) else {
                continue;
            };
            if let Some(entry) = columns.iter_mut().find(|(phase, _)| *phase == column) {
                entry
                    .1
                    .push((index, slot.color, slot.task_space.title.clone()));
            }
        }
        let mut row = div()
            .flex()
            .items_start()
            .gap(px(6.))
            .px(px(12.))
            .py(px(6.));
        let total = columns.len();
        let sessions_hint = self.project_sessions.sessions.len(); // クリック没入用に範囲確認
        for (position, (phase, chips)) in columns.into_iter().enumerate() {
            let mut column = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(5.))
                        .child(
                            div()
                                .text_size(px(9.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.fg2)
                                .child(SharedString::from(phase.as_str().to_uppercase())),
                        )
                        .child(
                            div()
                                .text_size(px(9.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(chips.len().to_string())),
                        ),
                );
            for (session_index, color, title) in chips {
                let clickable = session_index < sessions_hint;
                column = column.child(
                    div()
                        .id(("pipeline-chip", session_index))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .px(px(6.))
                        .py(px(3.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.bg0)
                        .text_size(px(10.))
                        .text_color(theme.fg1)
                        .when(clickable, |element| {
                            element.cursor_pointer().hover(|style| style.bg(theme.bg2))
                        })
                        .child(
                            div()
                                .w(px(2.5))
                                .h(px(12.))
                                .rounded_full()
                                .bg(color)
                                .flex_none(),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(title),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.immerse_from_control(session_index, 0, window, cx);
                            }),
                        ),
                );
            }
            row = row.child(column);
            if position + 1 < total {
                row = row.child(
                    div()
                        .flex_none()
                        .pt(px(2.))
                        .text_size(px(9.))
                        .text_color(theme.fg2.alpha(0.6))
                        .child("→"),
                );
            }
        }
        div()
            .flex_none()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.bg0)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(8.))
                    .px(px(12.))
                    .pt(px(6.))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("control.pipeline"))),
                    )
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(theme.fg2.alpha(0.7))
                            .child(SharedString::from(i18n::t!("control.pipeline_hint"))),
                    ),
            )
            .child(row)
            .into_any_element()
    }
}
