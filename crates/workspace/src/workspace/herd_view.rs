use crate::workspace::*;

impl Workspace {
    /// Task 見出し / セルヘッダのダブルクリック → 改名（スレッドタブと同じ EditorView・IME 対応・2026-07-24）。
    /// `site` = 入力欄をどこに描くか（herd 見出し / セルヘッダ）。編隊モードで両方が同時に見えても
    /// 同じ入力欄 Entity を二重描画しないための識別。改名は表示名（title）だけを変える。
    pub(crate) fn start_task_rename(
        &mut self,
        project_index: usize,
        site: RenameSite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get(project_index) else {
            return;
        };
        let color = slot.color;
        let title = slot.task_space.title.clone();
        let editor = cx.new(|cx| {
            let mut view = EditorView::plain(self.theme.clone(), color, true, cx);
            view.set_plain_text(title.as_ref(), cx);
            view
        });
        cx.subscribe(&editor, |workspace, _editor, event, cx| match event {
            ComposerEvent::Submit => workspace.confirm_task_rename(cx),
            // 改名入力は固定高（1 行）なので高さ追従は不要。
            ComposerEvent::ContentHeightChanged => {}
        })
        .detach();
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.chrome.task_renaming = Some(TaskRenaming {
            index: project_index,
            site,
            editor,
        });
        cx.notify();
    }

