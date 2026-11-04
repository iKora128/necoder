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
    pub(crate) fn render_herd_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let keys_here = self.chrome.threads_focus.is_focused(window);
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
                total_threads: statuses.len(),
                statuses,
                counts: self.slot_attention(index, cx),
                first_row: 0,
                collapsed: false,
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
                    total_threads: statuses.len(),
                    statuses,
                    first_row: 0,
                    collapsed: false,
                });
            }
        }

        let total_threads: usize = groups.iter().map(|group| group.total_threads).sum();
        // 絞り込みで残る行だけにする。行の番号（要素の id）は出す順の通し番号。
        groups.retain_mut(|group| {
            if to_check_only {
                group
                    .statuses
                    .retain(|(_, _, status)| agent_panel::wants_attention(status.activity));
            }
            !group.statuses.is_empty()
        });
        let mut shown = 0usize;
        for group in &mut groups {
            group.first_row = shown;
            group.collapsed =
                self.chrome.fleet_mode && !self.chrome.herd_solo_expanded && group.is_integration;
            if !group.collapsed {
                shown += group.statuses.len();
            }
        }
        let shapes: Vec<HerdGroupShape> = groups
            .iter()
            .map(|group| HerdGroupShape::of(group, self.herd_group_is_current(group)))
            .collect();
        sync_herd_list(
            &self.chrome.herd_list,
            &mut self.chrome.herd_list_shape.borrow_mut(),
            shapes,
        );
        let groups = std::rc::Rc::new(groups);
        let group_count = groups.len();
        // 枠ごとに描く list。画面外の枠は組み立てもレイアウトもしない（スクロールのたびに全行を計算
        // するとカクつく）。Auto は要素の高さを親の flex に任せ、幅の分からない 1 回目の計測を省く。
        let list = gpui::list(
            self.chrome.herd_list.clone(),
            cx.processor(move |this: &mut Self, group_index: usize, _window, cx| {
                let Some(group) = groups.get(group_index) else {
                    return div().into_any_element();
                };
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .px(px(8.))
                    .when(group_index == 0, |item| item.pt(px(2.)))
                    .pb(px(if group_index + 1 == group_count {
                        10.
                    } else {
                        8.
                    }))
                    .child(this.render_herd_group(group, group_index, keys_here, cx))
                    .into_any_element()
            }),
        )
        .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
        .flex_1()
        .min_h_0()
        .w_full();

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
                    i18n::t!("herd.filter_all", "n" => total_threads),
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
            .id("herd-sidebar")
            .track_focus(&self.chrome.threads_focus)
            .on_key_down(cx.listener(Self::on_threads_key_down))
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

    /// 今いるワークスペースの枠か（枠の線を濃くする）。
    fn herd_group_is_current(&self, group: &HerdGroup) -> bool {
        !self.chat_mode() && group.project_index == Some(self.project_sessions.active)
    }

    /// 枠 1 つ: 見出し + スレッドの行。枠 = ワークスペースの境界。
    fn render_herd_group(
        &self,
        group: &HerdGroup,
        group_index: usize,
        keys_here: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = &self.theme;
        let is_current = self.herd_group_is_current(group);
        let mut frame = div()
            .id(("herd-group-frame", group_index))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(px(8.))
            .overflow_hidden()
            .bg(theme.bg1)
            // 今いる枠だけ濃くする（窓縁の枠と同じ色で結ばれる）。
            .map(|frame| {
                if is_current {
                    frame.border(px(1.5)).border_color(group.color)
                } else {
                    frame.border_1().border_color(group.color.alpha(0.45))
                }
            })
            .child(self.render_herd_group_head(group, group_index, is_current, cx));
        if !group.collapsed {
            for (row, (panel, thread_index, status)) in group.statuses.iter().enumerate() {
                frame = frame.child(self.render_herd_row(
                    group.project_index,
                    panel,
                    *thread_index,
                    status,
                    group.first_row + row,
                    keys_here,
                    cx,
                ));
            }
        }
        frame.into_any_element()
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
                element.child(
                    self.attention_badge(group.counts.waiting)
                        .id(("herd-waiting", group_index))
                        .tooltip(Tooltip::text(
                            i18n::t!("herd.waiting_tip", "n" => group.counts.waiting),
                            theme.clone(),
                        )),
                )
            })
            .when(group.counts.unread > 0, |element| {
                element.child(
                    div()
                        .id(("herd-unread", group_index))
                        .tooltip(Tooltip::text(
                            i18n::t!("herd.unread_tip", "n" => group.counts.unread),
                            theme.clone(),
                        ))
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
            // 枠の総数。Chat の枠は確認待ちしか並ばないので出さない。
            .when(project_index.is_some(), |element| {
                element.child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_size(px(10.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!(
                            "herd.group_total",
                            "n" => group.total_threads
                        ))),
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

    /// ↑↓ で巡る行（一覧に見えている順）。Chat の行は押すと Chat モードへ移って一覧が消えるので含めない。
    fn herd_keyboard_rows(&self, cx: &App) -> Vec<(usize, Entity<AgentPanel>, usize)> {
        let mut rows = Vec::new();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            let Some(session) = self.project_sessions.sessions.get(index) else {
                continue;
            };
            if !slot.task_space.is_integration() && slot.task_space.phase == TaskPhase::Archived {
                continue;
            }
            for (panel, thread, status) in session.agent_statuses(cx) {
                if !self.chrome.herd_to_check || agent_panel::wants_attention(status.activity) {
                    rows.push((index, panel, thread));
                }
            }
        }
        rows
    }

    /// スレッド一覧にフォーカスがある間のキー。↑↓ でスレッドを切り替え（ワークスペースをまたぐ）、
    /// ⏎ でそのスレッドの入力欄へ、Escape で作業面へ戻る。素の矢印をここで受ける理由はレールと同じ
    /// （keymap に足すとエディタのキャレットと衝突する・`on_rail_key_down`）。
    pub(crate) fn on_threads_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.modified() {
            return; // ⌃⌘↑↓ や ⌘{ ⌘} を横取りしない
        }
        match event.keystroke.key.as_str() {
            "down" => self.move_herd_selection(1, window, cx),
            "up" => self.move_herd_selection(-1, window, cx),
            "enter" => {
                let panel = self.agent_panel.clone();
                panel.update(cx, |panel, cx| panel.focus_composer(window, cx));
            }
            "escape" => self.focus_session_surface(false, false, window, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn move_herd_selection(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.herd_keyboard_rows(cx);
        if rows.is_empty() {
            return;
        }
        let current = rows.iter().position(|(index, panel, thread)| {
            *index == self.project_sessions.active
                && *panel == self.agent_panel
                && panel.read(cx).active_thread() == *thread
        });
        let count = rows.len() as isize;
        let next = match current {
            Some(position) => (position as isize + step).rem_euclid(count),
            None if step > 0 => 0,
            None => count - 1,
        } as usize;
        let (index, panel, thread) = rows[next].clone();
        self.project_sessions.sessions[index].agent_panel = panel.clone();
        self.switch_project(index, window, cx);
        // 通り過ぎただけでは未確認を消さない（留まったら消える・`focus_thread` は使わない）。
        panel.update(cx, |panel, cx| panel.select_thread(thread, cx));
        self.chrome.show_right = true;
        self.agent_active = true;
        self.reveal_current_herd_row(window, cx);
    }

    /// 選んだ行が見えていなければ、見えるだけ動かす。画面外の枠は描かれず行の位置が分からないので、
    /// 先に枠を見えるところへ出す。行の位置は次の描画で決まるので 2 フレーム待つ。
    fn reveal_current_herd_row(&self, window: &mut Window, cx: &mut Context<Self>) {
        let list = self.chrome.herd_list.clone();
        let row_bounds = self.chrome.herd_current_row_bounds.clone();
        let active = Some(self.project_sessions.active);
        let group_index = self
            .chrome
            .herd_list_shape
            .borrow()
            .iter()
            .position(|shape| shape.project_index == active);
        if let Some(group_index) =
            group_index.filter(|&index| herd_group_is_outside_viewport(&list, index))
        {
            list.scroll_to_reveal_item(group_index);
        }
        let view = cx.entity_id();
        window.on_next_frame(move |window, _cx| {
            window.on_next_frame(move |_window, cx| {
                if scroll_herd_row_into_view(&list, row_bounds.get()) {
                    cx.notify(view);
                }
            });
        });
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
        keys_here: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let current_row_bounds = self.chrome.herd_current_row_bounds.clone();
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
            // キーが一覧に来ている間は選択の面を 1 段上げる（↑↓ が効く合図）。
            .when(is_current_row, |row| {
                row.bg(if keys_here { theme.bg3 } else { theme.bg2 }).child(
                    gpui::canvas(
                        move |bounds, _window, _cx| current_row_bounds.set(bounds),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
            })
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
                    // 続けて ↑↓ で切り替えられるよう、キーは一覧に置く（⏎ で入力欄へ）。
                    let threads = this.chrome.threads_focus.clone();
                    window.focus(&threads, cx);
                    this.release_native_key_focus(cx);
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
    /// 枠のスレッドの総数（絞り込む前）。
    total_threads: usize,
    /// この枠の最初の行の通し番号（行の要素 id）。
    first_row: usize,
    /// 行を畳んで見出しだけ出すか。
    collapsed: bool,
}

/// 枠の高さを決めるもの。変わった枠だけ list に測り直してもらう（画面外の枠は測り直されないので、
/// 伝えないとスクロールの上限がずれる）。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HerdGroupShape {
    project_index: Option<usize>,
    rows: usize,
    sub_lines: usize,
    is_current: bool,
}

impl HerdGroupShape {
    fn of(group: &HerdGroup, is_current: bool) -> Self {
        let (rows, sub_lines) = if group.collapsed {
            (0, 0)
        } else {
            let sub_lines = group
                .statuses
                .iter()
                .map(|(_, _, status)| {
                    usize::from(status.last_prompt.is_some())
                        + usize::from(status.activity != agent_panel::ThreadActivity::Idle)
                })
                .sum();
            (group.statuses.len(), sub_lines)
        };
        Self {
            project_index: group.project_index,
            rows,
            sub_lines,
            is_current,
        }
    }
}

/// list に伝える変更: 測り直す枠（前の並びの添字）と、入れ替わった範囲。
#[derive(Debug, Default, PartialEq)]
struct HerdListChange {
    remeasure: Vec<usize>,
    splice: Option<(std::ops::Range<usize>, usize)>,
}

/// 前後の枠の並びを比べる。全部を作り直すとスクロール位置が飛ぶので、枠が増減した範囲だけを入れ替える。
fn diff_herd_groups(old: &[HerdGroupShape], new: &[HerdGroupShape]) -> HerdListChange {
    let same_group = |a: &HerdGroupShape, b: &HerdGroupShape| a.project_index == b.project_index;
    let prefix = old
        .iter()
        .zip(new)
        .take_while(|(a, b)| same_group(a, b))
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| same_group(a, b))
        .count();
    let remeasure = (0..prefix)
        .filter(|&index| old[index] != new[index])
        .chain((1..=suffix).filter_map(|from_end| {
            (old[old.len() - from_end] != new[new.len() - from_end]).then_some(old.len() - from_end)
        }))
        .collect();
    let old_end = old.len() - suffix;
    let new_count = new.len() - suffix - prefix;
    let splice = (old_end > prefix || new_count > 0).then_some((prefix..old_end, new_count));
    HerdListChange { remeasure, splice }
}

fn sync_herd_list(
    list: &gpui::ListState,
    shapes: &mut Vec<HerdGroupShape>,
    new_shapes: Vec<HerdGroupShape>,
) {
    if *shapes == new_shapes {
        return;
    }
    let change = diff_herd_groups(shapes, &new_shapes);
    for index in change.remeasure {
        list.remeasure_items(index..index + 1);
    }
    if let Some((range, count)) = change.splice {
        list.splice(range, count);
    }
    *shapes = new_shapes;
}

/// 枠が丸ごと画面の外か。まだ測っていない枠も外として扱う。
fn herd_group_is_outside_viewport(list: &gpui::ListState, group_index: usize) -> bool {
    list.item_is_above_viewport(group_index).unwrap_or(true)
        || list.item_is_below_viewport(group_index).unwrap_or(true)
}

/// 行が見えるだけ list を動かす距離（下へ動かすなら正）。見えていれば `None`。
fn herd_row_reveal_distance(
    row: gpui::Bounds<gpui::Pixels>,
    viewport: gpui::Bounds<gpui::Pixels>,
) -> Option<gpui::Pixels> {
    if row.top() < viewport.top() {
        Some(row.top() - viewport.top())
    } else if row.bottom() > viewport.bottom() {
        Some(row.bottom() - viewport.bottom())
    } else {
        None
    }
}

fn scroll_herd_row_into_view(list: &gpui::ListState, row: gpui::Bounds<gpui::Pixels>) -> bool {
    let Some(distance) = herd_row_reveal_distance(row, list.viewport_bounds()) else {
        return false;
    };
    list.scroll_by(distance);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(stroke: &str) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: gpui::Keystroke::parse(stroke).expect("キーを解釈できる"),
            is_held: false,
            prefer_character_input: false,
        }
    }

    fn shape(project_index: usize, rows: usize) -> HerdGroupShape {
        HerdGroupShape {
            project_index: Some(project_index),
            rows,
            sub_lines: 0,
            is_current: false,
        }
    }

    #[test]
    fn group_changes_touch_only_the_groups_that_changed() {
        let old = [shape(0, 1), shape(1, 2), shape(2, 3)];
        assert_eq!(diff_herd_groups(&old, &old), HerdListChange::default());

        // 行が増えた枠だけ測り直す。枠の増減は無いので入れ替えない。
        let grown = [shape(0, 1), shape(1, 5), shape(2, 3)];
        assert_eq!(
            diff_herd_groups(&old, &grown),
            HerdListChange {
                remeasure: vec![1],
                splice: None
            }
        );

        // 真ん中の枠が消えても、前後の枠の測定は残す。
        let removed = [shape(0, 1), shape(2, 4)];
        assert_eq!(
            diff_herd_groups(&old, &removed),
            HerdListChange {
                remeasure: vec![2],
                splice: Some((1..2, 0))
            }
        );

        // 枠が現れる。
        assert_eq!(
            diff_herd_groups(&removed, &[shape(0, 1), shape(1, 2), shape(2, 4)]),
            HerdListChange {
                remeasure: vec![],
                splice: Some((1..1, 1))
            }
        );
        assert_eq!(
            diff_herd_groups(&[], &old),
            HerdListChange {
                remeasure: vec![],
                splice: Some((0..0, 3))
            }
        );
    }

    #[test]
    fn reveal_distance_moves_only_as_far_as_needed() {
        let bounds = |top: f32, height: f32| {
            gpui::Bounds::new(
                gpui::point(px(0.), px(top)),
                gpui::size(px(100.), px(height)),
            )
        };
        let viewport = bounds(100., 200.);
        assert_eq!(herd_row_reveal_distance(bounds(150., 40.), viewport), None);
        assert_eq!(
            herd_row_reveal_distance(bounds(80., 40.), viewport),
            Some(px(-20.))
        );
        assert_eq!(
            herd_row_reveal_distance(bounds(280., 40.), viewport),
            Some(px(20.))
        );
    }

    /// 低い窓に枠をたくさん並べると、画面外の枠は測られない。スクロールすれば末尾まで届き、
    /// ↓ で画面外の枠のスレッドへ移ると、その枠が見えるところへ出てくる。
    #[gpui::test]
    fn only_visible_groups_are_laid_out(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_herd_virtual_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let projects: Vec<_> = (0..12)
            .map(|index| root.join(format!("p{index}")))
            .collect();
        for project in &projects {
            std::fs::create_dir_all(project).expect("作業フォルダを作れる");
        }
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false,"system_notifications":false}"#,
        )
        .expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(projects.clone(), Theme::dark(), None, cx)
        });
        cx.simulate_resize(gpui::size(px(1200.), px(420.)));
        let list = workspace.update_in(cx, |workspace, window, cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
            for index in 0..projects.len() {
                let panel = workspace.project_sessions.sessions[index]
                    .agent_panel
                    .clone();
                panel.update(cx, |panel, cx| {
                    panel.new_thread(cx);
                    panel.new_thread(cx);
                });
            }
            workspace.chrome.show_left = true;
            workspace.chrome.show_herd = true;
            let threads = workspace.chrome.threads_focus.clone();
            window.focus(&threads, cx);
            cx.notify();
            workspace.chrome.herd_list.clone()
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| window.draw(cx).clear(cx));
        };
        draw(cx);
        draw(cx);
        let last = projects.len() - 1;
        assert_eq!(list.item_count(), projects.len());
        assert!(list.bounds_for_item(0).is_some(), "先頭の枠は測られている");
        assert!(list.bounds_for_item(last).is_none(), "画面外の枠は測らない");

        // 末尾までスクロールできる（まだ測っていない枠は描きながら測る）。
        for _ in 0..40 {
            list.scroll_by(px(300.));
            draw(cx);
        }
        let last_group = list.bounds_for_item(last).expect("末尾の枠まで届く");
        assert!(last_group.bottom() <= list.viewport_bounds().bottom() + px(1.));

        // 先頭へ戻ってから、画面外の枠のスレッドへキーで移る。
        list.scroll_to(gpui::ListOffset::default());
        draw(cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.project_sessions.active = 0;
            let rows = workspace.herd_keyboard_rows(cx);
            let (_, panel, _) = rows
                .iter()
                .find(|(index, _, _)| *index == 8)
                .expect("9 つ目のワークスペースの行")
                .clone();
            workspace.project_sessions.sessions[8].agent_panel = panel;
            workspace.switch_project(8, window, cx);
            workspace.reveal_current_herd_row(window, cx);
        });
        draw(cx);
        assert!(
            !herd_group_is_outside_viewport(&list, 8),
            "選んだスレッドの枠が見えるところへ出る"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// 一覧で ↑↓ を押すと、ワークスペースをまたいでスレッドが切り替わり、キーは一覧に残る。
    #[gpui::test]
    fn arrow_keys_walk_threads_across_workspaces(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!("necoder_herd_arrows_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let (project_a, project_b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&project_a).expect("作業フォルダを作れる");
        std::fs::create_dir_all(&project_b).expect("作業フォルダを作れる");
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false,"system_notifications":false}"#,
        )
        .expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project_a, project_b], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
            for index in 0..2 {
                let panel = workspace.project_sessions.sessions[index]
                    .agent_panel
                    .clone();
                panel.update(cx, |panel, cx| {
                    panel.new_thread(cx);
                    panel.new_thread(cx);
                    panel.select_thread(0, cx);
                });
            }
            workspace.chrome.show_herd = true;
            let threads = workspace.chrome.threads_focus.clone();
            window.focus(&threads, cx);

            let position = |workspace: &Workspace, cx: &App| {
                (
                    workspace.project_sessions.active,
                    workspace.agent_panel.read(cx).active_thread(),
                )
            };
            let rows = workspace.herd_keyboard_rows(cx);
            let (first_of_b, _, _) = rows[rows
                .iter()
                .position(|(index, _, _)| *index == 1)
                .expect("2 つ目のワークスペースの行")]
            .clone();
            assert_eq!(first_of_b, 1);
            let threads_in_a = rows.iter().filter(|(index, _, _)| *index == 0).count();

            workspace.on_threads_key_down(&key("down"), window, cx);
            assert_eq!(
                position(workspace, cx),
                (0, 1),
                "同じワークスペースの次のタブへ"
            );
            assert!(
                workspace.chrome.threads_focus.is_focused(window),
                "切り替えてもキーは一覧に残る"
            );

            for _ in 1..threads_in_a {
                workspace.on_threads_key_down(&key("down"), window, cx);
            }
            assert_eq!(
                position(workspace, cx),
                (1, 0),
                "次のワークスペースへまたぐ"
            );
            assert!(workspace.chrome.threads_focus.is_focused(window));

            workspace.on_threads_key_down(&key("up"), window, cx);
            assert_eq!(position(workspace, cx), (0, threads_in_a - 1), "↑ で戻る");

            // 修飾キー付きは横取りしない。
            workspace.on_threads_key_down(&key("ctrl-cmd-down"), window, cx);
            assert_eq!(position(workspace, cx), (0, threads_in_a - 1));

            // ⏎ で入力欄へ移ると、以後の矢印は入力欄のもの。
            workspace.on_threads_key_down(&key("enter"), window, cx);
            assert!(!workspace.chrome.threads_focus.is_focused(window));
            assert!(workspace.agent_panel.read(cx).contains_focus(window, cx));
        });
        std::fs::remove_dir_all(&root).ok();
    }
}