    /// Task 改名の確定（Enter・空白のみは無視）。title と slot 名を揃えて台帳へ永続化。
    fn confirm_task_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.chrome.task_renaming.take() else {
            return;
        };
        let text = renaming.editor.read(cx).plain_text();
        // `:rocket:` は絵文字に（O20・A08）。
        let name = ui::emoji::expand_shortcodes(text.trim());
        if !name.is_empty() {
            if let Some(slot) = self.project_sessions.projects.get_mut(renaming.index) {
                slot.task_space.title = SharedString::from(name.clone());
                slot.name = SharedString::from(name);
            }
            self.persist_task_space(renaming.index, cx);
        }
        cx.notify();
    }

    /// レールのスレッド一覧のアイコン。左カラムを herd と切り替える。Editor では全ワークスペースの
    /// スレッド、Fleet では同じ枠が Fleet サイドバーになる（エクスプローラへ切り替えた後の帰り道を兼ねる）。
    /// Todo / git とは排他で、押し直すとエクスプローラへ戻る（git / Todo のトグルと同じ所作）。
    pub(crate) fn toggle_herd_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.herd_column_visible(cx) {
            self.chrome.show_herd = false;
        } else {
            self.chrome.show_herd = true;
            self.chrome.show_left = true;
            self.todo_panel
                .update(cx, |panel, cx| panel.set_open(false, cx));
            self.git_panel
                .update(cx, |panel, cx| panel.set_open(false, cx));
            self.ensure_fleet_clock(cx); // 行の相対時刻（開始 / 最終入力）を古びさせない
        }
        cx.notify();
    }

    /// 左カラムにいま herd が出ているか。レールの点灯とトグルが同じ判定を読む。左カラムの優先順は
    /// Editor（`render`）が herd 先頭、Fleet（`render_fleet_left_column`）が Todo / git 先頭なので分ける。
    pub(crate) fn herd_column_visible(&self, cx: &App) -> bool {
        self.chrome.show_left
            && self.chrome.show_herd
            && (!self.chrome.fleet_mode
                || (!self.todo_panel.read(cx).open && !self.git_panel_open(cx)))
    }

    /// スレッド一覧本体（UI-SPEC §8「確認待ち」）。全ワークスペースのスレッドを、プロジェクト色の
    /// 枠で分けて出す。並びはレールの順で固定する（確認待ちで箱を動かさない）。Chat は確認待ちが
    /// ある時だけ末尾に灰色の枠で出す。スレッドだけを出す（ファイルはエクスプローラで見る）。
    pub(crate) fn render_herd_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let to_check_only = self.chrome.herd_to_check;
        let totals = self.attention_totals(cx);

        let mut groups: Vec<HerdGroup> = Vec::new();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            let Some(session) = self.project_sessions.sessions.get(index) else {
                continue;
            };
            let statuses = session.agent_statuses(cx);
            if statuses.is_empty() {
                continue;
            }
            // アーカイブ済み Task は一覧から消す（完全削除は見出し右クリックのメニュー）。
            if !slot.task_space.is_integration() && slot.task_space.phase == TaskPhase::Archived {
                continue;
            }
            groups.push(HerdGroup {
                project_index: Some(index),
                name: slot.name.clone(),
                color: slot.color,
                is_integration: slot.task_space.is_integration(),
                // ⎇ ブランチ = この枠が「どの worktree か」の識別。
                branch: slot
                    .branch
                    .clone()
                    .or_else(|| slot.worktree_branch.clone())
                    .map(SharedString::from),
                statuses,
                counts: self.slot_attention(index, cx),
            });
        }
        if let Some(panel) = self.chat_panel() {
            let statuses: Vec<_> = panel
                .read(cx)
                .statuses()
                .into_iter()
                .enumerate()
                .filter(|(_, status)| agent_panel::wants_attention(status.activity))
                .map(|(thread, status)| (panel.clone(), thread, status))
                .collect();
            if !statuses.is_empty() {
                groups.push(HerdGroup {
                    project_index: None,
                    name: SharedString::from(i18n::t!("titlebar.chat")),
                    color: theme.fg2,
                    is_integration: false,
                    branch: None,
                    counts: panel.read(cx).attention_counts(),
                    statuses,
                });
            }
        }

        let mut list = div()
            .id("herd-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(8.))
            .pt(px(2.))
            .pb(px(10.));
        let mut seq = 0usize;
        let mut shown = 0usize;
        for (group_index, group) in groups.iter().enumerate() {
            let rows: Vec<_> = group
                .statuses
                .iter()
                .filter(|(_, _, status)| {
                    !to_check_only || agent_panel::wants_attention(status.activity)
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            let is_current =
                !self.chat_mode() && group.project_index == Some(self.project_sessions.active);
            let mut frame = div()
                .id(("herd-group-frame", group_index))
                .flex_none()
                .flex()
                .flex_col()
                .rounded(px(8.))
                .overflow_hidden()
                .bg(theme.bg1)
                // 枠 = ワークスペースの境界。今いる枠だけ濃くする（窓縁の枠と同じ色で結ばれる）。
                .map(|frame| {
                    if is_current {
                        frame.border(px(1.5)).border_color(group.color)
                    } else {
                        frame.border_1().border_color(group.color.alpha(0.45))
                    }
                })
                .child(self.render_herd_group_head(group, group_index, is_current, cx));
            let collapsed =
                self.chrome.fleet_mode && !self.chrome.herd_solo_expanded && group.is_integration;
            if !collapsed {
                for (panel, thread_index, status) in rows {
                    frame = frame.child(self.render_herd_row(
                        group.project_index,
                        panel,
                        *thread_index,
                        status,
                        seq,
                        cx,
                    ));
                    seq += 1;
                    shown += 1;
                }
            }
            list = list.child(frame);
        }

        let body = if shown == 0 {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px(px(16.))
                .text_size(px(11.))
                .text_color(theme.fg2)
                .child(SharedString::from(if to_check_only {
                    i18n::t!("herd.none_to_check")
                } else {
                    i18n::t!("herd.empty")
                }))
                .into_any_element()
        } else {
            list.into_any_element()
        };

        let chip = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .flex_none()
                .h(px(18.))
                .flex()
                .items_center()
                .px(px(7.))
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(px(10.5))
                .font_weight(FontWeight::NORMAL)
                .text_color(if on { theme.fg0 } else { theme.fg2 })
                .when(on, |chip| chip.bg(theme.bg3))
                .when(!on, |chip| chip.hover(|style| style.text_color(theme.fg1)))
                .child(SharedString::from(label))
        };
        let head = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(36.))
            .pl(px(12.))
            .pr(px(8.))
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.fg0)
            .child(SharedString::from(i18n::t!("herd.title")))
            .child(div().flex_1())
            .child(
                chip(
                    "herd-filter-all",
                    i18n::t!("herd.filter_all"),
                    !to_check_only,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        this.chrome.herd_to_check = false;
                        cx.notify();
                    }),
                ),
            )
            .child(
                chip(
                    "herd-filter-to-check",
                    i18n::t!("herd.filter_to_check", "n" => totals.to_check()),
                    to_check_only,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        this.chrome.herd_to_check = true;
                        cx.notify();
                    }),
                ),
            );

        div()
            .w(px(self.chrome.explorer_width))
            .h_full()
            .flex_none()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .child(head)
            .child(body)
            .child(self.left_dock_resize_handle(cx))
            .into_any_element()
    }

    /// 枠の見出し: 名前 + ⎇ ブランチ + 赤い丸（あなた待ち）+ グレーの丸（未確認）。
    fn render_herd_group_head(
        &self,
        group: &HerdGroup,
        group_index: usize,
        is_current: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let project_index = group.project_index;
        let solo_toggle = self.chrome.fleet_mode && group.is_integration;
        let is_task_group = project_index.is_some() && !group.is_integration;
        let collapsed = solo_toggle && !self.chrome.herd_solo_expanded;
        let renaming_editor = project_index.and_then(|project_index| {
            self.chrome
                .task_renaming
                .as_ref()
                .filter(|renaming| {
                    renaming.index == project_index && renaming.site == RenameSite::Herd
                })
                .map(|renaming| renaming.editor.clone())
        });
        div()
            .id(("herd-group", group_index))
            .group("herd-head")
            .when(solo_toggle, |element| {
                element.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _e: &MouseDownEvent, _window, cx| {
                        this.chrome.herd_solo_expanded = !this.chrome.herd_solo_expanded;
                        cx.notify();
                    }),
                )
            })
            // Task 見出し: ダブルクリック=改名 / 右クリック=メニュー（アーカイブ・worktree/
            // ブランチ削除の完全削除まで。レールから外れた Task の削除入口・2026-07-24）。
            .when_some(
                project_index.filter(|_| is_task_group),
                |element, project_index| {
                    element
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                if event.click_count == 2 {
                                    this.start_task_rename(
                                        project_index,
                                        RenameSite::Herd,
                                        window,
                                        cx,
                                    );
                                }
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                this.open_rail_menu(project_index, event.position, cx);
                            }),
                        )
                },
            )
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .pl(px(10.))
            .pr(px(8.))
            .child(match renaming_editor {
                Some(editor) => div()
                    .flex_1()
                    .min_w_0()
                    .h(px(20.))
                    .child(editor)
                    .into_any_element(),
                None => div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if is_current { theme.fg0 } else { theme.fg1 })
                    .child(group.name.clone())
                    .into_any_element(),
            })
            // ⎇ ブランチ = この枠が「どの worktree か」の識別。
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .when_some(group.branch.clone(), |element, branch| {
                        element.child(SharedString::from(format!("⎇ {branch}")))
                    }),
            )
            .when(group.counts.waiting > 0, |element| {
                element.child(self.attention_badge(group.counts.waiting))
            })
            .when(group.counts.unread > 0, |element| {
                element.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .min_w(px(16.))
                        .h(px(16.))
                        .px(px(4.))
                        .rounded(px(8.))
                        .bg(theme.bg3)
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.fg1)
                        .child(SharedString::from(group.counts.unread.to_string())),
                )
            })
            // worktree を削除 🗑（ホバーで出現）。押すと「失うものを数える」確認ダイアログ
            // （レール右クリック・セルの ⋯ と同じ `request_worktree_delete` へ委譲）。
            .when_some(
                project_index.filter(|_| is_task_group),
                |element, project_index| {
                    element.child(
                        div()
                            .id(("herd-head-trash", project_index))
                            .group("herd-head-trash")
                            .flex_none()
                            .invisible()
                            .group_hover("herd-head", |style| style.visible())
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg2))
                            // svg は親の text_color を継承しないため直接指定 + hover は自グループで赤に。
                            .child(
                                svg()
                                    .path("icons/trash-2.svg")
                                    .size(px(10.))
                                    .text_color(theme.fg2)
                                    .group_hover("herd-head-trash", |style| {
                                        style.text_color(theme.err)
                                    }),
                            )
                            .tooltip(Tooltip::text(
                                i18n::t!("fleet.delete_worktree_tip"),
                                theme.clone(),
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.request_worktree_delete(project_index, false, window, cx);
                                }),
                            ),
                    )
                },
            )
            .when(solo_toggle, |element| {
                element.child(
                    div()
                        .flex_none()
                        .text_size(px(9.))
                        .text_color(theme.fg2)
                        .child(if collapsed { "▸" } else { "⌄" }),
                )
            })
            .into_any_element()
    }

    /// スレッド 1 行: 状態ドット + エージェント + 名前 + 時刻 · トークン / › 頼んだこと / digest。
    /// 確認済みは 2 段に畳む。状態の面・線・動きは `thread_state_layers`。
    fn render_herd_row(
        &self,
        project_index: Option<usize>,
        panel: &Entity<AgentPanel>,
        thread_index: usize,
        status: &agent_panel::AgentStatus,
        seq: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let color = status.color;
        let activity = status.activity;
        let attention = agent_panel::wants_attention(activity);
        let is_current_row = !self.chat_mode()
            && project_index == Some(self.project_sessions.active)
            && *panel == self.agent_panel
            && panel.read(cx).active_thread() == thread_index;
        let latest_time = agent_panel::relative_time_label(
            status.last_input_at_ms.unwrap_or(status.created_at_ms),
        );
        let tokens = if status.tokens_used == 0 {
            SharedString::from("—")
        } else {
            SharedString::from(agent_panel::human_tokens(status.tokens_used))
        };
        let times_tooltip = thread_times_label(status.created_at_ms, status.last_input_at_ms);
        let prompt = status
            .last_prompt
            .as_ref()
            .and_then(|prompt| prompt.lines().next().map(str::to_string));
        // 遷移スナップショット: Blocked=許可待ちの内容 / Done=最後の発言の末尾 / Working=ライブ素材。
        let digest = status
            .digest
            .clone()
            .unwrap_or_else(|| SharedString::from(activity_label(activity)));
        let sub_line = |text: SharedString, text_color: Hsla| {
            div()
                .pl(px(21.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(10.5))
                .text_color(text_color)
                .child(text)
        };
        let close_panel = panel.clone();
        let mute_panel = panel.clone();
        let focus_panel = panel.clone();
        let chat_thread_id = status.thread_id.clone();
        let muted = status.muted;
        let is_chat = project_index.is_none();
        let row_button = |id: (&'static str, usize)| {
            div()
                .id(id)
                .flex_none()
                .size(px(17.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_size(px(13.))
                .text_color(theme.fg2)
                .bg(theme.bg2)
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
        };
        div()
            .id(("herd-row", seq))
            .group("herd-row")
            .relative()
            .overflow_hidden()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(1.))
            .pl(px(10.))
            .pr(px(10.))
            .pt(px(6.))
            .pb(px(7.))
            .border_t_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg2))
            .when(is_current_row, |row| row.bg(theme.bg3))
            .children(agent_panel::thread_state_layers(
                format!("herd-state-{}", status.thread_id),
                &theme,
                color,
                activity,
                status.just_finished,
                agent_panel::StateEdge::Left,
                self.window_active && !cx.reduce_motion(),
            ))
            // 選択の左バー（状態の線が無い行だけ）。
            .when(is_current_row && !attention, |row| {
                row.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .w(px(2.))
                        .bg(color),
                )
            })
            // 開始 / 最終入力の両方はホバーで。
            .tooltip(Tooltip::text(times_tooltip, theme.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(agent_panel::activity_dot(
                        ("herd-dot", seq),
                        8.0,
                        color,
                        activity,
                    ))
                    .child(agent_panel::agent_badge(status.agent.as_ref(), 14.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.))
                            .text_color(if attention { theme.fg0 } else { theme.fg1 })
                            .when(attention, |name| name.font_weight(FontWeight::BOLD))
                            .child(status.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(px(10.))
                            .text_color(theme.fg2)
                            .when(muted, |meta| meta.mr(px(20.)))
                            // ホバー中はここに操作を出す。
                            .group_hover("herd-row", |style| style.invisible())
                            .child(SharedString::from(format!("{latest_time} · {tokens}"))),
                    ),
            )
            .when_some(prompt, |row, prompt| {
                row.child(sub_line(
                    SharedString::from(format!("› {prompt}")),
                    if activity == agent_panel::ThreadActivity::Idle {
                        theme.fg2
                    } else {
                        theme.fg1
                    },
                ))
            })
            // 確認済みは畳む（目を引かない）。
            .when(activity != agent_panel::ThreadActivity::Idle, |row| {
                row.child(sub_line(
                    digest,
                    if activity == agent_panel::ThreadActivity::Blocked {
                        theme.fg0
                    } else {
                        theme.fg2
                    },
                ))
            })
            // ホバーの操作: ミュート（中は常に見せる）/ 閉じる。Chat の行は閉じない。
            .child(
                div()
                    .absolute()
                    .top(px(4.))
                    .right(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        row_button(("herd-mute", seq))
                            .map(|button| {
                                if muted {
                                    button
                                } else {
                                    button
                                        .invisible()
                                        .group_hover("herd-row", |style| style.visible())
                                }
                            })
                            // フラットアイコン。text_color を継承しないため直接指定（GPUI の罠）。
                            .child(
                                svg()
                                    .path(if muted {
                                        "icons/bell-off.svg"
                                    } else {
                                        "icons/bell.svg"
                                    })
                                    .size(px(11.))
                                    .text_color(theme.fg2),
                            )
                            .tooltip(Tooltip::text(
                                i18n::t!(if muted { "herd.unmute" } else { "herd.mute" }),
                                theme.clone(),
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    mute_panel.update(cx, |panel, cx| {
                                        panel.toggle_thread_mute(thread_index, cx)
                                    });
                                    cx.notify();
                                }),
                            ),
                    )
                    .when(!is_chat, |actions| {
                        actions.child(
                            row_button(("herd-del", seq))
                                .invisible()
                                .group_hover("herd-row", |style| style.visible())
                                .child("×")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(
                                        move |this, _event: &MouseDownEvent, _window, cx| {
                                            cx.stop_propagation(); // 行本体へ伝えない
                                            this.close_agent(&close_panel, thread_index, cx);
                                        },
                                    ),
                                ),
                        )
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                    let Some(project_index) = project_index else {
                        this.jump_to_notified_thread(
                            &focus_panel.downgrade(),
                            &chat_thread_id,
                            window,
                            cx,
                        );
                        return;
                    };
                    this.project_sessions.sessions[project_index].agent_panel = focus_panel.clone();
                    // Fleet ならグリッドに出し、Editor ならそのプロジェクト + スレッドへ。
                    this.jump_to_thread(project_index, thread_index, window, cx);
                }),
            )
            .into_any_element()
    }
}

/// スレッド一覧の 1 枠（`project_index` が `None` なら Chat）。
struct HerdGroup {
    project_index: Option<usize>,
    name: SharedString,
    color: Hsla,
    branch: Option<SharedString>,
    statuses: Vec<(Entity<AgentPanel>, usize, agent_panel::AgentStatus)>,
    is_integration: bool,
    counts: agent_panel::AttentionCounts,
}
